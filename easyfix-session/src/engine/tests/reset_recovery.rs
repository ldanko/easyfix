use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    deserializer::{DeserializeErrorKind, raw_message},
    fix_str,
    message::{HeaderAccess, SessionMessage},
    version::Version,
};
use easyfix_test_messages::Message;

use super::{
    resend_support::drain_queued,
    reset_support::{
        broken_reset_response, commit_reset_admin, committed_reset_probe_id, running_reset_probe,
    },
    support::assert_msg_type,
};
use crate::{
    application::InputAction,
    engine::{InputResult, LogonState, PendingOutput, ResetPhase},
    io::ControlMsg,
    messages_storage::MessagesStorage,
    session_id::SessionId,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, take_admin},
};

#[tokio::test]
async fn running_reset_probe_rechecks_recovery_opened_after_it_was_sent() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    let id = running_reset_probe(&mut engine, &mut storage);
    let first_id = id.clone();
    let mut error = broken_reset_response(None, 55, true);
    if let DeserializeErrorKind::Reject {
        msg_type, reason, ..
    } = &mut error.kind
    {
        *msg_type = Some(fix_str!("D").to_owned());
        *reason = SessionRejectReasonBase::RequiredTagMissing.into();
    }
    engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_eq!(engine.state.resend_range, Some(40..=55));
    assert!(engine.state.queue.is_empty());
    let request = commit_reset_admin(&mut engine, &mut storage);
    assert_matches!(as_admin(&request), AdminBase::ResendRequest(rr)
        if rr.begin_seq_no == 40 && rr.end_seq_no == 55);
    let sender = storage.next_sender_msg_seq_num();
    let history = storage
        .fetch(nz_seq(40), nz_seq(40))
        .await
        .unwrap()
        .to_vec();
    accept_input(
        &mut engine,
        test_helpers::heartbeat(40, Some(id)),
        &mut storage,
    );
    assert_eq!(engine.reset_phase(), Some(ResetPhase::Pending));
    assert_eq!(storage.next_sender_msg_seq_num(), sender);
    assert_eq!(
        storage.fetch(nz_seq(40), nz_seq(40)).await.unwrap(),
        history.as_slice()
    );
    assert_eq!(storage.next_target_msg_seq_num().get(), 41);
    assert!(!engine.reset_ready(true, &storage));
    assert!(!engine.state.local_reset_unconfirmed);
    assert!(engine.state.reset_probe_id.is_none());
    assert!(!engine.state.probe_stale);
    assert!(!engine.has_admin_output());
    let mut gap = test_helpers::sequence_reset(41, 56, true);
    gap.header.poss_dup_flag = Some(true);
    gap.header.orig_sending_time = Some(gap.header.sending_time);
    accept_input(&mut engine, gap, &mut storage);
    assert_eq!(engine.state.resend_range, Some(40..=55));
    assert!(engine.reset_ready(true, &storage));
    engine.start_reset_probe();
    let id = committed_reset_probe_id(&mut engine, &mut storage);
    assert_ne!(id, first_id);
    accept_input(
        &mut engine,
        test_helpers::heartbeat(56, Some(id)),
        &mut storage,
    );
    assert_eq!(engine.reset_phase(), Some(ResetPhase::Sent));
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
    assert_matches!(as_admin(&take_admin(&mut engine)), AdminBase::Logon(l) if l.reset_seq_num_flag == Some(true));
}

#[test]
fn running_reset_ready_waits_for_the_end_of_recovery_and_the_queued_input() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(10)).unwrap();
    engine.on_control(ControlMsg::ResetRunningSession);
    accept_input(&mut engine, test_helpers::heartbeat(12, None), &mut storage);
    let _ = commit_reset_admin(&mut engine, &mut storage);
    assert_eq!(engine.state.resend_range, Some(10..=11));
    assert!(!engine.reset_ready(true, &storage));
    for seq in [10, 11] {
        accept_input(
            &mut engine,
            test_helpers::heartbeat(seq, None),
            &mut storage,
        );
        assert!(!engine.reset_ready(true, &storage));
    }
    drain_queued(&mut engine, &mut storage);
    assert_eq!(storage.next_target_msg_seq_num().get(), 13);
    assert_eq!(engine.state.resend_range, Some(10..=11));
    assert!(engine.reset_ready(true, &storage));
    accept_input(
        &mut engine,
        test_helpers::resend_request(13, 1, 1),
        &mut storage,
    );
    assert!(engine.has_pending_resends());
    assert!(!engine.reset_ready(true, &storage));
}

#[test]
fn running_reset_ready_accepts_gap_fills_and_completed_decoder_recovery() {
    for broken in [false, true] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        storage.set_next_target_msg_seq_num(nz_seq(10)).unwrap();
        engine.on_control(ControlMsg::ResetRunningSession);
        if broken {
            let mut error = broken_reset_response(Some(fix_str!("D")), 12, true);
            if let DeserializeErrorKind::Reject { reason, .. } = &mut error.kind {
                *reason = SessionRejectReasonBase::RequiredTagMissing.into();
            }
            engine.on_deserialize_error(error, &mut storage).unwrap();
            assert!(engine.state.queue.is_empty());
        } else {
            accept_input(&mut engine, test_helpers::heartbeat(12, None), &mut storage);
        }
        let range = if broken { 10..=12 } else { 10..=11 };
        assert_eq!(engine.state.resend_range, Some(range.clone()));
        let request = commit_reset_admin(&mut engine, &mut storage);
        assert_matches!(as_admin(&request), AdminBase::ResendRequest(rr)
            if rr.begin_seq_no == 10 && rr.end_seq_no == *range.end());
        if broken {
            for seq in [10, 11] {
                accept_input(
                    &mut engine,
                    test_helpers::heartbeat(seq, None),
                    &mut storage,
                );
                assert!(!engine.reset_ready(true, &storage));
            }
            assert_eq!(storage.next_target_msg_seq_num().get(), 12);
            let mut replay = test_helpers::heartbeat(12, None);
            replay.header.poss_dup_flag = Some(true);
            replay.header.orig_sending_time = Some(replay.header.sending_time);
            accept_input(&mut engine, replay, &mut storage);
        } else {
            let mut gap = test_helpers::sequence_reset(10, 12, true);
            gap.header.poss_dup_flag = Some(true);
            gap.header.orig_sending_time = Some(gap.header.sending_time);
            accept_input(&mut engine, gap, &mut storage);
            assert_eq!(storage.next_target_msg_seq_num().get(), 12);
            assert!(!engine.reset_ready(true, &storage));
            drain_queued(&mut engine, &mut storage);
        }
        assert_eq!(storage.next_target_msg_seq_num().get(), 13);
        assert!(engine.state.queue.is_empty());
        assert_eq!(engine.state.resend_range, Some(range));
        assert!(engine.reset_ready(true, &storage));
    }
}

#[tokio::test]
async fn running_reset_queued_probe_requires_a_fresh_answer_after_the_queue_drains() {
    for extra in [false, true] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        storage.set_next_sender_msg_seq_num(nz_seq(100)).unwrap();
        storage.set_next_target_msg_seq_num(nz_seq(500)).unwrap();
        let first = running_reset_probe(&mut engine, &mut storage);
        let history = storage
            .fetch(nz_seq(100), nz_seq(100))
            .await
            .unwrap()
            .to_vec();
        accept_input(
            &mut engine,
            test_helpers::heartbeat(504, Some(first.clone())),
            &mut storage,
        );
        while engine.has_admin_output() {
            let _ = commit_reset_admin(&mut engine, &mut storage);
        }
        if extra {
            accept_input(
                &mut engine,
                test_helpers::new_order_single(505),
                &mut storage,
            );
            while engine.has_admin_output() {
                let _ = commit_reset_admin(&mut engine, &mut storage);
            }
        }
        accept_input(
            &mut engine,
            test_helpers::sequence_reset(500, 504, true),
            &mut storage,
        );
        let sender = storage.next_sender_msg_seq_num();
        if !extra {
            engine.mark_probe_stale();
        }
        drain_queued(&mut engine, &mut storage);
        assert_eq!(engine.reset_phase(), Some(ResetPhase::Pending));
        assert_eq!(storage.next_target_msg_seq_num().get(), 505);
        assert_eq!(storage.next_sender_msg_seq_num(), sender);
        assert_eq!(
            storage.fetch(nz_seq(100), nz_seq(100)).await.unwrap(),
            history.as_slice()
        );
        assert!(!engine.state.local_reset_unconfirmed);
        assert!(engine.state.reset_probe_id.is_none());
        assert!(!engine.state.probe_stale);
        assert!(!engine.has_admin_output());
        if extra {
            assert!(!engine.reset_ready(true, &storage));
            let Some(InputResult::AppMsg(msg)) = engine.next_queued_message(&mut storage).unwrap()
            else {
                panic!("expected queued application input");
            };
            engine
                .process_app_input(
                    msg.header.msg_seq_num,
                    SessionMessage::msg_type(&*msg),
                    InputAction::Accept,
                    &mut storage,
                )
                .unwrap();
        }
        assert!(engine.reset_ready(true, &storage));
        engine.start_reset_probe();
        let second = committed_reset_probe_id(&mut engine, &mut storage);
        assert_ne!(first, second);
        let target = storage.next_target_msg_seq_num().get();
        accept_input(
            &mut engine,
            test_helpers::heartbeat(target, Some(second)),
            &mut storage,
        );
        assert_eq!(engine.reset_phase(), Some(ResetPhase::Sent));
        assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
        assert_eq!(storage.next_target_msg_seq_num().get(), 1);
        assert!(storage.fetch(nz_seq(100), nz_seq(100)).await.is_err());
        assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logon);
    }
}

#[test]
fn simultaneous_running_resets_converge_and_ignore_the_replayed_logon_gap() {
    let (mut a, mut a_store) = EngineBuilder::new()
        .logged_on()
        .enable_next_expected_msg_seq_num()
        .build();
    let (mut b, mut b_store) = EngineBuilder::new()
        .logged_on()
        .enable_next_expected_msg_seq_num()
        .build();
    b.session_id = SessionId::new(
        Version::FIXT11,
        fix_str!("TARGET").to_owned(),
        fix_str!("SENDER").to_owned(),
    );
    for (engine, storage) in [(&mut a, &mut a_store), (&mut b, &mut b_store)] {
        storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
        storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
        engine.on_control(ControlMsg::ResetRunningSession);
        assert!(engine.reset_ready(true, storage));
        engine.start_reset_probe();
    }
    let probe_a = commit_reset_admin(&mut a, &mut a_store);
    let probe_b = commit_reset_admin(&mut b, &mut b_store);
    for (msg, sender, target) in [
        (&probe_a, fix_str!("SENDER"), fix_str!("TARGET")),
        (&probe_b, fix_str!("TARGET"), fix_str!("SENDER")),
    ] {
        assert_eq!(msg.sender_comp_id(), sender);
        assert_eq!(msg.target_comp_id(), target);
        assert_eq!(msg.header.msg_seq_num, 40);
    }
    accept_input(&mut b, probe_a, &mut b_store);
    accept_input(&mut a, probe_b, &mut a_store);
    let response_a = commit_reset_admin(&mut a, &mut a_store);
    let response_b = commit_reset_admin(&mut b, &mut b_store);
    assert_eq!(response_a.header.msg_seq_num, 41);
    assert_eq!(response_b.header.msg_seq_num, 41);
    accept_input(&mut a, response_b, &mut a_store);
    accept_input(&mut b, response_a, &mut b_store);
    let logon_a = commit_reset_admin(&mut a, &mut a_store);
    let logon_b = commit_reset_admin(&mut b, &mut b_store);
    for (msg, sender, target) in [
        (&logon_a, fix_str!("SENDER"), fix_str!("TARGET")),
        (&logon_b, fix_str!("TARGET"), fix_str!("SENDER")),
    ] {
        assert_eq!(msg.sender_comp_id(), sender);
        assert_eq!(msg.target_comp_id(), target);
        assert_eq!(msg.header.msg_seq_num, 1);
        assert_matches!(as_admin(msg), AdminBase::Logon(l) if l.reset_seq_num_flag == Some(true) && l.next_expected_msg_seq_num == Some(1));
    }
    accept_input(&mut a, logon_b, &mut a_store);
    accept_input(&mut b, logon_a, &mut b_store);
    let mut gaps = Vec::new();
    for (engine, storage) in [(&mut a, &mut a_store), (&mut b, &mut b_store)] {
        assert_eq!(engine.state.logon_state, LogonState::Established);
        assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert_eq!(
            engine.pending_resends.iter().cloned().collect::<Vec<_>>(),
            vec![1..=1]
        );
        assert!(!engine.state.local_reset_unconfirmed);
        assert!(!engine.has_admin_output());
        assert_eq!(engine.take_pending_resend(), Some(1..=1));
        engine.accumulate_resend_gap(1);
        engine.flush_resend_gap().unwrap();
        let Some(PendingOutput::Transient { len }) = engine.take_pending() else {
            panic!("expected gap fill");
        };
        let (_, raw) = raw_message(&engine.scratch()[..len]).unwrap();
        let gap = Message::from_raw_message(raw).unwrap();
        assert_eq!(gap.header.msg_seq_num, 1);
        assert_eq!(gap.header.poss_dup_flag, Some(true));
        assert_matches!(as_admin(&gap), AdminBase::SequenceReset(sr) if sr.gap_fill_flag == Some(true) && sr.new_seq_no == 2);
        gaps.push(gap);
    }
    assert_matches!(
        a.on_input(gaps.pop().unwrap(), &mut a_store).unwrap(),
        InputResult::Handled
    );
    assert_matches!(
        b.on_input(gaps.pop().unwrap(), &mut b_store).unwrap(),
        InputResult::Handled
    );
    for (engine, storage) in [(&a, &a_store), (&b, &b_store)] {
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
        assert!(!engine.should_disconnect());
        assert!(!engine.has_admin_output());
    }
}
