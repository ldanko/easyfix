use assert_matches::assert_matches;
use easyfix_core::{base_messages::SessionRejectReasonBase, message::SessionMessage};
use easyfix_messages::{
    deserializer::DeserializeError,
    fields::{
        DefaultApplVerId, EncryptMethod, FixString, MsgDirection, MsgType, ToFixString, Utc,
        UtcTimestamp,
    },
    groups::MsgTypeGrp,
    messages::{BEGIN_STRING, Body, Header, Heartbeat, Logon, Message, Trailer},
};

fn header() -> Header {
    Header {
        begin_string: BEGIN_STRING.to_owned(),
        body_length: 0, // Serializer will overwrite this
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
    let serialized = msg.serialize();
    Message::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_no_present() {
    let msg = fixt_message(Box::new(Body::Logon(Logon {
        encrypt_method: EncryptMethod::None,
        heart_bt_int: 30,
        default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
        ..Default::default()
    })));
    let serialized = msg.serialize();
    Message::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_1() {
    let msg = fixt_message(Box::new(Body::Logon(Logon {
        encrypt_method: EncryptMethod::None,
        heart_bt_int: 30,
        default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
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
    let serialized = msg.serialize();
    Message::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_2() {
    let msg = fixt_message(Box::new(Body::Logon(Logon {
        encrypt_method: EncryptMethod::None,
        heart_bt_int: 30,
        default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
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
    let serialized = msg.serialize();
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
