use std::assert_matches;

use easyfix_core::{
    base_messages::{
        AdminBase, MsgTypeBase, SequenceResetBase, SessionRejectReasonBase, SessionStatusBase,
    },
    basic_types::SeqNum,
    fix_str,
    message::SessionMessage,
};
use easyfix_test_messages::Message;

use super::support::assert_msg_type;
use crate::{
    application::DisconnectReason,
    engine::InputResult,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, drain_all_admin, nz_seq, take_admin},
};

// --- SequenceReset ---

#[test]
fn on_sequence_reset_gap_fill_advances_target() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::sequence_reset(1, 5, true);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

#[test]
fn on_sequence_reset_reset_advances_target() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // gap_fill=false, new_seq > target
    let msg = test_helpers::sequence_reset(1, 10, false);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 10);
}

#[test]
fn on_sequence_reset_too_low_rejects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(10)).unwrap();
    // new_seq=5 < next_target=10 -> Reject
    let msg = test_helpers::sequence_reset(10, 5, false);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let AdminBase::Reject(reject) = as_admin(&reject_msg) else {
        panic!("expected Reject");
    };
    assert_eq!(
        reject.session_reject_reason,
        Some(SessionRejectReasonBase::ValueIsIncorrect.into())
    );
    // Session Test Cases Scenario 11c: a too-low SequenceReset-Reset is
    // Rejected and must NOT change NextNumIn.
    assert_eq!(storage.next_target_msg_seq_num().get(), 10);
}

/// Session Test Cases Scenario 10e: a `SequenceReset` with
/// `GapFillFlag(123)=Y`, in sequence (`MsgSeqNum(34) == NextNumIn`), whose
/// `NewSeqNo(36) <= MsgSeqNum(34)` is an "attempt to lower sequence number"
/// and must be answered with `Reject(35=3)` (`SessionRejectReason=ValueIsIncorrect`,
/// `RefTagID=36`) - NOT silently accepted. The degenerate
/// `NewSeqNo == MsgSeqNum == NextNumIn` case was previously accepted as a no-op
/// because the comparison ignored `GapFillFlag`. `NextNumIn` must not change.
#[test]
fn on_sequence_reset_gap_fill_new_seq_no_equal_target_rejects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // GapFill in sequence (MsgSeqNum=5=NextNumIn) with NewSeqNo=5 (== MsgSeqNum).
    let msg = test_helpers::sequence_reset(5, 5, true);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let AdminBase::Reject(reject) = as_admin(&reject_msg) else {
        panic!("expected Reject");
    };
    assert_eq!(
        reject.session_reject_reason,
        Some(SessionRejectReasonBase::ValueIsIncorrect.into())
    );
    assert_eq!(reject.ref_tag_id, Some(36)); // NewSeqNo
    // The rejected SequenceReset must NOT advance NextNumIn.
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// Session Test Cases Scenario 11b: a `SequenceReset` with
/// `GapFillFlag(123)=N` and `NewSeqNo(36) == NextNumIn` is accepted (warning
/// only) - its `MsgSeqNum(34)` is ignored and it is NOT rejected. Guards the
/// GapFillFlag-aware distinction so the fix for the GapFill `==` case does
/// not over-reject the Reset `==` case.
#[test]
fn on_sequence_reset_reset_new_seq_no_equal_target_accepted() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // Reset (gap_fill=false), NewSeqNo == NextNumIn; MsgSeqNum is irrelevant.
    let msg = test_helpers::sequence_reset(99, 5, false);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    // No Reject emitted; NextNumIn unchanged.
    assert!(engine.take_admin_output().is_none());
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// Session Test Cases Scenario 11a: a `SequenceReset-Reset` (`GapFillFlag=N`)
/// is processed *without regard to its own MsgSeqNum* - here MsgSeqNum=99 is
/// wildly out of sequence yet NewSeqNo=10 still advances NextNumIn, with no
/// ResendRequest and no Reject.
#[test]
fn on_sequence_reset_reset_ignores_out_of_sequence_msg_seq_num() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // gap_fill=false, MsgSeqNum=99 (ignored), NewSeqNo=10 advances NextNumIn.
    let msg = test_helpers::sequence_reset(99, 10, false);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.take_admin_output().is_none());
    assert_eq!(storage.next_target_msg_seq_num().get(), 10);
}

/// A `SequenceReset-Reset` jumping past a parked out-of-order message drops
/// it: the queue is drained only at exactly NextNumIn, which never goes
/// down, so the entry could never be dispatched again.
#[test]
fn on_sequence_reset_reset_discards_queued_below_new_seq_no() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // Too-high app message parks at 20 and requests 5..=19.
    engine
        .on_input(test_helpers::new_order_single(20), &mut storage)
        .unwrap();
    drain_all_admin(&mut engine);
    assert!(engine.has_queued_message(20));

    let msg = test_helpers::sequence_reset(5, 30, false);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 30);
    assert!(!engine.has_queued_message(20));
    assert_eq!(engine.queued_count(), 0);
}

/// Same cut for an in-sequence `SequenceReset-GapFill` (Scenario 10b): the
/// peer skipped over a number it had already transmitted.
#[test]
fn on_sequence_reset_gap_fill_discards_queued_below_new_seq_no() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    engine
        .on_input(test_helpers::new_order_single(20), &mut storage)
        .unwrap();
    drain_all_admin(&mut engine);
    assert!(engine.has_queued_message(20));

    let msg = test_helpers::sequence_reset(5, 30, true);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 30);
    assert_eq!(engine.queued_count(), 0);
}

/// The cut is exclusive: a message parked exactly at NewSeqNo is the next
/// one due and must stay queued for the drain to pick up.
#[test]
fn on_sequence_reset_keeps_queued_message_at_new_seq_no() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    engine
        .on_input(test_helpers::new_order_single(20), &mut storage)
        .unwrap();
    drain_all_admin(&mut engine);

    let msg = test_helpers::sequence_reset(5, 20, false);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 20);
    assert!(engine.has_queued_message(20));

    let queued = engine.next_queued_message(&mut storage).unwrap();
    assert_matches!(queued, Some(InputResult::AppMsg(_)));
    assert_eq!(engine.queued_count(), 0);
}

/// A `SequenceReset`-GapFill numbered below `NextNumIn`, as a re-run resend
/// produces one.
fn gap_fill_below_target(seq: SeqNum, new_seq: SeqNum, poss_dup: bool) -> Box<Message> {
    let mut header = test_helpers::inbound_header(seq);
    header.poss_dup_flag = poss_dup.then_some(true);
    Box::new(Message::from_admin(
        header,
        AdminBase::SequenceReset(SequenceResetBase {
            gap_fill_flag: Some(true),
            new_seq_no: new_seq,
        }),
    ))
}

/// Scenario 10(c): a GapFill with `MsgSeqNum(34) < NextNumIn` and
/// `PossDupFlag(43)=Y` is the duplicate a repeated resend produces - ignored
/// outright, `NextNumIn` untouched. FIX Transport Section 5.6.2 names this the
/// hazard to watch for: a duplicate GapFill "attempting to lower the next
/// expected sequence number".
#[test]
fn on_sequence_reset_gap_fill_too_low_poss_dup_is_ignored() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(10)).unwrap();

    let result = accept_input(
        &mut engine,
        gap_fill_below_target(5, 12, true),
        &mut storage,
    );

    assert_matches!(result, InputResult::Handled);
    assert!(engine.take_admin_output().is_none(), "nothing to answer");
    assert!(!engine.should_disconnect());
    assert_eq!(
        storage.next_target_msg_seq_num().get(),
        10,
        "a lowering GapFill must not move NextNumIn either way"
    );
}

/// Scenario 10(d): the same GapFill without `PossDupFlag(43)` is a genuine
/// too-low message - `Logout<5>` naming the numbers, then disconnect, with
/// `NextNumIn` untouched.
#[test]
fn on_sequence_reset_gap_fill_too_low_without_poss_dup_logs_out() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(10)).unwrap();

    let result = accept_input(
        &mut engine,
        gap_fill_below_target(5, 12, false),
        &mut storage,
    );

    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::MsgSeqNumTooLow)
    );
    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    let AdminBase::Logout(lo) = as_admin(&logout) else {
        panic!("expected Logout");
    };
    assert_eq!(
        lo.session_status,
        Some(SessionStatusBase::ReceivedMsgSeqNumTooLow.into())
    );
    assert_eq!(
        lo.text.as_deref(),
        Some(fix_str!("MsgSeqNum too low, expected 10, got 5"))
    );
    assert_eq!(storage.next_target_msg_seq_num().get(), 10);
}
