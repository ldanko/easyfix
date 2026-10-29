use std::{
    assert_matches, borrow::Cow, cell::Cell, error::Error as StdError, fmt, future, io::ErrorKind,
    net::SocketAddr, rc::Rc,
};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    basic_types::{NonZeroLength, SeqNum, Utc},
    deserializer::raw_message,
    fix_str,
    message::{HeaderAccess, SessionMessage},
    serializer::SerializeError,
};
use easyfix_test_messages::Message;
use futures_util::FutureExt;
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, mpsc},
    task::{self, JoinHandle, LocalSet, yield_now},
    time::{self, Duration, timeout},
};

use super::{Initiator, InitiatorError, SessionStart};
use crate::{
    Acceptor, AcceptorError, SendError, Sender, SessionId,
    application::{
        EndContext, InputAction, IoOperation, SessionContext, SessionEnd, SessionEndReason,
        SessionHandler,
    },
    messages_storage::{InMemoryStorage, MessagesStorage},
    settings::SessionSettings,
    test_helpers::{
        self, CountedResetMessage, FailingStorage, FailureTiming, InjectedStorageError,
        ObservedStorage, SessionEndSnapshot, StorageOp, TerminationSnapshot, nz_seq,
        reset_support_probe_count,
    },
};

#[test]
fn busywait_initiator_reconnects_without_a_tokio_runtime() {
    let initiator = Initiator::<Message, InMemoryStorage, _>::with_busywait_timers(
        test_helpers::default_session_id(),
        test_helpers::default_session_settings(),
        test_helpers::make_stub_handler,
        |_, size| Ok(InMemoryStorage::new(size)),
    )
    .unwrap();

    for _ in 0..2 {
        // Write the opening message, then complete on the peer's EOF.
        let task = initiator.session_task(io::empty(), io::sink(), None);
        assert_matches!(task.now_or_never(), Some(Ok(())));
    }
    assert_eq!(
        initiator
            .inner
            .storage
            .borrow()
            .as_ref()
            .unwrap()
            .next_sender_msg_seq_num()
            .get(),
        3,
    );
}

fn no_callbacks(_: &SessionContext<'_>) -> test_helpers::ReadyHandler<test_helpers::StubHandler> {
    panic!("management operations must not create a handler");
}

#[tokio::test]
async fn connect_reserves_immediately_and_duplicate_failure_preserves_first_task() {
    LocalSet::new()
        .run_until(async {
            let entered = Rc::new(Notify::new());
            let initiator = initiator({
                let entered = entered.clone();
                async move |_: &SessionContext<'_>| {
                    entered.notify_one();
                    future::pending::<()>().await;
                    Ok(test_helpers::StubHandler)
                }
            });
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let first = initiator.connect(address);
            assert!(initiator.is_session_active());
            assert_matches!(
                initiator.reset_session(),
                Err(InitiatorError::SessionActive)
            );

            let second = initiator.connect(address);
            assert_matches!(
                timeout(Duration::from_secs(1), second)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(InitiatorError::SessionActive)
            );
            assert!(initiator.is_session_active());
            let (_peer, _) = timeout(Duration::from_secs(1), listener.accept())
                .await
                .unwrap()
                .unwrap();
            timeout(Duration::from_secs(1), entered.notified())
                .await
                .unwrap();
            assert!(listener.accept().now_or_never().is_none());
            assert!(!first.is_finished());

            timeout(Duration::from_secs(1), initiator.close())
                .await
                .unwrap()
                .unwrap();
            first.await.unwrap().unwrap();
            assert!(!initiator.is_session_active());
            initiator.reset_session().unwrap();
        })
        .await;
}

#[tokio::test]
async fn connect_can_be_cancelled_before_first_poll_without_opening_tcp() {
    #[derive(Clone, Copy, Debug)]
    enum Stop {
        Close,
        Logout,
        Disconnect,
        Abort,
    }
    LocalSet::new()
        .run_until(async {
            let initiator = initiator(no_callbacks);
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            for action in [Stop::Close, Stop::Logout, Stop::Disconnect, Stop::Abort] {
                let handle = initiator.connect(listener.local_addr().unwrap());
                assert!(initiator.is_session_active(), "{action:?}");
                match action {
                    Stop::Close => timeout(Duration::from_secs(1), initiator.close())
                        .await
                        .unwrap()
                        .unwrap(),
                    Stop::Logout => initiator.logout(None, None, false).await.unwrap(),
                    Stop::Disconnect => initiator.disconnect().await.unwrap(),
                    Stop::Abort => handle.abort(),
                }
                let result = timeout(Duration::from_secs(1), handle).await.unwrap();
                if matches!(action, Stop::Abort) {
                    assert!(result.unwrap_err().is_cancelled());
                } else {
                    result.unwrap().unwrap();
                }
                assert!(!initiator.is_session_active(), "{action:?}");
                assert!(listener.accept().now_or_never().is_none(), "{action:?}");
                initiator.reset_session().unwrap();
            }
        })
        .await;
}

#[tokio::test]
async fn tcp_setup_error_is_returned_by_task_and_allows_retry() {
    LocalSet::new()
        .run_until(async {
            let creates = Rc::new(Cell::new(0));
            let initiator = initiator(counting_start_handler(creates.clone()));
            let no_addresses: &'static [SocketAddr] = &[];
            let failed = initiator.connect(no_addresses);
            assert!(initiator.is_session_active());
            assert_matches!(
                timeout(Duration::from_secs(1), failed)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(InitiatorError::Io(_))
            );
            assert!(!initiator.is_session_active());
            assert_eq!(creates.get(), 0);

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let retry = initiator.connect(listener.local_addr().unwrap());
            let (mut peer, _) = timeout(Duration::from_secs(1), listener.accept())
                .await
                .unwrap()
                .unwrap();
            timeout(
                Duration::from_secs(1),
                test_helpers::read_one_message(&mut peer, &mut Vec::new()),
            )
            .await
            .unwrap();
            assert_eq!(creates.get(), 1);
            timeout(Duration::from_secs(1), initiator.close())
                .await
                .unwrap()
                .unwrap();
            retry.await.unwrap().unwrap();
            assert!(!initiator.is_session_active());
        })
        .await;
}

#[derive(Debug, thiserror::Error)]
#[error("handler initialization failed")]
struct HandlerInitFailure(Rc<()>);

#[tokio::test]
async fn async_handler_failure_preserves_source_and_storage_for_retry() {
    LocalSet::new()
        .run_until(async {
            let error_identity = Rc::new(());
            let attempts = Rc::new(Cell::new(0));
            let address = "127.0.0.1:4321".parse().unwrap();
            let initiator = Initiator::<Message, InMemoryStorage, _>::new(
                test_helpers::default_session_id(),
                SessionSettings::default(),
                {
                    let error_identity = error_identity.clone();
                    let attempts = attempts.clone();
                    async move |ctx: &SessionContext<'_>| {
                        yield_now().await;
                        // Both context and captured state are borrowed across await.
                        assert_eq!(ctx.session_id(), &test_helpers::default_session_id());
                        assert_eq!(ctx.peer_addr(), Some(address));
                        attempts.set(attempts.get() + 1);
                        if attempts.get() == 1 {
                            return Err(HandlerInitFailure(error_identity.clone()).into());
                        }
                        Ok(test_helpers::StubHandler)
                    }
                },
                |_, max| {
                    let mut storage = InMemoryStorage::new(max);
                    storage.set_next_sender_msg_seq_num(nz_seq(7)).unwrap();
                    storage.set_next_target_msg_seq_num(nz_seq(11)).unwrap();
                    Ok(storage)
                },
            )
            .unwrap();
            let (local, mut peer) = io::duplex(8192);
            let (reader, writer) = io::split(local);
            let handle = initiator.run_session_with_reset(reader, writer, Some(address));
            let error = timeout(Duration::from_secs(1), handle)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_matches!(&error, InitiatorError::HandlerInit(_));
            let source = error
                .source()
                .unwrap()
                .downcast_ref::<HandlerInitFailure>()
                .unwrap();
            assert!(Rc::ptr_eq(&source.0, &error_identity));
            let mut wire = Vec::new();
            timeout(Duration::from_secs(1), peer.read_to_end(&mut wire))
                .await
                .unwrap()
                .unwrap();
            assert!(wire.is_empty());
            assert!(!initiator.is_session_active());
            {
                let storage = initiator.inner.storage.borrow();
                let storage = storage.as_ref().unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 7);
                assert_eq!(storage.next_target_msg_seq_num().get(), 11);
            }
            let retry = initiator.run_session(io::empty(), io::sink(), Some(address));
            timeout(Duration::from_secs(1), retry)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(attempts.get(), 2);
            assert!(!initiator.is_session_active());
        })
        .await;
}

#[tokio::test]
async fn pending_handler_initialization_is_cancelled_and_releases_resources() {
    #[derive(Clone, Copy, Debug)]
    enum Stop {
        Close,
        Logout,
        Abort,
    }
    LocalSet::new()
        .run_until(async {
            for action in [Stop::Close, Stop::Logout, Stop::Abort] {
                let resource = Rc::new(());
                let entered = Rc::new(Notify::new());
                let initiator = initiator({
                    let resource = resource.clone();
                    let entered = entered.clone();
                    async move |_: &SessionContext<'_>| {
                        let held = resource.clone();
                        entered.notify_one();
                        future::pending::<()>().await;
                        drop(held);
                        Ok(test_helpers::StubHandler)
                    }
                });
                let (local, mut peer) = io::duplex(8192);
                let (reader, writer) = io::split(local);
                let handle = initiator.run_session(reader, writer, None);
                timeout(Duration::from_secs(1), entered.notified())
                    .await
                    .unwrap();
                assert_eq!(Rc::strong_count(&resource), 3);
                assert!(initiator.is_session_active());
                // An in-session reset request must not abandon pending init.
                initiator.request_running_session_reset().await.unwrap();
                yield_now().await;
                assert!(!handle.is_finished());
                match action {
                    Stop::Close => timeout(Duration::from_secs(1), initiator.close())
                        .await
                        .unwrap()
                        .unwrap(),
                    Stop::Logout => initiator.logout(None, None, false).await.unwrap(),
                    Stop::Abort => handle.abort(),
                }
                let result = timeout(Duration::from_secs(1), handle).await.unwrap();
                if matches!(action, Stop::Abort) {
                    assert!(result.unwrap_err().is_cancelled());
                } else {
                    result.unwrap().unwrap();
                }
                assert_eq!(Rc::strong_count(&resource), 2, "{action:?}");
                let mut wire = Vec::new();
                timeout(Duration::from_secs(1), peer.read_to_end(&mut wire))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(wire.is_empty(), "{action:?}");
                assert!(!initiator.is_session_active(), "{action:?}");
                let retry = initiator.session_task(io::empty(), io::sink(), None);
                drop(retry);
                initiator.reset_session().unwrap();
            }
        })
        .await;
}

#[test]
fn invalid_settings_prevent_storage_factory_calls() {
    let id = test_helpers::default_session_id();
    let settings = SessionSettings {
        max_message_size: NonZeroLength::new(1).unwrap(),
        ..SessionSettings::default()
    };
    let result = Initiator::<Message, FailingStorage, _>::new(
        id.clone(),
        settings.clone(),
        no_callbacks,
        |_, _| panic!("invalid settings must reject before opening storage"),
    );
    assert_matches!(
        result.err(),
        Some(InitiatorError::MaxMessageSizeTooSmall { .. })
    );
    let result = Initiator::<Message, FailingStorage, _>::with_busywait_timers(
        id.clone(),
        settings.clone(),
        no_callbacks,
        |_, _| panic!("invalid settings must reject before opening storage"),
    );
    assert_matches!(
        result.err(),
        Some(InitiatorError::MaxMessageSizeTooSmall { .. })
    );
    let acceptor = Acceptor::<Message, FailingStorage, _>::new(no_callbacks);
    let result = acceptor.register_session(id.clone(), settings, |_, _| {
        panic!("invalid settings must reject before opening storage");
    });
    assert_matches!(result, Err(AcceptorError::MaxMessageSizeTooSmall { .. }));
    assert_matches!(
        acceptor.remove_session(&id),
        Err(AcceptorError::UnknownSession)
    );
}

#[test]
fn storage_factory_failures_preserve_source_and_allow_registration_retry() {
    let id = test_helpers::default_session_id();
    let error = Initiator::<Message, FailingStorage, _>::new(
        id.clone(),
        SessionSettings::default(),
        no_callbacks,
        |_, _| Err(InjectedStorageError(StorageOp::Reset)),
    )
    .err()
    .unwrap();
    assert_matches!(error, InitiatorError::Storage(_));
    assert_matches!(
        error
            .source()
            .unwrap()
            .downcast_ref::<InjectedStorageError>(),
        Some(InjectedStorageError(StorageOp::Reset))
    );

    let acceptor = Acceptor::<Message, FailingStorage, _>::new(no_callbacks);
    let error = acceptor
        .register_session(id.clone(), SessionSettings::default(), |_, _| {
            Err(InjectedStorageError(StorageOp::Reset))
        })
        .unwrap_err();
    assert_matches!(error, AcceptorError::Storage(_));
    assert_matches!(
        error
            .source()
            .unwrap()
            .downcast_ref::<InjectedStorageError>(),
        Some(InjectedStorageError(StorageOp::Reset))
    );
    assert_matches!(
        acceptor.remove_session(&id),
        Err(AcceptorError::UnknownSession)
    );
    acceptor
        .register_session(id.clone(), SessionSettings::default(), |_, _| {
            Ok(FailingStorage::new())
        })
        .unwrap();
    acceptor.remove_session(&id).unwrap();
}

#[test]
fn offline_reset_failure_returns_source_without_locking_either_endpoint() {
    for timing in [FailureTiming::Before, FailureTiming::After] {
        let id = test_helpers::default_session_id();
        let build = |_: &SessionId, _| {
            let mut storage = FailingStorage::new();
            storage.sender = nz_seq(40);
            storage.target = nz_seq(30);
            storage.fail_on(StorageOp::Reset, 1, timing);
            Ok(storage)
        };
        let initiator = Initiator::<Message, FailingStorage, _>::new(
            id.clone(),
            SessionSettings::default(),
            no_callbacks,
            build,
        )
        .unwrap();
        let error = initiator.reset_session().unwrap_err();
        assert_matches!(error, InitiatorError::Storage(_));
        assert_matches!(
            error
                .source()
                .unwrap()
                .downcast_ref::<InjectedStorageError>(),
            Some(InjectedStorageError(StorageOp::Reset))
        );
        assert!(initiator.inner.current_session.borrow().is_none());
        let storage = initiator.inner.storage.borrow_mut().take().unwrap();
        assert_eq!(storage.trace.borrow().calls, [StorageOp::Reset]);

        let acceptor = Acceptor::<Message, FailingStorage, _>::new(no_callbacks);
        acceptor
            .register_session(id.clone(), SessionSettings::default(), build)
            .unwrap();
        let error = acceptor.reset_session(&id).unwrap_err();
        assert_matches!(error, AcceptorError::Storage(_));
        assert_matches!(
            error
                .source()
                .unwrap()
                .downcast_ref::<InjectedStorageError>(),
            Some(InjectedStorageError(StorageOp::Reset))
        );
        let storage = acceptor.remove_session(&id).unwrap();
        assert_eq!(storage.trace.borrow().calls, [StorageOp::Reset]);
    }
}

enum StartEvent {
    Ready(Sender<Message>),
    Admin(Box<Message>),
    App(Box<Message>),
    End(SessionEndSnapshot),
}

enum CapabilityEvent {
    Ready(Sender<CountedResetMessage>),
    Logon(SeqNum, Option<bool>),
    App(SeqNum),
    End(SessionEndSnapshot),
}

impl fmt::Debug for CapabilityEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready(_) => f.write_str("Ready"),
            Self::Logon(seq, reset) => f.debug_tuple("Logon").field(seq).field(reset).finish(),
            Self::App(seq) => f.debug_tuple("App").field(seq).finish(),
            Self::End(reason) => f.debug_tuple("End").field(reason).finish(),
        }
    }
}

struct CapabilityHandler(mpsc::UnboundedSender<CapabilityEvent>);

impl SessionHandler<CountedResetMessage> for CapabilityHandler {
    fn on_output_error(&mut self, _: Box<CountedResetMessage>, _: &SerializeError) {
        panic!("serialization failed");
    }

    async fn on_session_ready(&mut self, _: &SessionId, sender: Sender<CountedResetMessage>) {
        self.0.send(CapabilityEvent::Ready(sender)).unwrap();
    }

    async fn on_session_end(&mut self, _: &SessionId, reason: SessionEnd) {
        let reason = SessionEndSnapshot::from(&reason);
        self.0.send(CapabilityEvent::End(reason)).unwrap();
    }

    async fn on_app_msg_in(&mut self, message: Box<CountedResetMessage>) -> InputAction {
        self.0
            .send(CapabilityEvent::App(message.msg_seq_num()))
            .unwrap();
        InputAction::Accept
    }

    async fn on_admin_msg_in(&mut self, message: &CountedResetMessage) -> InputAction {
        let Some(AdminBase::Logon(logon)) = message.try_as_admin() else {
            panic!("unexpected admin message: {message:?}");
        };
        self.0
            .send(CapabilityEvent::Logon(
                message.msg_seq_num(),
                logon.reset_seq_num_flag,
            ))
            .unwrap();
        InputAction::Accept
    }
}

fn capability_handler(
    events: mpsc::UnboundedSender<CapabilityEvent>,
) -> impl AsyncFn(&SessionContext<'_>) -> Result<CapabilityHandler, Box<dyn StdError>> {
    async move |_: &SessionContext<'_>| Ok(CapabilityHandler(events.clone()))
}

fn counted_order() -> Box<CountedResetMessage> {
    let bytes = test_helpers::serialize_message(&test_helpers::new_order_single(1));
    let mut msg = CountedResetMessage::from_bytes(&bytes).unwrap();
    msg.0.header = Default::default();
    msg
}

#[tokio::test(start_paused = true)]
async fn unsupported_running_reset_preserves_both_sessions_and_reuses_registration_checks() {
    LocalSet::new().run_until(async {
        let baseline = reset_support_probe_count();
        let settings = SessionSettings { heartbeat_interval: None, ..SessionSettings::default() };
        let (ini_tx, mut ini_events) = mpsc::unbounded_channel();
        let (acc_tx, mut acc_events) = mpsc::unbounded_channel();
        let (mut ini_store, ini_snapshot) = ObservedStorage::new(4096);
        let (mut acc_store, acc_snapshot) = ObservedStorage::new(4096);
        let history = test_helpers::serialize_message(&test_helpers::heartbeat(1, None));
        for store in [&mut ini_store, &mut acc_store] {
            store.store(nz_seq(1), |buf| { buf[..history.len()].copy_from_slice(&history); Ok(history.len()) }).unwrap();
            store.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
            store.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
        }
        let initiator = Initiator::new(test_helpers::default_session_id(), settings.clone(), capability_handler(ini_tx), |_, _| Ok(ini_store)).unwrap();
        assert_eq!(reset_support_probe_count(), baseline + 1);
        let acceptor = Acceptor::new(capability_handler(acc_tx));
        let id = test_helpers::default_session_id().reverse_route();
        acceptor.register_session(id.clone(), settings.clone(), |_, _| Ok(acc_store)).unwrap();
        assert_eq!(reset_support_probe_count(), baseline + 2);
        for opening_seq in [40, 1] {
            let (client, server) = io::duplex(8192);
            let (cr, cw) = io::split(client);
            let (sr, sw) = io::split(server);
            let acc_task = acceptor.run_session(sr, sw, test_helpers::TEST_PEER_ADDR);
            let ini_task = initiator.run_session(cr, cw, None);
            let CapabilityEvent::Ready(ini_sender) = ini_events.recv().await.unwrap() else { panic!("expected initiator ready"); };
            assert_matches!(acc_events.recv().await.unwrap(), CapabilityEvent::Logon(seq, None) if seq == opening_seq);
            let CapabilityEvent::Ready(acc_sender) = acc_events.recv().await.unwrap() else { panic!("expected acceptor ready"); };
            assert_matches!(ini_events.recv().await.unwrap(), CapabilityEvent::Logon(seq, None) if seq == opening_seq);
            let before_ini = ini_snapshot.borrow().clone();
            let before_acc = acc_snapshot.borrow().clone();
            for _ in 0..3 {
                assert_matches!(initiator.request_running_session_reset().await, Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon));
                assert_matches!(acceptor.request_running_session_reset(&id).await, Err(AcceptorError::ResetSeqNumFlagNotSupportedInLogon));
                assert_eq!(*ini_snapshot.borrow(), before_ini);
                assert_eq!(*acc_snapshot.borrow(), before_acc);
                assert_eq!(reset_support_probe_count(), baseline + 2);
            }
            time::advance(Duration::from_secs(35)).await;
            yield_now().await;
            assert!(!ini_task.is_finished());
            assert!(!acc_task.is_finished());
            assert!(ini_events.try_recv().is_err());
            assert!(acc_events.try_recv().is_err());
            assert_eq!(*ini_snapshot.borrow(), before_ini);
            assert_eq!(*acc_snapshot.borrow(), before_acc);
            ini_sender.send(counted_order()).unwrap();
            acc_sender.send(counted_order()).unwrap();
            assert_matches!(acc_events.recv().await.unwrap(), CapabilityEvent::App(seq) if seq == opening_seq + 1);
            assert_matches!(ini_events.recv().await.unwrap(), CapabilityEvent::App(seq) if seq == opening_seq + 1);
            initiator.disconnect().await.unwrap();
            assert_matches!(ini_events.recv().await.unwrap(), CapabilityEvent::End(SessionEndSnapshot { context: EndContext::Closing(SessionEndReason::ApplicationForcedDisconnect), termination: TerminationSnapshot::LocalClose, reset_unconfirmed: false }));
            assert_matches!(acc_events.recv().await.unwrap(), CapabilityEvent::End(SessionEndSnapshot { context: EndContext::Established, termination: TerminationSnapshot::PeerClose, reset_unconfirmed: false }));
            ini_task.await.unwrap().unwrap(); acc_task.await.unwrap();
            for snapshot in [&ini_snapshot, &acc_snapshot] {
                let snapshot = snapshot.borrow();
                assert_eq!(snapshot.sender.get(), opening_seq + 2);
                assert_eq!(snapshot.target.get(), opening_seq + 2);
                if opening_seq == 40 { assert_eq!(snapshot.messages.get(&nz_seq(1)), Some(&history)); }
            }
            assert_eq!(reset_support_probe_count(), baseline + 2);
            initiator.reset_session().unwrap();
            acceptor.reset_session(&id).unwrap();
            for snapshot in [&ini_snapshot, &acc_snapshot] {
                let snapshot = snapshot.borrow();
                assert_eq!(snapshot.sender.get(), 1); assert_eq!(snapshot.target.get(), 1);
                assert!(snapshot.messages.is_empty());
            }
        }
        let storage = acceptor.remove_session(&id).unwrap();
        acceptor.register_session(id, settings, |_, _| Ok(storage)).unwrap();
        assert_eq!(reset_support_probe_count(), baseline + 3);
    }).await;
}

#[tokio::test]
async fn running_reset_requires_an_active_session_before_message_support() {
    let initiator = Initiator::<CountedResetMessage, _, _>::new(
        test_helpers::default_session_id(),
        SessionSettings::default(),
        test_helpers::make_stub_handler,
        |_, max| Ok(InMemoryStorage::new(max)),
    )
    .unwrap();
    assert_matches!(
        initiator.request_running_session_reset().await,
        Err(InitiatorError::NoActiveSession)
    );
    assert!(initiator.inner.current_session.borrow().is_none());
    assert!(initiator.inner.storage.borrow().is_some());
}

struct StartHandler {
    events: mpsc::UnboundedSender<StartEvent>,
    decision: usize,
    skip_admin: usize,
}

fn start_input_action(decision: usize) -> InputAction {
    match decision {
        1 => InputAction::Reject {
            reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
            text: None,
            tag: None,
        },
        2 | 4 => InputAction::Logout {
            session_status: None,
            text: None,
            disconnect: decision == 4,
        },
        3 => InputAction::Disconnect,
        _ => InputAction::Accept,
    }
}

impl SessionHandler<Message> for StartHandler {
    fn on_output_error(&mut self, _msg: Box<Message>, _error: &SerializeError) {
        panic!("unexpected serialization error");
    }

    async fn on_session_ready(&mut self, _id: &SessionId, sender: Sender<Message>) {
        self.events.send(StartEvent::Ready(sender)).unwrap();
    }

    async fn on_session_end(&mut self, _id: &SessionId, reason: SessionEnd) {
        let reason = SessionEndSnapshot::from(&reason);
        self.events.send(StartEvent::End(reason)).unwrap();
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        self.events.send(StartEvent::App(msg)).unwrap();
        InputAction::Accept
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        self.events
            .send(StartEvent::Admin(Box::new(msg.clone())))
            .unwrap();
        if self.skip_admin > 0 {
            self.skip_admin -= 1;
            return InputAction::Accept;
        }
        let decision = self.decision;
        self.decision = 0;
        start_input_action(decision)
    }
}

fn start_handler(
    events: mpsc::UnboundedSender<StartEvent>,
    decision: usize,
    skip_admin: usize,
) -> impl AsyncFn(&SessionContext<'_>) -> Result<StartHandler, Box<dyn StdError>> {
    async move |_: &SessionContext<'_>| {
        Ok(StartHandler {
            events: events.clone(),
            decision,
            skip_admin,
        })
    }
}

async fn start_event(events: &mut mpsc::UnboundedReceiver<StartEvent>) -> StartEvent {
    timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn initiator_storage_failure_returns_backend_and_clears_active_handle() {
    LocalSet::new()
        .run_until(async {
            for fetch_number in [1, 2] {
                let mut storage = FailingStorage::new();
                storage.fail_on(StorageOp::Fetch, fetch_number, FailureTiming::Before);
                let trace = storage.trace.clone();
                let (events, mut received) = mpsc::unbounded_channel();
                let initiator = Initiator::<Message, _, _>::new(
                    test_helpers::default_session_id(),
                    SessionSettings::default(),
                    start_handler(events, 0, 0),
                    |_, _| Ok(storage),
                )
                .unwrap();
                let (local, mut peer) = io::duplex(8192);
                let (reader, writer) = io::split(local);
                let task = initiator.run_session(reader, writer, None);
                let sender = if fetch_number == 2 {
                    timeout(
                        Duration::from_secs(5),
                        test_helpers::read_one_message(&mut peer, &mut Vec::new()),
                    )
                    .await
                    .unwrap();
                    let StartEvent::Ready(sender) = start_event(&mut received).await else {
                        panic!("expected ready callback");
                    };
                    peer.write_all(&test_helpers::logon_bytes(1, 30))
                        .await
                        .unwrap();
                    assert!(matches!(
                        start_event(&mut received).await,
                        StartEvent::Admin(_)
                    ));
                    sender
                        .send(test_helpers::new_order_single_with_empty_header())
                        .unwrap();
                    Some(sender)
                } else {
                    None
                };
                assert!(matches!(
                    start_event(&mut received).await,
                    StartEvent::End(SessionEndSnapshot {
                        context: EndContext::Closing(SessionEndReason::StorageError),
                        termination: TerminationSnapshot::LocalClose,
                        reset_unconfirmed: false
                    })
                ));
                timeout(Duration::from_secs(5), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert!(
                    received.try_recv().is_err(),
                    "end callback must occur exactly once"
                );
                let mut byte = [0];
                assert_eq!(
                    timeout(Duration::from_secs(5), peer.read(&mut byte))
                        .await
                        .unwrap()
                        .unwrap(),
                    0
                );
                if let Some(sender) = sender {
                    assert_matches!(
                        sender.send(test_helpers::new_order_single_with_empty_header()),
                        Err(SendError::Closed(_))
                    );
                }
                assert!(initiator.inner.storage.borrow().is_some());
                assert!(initiator.inner.current_session.borrow().is_none());
                initiator.disconnect().await.unwrap();
                assert!(trace.borrow().failed);
                assert_eq!(trace.borrow().calls.last(), Some(&StorageOp::Fetch));

                // Taking and dropping an unpolled task tests the active-state gate
                // without reusing the failed backend or making another storage call.
                let (local, _peer) = io::duplex(8192);
                let (reader, writer) = io::split(local);
                drop(initiator.session_task(reader, writer, None));
                assert!(initiator.inner.storage.borrow().is_some());
            }
        })
        .await;
}

#[tokio::test]
async fn acceptor_storage_failure_returns_backend_and_releases_registration() {
    LocalSet::new()
        .run_until(async {
            for fetch_number in [1, 2] {
                let mut storage = FailingStorage::new();
                storage.fail_on(StorageOp::Fetch, fetch_number, FailureTiming::Before);
                let trace = storage.trace.clone();
                let (events, mut received) = mpsc::unbounded_channel();
                let acceptor = Acceptor::<Message, _, _>::new(start_handler(events, 0, 0));
                let id = test_helpers::default_session_id();
                acceptor
                    .register_session(id.clone(), SessionSettings::default(), |_, _| Ok(storage))
                    .unwrap();
                let (local, mut peer) = io::duplex(8192);
                let (reader, writer) = io::split(local);
                let task = acceptor.run_session(reader, writer, test_helpers::TEST_PEER_ADDR);
                peer.write_all(&test_helpers::logon_bytes(1, 30))
                    .await
                    .unwrap();
                assert!(matches!(
                    start_event(&mut received).await,
                    StartEvent::Admin(_)
                ));
                let sender = if fetch_number == 2 {
                    timeout(
                        Duration::from_secs(5),
                        test_helpers::read_one_message(&mut peer, &mut Vec::new()),
                    )
                    .await
                    .unwrap();
                    let StartEvent::Ready(sender) = start_event(&mut received).await else {
                        panic!("expected ready callback");
                    };
                    assert!(acceptor.is_session_active(&id).unwrap());
                    sender
                        .send(test_helpers::new_order_single_with_empty_header())
                        .unwrap();
                    Some(sender)
                } else {
                    None
                };
                assert!(matches!(
                    start_event(&mut received).await,
                    StartEvent::End(SessionEndSnapshot {
                        context: EndContext::Closing(SessionEndReason::StorageError),
                        termination: TerminationSnapshot::LocalClose,
                        reset_unconfirmed: false
                    })
                ));
                timeout(Duration::from_secs(5), task)
                    .await
                    .unwrap()
                    .unwrap();
                timeout(Duration::from_secs(5), acceptor.await_session_closed(&id))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(!acceptor.is_session_active(&id).unwrap());
                assert!(
                    received.try_recv().is_err(),
                    "end callback must occur exactly once"
                );
                let mut byte = [0];
                assert_eq!(
                    timeout(Duration::from_secs(5), peer.read(&mut byte))
                        .await
                        .unwrap()
                        .unwrap(),
                    0
                );
                if let Some(sender) = sender {
                    assert_matches!(
                        sender.send(test_helpers::new_order_single_with_empty_header()),
                        Err(SendError::Closed(_))
                    );
                }
                let storage = acceptor.remove_session(&id).unwrap();
                assert!(Rc::ptr_eq(&storage.trace, &trace));
                assert!(trace.borrow().failed);
                assert_eq!(trace.borrow().calls.last(), Some(&StorageOp::Fetch));
                acceptor
                    .register_session(id.clone(), SessionSettings::default(), |_, _| {
                        Ok(FailingStorage::new())
                    })
                    .unwrap();
                acceptor.remove_session(&id).unwrap();
            }
        })
        .await;
}

type StartTestInitiator<F> = (
    Initiator<Message, InMemoryStorage, F>,
    mpsc::UnboundedReceiver<StartEvent>,
);

fn start_test_initiator(
    decision: usize,
) -> StartTestInitiator<impl AsyncFn(&SessionContext<'_>) -> Result<StartHandler, Box<dyn StdError>>>
{
    start_test_initiator_for_reset(decision, false)
}

fn start_test_initiator_for_reset(
    decision: usize,
    in_session: bool,
) -> StartTestInitiator<impl AsyncFn(&SessionContext<'_>) -> Result<StartHandler, Box<dyn StdError>>>
{
    let (events, rx) = mpsc::unbounded_channel();
    let mut settings = test_helpers::default_session_settings();
    settings.auto_disconnect_after_no_logon_response = Duration::MAX;
    let initiator = Initiator::new(
        test_helpers::default_session_id(),
        settings,
        start_handler(events, decision, if in_session { 2 } else { 0 }),
        |_, max| Ok(InMemoryStorage::new(max)),
    )
    .unwrap();
    (initiator, rx)
}

fn start_ack(flag: bool) -> Vec<u8> {
    test_helpers::serialize_message(&test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        flag.then_some(true),
        None,
    ))
}

async fn start_reset_origin(
    initiator: &Initiator<
        Message,
        InMemoryStorage,
        impl AsyncFn(&SessionContext<'_>) -> Result<StartHandler, Box<dyn StdError>> + 'static,
    >,
    events: &mut mpsc::UnboundedReceiver<StartEvent>,
    in_session: bool,
) -> (
    JoinHandle<Result<(), InitiatorError>>,
    io::DuplexStream,
    Vec<u8>,
) {
    let (local, mut peer) = io::duplex(8192);
    let (reader, writer) = io::split(local);
    let task = if in_session {
        initiator.run_session(reader, writer, None)
    } else {
        initiator.run_session_with_reset(reader, writer, None)
    };
    let mut buf = Vec::new();
    let first = test_helpers::read_one_message(&mut peer, &mut buf).await;
    assert_eq!(first.msg_seq_num(), 1);
    assert_matches!(first.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == if in_session { None } else { Some(true) });
    assert!(matches!(start_event(events).await, StartEvent::Ready(_)));
    if in_session {
        peer.write_all(&start_ack(false)).await.unwrap();
        assert!(
            matches!(start_event(events).await, StartEvent::Admin(m) if matches!(m.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag.is_none()))
        );
        initiator.request_running_session_reset().await.unwrap();
        let probe = test_helpers::read_one_message(&mut peer, &mut buf).await;
        assert_eq!(probe.msg_seq_num(), 2);
        let Some(AdminBase::TestRequest(request)) = probe.try_as_admin() else {
            panic!("expected reset probe");
        };
        peer.write_all(&test_helpers::serialize_message(&test_helpers::heartbeat(
            2,
            Some(request.test_req_id.into_owned()),
        )))
        .await
        .unwrap();
        assert!(
            matches!(start_event(events).await, StartEvent::Admin(m) if matches!(m.try_as_admin(), Some(AdminBase::Heartbeat(_))))
        );
        let logon = test_helpers::read_one_message(&mut peer, &mut buf).await;
        assert_eq!(logon.msg_seq_num(), 1);
        assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
    }
    (task, peer, buf)
}

#[tokio::test]
async fn consumed_reset_ack_number_stops_the_next_input() {
    LocalSet::new().run_until(async {
        for in_session in [false, true] {
        for local_logout in [false, true] {
            for first in 0..4 {
                let decision = match first { 2 => 1, 3 => 2, _ => 0 };
                let (initiator, mut events) = start_test_initiator_for_reset(decision, in_session);
                let (task, mut peer, mut buf) = start_reset_origin(&initiator, &mut events, in_session).await;
                if local_logout {
                    initiator.logout(None, None, false).await.unwrap();
                    let logout = test_helpers::read_one_message(&mut peer, &mut buf).await;
                    assert_eq!(logout.header.msg_seq_num, 2);
                    assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
                }
                let now = Utc::now().format("%Y%m%d-%H:%M:%S%.3f");
                let mut input = match first {
                    0 => test_helpers::frame_message("FIXT.1.1", &format!("35=ZZ|49=TARGET|56=SENDER|34=1|52={now}|")),
                    1 => test_helpers::frame_message("FIXT.1.1", &format!("35=A|49=TARGET|56=SENDER|34=1|43=Y|52={now}|98=0|108=abc|141=Y|1137=9|")),
                    _ => test_helpers::serialize_message(&test_helpers::logout(1)),
                };
                input.extend_from_slice(&start_ack(true));
                peer.write_all(&input).await.unwrap();
                let expected: &[MsgTypeBase] = match (local_logout, first) {
                    (false, 0..=2) => &[MsgTypeBase::Reject, MsgTypeBase::Logout],
                    (true, 0..=2) => &[MsgTypeBase::Reject],
                    (false, 3) => &[MsgTypeBase::Logout],
                    _ => &[],
                };
                let mut next_out = if local_logout { 3 } else { 2 };
                for msg_type in expected {
                    let reply = timeout(Duration::from_secs(5), test_helpers::read_one_message(&mut peer, &mut buf)).await.unwrap();
                    assert_eq!(SessionMessage::msg_type(&*reply), *msg_type);
                    assert_eq!(reply.header.msg_seq_num, next_out);
                    next_out += 1;
                    if *msg_type == MsgTypeBase::Reject {
                        assert_matches!(reply.try_as_admin(), Some(AdminBase::Reject(reject))
                            if reject.session_reject_reason == Some(match first {
                                0 => SessionRejectReasonBase::InvalidMsgType,
                                1 => SessionRejectReasonBase::RequiredTagMissing,
                                _ => SessionRejectReasonBase::ValueIsIncorrect,
                            }.into()));
                    }
                }
                assert!(buf.is_empty());
                assert_eq!(timeout(Duration::from_secs(5), peer.read(&mut [0u8; 1])).await.unwrap().unwrap(), 0);
                timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
                if first >= 2 {
                    let StartEvent::Admin(logout) = start_event(&mut events).await else { panic!("expected refusal callback") };
                    assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
                }
                let StartEvent::End(end) = start_event(&mut events).await else { panic!("expected session end") };
                assert_eq!(end, SessionEndSnapshot {
                    context: EndContext::Closing(SessionEndReason::InvalidLogonState),
                    termination: TerminationSnapshot::LocalClose,
                    reset_unconfirmed: true,
                });
                assert!(events.try_recv().is_err());
                let saved = initiator.inner.storage.borrow();
                let storage = saved.as_ref().unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), next_out);
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            }
        }
        }
    }).await;
}

#[tokio::test]
async fn unsolicited_reset_flag_in_ack_is_refused() {
    LocalSet::new().run_until(async {
        let (initiator, mut events) = start_test_initiator(0);
        let (local, mut peer) = io::duplex(8192);
        let (reader, writer) = io::split(local);
        let task = initiator.run_session(reader, writer, None);
        let mut buf = Vec::new();
        let request = test_helpers::read_one_message(&mut peer, &mut buf).await;
        assert_matches!(request.try_as_admin(), Some(AdminBase::Logon(logon)) if logon.reset_seq_num_flag.is_none());
        let StartEvent::Ready(_sender) = start_event(&mut events).await else { panic!("expected ready") };
        peer.write_all(&start_ack(true)).await.unwrap();
        let logout = test_helpers::read_one_message(&mut peer, &mut buf).await;
        assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(logout))
            if logout.text.as_deref() == Some(fix_str!("Unsolicited ResetSeqNumFlag=Y in Logon response")));
        assert!(matches!(start_event(&mut events).await, StartEvent::End(SessionEndSnapshot { context: EndContext::Closing(SessionEndReason::InvalidLogonState), termination: TerminationSnapshot::LocalClose, reset_unconfirmed: false })));
        task.await.unwrap().unwrap();
        assert_eq!(initiator.inner.storage.borrow().as_ref().unwrap().next_target_msg_seq_num().get(), 1);
        assert!(events.try_recv().is_err());
    }).await;
}

#[tokio::test]
async fn accepted_reset_refusal_waits_for_eof_and_ignores_a_buffered_ack() {
    LocalSet::new()
        .run_until(async {
            for in_session in [false, true] {
                let (initiator, mut events) = start_test_initiator_for_reset(0, in_session);
                let (task, mut peer, mut buf) =
                    start_reset_origin(&initiator, &mut events, in_session).await;
                let mut input = test_helpers::serialize_message(&test_helpers::logout(1));
                input.extend_from_slice(&start_ack(true));
                peer.write_all(&input).await.unwrap();
                let reply = test_helpers::read_one_message(&mut peer, &mut buf).await;
                assert_eq!(reply.header.msg_seq_num, 2);
                assert_eq!(SessionMessage::msg_type(&*reply), MsgTypeBase::Logout);
                let StartEvent::Admin(logout) = start_event(&mut events).await else {
                    panic!("expected refusal callback")
                };
                assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
                assert!(!task.is_finished());
                assert!(events.try_recv().is_err());
                peer.shutdown().await.unwrap();
                assert!(matches!(
                    start_event(&mut events).await,
                    StartEvent::End(SessionEndSnapshot {
                        context: EndContext::RemoteLogout,
                        termination: TerminationSnapshot::PeerClose,
                        reset_unconfirmed: true
                    })
                ));
                task.await.unwrap().unwrap();
                assert!(events.try_recv().is_err());
                let saved = initiator.inner.storage.borrow();
                let storage = saved.as_ref().unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            }
        })
        .await;
}

#[tokio::test]
async fn confirmed_reset_ack_preserves_application_decisions_in_the_io_loop() {
    LocalSet::new().run_until(async {
        for in_session in [false, true] {
        for local_logout in [false, true] {
            for decision in 0..5 {
                let (initiator, mut events) = start_test_initiator_for_reset(decision, in_session);
                let (task, mut peer, mut buf) = start_reset_origin(&initiator, &mut events, in_session).await;
                if local_logout {
                    initiator.logout(None, None, false).await.unwrap();
                    let logout = test_helpers::read_one_message(&mut peer, &mut buf).await;
                    assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
                    assert_eq!(logout.header.msg_seq_num, 2);
                }
                peer.write_all(&start_ack(true)).await.unwrap();
                let StartEvent::Admin(ack) = start_event(&mut events).await else { panic!("expected ACK callback") };
                assert_eq!(SessionMessage::msg_type(&*ack), MsgTypeBase::Logon);
                let expected_replies: &[MsgTypeBase] = match (local_logout, decision) {
                    (false, 1) => &[MsgTypeBase::Reject, MsgTypeBase::Logout],
                    (true, 1) => &[MsgTypeBase::Reject],
                    (false, 2 | 4) => &[MsgTypeBase::Logout],
                    _ => &[],
                };
                let mut next_out = if local_logout { 3 } else { 2 };
                for msg_type in expected_replies {
                    let reply = test_helpers::read_one_message(&mut peer, &mut buf).await;
                    assert_eq!(SessionMessage::msg_type(&*reply), *msg_type);
                    assert_eq!(reply.header.msg_seq_num, next_out);
                    next_out += 1;
                }
                let reason = match decision {
                    1 | 3 | 4 => SessionEndSnapshot { context: EndContext::Closing(SessionEndReason::ApplicationForcedDisconnect), termination: TerminationSnapshot::LocalClose, reset_unconfirmed: false },
                    2 => {
                        assert!(!task.is_finished());
                        assert!(events.try_recv().is_err());
                        peer.write_all(&test_helpers::serialize_message(&test_helpers::logout(2))).await.unwrap();
                        let StartEvent::Admin(logout) = start_event(&mut events).await else { panic!("expected Logout ACK callback") };
                        assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
                        SessionEndSnapshot { context: EndContext::LocalLogout, termination: TerminationSnapshot::LocalClose, reset_unconfirmed: false }
                    }
                    _ => {
                        assert!(!task.is_finished());
                        assert!(events.try_recv().is_err());
                        initiator.disconnect().await.unwrap();
                        SessionEndSnapshot { context: EndContext::Closing(SessionEndReason::ApplicationForcedDisconnect), termination: TerminationSnapshot::LocalClose, reset_unconfirmed: false }
                    }
                };
                assert!(matches!(start_event(&mut events).await, StartEvent::End(actual) if actual == reason));
                timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
                assert!(buf.is_empty());
                assert_eq!(timeout(Duration::from_secs(5), peer.read(&mut [0u8; 1])).await.unwrap().unwrap(), 0);
                assert!(events.try_recv().is_err());
                let saved = initiator.inner.storage.borrow();
                let storage = saved.as_ref().unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), next_out);
                assert_eq!(storage.next_target_msg_seq_num().get(), if decision == 2 { 3 } else { 2 });
            }
        }
        }
    }).await;
}

#[tokio::test]
async fn manual_reset_session_then_reconnect() {
    LocalSet::new()
        .run_until(async {
            let (initiator, mut ini_events) = start_test_initiator(0);
            {
                let mut saved = initiator.inner.storage.borrow_mut();
                let storage = saved.as_mut().unwrap();
                storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
                storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
            }
            let (events, mut acc_events) = mpsc::unbounded_channel();
            let acceptor =
                Acceptor::<Message, InMemoryStorage, _>::new(start_handler(events, 0, 0));
            let sid = test_helpers::default_session_id().reverse_route();
            acceptor
                .register_session(
                    sid.clone(),
                    test_helpers::default_session_settings(),
                    |_, max| {
                        let mut storage = InMemoryStorage::new(max);
                        storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
                        storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
                        Ok(storage)
                    },
                )
                .unwrap();

            for expected in [40, 1] {
                let (client, server) = io::duplex(8192);
                let (client_r, client_w) = io::split(client);
                let (server_r, server_w) = io::split(server);
                let acc_task =
                    acceptor.run_session(server_r, server_w, "127.0.0.1:9876".parse().unwrap());
                let ini_task = initiator.run_session(client_r, client_w, None);
                assert!(matches!(
                    start_event(&mut ini_events).await,
                    StartEvent::Ready(_)
                ));
                let StartEvent::Admin(request) = start_event(&mut acc_events).await else {
                    panic!("expected Logon request")
                };
                assert_eq!(request.header.msg_seq_num, expected);
                assert_matches!(request.try_as_admin(), Some(AdminBase::Logon(logon))
                if logon.reset_seq_num_flag.is_none());
                assert!(matches!(
                    start_event(&mut acc_events).await,
                    StartEvent::Ready(_)
                ));
                let StartEvent::Admin(ack) = start_event(&mut ini_events).await else {
                    panic!("expected Logon ACK")
                };
                assert_eq!(ack.header.msg_seq_num, expected);
                assert_matches!(ack.try_as_admin(), Some(AdminBase::Logon(logon))
                if logon.reset_seq_num_flag.is_none());
                assert_matches!(
                    initiator.reset_session(),
                    Err(InitiatorError::SessionActive)
                );
                assert_matches!(
                    acceptor.reset_session(&sid),
                    Err(AcceptorError::SessionActive)
                );

                initiator.disconnect().await.unwrap();
                ini_task.await.unwrap().unwrap();
                acc_task.await.unwrap();
                assert!(matches!(
                    start_event(&mut ini_events).await,
                    StartEvent::End(SessionEndSnapshot {
                        context: EndContext::Closing(SessionEndReason::ApplicationForcedDisconnect),
                        termination: TerminationSnapshot::LocalClose,
                        reset_unconfirmed: false
                    })
                ));
                assert!(matches!(
                    start_event(&mut acc_events).await,
                    StartEvent::End(SessionEndSnapshot {
                        context: EndContext::Established,
                        termination: TerminationSnapshot::PeerClose,
                        reset_unconfirmed: false
                    })
                ));
                let mut storage = initiator.inner.storage.borrow_mut().take().unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), expected + 1);
                assert_eq!(storage.next_target_msg_seq_num().get(), expected + 1);
                assert!(
                    storage
                        .fetch(nz_seq(expected), nz_seq(expected))
                        .await
                        .is_ok()
                );
                *initiator.inner.storage.borrow_mut() = Some(storage);
                if expected == 40 {
                    initiator.reset_session().unwrap();
                    acceptor.reset_session(&sid).unwrap();
                    let mut storage = initiator.inner.storage.borrow_mut().take().unwrap();
                    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
                    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                    assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
                    *initiator.inner.storage.borrow_mut() = Some(storage);
                }
            }
            let mut storage = acceptor.remove_session(&sid).unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
        })
        .await;
}

#[tokio::test]
async fn offline_reset_session_does_not_require_reset_flag_support() {
    let sid = test_helpers::default_session_id();
    let history = test_helpers::serialize_message(&test_helpers::heartbeat(1, None));
    let build_storage = |_: &SessionId, max| {
        let mut storage = InMemoryStorage::new(max);
        storage
            .store(nz_seq(1), |buf| {
                buf[..history.len()].copy_from_slice(&history);
                Ok(history.len())
            })
            .unwrap();
        storage
            .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
            .unwrap();
        storage
            .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
            .unwrap();
        Ok(storage)
    };
    let initiator = Initiator::<CountedResetMessage, _, _>::new(
        sid.clone(),
        SessionSettings::default(),
        test_helpers::make_stub_handler,
        build_storage,
    )
    .unwrap();
    let acceptor =
        Acceptor::<CountedResetMessage, InMemoryStorage, _>::new(test_helpers::make_stub_handler);
    acceptor
        .register_session(sid.clone(), SessionSettings::default(), build_storage)
        .unwrap();
    initiator.reset_session().unwrap();
    acceptor.reset_session(&sid).unwrap();
    let ini_storage = initiator.inner.storage.borrow_mut().take().unwrap();
    let acc_storage = acceptor.remove_session(&sid).unwrap();
    for mut storage in [ini_storage, acc_storage] {
        assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
        assert_eq!(storage.next_target_msg_seq_num().get(), 1);
        assert!(storage.fetch(nz_seq(1), nz_seq(1)).await.is_err());
    }
}

#[tokio::test]
async fn reset_then_resume_against_accepting_acceptor() {
    enum Opening {
        Connect,
        RunSession,
        SessionTask,
    }

    LocalSet::new()
        .run_until(async {
            for opening in [Opening::Connect, Opening::RunSession, Opening::SessionTask] {
                let (initiator, mut ini_events) = start_test_initiator(0);
                {
                    let mut saved = initiator.inner.storage.borrow_mut();
                    let storage = saved.as_mut().unwrap();
                    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
                    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
                }
                let (events, mut acc_events) = mpsc::unbounded_channel();
                let acceptor =
                    Acceptor::<Message, InMemoryStorage, _>::new(start_handler(events, 0, 0));
                acceptor
                    .register_session(
                        test_helpers::default_session_id().reverse_route(),
                        test_helpers::default_session_settings(),
                        |_, max| {
                            let mut storage = InMemoryStorage::new(max);
                            storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
                            storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
                            Ok(storage)
                        },
                    )
                    .unwrap();
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let mut retained_order = None;
                for start in [SessionStart::Reset, SessionStart::Resume] {
                    let addr = listener.local_addr().unwrap();
                    let ini_task = match opening {
                        Opening::Connect => match start {
                            SessionStart::Reset => initiator.connect_with_reset(addr),
                            SessionStart::Resume => initiator.connect(addr),
                        },
                        Opening::RunSession | Opening::SessionTask => {
                            let tcp = TcpStream::connect(addr).await.unwrap();
                            let (reader, writer) = tcp.into_split();
                            match (&opening, start) {
                                (Opening::RunSession, SessionStart::Reset) => {
                                    initiator.run_session_with_reset(reader, writer, Some(addr))
                                }
                                (Opening::RunSession, SessionStart::Resume) => {
                                    initiator.run_session(reader, writer, Some(addr))
                                }
                                (Opening::SessionTask, SessionStart::Reset) => task::spawn_local(
                                    initiator.session_task_with_reset(reader, writer, Some(addr)),
                                ),
                                (Opening::SessionTask, SessionStart::Resume) => task::spawn_local(
                                    initiator.session_task(reader, writer, Some(addr)),
                                ),
                                (Opening::Connect, _) => unreachable!(),
                            }
                        }
                    };
                    let (tcp, peer_addr) = listener.accept().await.unwrap();
                    let (reader, writer) = tcp.into_split();
                    let acc_task = acceptor.run_session(reader, writer, peer_addr);
                    let StartEvent::Ready(sender) = start_event(&mut ini_events).await else {
                        panic!("expected initiator ready")
                    };
                    let StartEvent::Admin(request) = start_event(&mut acc_events).await else {
                        panic!("expected Logon request")
                    };
                    assert_matches!(request.try_as_admin(), Some(AdminBase::Logon(logon))
                if logon.reset_seq_num_flag == (start == SessionStart::Reset).then_some(true));
                    assert_eq!(
                        request.header.msg_seq_num,
                        if start == SessionStart::Reset { 1 } else { 3 }
                    );
                    let StartEvent::Ready(_acc_sender) = start_event(&mut acc_events).await else {
                        panic!("expected acceptor ready")
                    };
                    let StartEvent::Admin(ack) = start_event(&mut ini_events).await else {
                        panic!("expected Logon ACK")
                    };
                    assert_matches!(ack.try_as_admin(), Some(AdminBase::Logon(logon))
                if logon.reset_seq_num_flag == (start == SessionStart::Reset).then_some(true));
                    assert_eq!(
                        ack.header.msg_seq_num,
                        if start == SessionStart::Reset { 1 } else { 2 }
                    );
                    if start == SessionStart::Reset {
                        assert!(
                            sender
                                .send(test_helpers::new_order_single_with_empty_header())
                                .is_ok()
                        );
                        let StartEvent::App(order) = start_event(&mut acc_events).await else {
                            panic!("expected order")
                        };
                        assert_eq!(order.header.msg_seq_num, 2);
                    }
                    initiator.disconnect().await.unwrap();
                    ini_task.await.unwrap().unwrap();
                    acc_task.await.unwrap();
                    assert!(matches!(
                        start_event(&mut ini_events).await,
                        StartEvent::End(SessionEndSnapshot {
                            context: EndContext::Closing(
                                SessionEndReason::ApplicationForcedDisconnect
                            ),
                            termination: TerminationSnapshot::LocalClose,
                            reset_unconfirmed: false
                        })
                    ));
                    assert!(matches!(
                        start_event(&mut acc_events).await,
                        StartEvent::End(SessionEndSnapshot {
                            context: EndContext::Established,
                            termination: TerminationSnapshot::PeerClose,
                            reset_unconfirmed: false
                        })
                    ));
                    let mut storage = initiator.inner.storage.borrow_mut().take().unwrap();
                    assert_eq!(
                        storage.next_sender_msg_seq_num().get(),
                        if start == SessionStart::Reset { 3 } else { 4 }
                    );
                    assert_eq!(
                        storage.next_target_msg_seq_num().get(),
                        if start == SessionStart::Reset { 2 } else { 3 }
                    );
                    let order_bytes = storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap();
                    if let Some(expected) = retained_order.as_ref() {
                        assert_eq!(order_bytes, expected);
                    } else {
                        retained_order = Some(order_bytes.to_vec());
                    }
                    *initiator.inner.storage.borrow_mut() = Some(storage);
                }
            }
        })
        .await;
}

#[tokio::test]
async fn reset_at_connect_refused_after_local_logout() {
    reset_at_connect_refusal(true).await;
}

#[tokio::test]
async fn reset_at_connect_against_refusing_acceptor() {
    reset_at_connect_refusal(false).await;
}

async fn reset_at_connect_refusal(local_logout: bool) {
    LocalSet::new().run_until(async {
        let (initiator, mut ini_events) = start_test_initiator(0);
        let (events, mut acc_events) = mpsc::unbounded_channel();
        let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(start_handler(events, 0, 0));
        let mut settings = test_helpers::default_session_settings();
        settings.accept_reset_on_connect = false;
        acceptor.register_session(test_helpers::default_session_id().reverse_route(), settings, |_, max| {
            let mut storage = InMemoryStorage::new(max);
            storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
            storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
            Ok(storage)
        }).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ini_task = initiator.connect_with_reset(listener.local_addr().unwrap());
        let (tcp, peer_addr) = listener.accept().await.unwrap();
        let StartEvent::Ready(_sender) = start_event(&mut ini_events).await else { panic!("expected ready") };
        if local_logout {
            initiator.logout(None, None, false).await.unwrap();
            // Keep the acceptor from responding until both opening messages
            // are on the wire; peeking preserves them for its normal input path.
            timeout(Duration::from_secs(5), async {
                loop {
                    let mut bytes = vec![0u8; 8192];
                    let len = tcp.peek(&mut bytes).await.unwrap();
                    assert_ne!(len, 0);
                    if let Ok((rest, raw)) = raw_message(&bytes[..len])
                        && let Ok((_, raw_logout)) = raw_message(rest)
                    {
                        let request = Message::from_raw_message(raw).unwrap();
                        let logout = Message::from_raw_message(raw_logout).unwrap();
                        assert_eq!(request.header.msg_seq_num, 1);
                        assert_eq!(logout.header.msg_seq_num, 2);
                        assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
                        break;
                    }
                    yield_now().await;
                }
            }).await.unwrap();
        }
        let (reader, writer) = tcp.into_split();
        let acc_task = acceptor.run_session(reader, writer, peer_addr);
        let StartEvent::Admin(refusal) = start_event(&mut ini_events).await else { panic!("expected refusal callback") };
        assert_eq!(refusal.header.msg_seq_num, 40);
        assert_matches!(refusal.try_as_admin(), Some(AdminBase::Logout(logout))
            if logout.text.as_deref() == Some(fix_str!("Resetting the sequence number upon FIX connection establishment is not supported")));
        let StartEvent::End(end) = start_event(&mut ini_events).await else { panic!("expected session end") };
        assert!(end.reset_unconfirmed);
        assert_eq!(end.context, if local_logout { EndContext::LocalLogout } else { EndContext::RemoteLogout });
        if local_logout {
            assert_eq!(end.termination, TerminationSnapshot::LocalClose);
        } else {
            // TCP can report a reset when the refusing peer closes with our
            // acknowledgement unread; both retain the remote Logout context.
            assert_matches!(end.termination,
                TerminationSnapshot::PeerClose | TerminationSnapshot::IoError {
                    operation: IoOperation::Read, kind: ErrorKind::ConnectionReset,
                });
        }
        assert!(matches!(start_event(&mut acc_events).await, StartEvent::End(SessionEndSnapshot { context: EndContext::Closing(SessionEndReason::InvalidLogonState), termination: TerminationSnapshot::LocalClose, reset_unconfirmed: false })));
        ini_task.await.unwrap().unwrap();
        acc_task.await.unwrap();
        let mut storage = initiator.inner.storage.borrow_mut().take().unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        let bytes = storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap();
        let (_, raw) = raw_message(bytes).unwrap();
        let logout = Message::from_raw_message(raw).unwrap();
        assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
        assert_eq!(logout.header.msg_seq_num, 2);
    }).await;
}

fn counting_start_handler(
    creates: Rc<Cell<usize>>,
) -> impl AsyncFn(&SessionContext<'_>) -> Result<test_helpers::StubHandler, Box<dyn StdError>> {
    async move |_: &SessionContext<'_>| {
        creates.set(creates.get() + 1);
        Ok(test_helpers::StubHandler)
    }
}

#[tokio::test]
async fn unsupported_reset_start_preserves_the_stored_session() {
    LocalSet::new().run_until(async {
        let creates = Rc::new(Cell::new(0));
        let history = test_helpers::serialize_message(&test_helpers::heartbeat(1, None));
        let initiator = Initiator::<CountedResetMessage, _, _>::new(
            test_helpers::default_session_id(), SessionSettings::default(),
            counting_start_handler(creates.clone()), |_, max| {
                let mut storage = InMemoryStorage::new(max);
                storage.store(nz_seq(1), |buf| {
                    buf[..history.len()].copy_from_slice(&history);
                    Ok(history.len())
                }).unwrap();
                storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
                storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
                Ok(storage)
            }).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        assert!(matches!(initiator.connect_with_reset(listener.local_addr().unwrap()).await.unwrap(),
            Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon)));
        assert!(timeout(Duration::from_millis(20), listener.accept()).await.is_err());
        for spawn in [false, true] {
            let (local, mut peer) = io::duplex(8192);
            let (reader, writer) = io::split(local);
            if spawn {
                assert!(matches!(initiator.run_session_with_reset(reader, writer, None).await.unwrap(),
                    Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon)));
            } else {
                assert!(matches!(initiator.session_task_with_reset(reader, writer, None).await,
                    Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon)));
            }
            assert_eq!(peer.read(&mut [0u8; 1]).await.unwrap(), 0);
        }
        assert_eq!(creates.get(), 0);
        assert!(initiator.inner.current_session.borrow().is_none());
        {
            let mut storage = initiator.inner.storage.borrow_mut().take().unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
            assert_eq!(storage.next_target_msg_seq_num().get(), 40);
            assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(history.as_slice()));
            *initiator.inner.storage.borrow_mut() = Some(storage);
        }
        let (local, mut peer) = io::duplex(8192);
        let (reader, writer) = io::split(local);
        let task = initiator.run_session(reader, writer, None);
        let request = test_helpers::read_one_message(&mut peer, &mut Vec::new()).await;
        assert_eq!(request.header.msg_seq_num, 40);
        assert_matches!(request.try_as_admin(), Some(AdminBase::Logon(logon)) if logon.reset_seq_num_flag.is_none());
        assert_eq!(creates.get(), 1);
        initiator.disconnect().await.unwrap();
        task.await.unwrap().unwrap();
        let mut storage = initiator.inner.storage.borrow_mut().take().unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 41);
        assert_eq!(storage.next_target_msg_seq_num().get(), 40);
        assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(history.as_slice()));
    }).await;
}

fn initiator<A, H>(make_handler: A) -> Initiator<Message, InMemoryStorage, A>
where
    A: AsyncFn(&SessionContext<'_>) -> Result<H, Box<dyn StdError>> + 'static,
    H: SessionHandler<Message> + 'static,
{
    Initiator::new(
        test_helpers::default_session_id(),
        SessionSettings::default(),
        make_handler,
        |_, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
    )
    .expect("initiator")
}

#[tokio::test(start_paused = true)]
async fn idle_lifecycle_operations_preserve_storage_without_callbacks() {
    let mut storage = FailingStorage::new();
    storage.sender = nz_seq(40);
    storage.target = nz_seq(30);
    let trace = storage.trace.clone();
    let initiator = Initiator::<Message, _, _>::new(
        test_helpers::default_session_id(),
        SessionSettings::default(),
        no_callbacks,
        |_, _| Ok(storage),
    )
    .unwrap();

    assert!(!initiator.is_session_active());
    initiator
        .logout(None, Some(Cow::Borrowed(fix_str!("Window closed"))), false)
        .await
        .unwrap();
    initiator.disconnect().await.unwrap();
    timeout(Duration::from_secs(1), initiator.close())
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(1), initiator.await_session_closed())
        .await
        .unwrap();

    assert!(trace.borrow().calls.is_empty());
    let storage = initiator.inner.storage.borrow();
    let storage = storage.as_ref().unwrap();
    assert_eq!(storage.sender, nz_seq(40));
    assert_eq!(storage.target, nz_seq(30));
}

#[tokio::test(start_paused = true)]
async fn lifecycle_control_reaches_a_just_spawned_session() {
    LocalSet::new()
        .run_until(async {
            let initiator = initiator(test_helpers::make_stub_handler);
            for close in [false, true] {
                let (local, _peer) = io::duplex(8192);
                let (reader, writer) = io::split(local);
                let task = initiator.run_session(reader, writer, None);
                assert!(initiator.is_session_active());
                if close {
                    timeout(Duration::from_secs(1), initiator.close())
                        .await
                        .unwrap()
                        .unwrap();
                } else {
                    initiator.disconnect().await.unwrap();
                    timeout(Duration::from_secs(1), initiator.await_session_closed())
                        .await
                        .unwrap();
                }
                assert!(!initiator.is_session_active());
                task.await.unwrap().unwrap();
                initiator.reset_session().unwrap();
            }
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn logout_queued_before_first_poll_reaches_the_session() {
    LocalSet::new()
        .run_until(async {
            let initiator = initiator(test_helpers::make_stub_handler);
            let (local, mut peer) = io::duplex(8192);
            let (reader, writer) = io::split(local);
            let session = initiator.session_task(reader, writer, None);
            initiator
                .logout(None, Some(Cow::Borrowed(fix_str!("Window closed"))), false)
                .await
                .unwrap();
            let task = task::spawn_local(session);
            let mut buf = Vec::new();
            let opening = timeout(
                Duration::from_secs(1),
                test_helpers::read_one_message(&mut peer, &mut buf),
            )
            .await
            .unwrap();
            assert_matches!(opening.try_as_admin(), Some(AdminBase::Logon(_)));
            let logout = timeout(
                Duration::from_secs(1),
                test_helpers::read_one_message(&mut peer, &mut buf),
            )
            .await
            .unwrap();
            assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(logout))
            if logout.text.as_deref() == Some(fix_str!("Window closed")));
            initiator.close().await.unwrap();
            task.await.unwrap().unwrap();
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn closure_waiters_follow_replacement_session_until_it_is_dropped() {
    LocalSet::new()
        .run_until(async {
            let initiator = Rc::new(initiator(no_callbacks));
            let first = initiator.session_task(io::empty(), io::sink(), None);
            let waiters: Vec<_> = (0..2)
                .map(|_| {
                    let initiator = initiator.clone();
                    task::spawn_local(async move { initiator.await_session_closed().await })
                })
                .collect();
            yield_now().await;
            assert!(waiters.iter().all(|waiter| !waiter.is_finished()));

            drop(first);
            let replacement = initiator.session_task(io::empty(), io::sink(), None);
            yield_now().await;
            assert!(waiters.iter().all(|waiter| !waiter.is_finished()));
            assert!(initiator.is_session_active());

            drop(replacement);
            for waiter in waiters {
                timeout(Duration::from_secs(1), waiter)
                    .await
                    .unwrap()
                    .unwrap();
            }
            assert!(!initiator.is_session_active());
            initiator.reset_session().unwrap();
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn closure_waits_for_end_callback_and_wakes_after_completion_or_abort() {
    LocalSet::new()
        .run_until(async {
            for abort in [false, true] {
                let gate = test_helpers::GatedSessionEnd::default();
                let make_handler = {
                    let gate = gate.clone();
                    move |_: &SessionContext<'_>| future::ready(Ok(gate.clone()))
                };
                let initiator = Rc::new(initiator(make_handler));
                let task = initiator.run_session(io::empty(), io::sink(), None);
                timeout(Duration::from_secs(1), gate.entered.notified())
                    .await
                    .unwrap();
                let waiter = {
                    let initiator = initiator.clone();
                    task::spawn_local(async move { initiator.close().await })
                };
                yield_now().await;
                assert!(initiator.is_session_active());
                assert!(!waiter.is_finished());
                assert_matches!(
                    initiator.reset_session(),
                    Err(InitiatorError::SessionActive)
                );

                if abort {
                    task.abort();
                    assert_matches!(task.await, Err(error) if error.is_cancelled());
                } else {
                    gate.release.notify_one();
                    task.await.unwrap().unwrap();
                }
                timeout(Duration::from_secs(1), waiter)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert!(!initiator.is_session_active());
                initiator.reset_session().unwrap();
            }
        })
        .await;
}

/// Unsupported acceptance fails before constructing storage; the
/// acceptor-only setting has no effect on an initiator.
#[test]
fn reset_acceptance_requires_message_type_support() {
    let defaults = SessionSettings::default();
    assert!(!defaults.accept_reset_on_connect);
    assert!(!defaults.accept_reset_in_session);
    for on_connect in [false, true] {
        for in_session in [false, true] {
            let builds = Cell::new(0);
            let settings = SessionSettings {
                accept_reset_on_connect: on_connect,
                accept_reset_in_session: in_session,
                ..SessionSettings::default()
            };
            let result = Initiator::<CountedResetMessage, _, _>::new(
                test_helpers::default_session_id(),
                settings.clone(),
                test_helpers::make_stub_handler,
                |_, max| {
                    builds.set(builds.get() + 1);
                    Ok(InMemoryStorage::new(max))
                },
            );
            if in_session {
                assert!(matches!(
                    result,
                    Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon)
                ));
                assert_eq!(builds.get(), 0);
            } else {
                assert!(!result.unwrap().inner.supports_seq_num_reset);
                assert_eq!(builds.get(), 1);
            }
            let supported = Initiator::<Message, _, _>::new(
                test_helpers::default_session_id(),
                settings,
                test_helpers::make_stub_handler,
                |_, max| Ok(InMemoryStorage::new(max)),
            )
            .unwrap();
            assert!(supported.inner.supports_seq_num_reset);
        }
    }
}

/// A panic in a session task must NOT permanently lose the storage. It is
/// taken out of the `Initiator` for the duration of the task and - because
/// `storage.is_none()` doubles as the "session active" flag - a task that
/// unwinds without returning it leaves every later `connect()` answering
/// `SessionActive` with no session running, discarding the persisted
/// counters and the messages retained for resend (FIX Session Layer §4.1).
#[tokio::test]
async fn session_task_panic_restores_storage() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let initiator = Rc::new(initiator(test_helpers::make_panic_handler));

            let (peer_io, mut peer) = io::duplex(8192);
            let (reader, writer) = io::split(peer_io);

            // The initiator sends its Logon, then reads ours - which fires
            // `on_admin_msg_in` and panics.
            let logon = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
            peer.write_all(&test_helpers::serialize_message(&logon))
                .await
                .expect("write logon");

            let session = initiator.session_task(reader, writer, None);
            let waiter = {
                let initiator = initiator.clone();
                task::spawn_local(async move { initiator.await_session_closed().await })
            };
            yield_now().await;
            assert!(!waiter.is_finished());
            let handle = task::spawn_local(session);

            assert_matches!(
                handle.await,
                Err(e) if e.is_panic(),
                "session task should have panicked in the callback"
            );
            timeout(Duration::from_secs(1), waiter)
                .await
                .unwrap()
                .unwrap();
            assert!(!initiator.is_session_active());

            assert!(
                initiator.inner.storage.borrow().is_some(),
                "storage must be restored after a panic, otherwise the \
                 Initiator is permanently bricked"
            );
            assert!(
                initiator.inner.current_session.borrow().is_none(),
                "control handle must be cleared after a panic"
            );
        })
        .await;
}

/// `session_task` hands the caller a future it may never poll - a losing
/// `select!` arm, an early return elsewhere. The storage is taken when the
/// future is built, so dropping it unpolled must give the storage back;
/// otherwise the `Initiator` is bricked without a single session ever
/// having run.
#[tokio::test]
async fn dropped_unpolled_session_task_restores_storage() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let initiator = initiator(test_helpers::make_stub_handler);

            let (peer_io, _peer) = io::duplex(8192);
            let (reader, writer) = io::split(peer_io);

            let task = initiator.session_task(reader, writer, None);
            assert!(
                initiator.inner.storage.borrow().is_none(),
                "storage is taken while a session task is outstanding"
            );

            drop(task);

            assert!(
                initiator.inner.storage.borrow().is_some(),
                "dropping an unpolled session task must return the storage"
            );

            // The Initiator can reserve the session again.
            let (peer_io, _peer) = io::duplex(8192);
            let (reader, writer) = io::split(peer_io);
            let retry = initiator.session_task(reader, writer, None);
            assert!(initiator.is_session_active());
            drop(retry);
            assert!(!initiator.is_session_active());
        })
        .await;
}

/// A second task reports the reservation conflict without releasing the first.
#[tokio::test]
async fn second_session_task_while_first_outstanding_is_refused() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let initiator = initiator(test_helpers::make_stub_handler);

            let (peer_io, _peer) = io::duplex(8192);
            let (reader, writer) = io::split(peer_io);
            let task = initiator.session_task(reader, writer, None);

            let (peer_io, _peer) = io::duplex(8192);
            let (reader, writer) = io::split(peer_io);
            assert!(matches!(
                initiator.session_task(reader, writer, None).await,
                Err(InitiatorError::SessionActive)
            ));
            assert!(initiator.is_session_active());
            drop(task);
            assert!(!initiator.is_session_active());
        })
        .await;
}

/// An offline reset reopens exhausted numbering without asking the peer to
/// reset on the next Resume connection.
#[tokio::test]
async fn reset_session_reopens_the_numbering_and_is_refused_while_a_session_runs() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let initiator = initiator(test_helpers::make_stub_handler);

            {
                let mut storage = initiator.inner.storage.borrow_mut();
                let storage = storage.as_mut().expect("no session running");
                storage
                    .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
                    .unwrap();
            }

            initiator.reset_session().expect("no session is running");

            {
                let storage = initiator.inner.storage.borrow();
                let storage = storage.as_ref().expect("no session running");
                assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
            }

            let (peer_io, _peer) = io::duplex(8192);
            let (reader, writer) = io::split(peer_io);
            let _task = initiator.session_task(reader, writer, None);

            assert!(matches!(
                initiator.reset_session(),
                Err(InitiatorError::SessionActive)
            ));
        })
        .await;
}
