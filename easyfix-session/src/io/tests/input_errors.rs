use std::{assert_matches, str, time::Duration};

use chrono::Utc;
use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    basic_types::SeqNum,
    fix_str,
    message::SessionMessage,
};
use tokio::{
    io,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc::error::TryRecvError,
    task,
    task::LocalSet,
    time,
};

use super::{
    harness::{TestEvent, build_harness},
    wire::{
        build_order_missing_symbol_bytes, build_peer_logon, logon_handshake, read_lone_message,
    },
};
use crate::{
    application::DisconnectReason,
    io::{ControlMsg, InputStream, SessionOpening, session_loop},
    test_helpers,
    test_helpers::{DEFAULT_MAX_MESSAGE_SIZE, read_one_message},
};

/// Rewrite the `BodyLength(9)` of a framed message by `delta`, leaving
/// every other byte - the trailer included - as it was.
fn misdeclare_body_length(mut bytes: Vec<u8>, delta: i64) -> Vec<u8> {
    let start = bytes
        .windows(3)
        .position(|w| w == b"\x019=")
        .expect("a framed message carries BodyLength(9)")
        + 3;
    let end = start
        + bytes[start..]
            .iter()
            .position(|&b| b == b'\x01')
            .expect("BodyLength(9) is SOH-terminated");
    let declared: i64 = str::from_utf8(&bytes[start..end])
        .expect("ASCII digits")
        .parse()
        .expect("a decimal BodyLength(9)");
    bytes.splice(start..end, (declared + delta).to_string().into_bytes());
    bytes
}

/// A well-framed `Heartbeat<0>` with no `MsgSeqNum(34)`: `BodyLength(9)`
/// and `CheckSum(10)` are valid, so deserialization reaches the
/// missing-34 classification (`LogoutReason::MsgSeqNumMissing`) instead
/// of reporting a garbled frame.
fn build_missing_seq_num_bytes() -> Vec<u8> {
    let body = b"35=0\x0149=TARGET\x0156=SENDER\x0152=20260611-00:00:00.000\x01";
    let mut bytes = format!("8=FIXT.1.1\x019={}\x01", body.len()).into_bytes();
    bytes.extend_from_slice(body);
    let checksum = bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
    bytes.extend_from_slice(format!("10={checksum:03}\x01").as_bytes());
    bytes
}

/// An application rejecting without a diagnostic must still put a real
/// `Reject<3>` on the wire. `Text(58)` is optional there (FIX Transport
/// 5.5), but an empty value is not a legal FIX field, so an empty `Text`
/// fails serialization - and a failed commit used to queue output anyway,
/// turning the Reject into a live `SequenceReset`-GapFill. The peer would
/// then be told to skip the seq num and would never learn its message was
/// rejected.
#[tokio::test]
async fn reject_without_text_reaches_peer_as_a_reject() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut harness = build_harness();
            harness.app.reject_without_text = true;
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::new_order_single(2),
                ))
                .await
                .unwrap();

            let mut buf = Vec::new();
            let reject = read_one_message(&mut client_io, &mut buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*reject),
                MsgTypeBase::Reject,
                "a Reject without Text(58) must reach the wire as a Reject, \
                     not as a gap-fill substituted for a message that failed to serialize"
            );

            assert_matches!(events_rx.recv().await.unwrap(), TestEvent::AppMsgIn);

            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            session_task.await.unwrap();
        })
        .await;
}

/// A well-framed message without `MsgSeqNum(34)` is answered with
/// `Logout<5>` and the session ends with the dedicated
/// `MsgSeqNumNotFound` reason (FIX Session Layer Section 4.5.3) - not with the
/// catch-all `Disconnected`.
#[tokio::test]
async fn missing_msg_seq_num_ends_with_dedicated_reason() {
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

            client_io
                .write_all(&build_missing_seq_num_bytes())
                .await
                .unwrap();

            // The session answers with Logout<5> before dropping.
            let logout = read_lone_message(&mut client_io).await;
            assert_eq!(SessionMessage::msg_type(&*logout), MsgTypeBase::Logout);

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::MsgSeqNumNotFound)
            );

            session_task.await.unwrap();

            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// An input I/O error on an established session ends it with `IoError`.
#[tokio::test]
async fn input_io_error_ends_with_io_error() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (_server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);

            // `spawn_acceptor` is hard-wired to a duplex reader; wire the
            // failing reader manually.
            let mut events_rx = harness.events_rx;
            let session_task = task::spawn_local(async move {
                let input = InputStream::new(test_helpers::FailingReader, DEFAULT_MAX_MESSAGE_SIZE);
                let mut storage = harness.storage;
                session_loop(
                    SessionOpening::FirstMessage(Ok(first_msg)),
                    input,
                    server_writer,
                    harness.engine,
                    &mut storage,
                    harness.app,
                    harness.sender,
                    harness.app_rx,
                    harness.control_rx,
                )
                .await;
            });

            logon_handshake(&mut client_io, &mut events_rx).await;

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::IoError)
            );

            session_task.await.unwrap();

            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}

/// `max_message_size` is enforced on inbound messages from
/// `BodyLength(9)` alone: a frame declaring more is never read. The
/// session answers the way FIX Session Layer Section 4.3.6 has a peer answer a
/// size it cannot process - a `Logout<5>` naming the sizes in `Text(58)`
/// - and disconnects. Only the frame's prefix is written and the writer
/// stays open, so nothing but the declared length can produce the
/// Logout.
#[tokio::test]
async fn oversized_inbound_message_logs_out_before_it_is_read() {
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

            // BodyLength(9) claims more than the 4096-byte limit; the
            // body never follows. "8=FIXT.1.1|" is 11 bytes, "9=5000|"
            // is 7, the trailer is 7: a 5025-byte frame.
            client_io
                .write_all(b"8=FIXT.1.1\x019=5000\x0135=0\x01")
                .await
                .unwrap();

            let logout = time::timeout(Duration::from_secs(5), read_lone_message(&mut client_io))
                .await
                .expect("the declared length must produce the Logout, not the body");
            assert_matches!(
                SessionMessage::try_as_admin(&*logout),
                Some(AdminBase::Logout(logout))
                    if logout.text.as_deref()
                        == Some(fix_str!(
                            "Message size 5025 exceeds maximum message size of 4096"
                        ))
            );
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::MessageTooLarge)
            );
            session_task.await.unwrap();
        })
        .await;
}

/// A batch of well-framed messages that fail body decoding, written in
/// one go, draws one `Reject<3>` per message - each naming its
/// `RefSeqNum(45)` and `SessionRejectReason(373)` - and leaves the
/// session up with `NextNumIn` advanced past the batch (FIX Session Layer
/// Section 4.5.4; Test Cases Scenario 14(b)). The property under test is that a
/// flood of rejectable input neither stalls nor kills the session task.
#[tokio::test]
async fn reject_flood_does_not_kill_the_session() {
    const FLOOD_LEN: SeqNum = 20;

    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(65536);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            // Seq 2..=21, one write.
            let batch: Vec<u8> = (2..2 + FLOOD_LEN)
                .flat_map(build_order_missing_symbol_bytes)
                .collect();
            client_io.write_all(&batch).await.unwrap();

            let mut buf = Vec::new();
            for seq in 2..2 + FLOOD_LEN {
                let msg = read_one_message(&mut client_io, &mut buf).await;
                let AdminBase::Reject(ref reject) = SessionMessage::try_as_admin(&*msg)
                    .expect("every flooded message is answered with a Reject")
                else {
                    panic!("expected Reject for seq {seq}");
                };
                assert_eq!(reject.ref_seq_num, seq);
                assert_eq!(reject.ref_tag_id, Some(55));
                assert_eq!(
                    reject.session_reject_reason,
                    Some(SessionRejectReasonBase::RequiredTagMissing.into())
                );
            }

            // Still alive, and the counter sits past the batch: the next
            // in-sequence message is answered, not parked behind a gap.
            client_io
                .write_all(&test_helpers::serialize_message(
                    &test_helpers::test_request(2 + FLOOD_LEN, fix_str!("alive")),
                ))
                .await
                .unwrap();
            let msg = read_one_message(&mut client_io, &mut buf).await;
            let AdminBase::Heartbeat(ref hb) = SessionMessage::try_as_admin(&*msg)
                .expect("TestRequest is answered with a Heartbeat")
            else {
                panic!("expected Heartbeat");
            };
            assert_eq!(hb.test_req_id.as_deref(), Some(fix_str!("alive")));
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

/// A garbled frame is disregarded without touching `NextNumIn` (FIX
/// Session Layer Section 4.5.2; Transport Section 8.4), and the session keeps reading:
/// the well-formed message right behind it, carrying the very sequence
/// number the garbled one claimed, is processed as the next in sequence
/// - no ResendRequest, no Logout - and the TestRequest after that is
/// answered. Four shapes, each a fault in the framing fields:
///
/// - `BodyLength(9)` understated and overstated (Test Cases Section 4.5.1
///   Scenario 2(m)). Overstated, the frame swallows its own trailer and
///   the head of the next message before the checksum position turns out
///   not to hold one, so it is the resynchronisation scan that recovers
///   that next message.
/// - `MsgType(35)` ahead of `BodyLength(9)`, and `MsgSeqNum(34)` ahead of
///   `MsgType(35)` in an otherwise well-framed message (Scenario 2(t)).
///   The first fails framing, the second decoding
///   (`GarbledReason::MsgTypeNotThirdTag`); the session treats both alike.
///
/// Everything goes in one write, so the overstated frame completes and
/// fails instead of waiting for input that never comes.
#[tokio::test]
async fn garbled_frames_are_skipped_without_consuming_a_sequence_number() {
    let now = Utc::now().format("%Y%m%d-%H:%M:%S%.3f");
    let cases: [(&str, Vec<u8>); 4] = [
        (
            "BodyLength understated",
            misdeclare_body_length(test_helpers::heartbeat_bytes(2), -5),
        ),
        (
            "BodyLength overstated",
            misdeclare_body_length(test_helpers::heartbeat_bytes(2), 20),
        ),
        (
            "MsgType before BodyLength",
            b"8=FIXT.1.1\x0135=0\x019=5\x0110=000\x01".to_vec(),
        ),
        (
            "MsgSeqNum before MsgType",
            test_helpers::frame_message(
                "FIXT.1.1",
                &format!("34=2|35=0|49=TARGET|56=SENDER|52={now}|"),
            ),
        ),
    ];

    let local = LocalSet::new();
    local
        .run_until(async {
            for (shape, garbled) in cases {
                let harness = build_harness();
                let (server_io, mut client_io) = io::duplex(8192);
                let (server_reader, server_writer) = io::split(server_io);
                let (session_task, mut events_rx, control_tx) =
                    harness.spawn_acceptor(server_reader, server_writer, build_peer_logon(1, 30));

                logon_handshake(&mut client_io, &mut events_rx).await;

                let mut batch = garbled;
                batch.extend_from_slice(&test_helpers::heartbeat_bytes(2));
                batch.extend_from_slice(&test_helpers::serialize_message(
                    &test_helpers::test_request(3, fix_str!("probe")),
                ));
                client_io.write_all(&batch).await.unwrap();

                // Both well-formed messages reach the application in
                // sequence: the garbled frame consumed no number.
                assert_matches!(
                    events_rx.recv().await.unwrap(),
                    TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat),
                    "{shape}"
                );
                assert_matches!(
                    events_rx.recv().await.unwrap(),
                    TestEvent::AdminMsgIn(MsgTypeBase::TestRequest),
                    "{shape}"
                );
                // And the first thing on the wire after the handshake is
                // the answer to the TestRequest - not a ResendRequest for
                // a gap the garbled frame supposedly opened, not a Logout.
                let mut buf = Vec::new();
                let msg = read_one_message(&mut client_io, &mut buf).await;
                let AdminBase::Heartbeat(ref hb) = SessionMessage::try_as_admin(&*msg)
                    .unwrap_or_else(|| panic!("{shape}: expected an admin message"))
                else {
                    panic!("{shape}: expected Heartbeat");
                };
                assert_eq!(
                    hb.test_req_id.as_deref(),
                    Some(fix_str!("probe")),
                    "{shape}"
                );

                control_tx.send(ControlMsg::Disconnect).await.unwrap();
                assert_matches!(
                    events_rx.recv().await.unwrap(),
                    TestEvent::SessionEnd(DisconnectReason::Disconnected),
                    "{shape}"
                );
                session_task.await.unwrap();
            }
        })
        .await;
}

/// The peer's transport dies in the middle of a frame - here inside the
/// `CheckSum(10)` field, the last bytes of a message. A cut-off frame is
/// never a message, so nothing is answered, and the EOF behind it ends
/// the session as a plain `Disconnected` - not `IoError`, the read
/// succeeded and the peer is simply gone - with no farewell of our own: a
/// Logout to a closed socket would only consume a `MsgSeqNum`.
#[tokio::test]
async fn peer_closing_mid_frame_ends_the_session_as_disconnected() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let (session_task, mut events_rx, _control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, build_peer_logon(1, 30));

            logon_handshake(&mut client_io, &mut events_rx).await;

            // The trailer is "10=NNN\x01"; keep "10=N" and drop the rest.
            let frame = test_helpers::heartbeat_bytes(2);
            client_io
                .write_all(&frame[..frame.len() - 3])
                .await
                .unwrap();
            client_io.shutdown().await.unwrap();

            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );
            session_task.await.unwrap();

            let mut rest = Vec::new();
            client_io.read_to_end(&mut rest).await.unwrap();
            assert!(
                rest.is_empty(),
                "nothing is written in reply to a cut-off frame"
            );
            assert_matches!(events_rx.try_recv(), Err(TryRecvError::Disconnected));
        })
        .await;
}
