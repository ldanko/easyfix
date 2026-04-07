use std::{
    assert_matches,
    cell::Cell,
    io::{Error, ErrorKind},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    deserializer::raw_message,
    message::{HeaderAccess, SessionMessage},
};
use easyfix_test_messages::{Body, Message};
use tokio::{
    io,
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc::error::TryRecvError,
    task,
    task::LocalSet,
    time,
};

use super::{
    harness::{TestEvent, build_harness, build_harness_with_settings},
    wire::{build_peer_logon, logon_handshake, read_lone_message},
};
use crate::{
    application::DisconnectReason,
    io::{ControlMsg, InputStream, SessionOpening, session_loop},
    test_helpers,
    test_helpers::{DEFAULT_MAX_MESSAGE_SIZE, as_admin, read_one_message},
};

#[tokio::test(start_paused = true)]
async fn refused_encrypt_method_flushes_output_before_disconnect() {
    LocalSet::new().run_until(async {
        for method in ["1", "99"] {
            let harness = build_harness();
            let (server, mut peer) = io::duplex(8192);
            let (reader, writer) = io::split(server);
            let bytes = test_helpers::logon_bytes_with_encrypt_method(1, method, false);
            let first = Message::from_raw_message(raw_message(&bytes).unwrap().1);
            let mut storage = harness.storage;
            let mut events = harness.events_rx;
            let started = time::Instant::now();
            session_loop(
                SessionOpening::FirstMessage(first),
                InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE),
                writer,
                harness.engine,
                &mut storage,
                harness.app,
                harness.sender,
                harness.app_rx,
                harness.control_rx,
            ).await;
            assert_eq!(time::Instant::now(), started, "no Logout acknowledgement wait");
            let mut buffer = Vec::new();
            let logout_seq = if method == "99" {
                let reject = read_one_message(&mut peer, &mut buffer).await;
                assert_matches!(as_admin(&reject), AdminBase::Reject(r)
                    if r.session_reject_reason == Some(SessionRejectReasonBase::ValueIsIncorrect.into())
                        && r.ref_tag_id == Some(98) && r.ref_seq_num == 1);
                assert_eq!(reject.header.msg_seq_num, 1);
                2
            } else {
                1
            };
            let logout = read_one_message(&mut peer, &mut buffer).await;
            assert_matches!(as_admin(&logout), AdminBase::Logout(l) if l.text.is_some());
            assert_eq!(logout.header.msg_seq_num, logout_seq);
            assert!(buffer.is_empty());
            assert_eq!(peer.read(&mut [0u8; 1]).await.unwrap(), 0);
            let mut session_end = None;
            while let Some(event) = events.recv().await {
                assert!(!matches!(event, TestEvent::AdminMsgIn(_) | TestEvent::SessionReady));
                if let TestEvent::SessionEnd(reason) = event {
                    assert!(session_end.replace(reason).is_none());
                }
            }
            assert_eq!(session_end, Some(DisconnectReason::InvalidLogonState));
        }
    }).await;
}

enum PendingWriteFailure {
    Error,
    Stall(Pin<Box<time::Sleep>>),
}

/// Fail once after writing one byte, then allow subsequent writes to succeed.
struct FailAfterPrefix<W> {
    inner: W,
    armed: Rc<Cell<bool>>,
    stall_for: Option<Duration>,
    failure: Option<PendingWriteFailure>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for FailAfterPrefix<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.armed.replace(false) {
            let result = Pin::new(&mut this.inner).poll_write(cx, &buf[..1]);
            match result {
                Poll::Pending => this.armed.set(true),
                Poll::Ready(Ok(1)) => {
                    this.failure = Some(match this.stall_for {
                        Some(delay) => PendingWriteFailure::Stall(Box::pin(time::sleep(delay))),
                        None => PendingWriteFailure::Error,
                    });
                }
                _ => panic!("failed to write the injected partial frame: {result:?}"),
            }
            return result;
        }
        if let Some(failure) = &mut this.failure {
            match failure {
                PendingWriteFailure::Error => {
                    this.failure = None;
                    return Poll::Ready(Err(Error::new(ErrorKind::BrokenPipe, "injected failure")));
                }
                PendingWriteFailure::Stall(deadline) => {
                    if deadline.as_mut().poll(cx).is_pending() {
                        return Poll::Pending;
                    }
                    this.failure = None;
                }
            }
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[tokio::test(start_paused = true)]
async fn terminating_write_failure_stops_output_and_skips_peer_close_wait() {
    LocalSet::new()
        .run_until(async {
            for app_logout in [true, false] {
                for write_timeout in [false, true] {
                    let budget = Duration::from_secs(1);
                    let mut settings = test_helpers::default_session_settings();
                    settings.write_timeout = budget;
                    settings.auto_disconnect_after_no_logout = Duration::from_secs(10);
                    let mut harness = build_harness_with_settings(settings);
                    harness.app.send_then_logout = app_logout;
                    let (server, mut peer) = io::duplex(8192);
                    let (reader, writer) = io::split(server);
                    let armed = Rc::new(Cell::new(false));
                    let writer = FailAfterPrefix {
                        inner: writer,
                        armed: armed.clone(),
                        // Recover just after the first write times out. A
                        // second write would succeed within its own budget.
                        stall_for: write_timeout.then_some(budget + Duration::from_millis(1)),
                        failure: None,
                    };
                    let (session_task, mut events, _control) =
                        harness.spawn_acceptor(reader, writer, build_peer_logon(1, 30));
                    logon_handshake(&mut peer, &mut events).await;

                    armed.set(true);
                    let started_at = time::Instant::now();
                    let request = if app_logout {
                        test_helpers::serialize_message(&test_helpers::new_order_single(2))
                    } else {
                        test_helpers::logout_bytes(2)
                    };
                    peer.write_all(&request).await.unwrap();
                    if app_logout {
                        assert_matches!(events.recv().await.unwrap(), TestEvent::AppMsgIn);
                    } else {
                        assert_matches!(
                            events.recv().await.unwrap(),
                            TestEvent::AdminMsgIn(MsgTypeBase::Logout)
                        );
                    }
                    let expected_reason = if app_logout {
                        DisconnectReason::ApplicationForcedDisconnect
                    } else {
                        DisconnectReason::IoError
                    };
                    assert_matches!(
                        events.recv().await.unwrap(),
                        TestEvent::SessionEnd(reason) if reason == expected_reason
                    );
                    assert_eq!(
                        started_at.elapsed(),
                        if write_timeout {
                            budget
                        } else {
                            Duration::ZERO
                        },
                        "no second write budget or peer-close wait is allowed"
                    );
                    session_task.await.unwrap();

                    // Keep the peer open until the session ends. The only
                    // output is the prefix written before the failure.
                    let mut received = Vec::new();
                    peer.read_to_end(&mut received).await.unwrap();
                    assert_eq!(received, b"8");
                    assert_matches!(events.try_recv(), Err(TryRecvError::Disconnected));
                }
            }
        })
        .await;
}

#[tokio::test]
async fn acceptor_logon_handshake() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            // Admin callback fires while the first Logon is processed,
            // before the Logon response is flushed and SessionReady runs.
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );

            // Client reads Logon response
            let response = read_lone_message(&mut client_io).await;
            assert_eq!(SessionMessage::msg_type(&*response), MsgTypeBase::Logon);
            assert_eq!(response.sender_comp_id().as_utf8(), "SENDER");
            assert_eq!(response.target_comp_id().as_utf8(), "TARGET");

            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);

            // Trigger disconnect
            control_tx.send(ControlMsg::Disconnect).await.unwrap();

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );

            session_task.await.unwrap();

            // No further events should have been emitted.
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

#[tokio::test]
async fn initiator_logon_handshake() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_initiator(server_reader, server_writer);

            // Client reads the Logon request sent by the initiator
            let logon_request = read_lone_message(&mut client_io).await;
            assert_eq!(
                SessionMessage::msg_type(&*logon_request),
                MsgTypeBase::Logon
            );

            // SessionReady fires after Logon request is flushed, before
            // any peer response arrives.
            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);

            // Client writes a Logon response back to the session
            client_io
                .write_all(&test_helpers::logon_bytes(1, 30))
                .await
                .unwrap();

            // Session processes Logon response and fires the admin callback.
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );

            // Trigger disconnect
            control_tx.send(ControlMsg::Disconnect).await.unwrap();

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );

            session_task.await.unwrap();

            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// A peer's Logout is acknowledged, and then the connection is the
/// peer's to close: the session stays up until the peer's EOF and ends
/// with `RemoteRequestedLogout` only then (FIX Session Layer Section 4.6,
/// Figure 9; Test Cases Scenario 13(b)). Whatever the peer sends in the
/// meantime is not read as FIX - the exchange is complete.
#[tokio::test]
async fn peer_logout_closes_session_gracefully() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Client sends Logout (seq 2)
            client_io
                .write_all(&test_helpers::logout_bytes(2))
                .await
                .unwrap();

            // Server should send Logout response
            let logout_response = read_lone_message(&mut client_io).await;
            assert_eq!(
                SessionMessage::msg_type(&*logout_response),
                MsgTypeBase::Logout
            );

            // Inbound Logout fires AdminMsgIn before the engine reacts
            // (sends Logout response). The application could decline at
            // this point - `TestApp` always Accepts.
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logout)
            );

            // The session has not ended: the close is the peer's move.
            // A stray Heartbeat after the exchange is discarded unread.
            client_io
                .write_all(&test_helpers::heartbeat_bytes(3))
                .await
                .unwrap();
            task::yield_now().await;
            assert!(!session_task.is_finished());
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Empty));

            // The peer closes; that is what ends the session.
            client_io.shutdown().await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::RemoteRequestedLogout)
            );

            session_task.await.unwrap();

            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// A peer that never closes after its Logout was acknowledged is cut off
/// once `auto_disconnect_after_no_logout` expires, with the reason naming
/// the peer's failure (Test Cases Scenario 13(b): "If max exceeded,
/// disconnect and generate an error condition").
#[tokio::test(start_paused = true)]
async fn peer_logout_without_close_ends_with_the_remote_logout_timeout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let budget = test_helpers::default_session_settings().auto_disconnect_after_no_logout;
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            client_io
                .write_all(&test_helpers::logout_bytes(2))
                .await
                .unwrap();
            let logout_response = read_lone_message(&mut client_io).await;
            assert_eq!(
                SessionMessage::msg_type(&*logout_response),
                MsgTypeBase::Logout
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logout)
            );
            let acknowledged_at = time::Instant::now();

            // The peer holds the connection open; the paused clock jumps
            // to the only armed timer, the close deadline.
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::RemoteRequestedLogoutTimeout)
            );
            assert_eq!(acknowledged_at.elapsed(), budget);
            session_task.await.unwrap();

            // Nothing but the acknowledgement went out.
            let mut rest = Vec::new();
            client_io.read_to_end(&mut rest).await.unwrap();
            assert!(
                rest.is_empty(),
                "unexpected bytes after the Logout: {rest:?}"
            );
        })
        .await;
}

/// An application that stages a message and *then* returns
/// `InputAction::Logout { disconnect: true }` staged that message first,
/// so it must reach the peer first. `fix_service` answers an
/// anti-flooding penalty exactly this way: a BusinessMessageReject
/// explaining the breach, then the Logout that enacts it.
///
/// The two travel different queues - the app send lands in `app_rx`,
/// drained by the event select, while the engine's Logout lands in
/// `admin_output`, flushed at the top of the loop - and the disconnect
/// check sits between them. Without the terminating-iteration drain the
/// Logout overtakes the staged message, which then goes to
/// `store_for_resend` and waits for a `ResendRequest` that a disconnected
/// peer will never send.
#[tokio::test]
async fn app_message_staged_before_logout_reaches_peer_first() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut harness = build_harness();
            harness.app.send_then_logout = true;
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Drives `on_app_msg_in`, which stages a message and asks for
            // a disconnecting Logout in the same dispatch.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::new_order_single(2),
                ))
                .await
                .unwrap();

            let mut buf = Vec::new();
            let staged = read_one_message(&mut client_io, &mut buf).await;
            assert_matches!(
                *staged.body,
                Body::NewOrderSingle(_),
                "the staged app message must precede the Logout it was staged before"
            );

            let logout = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);

            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::AppMsgIn);
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::ApplicationForcedDisconnect)
            );

            session_task.await.unwrap();

            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// A locally requested Logout that gets no acknowledgement ends the
/// session with `LocalRequestedLogoutTimeout` once `auto_disconnect_after_no_logout`
/// expires (FIX Session Layer Section 4.6.2) - not with the catch-all
/// `Disconnected`.
#[tokio::test(start_paused = true)]
async fn logout_without_response_ends_with_logout_timeout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let logout_timeout =
                test_helpers::default_session_settings().auto_disconnect_after_no_logout;
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Local logout request; the peer never acknowledges.
            control_tx
                .send(ControlMsg::Logout {
                    session_status: None,
                    text: None,
                })
                .await
                .unwrap();
            let logout = read_lone_message(&mut client_io).await;
            assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);

            // Let the logout deadline expire.
            time::advance(logout_timeout + Duration::from_secs(1)).await;

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::LocalRequestedLogoutTimeout)
            );

            session_task.await.unwrap();

            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// An acceptor whose application answers the first `Logon<A>` with
/// `InputAction::Reject` stays in `LogonState::Idle`: the reject path sets
/// no `should_disconnect` and performs no state transition, so the session
/// enters the main loop pre-handshake. Every other deadline there is
/// unavailable to it - the keep-alive pair is held back until the session
/// is logged on, and the logout deadline needs a `Logout<5>` nobody sent -
/// so the Logon deadline is the only thing standing between a peer that
/// then goes quiet and a session task parked forever, holding its storage
/// and never reaching `on_session_end`.
///
/// The 5s guard is what turns that regression into a failure instead of a
/// hang: with no deadline armed the paused clock has nothing to advance to.
#[tokio::test(start_paused = true)]
async fn acceptor_gives_up_when_the_rejected_handshake_stalls() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut settings = test_helpers::default_session_settings();
            settings.auto_disconnect_after_no_logon_response = Duration::from_secs(3);
            let mut harness = build_harness_with_settings(settings);
            harness.app.reject_admin = true;

            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );
            // A Reject, not the Logon acknowledgement - the handshake never
            // completes.
            let answer = read_lone_message(&mut client_io).await;
            assert_eq!(SessionMessage::msg_type(&*answer), MsgTypeBase::Reject);
            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);

            // The peer says nothing further.
            let end = time::timeout(Duration::from_secs(5), events_rx.recv())
                .await
                .expect("session task parked with no deadline armed")
                .unwrap();
            assert_matches!(end, TestEvent::SessionEnd(DisconnectReason::LogonTimeout));

            session_task.await.unwrap();
        })
        .await;
}

/// A first `Logon<A>` the engine refuses ends the connection before there
/// is a session to announce. The application sees the Logon in
/// `on_admin_msg_in`, the Scenario 1S(d) Reject and Logout go on the wire,
/// and then `on_session_end` runs with no `on_session_ready` before it -
/// no `Sender` is ever handed over. This is the contract
/// `Application::on_session_end` documents; the test pins it so the
/// callback pairing an application may rely on stays what the doc says.
#[tokio::test]
async fn refused_first_logon_ends_without_session_ready() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            // A negative HeartBtInt(108) fails the engine's own check on
            // the first Logon, after the application has seen it.
            let first_msg = build_peer_logon(1, -1);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            session_task.await.unwrap();

            let mut rbuf = Vec::new();
            let reject = read_one_message(&mut client_io, &mut rbuf).await;
            assert_eq!(SessionMessage::msg_type(&*reject), MsgTypeBase::Reject);
            let logout = read_one_message(&mut client_io, &mut rbuf).await;
            assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::InvalidLogonState)
            );
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}
