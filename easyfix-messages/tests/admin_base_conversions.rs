//! Round-trip tests for admin base message ↔ generated message conversions.
//!
//! Each test verifies that fields survive the conversion in both directions:
//! - Incoming: Generated → Base (zero-copy borrow)
//! - Outgoing: Base → Generated (owned construction)

mod messages;

use std::borrow::Cow;

use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, HeartbeatBase, LogonBase, LogoutBase, RejectBase,
        ResendRequestBase, SequenceResetBase, SessionRejectReasonBase, SessionStatusBase,
        TestRequestBase,
    },
    fix_str,
};
use messages::{Body, Heartbeat, Logon, Logout, Reject, ResendRequest, SequenceReset, TestRequest};

// ---------------------------------------------------------------------------
// HeartbeatBase ↔ Heartbeat
// ---------------------------------------------------------------------------

#[test]
fn heartbeat_incoming_with_test_req_id() {
    let msg = Heartbeat {
        test_req_id: Some(fix_str!("TEST123").to_owned()),
    };
    let base = HeartbeatBase::from(&msg);
    assert_eq!(base.test_req_id.as_deref(), Some(fix_str!("TEST123")));
}

#[test]
fn heartbeat_incoming_without_test_req_id() {
    let msg = Heartbeat { test_req_id: None };
    let base = HeartbeatBase::from(&msg);
    assert!(base.test_req_id.is_none());
}

#[test]
fn heartbeat_outgoing_with_test_req_id() {
    let base = HeartbeatBase {
        test_req_id: Some(Cow::Owned(fix_str!("TEST123").to_owned())),
    };
    let msg = Heartbeat::from(base);
    assert_eq!(
        msg.test_req_id.as_ref().map(|s| s.as_bytes()),
        Some(b"TEST123".as_slice())
    );
}

#[test]
fn heartbeat_outgoing_without_test_req_id() {
    let base = HeartbeatBase { test_req_id: None };
    let msg = Heartbeat::from(base);
    assert!(msg.test_req_id.is_none());
}

#[test]
fn heartbeat_round_trip() {
    let original = Heartbeat {
        test_req_id: Some(fix_str!("RT_TEST").to_owned()),
    };
    let base = HeartbeatBase::from(&original);
    let reconstructed = Heartbeat::from(base);
    assert_eq!(original.test_req_id, reconstructed.test_req_id);
}

// ---------------------------------------------------------------------------
// TestRequestBase ↔ TestRequest
// ---------------------------------------------------------------------------

#[test]
fn test_request_incoming() {
    let msg = TestRequest {
        test_req_id: fix_str!("REQ42").to_owned(),
    };
    let base = TestRequestBase::from(&msg);
    assert_eq!(base.test_req_id.as_bytes(), b"REQ42");
}

#[test]
fn test_request_outgoing() {
    let base = TestRequestBase {
        test_req_id: Cow::Owned(fix_str!("REQ42").to_owned()),
    };
    let msg = TestRequest::from(base);
    assert_eq!(msg.test_req_id.as_bytes(), b"REQ42");
}

#[test]
fn test_request_round_trip() {
    let original = TestRequest {
        test_req_id: fix_str!("ROUND_TRIP").to_owned(),
    };
    let base = TestRequestBase::from(&original);
    let reconstructed = TestRequest::from(base);
    assert_eq!(original.test_req_id, reconstructed.test_req_id);
}

// ---------------------------------------------------------------------------
// ResendRequestBase ↔ ResendRequest
// ---------------------------------------------------------------------------

#[test]
fn resend_request_incoming() {
    let msg = ResendRequest {
        begin_seq_no: 5,
        end_seq_no: 10,
    };
    let base = ResendRequestBase::from(&msg);
    assert_eq!(base.begin_seq_no, 5);
    assert_eq!(base.end_seq_no, 10);
}

#[test]
fn resend_request_outgoing() {
    let base = ResendRequestBase {
        begin_seq_no: 1,
        end_seq_no: 0, // 0 = infinity in FIX
    };
    let msg = ResendRequest::from(base);
    assert_eq!(msg.begin_seq_no, 1);
    assert_eq!(msg.end_seq_no, 0);
}

#[test]
fn resend_request_round_trip() {
    let original = ResendRequest {
        begin_seq_no: 100,
        end_seq_no: 200,
    };
    let base = ResendRequestBase::from(&original);
    let reconstructed = ResendRequest::from(base);
    assert_eq!(original.begin_seq_no, reconstructed.begin_seq_no);
    assert_eq!(original.end_seq_no, reconstructed.end_seq_no);
}

// ---------------------------------------------------------------------------
// SequenceResetBase ↔ SequenceReset
// ---------------------------------------------------------------------------

#[test]
fn sequence_reset_incoming() {
    let msg = SequenceReset {
        gap_fill_flag: Some(true),
        new_seq_no: 42,
    };
    let base = SequenceResetBase::from(&msg);
    assert_eq!(base.gap_fill_flag, Some(true));
    assert_eq!(base.new_seq_no, 42);
}

#[test]
fn sequence_reset_incoming_no_gap_fill() {
    let msg = SequenceReset {
        gap_fill_flag: None,
        new_seq_no: 10,
    };
    let base = SequenceResetBase::from(&msg);
    assert!(base.gap_fill_flag.is_none());
    assert_eq!(base.new_seq_no, 10);
}

#[test]
fn sequence_reset_outgoing() {
    let base = SequenceResetBase {
        gap_fill_flag: Some(true),
        new_seq_no: 50,
    };
    let msg = SequenceReset::from(base);
    assert_eq!(msg.gap_fill_flag, Some(true));
    assert_eq!(msg.new_seq_no, 50);
}

#[test]
fn sequence_reset_round_trip() {
    let original = SequenceReset {
        gap_fill_flag: Some(false),
        new_seq_no: 99,
    };
    let base = SequenceResetBase::from(&original);
    let reconstructed = SequenceReset::from(base);
    assert_eq!(original.gap_fill_flag, reconstructed.gap_fill_flag);
    assert_eq!(original.new_seq_no, reconstructed.new_seq_no);
}

// ---------------------------------------------------------------------------
// LogoutBase ↔ Logout
// ---------------------------------------------------------------------------

#[test]
fn logout_incoming_with_text() {
    let msg = Logout {
        text: Some(fix_str!("Session ended").to_owned()),
        ..Default::default()
    };
    let base = LogoutBase::from(&msg);
    assert_eq!(base.text.as_deref(), Some(fix_str!("Session ended")));
}

#[test]
fn logout_incoming_without_text() {
    let msg = Logout::default();
    let base = LogoutBase::from(&msg);
    assert!(base.text.is_none());
}

#[test]
fn logout_outgoing_with_text() {
    let base = LogoutBase {
        session_status: None,
        text: Some(Cow::Owned(fix_str!("Goodbye").to_owned())),
    };
    let msg = Logout::from(base);
    assert_eq!(
        msg.text.as_ref().map(|s| s.as_bytes()),
        Some(b"Goodbye".as_slice())
    );
}

#[test]
fn logout_round_trip() {
    let original = Logout {
        text: Some(fix_str!("Round trip text").to_owned()),
        ..Default::default()
    };
    let base = LogoutBase::from(&original);
    let reconstructed = Logout::from(base);
    assert_eq!(original.text, reconstructed.text);
}

// ---------------------------------------------------------------------------
// RejectBase ↔ Reject
// ---------------------------------------------------------------------------

#[test]
fn reject_incoming_full() {
    use messages::SessionRejectReason;

    let msg = Reject {
        ref_seq_num: 7,
        ref_tag_id: Some(35),
        ref_msg_type: Some(fix_str!("D").to_owned()),
        session_reject_reason: Some(SessionRejectReason::try_from(1i64).unwrap()), // RequiredTagMissing
        text: Some(fix_str!("Missing required tag").to_owned()),
        ..Default::default()
    };
    let base = RejectBase::from(&msg);
    assert_eq!(base.ref_seq_num, 7);
    assert_eq!(base.ref_tag_id, Some(35));
    assert_eq!(base.ref_msg_type.as_deref(), Some(fix_str!("D")));
    // Incoming: newtype field has the validated Int value
    assert_eq!(base.session_reject_reason.map(|f| f.into_inner()), Some(1),);
    assert_eq!(base.text.as_deref(), Some(fix_str!("Missing required tag")));
}

#[test]
fn reject_incoming_minimal() {
    let msg = Reject {
        ref_seq_num: 3,
        ..Default::default()
    };
    let base = RejectBase::from(&msg);
    assert_eq!(base.ref_seq_num, 3);
    assert!(base.ref_tag_id.is_none());
    assert!(base.ref_msg_type.is_none());
    assert!(base.session_reject_reason.is_none());
    assert!(base.text.is_none());
}

#[test]
fn reject_outgoing_with_reason() {
    let base = RejectBase {
        ref_seq_num: 5,
        ref_tag_id: Some(49),
        ref_msg_type: Some(Cow::Owned(fix_str!("A").to_owned())),
        session_reject_reason: Some(SessionRejectReasonBase::CompIdProblem.into()),
        text: Some(Cow::Owned(fix_str!("CompID problem").to_owned())),
    };
    let msg = Reject::from(base);
    assert_eq!(msg.ref_seq_num, 5);
    assert_eq!(msg.ref_tag_id, Some(49));
    assert_eq!(
        msg.ref_msg_type.as_ref().map(|s| s.as_bytes()),
        Some(b"A".as_slice())
    );
    // SessionRejectReason value 9 = CompIdProblem
    assert_eq!(
        msg.session_reject_reason.as_ref().map(|v| v.as_bytes()),
        Some(b"9".as_slice()),
    );
    assert_eq!(
        msg.text.as_ref().map(|s| s.as_bytes()),
        Some(b"CompID problem".as_slice())
    );
}

#[test]
fn reject_round_trip_preserves_copy_fields() {
    let original = Reject {
        ref_seq_num: 42,
        ref_tag_id: Some(371),
        ref_msg_type: Some(fix_str!("8").to_owned()),
        text: Some(fix_str!("test").to_owned()),
        ..Default::default()
    };
    let base = RejectBase::from(&original);
    let reconstructed = Reject::from(base);
    assert_eq!(original.ref_seq_num, reconstructed.ref_seq_num);
    assert_eq!(original.ref_tag_id, reconstructed.ref_tag_id);
    assert_eq!(original.ref_msg_type, reconstructed.ref_msg_type);
    assert_eq!(original.text, reconstructed.text);
}

// ---------------------------------------------------------------------------
// LogonBase ↔ Logon
// ---------------------------------------------------------------------------

#[test]
fn logon_incoming() {
    use messages::{DefaultApplVerId, EncryptMethod, SessionStatus};

    let msg = Logon {
        encrypt_method: EncryptMethod::try_from(0i64).unwrap(),
        heart_bt_int: 30,
        reset_seq_num_flag: Some(true),
        next_expected_msg_seq_num: Some(5),
        default_appl_ver_id: DefaultApplVerId::from_bytes(b"9").unwrap(), // FIX50SP2
        session_status: Some(SessionStatus::try_from(0i64).unwrap()),     // SessionActive
        ..Default::default()
    };
    let base = LogonBase::from(&msg);

    assert_eq!(base.encrypt_method, EncryptMethodBase::None);
    assert_eq!(base.encrypt_method_raw, 0);
    assert_eq!(base.heart_bt_int, 30);
    assert_eq!(base.reset_seq_num_flag, Some(true));
    assert_eq!(base.next_expected_msg_seq_num, Some(5));
    assert_eq!(base.default_appl_ver_id.as_deref(), Some(fix_str!("9")));
    // SessionStatus: newtype field has the validated Int value
    assert_eq!(base.session_status.map(|f| f.into_inner()), Some(0),);
}

#[test]
fn logon_incoming_minimal() {
    use messages::{DefaultApplVerId, EncryptMethod};

    let msg = Logon {
        encrypt_method: EncryptMethod::try_from(0i64).unwrap(),
        heart_bt_int: 60,
        default_appl_ver_id: DefaultApplVerId::from_bytes(b"9").unwrap(),
        ..Default::default()
    };
    let base = LogonBase::from(&msg);
    assert_eq!(base.encrypt_method_raw, 0);
    assert_eq!(base.heart_bt_int, 60);
    assert!(base.reset_seq_num_flag.is_none());
    assert!(base.next_expected_msg_seq_num.is_none());
    assert!(base.session_status.is_none());
}

#[test]
fn logon_outgoing() {
    let base = LogonBase {
        encrypt_method: EncryptMethodBase::None,
        encrypt_method_raw: 0,
        heart_bt_int: 30,
        reset_seq_num_flag: None,
        next_expected_msg_seq_num: Some(1),
        default_appl_ver_id: Some(Cow::Owned(fix_str!("9").to_owned())),
        session_status: Some(SessionStatusBase::SessionActive.into()),
    };
    let msg = Logon::from(base);

    // EncryptMethod value "0"
    assert_eq!(msg.encrypt_method.as_bytes(), b"0");
    assert_eq!(msg.heart_bt_int, 30);
    assert!(msg.reset_seq_num_flag.is_none());
    assert_eq!(msg.next_expected_msg_seq_num, Some(1));
    // DefaultApplVerId value "9"
    assert_eq!(msg.default_appl_ver_id.as_bytes(), b"9");
    // SessionStatus value "0" = SessionActive
    assert_eq!(
        msg.session_status.as_ref().map(|v| v.as_bytes()),
        Some(b"0".as_slice()),
    );
}

#[test]
fn logon_outgoing_without_optional_fields() {
    let base = LogonBase {
        encrypt_method: EncryptMethodBase::None,
        encrypt_method_raw: 0,
        heart_bt_int: 60,
        reset_seq_num_flag: None,
        next_expected_msg_seq_num: None,
        default_appl_ver_id: None,
        session_status: None,
    };
    let msg = Logon::from(base);
    assert_eq!(msg.heart_bt_int, 60);
    assert!(msg.reset_seq_num_flag.is_none());
    assert!(msg.next_expected_msg_seq_num.is_none());
    assert!(msg.session_status.is_none());
}

#[test]
fn logon_round_trip_copy_fields() {
    use messages::{DefaultApplVerId, EncryptMethod};

    let original = Logon {
        encrypt_method: EncryptMethod::try_from(0i64).unwrap(),
        heart_bt_int: 30,
        reset_seq_num_flag: Some(true),
        next_expected_msg_seq_num: Some(10),
        default_appl_ver_id: DefaultApplVerId::from_bytes(b"9").unwrap(),
        ..Default::default()
    };
    let base = LogonBase::from(&original);
    let reconstructed = Logon::from(base);
    assert_eq!(original.heart_bt_int, reconstructed.heart_bt_int);
    assert_eq!(
        original.reset_seq_num_flag,
        reconstructed.reset_seq_num_flag
    );
    assert_eq!(
        original.next_expected_msg_seq_num,
        reconstructed.next_expected_msg_seq_num
    );
    assert_eq!(original.encrypt_method, reconstructed.encrypt_method);
    assert_eq!(
        original.default_appl_ver_id,
        reconstructed.default_appl_ver_id
    );
}

// ---------------------------------------------------------------------------
// AdminBase dispatch ↔ Message
// ---------------------------------------------------------------------------

#[test]
fn admin_base_dispatch_incoming_heartbeat() {
    let msg = Body::Heartbeat(Heartbeat {
        test_req_id: Some(fix_str!("TEST123").to_owned()),
    });

    let base = msg
        .try_as_admin_base()
        .expect("expected heartbeat admin base");

    match base {
        AdminBase::Heartbeat(base) => {
            assert_eq!(base.test_req_id.as_deref(), Some(fix_str!("TEST123")));
        }
        other => panic!("expected Heartbeat admin base, got {other:?}"),
    }
}

#[test]
fn admin_base_dispatch_outgoing_logon() {
    let admin = AdminBase::Logon(LogonBase {
        encrypt_method: EncryptMethodBase::None,
        encrypt_method_raw: 0,
        heart_bt_int: 30,
        reset_seq_num_flag: Some(true),
        next_expected_msg_seq_num: Some(7),
        default_appl_ver_id: Some(Cow::Owned(fix_str!("9").to_owned())),
        session_status: None,
    });

    let msg = Body::from(admin);

    match msg {
        Body::Logon(logon) => {
            assert_eq!(logon.encrypt_method.as_bytes(), b"0");
            assert_eq!(logon.heart_bt_int, 30);
            assert_eq!(logon.reset_seq_num_flag, Some(true));
            assert_eq!(logon.next_expected_msg_seq_num, Some(7));
            assert_eq!(logon.default_appl_ver_id.as_bytes(), b"9");
        }
        other => panic!("expected Logon message, got {other:?}"),
    }
}

#[test]
fn admin_base_dispatch_outgoing_heartbeat() {
    let admin = AdminBase::Heartbeat(HeartbeatBase {
        test_req_id: Some(Cow::Owned(fix_str!("PING").to_owned())),
    });

    let msg = Body::from(admin);

    match msg {
        Body::Heartbeat(heartbeat) => {
            assert_eq!(
                heartbeat.test_req_id.as_ref().map(|value| value.as_bytes()),
                Some(b"PING".as_slice())
            );
        }
        other => panic!("expected Heartbeat message, got {other:?}"),
    }
}
