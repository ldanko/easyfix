use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase, SessionStatusBase},
    fix_str,
};

use super::support::assert_msg_type;
use crate::{
    application::DisconnectReason,
    engine::InputResult,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, as_admin, nz_seq, take_admin, timestamp_offset_secs},
};

// --- on_deserialize_error with recovered header ---
//
// When the failed message's header parsed (DeserializeError.header is Some),
// header verdicts run through the same validation as cleanly-parsed
// messages and take precedence over the body-level error
// (FIX Session Layer 4.8.1/4.8.2, Scenario 2(b)/(c)/(e);
// DESIGN-parse-error-header.md).

/// Scenario 2(c): too-low MsgSeqNum without PossDup on a broken-body
/// message - the header verdict wins over the body Reject:
/// Logout(ReceivedMsgSeqNumTooLow) + disconnect, no Reject, NextNumIn
/// unchanged. Mirrors `too_low_no_poss_dup_logs_out_and_disconnects`
/// on the clean path.
#[test]
fn on_deserialize_error_with_header_too_low_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        test_helpers::inbound_header(1),
        Some(44),
        SessionRejectReasonBase::IncorrectDataFormatForValue,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(_));
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::MsgSeqNumTooLow)
    );
    assert!(engine.should_disconnect());
    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    let AdminBase::Logout(ref lo) = as_admin(&logout) else {
        panic!("expected Logout");
    };
    assert_eq!(
        lo.session_status,
        Some(SessionStatusBase::ReceivedMsgSeqNumTooLow.into())
    );
    // Only the Logout - the body-level Reject must NOT be sent.
    assert!(engine.take_admin_output().is_none());
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// Scenario 2(e): too-low + PossDupFlag=Y with a valid OrigSendingTime on
/// a broken-body message is a duplicate of an already-received message -
/// ignored silently: no Reject, no Logout, NextNumIn unchanged. Critical
/// during resend: the peer redelivers with PossDup=Y, and a naive
/// "too low -> Logout" would kill the session on a broken duplicate.
#[test]
fn on_deserialize_error_with_header_duplicate_is_ignored() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let mut header = test_helpers::inbound_header(1);
    header.poss_dup_flag = Some(true);
    header.orig_sending_time = Some(timestamp_offset_secs(-10));
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        header,
        Some(44),
        SessionRejectReasonBase::IncorrectDataFormatForValue,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    assert!(!engine.should_disconnect());
    assert!(engine.take_admin_output().is_none());
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// A too-low PossDup=Y message without OrigSendingTime(122): the
/// header-level Reject (RequiredTagMissing, tag 122) replaces the
/// body-level one, mirroring `check_seq_num_too_low` on the clean path.
/// No advance - the message is below NextNumIn.
#[test]
fn on_deserialize_error_with_header_poss_dup_missing_orig_time_rejects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let mut header = test_helpers::inbound_header(1);
    header.poss_dup_flag = Some(true);
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        header,
        Some(44),
        SessionRejectReasonBase::IncorrectDataFormatForValue,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let AdminBase::Reject(ref reject) = as_admin(&reject_msg) else {
        panic!("expected Reject");
    };
    assert_eq!(reject.ref_seq_num, 1);
    assert_eq!(reject.ref_tag_id, Some(122));
    assert_eq!(
        reject.session_reject_reason,
        Some(SessionRejectReasonBase::RequiredTagMissing.into())
    );
    assert!(!engine.should_disconnect());
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// Scenario 2(b) + Session Layer 4.8.2: a too-high broken-body message
/// triggers gap recovery immediately. The unparseable message cannot be
/// queued, so the ResendRequest range extends THROUGH its seq num
/// (drop-and-request strategy, 4.8.2 Figure 12); no Reject is sent now
/// (4.8.2: not processed before the gap is filled) and NextNumIn is
/// unchanged. The redelivered copy then arrives in sequence and takes
/// the normal Reject path with the 4.5.4 advance.
#[test]
fn on_deserialize_error_with_header_too_high_requests_resend_through_failed_seq() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        test_helpers::inbound_header(9),
        Some(44),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    let rr_msg = take_admin(&mut engine);
    assert_msg_type(&rr_msg, MsgTypeBase::ResendRequest);
    let AdminBase::ResendRequest(ref rr) = as_admin(&rr_msg) else {
        panic!("expected ResendRequest");
    };
    assert_eq!(rr.begin_seq_no, 5);
    assert_eq!(rr.end_seq_no, 9);
    assert!(engine.take_admin_output().is_none(), "no Reject expected");
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
    assert!(!engine.should_disconnect());

    // Gap filled; the peer redelivers the broken message (PossDup=Y) in
    // sequence - now the body Reject stands and NextNumIn advances.
    storage.set_next_target_msg_seq_num(nz_seq(9)).unwrap();
    let mut header = test_helpers::inbound_header(9);
    header.poss_dup_flag = Some(true);
    header.orig_sending_time = Some(timestamp_offset_secs(-10));
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        header,
        Some(44),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    assert_eq!(storage.next_target_msg_seq_num().get(), 10);
}

/// In-sequence broken app message with the header recovered: header
/// verdicts pass, the body Reject stands and NextNumIn advances
/// (Session Layer 4.5.4) - parity with the header-less path.
#[test]
fn on_deserialize_error_with_header_in_sequence_rejects_and_increments() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        test_helpers::inbound_header(5),
        Some(44),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let AdminBase::Reject(ref reject) = as_admin(&reject_msg) else {
        panic!("expected Reject");
    };
    assert_eq!(reject.ref_seq_num, 5);
    assert_eq!(reject.ref_tag_id, Some(44));
    assert!(!engine.should_disconnect());
    assert_eq!(storage.next_target_msg_seq_num().get(), 6);
}

/// An in-sequence broken-body SequenceReset(35=4): Reject is sent but
/// NextNumIn must NOT advance. A Reset (GapFill=N) is processed without
/// regard to its own MsgSeqNum (Session Layer 4.8.8) and the flag is
/// unknowable from a broken body, so its seq num is never treated as
/// consumed - mirroring `consume_seq_num` on the clean path.
#[test]
fn on_deserialize_error_with_header_sequence_reset_rejects_without_advance() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = test_helpers::body_reject_error(
        fix_str!("4"),
        test_helpers::inbound_header(5),
        Some(36),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    assert!(engine.take_admin_output().is_none());
    assert!(!engine.should_disconnect());
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// A broken-body SequenceReset below NextNumIn must NOT trigger the
/// too-low Logout: a Reset (GapFill=N) ignores its own MsgSeqNum
/// (Session Layer 4.8.8) and the GapFillFlag is unknowable from a broken
/// body - the seq-num checks are skipped entirely, mirroring
/// `validate_sequence_reset`. Generic Reject, no advance, session up.
#[test]
fn on_deserialize_error_with_header_sequence_reset_too_low_stays_up() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = test_helpers::body_reject_error(
        fix_str!("4"),
        test_helpers::inbound_header(1),
        Some(36),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    assert!(engine.take_admin_output().is_none(), "no Logout expected");
    assert!(!engine.should_disconnect());
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// Scenario 2(k): a CompID mismatch on a broken-body message - the header
/// verdict (Reject(CompIDProblem) + Logout + disconnect, with the 2(k)
/// increment) replaces the body-level Reject.
#[test]
fn on_deserialize_error_with_header_comp_id_mismatch_escalates() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        test_helpers::header(5, fix_str!("EVIL"), fix_str!("SENDER")),
        Some(44),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(_));
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidCompId)
    );
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let AdminBase::Reject(ref reject) = as_admin(&reject_msg) else {
        panic!("expected Reject");
    };
    assert_eq!(
        reject.session_reject_reason,
        Some(SessionRejectReasonBase::CompIdProblem.into())
    );
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
    // Scenario 2(k) step 2: increment NextNumIn.
    assert_eq!(storage.next_target_msg_seq_num().get(), 6);
}

/// A broken-body message whose header is valid but whose type is not
/// allowed in the current logon state (here: an app message before any
/// Logon) disconnects SILENTLY - no Reject, no Logout - exactly like the
/// clean path's `InvalidLogonState` reaction, with the reason surfaced.
#[test]
fn on_deserialize_error_with_header_wrong_logon_state_disconnects_silently() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        test_helpers::inbound_header(1),
        Some(44),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(_));
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.should_disconnect());
    assert!(engine.take_admin_output().is_none(), "silent disconnect");
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
}

/// An established session receiving a broken-body Logon WITH a recovered
/// header: a second Logon is not allowed while established (no reset
/// flag is knowable from a broken body), so the header verdict is
/// `InvalidLogonState` - silent disconnect, parity with the clean path's
/// reaction to a spurious parsed Logon. Contrast with the header-less
/// fallback (`on_deserialize_error_invalid_logon_no_escalation_when_established`).
#[test]
fn on_deserialize_error_with_header_logon_when_established_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let error = test_helpers::body_reject_error(
        fix_str!("A"),
        test_helpers::inbound_header(1),
        Some(98),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(_));
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.should_disconnect());
    assert!(engine.take_admin_output().is_none(), "silent disconnect");
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
}

/// Scenario 1S(d): an invalid Logon during the logon exchange still
/// escalates to Reject + Logout + disconnect when the header was
/// recovered - the escalation is not lost on the with-header path, and
/// the in-sequence rejected Logon still advances NextNumIn (4.5.4).
#[test]
fn on_deserialize_error_with_header_invalid_logon_escalates_when_idle() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let error = test_helpers::body_reject_error(
        fix_str!("A"),
        test_helpers::inbound_header(1),
        Some(1137),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(_));
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}
