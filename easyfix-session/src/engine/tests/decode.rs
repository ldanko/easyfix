use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    deserializer::{DeserializeErrorKind, GarbledReason, LogoutReason},
    fix_str,
    message::SessionMessage,
};
use easyfix_test_messages::Message;

use super::support::assert_msg_type;
use crate::{
    application::DisconnectReason,
    engine::InputResult,
    initiator::SessionStart,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{
        EngineBuilder, as_admin, drain_all_admin, nz_seq, serialize_message, take_admin,
    },
};

// --- on_deserialize_error ---

#[test]
fn on_deserialize_error_garbled() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = DeserializeErrorKind::Garbled(GarbledReason::MessageNotWellFormed);
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    // Garbled: return Error for app callback, no engine action
    assert_matches!(result, InputResult::Error(..));
    assert!(engine.take_admin_output().is_none());
    assert!(!engine.should_disconnect());
    // A garbled message must NOT advance NextNumIn - it is logged and ignored,
    // and the resulting gap is recovered by ResendRequest once the next
    // well-formed message arrives (FIX Session Layer Section 4.5.2).
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

#[test]
fn on_deserialize_error_logout_missing_seq_num() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let error = DeserializeErrorKind::Logout(LogoutReason::MsgSeqNumMissing);
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    // Logout: send Logout, latch the dedicated disconnect reason, return Error
    // for the app callback.
    assert_matches!(result, InputResult::Error(_));
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::MsgSeqNumNotFound)
    );
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
}

/// Scenario 2(i): a recognized-but-wrong BeginString surfaces as a
/// `Logout(BeginStringMismatch)` deserialize error; the session must answer
/// with a Logout(35=5) + disconnect (not silently drop it). The acceptor's
/// first-Logon case is handled separately (silent drop, Section 4.6.4).
#[test]
fn on_deserialize_error_logout_begin_string_mismatch() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let error = DeserializeErrorKind::Logout(LogoutReason::BeginStringMismatch);
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
}

/// Rebuild `bytes` with a different BeginString(8) value and a recomputed
/// Checksum(10). BodyLength(9) is unaffected - it counts bytes after tag 9, and
/// the body is unchanged.
fn with_begin_string(bytes: &[u8], new_ver: &[u8]) -> Vec<u8> {
    let first_soh = bytes.iter().position(|&b| b == b'\x01').expect("8= field");
    let mut out = Vec::new();
    out.extend_from_slice(b"8=");
    out.extend_from_slice(new_ver);
    out.extend_from_slice(&bytes[first_soh..]);
    let ten = out
        .windows(3)
        .rposition(|w| w == b"10=")
        .expect("10= field");
    let sum = out[..ten].iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    out[ten + 3..ten + 6].copy_from_slice(format!("{sum:03}").as_bytes());
    out
}

/// Scenario 2(i): a well-framed message whose BeginString is a
/// recognized FIX version but not this session's (`FIX.4.2` vs `FIXT.1.1`) is
/// classified as `Logout(BeginStringMismatch)` - not garbled. (Previously the
/// generator bucketed any mismatch as `Garbled`.)
#[test]
fn deserialize_recognized_wrong_version_is_logout_mismatch() {
    let valid = serialize_message(&test_helpers::heartbeat(1, None));
    let bytes = with_begin_string(&valid, b"FIX.4.2");
    let err = <Message as SessionMessage>::from_bytes(&bytes).unwrap_err();
    assert_matches!(
        err.kind,
        DeserializeErrorKind::Logout(LogoutReason::BeginStringMismatch)
    );
}

/// FIX Session Layer Section 4.5.2: a BeginString that is not a defined FIX identifier
/// (`FOO.9.9`) is genuinely garbled, not a version mismatch.
#[test]
fn deserialize_undefined_version_is_garbled() {
    let valid = serialize_message(&test_helpers::heartbeat(1, None));
    let bytes = with_begin_string(&valid, b"FOO.9.9");
    let err = <Message as SessionMessage>::from_bytes(&bytes).unwrap_err();
    assert_matches!(
        err.kind,
        DeserializeErrorKind::Garbled(GarbledReason::InvalidBeginString)
    );
}

/// A field-level `Reject(35=3)` for an in-sequence message carries the
/// `SessionRejectReason(373)` the deserializer chose, unchanged, and
/// increments NextNumIn by 1 (FIX Session Layer Section 4.5.4). This pins the
/// session's half only - the forwarding and the counter - for every reason
/// code the deserializer can produce. Which code a given wire defect maps to
/// (Test Cases Scenario 14) is the codec's contract, pinned in
/// `easyfix-messages`; nothing here exercises that choice.
#[test]
fn on_deserialize_error_reject_forwards_reason_and_increments_next_num_in() {
    let reasons = [
        SessionRejectReasonBase::InvalidTagNumber, // 14a, reason 0
        SessionRejectReasonBase::RequiredTagMissing, // 14b, reason 1
        SessionRejectReasonBase::TagNotDefinedForThisMessageType, // 14c, reason 2
        SessionRejectReasonBase::TagSpecifiedWithoutAValue, // 14d, reason 4
        SessionRejectReasonBase::ValueIsIncorrect, // 14e, reason 5
        SessionRejectReasonBase::IncorrectDataFormatForValue, // 14f, reason 6
        SessionRejectReasonBase::InvalidMsgType,   // reason 11
        SessionRejectReasonBase::TagAppearsMoreThanOnce, // reason 13
        SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder, // 14g, reason 14
        SessionRejectReasonBase::RepeatingGroupFieldsOutOfOrder, // reason 15
        SessionRejectReasonBase::IncorrectNumInGroupCountForRepeatingGroup, // reason 16
    ];
    for reason in reasons {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
        let error = DeserializeErrorKind::Reject {
            msg_type: Some(fix_str!("D").to_owned()),
            seq_num: 5,
            tag: Some(44),
            reason: reason.into(),
        };
        let result = engine
            .on_deserialize_error(error.into(), &mut storage)
            .unwrap();
        // Reject: send Reject, return Error for app callback
        assert_matches!(result, InputResult::Error(..));
        let reject_msg = take_admin(&mut engine);
        assert_msg_type(&reject_msg, MsgTypeBase::Reject);
        let AdminBase::Reject(ref reject) = as_admin(&reject_msg) else {
            panic!("expected Reject");
        };
        assert_eq!(reject.ref_seq_num, 5);
        assert_eq!(
            reject
                .session_reject_reason
                .expect("reject reason must be set"),
            reason,
            "reject reason must be forwarded verbatim ({reason:?})",
        );
        assert_eq!(
            storage.next_target_msg_seq_num().get(),
            6,
            "NextNumIn must be incremented after in-sequence Reject ({reason:?})",
        );
        assert!(!engine.should_disconnect());
    }
}

/// Test Cases Scenario 1S(d): an invalid Logon(35=A) received by an acceptor
/// that has not yet established the session escalates after the (optional)
/// Reject - Logout(35=5) with Text(58) referencing the error condition, then
/// disconnect.
#[test]
fn on_deserialize_error_invalid_logon_escalates_when_idle() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let error = DeserializeErrorKind::Reject {
        msg_type: Some(fix_str!("A").to_owned()),
        seq_num: 1,
        tag: Some(1137),
        reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    let AdminBase::Logout(ref logout) = as_admin(&logout_msg) else {
        panic!("expected Logout");
    };
    assert!(
        logout.text.is_some(),
        "Logout must carry Text(58) referencing the error condition"
    );
    assert!(engine.should_disconnect());
    // The in-sequence rejected Logon still advances NextNumIn
    // (FIX Session Layer Section 4.5.4).
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// Scenario 1S(d), initiator side: our Logon is out, the peer's Logon
/// response fails decoding - the session can never establish, so it must
/// not linger waiting for the logon timeout: Reject, Logout, disconnect.
#[test]
fn on_deserialize_error_invalid_logon_escalates_when_logon_sent() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    drain_all_admin(&mut engine);

    let error = DeserializeErrorKind::Reject {
        msg_type: Some(fix_str!("A").to_owned()),
        seq_num: 1,
        tag: Some(98),
        reason: SessionRejectReasonBase::IncorrectDataFormatForValue.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
}

/// A spurious second `Logon<A>` is barred once established, and a decode
/// failure does not waive that: silent disconnect, no Reject. Matches the
/// with-header path
/// (`on_deserialize_error_with_header_logon_when_established_disconnects`);
/// the two used to disagree, the header-less one Rejecting and staying up.
#[test]
fn on_deserialize_error_logon_when_established_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let error = DeserializeErrorKind::Reject {
        msg_type: Some(fix_str!("A").to_owned()),
        seq_num: 1,
        tag: Some(1137),
        reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    assert!(engine.should_disconnect());
    assert!(
        engine.take_admin_output().is_none(),
        "\u{a7}4.3.1: terminate without Logout, and no Reject to a barred message"
    );
}

/// An undecodable message whose MsgType could not be recovered cannot be
/// the `Logon<A>` that a pre-logon state requires, so it fails the gate:
/// silent disconnect, no Reject, NextNumIn untouched. No 1S(d) escalation
/// either - that mandate needs a confirmed 35=A.
#[test]
fn on_deserialize_error_unknown_msg_type_pre_logon_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let error = DeserializeErrorKind::Reject {
        msg_type: None,
        seq_num: 1,
        tag: None,
        reason: SessionRejectReasonBase::RequiredTagMissing.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    assert!(engine.should_disconnect());
    assert!(engine.take_admin_output().is_none());
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
}

/// The other half of the unresolvable-MsgType rule: once established there
/// is nothing to gate, so the conservative Reject-and-continue handling
/// stands.
#[test]
fn on_deserialize_error_unknown_msg_type_rejects_when_established() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let error = DeserializeErrorKind::Reject {
        msg_type: None,
        seq_num: 1,
        tag: None,
        reason: SessionRejectReasonBase::RequiredTagMissing.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    assert!(engine.take_admin_output().is_none(), "no Logout expected");
    assert!(!engine.should_disconnect());
}

/// The gap this closes: in `LogonSent` only `Logon<A>` and `Logout<5>` are
/// permitted (Testcases Section 4.3.1 Scenario 1B(e)), and a decode failure does
/// not buy an exemption. A damaged Heartbeat used to draw a Reject to a
/// peer that had not finished logging on and to advance NextNumIn on the
/// persisted store, while its well-formed twin was disconnected outright.
#[test]
fn on_deserialize_error_non_logon_in_logon_sent_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    drain_all_admin(&mut engine);

    let error = DeserializeErrorKind::Reject {
        msg_type: Some(fix_str!("0").to_owned()),
        seq_num: 1,
        tag: Some(112),
        reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    assert!(engine.should_disconnect());
    assert!(
        engine.take_admin_output().is_none(),
        "no Reject to a peer that has not completed logon"
    );
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
}

/// `Logout<5>` IS permitted in `LogonSent`, so a damaged one keeps the
/// Reject-and-continue handling - the gate must mirror
/// `check_logon_state`, not blanket-disconnect every non-Logon.
#[test]
fn on_deserialize_error_logout_in_logon_sent_rejects() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    drain_all_admin(&mut engine);

    let error = DeserializeErrorKind::Reject {
        msg_type: Some(fix_str!("5").to_owned()),
        seq_num: 1,
        tag: Some(58),
        reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    assert!(!engine.should_disconnect());
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// A field-invalid message whose MsgSeqNum is *not* in sequence (too high) must
/// NOT advance NextNumIn - the gap is closed by ResendRequest once an
/// in-sequence message parses. Guards against a naive unconditional increment.
#[test]
fn on_deserialize_error_reject_does_not_increment_when_out_of_sequence() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let error = DeserializeErrorKind::Reject {
        msg_type: Some(fix_str!("D").to_owned()),
        seq_num: 8, // too high - a gap is present
        tag: Some(44),
        reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
    };
    let result = engine
        .on_deserialize_error(error.into(), &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Error(..));
    let _ = take_admin(&mut engine); // drain the Reject
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}
