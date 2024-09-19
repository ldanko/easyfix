use assert_matches::assert_matches;
use easyfix_messages::{
    deserializer::{DeserializeError, ParseRejectReason},
    fields::{
        DefaultApplVerId, EncryptMethod, FixString, MsgDirection, MsgType, ToFixString, Utc,
        UtcTimestamp,
    },
    groups::MsgTypeGrp,
    messages::{FixtMessage, Header, Heartbeat, Logon, Message, Trailer, BEGIN_STRING},
};

fn header(msg_type: MsgType) -> Box<Header> {
    Box::new(Header {
        begin_string: BEGIN_STRING.to_owned(),
        body_length: 0, // Serializer will overwrite this
        msg_type,
        sender_comp_id: FixString::from_ascii_lossy(b"test_sender".to_vec()),
        target_comp_id: FixString::from_ascii_lossy(b"test_target".to_vec()),
        msg_seq_num: 1,
        sending_time: UtcTimestamp::with_nanos(Utc::now()),
        ..Default::default()
    })
}

fn trailer() -> Box<Trailer> {
    Box::new(Trailer {
        check_sum: FixString::from_ascii_lossy(b"000".to_vec()), // Serializer will overwrite this
        ..Default::default()
    })
}

fn fixt_message(msg: Box<Message>) -> Box<FixtMessage> {
    Box::new(FixtMessage {
        header: header(msg.msg_type()),
        body: msg,
        trailer: trailer(),
    })
}

#[test]
fn heartbeat_ok() {
    // Simple test with simple message.
    let msg = fixt_message(Box::new(Message::Heartbeat(Heartbeat {
        test_req_id: None,
    })));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_no_present() {
    let msg = fixt_message(Box::new(Message::Logon(Logon {
        encrypt_method: EncryptMethod::NoneOther,
        heart_bt_int: 30,
        raw_data: None,
        reset_seq_num_flag: None,
        next_expected_msg_seq_num: None,
        max_message_size: None,
        test_message_indicator: None,
        username: None,
        password: None,
        default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
        msg_type_grp: None,
    })));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_1() {
    let msg = fixt_message(Box::new(Message::Logon(Logon {
        encrypt_method: EncryptMethod::NoneOther,
        heart_bt_int: 30,
        raw_data: None,
        reset_seq_num_flag: None,
        next_expected_msg_seq_num: None,
        max_message_size: None,
        test_message_indicator: None,
        username: None,
        password: None,
        default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
        msg_type_grp: Some(vec![
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                msg_direction: Some(MsgDirection::Send),
                ref_appl_ver_id: None,
                ref_appl_ext_id: None,
                ref_cstm_appl_ver_id: None,
                default_ver_indicator: None,
            },
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                msg_direction: Some(MsgDirection::Receive),
                ref_appl_ver_id: None,
                ref_appl_ext_id: None,
                ref_cstm_appl_ver_id: None,
                default_ver_indicator: None,
            },
        ]),
    })));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_2() {
    let msg = fixt_message(Box::new(Message::Logon(Logon {
        encrypt_method: EncryptMethod::NoneOther,
        heart_bt_int: 30,
        raw_data: None,
        reset_seq_num_flag: None,
        next_expected_msg_seq_num: None,
        max_message_size: None,
        test_message_indicator: None,
        username: None,
        password: None,
        default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
        msg_type_grp: Some(vec![
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                msg_direction: None,
                ref_appl_ver_id: None,
                ref_appl_ext_id: None,
                ref_cstm_appl_ver_id: None,
                default_ver_indicator: Some(true),
            },
            MsgTypeGrp {
                ref_msg_type: Some(MsgType::NewOrderSingle.to_fix_string()),
                msg_direction: None,
                ref_appl_ver_id: None,
                ref_appl_ext_id: None,
                ref_cstm_appl_ver_id: None,
                default_ver_indicator: Some(false),
            },
        ]),
    })));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn unknown_msg_type() {
    let msg_str = "8=FIXT.1.1|9=0077|35=UNKNOWN|49=test_sender|56=test_target|34=1|52=20230713-21:55:13.436187000|10=254|";

    assert_matches!(
        FixtMessage::from_bytes(msg_str.replace("|", "\x01").as_bytes()),
        Err(DeserializeError::Reject {
            tag: Some(35),
            reason: ParseRejectReason::InvalidMsgtype,
            ..
        })
    );
}
