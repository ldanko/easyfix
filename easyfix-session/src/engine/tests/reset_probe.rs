use std::{assert_matches, num::NonZeroU64};

use easyfix_core::{base_messages::AdminBase, basic_types::SeqNum, fix_str};

use super::{
    reset_peer_support::reset_test_session,
    reset_support::{
        commit_reset_admin, committed_reset_probe_id, reset_ack, running_reset_probe,
        running_reset_sent_engine,
    },
};
use crate::{
    application::DisconnectReason,
    engine::{LogonState, PendingOutput, ResetPhase},
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, serialize_message, take_admin},
};

#[tokio::test]
async fn running_reset_probe_resets_and_commits_before_the_ack() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .logged_on()
        .enable_next_expected_msg_seq_num()
        .heartbeat_interval_in_force(NonZeroU64::new(20))
        .build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(500)).unwrap();
    let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
    let _ = engine.take_pending();
    let id = running_reset_probe(&mut engine, &mut storage);
    assert!(engine.is_logged_on());
    assert!(!engine.accepts_app_sends());
    accept_input(
        &mut engine,
        test_helpers::heartbeat(500, Some(id)),
        &mut storage,
    );
    assert_eq!(engine.reset_phase(), Some(ResetPhase::Sent));
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
    assert!(storage.fetch(nz_seq(41), nz_seq(41)).await.is_err());
    assert!(storage.fetch(nz_seq(1), nz_seq(1)).await.is_err());
    assert!(!history.is_empty());
    assert!(engine.state.local_reset_unconfirmed);
    assert!(engine.state.reset_probe_id.is_none());
    assert!(engine.state.queue.is_empty());
    assert!(engine.state.resend_range.is_none());
    assert!(!engine.has_pending_resends());
    let mut logon = take_admin(&mut engine);
    assert_eq!(logon.header.msg_seq_num, 0);
    assert_matches!(as_admin(&logon), AdminBase::Logon(l)
        if l.reset_seq_num_flag == Some(true) && l.heart_bt_int == 20 && l.next_expected_msg_seq_num == Some(1));
    assert!(engine.fill_header(&mut logon, &mut storage).unwrap());
    assert_eq!(logon.header.msg_seq_num, 1);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert!(!engine.has_admin_output());
    assert!(storage.fetch(nz_seq(1), nz_seq(1)).await.is_err());
    assert!(engine.commit_send(logon, &mut storage).is_ok());
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert_matches!(engine.take_pending(), Some(PendingOutput::Stored(seq)) if seq.get() == 1);
    let committed = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
    let ack = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        20,
        Some(true),
        Some(2),
    );
    accept_input(&mut engine, ack, &mut storage);
    assert_eq!(engine.reset_phase(), None);
    assert_eq!(engine.state.logon_state, LogonState::Established);
    assert!(engine.accepts_app_sends());
    assert!(!engine.state.local_reset_unconfirmed);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        committed.as_slice()
    );
    assert!(!engine.has_admin_output());
}

#[tokio::test]
async fn running_reset_requires_matching_probe_ids_and_reprobes_after_stale_work() {
    for verify in [false, true] {
        let (mut engine, mut storage) = EngineBuilder::new()
            .logged_on()
            .verify_test_request_id(verify)
            .build();
        let first = running_reset_probe(&mut engine, &mut storage);
        let history = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
        for (seq, id) in [(1, None), (2, Some(fix_str!("WRONG")))] {
            accept_input(
                &mut engine,
                test_helpers::heartbeat(seq, id.map(ToOwned::to_owned)),
                &mut storage,
            );
            assert_eq!(engine.reset_phase(), Some(ResetPhase::Probe));
            assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
            assert_eq!(storage.next_target_msg_seq_num().get(), seq + 1);
            assert!(!engine.has_admin_output());
        }
        engine.mark_probe_stale();
        accept_input(
            &mut engine,
            test_helpers::heartbeat(3, Some(first.clone())),
            &mut storage,
        );
        assert_eq!(engine.reset_phase(), Some(ResetPhase::Pending));
        assert_eq!(storage.next_target_msg_seq_num().get(), 4);
        assert!(!engine.state.local_reset_unconfirmed);
        assert!(engine.state.reset_probe_id.is_none());
        assert!(!engine.state.probe_stale);
        assert!(!engine.has_admin_output());
        assert!(engine.reset_ready(true, &storage));
        assert_eq!(
            storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
            history.as_slice()
        );
        engine.start_reset_probe();
        let second = committed_reset_probe_id(&mut engine, &mut storage);
        assert_ne!(first, second);
        accept_input(
            &mut engine,
            test_helpers::heartbeat(4, Some(second)),
            &mut storage,
        );
        assert_eq!(engine.reset_phase(), Some(ResetPhase::Sent));
    }
}

#[test]
fn running_reset_barrier_preserves_keep_alive_ids_independently_of_verification() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .logged_on()
        .verify_test_request_id(false)
        .build();
    engine.on_input_timeout();
    let _ = commit_reset_admin(&mut engine, &mut storage);
    let ids = engine.state.grace_period_test_req_ids.clone();
    let id = ids.iter().next().unwrap().clone();
    engine.on_control(ControlMsg::ResetRunningSession);
    assert_eq!(engine.state.reset_barrier_ids, ids);
    assert!(!engine.has_admin_output());
    assert!(!engine.reset_ready(true, &storage));
    for (seq, answer) in [(1, None), (2, Some(fix_str!("WRONG")))] {
        accept_input(
            &mut engine,
            test_helpers::heartbeat(seq, answer.map(ToOwned::to_owned)),
            &mut storage,
        );
        assert_eq!(engine.state.reset_barrier_ids, ids);
        assert!(engine.state.grace_period_test_req_ids.is_empty());
        assert!(!engine.reset_ready(true, &storage));
    }
    accept_input(
        &mut engine,
        test_helpers::heartbeat(3, Some(id)),
        &mut storage,
    );
    assert!(engine.state.reset_barrier_ids.is_empty());
    assert!(!engine.reset_ready(false, &storage));
    assert!(engine.reset_ready(true, &storage));
}

#[tokio::test]
async fn running_reset_probe_answer_at_the_ceiling_does_not_reset() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let id = running_reset_probe(&mut engine, &mut storage);
    let history = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();
    accept_input(
        &mut engine,
        test_helpers::heartbeat(SeqNum::MAX - 1, Some(id)),
        &mut storage,
    );
    engine.end_session_if_target_numbering_exhausted(&storage);
    assert_eq!(storage.next_target_msg_seq_num().get(), SeqNum::MAX);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
    assert!(!engine.state.local_reset_unconfirmed);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        history.as_slice()
    );
    assert_eq!(engine.reset_phase(), Some(ResetPhase::Probe));
    assert_matches!(as_admin(&take_admin(&mut engine)), AdminBase::Logout(l)
        if l.text.as_deref() == Some(fix_str!("Incoming sequence numbers exhausted")));
    assert!(!engine.has_admin_output());
}

#[test]
fn running_reset_logout_holds_sends_until_the_ack_and_keeps_its_deadline() {
    let (mut engine, mut storage) = running_reset_sent_engine();
    engine.on_control(ControlMsg::Logout {
        session_status: None,
        text: None,
    });
    let _ = commit_reset_admin(&mut engine, &mut storage);
    let deadline = engine.logout_deadline();
    assert!(deadline.is_some());
    assert!(!engine.accepts_app_sends());
    engine.on_input_timeout();
    engine.on_output_timeout();
    assert!(!engine.has_admin_output());
    accept_input(&mut engine, reset_ack(1, Some(true), None), &mut storage);
    assert!(engine.accepts_app_sends());
    assert!(!engine.state.local_reset_unconfirmed);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert_eq!(engine.logout_deadline(), deadline);
    assert!(!engine.has_admin_output());
    accept_input(&mut engine, test_helpers::logout(2), &mut storage);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    assert_eq!(storage.next_target_msg_seq_num().get(), 3);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::LocalRequestedLogout)
    );
    assert!(!engine.has_admin_output());
}

#[tokio::test]
async fn running_reset_timeout_distinguishes_preparation_and_confirmation() {
    for phase in [ResetPhase::Pending, ResetPhase::Probe, ResetPhase::Sent] {
        let (mut engine, mut storage) = if phase == ResetPhase::Sent {
            running_reset_sent_engine()
        } else {
            let (mut engine, mut storage, _) = reset_test_session(true);
            if phase == ResetPhase::Probe {
                let _ = running_reset_probe(&mut engine, &mut storage);
            } else {
                engine.on_control(ControlMsg::ResetRunningSession);
            }
            (engine, storage)
        };
        let sender = storage.next_sender_msg_seq_num().get();
        let target = storage.next_target_msg_seq_num().get();
        let history = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
        if phase == ResetPhase::Sent {
            assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
        }
        engine.on_reset_timeout();
        assert_eq!(
            engine.disconnect_reason(),
            Some(if phase == ResetPhase::Sent {
                DisconnectReason::SeqNumResetFailed
            } else {
                DisconnectReason::ResetPreparationTimeout
            })
        );
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender);
        assert_eq!(storage.next_target_msg_seq_num().get(), target);
        assert_eq!(
            storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
            history.as_slice()
        );
        let mut logout = take_admin(&mut engine);
        assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender + 1);
        assert_eq!(storage.next_target_msg_seq_num().get(), target);
        assert!(storage.fetch(nz_seq(sender), nz_seq(sender)).await.is_err());
        assert_eq!(
            storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
            history.as_slice()
        );
        assert!(engine.commit_send(logout.clone(), &mut storage).is_ok());
        assert_eq!(logout.header.msg_seq_num, sender);
        assert_matches!(as_admin(&logout), AdminBase::Logout(l)
            if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender + 1);
        assert_eq!(storage.next_target_msg_seq_num().get(), target);
        assert_eq!(
            storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
            history.as_slice()
        );
        assert_eq!(
            storage.fetch(nz_seq(sender), nz_seq(sender)).await.unwrap(),
            serialize_message(&logout).as_slice()
        );
        assert!(!engine.has_admin_output());
    }
}
