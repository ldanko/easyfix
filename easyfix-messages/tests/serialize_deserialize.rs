use assert_matches::assert_matches;
use easyfix_core::{
    base_messages::SessionRejectReasonBase,
    basic_types::{FixString, ToFixString, Utc, UtcTimestamp},
    deserializer::DeserializeError,
    message::SessionMessage,
};
use easyfix_test_messages as messages;
use messages::{
    ApplVerId, Body, EncryptMethod, Header, Heartbeat, Logon, Message, MsgDirection, MsgType,
    MsgTypeGrp, Trailer,
};

fn header() -> Header {
    Header {
        sender_comp_id: FixString::from_ascii_lossy(b"test_sender".to_vec()),
        target_comp_id: FixString::from_ascii_lossy(b"test_target".to_vec()),
        msg_seq_num: 1,
        sending_time: UtcTimestamp::with_nanos(Utc::now()),
        ..Default::default()
    }
}

fn trailer() -> Trailer {
    Trailer {
        check_sum: FixString::from_ascii_lossy(b"000".to_vec()), // Serializer will overwrite this
        ..Default::default()
    }
}

fn fixt_message(msg: Box<Body>) -> Box<Message> {
    Box::new(Message {
        header: header(),
        body: msg,
        trailer: trailer(),
    })
}

#[test]
fn heartbeat_ok() {
    // Simple test with simple message.
    let msg = fixt_message(Box::new(Body::Heartbeat(Heartbeat { test_req_id: None })));
    let mut serialized = vec![0u8; 4096];
    let len = msg.serialize(&mut serialized).expect("serialize failed");
    serialized.truncate(len);
    Message::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_no_present() {
    let msg = fixt_message(Box::new(Body::Logon(Logon {
        encrypt_method: EncryptMethod::None,
        heart_bt_int: 30,
        default_appl_ver_id: ApplVerId::Fix50Sp2,
        ..Default::default()
    })));
    let mut serialized = vec![0u8; 4096];
    let len = msg.serialize(&mut serialized).expect("serialize failed");
    serialized.truncate(len);
    Message::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_1() {
    let msg = fixt_message(Box::new(Body::Logon(Logon {
        encrypt_method: EncryptMethod::None,
        heart_bt_int: 30,
        default_appl_ver_id: ApplVerId::Fix50Sp2,
        msg_type_grp: Some(vec![
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                msg_direction: Some(MsgDirection::Send),
                ..Default::default()
            },
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                msg_direction: Some(MsgDirection::Receive),
                ..Default::default()
            },
        ]),
        ..Default::default()
    })));
    let mut serialized = vec![0u8; 4096];
    let len = msg.serialize(&mut serialized).expect("serialize failed");
    serialized.truncate(len);
    Message::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_2() {
    let msg = fixt_message(Box::new(Body::Logon(Logon {
        encrypt_method: EncryptMethod::None,
        heart_bt_int: 30,
        default_appl_ver_id: ApplVerId::Fix50Sp2,
        msg_type_grp: Some(vec![
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                default_ver_indicator: Some(true),
                ..Default::default()
            },
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                default_ver_indicator: Some(false),
                ..Default::default()
            },
        ]),
        ..Default::default()
    })));
    let mut serialized = vec![0u8; 4096];
    let len = msg.serialize(&mut serialized).expect("serialize failed");
    serialized.truncate(len);
    Message::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn unknown_msg_type() {
    let msg_str = "8=FIXT.1.1|9=0077|35=UNKNOWN|49=test_sender|56=test_target|34=1|52=20230713-21:55:13.436187000|10=254|";

    assert_matches!(
        Message::from_bytes(msg_str.replace("|", "\x01").as_bytes()),
        Err(DeserializeError::Reject {
            tag: Some(35),
            reason,
            ..
        }) if reason == SessionRejectReasonBase::InvalidMsgType
    );
}

#[test]
fn known_msg_type() {
    let msg_str = "8=FIXT.1.1|9=0071|35=0|49=test_sender|56=test_target|34=1|52=20230713-21:55:13.436187000|10=248|";

    let msg = Message::from_bytes(msg_str.replace("|", "\x01").as_bytes()).unwrap();
    assert_eq!(msg.body.msg_type(), MsgType::Heartbeat);
}

/// Build a properly-framed FIXT.1.1 message from the `(tag, value)` pairs
/// that follow BodyLength(9) — i.e. starting at MsgType(35) and ending
/// before CheckSum(10). Computes BodyLength and CheckSum so the framing
/// layer accepts the message and parsing reaches the field-level checks.
fn build_fix(body_fields: &[(&str, &str)]) -> Vec<u8> {
    let mut body = String::new();
    for (tag, value) in body_fields {
        body.push_str(tag);
        body.push('=');
        body.push_str(value);
        body.push('\x01');
    }
    let without_checksum = format!("8=FIXT.1.1\x019={}\x01{body}", body.len());
    let checksum = without_checksum.bytes().map(u32::from).sum::<u32>() % 256;
    format!("{without_checksum}10={checksum:03}\x01").into_bytes()
}

/// Scenario 14a (`FIX_Session_Testcases`): a well-formed but
/// spec-undefined tag must be rejected with `SessionRejectReason=0`
/// (Invalid tag number), NOT `=3` (Undefined Tag). Tag 9999 is defined in
/// no dictionary; here it follows a body field (TestReqID 112), so it
/// surfaces through the body deserializer's catch-all.
#[test]
fn undefined_tag_in_body_rejected_with_invalid_tag_number() {
    let bytes = build_fix(&[
        ("35", "0"), // Heartbeat
        ("49", "test_sender"),
        ("56", "test_target"),
        ("34", "1"),
        ("52", "20230713-21:55:13.436187000"),
        ("112", "ABC"), // TestReqID — a body field, hands off header → body
        ("9999", "X"),  // undefined tag, reaches the body catch-all
    ]);

    assert_matches!(
        Message::from_bytes(&bytes),
        Err(DeserializeError::Reject {
            tag: Some(9999),
            reason,
            ..
        }) if reason == SessionRejectReasonBase::InvalidTagNumber
    );
}

/// Same mandate, header section: an undefined tag among the header fields
/// surfaces through the header deserializer's catch-all and must likewise
/// map to `InvalidTagNumber=0` (Scenario 14a).
#[test]
fn undefined_tag_in_header_rejected_with_invalid_tag_number() {
    let bytes = build_fix(&[
        ("35", "0"),
        ("49", "test_sender"),
        ("56", "test_target"),
        ("34", "1"),
        ("9999", "X"), // undefined tag, still in the header section
        ("52", "20230713-21:55:13.436187000"),
    ]);

    assert_matches!(
        Message::from_bytes(&bytes),
        Err(DeserializeError::Reject {
            tag: Some(9999),
            reason,
            ..
        }) if reason == SessionRejectReasonBase::InvalidTagNumber
    );
}

/// Scenario 14g: "Standard Header fields appear before Body fields which
/// appear before Standard Trailer fields."
///
/// A header field appearing in the Body section violates that order and must
/// be rejected with reason 14 (Tag specified out of required order),
/// NOT 2 (Tag not defined for this message type).
/// Here `PossDupFlag(43)` — an optional Standard Header field — appears after
/// a body field (`TestReqID 112`).
#[test]
fn header_field_in_body_rejected_with_out_of_required_order() {
    let bytes = build_fix(&[
        ("35", "0"), // Heartbeat
        ("49", "test_sender"),
        ("56", "test_target"),
        ("34", "1"),
        ("52", "20230713-21:55:13.436187000"),
        ("112", "ABC"), // TestReqID body field — header section has ended
        ("43", "Y"),    // PossDupFlag, a Standard Header field, now out of order
    ]);

    assert_matches!(
        Message::from_bytes(&bytes),
        Err(DeserializeError::Reject {
            tag: Some(43),
            reason,
            ..
        }) if reason == SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder
    );
}

/// Scenario 14e: an out-of-codeset `DefaultApplVerID(1137)` value must be
/// rejected with reason 5 (Value is incorrect). The ApplVerIDCodeSet is
/// closed (Session Layer 11.2) - both a non-numeric value and a numeric
/// value past the codeset are wire-illegal.
#[test]
fn out_of_codeset_default_appl_ver_id_rejected() {
    for bad_value in ["X", "11"] {
        let bytes = build_fix(&[
            ("35", "A"), // Logon
            ("49", "test_sender"),
            ("56", "test_target"),
            ("34", "1"),
            ("52", "20230713-21:55:13.436187000"),
            ("98", "0"),
            ("108", "30"),
            ("1137", bad_value),
        ]);

        assert_matches!(
            Message::from_bytes(&bytes),
            Err(DeserializeError::Reject {
                tag: Some(1137),
                reason,
                ..
            }) if reason == SessionRejectReasonBase::ValueIsIncorrect
        );
    }
}

/// Optional ApplVerID(1128): absent on the wire deserializes to `None` and
/// re-serializes without emitting the tag.
#[test]
fn absent_appl_ver_id_round_trips_as_none() {
    let bytes = build_fix(&[
        ("35", "0"), // Heartbeat
        ("49", "test_sender"),
        ("56", "test_target"),
        ("34", "1"),
        ("52", "20230713-21:55:13.436187000"),
    ]);

    let msg = Message::from_bytes(&bytes).expect("Deserialization failed");
    assert_eq!(msg.header.appl_ver_id, None);

    let mut serialized = vec![0u8; 4096];
    let len = msg.serialize(&mut serialized).expect("serialize failed");
    serialized.truncate(len);
    assert!(
        !serialized.windows(6).any(|w| w == b"\x011128="),
        "re-serialized message must not emit ApplVerID(1128)"
    );
}
