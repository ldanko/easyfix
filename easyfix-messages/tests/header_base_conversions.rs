//! Round-trip tests for header base ↔ generated header conversions.
//!
//! Each test verifies that fields survive the conversion in both directions:
//! - Incoming: Generated → Base (zero-copy borrow)
//! - Outgoing: Base → Generated (owned construction)

mod messages;

use std::borrow::Cow;

use easyfix_core::{base_messages::HeaderBase, basic_types::UtcTimestamp, fix_str};
use messages::{ApplVerId, Header};

// ---------------------------------------------------------------------------
// HeaderBase ↔ Header
// ---------------------------------------------------------------------------

#[test]
fn header_incoming() {
    let header = Header {
        begin_string: fix_str!("FIXT.1.1").to_owned(),

        sender_comp_id: fix_str!("SENDER").to_owned(),
        target_comp_id: fix_str!("TARGET").to_owned(),
        msg_seq_num: 42,
        sending_time: UtcTimestamp::now(),
        poss_dup_flag: Some(true),
        orig_sending_time: Some(UtcTimestamp::now()),
        appl_ver_id: Some(ApplVerId::from_bytes(b"9").unwrap()),
        ..Default::default()
    };
    let base = HeaderBase::from(&header);

    assert_eq!(base.begin_string.as_ref(), fix_str!("FIXT.1.1"));
    assert_eq!(base.sender_comp_id.as_ref(), fix_str!("SENDER"));
    assert_eq!(base.target_comp_id.as_ref(), fix_str!("TARGET"));
    assert_eq!(base.msg_seq_num, 42);
    assert_eq!(base.sending_time, header.sending_time);
    assert_eq!(base.poss_dup_flag, Some(true));
    assert_eq!(base.orig_sending_time, header.orig_sending_time);
    assert_eq!(base.appl_ver_id.as_deref(), Some(fix_str!("9")));
}

#[test]
fn header_incoming_minimal() {
    let header = Header {
        begin_string: fix_str!("FIXT.1.1").to_owned(),

        sender_comp_id: fix_str!("S").to_owned(),
        target_comp_id: fix_str!("T").to_owned(),
        msg_seq_num: 1,
        sending_time: UtcTimestamp::now(),
        ..Default::default()
    };
    let base = HeaderBase::from(&header);

    assert_eq!(base.msg_seq_num, 1);
    assert!(base.poss_dup_flag.is_none());
    assert!(base.orig_sending_time.is_none());
    // ApplVerID is optional even in FIXT
    assert!(base.appl_ver_id.is_none());
}

#[test]
fn header_outgoing() {
    let sending_time = UtcTimestamp::now();
    let orig_sending_time = UtcTimestamp::now();
    let base = HeaderBase {
        begin_string: Cow::Owned(fix_str!("FIXT.1.1").to_owned()),
        sender_comp_id: Cow::Owned(fix_str!("SENDER").to_owned()),
        target_comp_id: Cow::Owned(fix_str!("TARGET").to_owned()),
        msg_seq_num: 10,
        sending_time,
        poss_dup_flag: Some(true),
        orig_sending_time: Some(orig_sending_time),
        appl_ver_id: Some(Cow::Owned(fix_str!("9").to_owned())),
    };
    let header = Header::from(base);

    assert_eq!(header.begin_string.as_bytes(), b"FIXT.1.1");
    assert_eq!(header.sender_comp_id.as_bytes(), b"SENDER");
    assert_eq!(header.target_comp_id.as_bytes(), b"TARGET");
    assert_eq!(header.msg_seq_num, 10);
    assert_eq!(header.sending_time, sending_time);
    assert_eq!(header.poss_dup_flag, Some(true));
    assert_eq!(header.orig_sending_time, Some(orig_sending_time));
    assert_eq!(
        header.appl_ver_id,
        Some(ApplVerId::from_bytes(b"9").unwrap())
    );
}

#[test]
fn header_outgoing_without_optional_fields() {
    let base = HeaderBase {
        begin_string: Cow::Owned(fix_str!("FIXT.1.1").to_owned()),
        sender_comp_id: Cow::Owned(fix_str!("S").to_owned()),
        target_comp_id: Cow::Owned(fix_str!("T").to_owned()),
        msg_seq_num: 1,
        sending_time: UtcTimestamp::now(),
        poss_dup_flag: None,
        orig_sending_time: None,
        appl_ver_id: None,
    };
    let header = Header::from(base);

    assert!(header.poss_dup_flag.is_none());
    assert!(header.orig_sending_time.is_none());
    assert!(header.appl_ver_id.is_none());
}

#[test]
fn header_round_trip() {
    let original = Header {
        begin_string: fix_str!("FIXT.1.1").to_owned(),

        sender_comp_id: fix_str!("SENDER").to_owned(),
        target_comp_id: fix_str!("TARGET").to_owned(),
        msg_seq_num: 42,
        sending_time: UtcTimestamp::now(),
        poss_dup_flag: Some(true),
        orig_sending_time: Some(UtcTimestamp::now()),
        appl_ver_id: Some(ApplVerId::from_bytes(b"9").unwrap()),
        ..Default::default()
    };
    let base = HeaderBase::from(&original);
    let reconstructed = Header::from(base);

    assert_eq!(original.begin_string, reconstructed.begin_string);

    assert_eq!(original.sender_comp_id, reconstructed.sender_comp_id);
    assert_eq!(original.target_comp_id, reconstructed.target_comp_id);
    assert_eq!(original.msg_seq_num, reconstructed.msg_seq_num);
    assert_eq!(original.sending_time, reconstructed.sending_time);
    assert_eq!(original.poss_dup_flag, reconstructed.poss_dup_flag);
    assert_eq!(original.orig_sending_time, reconstructed.orig_sending_time);
    assert_eq!(original.appl_ver_id, reconstructed.appl_ver_id);
}

#[test]
fn header_round_trip_minimal() {
    let original = Header {
        begin_string: fix_str!("FIXT.1.1").to_owned(),

        sender_comp_id: fix_str!("A").to_owned(),
        target_comp_id: fix_str!("B").to_owned(),
        msg_seq_num: 1,
        sending_time: UtcTimestamp::now(),
        ..Default::default()
    };
    let base = HeaderBase::from(&original);
    let reconstructed = Header::from(base);

    assert_eq!(original.begin_string, reconstructed.begin_string);

    assert_eq!(original.sender_comp_id, reconstructed.sender_comp_id);
    assert_eq!(original.target_comp_id, reconstructed.target_comp_id);
    assert_eq!(original.msg_seq_num, reconstructed.msg_seq_num);
    assert_eq!(original.sending_time, reconstructed.sending_time);
    assert!(reconstructed.poss_dup_flag.is_none());
    assert!(reconstructed.orig_sending_time.is_none());
    assert!(reconstructed.appl_ver_id.is_none());
}
