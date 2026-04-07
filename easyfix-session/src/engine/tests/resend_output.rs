use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::{TimePrecision, Utc},
    deserializer::raw_message,
    fix_str,
    message::{HeaderAccess, SessionMessage},
};
use easyfix_test_messages::{Body, Message};

use super::support::{assert_msg_type, limit};
use crate::{
    application::DisconnectReason,
    engine::{PendingOutput, SessionEngine, validate_gap_fill_fits},
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, as_admin, serialize_message, timestamp_offset_secs},
};

// --- resend_as_gap_fill (should message be resent as gap fill?) ---

/// Session Layer Section 4.8.5: Logon, Logout, ResendRequest, Heartbeat,
/// TestRequest and SequenceReset are never retransmitted - each is gap
/// filled - while "Reject(35=3) and XMLnonFIX(35=n) are the only session
/// messages which may be retransmitted", so Reject goes out again like an
/// application message.
#[test]
fn resend_as_gap_fill_covers_every_admin_message_but_reject() {
    let (engine, _store) = EngineBuilder::new().logged_on().build();
    let cases: [(&str, Box<Message>, bool); 8] = [
        ("Heartbeat", test_helpers::heartbeat(1, None), true),
        (
            "TestRequest",
            test_helpers::test_request(1, fix_str!("REQ")),
            true,
        ),
        ("ResendRequest", test_helpers::resend_request(1, 1, 5), true),
        (
            "SequenceReset",
            test_helpers::sequence_reset(1, 5, true),
            true,
        ),
        (
            "Logon",
            test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER")),
            true,
        ),
        ("Logout", test_helpers::logout(1), true),
        ("Reject", test_helpers::reject(1, 1), false),
        ("NewOrderSingle", test_helpers::new_order_single(1), false),
    ];
    for (name, msg, gap_fill) in cases {
        assert_eq!(
            engine.resend_as_gap_fill(&msg),
            gap_fill,
            "{name}: gap-fill on resend"
        );
    }
}

// --- flush_resend_gap with no accumulated gap ---

#[test]
fn flush_resend_gap_no_gap_no_output() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    engine.flush_resend_gap().unwrap();
    assert!(engine.take_pending().is_none());
}

#[test]
fn corrupt_history_is_fatal_without_gap_fill() {
    let (mut engine, _) = EngineBuilder::new().logged_on().build();
    assert!(engine.process_resend_message(1, b"corrupt record").is_err());
    assert!(engine.has_fatal_error());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::StorageError)
    );
    assert!(!engine.has_accumulated_resend_gap());
    assert!(engine.take_pending().is_none());
}

#[test]
fn replay_serialization_failure_is_fatal_without_gap_fill() {
    let (mut source, mut storage) = EngineBuilder::new().logged_on().build();
    let bytes = serialized_new_order_single(&mut source, &mut storage);
    let (mut engine, _) = EngineBuilder::new()
        .max_message_size(limit(140))
        .logged_on()
        .build();
    assert!(engine.process_resend_message(1, &bytes).is_err());
    assert!(engine.has_fatal_error());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::StorageError)
    );
    assert!(!engine.has_accumulated_resend_gap());
    assert!(engine.take_pending().is_none());
}

// --- accumulate + flush ---

#[test]
fn accumulate_then_flush_single_gap_fill() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    engine.accumulate_resend_gap(3);
    engine.accumulate_resend_gap(4);
    engine.flush_resend_gap().unwrap();

    // Single Transient output
    let pending = engine.take_pending().expect("expected pending output");
    let len = match pending {
        PendingOutput::Transient { len } => len,
        PendingOutput::Stored(_) => panic!("expected Transient, got Stored"),
    };

    // Deserialize the gap-fill from scratch to verify content
    let bytes = &engine.scratch()[..len];
    let (_, raw) = raw_message(bytes).expect("valid message");
    let msg = Message::from_raw_message(raw).expect("valid deserialization");
    assert_msg_type(&msg, MsgTypeBase::SequenceReset);
    let AdminBase::SequenceReset(sr) = as_admin(&msg) else {
        panic!("expected SequenceReset");
    };
    assert_eq!(sr.gap_fill_flag, Some(true));
    assert_eq!(sr.new_seq_no, 5); // end(4) + 1
    assert_eq!(msg.msg_seq_num(), 3); // begin
    assert_eq!(msg.poss_dup_flag(), Some(true));

    assert!(engine.take_pending().is_none());
}

// --- process_resend_message sets correct headers ---

/// A retransmission goes out with `PossDupFlag(43)=Y`, the original
/// `SendingTime(52)` moved into `OrigSendingTime(122)`, and a fresh
/// `SendingTime` (FIX Session Layer Section 4.8.4). The original is stamped a minute
/// in the past so that a retransmission that kept it would fail - at equal
/// stamps a `>=` proves nothing.
#[test]
fn process_resend_message_sets_poss_dup_and_orig_sending_time() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let original_sending_time = timestamp_offset_secs(-60);
    let mut original = test_helpers::new_order_single(1);
    original.set_sending_time(original_sending_time);
    engine.fill_header(&mut original, &mut storage).unwrap();
    let bytes = serialize_message(&original);

    engine.process_resend_message(1, &bytes).unwrap();

    let pending = engine.take_pending().expect("expected pending");
    let len = match pending {
        PendingOutput::Transient { len } => len,
        PendingOutput::Stored(_) => panic!("expected Transient"),
    };

    // Deserialize the resent message from scratch
    let resent_bytes = &engine.scratch()[..len];
    let (_, raw) = raw_message(resent_bytes).expect("valid");
    let resent = Message::from_raw_message(raw).expect("valid");

    // PossDupFlag=Y
    assert_eq!(resent.poss_dup_flag(), Some(true));

    // OrigSendingTime = original's SendingTime
    assert_eq!(
        resent
            .orig_sending_time()
            .expect("OrigSendingTime should be set"),
        original_sending_time
    );

    // SendingTime restamped at retransmission: after the original, and now.
    let restamped = resent.sending_time().timestamp();
    assert!(
        restamped > original_sending_time.timestamp(),
        "SendingTime must be restamped, got {restamped} for an original of {}",
        original_sending_time.timestamp()
    );
    assert!(
        (Utc::now() - restamped).num_seconds() < 5,
        "SendingTime must be the retransmission time, got {restamped}"
    );
}

#[test]
fn process_resend_message_preserves_poss_resend() {
    // Session Layer Sections 4.8.4 and 4.9: session retransmission keeps
    // the original sequence number and application-owned PossResend flag.
    for poss_resend in [None, Some(false), Some(true)] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        let mut original = test_helpers::new_order_single(7);
        original.header.poss_resend = poss_resend;
        engine.fill_header(&mut original, &mut storage).unwrap();
        let bytes = serialize_message(&original);

        engine.process_resend_message(7, &bytes).unwrap();
        let resent = drain_transient(&mut engine);

        assert_matches!(&*resent.body, Body::NewOrderSingle(_));
        assert_eq!(resent.msg_seq_num(), 7);
        assert_eq!(resent.header.poss_resend, poss_resend);
        assert_eq!(resent.poss_dup_flag(), Some(true));
    }
}

// Simulates the IO loop pattern: flush gap -> drain -> process resend -> drain.
// Sequence: gap(3), flush+drain, resend(4)+drain, gap(5), gap(6),
//           flush+drain, resend(7)+drain, final flush (no-op).
#[test]
fn multiple_non_consecutive_gaps() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let app_bytes_4 = serialized_new_order_single(&mut engine, &mut storage);
    let app_bytes_7 = serialized_new_order_single(&mut engine, &mut storage);

    // Step 1: gap(3)
    engine.accumulate_resend_gap(3);

    // Step 2: flush gap before resend(4), drain
    engine.flush_resend_gap().unwrap();
    let gf1 = drain_transient(&mut engine);
    assert_msg_type(&gf1, MsgTypeBase::SequenceReset);
    let AdminBase::SequenceReset(sr1) = as_admin(&gf1) else {
        panic!("expected SequenceReset");
    };
    assert_eq!(gf1.msg_seq_num(), 3);
    assert_eq!(sr1.new_seq_no, 4);

    // Step 3: resend(4), drain
    engine.process_resend_message(4, &app_bytes_4).unwrap();
    let resent4 = drain_transient(&mut engine);
    assert_eq!(resent4.poss_dup_flag(), Some(true));

    // Step 4: gap(5), gap(6)
    engine.accumulate_resend_gap(5);
    engine.accumulate_resend_gap(6);

    // Step 5: flush gap before resend(7), drain
    engine.flush_resend_gap().unwrap();
    let gf2 = drain_transient(&mut engine);
    assert_msg_type(&gf2, MsgTypeBase::SequenceReset);
    let AdminBase::SequenceReset(sr2) = as_admin(&gf2) else {
        panic!("expected SequenceReset");
    };
    assert_eq!(gf2.msg_seq_num(), 5);
    assert_eq!(sr2.new_seq_no, 7);

    // Step 6: resend(7), drain
    engine.process_resend_message(7, &app_bytes_7).unwrap();
    let resent7 = drain_transient(&mut engine);
    assert_eq!(resent7.poss_dup_flag(), Some(true));

    // Step 7: final flush - no accumulated gap
    engine.flush_resend_gap().unwrap();
    assert!(engine.take_pending().is_none());
}

// Active only under `#[cfg(debug_assertions)]`. Guard the invariant that
// the shared `scratch` buffer must be flushed before a second serialize:
// otherwise the second write would alias bytes from the first that are
// still queued for TCP transmission.

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "scratch")]
fn double_process_resend_message_without_drain_panics() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let bytes = serialized_new_order_single(&mut engine, &mut storage);

    engine.process_resend_message(1, &bytes).unwrap();
    // No take_pending() / write between - second call must panic.
    engine.process_resend_message(2, &bytes).unwrap();
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "scratch")]
fn double_flush_resend_gap_without_drain_panics() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();

    engine.accumulate_resend_gap(3);
    engine.flush_resend_gap().unwrap();
    // No take_pending() between - second flush must panic.
    engine.accumulate_resend_gap(5);
    engine.flush_resend_gap().unwrap();
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "scratch")]
fn process_resend_then_flush_resend_gap_without_drain_panics() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let bytes = serialized_new_order_single(&mut engine, &mut storage);

    engine.process_resend_message(1, &bytes).unwrap();
    // No take_pending() between.
    engine.accumulate_resend_gap(2);
    engine.flush_resend_gap().unwrap();
}

#[test]
fn gap_fill_fits_default_max_message_size() {
    let session_id = test_helpers::default_session_id();
    // Nanos deliberately: the widest `SendingTime` any session can configure,
    // so "4096 is enough" holds for every setting, not just the default.
    assert_matches!(
        validate_gap_fill_fits::<Message>(&session_id, 4096, TimePrecision::Nanos),
        Ok(())
    );
}

#[test]
fn gap_fill_probe_rejects_too_small_max_message_size() {
    let session_id = test_helpers::default_session_id();
    let required = validate_gap_fill_fits::<Message>(&session_id, 64, TimePrecision::Nanos)
        .expect_err("64 bytes cannot hold a gap fill");
    assert!(required > 64, "reported requirement {required} <= 64");
}

/// The probe's whole point is that it measures what the runtime path will
/// emit, and `SendingTime` is the only part whose width the settings can
/// move. A probe run at a narrower precision than the session would
/// under-measure - which is exactly the case `write_gap_fill_to_scratch`
/// treats as unreachable.
#[test]
fn gap_fill_probe_size_tracks_time_precision() {
    let session_id = test_helpers::default_session_id();

    // Find the tightest budget each precision fits in, then compare.
    let required_at = |precision| {
        validate_gap_fill_fits::<Message>(&session_id, 1, precision)
            .expect_err("1 byte cannot hold a gap fill")
    };

    let secs = required_at(TimePrecision::Secs);
    let millis = required_at(TimePrecision::Millis);
    let nanos = required_at(TimePrecision::Nanos);

    // A fraction adds the period plus its digits.
    assert_eq!(millis, secs + 4, "millis = secs + '.' + 3 digits");
    assert_eq!(nanos, secs + 10, "nanos = secs + '.' + 9 digits");
}

/// Helper: build a filled+serialized NewOrderSingle for resend tests.
fn serialized_new_order_single(
    engine: &mut SessionEngine<Message>,
    storage: &mut impl MessagesStorage,
) -> Vec<u8> {
    let mut msg = test_helpers::new_order_single(1);
    engine.fill_header(&mut msg, storage).unwrap();
    let mut buf = vec![0u8; 4096];
    let len = msg.serialize(&mut buf).expect("serialize failed");
    buf.truncate(len);
    buf
}

/// Helper: drain one Transient from pending output and deserialize from scratch.
fn drain_transient(engine: &mut SessionEngine<Message>) -> Box<Message> {
    let pending = engine.take_pending().expect("expected pending output");
    let len = match pending {
        PendingOutput::Transient { len } => len,
        PendingOutput::Stored(_) => panic!("expected Transient, got Stored"),
    };
    let bytes = &engine.scratch()[..len];
    let (_, raw) = raw_message(bytes).expect("valid message");
    Message::from_raw_message(raw).expect("valid deserialization")
}
