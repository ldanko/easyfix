//! Tests for `SessionMessage` trait implementation on `Message`.

use std::borrow::Cow;

use assert_matches::assert_matches;
use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, HeaderBase, HeartbeatBase, LogonBase, MsgTypeBase,
    },
    basic_types::UtcTimestamp,
    fix_str,
    message::{MsgCat, SessionMessage},
};
use easyfix_messages::{
    fields::ApplVerId,
    messages::{Body, Header, Heartbeat, Message, Trailer},
};

fn make_heartbeat() -> Message {
    Message {
        header: Header {
            begin_string: fix_str!("FIXT.1.1").to_owned(),
            sender_comp_id: fix_str!("SENDER").to_owned(),
            target_comp_id: fix_str!("TARGET").to_owned(),
            msg_seq_num: 42,
            sending_time: UtcTimestamp::now(),
            poss_dup_flag: None,
            orig_sending_time: None,
            appl_ver_id: Some(ApplVerId::from_bytes(b"9").unwrap()),
            ..Default::default()
        },
        body: Box::new(Body::Heartbeat(Heartbeat { test_req_id: None })),
        trailer: Trailer::default(),
    }
}

// ---------------------------------------------------------------------------
// header()
// ---------------------------------------------------------------------------

#[test]
fn header_returns_borrowed_header_base() {
    let msg = make_heartbeat();
    let base = msg.header();

    assert_eq!(base.begin_string.as_ref(), fix_str!("FIXT.1.1"));
    assert_eq!(base.sender_comp_id.as_ref(), fix_str!("SENDER"));
    assert_eq!(base.target_comp_id.as_ref(), fix_str!("TARGET"));
    assert_eq!(base.msg_seq_num, 42);
    assert_eq!(base.appl_ver_id.as_deref(), Some(fix_str!("9")));
}

// ---------------------------------------------------------------------------
// as_admin()
// ---------------------------------------------------------------------------

#[test]
fn as_admin_returns_some_for_admin_message() {
    let msg = make_heartbeat();
    assert_matches!(msg.try_as_admin(), Some(AdminBase::Heartbeat(_)));
}

#[test]
fn as_admin_returns_none_for_app_message() {
    let msg = Message {
        header: Header {
            begin_string: fix_str!("FIXT.1.1").to_owned(),
            sender_comp_id: fix_str!("S").to_owned(),
            target_comp_id: fix_str!("T").to_owned(),
            msg_seq_num: 1,
            sending_time: UtcTimestamp::now(),
            ..Default::default()
        },
        body: Box::new(Body::NewOrderSingle(Default::default())),
        trailer: Trailer::default(),
    };
    assert!(msg.try_as_admin().is_none());
}

// ---------------------------------------------------------------------------
// msg_type() and msg_cat()
// ---------------------------------------------------------------------------

#[test]
fn msg_type_returns_msg_type_field() {
    let msg = make_heartbeat();
    assert_eq!(SessionMessage::msg_type(&msg), MsgTypeBase::Heartbeat);
}

#[test]
fn msg_cat_returns_admin_for_admin_message() {
    let msg = make_heartbeat();
    assert_eq!(SessionMessage::msg_cat(&msg), MsgCat::Admin);
}

// ---------------------------------------------------------------------------
// from_admin()
// ---------------------------------------------------------------------------

#[test]
fn from_admin_constructs_heartbeat() {
    let header = HeaderBase {
        begin_string: Cow::Owned(fix_str!("FIXT.1.1").to_owned()),
        sender_comp_id: Cow::Owned(fix_str!("SENDER").to_owned()),
        target_comp_id: Cow::Owned(fix_str!("TARGET").to_owned()),
        msg_seq_num: 10,
        sending_time: UtcTimestamp::now(),
        poss_dup_flag: None,
        orig_sending_time: None,
        appl_ver_id: None,
    };
    let admin = AdminBase::Heartbeat(HeartbeatBase {
        test_req_id: Some(Cow::Owned(fix_str!("REQ1").to_owned())),
    });

    let msg = Message::from_admin(header, admin);

    assert_eq!(msg.header.msg_seq_num, 10);
    assert_eq!(msg.header.sender_comp_id.as_bytes(), b"SENDER");
    assert_matches!(*msg.body, Body::Heartbeat(_));
}

#[test]
fn from_admin_constructs_logon() {
    let header = HeaderBase {
        begin_string: Cow::Owned(fix_str!("FIXT.1.1").to_owned()),
        sender_comp_id: Cow::Owned(fix_str!("S").to_owned()),
        target_comp_id: Cow::Owned(fix_str!("T").to_owned()),
        msg_seq_num: 1,
        sending_time: UtcTimestamp::now(),
        poss_dup_flag: None,
        orig_sending_time: None,
        appl_ver_id: None,
    };
    let admin = AdminBase::Logon(LogonBase {
        encrypt_method: EncryptMethodBase::None,
        encrypt_method_raw: 0,
        heart_bt_int: 30,
        reset_seq_num_flag: None,
        next_expected_msg_seq_num: None,
        default_appl_ver_id: None,
        session_status: None,
    });

    let msg = Message::from_admin(header, admin);

    assert_matches!(*msg.body, Body::Logon(ref logon) => {
        assert_eq!(logon.heart_bt_int, 30);
    });
}

// ---------------------------------------------------------------------------
// serialize / from_raw_message round-trip
// ---------------------------------------------------------------------------

#[test]
fn serialize_from_raw_message_round_trip() {
    let original = make_heartbeat();
    let bytes = SessionMessage::serialize(&original);

    let (_, raw) = easyfix_core::deserializer::raw_message(&bytes).unwrap();
    let restored = Message::from_raw_message(raw).unwrap();

    assert_eq!(restored.header.msg_seq_num, original.header.msg_seq_num);
    assert_eq!(
        restored.header.sender_comp_id,
        original.header.sender_comp_id
    );
    assert_matches!(*restored.body, Body::Heartbeat(_));
}
