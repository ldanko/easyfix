use std::assert_matches;

use easyfix_core::{
    base_messages::SessionRejectReasonBase,
    basic_types::{FixString, ToFixString, Utc, UtcTimestamp},
    deserializer::{DeserializeErrorKind, LogoutReason},
    message::SessionMessage,
};
use easyfix_test_messages as messages;
use messages::{
    ApplVerId, Body, EncryptMethod, Header, Heartbeat, Logon, Message, MsgDirection, MsgType,
    MsgTypeGrp, Trailer,
};
use rstest::rstest;

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
        Message::from_bytes(msg_str.replace("|", "\x01").as_bytes()).map_err(|e| e.kind),
        Err(DeserializeErrorKind::Reject {
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
/// that follow BodyLength(9) - i.e. starting at MsgType(35) and ending
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
        ("112", "ABC"), // TestReqID - a body field, hands off header -> body
        ("9999", "X"),  // undefined tag, reaches the body catch-all
    ]);

    assert_matches!(
        Message::from_bytes(&bytes).map_err(|e| e.kind),
        Err(DeserializeErrorKind::Reject {
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
        Message::from_bytes(&bytes).map_err(|e| e.kind),
        Err(DeserializeErrorKind::Reject {
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
/// Here `PossDupFlag(43)` - an optional Standard Header field - appears after
/// a body field (`TestReqID 112`).
#[test]
fn header_field_in_body_rejected_with_out_of_required_order() {
    let bytes = build_fix(&[
        ("35", "0"), // Heartbeat
        ("49", "test_sender"),
        ("56", "test_target"),
        ("34", "1"),
        ("52", "20230713-21:55:13.436187000"),
        ("112", "ABC"), // TestReqID body field - header section has ended
        ("43", "Y"),    // PossDupFlag, a Standard Header field, now out of order
    ]);

    assert_matches!(
        Message::from_bytes(&bytes).map_err(|e| e.kind),
        Err(DeserializeErrorKind::Reject {
            tag: Some(43),
            reason,
            ..
        }) if reason == SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder
    );
}

#[rstest]
#[case::header_ends_at_frame_boundary(
    "header ends at frame boundary",
    vec![],
    52,
    SessionRejectReasonBase::RequiredTagMissing,
)]
#[case::missing_sendingtime(
    "missing SendingTime",
    vec![("11", "ORDER")],
    52,
    SessionRejectReasonBase::RequiredTagMissing,
)]
#[case::misplaced_sendingtime(
    "misplaced SendingTime",
    vec![("11", "ORDER"), ("52", "20260917-12:00:00")],
    52,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::body_value_error_before_misplaced_sendingtime(
    "body value error before misplaced SendingTime",
    vec![("38", "abc"), ("52", "20260917-12:00:00")],
    52,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::missing_sendingtime_and_misplaced_optional_header_field(
    "missing SendingTime and misplaced optional header field",
    vec![("11", "ORDER"), ("43", "Y")],
    43,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::missing_sendingtime_and_body_field_after_trailer(
    "missing SendingTime and body field after trailer",
    vec![("11", "ORDER"), ("93", "3"), ("89", "ABC"), ("38", "100")],
    38,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
fn incomplete_header_distinguishes_missing_and_misplaced_fields(
    #[case] name: &str,
    #[case] tail: Vec<(&str, &str)>,
    #[case] expected_tag: u16,
    #[case] expected_reason: SessionRejectReasonBase,
) {
    let mut fields = vec![("35", "D"), ("49", "sender"), ("56", "target"), ("34", "7")];
    fields.extend(tail);
    let error = Message::from_bytes(&build_fix(&fields)).unwrap_err();
    assert!(error.header.is_none(), "{name}");
    assert_matches!(
        error.kind,
        DeserializeErrorKind::Reject { tag: Some(tag), reason, seq_num: 7, msg_type: Some(msg_type) }
            if tag == expected_tag && reason == expected_reason && msg_type.as_bytes() == b"D",
        "{name}"
    );
}

#[rstest]
#[case::transport("95", "96")]
#[case::application("354", "355")]
#[case::trailer("93", "89")]
fn incomplete_header_scan_skips_binary_data(
    #[case] length_tag: &str,
    #[case] data_tag: &str,
    #[values(false, true)] late_header: bool,
) {
    // Both transport and application pairs, plus a trailer pair. Embedded
    // delimiters and a header tag are data, not evidence of a late header.
    let data = "\x0152=20260917-12:00:00\x01";
    let length = data.len().to_string();
    let mut fields = vec![
        ("35", "A"),
        ("49", "sender"),
        ("56", "target"),
        ("34", "7"),
        (length_tag, length.as_str()),
        (data_tag, data),
    ];
    if late_header {
        fields.push(("52", "20260917-12:00:00"));
    }
    let expected_reason = if late_header {
        SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder
    } else {
        SessionRejectReasonBase::RequiredTagMissing
    };
    assert_matches!(
        Message::from_bytes(&build_fix(&fields)).unwrap_err().kind,
        DeserializeErrorKind::Reject { tag: Some(52), reason, seq_num: 7, .. }
            if reason == expected_reason,
        "length tag {length_tag}, late header {late_header}"
    );
}

#[rstest]
#[case::unknown_field("unknown field", vec![("11", "ORDER"), ("9999", "X")])]
#[case::unpaired_data("unpaired data", vec![("96", "X")])]
#[case::invalid_length("invalid length", vec![("95", "abc"), ("96", "X")])]
#[case::zero_length("zero length", vec![("95", "0"), ("96", "")])]
#[case::overflowing_length("overflowing length", vec![("95", "65536"), ("96", "X")])]
#[case::truncated_data("truncated data", vec![("95", "65535"), ("96", "X")])]
#[case::wrong_preceding_length("wrong preceding length", vec![("383", "1"), ("96", "X")])]
#[case::nonconsecutive_length("nonconsecutive length", vec![("95", "1"), ("58", "1"), ("96", "X")])]
#[case::different_data_pair("different data pair", vec![("95", "1"), ("355", "X")])]
#[case::missing_data_delimiter("missing data delimiter", vec![("95", "1"), ("96", "XX")])]
#[case::invalid_tag_syntax("invalid tag syntax", vec![("11", "ORDER"), ("038", "1")])]
fn incomplete_header_scan_stops_at_uncertain_boundaries(
    #[case] name: &str,
    #[case] tail: Vec<(&str, &str)>,
) {
    let mut fields = vec![("35", "A"), ("49", "sender"), ("56", "target"), ("34", "7")];
    fields.extend(tail);
    fields.push(("52", "20260917-12:00:00"));
    assert_matches!(
        Message::from_bytes(&build_fix(&fields)).unwrap_err().kind,
        DeserializeErrorKind::Reject { tag: Some(52), reason, seq_num: 7, .. }
            if reason == SessionRejectReasonBase::RequiredTagMissing,
        "{name}"
    );
}

#[rstest]
#[case::standalone_length("standalone Length", vec![("383", "4096")])]
#[case::length_followed_by_an_ordinary_field(
    "Length followed by an ordinary field",
    vec![("95", "3"), ("11", "ORDER")],
)]
#[case::invalid_unused_length_value(
    "invalid unused length value",
    vec![("95", "abc"), ("11", "ORDER")],
)]
#[case::length_followed_directly_by_a_header_field(
    "Length followed directly by a header field",
    vec![("95", "3")],
)]
fn incomplete_header_scan_treats_length_as_delimited_until_data(
    #[case] name: &str,
    #[case] tail: Vec<(&str, &str)>,
) {
    let mut fields = vec![("35", "A"), ("49", "sender"), ("56", "target"), ("34", "7")];
    fields.extend(tail);
    fields.push(("52", "20260921-12:00:00"));
    assert_matches!(
        Message::from_bytes(&build_fix(&fields)).unwrap_err().kind,
        DeserializeErrorKind::Reject { tag: Some(52), reason, seq_num: 7, .. }
            if reason == SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
        "{name}"
    );
}

#[rstest]
#[case::sender("49", 49)]
#[case::target("56", 56)]
fn incomplete_header_diagnosis_applies_to_other_required_fields(
    #[case] missing_tag: &str,
    #[case] expected_tag: u16,
    #[values(false, true)] misplaced: bool,
) {
    let header = [
        ("35", "0"),
        ("49", "sender"),
        ("56", "target"),
        ("34", "7"),
        ("52", "20260917-12:00:00"),
    ];
    let mut fields: Vec<_> = header
        .iter()
        .copied()
        .filter(|(tag, _)| *tag != missing_tag)
        .collect();
    fields.push(("112", "TEST"));
    if misplaced {
        fields.push((missing_tag, "late"));
    }
    let expected_reason = if misplaced {
        SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder
    } else {
        SessionRejectReasonBase::RequiredTagMissing
    };
    assert_matches!(
        Message::from_bytes(&build_fix(&fields)).unwrap_err().kind,
        DeserializeErrorKind::Reject { tag: Some(tag), reason, .. }
            if tag == expected_tag && reason == expected_reason,
        "missing tag {missing_tag}, misplaced {misplaced}"
    );
}

#[test]
fn missing_seq_num_is_reported_after_header_scan() {
    let fields = [
        ("35", "0"),
        ("49", "sender"),
        ("56", "target"),
        ("52", "20260917-12:00:00"),
        ("112", "TEST"),
    ];
    assert_matches!(
        Message::from_bytes(&build_fix(&fields)).unwrap_err().kind,
        DeserializeErrorKind::Logout(LogoutReason::MsgSeqNumMissing)
    );
}

// A late MsgSeqNum is out of order but still supplies RefSeqNum when
// constructing the Reject.
#[rstest]
#[case::without_suffix(vec![])]
#[case::with_body_and_header_suffix(vec![("11", "ORDER"), ("43", "Y")])]
fn misplaced_seq_num_supplies_reject_reference(#[case] suffix: Vec<(&str, &str)>) {
    let fields = vec![
        ("35", "0"),
        ("49", "sender"),
        ("56", "target"),
        ("52", "20260917-12:00:00"),
        ("112", "TEST"),
        ("34", "7"),
    ];
    let mut misplaced_seq_num = fields.clone();
    misplaced_seq_num.extend(suffix);
    assert_matches!(
        Message::from_bytes(&build_fix(&misplaced_seq_num)).unwrap_err().kind,
        DeserializeErrorKind::Reject { tag: Some(34), reason, seq_num: 7, .. }
            if reason == SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder
    );
}

#[rstest]
#[case::plain("ABC")]
#[case::embedded_fields("\x01112=FAKE\x0152=20260918-12:00:00\x01")]
fn trailer_signature_round_trips_after_empty_and_nonempty_body(
    #[values(false, true)] with_body: bool,
    #[case] signature: &str,
) {
    let length = signature.len().to_string();
    let mut fields = vec![
        ("35", "0"),
        ("49", "sender"),
        ("56", "target"),
        ("34", "7"),
        ("52", "20260918-12:00:00"),
    ];
    if with_body {
        fields.push(("112", "TEST"));
    }
    fields.extend([("93", length.as_str()), ("89", signature)]);
    let message = Message::from_bytes(&build_fix(&fields)).unwrap();
    assert_eq!(
        message.trailer.signature.as_deref(),
        Some(signature.as_bytes())
    );
    assert_matches!(message.body.as_ref(), Body::Heartbeat(heartbeat)
        if heartbeat.test_req_id.as_ref().map(|id| id.as_bytes())
            == with_body.then_some(b"TEST".as_slice()));

    let mut buf = vec![0u8; 4096];
    let len = message.serialize(&mut buf).unwrap();
    let restored = Message::from_bytes(&buf[..len]).unwrap();
    assert_eq!(restored.trailer.signature, message.trailer.signature);
    assert_matches!(restored.body.as_ref(), Body::Heartbeat(heartbeat)
        if heartbeat.test_req_id.as_ref().map(|id| id.as_bytes())
            == with_body.then_some(b"TEST".as_slice()));
}

#[rstest]
#[case::body_ends_at_frame_boundary(
    "body ends at frame boundary",
    vec![],
    112,
    SessionRejectReasonBase::RequiredTagMissing,
)]
#[case::missing_body_field_before_trailer(
    "missing body field before trailer",
    vec![("93", "3"), ("89", "ABC")],
    112,
    SessionRejectReasonBase::RequiredTagMissing,
)]
#[case::body_field_after_trailer(
    "body field after trailer",
    vec![("93", "3"), ("89", "ABC"), ("112", "TEST")],
    112,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::header_field_after_trailer(
    "header field after trailer",
    vec![("93", "3"), ("89", "ABC"), ("43", "Y")],
    43,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::unknown_field_before_late_body(
    "unknown field before late body",
    vec![("93", "3"), ("89", "ABC"), ("9999", "X"), ("112", "TEST")],
    112,
    SessionRejectReasonBase::RequiredTagMissing,
)]
#[case::invalid_signature_length(
    "invalid signature length",
    vec![("93", "abc"), ("89", "ABC"), ("112", "TEST")],
    112,
    SessionRejectReasonBase::RequiredTagMissing,
)]
#[case::signature_without_length(
    "signature without length",
    vec![("89", "ABC"), ("112", "TEST")],
    112,
    SessionRejectReasonBase::RequiredTagMissing,
)]
fn incomplete_body_distinguishes_missing_and_misplaced_fields(
    #[case] name: &str,
    #[case] tail: Vec<(&str, &str)>,
    #[case] expected_tag: u16,
    #[case] expected_reason: SessionRejectReasonBase,
) {
    let mut fields = vec![
        ("35", "1"),
        ("49", "sender"),
        ("56", "target"),
        ("34", "7"),
        ("52", "20260918-12:00:00"),
    ];
    fields.extend(tail);
    let error = Message::from_bytes(&build_fix(&fields)).unwrap_err();
    assert_eq!(error.header.as_ref().unwrap().msg_seq_num, 7, "{name}");
    assert_matches!(error.kind,
        DeserializeErrorKind::Reject { tag: Some(tag), reason, seq_num: 7, msg_type: Some(msg_type) }
            if tag == expected_tag && reason == expected_reason && msg_type.as_bytes() == b"1",
        "{name}");
}

#[rstest]
#[case::missing(false)]
#[case::misplaced(true)]
fn incomplete_body_scan_skips_signature_contents(#[case] late_body: bool) {
    let signature = "\x01112=FAKE\x01";
    let length = signature.len().to_string();
    let mut fields = vec![
        ("35", "1"),
        ("49", "sender"),
        ("56", "target"),
        ("34", "7"),
        ("52", "20260918-12:00:00"),
        ("93", length.as_str()),
        ("89", signature),
    ];
    if late_body {
        fields.push(("112", "TEST"));
    }
    let expected_reason = if late_body {
        SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder
    } else {
        SessionRejectReasonBase::RequiredTagMissing
    };
    assert_matches!(Message::from_bytes(&build_fix(&fields)).unwrap_err().kind,
        DeserializeErrorKind::Reject { tag: Some(112), reason, .. } if reason == expected_reason);
}

#[test]
fn missing_group_delimiter_does_not_scan_inside_field_values() {
    let bytes = build_fix(&[
        ("35", "A"),
        ("49", "sender"),
        ("56", "target"),
        ("34", "7"),
        ("52", "20260918-12:00:00"),
        ("98", "0"),
        ("108", "30"),
        ("1137", "9"),
        ("384", "1"),
        // MsgTypeGrp expects 372. Consuming this unexpected tag leaves the
        // cursor at its value; these bytes are not a section handoff.
        ("89", "93=3\x0189=ABC\x0152=FAKE\x01"),
    ]);
    let error = Message::from_bytes(&bytes).unwrap_err();
    assert_eq!(error.header.as_ref().unwrap().msg_seq_num, 7);
    assert_matches!(error.kind,
        DeserializeErrorKind::Reject { tag: Some(372), reason, seq_num: 7, .. }
            if reason == SessionRejectReasonBase::RequiredTagMissing);
}

#[rstest]
#[case::late_body(
    vec![("93", "3"), ("89", "ABC"), ("112", "LATE")],
    112,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::late_header(
    vec![("93", "3"), ("89", "ABC"), ("43", "Y")],
    43,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::unpaired_signature(
    vec![("89", "ABC")],
    89,
    SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
)]
#[case::missing_signature(vec![("93", "3")], 89, SessionRejectReasonBase::RequiredTagMissing)]
#[case::duplicate_signature(
    vec![("93", "3"), ("89", "ABC"), ("93", "3"), ("89", "DEF")],
    93,
    SessionRejectReasonBase::TagAppearsMoreThanOnce,
)]
fn trailer_errors_preserve_the_parsed_header(
    #[case] tail: Vec<(&str, &str)>,
    #[case] expected_tag: u16,
    #[case] expected_reason: SessionRejectReasonBase,
) {
    let mut fields = vec![
        ("35", "1"),
        ("49", "sender"),
        ("56", "target"),
        ("34", "7"),
        ("52", "20260918-12:00:00"),
        ("112", "TEST"),
    ];
    fields.extend(tail);
    let error = Message::from_bytes(&build_fix(&fields)).unwrap_err();
    assert_eq!(error.header.as_ref().unwrap().msg_seq_num, 7);
    assert_matches!(error.kind,
        DeserializeErrorKind::Reject { tag: Some(tag), reason, seq_num: 7, .. }
            if tag == expected_tag && reason == expected_reason);
}

/// Scenario 14e: an out-of-codeset `DefaultApplVerID(1137)` value must be
/// rejected with reason 5 (Value is incorrect). The ApplVerIDCodeSet is
/// closed (Session Layer 11.2) - both a non-numeric value and a numeric
/// value past the codeset are wire-illegal.
#[rstest]
#[case::non_numeric("X")]
#[case::outside_codeset("11")]
fn out_of_codeset_default_appl_ver_id_rejected(#[case] bad_value: &str) {
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
        Message::from_bytes(&bytes).map_err(|e| e.kind),
        Err(DeserializeErrorKind::Reject {
            tag: Some(1137),
            reason,
            ..
        }) if reason == SessionRejectReasonBase::ValueIsIncorrect
    );
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
