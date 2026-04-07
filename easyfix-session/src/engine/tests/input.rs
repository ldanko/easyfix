use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase, SessionStatusBase},
    fix_str,
};

use super::support::assert_msg_type;
use crate::{
    application::{DisconnectReason, InputAction},
    engine::InputResult,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{
        EngineBuilder, FailingStorage, FailureTiming, StorageOp, accept_input, as_admin,
        dispatch_app_for_action, take_admin,
    },
};

// --- Reject ---

#[test]
fn on_reject_normal() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::reject(1, 1);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(engine.take_admin_output().is_none());
}

#[test]
fn target_counter_failures_stop_admin_processing_and_future_dispatch() {
    for timing in [FailureTiming::Before, FailureTiming::After] {
        for msg in [
            test_helpers::heartbeat(1, None),
            test_helpers::sequence_reset(1, 5, true),
            test_helpers::reject(1, 1),
        ] {
            let (mut engine, _) = EngineBuilder::new().logged_on().build();
            let mut storage = FailingStorage::new();
            storage.fail_on(StorageOp::SetTarget, 1, timing);
            let InputResult::AdminMsg(msg) = engine.on_input(msg, &mut storage).unwrap() else {
                panic!("expected admin")
            };
            assert!(
                engine
                    .process_admin_input(msg, InputAction::Accept, &mut storage)
                    .is_err()
            );
            assert!(engine.has_fatal_error());
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::StorageError)
            );
            assert!(!engine.has_admin_output());
            assert!(engine.next_queued_message(&mut storage).is_err());
            engine.end_session_if_target_numbering_exhausted(&storage);
            assert!(
                engine
                    .on_input(test_helpers::heartbeat(2, None), &mut storage)
                    .is_err()
            );
        }
    }
}

#[test]
fn target_counter_failure_stops_application_action_before_reply() {
    for timing in [FailureTiming::Before, FailureTiming::After] {
        for action in [
            InputAction::Accept,
            InputAction::Disconnect,
            InputAction::Reject {
                reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
                text: None,
                tag: None,
            },
        ] {
            let (mut engine, _) = EngineBuilder::new().logged_on().build();
            let mut storage = FailingStorage::new();
            storage.fail_on(StorageOp::SetTarget, 1, timing);
            assert!(
                engine
                    .process_app_input(1, MsgTypeBase::Heartbeat.into(), action, &mut storage)
                    .is_err()
            );
            assert!(!engine.has_admin_output());
            assert!(engine.has_fatal_error());
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::StorageError)
            );
        }
    }
}

// --- on_input ---

#[test]
fn on_input_admin_heartbeat() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::heartbeat(1, None);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

#[test]
fn on_input_admin_test_request() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::test_request(1, fix_str!("REQ001"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    // Heartbeat response in admin_output
    let hb_msg = take_admin(&mut engine);
    assert_msg_type(&hb_msg, MsgTypeBase::Heartbeat);
}

#[test]
fn on_input_app_message() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::new_order_single(1);
    let result = engine.on_input(msg, &mut storage).unwrap();
    assert_matches!(result, InputResult::AppMsg(_));
    // Target seq NOT incremented yet - on_input_action(Accept) does it
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
}

#[test]
fn on_input_app_message_too_high() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // msg seq=5, target expects 1 -> too high
    let msg = test_helpers::new_order_single(5);
    let result = engine.on_input(msg, &mut storage).unwrap();
    assert_matches!(result, InputResult::Handled);
    assert_eq!(engine.queued_count(), 1);

    // ResendRequest in admin_output
    let rr_msg = take_admin(&mut engine);
    assert_msg_type(&rr_msg, MsgTypeBase::ResendRequest);
}

#[test]
fn on_input_before_logon() {
    // Not logged on, non-Logon message -> Disconnect
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let msg = test_helpers::heartbeat(1, None);
    let result = engine.on_input(msg, &mut storage).unwrap();
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    // Scenario 2S: the InvalidLogonState abort disconnects *without*
    // emitting a Logout or Reject - nothing precedes the bare disconnect.
    assert!(engine.take_admin_output().is_none());
}

// --- process_app_input ---

#[test]
fn process_app_input_accept() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::new_order_single(1);
    let (ref_seq_num, ref_msg_type) = dispatch_app_for_action(&mut engine, msg, &mut storage);

    let result = engine
        .process_app_input(ref_seq_num, ref_msg_type, InputAction::Accept, &mut storage)
        .unwrap();
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

#[test]
fn process_app_input_reject() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::new_order_single(1);
    let (ref_seq_num, ref_msg_type) = dispatch_app_for_action(&mut engine, msg, &mut storage);

    let result = engine
        .process_app_input(
            ref_seq_num,
            ref_msg_type,
            InputAction::Reject {
                reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
                text: Some(fix_str!("bad value").to_owned()),
                tag: Some(44),
            },
            &mut storage,
        )
        .unwrap();
    assert_matches!(result, InputResult::Handled);

    // Reject in admin_output
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let AdminBase::Reject(ref reject) = as_admin(&reject_msg) else {
        panic!("expected Reject");
    };
    assert_eq!(
        reject.session_reject_reason,
        Some(SessionRejectReasonBase::ValueIsIncorrect.into())
    );
    assert_eq!(reject.ref_seq_num, 1);

    // Target seq incremented
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

#[test]
fn process_app_input_logout_with_disconnect() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::new_order_single(1);
    let (ref_seq_num, ref_msg_type) = dispatch_app_for_action(&mut engine, msg, &mut storage);

    let result = engine
        .process_app_input(
            ref_seq_num,
            ref_msg_type,
            InputAction::Logout {
                session_status: None,
                text: Some(fix_str!("bad credentials").to_owned()),
                disconnect: true,
            },
            &mut storage,
        )
        .unwrap();

    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::ApplicationForcedDisconnect)
    );
    // The refused message was still received: a reconnecting peer carries on
    // from 2, and expecting 1 again would draw a resend of what the
    // application already saw.
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// `Logout { disconnect: false }` starts the logout exchange, and the peer's
/// response arrives one past the refused message - so the refused message's
/// number must be consumed here, or that response is "too high" and the
/// exchange stalls on a ResendRequest the peer never answers.
#[test]
fn process_app_input_logout_without_disconnect() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::new_order_single(1);
    let (ref_seq_num, ref_msg_type) = dispatch_app_for_action(&mut engine, msg, &mut storage);

    let result = engine
        .process_app_input(
            ref_seq_num,
            ref_msg_type,
            InputAction::Logout {
                session_status: Some(SessionStatusBase::SessionLogoutComplete.into()),
                text: None,
                disconnect: false,
            },
            &mut storage,
        )
        .unwrap();

    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    // Does NOT disconnect
    assert!(!engine.should_disconnect());
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);

    // The peer's Logout response, numbered right after the refused message,
    // is in sequence and completes the exchange.
    let response = accept_input(&mut engine, test_helpers::logout(2), &mut storage);
    assert_matches!(response, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::LocalRequestedLogout)
    );
    assert!(
        engine.take_admin_output().is_none(),
        "no ResendRequest: the refused message's number was consumed"
    );
}

#[test]
fn process_app_input_disconnect() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::new_order_single(1);
    let (ref_seq_num, ref_msg_type) = dispatch_app_for_action(&mut engine, msg, &mut storage);

    let result = engine
        .process_app_input(
            ref_seq_num,
            ref_msg_type,
            InputAction::Disconnect,
            &mut storage,
        )
        .unwrap();
    assert!(engine.should_disconnect());
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::ApplicationForcedDisconnect)
    );
    assert!(engine.take_admin_output().is_none());
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}
