use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    basic_types::SeqNum,
    fix_str,
};
use easyfix_test_messages::Message;

use super::{
    reset_peer_support::{assert_reset_refused, reset_test_session},
    reset_support::committed_reset_probe_id,
    support::assert_msg_type,
};
use crate::{
    application::DisconnectReason,
    engine::{InputResult, LogonState, SessionEngine},
    io::ControlMsg,
    messages_storage::{InMemoryStorage, MessagesStorage},
    test_helpers,
    test_helpers::{accept_input, as_admin, nz_seq, take_admin},
};

#[tokio::test]
async fn peer_reset_with_invalid_tag_789_preserves_storage() {
    for state in [
        LogonState::Idle,
        LogonState::Established,
        LogonState::ResetPending,
        LogonState::ResetProbe,
    ] {
        for next_expected in [0, 2, 40] {
            let (mut engine, mut storage, history) = peer_reset_test_session(state);
            assert!(!engine.has_admin_output());
            engine.session_settings.enable_next_expected_msg_seq_num = true;
            let msg = test_helpers::logon_with_options(
                1,
                fix_str!("TARGET"),
                fix_str!("SENDER"),
                30,
                Some(true),
                Some(next_expected),
            );
            assert_matches!(
                accept_input(&mut engine, msg, &mut storage),
                InputResult::Handled
            );
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::InvalidLogonState)
            );
            assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
            assert_eq!(storage.next_target_msg_seq_num().get(), 40);
            assert_eq!(
                storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                history.as_slice()
            );
            assert!(engine.pending_resends.is_empty());
            assert!(!engine.state.local_reset_unconfirmed);
            let mut logout = take_admin(&mut engine);
            assert_msg_type(&logout, MsgTypeBase::Logout);
            assert!(engine.take_admin_output().is_none());
            assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
            assert_eq!(logout.header.msg_seq_num, 40);
            assert!(engine.commit_send(logout, &mut storage).is_ok());
            assert_eq!(storage.next_sender_msg_seq_num().get(), 41);
            assert_eq!(storage.next_target_msg_seq_num().get(), 40);
            assert_eq!(
                storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                history.as_slice()
            );
        }
    }
    for state in [
        LogonState::Established,
        LogonState::ResetPending,
        LogonState::ResetProbe,
    ] {
        for heartbeat in [-1, 20] {
            let (mut engine, mut storage, history) = peer_reset_test_session(state);
            engine.session_settings.enable_next_expected_msg_seq_num = true;
            let msg = test_helpers::logon_with_options(
                1,
                fix_str!("TARGET"),
                fix_str!("SENDER"),
                heartbeat,
                Some(true),
                Some(40),
            );
            assert_matches!(
                accept_input(&mut engine, msg, &mut storage),
                InputResult::Handled
            );
            let expected_text = if heartbeat < 0 {
                let reject = take_admin(&mut engine);
                assert_matches!(as_admin(&reject), AdminBase::Reject(reject)
                if reject.ref_tag_id == Some(108)
                && reject.session_reject_reason == Some(SessionRejectReasonBase::ValueIsIncorrect.into()));
                "Invalid HeartBtInt(108)"
            } else {
                "Invalid HeartBtInt(108), expected value 30 seconds"
            };
            assert_reset_refused(&mut engine, &mut storage, &history, expected_text).await;
        }
    }
}

#[tokio::test]
async fn peer_reset_tag_789_validation_preserves_accepted_forms() {
    for state in [
        LogonState::Idle,
        LogonState::Established,
        LogonState::ResetPending,
        LogonState::ResetProbe,
    ] {
        for enabled in [false, true] {
            let variants: &[Option<SeqNum>] = if enabled {
                &[None, Some(1)]
            } else {
                &[None, Some(0), Some(2), Some(40)]
            };
            for &next_expected in variants {
                let (mut engine, mut storage, _) = peer_reset_test_session(state);
                engine.session_settings.enable_next_expected_msg_seq_num = enabled;
                let msg = test_helpers::logon_with_options(
                    1,
                    fix_str!("TARGET"),
                    fix_str!("SENDER"),
                    30,
                    Some(true),
                    next_expected,
                );
                assert_matches!(
                    accept_input(&mut engine, msg, &mut storage),
                    InputResult::Handled
                );
                assert!(engine.disconnect_reason().is_none());
                assert!(engine.is_logged_on());
                assert_eq!(engine.state.logon_state, LogonState::Established);
                assert_eq!(engine.reset_phase(), None);
                assert!(engine.state.reset_probe_id.is_none());
                assert!(engine.state.reset_barrier_ids.is_empty());
                assert!(!engine.state.probe_stale);
                assert!(!engine.state.local_reset_unconfirmed);
                assert!(engine.pending_resends.is_empty());
                assert!(storage.fetch(nz_seq(1), nz_seq(1)).await.is_err());
                let mut ack = take_admin(&mut engine);
                assert_matches!(as_admin(&ack), AdminBase::Logon(logon)
                    if logon.reset_seq_num_flag == Some(true)
                    && logon.next_expected_msg_seq_num == if enabled && next_expected.is_some() { Some(2) } else { None });
                assert!(engine.fill_header(&mut ack, &mut storage).unwrap());
                assert_eq!(ack.header.msg_seq_num, 1);
                assert!(engine.commit_send(ack, &mut storage).is_ok());
                assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                assert!(engine.take_admin_output().is_none());
            }
        }
    }
}

/// A reset Logon replay consumes only its original sequence number; it
/// cannot create another session reset. Duplicates are ignored per FIX
/// Session Layer Test Cases Scenario 2(e).
#[tokio::test]
async fn retransmitted_reset_logon_only_processes_sequence_numbers() {
    // First receive a real reset, then exchange messages under new numbering.
    let (mut engine, mut storage, _) = reset_test_session(true);
    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    accept_input(&mut engine, msg.clone(), &mut storage);
    let mut ack = take_admin(&mut engine);
    assert!(engine.fill_header(&mut ack, &mut storage).unwrap());
    assert!(engine.commit_send(ack, &mut storage).is_ok());
    let _ = engine.take_pending();
    let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
    let _ = engine.take_pending();
    accept_input(&mut engine, test_helpers::heartbeat(2, None), &mut storage);
    engine.session_settings.accept_reset_in_session = false;
    let mut replay = msg;
    replay.header.poss_dup_flag = Some(true);
    replay.header.orig_sending_time = Some(replay.header.sending_time);
    assert_matches!(
        engine.on_input(replay, &mut storage).unwrap(),
        InputResult::Handled
    );
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    assert_eq!(storage.next_target_msg_seq_num().get(), 3);
    assert_eq!(
        storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap(),
        history.as_slice()
    );
    assert!(engine.take_admin_output().is_none());
    assert!(engine.disconnect_reason().is_none());

    // A replay at the expected number on an unopened session only consumes it.
    let (mut engine, mut storage, history) = reset_test_session(false);
    storage.set_next_target_msg_seq_num(nz_seq(1)).unwrap();
    engine.session_settings.accept_reset_on_connect = false;
    let mut replay = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        20,
        Some(true),
        None,
    );
    replay.header.poss_dup_flag = Some(true);
    replay.header.orig_sending_time = Some(replay.header.sending_time);
    assert_matches!(
        engine.on_input(replay, &mut storage).unwrap(),
        InputResult::Handled
    );
    assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        history.as_slice()
    );
    assert_matches!(engine.state.logon_state, LogonState::Idle);
    assert!(engine.take_admin_output().is_none());
    assert!(engine.disconnect_reason().is_none());

    // A replay behind a gap is queued, then drained without executing its body.
    let (mut engine, mut storage, history) = reset_test_session(true);
    storage.set_next_target_msg_seq_num(nz_seq(10)).unwrap();
    engine.session_settings.accept_reset_in_session = false;
    let mut replay = test_helpers::logon_with_options(
        12,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        20,
        Some(true),
        None,
    );
    replay.header.poss_dup_flag = Some(true);
    replay.header.orig_sending_time = Some(replay.header.sending_time);
    assert_matches!(
        engine.on_input(replay, &mut storage).unwrap(),
        InputResult::Handled
    );
    let resend = take_admin(&mut engine);
    assert_matches!(as_admin(&resend), AdminBase::ResendRequest(rr)
        if rr.begin_seq_no == 10 && rr.end_seq_no == 11);
    for seq in [10, 11] {
        accept_input(
            &mut engine,
            test_helpers::heartbeat(seq, None),
            &mut storage,
        );
    }
    assert_matches!(
        engine.next_queued_message(&mut storage).unwrap(),
        Some(InputResult::Handled)
    );
    assert_eq!(storage.next_target_msg_seq_num().get(), 13);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        history.as_slice()
    );
    assert!(engine.state.queue.is_empty());
    assert!(engine.take_admin_output().is_none());
    assert!(engine.disconnect_reason().is_none());
}

fn peer_reset_test_session(
    state: LogonState,
) -> (SessionEngine<Message>, InMemoryStorage, Vec<u8>) {
    let (mut engine, mut storage, history) = reset_test_session(state != LogonState::Idle);
    if matches!(state, LogonState::ResetPending | LogonState::ResetProbe) {
        engine.on_control(ControlMsg::ResetRunningSession);
        if state == LogonState::ResetProbe {
            storage.set_next_sender_msg_seq_num(nz_seq(39)).unwrap();
            engine.start_reset_probe();
            let _ = committed_reset_probe_id(&mut engine, &mut storage);
        }
    }
    assert_eq!(engine.state.logon_state, state);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
    (engine, storage, history)
}
