use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    fix_str,
    message::HeaderAccess,
};

use super::support::assert_msg_type;
use crate::{
    application::DisconnectReason,
    engine::InputResult,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, drain_all_admin, nz_seq, take_admin},
};

/// Session Layer Section 4.4.2 resets the session over an active connection: the
/// peer sets its counters to 1 and sends `Logon(141=Y, 34=1)`; whoever
/// receives it sets NextNumIn to 2 and NextNumOut to 1 and answers with its
/// own `Logon(141=Y, 34=1)`. The answering side is "the peer receiving the
/// Logon", not the connection acceptor - the counterparties agree between
/// themselves which one initiates the daily reset.
#[test]
fn on_logon_mid_session_reset_is_acknowledged() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );

    let mut ack = take_admin(&mut engine);
    assert_msg_type(&ack, MsgTypeBase::Logon);
    {
        let AdminBase::Logon(ref logon) = as_admin(&ack) else {
            panic!("expected Logon");
        };
        assert_eq!(logon.reset_seq_num_flag, Some(true));
    }
    engine.fill_header(&mut ack, &mut storage).unwrap();
    assert_eq!(ack.msg_seq_num(), 1);

    // "Upon completion of the session reset, both peers must have
    // NextNumIn = 2 and NextNumOut = 2."
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// A reset starts "a new set of sequence numbers" (Session Layer Section 4.4.2),
/// so a resend range queued under the old numbering must go with it: the
/// store it named is cleared, and draining it would put gap-fills stamped
/// with discarded numbers in front of a peer that just moved to
/// `NextNumIn=2`.
///
/// Pins the engine's own contract. The IO loop cannot produce this state
/// today - it starts the replay before reading the next message - so the
/// test guards the engine against a future loop that could.
#[test]
fn on_logon_reset_discards_pending_resend_ranges() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();

    let msg = test_helpers::resend_request(40, 30, 39);
    accept_input(&mut engine, msg, &mut storage);
    assert!(engine.has_pending_resends());

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );

    assert!(!engine.has_pending_resends());
    assert_eq!(engine.take_pending_resend(), None);
    let ack = take_admin(&mut engine);
    assert_msg_type(&ack, MsgTypeBase::Logon);
}

/// Same for a gap-fill still accumulating over such a range - flushing it
/// after the reset would name numbers that no longer exist. Engine contract
/// only, like [`on_logon_reset_discards_pending_resend_ranges`].
#[test]
fn on_logon_reset_discards_accumulating_gap_fill() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    engine.accumulate_resend_gap(30);
    assert!(engine.has_accumulated_resend_gap());

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );

    assert!(!engine.has_accumulated_resend_gap());
}

/// The range we asked the peer for is moot once the peer resets instead of
/// replaying it - and, left in place, it suppresses a ResendRequest for a
/// new-numbering gap that happens to fall inside it. The counter restarts
/// below the old range, so the staleness rule in `request_resend` (range
/// end already passed) never clears it on its own.
#[test]
fn on_logon_reset_discards_outstanding_resend_range() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();

    // Gap 40..=44 opened by a too-high message: we ask for it and park 45.
    engine
        .on_input(test_helpers::new_order_single(45), &mut storage)
        .unwrap();
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::ResendRequest);

    // The peer resets rather than replaying.
    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logon);

    // Under the new numbering a gap 40..=42 opens - fully inside the range
    // asked for before the reset. It must be requested afresh.
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    engine
        .on_input(test_helpers::new_order_single(43), &mut storage)
        .unwrap();
    let request = engine
        .take_admin_output()
        .expect("ResendRequest for a gap opened after the reset");
    assert_msg_type(&request, MsgTypeBase::ResendRequest);
    let AdminBase::ResendRequest(ref resend) = as_admin(&request) else {
        panic!("expected ResendRequest");
    };
    assert_eq!(resend.begin_seq_no, 40);
    assert_eq!(resend.end_seq_no, 42);
}

/// Messages parked behind a gap belong to the numbering the reset threw
/// away. Kept, an entry under the old `34=42` would be handed to the
/// application as the new session's 42 - or collide with the real one.
/// The peer was supposed to close its gaps before resetting (Section 4.4.2), so
/// the cut is logged, but the jump is honoured.
#[test]
fn on_logon_reset_discards_parked_messages() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();

    engine
        .on_input(test_helpers::new_order_single(42), &mut storage)
        .unwrap();
    drain_all_admin(&mut engine);
    assert!(engine.has_queued_message(42));

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );

    assert_eq!(engine.queued_count(), 0);
    assert!(!engine.has_queued_message(42));

    // Reaching 42 under the new numbering finds nothing to re-dispatch.
    storage.set_next_target_msg_seq_num(nz_seq(42)).unwrap();
    assert!(engine.next_queued_message(&mut storage).unwrap().is_none());
}

/// A second Logon without ResetSeqNumFlag=Y fails the state gate before
/// its sequence number is judged against the persisted counter.
#[test]
fn on_logon_second_logon_without_reset_flag_fails_state_before_sequence() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();

    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.take_admin_output().is_none(), "silent disconnect");
    assert_eq!(storage.next_target_msg_seq_num().get(), 40);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
}

/// A peer reset must carry a valid tag 789 when enabled; zero names no
/// message (Session Layer Test Cases Scenario 1S(d)).
#[test]
fn on_logon_peer_reset_keeps_tag_789_checkable() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .enable_next_expected_msg_seq_num()
        .build();
    storage.set_next_sender_msg_seq_num(nz_seq(10)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(10)).unwrap();

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        Some(0),
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
}
