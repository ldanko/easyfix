use std::{assert_matches, time::Duration};

use easyfix_core::{
    base_messages::MsgTypeBase,
    basic_types::SeqNum,
    message::{HeaderAccess, SessionMessage},
};
use tokio::{
    io,
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    sync::{mpsc, mpsc::error::TryRecvError},
    task::{JoinHandle, LocalSet},
    time,
};

use super::{
    harness::{TestEvent, build_harness},
    wire::{build_peer_logon, logon_handshake, read_lone_message},
};
use crate::{
    application::DisconnectReason,
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{nz_seq, read_one_message},
};

/// Spawn an acceptor with its counters seeded at `sender_seq` /
/// `target_seq`, fed a peer Logon numbered `logon_seq`. Returns the
/// client end of the wire along with the usual handles.
fn spawn_acceptor_at(
    sender_seq: SeqNum,
    target_seq: SeqNum,
    logon_seq: SeqNum,
) -> (
    JoinHandle<()>,
    mpsc::UnboundedReceiver<TestEvent>,
    mpsc::Sender<ControlMsg>,
    DuplexStream,
) {
    let mut harness = build_harness();
    harness
        .storage
        .set_next_sender_msg_seq_num(nz_seq(sender_seq))
        .unwrap();
    harness
        .storage
        .set_next_target_msg_seq_num(nz_seq(target_seq))
        .unwrap();
    let (server_io, client_io) = io::duplex(8192);
    let (server_reader, server_writer) = io::split(server_io);
    let (session_task, events_rx, control_tx) = harness.spawn_acceptor(
        server_reader,
        server_writer,
        build_peer_logon(logon_seq, 30),
    );
    (session_task, events_rx, control_tx, client_io)
}

/// The next event must be the session ending for `reason`. Bounded by a
/// timeout: a missing reaction does not fail an assertion, it hangs.
async fn expect_session_end(
    events_rx: &mut mpsc::UnboundedReceiver<TestEvent>,
    reason: DisconnectReason,
) {
    let end = time::timeout(Duration::from_secs(5), events_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("session must end with {reason:?}"))
        .unwrap();
    assert_matches!(
        end,
        TestEvent::SessionEnd(r) if r == reason,
        "expected SessionEnd({reason:?})"
    );
}

/// The reaction to exhausted incoming numbering lives in the dispatch tail,
/// which is the funnel every input passes through. This pins both
/// main-loop entries: a parsed message, and a decode error whose header
/// never parsed, so the counter is read raw rather than through
/// `verify_header` - that one draws a Reject before the Logout.
#[tokio::test]
async fn main_loop_input_closing_incoming_numbering_ends_the_session() {
    // The last usable sequence number, either way. Accepted - and
    // then there is nothing left to expect.
    let cases: [(Vec<u8>, bool); 2] = [
        (test_helpers::heartbeat_bytes(SeqNum::MAX - 1), false),
        (build_bad_sending_time_bytes(SeqNum::MAX - 1), true),
    ];
    for (bytes, rejected) in cases {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (session_task, mut events_rx, _control_tx, mut client_io) =
                    spawn_acceptor_at(1, SeqNum::MAX - 2, SeqNum::MAX - 2);
                logon_handshake(&mut client_io, &mut events_rx).await;

                client_io.write_all(&bytes).await.unwrap();

                let mut buf = Vec::new();
                if rejected {
                    let reject = read_one_message(&mut client_io, &mut buf).await;
                    assert_eq!(SessionMessage::msg_type(&*reject), MsgTypeBase::Reject);
                } else {
                    assert_matches!(
                        events_rx.recv().await.unwrap(),
                        TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
                    );
                }
                let logout = read_one_message(&mut client_io, &mut buf).await;
                assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);

                expect_session_end(&mut events_rx, DisconnectReason::SeqNumExhausted).await;
                session_task.await.unwrap();
            })
            .await;
    }
}

/// The acceptor pre-loop dispatches through the same funnel, and its tail
/// runs before the `should_disconnect` check that follows it. This is the
/// only entry where that ordering matters.
#[tokio::test]
async fn preloop_logon_closing_incoming_numbering_ends_the_session() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (session_task, mut events_rx, _control_tx, mut client_io) =
                spawn_acceptor_at(1, SeqNum::MAX - 1, SeqNum::MAX - 1);

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );

            // The Logon is answered, and the Logout follows immediately -
            // the session is never announced as ready.
            let mut buf = Vec::new();
            let ack = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*ack), MsgTypeBase::Logon);
            let logout = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);

            expect_session_end(&mut events_rx, DisconnectReason::SeqNumExhausted).await;
            session_task.await.unwrap();
        })
        .await;
}

/// A message parked in the out-of-order queue advances the counter through
/// `next_queued_message`, which bypasses `on_input` entirely. The queue
/// drain is its own entry to the funnel.
#[tokio::test]
async fn queued_message_closing_incoming_numbering_ends_the_session() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Logon arrives one ahead of the expected number, so it is
            // queued and a ResendRequest goes out for the gap.
            let (session_task, mut events_rx, _control_tx, mut client_io) =
                spawn_acceptor_at(1, SeqNum::MAX - 2, SeqNum::MAX - 1);

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );
            let mut buf = Vec::new();
            let ack = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*ack), MsgTypeBase::Logon);
            let resend_request = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*resend_request),
                MsgTypeBase::ResendRequest
            );
            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);

            // Fill the gap so the counter lands on MAX - 1, where the queued
            // Logon is waiting: draining it exhausts the numbering.
            let gap_fill =
                test_helpers::sequence_reset_bytes(SeqNum::MAX - 2, SeqNum::MAX - 1, true);
            client_io.write_all(&gap_fill).await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::SequenceReset)
            );

            expect_session_end(&mut events_rx, DisconnectReason::SeqNumExhausted).await;
            session_task.await.unwrap();
        })
        .await;
}

/// Applying NewSeqNo=MAX ends the session before the newly reachable queued
/// input can reach the application. Both forms advance NextNumIn (FIX Session
/// Test Cases Scenarios 10(b) and 11(a)); ending at MAX is our limit policy.
#[tokio::test]
async fn sequence_reset_to_max_logs_out_without_dispatching_queued_input() {
    for gap_fill in [true, false] {
        let local = LocalSet::new();
        local
            .run_until(async {
                let (session_task, mut events_rx, _control_tx, mut client_io) =
                    spawn_acceptor_at(1, 1, 1);
                logon_handshake(&mut client_io, &mut events_rx).await;

                // Wait for the ResendRequest so the heartbeat is definitely
                // queued before the SequenceReset arrives.
                client_io
                    .write_all(&test_helpers::heartbeat_bytes(SeqNum::MAX))
                    .await
                    .unwrap();
                let mut buf = Vec::new();
                let resend_request = read_one_message(&mut client_io, &mut buf).await;
                assert_eq!(
                    SessionMessage::msg_type(&*resend_request),
                    MsgTypeBase::ResendRequest
                );
                assert_matches!(events_rx.try_recv(), Err(TryRecvError::Empty));

                // GapFill must be in sequence; Reset ignores its own number.
                let seq = if gap_fill { 2 } else { 7 };
                client_io
                    .write_all(&test_helpers::sequence_reset_bytes(
                        seq,
                        SeqNum::MAX,
                        gap_fill,
                    ))
                    .await
                    .unwrap();
                assert_matches!(
                    events_rx.recv().await.unwrap(),
                    TestEvent::AdminMsgIn(MsgTypeBase::SequenceReset)
                );

                let logout = read_one_message(&mut client_io, &mut buf).await;
                assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);
                expect_session_end(&mut events_rx, DisconnectReason::SeqNumExhausted).await;
                session_task.await.unwrap();
                assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));

                let mut rest = Vec::new();
                client_io.read_to_end(&mut rest).await.unwrap();
                assert!(rest.is_empty(), "nothing may follow the Logout: {rest:?}");
            })
            .await;
    }
}

/// Outgoing side, acceptor pre-loop: stamping the Logon acknowledgement
/// itself consumes the last sequence number.
#[tokio::test]
async fn preloop_logon_ack_closing_outgoing_numbering_ends_the_session() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (session_task, mut events_rx, _control_tx, mut client_io) =
                spawn_acceptor_at(SeqNum::MAX - 1, 1, 1);

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );

            // The acknowledgement still reaches the wire, carrying the last
            // number there is.
            let ack = read_lone_message(&mut client_io).await;
            assert_eq!(SessionMessage::msg_type(&*ack), MsgTypeBase::Logon);
            assert_eq!(ack.msg_seq_num(), SeqNum::MAX - 1);

            expect_session_end(&mut events_rx, DisconnectReason::SeqNumExhausted).await;
            // Never announced as ready, and no Logout - there is no number
            // left to stamp one with.
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));

            session_task.await.unwrap();
        })
        .await;
}

/// The initiator has no in-band way out of an exhausted outgoing
/// numbering: it cannot stamp even its own `Logon<A>`. Announcing the
/// session ready would hand the application a `Sender` for a connection
/// that is already over, and every reconnect would repeat it.
#[tokio::test]
async fn initiator_with_exhausted_outgoing_numbering_never_announces_the_session() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut harness = build_harness();
            harness
                .storage
                .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
                .unwrap();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_initiator(server_reader, server_writer);

            // SessionReady must not precede it.
            expect_session_end(&mut events_rx, DisconnectReason::SeqNumExhausted).await;

            let mut buf = Vec::new();
            client_io.read_to_end(&mut buf).await.unwrap();
            assert!(buf.is_empty(), "no Logon may reach the wire: {buf:?}");

            session_task.await.unwrap();
        })
        .await;
}

/// An initiator holds the `Sender` from `on_session_ready` on, which is
/// before the peer has acknowledged its `Logon<A>`. Whatever the
/// application stages in that window must not reach the wire when the
/// handshake then fails: "the initiator should not transmit any
/// application message until the Logon(35=A) acknowledgement has been
/// received" (FIX Session Layer Section 4.3.10). The farewell that answers the
/// bad acknowledgement used to make the session count as logged on for
/// exactly long enough to drain that backlog behind it.
#[tokio::test]
async fn farewell_before_the_logon_ack_does_not_release_staged_app_messages() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let staging = harness.sender.clone();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_initiator(server_reader, server_writer);

            let mut buf = Vec::new();
            let logon_request = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*logon_request),
                MsgTypeBase::Logon
            );
            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);

            // Staged while the handshake is still open.
            staging
                .send(test_helpers::new_order_single_with_empty_header())
                .expect("staging succeeds while the session is live");

            // The acknowledgement carries an invalid HeartBtInt(108):
            // Reject, Logout, disconnect (Test Cases Scenario 1B(d)).
            client_io
                .write_all(&test_helpers::logon_bytes(1, -1))
                .await
                .unwrap();

            let reject = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*reject),
                MsgTypeBase::Reject,
                "the staged order must not overtake the Reject"
            );
            let logout = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::AdminMsgIn(MsgTypeBase::Logon)
            );
            expect_session_end(&mut events_rx, DisconnectReason::InvalidLogonState).await;
            session_task.await.unwrap();

            let mut rest = Vec::new();
            client_io.read_to_end(&mut rest).await.unwrap();
            assert!(
                rest.is_empty(),
                "nothing may follow the Logout on an unacknowledged session: {rest:?}"
            );
        })
        .await;
}

/// A well-framed message whose *header* fails to decode: `SendingTime(52)`
/// is not a timestamp. The recovered `MsgSeqNum(34)` survives, but there is
/// no header for the session to validate, so the counter is read raw.
fn build_bad_sending_time_bytes(seq: SeqNum) -> Vec<u8> {
    test_helpers::frame_message(
        "FIXT.1.1",
        &format!("35=0|49=TARGET|56=SENDER|34={seq}|52=NOT-A-TIMESTAMP|"),
    )
}
