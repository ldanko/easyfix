use easyfix_messages::{
    fields::{
        DefaultApplVerId, EncryptMethod, FixString, MsgDirection, MsgType, ToFixString, Utc,
        UtcTimestamp,
    },
    groups::MsgTypeGrp,
    messages::{FixtMessage, Header, Heartbeat, Logon, Message, Trailer, BEGIN_STRING},
};

fn header(msg_type: MsgType) -> Header {
    Header {
        begin_string: BEGIN_STRING.to_owned(),
        body_length: 0, // Serializer will overwrite this
        msg_type,
        sender_comp_id: FixString::from_ascii_lossy(b"test_sender".to_vec()),
        target_comp_id: FixString::from_ascii_lossy(b"test_target".to_vec()),
        on_behalf_of_comp_id: None,
        deliver_to_comp_id: None,
        secure_data: None,
        msg_seq_num: 1,
        sender_sub_id: None,
        sender_location_id: None,
        target_sub_id: None,
        target_location_id: None,
        on_behalf_of_sub_id: None,
        on_behalf_of_location_id: None,
        deliver_to_sub_id: None,
        deliver_to_location_id: None,
        poss_dup_flag: None,
        poss_resend: None,
        sending_time: UtcTimestamp::with_nanos(Utc::now()),
        orig_sending_time: None,
        xml_data: None,
        message_encoding: None,
        last_msg_seq_num_processed: None,
        hop_grp: None,
        appl_ver_id: None,
        cstm_appl_ver_id: None,
    }
}

fn trailer() -> Trailer {
    Trailer {
        signature: None,
        check_sum: FixString::from_ascii_lossy(b"000".to_vec()), // Serializer will overwrite this
    }
}

fn fixt_message(msg: Message) -> Box<FixtMessage> {
    Box::new(FixtMessage {
        header: header(msg.msg_type()),
        body: msg,
        trailer: trailer(),
    })
}

#[test]
fn heartbeat_ok() {
    // Simple test with simple message.
    let msg = fixt_message(Message::Heartbeat(Heartbeat { test_req_id: None }));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_no_present() {
    let msg = fixt_message(Message::Logon(Logon {
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
    }));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_1() {
    let msg = fixt_message(Message::Logon(Logon {
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
    }));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}

#[test]
fn logon_msg_type_grp_present_with_two_entries_2() {
    let msg = fixt_message(Message::Logon(Logon {
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
    }));
    let serialized = msg.serialize();
    FixtMessage::from_bytes(&serialized).expect("Deserialization failed");
}
