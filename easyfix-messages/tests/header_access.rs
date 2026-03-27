//! Tests for `HeaderAccess` trait implementation on `Message`.
//!
//! Verifies that getters delegate to `self.header.*` fields and
//! setters modify them correctly, including enum-backed fields
//! (MsgType, ApplVerID).

mod messages;

use easyfix_core::{basic_types::UtcTimestamp, fix_str, message::HeaderAccess};
use messages::{ApplVerId, Body, Header, Heartbeat, Message, Trailer};

fn make_fixt_message() -> Message {
    Message {
        header: Header {
            begin_string: fix_str!("FIXT.1.1").to_owned(),
            sender_comp_id: fix_str!("SENDER").to_owned(),
            target_comp_id: fix_str!("TARGET").to_owned(),
            msg_seq_num: 42,
            sending_time: UtcTimestamp::now(),
            poss_dup_flag: Some(true),
            orig_sending_time: Some(UtcTimestamp::now()),
            appl_ver_id: Some(ApplVerId::from_bytes(b"9").unwrap()),
            ..Default::default()
        },
        body: Box::new(Body::Heartbeat(Heartbeat { test_req_id: None })),
        trailer: Trailer::default(),
    }
}

#[test]
fn getters_return_header_fields() {
    let msg = make_fixt_message();

    assert_eq!(msg.begin_string(), fix_str!("FIXT.1.1"));
    assert_eq!(msg.sender_comp_id(), fix_str!("SENDER"));
    assert_eq!(msg.target_comp_id(), fix_str!("TARGET"));
    assert_eq!(msg.msg_seq_num(), 42);
    assert_eq!(msg.sending_time(), msg.header.sending_time);
    assert_eq!(msg.poss_dup_flag(), Some(true));
    assert_eq!(msg.orig_sending_time(), msg.header.orig_sending_time);
    assert_eq!(msg.appl_ver_id(), Some(fix_str!("9")));
}

#[test]
fn getters_return_none_for_absent_optional_fields() {
    let msg = Message {
        header: Header {
            begin_string: fix_str!("FIXT.1.1").to_owned(),
            sender_comp_id: fix_str!("S").to_owned(),
            target_comp_id: fix_str!("T").to_owned(),
            msg_seq_num: 1,
            sending_time: UtcTimestamp::now(),
            ..Default::default()
        },
        body: Box::new(Body::Heartbeat(Heartbeat { test_req_id: None })),
        trailer: Trailer::default(),
    };

    assert!(msg.poss_dup_flag().is_none());
    assert!(msg.orig_sending_time().is_none());
    assert!(msg.appl_ver_id().is_none());
}

#[test]
fn setters_modify_header_fields() {
    let mut msg = make_fixt_message();

    let new_sending_time = UtcTimestamp::now();
    let new_orig_sending_time = UtcTimestamp::now();

    msg.set_begin_string(fix_str!("FIX.4.4").to_owned());
    msg.set_sender_comp_id(fix_str!("NEW_SENDER").to_owned());
    msg.set_target_comp_id(fix_str!("NEW_TARGET").to_owned());
    msg.set_msg_seq_num(99);
    msg.set_sending_time(new_sending_time);
    msg.set_poss_dup_flag(None);
    msg.set_orig_sending_time(Some(new_orig_sending_time));
    msg.set_appl_ver_id(Some(fix_str!("7").to_owned())); // FIX50

    assert_eq!(msg.begin_string(), fix_str!("FIX.4.4"));
    assert_eq!(msg.sender_comp_id(), fix_str!("NEW_SENDER"));
    assert_eq!(msg.target_comp_id(), fix_str!("NEW_TARGET"));
    assert_eq!(msg.msg_seq_num(), 99);
    assert_eq!(msg.sending_time(), new_sending_time);
    assert!(msg.poss_dup_flag().is_none());
    assert_eq!(msg.orig_sending_time(), Some(new_orig_sending_time));
    assert_eq!(msg.appl_ver_id(), Some(fix_str!("7")));
}

#[test]
fn set_appl_ver_id_to_none() {
    let mut msg = make_fixt_message();
    assert!(msg.appl_ver_id().is_some());

    msg.set_appl_ver_id(None);
    assert!(msg.appl_ver_id().is_none());
}

#[test]
fn set_poss_dup_flag_toggle() {
    let mut msg = make_fixt_message();
    assert_eq!(msg.poss_dup_flag(), Some(true));

    msg.set_poss_dup_flag(Some(false));
    assert_eq!(msg.poss_dup_flag(), Some(false));

    msg.set_poss_dup_flag(None);
    assert!(msg.poss_dup_flag().is_none());
}

#[test]
fn setters_update_underlying_header_struct() {
    let mut msg = make_fixt_message();

    msg.set_msg_seq_num(77);
    // Verify via direct struct access that the setter actually mutated the header
    assert_eq!(msg.header.msg_seq_num, 77);
}
