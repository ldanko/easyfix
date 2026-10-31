use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::FixStr,
    deserializer::DeserializeErrorKind,
    fix_str,
};
use easyfix_test_messages::Message;
use rstest::rstest;

use super::{
    reset_peer_support::reset_test_session,
    reset_support::{
        broken_reset_response, commit_reset_admin, committed_reset_probe_id, reset_ack,
        reset_callback_action, reset_waiting_engine_with_origin, running_reset_probe,
        running_reset_sent_engine,
    },
    support::assert_msg_type,
};
use crate::{
    application::{EndContext, SessionEndReason, Termination},
    engine::{InputResult, LogonState, ResetPhase},
    io::{ControlMsg, time::TimerBackend},
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, serialize_message, take_admin},
};

#[rstest]
#[case(true, 30)]
#[case(false, 30)]
#[case(true, 20)]
#[tokio::test]
async fn running_reset_peer_logon_abandons_preparation_only_when_accepted(
    #[values(false, true)] probe: bool,
    #[case] allowed: bool,
    #[case] heartbeat: i64,
) {
    let (mut engine, mut storage) = EngineBuilder::new()
        .logged_on()
        .accept_reset_in_session(allowed)
        .build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    if probe {
        let _ = running_reset_probe(&mut engine, &mut storage);
    } else {
        engine.on_input_timeout();
        let _ = commit_reset_admin(&mut engine, &mut storage);
        engine.on_control(ControlMsg::ResetRunningSession);
        assert!(!engine.state.reset_barrier_ids.is_empty());
    }
    let sender = storage.next_sender_msg_seq_num().get();
    let history = storage
        .fetch(nz_seq(40), nz_seq(40))
        .await
        .unwrap()
        .to_vec();
    let old_probe = engine.state.reset_probe_id.clone();
    let request = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        heartbeat,
        Some(true),
        None,
    );
    accept_input(&mut engine, request, &mut storage);
    if allowed && heartbeat == 30 {
        assert_eq!(engine.state.logon_state, LogonState::Established);
        assert_eq!(engine.reset_phase(), None);
        assert!(engine.state.reset_barrier_ids.is_empty());
        assert!(engine.state.reset_probe_id.is_none());
        assert!(!engine.state.probe_stale);
        assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
        assert!(engine.state.queue.is_empty());
        assert!(engine.state.resend_range.is_none());
        assert!(!engine.has_pending_resends());
        let ack = commit_reset_admin(&mut engine, &mut storage);
        assert_matches!(as_admin(&ack), AdminBase::Logon(l)
                if l.reset_seq_num_flag == Some(true) && l.heart_bt_int == 30);
        assert_eq!(ack.header.msg_seq_num, 1);
        assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
        if let Some(id) = old_probe {
            accept_input(
                &mut engine,
                test_helpers::heartbeat(2, Some(id)),
                &mut storage,
            );
            assert_eq!(storage.next_target_msg_seq_num().get(), 3);
            assert_eq!(engine.state.logon_state, LogonState::Established);
            assert_eq!(engine.reset_phase(), None);
        }
    } else {
        assert_eq!(
            engine.session_end().map(|end| end.context),
            Some(EndContext::Closing(SessionEndReason::InvalidLogonState))
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 40);
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender);
        assert_eq!(
            storage.fetch(nz_seq(40), nz_seq(40)).await.unwrap(),
            history.as_slice()
        );
        let mut logout = take_admin(&mut engine);
        assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender + 1);
        assert_eq!(storage.next_target_msg_seq_num().get(), 40);
        assert!(storage.fetch(nz_seq(sender), nz_seq(sender)).await.is_err());
        assert_eq!(
            storage.fetch(nz_seq(40), nz_seq(40)).await.unwrap(),
            history.as_slice()
        );
        assert!(engine.commit_send(logout.clone(), &mut storage).is_ok());
        assert_eq!(logout.header.msg_seq_num, sender);
        assert_matches!(as_admin(&logout), AdminBase::Logout(l) if l.text.as_deref() == Some(if allowed {
                    fix_str!("Invalid HeartBtInt(108), expected value 30 seconds")
                } else { fix_str!("Resetting the sequence number is not supported") }));
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender + 1);
        assert_eq!(
            storage.fetch(nz_seq(40), nz_seq(40)).await.unwrap(),
            history.as_slice()
        );
        assert_eq!(
            storage.fetch(nz_seq(sender), nz_seq(sender)).await.unwrap(),
            serialize_message(&logout).as_slice()
        );
    }
    assert!(!engine.state.local_reset_unconfirmed);
    assert!(!engine.has_admin_output());
}

#[rstest]
fn repeated_running_reset_control_preserves_pending_admin_output(
    #[values(ResetPhase::Pending, ResetPhase::Probe, ResetPhase::Sent)] phase: ResetPhase,
) {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    engine.on_control(ControlMsg::ResetRunningSession);
    if phase == ResetPhase::Pending {
        engine.on_output_timeout();
    } else {
        engine.start_reset_probe();
        if phase == ResetPhase::Sent {
            let id = committed_reset_probe_id(&mut engine, &mut storage);
            accept_input(
                &mut engine,
                test_helpers::heartbeat(1, Some(id)),
                &mut storage,
            );
        }
    }
    assert_eq!(engine.admin_output.len(), 1);
    let output = format!("{:?}", engine.admin_output);
    let id = engine.state.reset_probe_id.clone();
    let barrier = engine.state.reset_barrier_ids.clone();
    let sender = storage.next_sender_msg_seq_num();
    let target = storage.next_target_msg_seq_num();
    engine.on_control(ControlMsg::ResetRunningSession);
    assert_eq!(engine.reset_phase(), Some(phase));
    assert_eq!(engine.state.reset_probe_id, id);
    assert_eq!(engine.state.reset_barrier_ids, barrier);
    assert_eq!(storage.next_sender_msg_seq_num(), sender);
    assert_eq!(storage.next_target_msg_seq_num(), target);
    assert_eq!(format!("{:?}", engine.admin_output), output);
}

#[rstest]
fn running_reset_controls_and_keep_alive_respect_every_phase(
    #[values(ResetPhase::Pending, ResetPhase::Probe, ResetPhase::Sent)] phase: ResetPhase,
) {
    let (mut engine, mut storage) = if phase == ResetPhase::Sent {
        running_reset_sent_engine()
    } else {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        if phase == ResetPhase::Probe {
            let _ = running_reset_probe(&mut engine, &mut storage);
        } else {
            engine.on_control(ControlMsg::ResetRunningSession);
        }
        (engine, storage)
    };
    let id = engine.state.reset_probe_id.clone();
    let barrier = engine.state.reset_barrier_ids.clone();
    engine.on_control(ControlMsg::ResetRunningSession);
    assert_eq!(engine.reset_phase(), Some(phase));
    assert_eq!(engine.state.reset_probe_id, id);
    assert_eq!(engine.state.reset_barrier_ids, barrier);
    assert!(!engine.accepts_app_sends());
    assert!(engine.is_logged_on());
    // The held timeout must not escalate even when grace is at its limit.
    engine.session_settings.auto_disconnect_after_no_heartbeat = 1.try_into().unwrap();
    engine
        .state
        .grace_period_test_req_ids
        .insert(fix_str!("OUTSTANDING").to_owned());
    let grace = engine.state.grace_period_test_req_ids.clone();
    engine.on_input_timeout();
    assert!(!engine.has_admin_output());
    assert!(!engine.should_disconnect());
    assert_eq!(engine.state.grace_period_test_req_ids, grace);
    let sender = storage.next_sender_msg_seq_num().get();
    engine.on_output_timeout();
    if phase == ResetPhase::Sent {
        assert!(!engine.has_admin_output());
        assert_eq!(sender, 2);
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender);
    } else {
        let heartbeat = commit_reset_admin(&mut engine, &mut storage);
        assert_msg_type(&heartbeat, MsgTypeBase::Heartbeat);
        assert_eq!(heartbeat.header.msg_seq_num, sender);
        let seq = storage.next_target_msg_seq_num().get();
        accept_input(
            &mut engine,
            test_helpers::test_request(seq, fix_str!("PEER")),
            &mut storage,
        );
        let reply = commit_reset_admin(&mut engine, &mut storage);
        assert_eq!(reply.header.msg_seq_num, sender + 1);
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender + 2);
        assert_matches!(as_admin(&reply), AdminBase::Heartbeat(h)
                if h.test_req_id.as_deref() == Some(fix_str!("PEER")));
    }
}

#[rstest]
#[case::application(test_helpers::new_order_single(501), fix_str!("Unexpected MsgType(D) during sequence number reset"))]
#[case::heartbeat(test_helpers::heartbeat(501, None), fix_str!("Unexpected Heartbeat(0) during sequence number reset"))]
#[case::test_request(test_helpers::test_request(501, fix_str!("PEER")), fix_str!("Unexpected TestRequest(1) during sequence number reset"))]
#[case::resend_request(test_helpers::resend_request(501, 1, 1), fix_str!("Unexpected ResendRequest(2) during sequence number reset"))]
#[case::sequence_reset(test_helpers::sequence_reset(501, 700, false), fix_str!("Unexpected SequenceReset(4) during sequence number reset"))]
#[case::reject(test_helpers::reject(501, 1), fix_str!("Unexpected Reject(3) during sequence number reset"))]
fn running_reset_sent_rejects_unexpected_traffic_before_sequence_recovery(
    #[values(false, true)] logout_sent: bool,
    #[case] msg: Box<Message>,
    #[case] expected: &'static FixStr,
) {
    let (mut engine, mut storage) = running_reset_sent_engine();
    if logout_sent {
        engine.send_logout(None, None);
        let _ = commit_reset_admin(&mut engine, &mut storage);
    }
    assert_matches!(
        engine
            .process_input_without_callback(msg, &mut storage)
            .unwrap(),
        InputResult::Handled
    );
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(SessionEndReason::InvalidLogonState))
    );
    assert!(engine.session_end().unwrap().reset_unconfirmed);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert!(!engine.has_pending_resends());
    assert!(engine.state.queue.is_empty());
    if !logout_sent {
        let logout = commit_reset_admin(&mut engine, &mut storage);
        assert_matches!(as_admin(&logout), AdminBase::Logout(l)
                if l.text.as_deref() == Some(expected));
    }
    assert!(!engine.has_admin_output());
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
}

#[rstest]
fn running_reset_sent_rejects_undecodable_application_traffic(
    #[values(false, true)] logout_sent: bool,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, true);
    let mut error = broken_reset_response(Some(fix_str!("D")), 501, true);
    if let DeserializeErrorKind::Reject { tag, .. } = &mut error.kind {
        *tag = Some(55);
    }
    engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(SessionEndReason::InvalidLogonState))
    );
    assert!(engine.session_end().unwrap().reset_unconfirmed);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert!(engine.state.queue.is_empty());
    assert!(engine.state.resend_range.is_none());
    assert!(!engine.has_pending_resends());
    if !logout_sent {
        let logout = commit_reset_admin(&mut engine, &mut storage);
        assert_matches!(as_admin(&logout), AdminBase::Logout(l)
                if l.text.as_deref() == Some(fix_str!("Unexpected MsgType(D) during sequence number reset")));
    }
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    assert!(!engine.has_admin_output());
}

#[rstest]
fn running_reset_control_is_ignored_when_disconnected_or_not_established(
    #[values(
        LogonState::Idle,
        LogonState::LogonSent,
        LogonState::ResetPending,
        LogonState::ResetProbe,
        LogonState::ResetSent,
        LogonState::LogoutSent {
            sent_at: Some(TimerBackend::Tokio.now()),
        },
        LogonState::LogoutAcknowledged {
            sent_at: TimerBackend::Tokio.now(),
        },
        LogonState::Established,
    )]
    state: LogonState,
) {
    let (mut engine, storage) = EngineBuilder::new().build();
    engine.state.logon_state = state;
    if state == LogonState::Established {
        engine.end_transport(Termination::PeerClose);
    }
    engine.state.reset_probe_id = Some(fix_str!("EXISTING").to_owned());
    engine
        .state
        .reset_barrier_ids
        .insert(fix_str!("BARRIER").to_owned());
    engine.state.probe_stale = true;
    let id = engine.state.reset_probe_id.clone();
    let barrier = engine.state.reset_barrier_ids.clone();
    let reason = engine.session_end().map(|end| end.context);
    engine.on_control(ControlMsg::ResetRunningSession);
    assert_eq!(engine.state.logon_state, state);
    assert_eq!(engine.state.reset_probe_id, id);
    assert_eq!(engine.state.reset_barrier_ids, barrier);
    assert!(engine.state.probe_stale);
    assert_eq!(engine.session_end().map(|end| end.context), reason);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert!(!engine.has_admin_output());
}

#[rstest]
#[tokio::test]
async fn running_reset_probe_is_cancelled_by_logout_and_old_replies_stay_old(
    #[values(0, 1, 2)] action: usize,
) {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(100)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    let id = running_reset_probe(&mut engine, &mut storage);
    let history = storage
        .fetch(nz_seq(100), nz_seq(100))
        .await
        .unwrap()
        .to_vec();
    if action == 0 {
        engine.on_control(ControlMsg::Logout {
            disconnect: false,
            session_status: None,
            text: None,
        });
    } else if action == 1 {
        let InputResult::AdminMsg(msg) = engine
            .on_input(test_helpers::heartbeat(40, None), &mut storage)
            .unwrap()
        else {
            panic!("expected heartbeat callback");
        };
        engine
            .process_admin_input(msg, reset_callback_action(1), &mut storage)
            .unwrap();
    } else {
        accept_input(&mut engine, test_helpers::logout(40), &mut storage);
    }
    assert_eq!(engine.reset_phase(), None);
    assert!(engine.state.reset_probe_id.is_none());
    let logout = commit_reset_admin(&mut engine, &mut storage);
    assert_eq!(logout.header.msg_seq_num, 101);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    let state = engine.state.logon_state;
    if action < 2 {
        let target = storage.next_target_msg_seq_num().get();
        accept_input(
            &mut engine,
            test_helpers::heartbeat(target, Some(id)),
            &mut storage,
        );
        assert_eq!(engine.state.logon_state, state);
        assert_eq!(storage.next_target_msg_seq_num().get(), target + 1);
    }
    assert_eq!(storage.next_sender_msg_seq_num().get(), 102);
    assert_eq!(
        storage.fetch(nz_seq(100), nz_seq(100)).await.unwrap(),
        history.as_slice()
    );
    assert!(!engine.has_admin_output());
    assert!(!engine.state.local_reset_unconfirmed);
}

#[tokio::test]
async fn running_reset_probe_answers_test_request_before_resetting_numbering() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(100)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    let id = running_reset_probe(&mut engine, &mut storage);
    accept_input(
        &mut engine,
        test_helpers::test_request(40, fix_str!("BEFORE-RESET")),
        &mut storage,
    );
    assert!(engine.has_admin_output());
    let reply = commit_reset_admin(&mut engine, &mut storage);
    assert_eq!(reply.header.msg_seq_num, 101);
    assert_matches!(as_admin(&reply), AdminBase::Heartbeat(h) if h.test_req_id.as_deref() == Some(fix_str!("BEFORE-RESET")));
    assert!(!engine.has_admin_output());
    accept_input(
        &mut engine,
        test_helpers::heartbeat(41, Some(id)),
        &mut storage,
    );
    let logon = commit_reset_admin(&mut engine, &mut storage);
    assert_eq!(logon.header.msg_seq_num, 1);
    assert_matches!(as_admin(&logon), AdminBase::Logon(l) if l.reset_seq_num_flag == Some(true));
    assert!(storage.fetch(nz_seq(101), nz_seq(101)).await.is_err());
}

#[rstest]
#[tokio::test]
async fn running_reset_replayed_peer_logon_preserves_preparation(
    #[values(ResetPhase::Pending, ResetPhase::Probe)] phase: ResetPhase,
) {
    let (mut engine, mut storage, _) = reset_test_session(true);
    accept_input(&mut engine, reset_ack(1, Some(true), None), &mut storage);
    let _ = commit_reset_admin(&mut engine, &mut storage);
    let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
    let _ = engine.take_pending();
    accept_input(&mut engine, test_helpers::heartbeat(2, None), &mut storage);
    engine.on_control(ControlMsg::ResetRunningSession);
    if phase == ResetPhase::Probe {
        engine.start_reset_probe();
        let _ = committed_reset_probe_id(&mut engine, &mut storage);
    }
    engine.session_settings.accept_reset_in_session = false;
    let sender = storage.next_sender_msg_seq_num();
    let probe_id = engine.state.reset_probe_id.clone();
    let barrier = engine.state.reset_barrier_ids.clone();
    let stale = engine.state.probe_stale;
    let mut replay = reset_ack(1, Some(true), None);
    replay.header.poss_dup_flag = Some(true);
    replay.header.orig_sending_time = Some(replay.header.sending_time);
    assert_matches!(
        engine.on_input(replay, &mut storage).unwrap(),
        InputResult::Handled
    );
    assert_eq!(storage.next_target_msg_seq_num().get(), 3);
    assert_eq!(storage.next_sender_msg_seq_num(), sender);
    assert_eq!(
        storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap(),
        history.as_slice()
    );
    assert_eq!(engine.reset_phase(), Some(phase));
    assert_eq!(engine.state.reset_probe_id, probe_id);
    assert_eq!(engine.state.reset_barrier_ids, barrier);
    assert_eq!(engine.state.probe_stale, stale);
    assert!(!engine.has_admin_output());
    assert!(!engine.should_disconnect());
}
