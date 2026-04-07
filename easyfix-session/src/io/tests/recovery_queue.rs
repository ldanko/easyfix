use std::{assert_matches, num::NonZeroUsize};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    fix_str,
    message::{HeaderAccess, SessionMessage},
};
use easyfix_test_messages::Body;
use tokio::{
    io,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc::error::TryRecvError,
    task::LocalSet,
    time,
};

use super::{
    harness::{TestEvent, build_harness, build_harness_with_settings},
    wire::{build_peer_logon, logon_handshake},
};
use crate::{
    application::DisconnectReason, io::ControlMsg, test_helpers, test_helpers::read_one_message,
};

/// A Logout that arrives with a too-high `MsgSeqNum(34)` is parked in the
/// out-of-order queue behind a `ResendRequest<2>`; once the gap closes it
/// is re-dispatched by the queued-message drain rather than by the input
/// arm of the event select. The Logout response it stages must still reach
/// the peer - the session owes an answer before closing regardless of
/// which path processed the request (FIX Session Layer Section 4.6).
///
/// The drain runs *after* the top-of-loop flush, so without an explicit
/// flush on the disconnect path the response dies with the socket and the
/// peer sees a bare TCP close.
#[tokio::test]
async fn queued_logout_response_reaches_peer() {
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

            // Client sends Logout at seq 3 while seq 2 is still expected -
            // the engine queues it and asks for the gap to be filled.
            client_io
                .write_all(&test_helpers::logout_bytes(3))
                .await
                .unwrap();

            let mut buf = Vec::new();
            let resend_request = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*resend_request),
                MsgTypeBase::ResendRequest
            );

            // Client closes the gap; the queued Logout is now due.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::new_order_single(2),
                ))
                .await
                .unwrap();

            // The Logout response must be on the wire before the session
            // settles into waiting for the peer's close.
            let logout_response = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*logout_response),
                MsgTypeBase::Logout
            );

            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::AppMsgIn);
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

            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// A queued TestRequest is answered under the numbering in effect when
/// it was received, before a fresh peer Logon resets the session.
#[tokio::test(start_paused = true)]
async fn queued_test_request_before_a_peer_reset_logon_keeps_old_numbering() {
    LocalSet::new()
        .run_until(async {
            let settings = test_helpers::default_session_settings();
            assert_eq!(settings.queued_batch_size.get(), 1);
            let harness = build_harness_with_settings(settings);
            let (server_io, mut client_io) = io::duplex(8192);
            let (reader, writer) = io::split(server_io);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(reader, writer, build_peer_logon(1, 30));
            logon_handshake(&mut client_io, &mut events_rx).await;
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::test_request(4, fix_str!("BEFORE-RESET")),
                ))
                .await
                .unwrap();
            let mut wire = Vec::new();
            let resend = read_one_message(&mut client_io, &mut wire).await;
            assert_eq!(resend.header.msg_seq_num, 2);
            assert_matches!(test_helpers::as_admin(&resend), AdminBase::ResendRequest(rr)
            if rr.begin_seq_no == 2 && rr.end_seq_no == 3);

            // One write places the fresh reset behind the gap-closing input
            // in the session's read buffer when the queued request drains.
            let mut input = Vec::new();
            for seq in [2, 3] {
                let mut replay = test_helpers::heartbeat(seq, None);
                replay.header.poss_dup_flag = Some(true);
                replay.header.orig_sending_time = Some(replay.header.sending_time);
                input.extend(test_helpers::serialize_message(&replay));
            }
            input.extend(test_helpers::serialize_message(
                &test_helpers::logon_with_options(
                    1,
                    fix_str!("TARGET"),
                    fix_str!("SENDER"),
                    30,
                    Some(true),
                    None,
                ),
            ));
            client_io.write_all(&input).await.unwrap();
            let heartbeat = read_one_message(&mut client_io, &mut wire).await;
            assert_eq!(heartbeat.header.msg_seq_num, 3);
            assert_matches!(test_helpers::as_admin(&heartbeat), AdminBase::Heartbeat(hb)
            if hb.test_req_id.as_deref() == Some(fix_str!("BEFORE-RESET")));
            let ack = read_one_message(&mut client_io, &mut wire).await;
            assert_eq!(ack.header.msg_seq_num, 1);
            assert_matches!(test_helpers::as_admin(&ack), AdminBase::Logon(logon)
            if logon.reset_seq_num_flag == Some(true));
            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            session_task.await.unwrap();
        })
        .await;
}

/// A queued admin response is flushed without a new peer message,
/// application send, control, or timer wakeup.
#[tokio::test(start_paused = true)]
async fn admin_reply_from_the_queued_drain_goes_out_without_waiting_for_an_event() {
    LocalSet::new()
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (reader, writer) = io::split(server_io);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(reader, writer, build_peer_logon(1, 30));
            logon_handshake(&mut client_io, &mut events_rx).await;
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::test_request(4, fix_str!("QUEUED")),
                ))
                .await
                .unwrap();
            let mut wire = Vec::new();
            let resend = read_one_message(&mut client_io, &mut wire).await;
            assert_matches!(test_helpers::as_admin(&resend), AdminBase::ResendRequest(_));
            let mut input = test_helpers::heartbeat_bytes(2);
            input.extend(test_helpers::heartbeat_bytes(3));
            let before = time::Instant::now();
            client_io.write_all(&input).await.unwrap();
            let response = read_one_message(&mut client_io, &mut wire).await;
            assert_eq!(
                time::Instant::now(),
                before,
                "queued response waited for a timer"
            );
            assert_eq!(response.header.msg_seq_num, 3);
            assert_matches!(test_helpers::as_admin(&response), AdminBase::Heartbeat(hb)
            if hb.test_req_id.as_deref() == Some(fix_str!("QUEUED")));
            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            session_task.await.unwrap();
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn queued_input_waits_for_the_replay_started_by_the_gap_closing_message() {
    LocalSet::new()
        .run_until(async {
            let settings = test_helpers::default_session_settings();
            assert_eq!(settings.resend_batch_size.get(), 1);
            let harness = build_harness_with_settings(settings);
            let sender = harness.sender.clone();
            let (server_io, mut client_io) = io::duplex(8192);
            let (reader, writer) = io::split(server_io);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(reader, writer, build_peer_logon(1, 30));
            logon_handshake(&mut client_io, &mut events_rx).await;

            let mut wire = Vec::new();
            for seq in 2..=6 {
                sender
                    .send(test_helpers::new_order_single_with_empty_header())
                    .unwrap();
                let order = read_one_message(&mut client_io, &mut wire).await;
                assert_eq!(order.header.msg_seq_num, seq);
                assert_eq!(order.header.poss_dup_flag, None);
            }

            // Park an input that will produce an observable reply once
            // dispatched, then close its gap with a replay request.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::test_request(3, fix_str!("AFTER-REPLAY")),
                ))
                .await
                .unwrap();
            let request = read_one_message(&mut client_io, &mut wire).await;
            assert_eq!(request.header.msg_seq_num, 7);
            assert_matches!(test_helpers::as_admin(&request), AdminBase::ResendRequest(rr)
                if rr.begin_seq_no == 2 && rr.end_seq_no == 2);
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Empty));
            let before = time::Instant::now();
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::resend_request(2, 2, 7),
                ))
                .await
                .unwrap();

            // Recovery includes every requested order and the trailing
            // session-message gap-fill (Session Layer 4.8.5, Scenario 8).
            for seq in 2..=6 {
                let replay = read_one_message(&mut client_io, &mut wire).await;
                assert_eq!(replay.header.msg_seq_num, seq);
                assert_eq!(replay.header.poss_dup_flag, Some(true));
                assert_eq!(SessionMessage::msg_type(&*replay).as_bytes(), b"D");
            }
            let gap_fill = read_one_message(&mut client_io, &mut wire).await;
            assert_eq!(gap_fill.header.msg_seq_num, 7);
            assert_eq!(gap_fill.header.poss_dup_flag, Some(true));
            assert_matches!(test_helpers::as_admin(&gap_fill), AdminBase::SequenceReset(sr)
                if sr.gap_fill_flag == Some(true) && sr.new_seq_no == 8);
            let heartbeat = read_one_message(&mut client_io, &mut wire).await;
            assert_eq!(heartbeat.header.msg_seq_num, 8);
            assert_eq!(heartbeat.header.poss_dup_flag, None);
            assert_matches!(test_helpers::as_admin(&heartbeat), AdminBase::Heartbeat(hb)
                if hb.test_req_id.as_deref() == Some(fix_str!("AFTER-REPLAY")));
            assert_eq!(time::Instant::now(), before);
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::TestRequest)
            );
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Empty));
            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            session_task.await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// Once a gap closes, the out-of-order queue must drain all the way to the
/// first genuinely missing sequence number, however many messages that
/// takes. `queued_batch_size` bounds the work done per loop iteration, not
/// the total - so with more than that many messages parked, the loop has
/// to iterate again instead of blocking on the event select with the queue
/// still holding the message it is next expecting.
///
/// Leaving them parked desynchronizes the session: the peer's next
/// in-sequence message reads as too-high and draws a `ResendRequest` for
/// messages it has already sent (FIX Session Layer Section 4.8.2, park-and-
/// request).
#[tokio::test]
async fn queue_drains_past_batch_size_before_blocking() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // The default `queued_batch_size` is 1, so two parked messages
            // need a second drain iteration to clear.
            let settings = test_helpers::default_session_settings();
            assert_eq!(settings.queued_batch_size.get(), 1);
            let harness = build_harness_with_settings(settings);
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Two Heartbeats past the expected seq 2 - both park.
            client_io
                .write_all(&test_helpers::heartbeat_bytes(3))
                .await
                .unwrap();
            client_io
                .write_all(&test_helpers::heartbeat_bytes(4))
                .await
                .unwrap();

            let mut buf = Vec::new();
            let resend_request = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*resend_request),
                MsgTypeBase::ResendRequest
            );

            // Close the gap. Seq 2 is processed live; 3 and 4 must both
            // drain out of the queue, leaving the session expecting 5.
            client_io
                .write_all(&test_helpers::heartbeat_bytes(2))
                .await
                .unwrap();

            // A TestRequest at the next in-sequence number is the probe:
            // answered with a Heartbeat only if the queue drained fully.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::test_request(5, fix_str!("TR1")),
                ))
                .await
                .unwrap();

            let response = read_one_message(&mut client_io, &mut buf).await;
            let admin = test_helpers::as_admin(&response);
            assert_matches!(
                admin,
                AdminBase::Heartbeat(hb) if hb.test_req_id.as_deref() == Some(fix_str!("TR1")),
                "expected the TestRequest to be in sequence; a ResendRequest here \
                     means seq 4 was still parked"
            );

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::TestRequest)
            );

            drop(client_io);
            session_task.await.unwrap();
        })
        .await;
}

/// The same staging, reached through the out-of-order queue instead of
/// the input arm - and the application cannot tell the difference, so
/// neither may the result.
///
/// The queued-message drain is the one place that decides to disconnect
/// mid-iteration, after the terminating block at the top of the loop has
/// already passed. Unless it hands over to that block, the staged message
/// is routed to `store_for_resend`, consuming a sequence number for a
/// message only a `ResendRequest` from the peer we just disconnected
/// could ever deliver.
#[tokio::test]
async fn app_message_staged_from_queued_drain_reaches_peer_first() {
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

            // Seq 3 while 2 is expected: parked in the queue, no
            // `on_app_msg_in` yet, and a ResendRequest goes out.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::new_order_single(3),
                ))
                .await
                .unwrap();
            let mut buf = Vec::new();
            let rr = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*rr), MsgTypeBase::ResendRequest);

            // Closing the gap lets the queued drain re-dispatch seq 3,
            // and only then does the application stage its answer.
            client_io
                .write_all(&test_helpers::heartbeat_bytes(2))
                .await
                .unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
            );

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

            // Nothing beyond the one re-dispatched message and the session
            // end: a second parked message re-dispatched by the queued
            // drain, or a duplicated `on_session_end`, would show up here.
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// Once a re-dispatched message ends the session, the rest of the queue
/// stays parked: the application sees no further message, and the wire
/// carries nothing past the Logout. With `queued_batch_size` above one
/// the drain would otherwise keep going within the same iteration and
/// deliver a message to a session that is already over.
///
/// The session-ending message is a peer `Logout<5>` - a re-dispatch that
/// advances the target counter past itself, so the message behind it is
/// genuinely next in line rather than parked behind an unconsumed seq num.
#[tokio::test]
async fn queued_drain_stops_at_the_message_that_ends_the_session() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut settings = test_helpers::default_session_settings();
            settings.queued_batch_size = const { NonZeroUsize::new(2).unwrap() };
            let harness = build_harness_with_settings(settings);
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Logout at seq 3 and a Heartbeat at seq 4 while 2 is expected:
            // both park, one batch's worth, and a ResendRequest goes out.
            client_io
                .write_all(&test_helpers::logout_bytes(3))
                .await
                .unwrap();
            client_io
                .write_all(&test_helpers::heartbeat_bytes(4))
                .await
                .unwrap();
            let mut buf = Vec::new();
            let rr = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*rr), MsgTypeBase::ResendRequest);

            // Closing the gap re-dispatches the Logout, which ends the
            // session; the Heartbeat behind it must not follow.
            client_io
                .write_all(&test_helpers::heartbeat_bytes(2))
                .await
                .unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logout)
            );
            // The Logout response is on the wire; the peer closes.
            let logout = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
            client_io.shutdown().await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::RemoteRequestedLogout),
                "seq 4 must stay parked once seq 3 ended the session"
            );
            session_task.await.unwrap();
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));

            // Nothing followed the Logout response.
            let mut rest = Vec::new();
            client_io.read_to_end(&mut rest).await.unwrap();
            assert!(
                rest.is_empty(),
                "unexpected bytes after the Logout: {rest:?}"
            );
        })
        .await;
}

/// Scenario 20: a `ResendRequest<2>` that arrives while we are still
/// replaying an earlier one is served after it, in order, and a gap the
/// peer opens in the meantime still draws exactly one `ResendRequest<2>`
/// of our own. The replay iterations skip the event select, so the peer's
/// input waits until the first range is exhausted.
#[tokio::test]
async fn resend_request_during_a_replay_is_served_after_it() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let sender = harness.sender.clone();
            let (server_io, mut client_io) = io::duplex(65536);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Our seq 2..=6: five orders the peer reads.
            for _ in 0..5 {
                sender
                    .send(test_helpers::new_order_single_with_empty_header())
                    .expect("stage");
            }
            let mut buf = Vec::new();
            for seq in 2..=6 {
                let msg = read_one_message(&mut client_io, &mut buf).await;
                assert_eq!(msg.msg_seq_num(), seq);
            }

            // The peer asks for all of it (peer seq 2); the replay starts.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::resend_request(2, 2, 6),
                ))
                .await
                .unwrap();
            let first = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(first.msg_seq_num(), 2);
            assert_eq!(first.poss_dup_flag(), Some(true));

            // Mid-replay: a second request for 3..=4 (peer seq 3), then an
            // order at peer seq 5 - one ahead of the expected 4.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::resend_request(3, 3, 4),
                ))
                .await
                .unwrap();
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::new_order_single(5),
                ))
                .await
                .unwrap();

            // The first replay completes untouched.
            for seq in 3..=6 {
                let msg = read_one_message(&mut client_io, &mut buf).await;
                assert_eq!(msg.msg_seq_num(), seq, "first replay out of order");
                assert_eq!(msg.poss_dup_flag(), Some(true));
            }

            // Then the second replay (3, 4) and our one ResendRequest for
            // the gap the order opened - the admin flush runs ahead of the
            // replay batch, so their relative order is not pinned.
            let mut replayed = Vec::new();
            let mut own_requests = Vec::new();
            for _ in 0..3 {
                let msg = read_one_message(&mut client_io, &mut buf).await;
                match SessionMessage::try_as_admin(&*msg) {
                    Some(AdminBase::ResendRequest(rr)) => {
                        own_requests.push((rr.begin_seq_no, rr.end_seq_no));
                    }
                    _ => {
                        assert_eq!(msg.poss_dup_flag(), Some(true));
                        replayed.push(msg.msg_seq_num());
                    }
                }
            }
            assert_eq!(replayed, vec![3, 4], "second replay, in order");
            assert_eq!(own_requests, vec![(4, 4)], "one request for the one gap");

            // The peer closes the gap; the parked order is delivered.
            client_io
                .write_all(&test_helpers::sequence_reset_bytes(4, 5, true))
                .await
                .unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest)
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::SequenceReset)
            );
            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::AppMsgIn);

            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );
            session_task.await.unwrap();
        })
        .await;
}
