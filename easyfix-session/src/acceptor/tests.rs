use std::assert_matches;
use std::{
    cell::{Cell, RefCell},
    future::{self, Future},
    // Renamed: the bare `io` in this file is `tokio::io`.
    io as std_io,
    net::SocketAddr,
    num::NonZeroUsize,
    pin::pin,
    rc::Rc,
    task::{Context, Waker},
    time::Duration,
};

use easyfix_core::{base_messages::AdminBase, fix_str, version::Version};
use easyfix_test_messages::Message;
use futures_util::FutureExt;
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt, DuplexStream},
    sync::Notify,
    task::{self, JoinHandle, LocalSet},
    time::{self, sleep, timeout},
};

use super::{
    Acceptor, AcceptorError, AcceptorInner, Connection, ConnectionDropReason, ConnectionObserver,
    RegisteredSession, ShutdownMode, TaskGuard, acceptor_session_task,
};
use crate::{
    application::{ApplicationFactory, SessionContext},
    io::time::TimerBackend,
    messages_storage::{InMemoryStorage, MessagesStorage},
    session_id::SessionId,
    settings::{AcceptorSettings, SessionSettings},
    test_helpers::{self, CountedResetMessage, RecordingObserver, nz_seq},
};

#[test]
fn busywait_acceptor_runs_an_identified_session_without_a_tokio_runtime() {
    let acceptor = Acceptor::<Message, InMemoryStorage, _>::with_busywait_timers(
        AcceptorSettings::default(),
        test_helpers::StubAppFactory,
    );
    let id = test_helpers::default_session_id();
    acceptor
        .register_session(
            id.clone(),
            test_helpers::default_session_settings(),
            |_, size| Ok(InMemoryStorage::new(size)),
        )
        .unwrap();
    let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    let task = acceptor.session_task(
        std_io::Cursor::new(test_helpers::serialize_message(&logon)),
        io::sink(),
        test_helpers::TEST_PEER_ADDR,
    );
    assert_eq!(task.now_or_never(), Some(()));
    let storage = acceptor.remove_session(&id).unwrap();
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// A panic in a session task (e.g. user callback) must NOT permanently lose
/// the session's storage. The storage is taken out of the registry for the
/// duration of the task; if the task unwinds without returning it,
/// `reg.storage` stays `None` forever and - because `storage.is_none()`
/// doubles as the "session active" flag - every subsequent reconnect for
/// that `SessionId` is refused as a duplicate, defeating cross-connection
/// seq-num persistence (FIX Session Layer §4.1, Sequence numbers).
///
/// The session task must restore the storage to the registry on panic
/// unwind, leaving the `SessionId` free to reconnect.
#[tokio::test]
async fn session_task_panic_restores_storage_to_registry() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let inner = Rc::new(AcceptorInner::<Message, InMemoryStorage, _>::new(
                AcceptorSettings::default(),
                test_helpers::PanicAppFactory,
                TimerBackend::Tokio,
            ));

            // Register the session the inbound Logon will resolve to. The
            // peer's Logon carries sender=TARGET / target=SENDER, so
            // `SessionId::from_inbound` yields (SENDER, TARGET).
            let session_id =
                register_default_session(&inner, Some(test_helpers::default_storage()));

            // Feed the session task a first (Logon) message; processing it
            // fires `on_admin_msg_in`, which panics.
            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            let (handle, _client_io) =
                spawn_first_message(inner.clone(), &test_helpers::serialize_message(&logon)).await;

            // The task must panic (the callback unwinds it).
            let join = handle.await;
            assert_matches!(
                join,
                Err(e) if e.is_panic(),
                "session task should have panicked in the callback"
            );

            // The storage must have been returned to the registry, so the
            // SessionId is free to reconnect (not bricked as a duplicate).
            let sessions = inner.sessions.borrow();
            let reg = sessions.get(&session_id).expect("session still registered");
            assert!(
                reg.storage.is_some(),
                "storage must be restored to the registry after a panic, \
                 otherwise the SessionId is permanently locked out"
            );
        })
        .await;
}

/// Factory recording the [`SessionContext`] contents it was handed, so a test
/// can assert what the acceptor exposes to application code.
struct RecordingAppFactory {
    seen: Rc<Cell<Option<SocketAddr>>>,
}

impl ApplicationFactory<Message> for RecordingAppFactory {
    type App = test_helpers::PanicApp;

    fn create(&self, ctx: &SessionContext<'_>) -> test_helpers::PanicApp {
        self.seen.set(ctx.peer_addr());
        test_helpers::PanicApp
    }
}

/// The acceptor hands the connection's peer address to the factory, which is
/// what lets an application attribute a failed logon to a source address
/// (e.g. to lock an account out after repeated bad credentials). The address
/// must be available by the time the handler is built - before the inbound
/// `Logon<A>` reaches `on_admin_msg_in`.
#[tokio::test]
async fn factory_receives_peer_addr_in_session_context() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let seen = Rc::new(Cell::new(None));
            let inner = Rc::new(AcceptorInner::<Message, InMemoryStorage, _>::new(
                AcceptorSettings::default(),
                RecordingAppFactory { seen: seen.clone() },
                TimerBackend::Tokio,
            ));
            register_default_session(&inner, Some(test_helpers::default_storage()));

            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            let (handle, _client_io) =
                spawn_first_message(inner, &test_helpers::serialize_message(&logon)).await;
            // `PanicApp` unwinds the task in `on_admin_msg_in`, which happens
            // strictly after `create` - so the join error also pins down the
            // ordering: the address was recorded before the first message was
            // surfaced to the handler.
            let join = handle.await;
            assert_matches!(join, Err(e) if e.is_panic());

            assert_eq!(
                seen.get(),
                Some(test_helpers::TEST_PEER_ADDR),
                "acceptor must expose the connection's peer address to the factory"
            );
        })
        .await;
}

// ---------------------------------------------------------------------------
// Registry management API (synchronous): register / remove / suspend /
// resume / await_session_closed classification. These exercise the registry
// without a live connection, so no session task or runtime driving is needed.
// ---------------------------------------------------------------------------

fn stub_acceptor() -> Acceptor<Message, InMemoryStorage, test_helpers::StubAppFactory> {
    Acceptor::new(test_helpers::StubAppFactory)
}

#[tokio::test]
async fn running_reset_requires_an_active_session_before_message_support() {
    let acceptor =
        Acceptor::<CountedResetMessage, InMemoryStorage, _>::new(test_helpers::StubAppFactory);
    let id = test_helpers::default_session_id();
    assert_matches!(
        acceptor.request_running_session_reset(&id).await,
        Err(AcceptorError::UnknownSession)
    );
    acceptor
        .register_session(id.clone(), SessionSettings::default(), |_, max| {
            Ok(InMemoryStorage::new(max))
        })
        .unwrap();
    assert_matches!(
        acceptor.request_running_session_reset(&id).await,
        Err(AcceptorError::SessionInactive)
    );
    assert!(acceptor.inner.active_sessions.borrow().is_empty());
    assert!(acceptor.inner.sessions.borrow()[&id].storage.is_some());
}

/// Each acceptance permission requires reset-flag support before storage or a
/// registration is created; ordinary sessions can use a message type without it.
#[test]
fn reset_acceptance_requires_message_type_support() {
    for on_connect in [false, true] {
        for in_session in [false, true] {
            let acceptor = Acceptor::<CountedResetMessage, _, _>::new(test_helpers::StubAppFactory);
            let builds = Cell::new(0);
            let id = test_helpers::default_session_id();
            let settings = SessionSettings {
                accept_reset_on_connect: on_connect,
                accept_reset_in_session: in_session,
                ..SessionSettings::default()
            };
            let result = acceptor.register_session(id.clone(), settings.clone(), |_, max| {
                builds.set(builds.get() + 1);
                Ok(InMemoryStorage::new(max))
            });
            if on_connect || in_session {
                assert_matches!(
                    result,
                    Err(AcceptorError::ResetSeqNumFlagNotSupportedInLogon)
                );
                assert_eq!(builds.get(), 0);
                assert!(acceptor.inner.sessions.borrow().is_empty());
            } else {
                result.unwrap();
                assert_eq!(builds.get(), 1);
                assert!(!acceptor.inner.sessions.borrow()[&id].supports_seq_num_reset);
            }
            let supported = stub_acceptor();
            supported
                .register_session(id.clone(), settings, |_, max| Ok(InMemoryStorage::new(max)))
                .unwrap();
            assert!(supported.inner.sessions.borrow()[&id].supports_seq_num_reset);
        }
    }
}

/// A duplicate registration is rejected with `AlreadyRegistered` and, crucially,
/// the second `build_storage` closure is never called (the duplicate check
/// happens before the build).
#[test]
fn register_session_duplicate_is_rejected_without_rebuilding() {
    let acceptor = stub_acceptor();
    let session_id = test_helpers::default_session_id();
    let builds = Cell::new(0u32);

    acceptor
        .register_session(
            session_id.clone(),
            test_helpers::default_session_settings(),
            |_, max| {
                builds.set(builds.get() + 1);
                Ok(InMemoryStorage::new(max))
            },
        )
        .expect("first registration");
    assert_eq!(builds.get(), 1);

    let result = acceptor.register_session(
        session_id,
        test_helpers::default_session_settings(),
        |_, max| {
            builds.set(builds.get() + 1);
            Ok(InMemoryStorage::new(max))
        },
    );
    assert_matches!(result, Err(AcceptorError::AlreadyRegistered));
    assert_eq!(builds.get(), 1, "duplicate path must not run build_storage");
}

/// `remove_session` on an inactive session hands back the owned storage with
/// its sequence counters intact (the basis of the reconfiguration flow), and
/// the entry is gone afterwards.
#[test]
fn remove_session_returns_owned_storage_with_preserved_seq_nums() {
    let acceptor = stub_acceptor();
    let session_id = test_helpers::default_session_id();

    acceptor
        .register_session(
            session_id.clone(),
            test_helpers::default_session_settings(),
            |_, max| {
                let mut storage = InMemoryStorage::new(max);
                storage.set_next_sender_msg_seq_num(nz_seq(42)).unwrap();
                storage.set_next_target_msg_seq_num(nz_seq(7)).unwrap();
                Ok(storage)
            },
        )
        .expect("registration");

    let storage = acceptor.remove_session(&session_id).expect("remove");
    assert_eq!(storage.next_sender_msg_seq_num().get(), 42);
    assert_eq!(storage.next_target_msg_seq_num().get(), 7);
    assert_matches!(
        acceptor.is_session_active(&session_id),
        Err(AcceptorError::UnknownSession),
        "entry must be gone after removal"
    );
}

/// `remove_session` on an unknown id is a clean `UnknownSession` error.
#[test]
fn remove_session_unknown_returns_error() {
    let acceptor = stub_acceptor();
    assert_matches!(
        // `InMemoryStorage` is not `Debug`, so map the `Ok` away before the
        // match (assert_matches formats the value on failure).
        acceptor
            .remove_session(&test_helpers::default_session_id())
            .map(|_| ()),
        Err(AcceptorError::UnknownSession)
    );
}

/// Suspend / resume flip the gate flag without making the session "active"
/// (suspension is orthogonal to having a running task), and target an existing
/// session only.
#[test]
fn suspend_resume_do_not_affect_activeness_and_require_registration() {
    let acceptor = stub_acceptor();
    let session_id = test_helpers::default_session_id();

    assert_matches!(
        acceptor.suspend_session(&session_id),
        Err(AcceptorError::UnknownSession)
    );

    acceptor
        .register_session(
            session_id.clone(),
            test_helpers::default_session_settings(),
            |_, max| Ok(InMemoryStorage::new(max)),
        )
        .expect("registration");

    assert_matches!(acceptor.is_session_active(&session_id), Ok(false));
    acceptor.suspend_session(&session_id).expect("suspend");
    assert_matches!(
        acceptor.is_session_active(&session_id),
        Ok(false),
        "suspension must not report the session as active"
    );
    acceptor.resume_session(&session_id).expect("resume");
    assert_matches!(acceptor.is_session_active(&session_id), Ok(false));

    let other = SessionId::new(
        Version::FIXT11,
        fix_str!("SENDER").to_owned(),
        fix_str!("OTHER").to_owned(),
    );
    assert_matches!(
        acceptor.resume_session(&other),
        Err(AcceptorError::UnknownSession)
    );
}

/// Suspending the acceptor and suspending a session are two independent
/// gates: `suspend` / `resume` on the acceptor neither set nor clear the
/// per-session flag, and each query reports its own flag alone.
#[test]
fn acceptor_suspension_is_independent_of_session_suspension() {
    let acceptor = stub_acceptor();
    let session_id = test_helpers::default_session_id();

    assert!(!acceptor.is_suspended());
    assert_matches!(
        acceptor.is_session_suspended(&session_id),
        Err(AcceptorError::UnknownSession)
    );

    acceptor
        .register_session(
            session_id.clone(),
            test_helpers::default_session_settings(),
            |_, max| Ok(InMemoryStorage::new(max)),
        )
        .expect("registration");
    assert_matches!(acceptor.is_session_suspended(&session_id), Ok(false));

    acceptor.suspend_session(&session_id).expect("suspend");
    assert_matches!(acceptor.is_session_suspended(&session_id), Ok(true));
    assert!(
        !acceptor.is_suspended(),
        "the acceptor flag reports the acceptor only, not a suspended session"
    );

    acceptor.suspend();
    assert!(acceptor.is_suspended());
    acceptor.resume();
    assert!(!acceptor.is_suspended());
    assert_matches!(
        acceptor.is_session_suspended(&session_id),
        Ok(true),
        "resuming the acceptor must not resume a session suspended on its own"
    );

    acceptor.suspend();
    acceptor.resume_session(&session_id).expect("resume");
    assert_matches!(
        acceptor.is_session_suspended(&session_id),
        Ok(false),
        "the session flag reports the session only, not the suspended acceptor"
    );
    assert!(
        acceptor.is_suspended(),
        "resuming a session must not resume the acceptor"
    );
}

/// `await_session_closed` on an unknown id returns `UnknownSession`.
#[tokio::test]
async fn await_session_closed_unknown_returns_error() {
    let acceptor = stub_acceptor();
    assert_matches!(
        acceptor
            .await_session_closed(&test_helpers::default_session_id())
            .await,
        Err(AcceptorError::UnknownSession)
    );
}

/// `await_session_closed` on a registered-but-inactive session resolves
/// immediately (it is already "closed enough" to remove).
#[tokio::test]
async fn await_session_closed_inactive_resolves_immediately() {
    let acceptor = stub_acceptor();
    let session_id = test_helpers::default_session_id();
    acceptor
        .register_session(
            session_id.clone(),
            test_helpers::default_session_settings(),
            |_, max| Ok(InMemoryStorage::new(max)),
        )
        .expect("registration");

    timeout(
        Duration::from_secs(1),
        acceptor.await_session_closed(&session_id),
    )
    .await
    .expect("inactive session must resolve immediately")
    .expect("registered session");
}

/// A cloned `Acceptor` is a second handle to the same shared state: a
/// registration through one is visible (and mutable) through the other.
#[test]
fn clone_shares_session_registry() {
    let acceptor = stub_acceptor();
    let clone = acceptor.clone();
    let session_id = test_helpers::default_session_id();

    acceptor
        .register_session(
            session_id.clone(),
            test_helpers::default_session_settings(),
            |_, max| Ok(InMemoryStorage::new(max)),
        )
        .expect("registration");

    // The clone sees the registration and can operate on it...
    assert_matches!(clone.is_session_active(&session_id), Ok(false));
    let _storage = clone.remove_session(&session_id).expect("remove via clone");

    // ...and the removal is visible back through the original handle.
    assert_matches!(
        acceptor.is_session_active(&session_id),
        Err(AcceptorError::UnknownSession)
    );
}

// ---------------------------------------------------------------------------
// First-message decode failures (Test Cases Scenario 1S / 2S): what goes
// out on the wire before any session is established.
// ---------------------------------------------------------------------------

fn stub_inner() -> Rc<AcceptorInner<Message, InMemoryStorage, test_helpers::StubAppFactory>> {
    Rc::new(AcceptorInner::new(
        AcceptorSettings::default(),
        test_helpers::StubAppFactory,
        TimerBackend::Tokio,
    ))
}

/// Register `default_session_id()` in `inner`. `storage: None` simulates
/// a session whose storage is already taken by an active connection.
fn register_default_session<A: ApplicationFactory<Message>>(
    inner: &Rc<AcceptorInner<Message, InMemoryStorage, A>>,
    storage: Option<InMemoryStorage>,
) -> SessionId {
    let session_id = test_helpers::default_session_id();
    inner.sessions.borrow_mut().insert(
        session_id.clone(),
        RegisteredSession {
            session_settings: test_helpers::default_session_settings(),
            supports_seq_num_reset: super::supports_seq_num_reset::<Message>(
                &test_helpers::default_session_settings(),
            ),
            storage,
            closed: Rc::new(Notify::new()),
            suspended: false,
        },
    );
    session_id
}

/// Spawn a fresh acceptor session task over a duplex connection on which
/// the peer has already written `first_bytes`. Returns the task's handle and
/// the peer's end of the connection, for tests that need the join outcome
/// itself (a panic) or the wire before the task finishes;
/// [`run_first_message`] wraps it for the common case.
async fn spawn_first_message<A: ApplicationFactory<Message> + 'static>(
    inner: Rc<AcceptorInner<Message, InMemoryStorage, A>>,
    first_bytes: &[u8],
) -> (JoinHandle<()>, DuplexStream) {
    let (server_io, mut client_io) = io::duplex(8192);
    let (server_reader, server_writer) = io::split(server_io);
    client_io.write_all(first_bytes).await.expect("write");

    let guard = TaskGuard::new(inner.clone());
    let handle = task::spawn_local(acceptor_session_task(
        inner,
        server_reader,
        server_writer,
        test_helpers::TEST_PEER_ADDR,
        guard,
    ));
    (handle, client_io)
}

/// Feed `first_bytes` to a fresh acceptor session task over a duplex
/// connection, wait for the task to finish, and return everything the
/// acceptor wrote to the wire.
async fn run_first_message<A: ApplicationFactory<Message> + 'static>(
    inner: Rc<AcceptorInner<Message, InMemoryStorage, A>>,
    first_bytes: &[u8],
) -> Vec<u8> {
    let (handle, mut client_io) = spawn_first_message(inner, first_bytes).await;
    // Bounded: a task that stops closing its connection would otherwise
    // hang every test built on this helper instead of failing it.
    timeout(Duration::from_secs(5), handle)
        .await
        .expect("session task must finish")
        .expect("session task");

    let mut written = Vec::new();
    timeout(Duration::from_secs(5), client_io.read_to_end(&mut written))
        .await
        .expect("the connection must be closed")
        .expect("read");
    written
}

/// Scenario 1S(d): a well-framed but undecodable first Logon from a
/// registered peer is answered on the wire - Reject(35=3), then
/// Logout(35=5) with Text(58) - and the connection is closed. The
/// sequence numbers consumed by the answer are persisted back into the
/// registry.
///
/// The spec makes the Reject optional; this engine always sends it, and
/// the test deliberately pins that choice (first frame is the Reject,
/// NextNumOut advances by exactly two), not just the mandatory Logout.
#[tokio::test]
async fn invalid_first_logon_from_registered_peer_is_answered_per_1sd() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let inner = stub_inner();
            let session_id =
                register_default_session(&inner, Some(test_helpers::default_storage()));

            let written =
                run_first_message(inner.clone(), &test_helpers::invalid_logon_bytes()).await;

            // Parse what went out: Reject, then Logout, then nothing.
            let mut reader = &written[..];
            let mut buf = Vec::new();
            let reject = test_helpers::read_one_message(&mut reader, &mut buf).await;
            assert_matches!(
                test_helpers::as_admin(&reject),
                AdminBase::Reject(reject) if reject.ref_seq_num == 1
            );
            let logout = test_helpers::read_one_message(&mut reader, &mut buf).await;
            assert_matches!(
                test_helpers::as_admin(&logout),
                AdminBase::Logout(logout) if logout.text.is_some(),
                "Logout must carry Text(58) referencing the error condition"
            );
            assert!(
                reader.is_empty() && buf.is_empty(),
                "no output after Logout"
            );

            // The storage returned to the registry with the consumed
            // outbound seq nums (Reject + Logout) and the advanced
            // NextNumIn (the rejected in-sequence Logon).
            let sessions = inner.sessions.borrow();
            let reg = sessions.get(&session_id).expect("session registered");
            let storage = reg.storage.as_ref().expect("storage returned");
            assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        })
        .await;
}

/// Scenario 1S(c): an invalid Logon whose recovered identity matches no
/// registered session is dropped without sending anything - and reported
/// with that identity, so the silence is the deliberate one.
#[tokio::test]
async fn invalid_first_logon_from_unknown_peer_is_dropped_silently() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            // No session registered.
            let written = run_first_message(inner, &test_helpers::invalid_logon_bytes()).await;
            assert!(
                written.is_empty(),
                "1S(c): nothing may be sent to an unauthenticated peer"
            );
            let expected = format!("unknown:{}", test_helpers::default_session_id());
            assert_eq!(observer.summaries(), vec![expected]);
        })
        .await;
}

/// Scenario 1S(b): an invalid Logon for a session whose connection is
/// already active (storage taken) is dropped without sending anything, and
/// reported as the duplicate it is - not as an unknown identity.
#[tokio::test]
async fn invalid_first_logon_on_duplicate_connection_is_dropped_silently() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            register_default_session(&inner, None);
            let written = run_first_message(inner, &test_helpers::invalid_logon_bytes()).await;
            assert!(
                written.is_empty(),
                "1S(b): nothing may be sent on a duplicate connection"
            );
            let expected = format!("active:{}", test_helpers::default_session_id());
            assert_eq!(observer.summaries(), vec![expected]);
        })
        .await;
}

/// Scenario 2(d)/2S: a garbled first message (framing failure - it cannot
/// even be identified as a Logon) is dropped without sending anything, and
/// reported without an identity, since none could be recovered.
#[tokio::test]
async fn garbled_first_message_is_dropped_silently() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            register_default_session(&inner, Some(test_helpers::default_storage()));
            let written = run_first_message(inner, b"this is not a FIX message").await;
            assert!(written.is_empty(), "garbled input must be ignored");
            assert_eq!(observer.summaries(), vec!["undecodable".to_owned()]);
        })
        .await;
}

/// A first message that fails decoding but is NOT a Logon falls outside
/// Scenario 1S(d) - silent drop, reported without an identity: only an
/// invalid Logon has one recovered.
#[tokio::test]
async fn invalid_non_logon_first_message_is_dropped_silently() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            register_default_session(&inner, Some(test_helpers::default_storage()));
            // Well-framed NewOrderSingle missing all required body tags.
            let bytes = test_helpers::frame_message(
                "FIXT.1.1",
                "35=D|49=TARGET|56=SENDER|34=1|52=20260721-08:00:00.000|",
            );
            let written = run_first_message(inner, &bytes).await;
            assert!(written.is_empty(), "only an invalid Logon is answered");
            assert_eq!(observer.summaries(), vec!["undecodable".to_owned()]);
        })
        .await;
}

// ---------------------------------------------------------------------------
// ConnectionObserver: connections that die before becoming sessions
// ---------------------------------------------------------------------------

/// A `Logon<A>` naming an unregistered identity is reported with that
/// identity - the signal a policy needs to tell CompID enumeration (many
/// distinct unknown ids from one address) from a misconfigured counterparty
/// (one id, repeated).
#[tokio::test]
async fn unknown_session_is_reported_with_identity() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());

            // Nothing registered, so the Logon resolves to an unknown id.
            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            let bytes = test_helpers::serialize_message(&logon);
            let written = run_first_message(inner, &bytes).await;

            assert!(
                written.is_empty(),
                "an unregistered identity must be dropped silently \
                 (FIX Session Layer 4.6.4)"
            );
            let expected = format!("unknown:{}", test_helpers::default_session_id());
            assert_eq!(observer.summaries(), vec![expected]);
            assert_eq!(
                observer.peer_addrs(),
                vec![test_helpers::TEST_PEER_ADDR],
                "the peer address must accompany the identity"
            );
        })
        .await;
}

/// A suspended session refuses the connection the way an unknown or duplicate
/// one is refused - nothing on the wire, storage untouched - and the observer
/// is told it was the suspension, with the identity, so an operator can tell
/// a gated reconfiguration from a stray peer.
#[tokio::test]
async fn suspended_session_is_reported_with_identity() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            let session_id =
                register_default_session(&inner, Some(test_helpers::default_storage()));
            inner
                .sessions
                .borrow_mut()
                .get_mut(&session_id)
                .expect("registered")
                .suspended = true;

            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            let bytes = test_helpers::serialize_message(&logon);
            let written = run_first_message(inner.clone(), &bytes).await;

            assert!(written.is_empty(), "a suspended session answers nothing");
            assert_eq!(
                observer.summaries(),
                vec![format!("suspended:{session_id}")]
            );
            let sessions = inner.sessions.borrow();
            let reg = sessions.get(&session_id).expect("still registered");
            assert!(
                reg.storage.is_some(),
                "the refused connection must not have taken the storage"
            );
            assert_eq!(
                reg.storage
                    .as_ref()
                    .expect("storage present")
                    .next_target_msg_seq_num()
                    .get(),
                1,
                "the refused Logon must not have been processed"
            );
        })
        .await;
}

/// A suspended acceptor drops a connection before reading anything from it:
/// nothing on the wire, the session the Logon names is never looked up (its
/// storage stays put), and the observer sees `AcceptorSuspended` without an
/// identity. Once resumed, the same connection is read again - here far
/// enough to resolve and report its identity.
#[tokio::test]
async fn suspended_acceptor_drops_connections_before_reading_them() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            let session_id =
                register_default_session(&inner, Some(test_helpers::default_storage()));
            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            let bytes = test_helpers::serialize_message(&logon);

            inner.suspended.set(true);
            let written = run_first_message(inner.clone(), &bytes).await;

            assert!(written.is_empty(), "a suspended acceptor answers nothing");
            assert_eq!(observer.summaries(), vec!["acceptor_suspended".to_owned()]);
            assert_eq!(observer.peer_addrs(), vec![test_helpers::TEST_PEER_ADDR]);
            {
                let sessions = inner.sessions.borrow();
                let reg = sessions.get(&session_id).expect("still registered");
                let storage = reg
                    .storage
                    .as_ref()
                    .expect("the dropped connection must not have taken the storage");
                assert_eq!(
                    storage.next_target_msg_seq_num().get(),
                    1,
                    "the Logon must not have been read, let alone processed"
                );
            }

            // Resumed: the connection is read. An unregistered identity keeps
            // the task from running a session loop the test would then have
            // to tear down.
            inner.suspended.set(false);
            let stranger = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("STRANGER"));
            let written =
                run_first_message(inner.clone(), &test_helpers::serialize_message(&stranger)).await;
            assert!(written.is_empty());
            let stranger_id = SessionId::new(
                Version::FIXT11,
                fix_str!("STRANGER").to_owned(),
                fix_str!("TARGET").to_owned(),
            );
            assert_eq!(
                observer.summaries(),
                vec![
                    "acceptor_suspended".to_owned(),
                    format!("unknown:{stranger_id}")
                ],
                "after resume the first message is read and identified again"
            );
        })
        .await;
}

/// Scenario 2S: the first message must be a `Logon<A>`. `SessionId` is derived
/// from the CompIDs alone, so without this check any frame naming a registered
/// pair takes that session's storage out of the registry - locking the real
/// peer out with `SessionAlreadyActive` for as long as the stray connection
/// lives. SequenceReset-Reset (123=N) is the sharpest case: its own MsgSeqNum
/// is ignored by design (Session Layer §4.8.8), so no sequence check stands
/// between it and the session's counters.
///
/// The gate is on the message *type*, not on SequenceReset specifically - and
/// it runs before the registry is consulted at all. The second half makes that
/// observable by registering the session with its storage already taken: a
/// lookup would report `SessionAlreadyActive`, so `not_logon` proves the stray
/// Heartbeat never reached it.
#[tokio::test]
async fn non_logon_first_message_is_dropped_without_taking_storage() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            let session_id =
                register_default_session(&inner, Some(test_helpers::default_storage()));

            let msg = test_helpers::sequence_reset(1, 4_000_000, false);
            let bytes = test_helpers::serialize_message(&msg);
            let written = run_first_message(inner.clone(), &bytes).await;

            assert!(
                written.is_empty(),
                "Scenario 2S: log an error and disconnect - nothing on the wire"
            );
            assert_eq!(observer.summaries(), vec!["not_logon".to_owned()]);

            {
                let sessions = inner.sessions.borrow();
                let storage = sessions
                    .get(&session_id)
                    .expect("registered")
                    .storage
                    .as_ref()
                    .expect("storage must never leave the registry for a non-Logon");
                assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
            }

            // `None` = the real peer is connected and holds the storage.
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            register_default_session(&inner, None);

            let written = run_first_message(inner, &test_helpers::heartbeat_bytes(1)).await;

            assert!(written.is_empty());
            assert_eq!(observer.summaries(), vec!["not_logon".to_owned()]);
        })
        .await;
}

/// `max_first_message_size` is a limit, not a buffer hint: a first message
/// that outgrows it is dropped without a reply and reported as such, and the
/// registry is never consulted - the Logon names a registered session, yet
/// its storage stays in place.
#[tokio::test]
async fn oversized_first_message_is_dropped_silently() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let limit = NonZeroUsize::new(256).unwrap();
            let observer = Rc::new(RecordingObserver::default());
            let inner = Rc::new(AcceptorInner::new(
                AcceptorSettings {
                    max_first_message_size: limit,
                    ..AcceptorSettings::default()
                },
                test_helpers::StubAppFactory,
                TimerBackend::Tokio,
            ));
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            let session_id =
                register_default_session(&inner, Some(test_helpers::default_storage()));

            // A Logon for the registered session, padded past the limit
            // through Text(58).
            let text = "x".repeat(600);
            let logon = test_helpers::frame_message(
                "FIXT.1.1",
                &format!(
                    "35=A|49=TARGET|56=SENDER|34=1|52=20260721-08:00:00.000|98=0|108=30|58={text}|"
                ),
            );
            let written = run_first_message(inner.clone(), &logon).await;

            assert!(
                written.is_empty(),
                "an oversized first message must be dropped without a reply"
            );
            assert_eq!(observer.summaries(), vec!["too_large".to_owned()]);
            assert!(
                inner
                    .sessions
                    .borrow()
                    .get(&session_id)
                    .expect("registered")
                    .storage
                    .is_some(),
                "the registry must not be consulted for a message that never completed"
            );
        })
        .await;
}

/// A peer that opens a connection and closes it without sending anything is
/// reported too - it carries no identity, but repeated from one address it is
/// the signature of a port scan (and, equally, of a load-balancer probe: the
/// library reports, the application classifies).
#[tokio::test]
async fn silent_peer_is_reported_as_closed_before_first_message() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());

            let (server_io, client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            // Close the client side without writing - the session task sees EOF
            // rather than waiting out the logon timeout.
            drop(client_io);

            let guard = TaskGuard::new(inner.clone());
            task::spawn_local(acceptor_session_task(
                inner,
                server_reader,
                server_writer,
                test_helpers::TEST_PEER_ADDR,
                guard,
            ))
            .await
            .expect("session task");

            assert_eq!(observer.summaries(), vec!["closed".to_owned()]);
        })
        .await;
}

/// A peer that opens a connection and then says nothing is reported when
/// `auto_disconnect_after_no_logon_received` runs out - as a timeout, not as
/// a close: the connection is still open when the acceptor gives up on it.
/// The paused clock jumps straight to the deadline, so the default 10 s
/// budget costs nothing; the peer end is held open for the whole test, since
/// an EOF would turn the verdict into `ClosedBeforeFirstMessage`.
#[tokio::test(start_paused = true)]
async fn silent_peer_is_reported_as_a_logon_timeout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());

            let (server_io, _client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);

            let guard = TaskGuard::new(inner.clone());
            let task = task::spawn_local(acceptor_session_task(
                inner,
                server_reader,
                server_writer,
                test_helpers::TEST_PEER_ADDR,
                guard,
            ));
            timeout(Duration::from_secs(60), task)
                .await
                .expect("the logon timeout must end the connection")
                .expect("session task");

            assert_eq!(observer.summaries(), vec!["timeout".to_owned()]);
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn acceptors_keep_independent_clocks_before_session_identification() {
    let settings = AcceptorSettings {
        auto_disconnect_after_no_logon_received: Duration::from_secs(3600),
        ..AcceptorSettings::default()
    };
    let busywait_acceptor = Acceptor::<Message, InMemoryStorage, _>::with_busywait_timers(
        settings.clone(),
        test_helpers::StubAppFactory,
    );
    let tokio_acceptor = Acceptor::<Message, InMemoryStorage, _>::with_settings(
        settings,
        test_helpers::StubAppFactory,
    );
    let busywait_observer = Rc::new(RecordingObserver::default());
    let tokio_observer = Rc::new(RecordingObserver::default());
    busywait_acceptor.set_connection_observer(busywait_observer.clone());
    tokio_acceptor.set_connection_observer(tokio_observer.clone());

    let (busywait_server, busywait_peer) = io::duplex(8192);
    let (tokio_server, _tokio_peer) = io::duplex(8192);
    let (reader, writer) = io::split(busywait_server);
    let mut busywait_task =
        pin!(busywait_acceptor.session_task(reader, writer, test_helpers::TEST_PEER_ADDR));
    let (reader, writer) = io::split(tokio_server);
    let mut tokio_task =
        pin!(tokio_acceptor.session_task(reader, writer, test_helpers::TEST_PEER_ADDR));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(busywait_task.as_mut().poll(&mut cx).is_pending());
    assert!(tokio_task.as_mut().poll(&mut cx).is_pending());

    // Manual polling keeps the busywait task from spinning while Tokio's
    // virtual clock advances. Both connections still have their custom budget.
    time::advance(Duration::from_secs(1800)).await;
    assert!(busywait_task.as_mut().poll(&mut cx).is_pending());
    assert!(tokio_task.as_mut().poll(&mut cx).is_pending());

    time::advance(Duration::from_secs(1801)).await;
    assert!(tokio_task.as_mut().poll(&mut cx).is_ready());
    assert_eq!(tokio_observer.summaries(), ["timeout"]);
    assert!(busywait_task.as_mut().poll(&mut cx).is_pending());
    assert!(busywait_observer.summaries().is_empty());

    drop(busywait_peer);
    assert!(busywait_task.as_mut().poll(&mut cx).is_ready());
    assert_eq!(busywait_observer.summaries(), ["closed"]);
}

/// A transport that fails before the first message is complete is reported
/// as an I/O error - the one drop reason that says nothing about the peer's
/// behaviour, so a policy counting hostile connections can leave it out.
#[tokio::test]
async fn read_failure_before_first_message_is_reported_as_io_error() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());

            let (server_io, _client_io) = io::duplex(8192);
            let (_server_reader, server_writer) = io::split(server_io);

            let guard = TaskGuard::new(inner.clone());
            task::spawn_local(acceptor_session_task(
                inner,
                test_helpers::FailingReader,
                server_writer,
                test_helpers::TEST_PEER_ADDR,
                guard,
            ))
            .await
            .expect("session task");

            assert_eq!(observer.summaries(), vec!["io_error".to_owned()]);
        })
        .await;
}

/// Connections torn down by the acceptor's own `shutdown` are NOT reported:
/// the caller initiated the shutdown and already knows, so counting these
/// would penalize well-behaved peers for our restart.
#[tokio::test]
async fn shutdown_teardown_is_not_reported() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let observer = Rc::new(RecordingObserver::default());
            let inner = stub_inner();
            *inner.connection_observer.borrow_mut() = Some(observer.clone());
            let session_id =
                register_default_session(&inner, Some(test_helpers::default_storage()));
            // Already shut down when the connection arrives, so the task leaves
            // at its first-message read without touching the registry.
            inner.stopping.set(true);

            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            let bytes = test_helpers::serialize_message(&logon);
            run_first_message(inner.clone(), &bytes).await;

            assert!(
                observer.summaries().is_empty(),
                "shutdown is the caller's own doing - not an observable drop"
            );
            let sessions = inner.sessions.borrow();
            assert!(
                sessions
                    .get(&session_id)
                    .expect("registered")
                    .storage
                    .is_some(),
                "a connection dropped at shutdown must leave the session's \
                 storage in the registry, or the SessionId is locked out of \
                 the acceptor that replaces this one"
            );
        })
        .await;
}

/// Observer holding an `Acceptor` handle, used to prove the callback may
/// re-enter the acceptor. The resulting reference cycle leaks, which is
/// acceptable for the duration of a test.
struct ReentrantObserver {
    acceptor: RefCell<Option<Acceptor<Message, InMemoryStorage, test_helpers::StubAppFactory>>>,
    reentered: Cell<bool>,
}

impl ConnectionObserver for ReentrantObserver {
    fn on_connection_dropped(&self, _: SocketAddr, _: ConnectionDropReason<'_>) {
        let acceptor = self.acceptor.borrow().clone().expect("acceptor set");
        // Re-enters `inner.sessions`. If the drop were reported while the
        // registry lookup still held its `borrow_mut`, this panics.
        let _ = acceptor.is_session_active(&test_helpers::default_session_id());
        self.reentered.set(true);
    }
}

/// The observer is documented as free to call back into the `Acceptor`. Three
/// of the reported drops are classified inside a live `sessions.borrow_mut()`,
/// so the report must be deferred until that borrow is released - otherwise
/// any observer touching the registry panics.
#[tokio::test]
async fn observer_may_reenter_the_acceptor() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let acceptor = Acceptor::new(test_helpers::StubAppFactory);
            let observer = Rc::new(ReentrantObserver {
                acceptor: RefCell::new(Some(acceptor.clone())),
                reentered: Cell::new(false),
            });
            acceptor.set_connection_observer(observer.clone());

            // Register, then suspend: the inbound Logon is classified as
            // SessionSuspended - one of the in-borrow verdicts.
            let session_id = test_helpers::default_session_id();
            acceptor
                .register_session(
                    session_id.clone(),
                    test_helpers::default_session_settings(),
                    |_, max| Ok(InMemoryStorage::new(max)),
                )
                .expect("register");
            acceptor.suspend_session(&session_id).expect("suspend");

            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            let bytes = test_helpers::serialize_message(&logon);
            client_io.write_all(&bytes).await.expect("write logon");

            acceptor
                .run_session(server_reader, server_writer, test_helpers::TEST_PEER_ADDR)
                .await
                .expect("session task must not panic re-entering the acceptor");

            assert!(observer.reentered.get(), "observer was not called");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Listener loop
// ---------------------------------------------------------------------------

/// A [`Connection`] whose `accept()` fails for the first `failures` calls and
/// parks forever afterwards, recording the co-scheduled task's tick count at
/// each attempt. Parking at the end keeps a starving listener from wedging the
/// test thread, so the assertion below reports a failure instead of hanging.
struct FailingConnection {
    failures: usize,
    ticks: Rc<Cell<usize>>,
    ticks_at_attempt: Rc<RefCell<Vec<usize>>>,
}

#[expect(
    refining_impl_trait_internal,
    reason = "naming the success types is the only way to write an accept() that never succeeds"
)]
impl Connection for FailingConnection {
    async fn accept(
        &mut self,
    ) -> Result<(io::DuplexStream, io::DuplexStream, SocketAddr), std_io::Error> {
        self.ticks_at_attempt.borrow_mut().push(self.ticks.get());
        if self.failures == 0 {
            future::pending::<()>().await;
        }
        self.failures -= 1;
        Err(std_io::Error::other("accept failed"))
    }
}

/// A failing `accept()` must hand the runtime back between attempts. Tokio
/// clears the listener's readiness only on `WouldBlock`, so on any other error
/// the next `accept()` completes synchronously with the same error - a listener
/// that retries straight away never returns `Poll::Pending`, and on the
/// mandatory current-thread runtime nothing else is ever polled again. Every
/// established session then stops reading input and stops sending
/// `Heartbeat<0>`, so its peer times it out (FIX Session Layer §4.5.5).
///
/// A concurrent task ticking on its own timer therefore has to make progress
/// *between* consecutive accept attempts.
///
/// The paused clock makes the ordering deterministic: every wait auto-advances
/// to the next armed timer, so the 1 ms ticker fires ten times inside each
/// 10 ms backoff instead of racing it on a loaded runner.
#[tokio::test(start_paused = true)]
async fn failing_accept_yields_between_attempts() {
    const FAILURES: usize = 4;

    let local = LocalSet::new();
    local
        .run_until(async {
            let ticks = Rc::new(Cell::new(0));
            let ticks_at_attempt = Rc::new(RefCell::new(Vec::new()));

            let ticker_ticks = ticks.clone();
            let ticker = task::spawn_local(async move {
                loop {
                    sleep(Duration::from_millis(1)).await;
                    ticker_ticks.set(ticker_ticks.get() + 1);
                }
            });

            let acceptor = stub_acceptor();
            let listener = acceptor.start(FailingConnection {
                failures: FAILURES,
                ticks,
                ticks_at_attempt: ticks_at_attempt.clone(),
            });

            // The listener parks on attempt FAILURES + 1, so waiting for the
            // full sequence cannot outlast the backoff it is measuring. Polled
            // on a timer, not with `yield_now`: the paused clock only advances
            // while every task is idle, and a spinning waiter would hold it.
            timeout(Duration::from_secs(5), async {
                while ticks_at_attempt.borrow().len() <= FAILURES {
                    sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("listener never reached the parking attempt");

            listener.abort();
            ticker.abort();

            let recorded = ticks_at_attempt.borrow().clone();
            assert!(
                recorded.windows(2).all(|w| w[1] > w[0]),
                "listener starved the runtime between accept attempts: {recorded:?}"
            );
        })
        .await;
}

/// A `FailingConnection` that parks on its first `accept()` - the shape of a
/// listener idling on a quiet port.
fn parked_connection() -> FailingConnection {
    FailingConnection {
        failures: 0,
        ticks: Rc::new(Cell::new(0)),
        ticks_at_attempt: Rc::new(RefCell::new(Vec::new())),
    }
}

/// `shutdown` must end the listener *before returning*, so the bound address is
/// free the moment it resolves. Parked in `accept()`, the loop would otherwise
/// sit on the listener until some unrelated connection happened to arrive, and
/// a replacement acceptor rebinding that address would fail until then.
///
/// Merely signalling the listener is not enough: with no active sessions
/// `shutdown` has nothing to await, so it would return before the listener was
/// ever polled - which is why the listener carries a [`TaskGuard`] of its own.
#[tokio::test]
async fn shutdown_ends_the_listener_before_returning() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let acceptor = stub_acceptor();
            let listener = acceptor.start(parked_connection());
            // Let the listener reach its parked `accept()`.
            task::yield_now().await;

            timeout(
                Duration::from_secs(1),
                acceptor.shutdown(ShutdownMode::Disconnect),
            )
            .await
            .expect("listener still parked in accept() after shutdown");

            assert!(
                listener.is_finished(),
                "shutdown returned with the listener still holding the address"
            );
            listener.await.expect("listener task");
        })
        .await;
}

/// `shutdown` must not wait out the Logon timeout on connections that have not
/// become sessions yet.
///
/// `TaskGuard::new` counts a connection in at spawn time, but its
/// `active_sessions` control channel only appears once the first message has
/// identified a session - so a connection still waiting for that message is
/// something `shutdown` waits for and cannot reach. A silent peer would
/// otherwise hold up even [`ShutdownMode::Disconnect`], documented as an
/// immediate disconnect, for the whole `auto_disconnect_after_no_logon_received`
/// (10s by default, and deployments configure far longer).
#[tokio::test]
async fn shutdown_does_not_wait_out_the_logon_timeout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let acceptor = stub_acceptor();
            // Held open for the whole test: on EOF the read would finish on its
            // own and the connection would never reach the state under test.
            let (server_io, _client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let session =
                acceptor.run_session(server_reader, server_writer, test_helpers::TEST_PEER_ADDR);
            // Let the session task reach its first-message read.
            task::yield_now().await;

            timeout(
                Duration::from_secs(1),
                acceptor.shutdown(ShutdownMode::Disconnect),
            )
            .await
            .expect("shutdown waited out the logon timeout");

            // `_client_io` never sends EOF, so a session task that outlives
            // `shutdown` would hang here rather than fail.
            timeout(Duration::from_secs(1), session)
                .await
                .expect("session task outlived shutdown")
                .expect("session task");
        })
        .await;
}
