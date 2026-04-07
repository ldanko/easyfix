use std::ops::RangeInclusive;

use easyfix_core::{
    base_messages::MsgTypeBase,
    basic_types::SeqNum,
    deserializer::raw_message,
    message::{HeaderAccess, MsgCat, SessionMessage},
};
use easyfix_test_messages::Message;
use tokio::io;

use super::wire::wire_bytes;
use crate::{
    io::{flush_output, process_one_resend},
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, nz_seq},
};

/// `process_one_resend` must produce at most one [`PendingOutput::Transient`]
/// per call so the shared `engine.scratch` buffer is drained before the
/// next serialize overwrites it. Specifically: when the next seq_num is a
/// real resend AND a gap-fill is currently accumulating, the function
/// flushes the gap on this invocation and leaves the cursor on the resend's
/// seq_num - the resend itself is processed on the following call.
///
/// This test drives `process_one_resend` end-to-end across a 3..=4 range
/// where seq 3 is admin (gap-filled) and seq 4 is app (resent), and asserts
/// the wire output is the gap-fill SequenceReset followed by the resent
/// NewOrderSingle with `PossDupFlag=Y`.
#[tokio::test]
async fn process_one_resend_defers_real_resend_with_pending_gap() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let mut app = test_helpers::StubApp;

    // Skip seq 1 and 2 - irrelevant to this test. Commits land at 3 and 4.
    storage.set_next_sender_msg_seq_num(nz_seq(3)).unwrap();

    // seq 3 - admin Heartbeat (resend_as_gap_fill returns true).
    engine.send_heartbeat(None);
    let mut hb = engine.take_admin_output().expect("heartbeat queued");
    assert!(engine.fill_header(&mut hb, &mut storage).unwrap());
    engine
        .commit_send(hb, &mut storage)
        .expect("commit succeeds");

    // seq 4 - app NewOrderSingle (default should_gap_fill returns false).
    let mut nos = test_helpers::new_order_single_with_empty_header();
    assert!(engine.fill_header(&mut nos, &mut storage).unwrap());
    engine
        .commit_send(nos, &mut storage)
        .expect("commit succeeds");

    // Drop the Stored entries the commits pushed - they're outside the
    // scope of this test. The resend path operates on storage bytes,
    // not the live output queue.
    while engine.take_pending().is_some() {}

    let mut active_resend: Option<RangeInclusive<SeqNum>> = Some(3..=4);
    let (mut writer, reader) = io::duplex(16384);

    // Iteration 1: seq 3 (admin) - accumulate gap, advance to 4..=4.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut app)
        .await
        .unwrap();
    assert!(cont, "iter 1 should continue");
    assert_eq!(active_resend.as_ref().unwrap(), &(4..=4));
    assert!(engine.has_accumulated_resend_gap());
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .expect("flush ok");

    // Iteration 2: would resend seq 4, but a gap-fill is pending -
    // flush the gap, defer the resend. Cursor must NOT advance.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut app)
        .await
        .unwrap();
    assert!(cont, "iter 2 should continue");
    assert_eq!(
        active_resend.as_ref().unwrap(),
        &(4..=4),
        "cursor must stay on seq 4 when deferring resend"
    );
    assert!(
        !engine.has_accumulated_resend_gap(),
        "gap-fill should have been flushed"
    );
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .expect("flush ok");

    // Iteration 3: seq 4 (app, no gap pending) - process resend, exhaust range.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut app)
        .await
        .unwrap();
    assert!(cont, "iter 3 should continue");
    assert!(active_resend.is_none(), "range exhausted after iter 3");
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .expect("flush ok");

    // Iteration 4: range already None - returns false.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut app)
        .await
        .unwrap();
    assert!(!cont, "iter 4 - range exhausted, should return false");

    let wire = wire_bytes(writer, reader).await;

    // First wire message: gap-fill SequenceReset for seq 3.
    let (rest, raw1) = raw_message(&wire).expect("first wire message must parse");
    let parsed1 = Message::from_raw_message(raw1).expect("first message must deserialize");
    assert_eq!(
        SessionMessage::msg_type(&*parsed1),
        MsgTypeBase::SequenceReset,
        "first wire message should be the gap-fill SequenceReset"
    );
    assert_eq!(
        HeaderAccess::msg_seq_num(&*parsed1),
        3,
        "gap-fill should reference seq 3"
    );

    // Second wire message: resent NewOrderSingle at seq 4 with PossDupFlag=Y.
    let (rest, raw2) = raw_message(rest).expect("second wire message must parse");
    let parsed2 = Message::from_raw_message(raw2).expect("second message must deserialize");
    assert_eq!(parsed2.msg_cat(), MsgCat::App);
    assert_eq!(HeaderAccess::msg_seq_num(&*parsed2), 4);
    assert_eq!(HeaderAccess::poss_dup_flag(&*parsed2), Some(true));

    assert!(rest.is_empty(), "no leftover bytes on wire");
}
