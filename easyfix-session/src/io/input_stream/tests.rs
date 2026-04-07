use std::{
    assert_matches, io::ErrorKind, iter, num::NonZeroUsize, pin::pin, task::Poll, time::Duration,
};

use bytes::BytesMut;
use easyfix_core::{
    basic_types::NonZeroLength,
    deserializer::{DeserializeErrorKind, GarbledReason},
    fix_str,
    message::{DeserializeError, HeaderAccess},
    version::Version,
};
use easyfix_test_messages::Message;
use futures_util::{StreamExt, poll};
use tokio::{
    io::{AsyncWriteExt, DuplexStream, duplex},
    time::timeout,
};

use super::{
    FirstMessageEvent, InputEvent, InputStream, invalid_logon_identity, process_garbled_data,
};
use crate::{session_id::SessionId, test_helpers};

/// Write `bytes`, close the writer, and return an `InputStream` over the read
/// side. Tests that need the writer open mid-test set up the duplex manually.
async fn stream_over(bytes: &[u8]) -> InputStream<DuplexStream, Message> {
    let (mut writer, reader) = duplex(8192);
    writer.write_all(bytes).await.unwrap();
    drop(writer);
    InputStream::new(reader, test_helpers::DEFAULT_MAX_MESSAGE_SIZE)
}

// --- Happy path ---

#[tokio::test]
async fn parses_single_complete_message() {
    let mut stream = stream_over(&test_helpers::heartbeat_bytes(1)).await;

    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::Message(msg) if msg.msg_seq_num() == 1);

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn parses_two_messages_sent_together() {
    let mut bytes = test_helpers::heartbeat_bytes(1);
    bytes.extend_from_slice(&test_helpers::heartbeat_bytes(2));
    let mut stream = stream_over(&bytes).await;

    let event1 = stream.next().await.unwrap();
    assert_matches!(event1, InputEvent::Message(msg) if msg.msg_seq_num() == 1);

    let event2 = stream.next().await.unwrap();
    assert_matches!(event2, InputEvent::Message(msg) if msg.msg_seq_num() == 2);

    assert!(stream.next().await.is_none());
}

// --- Partial message buffering ---

#[tokio::test]
async fn reassembles_message_split_across_two_writes() {
    let (mut writer, reader) = duplex(8192);
    let bytes = test_helpers::heartbeat_bytes(1);
    let mid = bytes.len() / 2;

    let mut stream = InputStream::<_, Message>::new(reader, test_helpers::DEFAULT_MAX_MESSAGE_SIZE);

    writer.write_all(&bytes[..mid]).await.unwrap();

    // Start reading - should not complete with just the first half
    let mut next_fut = pin!(stream.next());
    assert_matches!(poll!(&mut next_fut), Poll::Pending);

    writer.write_all(&bytes[mid..]).await.unwrap();
    drop(writer);

    // Now the message should be complete
    let event = next_fut.await.unwrap();
    assert_matches!(event, InputEvent::Message(msg) if msg.msg_seq_num() == 1);
}

// --- Garbled data recovery ---

#[tokio::test]
async fn recovers_after_garbled_prefix() {
    let mut bytes = b"this is garbage data".to_vec();
    bytes.extend_from_slice(&test_helpers::heartbeat_bytes(1));
    let mut stream = stream_over(&bytes).await;

    // First event: deserialization error from garbled data
    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::DeserializeError(_));

    // Second event: the valid message after the garbage
    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::Message(msg) if msg.msg_seq_num() == 1);
}

/// A read boundary can cut the next message's `8=FIX` in half. The resync
/// must keep that fragment so the following read completes it instead of
/// destroying a good message (Session Layer Section 4.5.2 says to ignore
/// the garbled bytes, not the message behind them).
#[tokio::test]
async fn recovers_message_whose_start_is_split_by_the_garbled_read() {
    let (mut writer, reader) = duplex(8192);
    let bytes = test_helpers::heartbeat_bytes(1);

    let mut stream = InputStream::<_, Message>::new(reader, test_helpers::DEFAULT_MAX_MESSAGE_SIZE);

    let mut first_chunk = b"ZZZZ".to_vec();
    first_chunk.extend_from_slice(&bytes[..3]);
    assert_eq!(&first_chunk[4..], b"8=F");
    writer.write_all(&first_chunk).await.unwrap();

    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::DeserializeError(_));

    let mut next_fut = pin!(stream.next());
    assert_matches!(poll!(&mut next_fut), Poll::Pending);

    writer.write_all(&bytes[3..]).await.unwrap();
    drop(writer);

    let event = next_fut.await.unwrap();
    assert_matches!(event, InputEvent::Message(msg) if msg.msg_seq_num() == 1);
}

/// A frame rejected on `CheckSum(10)` is skipped whole. Scanning it for the
/// next `8=FIX` would find the look-alike planted in `Text(58)`, whose
/// `9=500` then swallows the real Heartbeat behind it as phantom body - the
/// stream would hang on it until EOF instead of yielding the message.
#[tokio::test]
async fn invalid_checksum_skips_the_frame_without_scanning_its_body() {
    let mut bad = test_helpers::frame_message(
        "FIXT.1.1",
        "35=B|49=TARGET|56=SENDER|34=1|52=20260611-00:00:00.000|58=8=FIX.4.2|9=500|",
    );
    // The trailing field is "10=DDD\x01"; flip the last digit.
    let last_digit = bad.len() - 2;
    bad[last_digit] = if bad[last_digit] == b'9' {
        b'0'
    } else {
        bad[last_digit] + 1
    };
    let mut bytes = bad;
    bytes.extend_from_slice(&test_helpers::heartbeat_bytes(1));
    let mut stream = stream_over(&bytes).await;

    let event = stream.next().await.unwrap();
    assert_matches!(
        event,
        InputEvent::DeserializeError(DeserializeError {
            kind: DeserializeErrorKind::Garbled(GarbledReason::InvalidChecksum),
            ..
        })
    );

    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::Message(msg) if msg.msg_seq_num() == 1);

    assert!(stream.next().await.is_none());
}

#[test]
fn garbled_resync_keeps_a_partial_message_start() {
    for (input, kept) in [
        (&b"ZZZZ8=FI"[..], &b"8=FI"[..]),
        (b"ZZZZ8=F", b"8=F"),
        (b"ZZZZ8=", b"8="),
        (b"ZZZZ8", b"8"),
        (b"ZZZZ", b""),
        // A tail that is not a prefix of "8=FIX" is garbage like the rest.
        (b"ZZ=FI", b""),
        (b"ZZZFIX", b""),
        // A complete needle already failed at offset 0 and, with nothing
        // usable behind it, its partial echo at the end is all that stays.
        (b"8=FIX\x00zz8=F", b"8=F"),
    ] {
        let mut buf = BytesMut::from(input);
        process_garbled_data(&mut buf);
        assert_eq!(&buf[..], kept, "input {input:?}");
    }
}

// --- first_message ---

/// A `Logon<A>` whose `Text(58)` pads it past `len` bytes - the shape of a
/// first message an acceptor cannot bound by any other means.
fn oversized_logon_bytes(len: usize) -> Vec<u8> {
    let text = "x".repeat(len);
    test_helpers::frame_message(
        "FIXT.1.1",
        &format!("35=A|49=TARGET|56=SENDER|34=1|52=20260721-08:00:00.000|98=0|108=30|58={text}|"),
    )
}

/// `first_message` on a well-framed Logon that fails decoding recovers
/// the peer identity from its bytes (Test Cases Scenario 1S(d)) - and the
/// stream continues with the next message, proving the failed span was
/// consumed exactly.
#[tokio::test]
async fn first_message_invalid_logon_recovers_identity() {
    let mut bytes = test_helpers::invalid_logon_bytes();
    bytes.extend_from_slice(&test_helpers::heartbeat_bytes(2));
    let mut stream = stream_over(&bytes).await;

    let event = stream.first_message().await.unwrap();
    assert_matches!(
        event,
        FirstMessageEvent::DeserializeError { invalid_logon_identity: Some(session_id), .. }
            if session_id == test_helpers::default_session_id()
    );

    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::Message(msg) if msg.msg_seq_num() == 2);
}

/// The same recovery when the peer delivers the Logon one byte per read:
/// framing, and the copy it guards, must survive an arbitrary number of
/// partial reads. `duplex(1)` caps the pipe at a single byte, so the
/// reader drains one byte per `read_buf`.
#[tokio::test]
async fn first_message_recovers_identity_when_delivered_byte_by_byte() {
    let (mut writer, reader) = duplex(1);
    let bytes = test_helpers::invalid_logon_bytes();
    let write = tokio::spawn(async move { writer.write_all(&bytes).await.unwrap() });

    let mut stream = InputStream::<_, Message>::new(reader, test_helpers::DEFAULT_MAX_MESSAGE_SIZE);
    let event = stream.first_message().await.unwrap();
    assert_matches!(
        event,
        FirstMessageEvent::DeserializeError { invalid_logon_identity: Some(session_id), .. }
            if session_id == test_helpers::default_session_id()
    );

    write.await.unwrap();
}

/// A failed-decode first message that is NOT a Logon yields no identity -
/// only an invalid Logon is answered per Scenario 1S(d).
#[tokio::test]
async fn first_message_invalid_non_logon_has_no_identity() {
    let invalid = test_helpers::frame_message(
        "FIXT.1.1",
        "35=D|49=PEER|56=OWN|34=1|52=20260721-08:00:00.000|",
    );
    let mut stream = stream_over(&invalid).await;
    let event = stream.first_message().await.unwrap();
    assert_matches!(
        event,
        FirstMessageEvent::DeserializeError {
            invalid_logon_identity: None,
            ..
        }
    );
}

/// `first_message` on garbled input has no message span to recover an
/// identity from.
#[tokio::test]
async fn first_message_garbled_input_has_no_identity() {
    let mut stream = stream_over(b"this is not a FIX message").await;
    let event = stream.first_message().await.unwrap();
    assert_matches!(
        event,
        FirstMessageEvent::DeserializeError {
            invalid_logon_identity: None,
            ..
        }
    );
}

/// `first_message` parses a valid message exactly like the `Stream` impl.
#[tokio::test]
async fn first_message_parses_valid_message() {
    let mut stream = stream_over(&test_helpers::heartbeat_bytes(1)).await;
    let event = stream.first_message().await.unwrap();
    assert_matches!(event, FirstMessageEvent::Message(msg) if msg.msg_seq_num() == 1);
    assert!(stream.next().await.is_none());
}

/// A first message declaring more than the limit is reported as too large
/// from its framing fields, without being read: only the frame's prefix is
/// written and the writer stays open, so nothing but the declared length can
/// end the read. `frame_len` is what `BodyLength(9)` announced.
#[tokio::test]
async fn first_message_over_the_limit_is_too_large() {
    let limit = NonZeroUsize::new(256).unwrap();
    let (mut writer, reader) = duplex(8192);
    let oversized = oversized_logon_bytes(600);
    writer.write_all(&oversized[..64]).await.unwrap();

    let mut stream = InputStream::<_, Message>::for_first_message(reader, limit);
    let event = timeout(Duration::from_secs(5), stream.first_message())
        .await
        .expect("the declared length must end the read, not the body")
        .unwrap();
    assert_matches!(event, FirstMessageEvent::TooLarge { frame_len } if frame_len == oversized.len());
}

/// The limit is on the frame, not on the read: a Logon within the limit
/// followed by more bytes in the same read parses normally, however far past
/// the limit the buffer went. `from_parts` gives the buffer a burst's worth
/// of capacity, so one read takes everything.
#[tokio::test]
async fn first_message_pipelined_past_the_limit_is_not_too_large() {
    let limit = NonZeroLength::new(256).unwrap();
    let mut bytes = test_helpers::logon_bytes(1, 30);
    for seq in 2..8 {
        bytes.extend_from_slice(&test_helpers::heartbeat_bytes(seq));
    }
    assert!(
        bytes.len() > usize::from(limit.get()),
        "fixture must overshoot the limit"
    );
    let (mut writer, reader) = duplex(8192);
    writer.write_all(&bytes).await.unwrap();
    drop(writer);

    let mut stream = InputStream::<_, Message>::from_parts(reader, BytesMut::new(), limit);
    let event = stream.first_message().await.unwrap();
    assert_matches!(event, FirstMessageEvent::Message(msg) if msg.msg_seq_num() == 1);
    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::Message(msg) if msg.msg_seq_num() == 2);
}

/// A `BeginString(8)` that never terminates is not an incomplete frame that
/// could keep growing the buffer - `raw_message` garbles it once it runs past
/// the longest known BeginString, and the stream resynchronizes as for any
/// garbled input. The writer stays open, so only that verdict can end the
/// read.
#[tokio::test]
async fn unterminated_begin_string_is_garbled_not_buffered() {
    let (mut writer, reader) = duplex(8192);
    let mut bytes = b"8=".to_vec();
    bytes.extend(iter::repeat_n(b'X', 2 * Version::MAX_BEGIN_STRING_LEN));
    writer.write_all(&bytes).await.unwrap();

    let mut stream = InputStream::<_, Message>::new(reader, test_helpers::DEFAULT_MAX_MESSAGE_SIZE);
    let event = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("an unterminated BeginString must be garbled, not read forever")
        .unwrap();
    assert_matches!(
        event,
        InputEvent::DeserializeError(DeserializeError {
            kind: DeserializeErrorKind::Garbled(_),
            ..
        })
    );
}

/// The session stream enforces `max_message_size` the same way: a frame
/// declaring more is reported from `BodyLength(9)` before its body arrives.
/// The limit is far below the buffer's starting capacity, so this cannot be
/// the buffer filling up.
#[tokio::test]
async fn oversized_frame_is_reported_before_its_body_arrives() {
    let limit = NonZeroLength::new(256).unwrap();
    let (mut writer, reader) = duplex(8192);
    // "8=FIXT.1.1|" is 11 bytes, "9=5000|" is 7, the trailer is 7.
    writer
        .write_all(b"8=FIXT.1.1\x019=5000\x0135=0\x01")
        .await
        .unwrap();

    let mut stream = InputStream::<_, Message>::new(reader, limit);
    let event = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the declared length must end the read, not the body")
        .unwrap();
    assert_matches!(event, InputEvent::TooLarge { frame_len } if frame_len == 11 + 7 + 5000 + 7);
}

// --- invalid_logon_identity: the Scenario 1S(d) identity scan ---
//
// All inputs are well-framed (BodyLength and CheckSum are valid) - the
// scan only ever runs on messages that passed framing but failed
// decoding.

/// Expected identity for bodies carrying 49=PEER / 56=OWN: the CompIDs
/// are swapped like `SessionId::from_inbound`.
fn expected_id() -> SessionId {
    SessionId::new(
        Version::FIXT11,
        fix_str!("OWN").to_owned(),
        fix_str!("PEER").to_owned(),
    )
}

#[test]
fn scan_recovers_identity_from_invalid_logon() {
    let bytes = test_helpers::frame_message(
        "FIXT.1.1",
        "35=A|49=PEER|56=OWN|34=1|52=20260721-08:00:00.000|98=0|108=XX|",
    );
    assert_eq!(invalid_logon_identity(&bytes), Some(expected_id()));
}

/// A first message of any other type is dropped without a response
/// (Scenario 2S) - no identity is recovered for it.
#[test]
fn scan_yields_no_identity_for_non_logon() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "35=5|49=PEER|56=OWN|34=1|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

/// A header XmlData(213) field may contain arbitrary bytes, including a
/// fake `<SOH>49=` sequence. The declared XmlDataLen(212) skips the
/// content wholesale - the fake CompID must never win over the real one.
#[test]
fn scan_skips_header_xml_data_content_by_declared_length() {
    // XmlData content is "ab|49=EV" (8 bytes after SOH replacement).
    let bytes =
        test_helpers::frame_message("FIXT.1.1", "35=A|212=8|213=ab|49=EV|49=PEER|56=OWN|34=1|");
    assert_eq!(invalid_logon_identity(&bytes), Some(expected_id()));
}

/// A data field without its length field directly before it cannot be
/// delimited - scanning into its content could fabricate tags, so the
/// scan gives up.
#[test]
fn scan_aborts_on_data_field_without_length_pairing() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "35=A|213=abc|49=PEER|56=OWN|34=1|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

/// A length field not followed by its data field breaks the TagValue
/// pairing rule - the later data content could not be skipped safely.
#[test]
fn scan_aborts_on_length_field_with_broken_pairing() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "35=A|212=3|49=PEER|56=OWN|34=1|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

/// The scan is confined to the standard-header tag set: the first tag
/// outside it (98 = EncryptMethod, a Logon body field) ends the walk, so
/// CompIDs placed after it are never collected.
#[test]
fn scan_ignores_comp_ids_after_body_start() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "35=A|34=1|98=0|49=PEER|56=OWN|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

/// Body Data fields under tags the session layer does not know (95/96
/// here) are unreachable - the walk stops at the first body tag, so
/// their content cannot poison the identity.
#[test]
fn scan_never_enters_body_data_fields() {
    let bytes = test_helpers::frame_message(
        "FIXT.1.1",
        "35=A|49=PEER|56=OWN|34=1|98=0|95=8|96=ab|49=EV|108=30|",
    );
    assert_eq!(invalid_logon_identity(&bytes), Some(expected_id()));
}

/// A duplicated identity-relevant tag marks the message as too damaged
/// to trust.
#[test]
fn scan_aborts_on_duplicate_comp_id() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "35=A|49=PEER|49=PEER|56=OWN|34=1|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

/// An unrecognized BeginString yields no `Version`, hence no identity
/// (the caller's silent drop then matches FIX Session Layer §4.6.4).
#[test]
fn scan_aborts_on_unknown_begin_string() {
    let bytes = test_helpers::frame_message("FOO.1.1", "35=A|49=PEER|56=OWN|34=1|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

#[test]
fn scan_aborts_on_missing_comp_id() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "35=A|49=PEER|34=1|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

/// MsgType(35) must be the third tag on the wire (TagValue encoding) -
/// a message that opens its body with any other tag yields no identity.
#[test]
fn scan_aborts_when_msg_type_is_not_third_tag() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "49=PEER|35=A|56=OWN|34=1|");
    assert!(invalid_logon_identity(&bytes).is_none());
}

/// A malformed tag ends the walkable region like any out-of-domain tag -
/// identity collected before it stands, bytes after it are never
/// interpreted.
#[test]
fn scan_stops_at_malformed_tag() {
    let bytes = test_helpers::frame_message("FIXT.1.1", "35=A|49=PEER|56=OWN|garbage-no-equals");
    assert_eq!(invalid_logon_identity(&bytes), Some(expected_id()));
}

// --- EOF handling ---

#[tokio::test]
async fn clean_eof_returns_none() {
    let mut stream = stream_over(&[]).await;
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn eof_with_partial_data_returns_none() {
    let bytes = test_helpers::heartbeat_bytes(1);
    let mut stream = stream_over(&bytes[..bytes.len() / 2]).await;
    assert!(stream.next().await.is_none());
}

// --- IO error ---

#[tokio::test]
async fn io_error_yields_io_error_event() {
    let mut stream = InputStream::<_, Message>::new(
        test_helpers::FailingReader,
        test_helpers::DEFAULT_MAX_MESSAGE_SIZE,
    );

    let event = stream.next().await.unwrap();
    assert_matches!(event, InputEvent::IoError(e) if e.kind() == ErrorKind::ConnectionReset);
}
