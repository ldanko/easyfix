use std::{assert_matches, io, num::NonZeroU64};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    basic_types::FixStr,
    deserializer::{DeserializeErrorKind, GarbledReason},
    fix_str,
};
use rstest::rstest;

use super::{
    reset_support::{
        broken_reset_response, commit_reset_admin, reset_ack, reset_callback_action,
        reset_waiting_engine, reset_waiting_engine_with_origin, running_reset_probe,
    },
    support::assert_msg_type,
};
use crate::{
    application::{
        EndContext, InputAction, IoOperation, SessionEndReason, Termination, TimeoutKind,
    },
    engine::{InputResult, LogonState},
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, take_admin},
};

#[rstest]
#[case::lower_echo(30, 20)]
#[case::higher_echo(20, 30)]
#[case::matching_echo(20, 20)]
fn running_reset_ack_uses_the_effective_heartbeat_and_current_sender_counter(
    #[values(false, true)] logout_sent: bool,
    #[case] effective: u64,
    #[case] echoed: i64,
) {
    let (mut engine, mut storage) = EngineBuilder::new()
        .logged_on()
        .heartbeat_interval_in_force(NonZeroU64::new(effective))
        .build();
    let id = running_reset_probe(&mut engine, &mut storage);
    accept_input(
        &mut engine,
        test_helpers::heartbeat(1, Some(id)),
        &mut storage,
    );
    let logon = commit_reset_admin(&mut engine, &mut storage);
    assert_matches!(as_admin(&logon), AdminBase::Logon(l) if l.heart_bt_int == i64::try_from(effective).unwrap());
    if logout_sent {
        engine.on_control(ControlMsg::Logout {
            disconnect: false,
            session_status: None,
            text: None,
        });
        let _ = commit_reset_admin(&mut engine, &mut storage);
    }
    let before = engine.state.logon_state;
    let ack = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        echoed,
        Some(true),
        None,
    );
    accept_input(&mut engine, ack, &mut storage);
    assert!(!engine.state.local_reset_unconfirmed);
    assert_eq!(engine.state.heartbeat_interval, NonZeroU64::new(effective));
    if echoed == i64::try_from(effective).unwrap() {
        assert_eq!(
            engine.state.logon_state,
            if logout_sent {
                before
            } else {
                LogonState::Established
            }
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert_eq!(
            storage.next_sender_msg_seq_num().get(),
            if logout_sent { 3 } else { 2 }
        );
    } else {
        assert_eq!(
            engine.session_end().map(|end| end.context),
            Some(EndContext::Closing(SessionEndReason::InvalidLogonState))
        );
        assert_eq!(engine.state.logon_state, before);
        assert_eq!(storage.next_target_msg_seq_num().get(), 1);
        if !logout_sent {
            let reply = commit_reset_admin(&mut engine, &mut storage);
            let expected =
                format!("HeartBtInt(108) not echoed: expected {effective}, got {echoed}");
            assert_matches!(as_admin(&reply), AdminBase::Logout(l) if l.text.as_deref() == Some(FixStr::from_ascii(expected.as_bytes()).unwrap()));
        }
        assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    }
    assert!(!engine.has_admin_output());
}

#[rstest]
fn running_reset_ack_uses_the_current_sender_counter_for_tag_789(
    #[values(false, true)] logout_sent: bool,
    #[values(false, true)] enabled: bool,
    #[values(None, Some(1), Some(2), Some(3), Some(4), Some(40))] offered: Option<u32>,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, true);
    engine.session_settings.enable_next_expected_msg_seq_num = enabled;
    let sender = storage.next_sender_msg_seq_num().get();
    let before = engine.state.logon_state;
    accept_input(&mut engine, reset_ack(1, Some(true), offered), &mut storage);
    let refused = enabled && offered.is_some_and(|n| n > sender);
    assert_eq!(
        engine.session_end().map(|end| end.context),
        refused.then_some(EndContext::Closing(SessionEndReason::InvalidLogonState))
    );
    assert_eq!(engine.state.local_reset_unconfirmed, refused);
    assert_eq!(
        engine.session_end().map(|end| end.reset_unconfirmed),
        refused.then_some(true)
    );
    assert_eq!(
        storage.next_target_msg_seq_num().get(),
        if refused { 1 } else { 2 }
    );
    if refused && !logout_sent {
        let _ = commit_reset_admin(&mut engine, &mut storage);
    }
    assert!(!engine.has_admin_output());
    let expected: Vec<_> = if enabled && !refused {
        offered
            .filter(|n| *n < sender)
            .map(|n| n..=sender - 1)
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    assert_eq!(
        engine.pending_resends.iter().cloned().collect::<Vec<_>>(),
        expected
    );
    if !refused {
        assert_eq!(
            engine.state.logon_state,
            if logout_sent {
                before
            } else {
                LogonState::Established
            }
        );
        assert_eq!(storage.next_sender_msg_seq_num().get(), sender);
    }
}

#[rstest]
#[tokio::test]
async fn rejected_reset_ack_disconnects_after_reject_and_logout(
    #[values(false, true)] running: bool,
    #[values(false, true)] logout_sent: bool,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, running);
    let initial = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
    let InputResult::AdminMsg(ack) = engine
        .on_input(reset_ack(1, Some(true), None), &mut storage)
        .unwrap()
    else {
        panic!("expected ACK");
    };
    engine
        .process_admin_input(ack, reset_callback_action(0), &mut storage)
        .unwrap();
    assert!(!engine.state.local_reset_unconfirmed);
    assert!(engine.should_disconnect());
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(
            SessionEndReason::ApplicationForcedDisconnect
        ))
    );
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    let reject = commit_reset_admin(&mut engine, &mut storage);
    assert_eq!(reject.header.msg_seq_num, if logout_sent { 3 } else { 2 });
    assert_matches!(as_admin(&reject), AdminBase::Reject(r)
            if r.ref_seq_num == 1
                && r.ref_msg_type.as_deref() == Some(fix_str!("A"))
                && r.session_reject_reason == Some(SessionRejectReasonBase::ValueIsIncorrect.into()));
    if !logout_sent {
        let logout = commit_reset_admin(&mut engine, &mut storage);
        assert_eq!(logout.header.msg_seq_num, 3);
        assert_matches!(as_admin(&logout), AdminBase::Logout(l)
                if l.text.as_deref() == Some(fix_str!("Reset Logon acknowledgement rejected by application")));
    }
    assert!(!engine.has_admin_output());
    assert_eq!(storage.next_sender_msg_seq_num().get(), 4);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        initial.as_slice()
    );
}

#[test]
fn reset_timeout_outside_a_reset_has_no_effect() {
    let (mut engine, _) = EngineBuilder::new().logged_on().build();
    engine.on_reset_timeout();
    assert!(!engine.has_admin_output());
    assert!(!engine.should_disconnect());
}

#[rstest]
fn disconnect_preserves_the_cause_and_records_unconfirmed_reset_separately(
    #[values(
        SessionEndReason::ApplicationForcedDisconnect,
        SessionEndReason::InvalidLogonState
    )]
    reason: SessionEndReason,
    #[values(false, true)] unconfirmed: bool,
) {
    let (mut engine, _) = reset_waiting_engine(false);
    engine.state.local_reset_unconfirmed = unconfirmed;
    engine.begin_disconnect(reason);
    let expected = EndContext::Closing(reason);
    assert_eq!(engine.session_end().map(|end| end.context), Some(expected));
    assert_eq!(engine.session_end().unwrap().reset_unconfirmed, unconfirmed);
    engine.state.local_reset_unconfirmed = !unconfirmed;
    engine.begin_disconnect(SessionEndReason::StorageError);
    assert_eq!(engine.session_end().map(|end| end.context), Some(expected));
    assert_eq!(engine.session_end().unwrap().reset_unconfirmed, unconfirmed);
    engine.end_transport(Termination::IoError {
        operation: IoOperation::Write,
        error: io::Error::new(io::ErrorKind::BrokenPipe, "farewell failed"),
    });
    let end = engine.session_end().unwrap();
    assert_eq!(end.context, expected);
    assert_eq!(end.reset_unconfirmed, unconfirmed);
    assert_matches!(&end.termination, Termination::IoError { operation: IoOperation::Write, error }
            if error.kind() == io::ErrorKind::BrokenPipe && error.to_string() == "farewell failed");
}

#[rstest]
fn reset_waiting_timeouts_report_whether_the_reset_was_confirmed(
    #[values(false, true)] logout_sent: bool,
    #[values(false, true)] unconfirmed: bool,
) {
    let (mut engine, _) = reset_waiting_engine(logout_sent);
    engine.state.local_reset_unconfirmed = unconfirmed;
    if logout_sent {
        engine.on_logout_timeout();
    } else {
        engine.on_logon_timeout();
    }
    let expected = if logout_sent {
        TimeoutKind::LogoutResponse
    } else {
        TimeoutKind::Logon
    };
    let end = engine.session_end().unwrap();
    assert_eq!(
        end.context,
        EndContext::Closing(SessionEndReason::Timeout(expected))
    );
    assert_eq!(end.reset_unconfirmed, unconfirmed);
    assert_matches!(end.termination, Termination::Timeout(kind) if kind == expected);
    assert!(!engine.has_admin_output());
}

#[test]
fn logout_timeout_after_reset_ack_reports_confirmed_reset() {
    let (mut engine, mut storage) = reset_waiting_engine(true);
    accept_input(&mut engine, reset_ack(1, Some(true), None), &mut storage);
    assert!(!engine.state.local_reset_unconfirmed);
    engine.on_logout_timeout();
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(SessionEndReason::Timeout(
            TimeoutKind::LogoutResponse
        )))
    );
    assert_matches!(
        engine.session_end().unwrap().termination,
        Termination::Timeout(TimeoutKind::LogoutResponse)
    );
    assert!(!engine.session_end().unwrap().reset_unconfirmed);
    assert!(!engine.has_admin_output());
}

#[rstest]
fn running_reset_logout_timeout_reports_confirmation(#[values(false, true)] confirmed: bool) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, true);
    if confirmed {
        accept_input(&mut engine, reset_ack(1, Some(true), None), &mut storage);
        assert!(!engine.state.local_reset_unconfirmed);
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    }
    engine.on_logout_timeout();
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(SessionEndReason::Timeout(
            TimeoutKind::LogoutResponse
        )))
    );
    assert_matches!(
        engine.session_end().unwrap().termination,
        Termination::Timeout(TimeoutKind::LogoutResponse)
    );
    assert_eq!(engine.session_end().unwrap().reset_unconfirmed, !confirmed);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    assert_eq!(
        storage.next_target_msg_seq_num().get(),
        if confirmed { 2 } else { 1 }
    );
    assert!(!engine.has_admin_output());
}

#[rstest]
#[case(LogonState::Idle, EndContext::Logon)]
#[case(LogonState::LogonSent, EndContext::Logon)]
#[case(LogonState::Established, EndContext::Established)]
#[case(LogonState::ResetPending, EndContext::ResetPreparation)]
#[case(LogonState::ResetProbe, EndContext::ResetPreparation)]
#[case(LogonState::ResetSent, EndContext::Reset)]
#[case(LogonState::LogoutSent { sent_at: None }, EndContext::LocalLogout)]
fn transport_failure_keeps_the_session_phase_and_reset_recovery_state(
    #[case] state: LogonState,
    #[case] context: EndContext,
    #[values(false, true)] unconfirmed: bool,
) {
    let (mut engine, _) = EngineBuilder::new().build();
    engine.state.logon_state = state;
    if !matches!(state, LogonState::Idle | LogonState::LogonSent) {
        engine.mark_opening_complete();
    }
    engine.state.local_reset_unconfirmed = unconfirmed;
    engine.end_transport(Termination::IoError {
        operation: IoOperation::Read,
        error: io::Error::new(io::ErrorKind::ConnectionReset, "peer reset"),
    });
    let end = engine.session_end().unwrap();
    assert_eq!(end.context, context);
    assert_eq!(end.reset_unconfirmed, unconfirmed);
    assert_matches!(&end.termination, Termination::IoError { operation: IoOperation::Read, error }
            if error.kind() == io::ErrorKind::ConnectionReset && error.to_string() == "peer reset");
}

#[rstest]
fn farewell_write_failure_preserves_the_timeout_that_started_teardown(
    #[values(
        TimeoutKind::Logon,
        TimeoutKind::LogoutResponse,
        TimeoutKind::ResetPreparation
    )]
    timeout: TimeoutKind,
) {
    let (mut engine, _) = EngineBuilder::new().build();
    engine.end_timeout(timeout);
    engine.end_transport(Termination::IoError {
        operation: IoOperation::Write,
        error: io::Error::new(io::ErrorKind::BrokenPipe, "farewell failed"),
    });
    let end = engine.session_end().unwrap();
    assert_eq!(
        end.context,
        EndContext::Closing(SessionEndReason::Timeout(timeout))
    );
    assert_matches!(&end.termination, Termination::IoError { operation: IoOperation::Write, error }
            if error.kind() == io::ErrorKind::BrokenPipe && error.to_string() == "farewell failed");
}

#[rstest]
fn application_cannot_keep_reset_ack_open_after_consuming_its_number(
    #[values(false, true)] running: bool,
    #[values(false, true)] logout_sent: bool,
    #[values(0, 1, 2, 3)] action: usize,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, running);
    let InputResult::AdminMsg(msg) = engine
        .on_input(test_helpers::logout(1), &mut storage)
        .unwrap()
    else {
        panic!("expected Logout callback")
    };
    engine
        .process_admin_input(msg, reset_callback_action(action), &mut storage)
        .unwrap();
    engine.end_session_if_reset_ack_number_consumed(&storage);
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(if action >= 2 {
            SessionEndReason::ApplicationForcedDisconnect
        } else {
            SessionEndReason::InvalidLogonState
        }))
    );
    assert!(engine.session_end().unwrap().reset_unconfirmed);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(engine.state.local_reset_unconfirmed);
    let expected: &[MsgTypeBase] = match (logout_sent, action) {
        (false, 0) => &[MsgTypeBase::Reject, MsgTypeBase::Logout],
        (true, 0) => &[MsgTypeBase::Reject],
        (false, 1 | 3) => &[MsgTypeBase::Logout],
        _ => &[],
    };
    assert_eq!(engine.admin_output.len(), expected.len());
    for msg_type in expected {
        assert_msg_type(&take_admin(&mut engine), *msg_type);
    }
}

#[rstest]
fn reset_ack_number_guard_preserves_completed_input_outcomes(
    #[values(false, true)] running: bool,
    #[values(false, true)] logout_sent: bool,
    #[values(0, 1, 2, 3, 4)] action: usize,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, running);
    let before = engine.logout_deadline();
    let initial_state = engine.state.logon_state;
    let initial_sender = if logout_sent { 3 } else { 2 };
    let InputResult::AdminMsg(msg) = engine
        .on_input(reset_ack(1, Some(true), None), &mut storage)
        .unwrap()
    else {
        panic!("expected ACK callback")
    };
    assert!(!engine.state.local_reset_unconfirmed);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    engine
        .process_admin_input(
            msg,
            if action == 4 {
                InputAction::Accept
            } else {
                reset_callback_action(action)
            },
            &mut storage,
        )
        .unwrap();
    let ended = matches!(action, 0 | 2 | 3);
    let expected_reason = ended.then_some(EndContext::Closing(
        SessionEndReason::ApplicationForcedDisconnect,
    ));
    let expected_replies: &[MsgTypeBase] = match (logout_sent, action) {
        (false, 0) => &[MsgTypeBase::Reject, MsgTypeBase::Logout],
        (true, 0) => &[MsgTypeBase::Reject],
        (false, 1 | 3) => &[MsgTypeBase::Logout],
        _ => &[],
    };
    // Assert the callback's outcome itself, then assert that the
    // completed-input guard preserves that expected outcome.
    for after_guard in [false, true] {
        if after_guard {
            engine.end_session_if_reset_ack_number_consumed(&storage);
        }
        assert_eq!(engine.session_end().map(|end| end.context), expected_reason);
        assert_eq!(engine.should_disconnect(), ended);
        assert_eq!(engine.admin_output.len(), expected_replies.len());
        for (reply, expected_type) in engine.admin_output.iter().zip(expected_replies) {
            assert_msg_type(reply, *expected_type);
        }
        assert_eq!(storage.next_sender_msg_seq_num().get(), initial_sender);
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert!(!engine.state.local_reset_unconfirmed);
        if logout_sent {
            assert_eq!(engine.state.logon_state, initial_state);
            assert_eq!(engine.logout_deadline(), before);
        } else {
            match action {
                1 => {
                    assert_matches!(engine.state.logon_state, LogonState::LogoutSent { .. });
                    assert!(engine.logout_deadline().is_none());
                }
                4 => assert_eq!(engine.state.logon_state, LogonState::Established),
                _ => assert_eq!(engine.state.logon_state, initial_state),
            }
        }
    }
    let mut expected_seq_num = initial_sender;
    while let Some(mut reply) = engine.take_admin_output() {
        assert!(engine.fill_header(&mut reply, &mut storage).unwrap());
        assert_eq!(reply.header.msg_seq_num, expected_seq_num);
        assert!(engine.commit_send(reply, &mut storage).is_ok());
        expected_seq_num += 1;
    }
    let final_sender = match (logout_sent, action) {
        (_, 0) => 4,
        (true, _) | (false, 1 | 3) => 3,
        (false, _) => 2,
    };
    assert_eq!(storage.next_sender_msg_seq_num().get(), final_sender);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    if running && action == 1 {
        assert!(engine.accepts_app_sends());
        accept_input(&mut engine, test_helpers::logout(2), &mut storage);
        assert_eq!(storage.next_target_msg_seq_num().get(), 3);
        assert_eq!(
            engine.session_end().map(|end| end.context),
            Some(EndContext::LocalLogout)
        );
        assert!(!engine.has_admin_output());
    }
}

#[rstest]
fn header_rejected_ack_fails_the_reset(
    #[values(false, true)] running: bool,
    #[values(false, true)] logout_sent: bool,
    #[values(false, true)] wrong_comp_id: bool,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, running);
    let mut ack = reset_ack(1, Some(true), None);
    ack.header.poss_dup_flag = Some(true);
    if wrong_comp_id {
        ack.header.sender_comp_id = fix_str!("WRONG").to_owned();
    }
    assert_matches!(
        engine
            .process_input_without_callback(ack, &mut storage)
            .unwrap(),
        InputResult::Handled
    );
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(if wrong_comp_id {
            SessionEndReason::InvalidCompId
        } else {
            SessionEndReason::InvalidLogonState
        }))
    );
    assert!(engine.session_end().unwrap().reset_unconfirmed);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    let (expected_reason, expected_text) = if wrong_comp_id {
        (
            SessionRejectReasonBase::CompIdProblem,
            fix_str!("SenderCompID does not match"),
        )
    } else {
        (
            SessionRejectReasonBase::RequiredTagMissing,
            fix_str!("Required tag missing: OrigSendingTime(122)"),
        )
    };
    assert_matches!(as_admin(&take_admin(&mut engine)), AdminBase::Reject(reject)
            if reject.ref_tag_id == Some(if wrong_comp_id { 49 } else { 122 })
            && reject.session_reject_reason == Some(expected_reason.into())
            && reject.text.as_deref() == Some(expected_text));
    if !logout_sent {
        assert_matches!(as_admin(&take_admin(&mut engine)), AdminBase::Logout(logout)
                if logout.text.as_deref() == Some(expected_text));
    }
    assert!(!engine.has_admin_output());
}

#[rstest]
fn garbled_reset_response_leaves_confirmation_and_numbering_unchanged(
    #[values(false, true)] running: bool,
    #[values(false, true)] logout_sent: bool,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, running);
    engine
        .on_deserialize_error(
            DeserializeErrorKind::Garbled(GarbledReason::MessageNotWellFormed).into(),
            &mut storage,
        )
        .unwrap();
    engine.end_session_if_reset_ack_number_consumed(&storage);
    assert!(engine.state.local_reset_unconfirmed);
    assert!(engine.session_end().map(|end| end.context).is_none());
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert!(!engine.has_admin_output());
    if running {
        assert_matches!(
            engine
                .process_input_without_callback(test_helpers::heartbeat(2, None), &mut storage)
                .unwrap(),
            InputResult::Handled
        );
        assert_eq!(
            engine.session_end().map(|end| end.context),
            Some(EndContext::Closing(SessionEndReason::InvalidLogonState))
        );
        assert!(engine.session_end().unwrap().reset_unconfirmed);
        assert_eq!(storage.next_target_msg_seq_num().get(), 1);
        if !logout_sent {
            let logout = commit_reset_admin(&mut engine, &mut storage);
            assert_matches!(as_admin(&logout), AdminBase::Logout(l) if l.text.as_deref() == Some(fix_str!("Unexpected Heartbeat(0) during sequence number reset")));
        }
        assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
        assert!(!engine.has_admin_output());
    }
}

#[rstest]
fn reset_ack_during_logout_preserves_the_deadline_and_clears_negotiation(
    #[values(false, true)] running: bool,
    #[values(false, true)] second_logon: bool,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, running);
    engine.session_settings.enable_next_expected_msg_seq_num = true;
    engine.state.next_expected_msg_seq_num = Some(nz_seq(1));
    let before = engine.state.logon_state;
    accept_input(&mut engine, reset_ack(1, Some(true), Some(3)), &mut storage);
    assert!(!engine.state.local_reset_unconfirmed);
    assert!(engine.state.next_expected_msg_seq_num.is_none());
    assert_eq!(engine.state.logon_state, before);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(!engine.has_admin_output());
    if second_logon {
        assert_matches!(
            engine
                .process_input_without_callback(reset_ack(1, Some(true), None), &mut storage)
                .unwrap(),
            InputResult::Handled
        );
        assert_eq!(
            engine.session_end().map(|end| end.context),
            Some(EndContext::Closing(SessionEndReason::InvalidLogonState))
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    } else {
        accept_input(&mut engine, test_helpers::logout(2), &mut storage);
        assert_eq!(
            engine.session_end().map(|end| end.context),
            Some(EndContext::LocalLogout)
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 3);
    }
    assert!(!engine.has_admin_output());
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
}

#[rstest]
fn non_ack_traffic_during_logout_cannot_consume_the_reset_ack_number(
    #[values(false, true)] running: bool,
    #[values(false, true)] decoded: bool,
) {
    let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, running);
    if decoded {
        assert_matches!(
            engine
                .process_input_without_callback(test_helpers::heartbeat(1, None), &mut storage)
                .unwrap(),
            InputResult::Handled
        );
    } else {
        engine
            .on_deserialize_error(
                broken_reset_response(Some(fix_str!("0")), 1, true),
                &mut storage,
            )
            .unwrap();
    }
    assert_eq!(
        engine.session_end().map(|end| end.context),
        Some(EndContext::Closing(SessionEndReason::InvalidLogonState))
    );
    assert!(engine.session_end().unwrap().reset_unconfirmed);
    assert!(engine.state.local_reset_unconfirmed);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
    assert!(!engine.has_admin_output());
}
