use std::{assert_matches, num::NonZeroU8, time::Duration};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::Int,
    fix_str,
    message::SessionMessage,
};
use tokio::{io, io::AsyncWriteExt, sync::mpsc::error::TryRecvError, task, task::LocalSet, time};

use super::{
    harness::{TestEvent, build_harness, build_harness_with_settings},
    wire::{build_peer_logon, logon_handshake, read_lone_message},
};
use crate::{
    application::DisconnectReason, io::ControlMsg, test_helpers, test_helpers::read_one_message,
};

#[tokio::test(start_paused = true)]
async fn heartbeat_timeout_sends_heartbeat() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Advance time past the output deadline (30s heartbeat interval)
            // so engine fires on_output_timeout -> Heartbeat.
            time::advance(Duration::from_secs(31)).await;

            // Read the Heartbeat (outbound only - no inbound event recorded)
            let heartbeat = read_lone_message(&mut client_io).await;
            assert_eq!(
                SessionMessage::msg_type(&*heartbeat),
                MsgTypeBase::Heartbeat
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

/// A garbled message must be disregarded (FIX Session Layer Section 4.5.2)
/// and must NOT reset the keep-alive input deadline - otherwise a
/// garbled-only stream suppresses the input-timeout TestRequest /
/// heartbeat-timeout escalation forever (Section 4.5.1).
///
/// The negotiated interval is 1s, so the input deadline is 1.2s (1.2x).
/// A garbled frame is delivered right after the first output Heartbeat
/// (1s); the input-timeout TestRequest must still be the next message on
/// the wire, at 1.2s. A garbled-induced reset at 1s would push it to
/// 2.2s, behind the 2s output Heartbeat.
///
/// The paused clock only advances while every task is idle, so the
/// garbled frame is processed before the wheel moves on, and the next
/// timer to fire is the one the assertion is about.
#[tokio::test(start_paused = true)]
async fn garbled_input_does_not_reset_input_deadline() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 1); // HeartBtInt = 1s
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            // Logon handshake. The input deadline is armed at 1.2s from here.
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );
            let mut rbuf: Vec<u8> = Vec::new();
            let response = read_one_message(&mut client_io, &mut rbuf).await;
            assert_eq!(SessionMessage::msg_type(&*response), MsgTypeBase::Logon);
            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);
            let start = time::Instant::now();

            // Drain the first output Heartbeat (fires at the 1s interval);
            // the input deadline is still 1.2s.
            let hb = read_one_message(&mut client_io, &mut rbuf).await;
            assert_eq!(SessionMessage::msg_type(&*hb), MsgTypeBase::Heartbeat);
            assert_eq!(start.elapsed(), Duration::from_secs(1));

            // A garbled frame arrives now (1s). It must be disregarded and
            // must NOT push the input deadline.
            client_io
                .write_all(&build_garbled_bytes())
                .await
                .expect("write garbled");

            // The next message is the input-timeout TestRequest at 1.2s
            // (deadline unmoved) - not the 2s output Heartbeat.
            let msg = time::timeout(
                Duration::from_secs(10),
                read_one_message(&mut client_io, &mut rbuf),
            )
            .await
            .expect("timed out waiting for TestRequest");
            assert_eq!(
                SessionMessage::msg_type(&*msg),
                MsgTypeBase::TestRequest,
                "garbled input must not have pushed the input deadline"
            );
            assert_eq!(start.elapsed(), Duration::from_millis(1200));

            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );
            session_task.await.unwrap();
        })
        .await;
}

/// `HeartBtInt(108)` is an `Int`, so a peer can propose `i64::MAX`
/// seconds, and the acceptor adopts the offered value verbatim - whether
/// it is acceptable is the application's call (Section 4.3.4). Every deadline
/// reset in the IO loop then computes `now + interval`, which `Instant`
/// panics on rather than saturating.
///
/// The first reset to run is the input deadline's, on the peer's next
/// message: the Logon response is flushed before the loop, so nothing has
/// set `any_bytes_written` yet, and the output deadline never fires.
#[tokio::test]
async fn absurd_heart_bt_int_does_not_panic_the_loop() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, Int::MAX);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Drives the input-deadline reset.
            client_io
                .write_all(&test_helpers::heartbeat_bytes(2))
                .await
                .unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
            );

            // The loop survived, so it still answers the Logout.
            client_io
                .write_all(&test_helpers::logout_bytes(3))
                .await
                .unwrap();
            let response = read_lone_message(&mut client_io).await;
            assert_eq!(SessionMessage::msg_type(&*response), MsgTypeBase::Logout);
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logout)
            );
            client_io.shutdown().await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::RemoteRequestedLogout)
            );

            session_task.await.unwrap();
        })
        .await;
}

/// With `verify_test_request_id`, a peer that emits Heartbeats
/// WITHOUT the matching TestReqID must still be disconnected. The
/// TestRequest-response escalation must run on a timer independent of
/// inbound resets, so a stream of non-matching heartbeats cannot defeat
/// the Logout+disconnect (FIX Session Layer Section 4.5.5; Testcase 185 - the
/// matching Heartbeat "may not be the next message received").
///
/// HeartBtInt=1s -> input deadline 1.2s; `auto_disconnect_after_no_heartbeat=1`,
/// so the escalation lands at 2.4s.
#[tokio::test(start_paused = true)]
async fn non_matching_heartbeats_do_not_defeat_test_request_escalation() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut settings = test_helpers::default_session_settings();
            settings.auto_disconnect_after_no_heartbeat = const { NonZeroU8::new(1).unwrap() };
            let harness = build_harness_with_settings(settings);

            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 1); // HeartBtInt = 1s
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            // Logon handshake.
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );
            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);
            let start = time::Instant::now();

            // Peer: stay idle 1.3s so the server's 1.2s input deadline
            // fires and it sends TestRequest probe #1, then emit Heartbeats
            // WITHOUT a matching TestReqID every 300ms. Each resets the
            // inbound idle timer but must NOT reset the independent grace
            // deadline that drives escalation.
            let client_task = task::spawn_local(async move {
                time::sleep(Duration::from_millis(1300)).await;
                let mut seq = 2;
                loop {
                    if client_io
                        .write_all(&test_helpers::heartbeat_bytes(seq))
                        .await
                        .is_err()
                    {
                        break; // server disconnected - escalation fired
                    }
                    seq += 1;
                    time::sleep(Duration::from_millis(300)).await;
                }
            });

            // The escalation must terminate the session despite the
            // non-matching heartbeats. Buggy code resets its only timer on
            // every heartbeat and never escalates, so SessionEnd never
            // arrives and this bounded wait fails.
            let reason = time::timeout(Duration::from_secs(8), async {
                loop {
                    // Ignore AdminMsgIn(Heartbeat) etc. - only SessionEnd
                    // ends the wait.
                    if let TestEvent::SessionEnd(reason) =
                        events_rx.recv().await.expect("events channel closed")
                    {
                        break reason;
                    }
                }
            })
            .await
            .expect(
                "session must escalate to Logout+disconnect despite \
                     non-matching heartbeats",
            );
            assert_eq!(reason, DisconnectReason::HeartbeatTimeout);
            // Probe at 1.2s, grace deadline one input timeout later.
            assert_eq!(start.elapsed(), Duration::from_millis(2400));

            client_task.abort();
            session_task.await.unwrap();
        })
        .await;
}

/// `HeartBtInt(108)=0` disables the keep-alive timers (FIX Transport
/// Section 5.1): however long the wire stays quiet, an established session sends
/// no Heartbeat or TestRequest of its own and does not end - which is to
/// say a dead peer goes undetected in this mode (`TODO.md` item 51). The
/// peer's own probes are still answered.
#[tokio::test(start_paused = true)]
async fn heart_bt_int_zero_keeps_the_wire_silent_and_the_session_up() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 0);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            time::advance(Duration::from_secs(3600)).await;

            // Nothing armed on the session side, so the paused clock has
            // only this timeout to advance to.
            let mut buf = Vec::new();
            let leaked = time::timeout(
                Duration::from_secs(1),
                read_one_message(&mut client_io, &mut buf),
            )
            .await;
            assert!(leaked.is_err(), "keep-alive traffic with HeartBtInt=0");
            assert!(!session_task.is_finished());
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Empty));

            // A probe from the peer is answered regardless of the interval.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::test_request(2, fix_str!("probe")),
                ))
                .await
                .unwrap();
            let msg = read_one_message(&mut client_io, &mut buf).await;
            let AdminBase::Heartbeat(ref hb) = SessionMessage::try_as_admin(&*msg)
                .expect("TestRequest is answered with a Heartbeat")
            else {
                panic!("expected Heartbeat");
            };
            assert_eq!(hb.test_req_id.as_deref(), Some(fix_str!("probe")));
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::TestRequest)
            );

            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );
            session_task.await.unwrap();
        })
        .await;
}

/// A complete, well-framed message with a corrupted `Checksum(10)` - so
/// `raw_message` returns `InvalidChecksum`, surfaced as a garbled
/// `DeserializeError`. Used to drive the garbled-input path.
fn build_garbled_bytes() -> Vec<u8> {
    let mut bytes = test_helpers::logon_bytes(2, 30);
    // The trailing field is "10=DDD\x01"; flip the last digit so the
    // checksum no longer matches.
    let last_digit = bytes.len() - 2;
    bytes[last_digit] = if bytes[last_digit] == b'9' {
        b'0'
    } else {
        bytes[last_digit] + 1
    };
    bytes
}
