use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    fix_str,
};
use easyfix_test_messages::Message;

use super::{
    reset_peer_support::reset_test_session,
    reset_support::{reset_ack, reset_waiting_engine},
    support::assert_msg_type,
};
use crate::{
    application::{DisconnectReason, InputAction},
    engine::{InputResult, LogonState, supports_seq_num_reset},
    initiator::SessionStart,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{
        CountedResetMessage, EngineBuilder, FailingStorage, FailureTiming, StorageOp, accept_input,
        as_admin, nz_seq, take_admin,
    },
};

/// Capability detection must inspect the converted Logon.
#[test]
fn reset_support_checks_the_converted_logon() {
    let settings = test_helpers::default_session_settings();
    assert!(!supports_seq_num_reset::<CountedResetMessage>(&settings));
    assert!(supports_seq_num_reset::<Message>(&settings));
}

#[test]
fn failed_initial_reset_preserves_engine_state_and_emits_no_logon() {
    for timing in [FailureTiming::Before, FailureTiming::After] {
        let (mut engine, _) = EngineBuilder::new().build();
        let mut storage = FailingStorage::new();
        storage.sender = nz_seq(40);
        storage.target = nz_seq(40);
        engine.pending_resends.push_back(2..=3);
        storage.fail_on(StorageOp::Reset, 1, timing);
        assert!(
            engine
                .send_logon_request(&mut storage, SessionStart::Reset)
                .is_err()
        );
        assert_eq!(engine.state.logon_state, LogonState::Idle);
        assert!(!engine.state.local_reset_unconfirmed);
        assert!(engine.has_pending_resends());
        assert!(!engine.has_admin_output());
        assert!(engine.has_fatal_error());
        assert_eq!(
            engine.disconnect_reason(),
            Some(DisconnectReason::StorageError)
        );
    }
}

#[test]
fn failed_peer_reset_emits_no_ack_and_preserves_recovery() {
    for established in [false, true] {
        for timing in [FailureTiming::Before, FailureTiming::After] {
            let (mut engine, _) = EngineBuilder::new().build();
            if established {
                engine.set_logged_on();
            }
            engine.session_settings_mut().accept_reset_on_connect = true;
            engine.session_settings_mut().accept_reset_in_session = true;
            let previous_state = engine.state.logon_state;
            let mut storage = FailingStorage::new();
            storage.sender = nz_seq(40);
            storage.target = nz_seq(40);
            engine.pending_resends.push_back(2..=3);
            storage.fail_on(StorageOp::Reset, 1, timing);
            let msg = test_helpers::logon_with_options(
                1,
                fix_str!("TARGET"),
                fix_str!("SENDER"),
                30,
                Some(true),
                None,
            );
            let InputResult::AdminMsg(msg) = engine.on_input(msg, &mut storage).unwrap() else {
                panic!("expected reset Logon")
            };
            assert!(
                engine
                    .process_admin_input(msg, InputAction::Accept, &mut storage)
                    .is_err()
            );
            assert_eq!(engine.state.logon_state, previous_state);
            assert!(engine.has_pending_resends());
            assert!(!engine.has_admin_output());
            assert!(engine.has_fatal_error());
        }
    }
}

#[test]
fn failed_probe_reset_emits_no_reset_logon() {
    for timing in [FailureTiming::Before, FailureTiming::After] {
        let (mut engine, _) = EngineBuilder::new().logged_on().build();
        engine.state.logon_state = LogonState::ResetProbe;
        engine.state.reset_probe_id = Some(fix_str!("RESET-1").to_owned());
        let mut storage = FailingStorage::new();
        storage.fail_on(StorageOp::Reset, 1, timing);
        let msg = test_helpers::heartbeat(1, Some(fix_str!("RESET-1").to_owned()));
        let InputResult::AdminMsg(msg) = engine.on_input(msg, &mut storage).unwrap() else {
            panic!("expected heartbeat")
        };
        assert!(
            engine
                .process_admin_input(msg, InputAction::Accept, &mut storage)
                .is_err()
        );
        assert_eq!(engine.state.logon_state, LogonState::ResetProbe);
        assert!(!engine.state.local_reset_unconfirmed);
        assert!(!engine.has_admin_output());
        assert!(engine.has_fatal_error());
    }
}

// --- Logon ---

#[tokio::test]
async fn send_logon_request_start_selects_reset_or_stored_numbering() {
    for start in [SessionStart::Reset, SessionStart::Resume] {
        for next_expected_enabled in [false, true] {
            let (mut engine, mut storage, history) = reset_test_session(false);
            engine.session_settings.enable_next_expected_msg_seq_num = next_expected_enabled;
            engine.send_logon_request(&mut storage, start).unwrap();
            let reset = start == SessionStart::Reset;
            assert_eq!(
                storage.next_sender_msg_seq_num().get(),
                if reset { 1 } else { 40 }
            );
            assert_eq!(
                storage.next_target_msg_seq_num().get(),
                if reset { 1 } else { 40 }
            );
            if reset {
                assert!(storage.fetch(nz_seq(1), nz_seq(1)).await.is_err());
            } else {
                assert_eq!(
                    storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                    history.as_slice()
                );
            }
            assert_eq!(engine.state.local_reset_unconfirmed, reset);
            let mut request = take_admin(&mut engine);
            assert_matches!(as_admin(&request), AdminBase::Logon(logon)
                if logon.reset_seq_num_flag == reset.then_some(true)
                && logon.next_expected_msg_seq_num == next_expected_enabled.then_some(if reset { 1 } else { 40 }));
            assert!(engine.fill_header(&mut request, &mut storage).unwrap());
            assert_eq!(request.header.msg_seq_num, if reset { 1 } else { 40 });
            assert!(engine.commit_send(request, &mut storage).is_ok());
            assert_eq!(
                storage.next_sender_msg_seq_num().get(),
                if reset { 2 } else { 41 }
            );
            assert!(engine.state.queue.is_empty());
            assert!(engine.state.resend_range.is_none());
        }
    }
}

#[tokio::test]
async fn reset_ack_preserves_our_logon_and_allows_a_later_peer_reset() {
    let (mut engine, mut storage) = reset_waiting_engine(false);
    engine.session_settings.enable_next_expected_msg_seq_num = true;
    engine.state.next_expected_msg_seq_num = Some(nz_seq(1));
    let our_logon = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
    accept_input(&mut engine, reset_ack(1, Some(true), Some(2)), &mut storage);
    assert_eq!(engine.state.logon_state, LogonState::Established);
    assert!(!engine.state.local_reset_unconfirmed);
    assert!(engine.state.next_expected_msg_seq_num.is_none());
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        our_logon.as_slice()
    );
    assert!(!engine.has_admin_output());
    let _ = engine.take_pending();
    let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
    let _ = engine.take_pending();
    assert_eq!(
        storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap(),
        history.as_slice()
    );
    accept_input(&mut engine, reset_ack(1, Some(true), Some(1)), &mut storage);
    assert!(storage.fetch(nz_seq(2), nz_seq(2)).await.is_err());
    let mut ack = take_admin(&mut engine);
    assert_matches!(as_admin(&ack), AdminBase::Logon(logon) if logon.reset_seq_num_flag == Some(true));
    assert!(engine.fill_header(&mut ack, &mut storage).unwrap());
    assert_eq!(ack.header.msg_seq_num, 1);
    assert!(engine.commit_send(ack, &mut storage).is_ok());
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(!engine.state.local_reset_unconfirmed);
}

#[test]
fn reset_ack_tag_789_uses_the_current_sender_counter_when_enabled() {
    for enabled in [false, true] {
        for next_expected in [None, Some(1), Some(2), Some(3), Some(40)] {
            let (mut engine, mut storage) = reset_waiting_engine(false);
            engine.session_settings.enable_next_expected_msg_seq_num = enabled;
            accept_input(
                &mut engine,
                reset_ack(1, Some(true), next_expected),
                &mut storage,
            );
            let refused = enabled && next_expected.is_some_and(|seq| seq > 2);
            assert_eq!(
                engine.disconnect_reason(),
                refused.then_some(DisconnectReason::SeqNumResetFailed)
            );
            assert_eq!(engine.state.local_reset_unconfirmed, refused);
            assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
            assert_eq!(
                storage.next_target_msg_seq_num().get(),
                if refused { 1 } else { 2 }
            );
            if refused {
                assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
            } else {
                assert_eq!(engine.state.logon_state, LogonState::Established);
            }
            if enabled && next_expected == Some(1) {
                assert_eq!(engine.pending_resends.len(), 1);
                assert_eq!(engine.pending_resends.front(), Some(&(1..=1)));
            } else {
                assert!(engine.pending_resends.is_empty());
            }
            assert!(!engine.has_admin_output());
        }
    }
}
