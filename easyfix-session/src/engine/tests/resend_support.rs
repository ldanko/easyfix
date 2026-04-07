use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::SeqNum,
};
use easyfix_test_messages::Message;

use super::support::assert_msg_type;
use crate::{
    application::InputAction,
    engine::{InputResult, SessionEngine},
    messages_storage::MessagesStorage,
    test_helpers::{as_admin, take_admin},
};

// --- ResendRequest suppression across gaps ---

/// Drain one queued out-of-order message, feeding `Accept` back for
/// dispatched admin messages (mirrors the io loop's queued-message drain).
pub(super) fn drain_queued(
    engine: &mut SessionEngine<Message>,
    storage: &mut impl MessagesStorage,
) {
    match engine.next_queued_message(storage).unwrap() {
        Some(InputResult::AdminMsg(msg)) => {
            engine
                .process_admin_input(msg, InputAction::Accept, storage)
                .unwrap();
        }
        Some(InputResult::Handled) => {}
        other => panic!("unexpected queued result: {other:?}"),
    }
}

/// Take one admin message and assert it is a ResendRequest for exactly
/// `begin..=end`.
pub(super) fn assert_resend_request(
    engine: &mut SessionEngine<Message>,
    begin: SeqNum,
    end: SeqNum,
) {
    let rr_msg = take_admin(engine);
    assert_msg_type(&rr_msg, MsgTypeBase::ResendRequest);
    let AdminBase::ResendRequest(ref rr) = as_admin(&rr_msg) else {
        panic!("expected ResendRequest");
    };
    assert_eq!(rr.begin_seq_no, begin);
    assert_eq!(rr.end_seq_no, end);
}
