use std::{assert_matches, num::NonZeroU64};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    basic_types::FixStr,
    deserializer::{DeserializeErrorKind, GarbledReason},
    fix_str,
};

use super::{
    reset_support::{
        broken_reset_response, commit_reset_admin, reset_ack, reset_callback_action,
        reset_waiting_engine, reset_waiting_engine_with_origin, running_reset_probe,
    },
    support::assert_msg_type,
};
use crate::{
    application::{DisconnectReason, InputAction},
    engine::{InputResult, LogonState},
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, take_admin},
};

#[test]
fn running_reset_ack_uses_the_effective_heartbeat_and_current_sender_counter() {
    for logout_sent in [false, true] {
        for (effective, echoed) in [(30, 20), (20, 30), (20, 20)] {
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
                    engine.disconnect_reason(),
                    Some(DisconnectReason::InvalidLogonState)
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
        for enabled in [false, true] {
            for offered in [None, Some(1), Some(2), Some(3), Some(4), Some(40)] {
                let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, true);
                engine.session_settings.enable_next_expected_msg_seq_num = enabled;
                let sender = storage.next_sender_msg_seq_num().get();
                let before = engine.state.logon_state;
                accept_input(&mut engine, reset_ack(1, Some(true), offered), &mut storage);
                let refused = enabled && offered.is_some_and(|n| n > sender);
                assert_eq!(
                    engine.disconnect_reason(),
                    refused.then_some(DisconnectReason::SeqNumResetFailed)
                );
                assert_eq!(engine.state.local_reset_unconfirmed, refused);
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
        }
    }
}

#[tokio::test]
async fn rejected_reset_ack_disconnects_after_reject_and_logout() {
    for running in [false, true] {
        for logout_sent in [false, true] {
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
                engine.disconnect_reason(),
                Some(DisconnectReason::ApplicationForcedDisconnect)
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
    }
}

#[test]
fn reset_timeout_outside_a_reset_has_no_effect() {
    let (mut engine, _) = EngineBuilder::new().logged_on().build();
    engine.on_reset_timeout();
    assert!(!engine.has_admin_output());
    assert!(!engine.should_disconnect());
}

#[test]
fn begin_disconnect_normalises_to_seq_num_reset_failed_while_a_local_reset_is_unconfirmed() {
    for reason in [
        DisconnectReason::IoError,
        DisconnectReason::Disconnected,
        DisconnectReason::LocalRequestedLogout,
        DisconnectReason::InvalidLogonState,
    ] {
        for unconfirmed in [false, true] {
            let (mut engine, _) = reset_waiting_engine(false);
            engine.state.local_reset_unconfirmed = unconfirmed;
            engine.begin_disconnect(reason);
            let expected = if unconfirmed {
                DisconnectReason::SeqNumResetFailed
            } else {
                reason
            };
            assert_eq!(engine.disconnect_reason(), Some(expected));
            engine.state.local_reset_unconfirmed = !unconfirmed;
            engine.begin_disconnect(DisconnectReason::Disconnected);
            assert_eq!(engine.disconnect_reason(), Some(expected));
        }
    }
}

#[test]
fn reset_waiting_timeouts_report_whether_the_reset_was_confirmed() {
    for logout_sent in [false, true] {
        for unconfirmed in [false, true] {
            let (mut engine, _) = reset_waiting_engine(logout_sent);
            engine.state.local_reset_unconfirmed = unconfirmed;
            if logout_sent {
                engine.on_logout_timeout();
            } else {
                engine.on_logon_timeout();
            }
            let expected = if unconfirmed {
                DisconnectReason::SeqNumResetFailed
            } else if logout_sent {
                DisconnectReason::LocalRequestedLogoutTimeout
            } else {
                DisconnectReason::LogonTimeout
            };
            assert_eq!(engine.disconnect_reason(), Some(expected));
            assert!(!engine.has_admin_output());
        }
    }
    let (mut engine, mut storage) = reset_waiting_engine(true);
    accept_input(&mut engine, reset_ack(1, Some(true), None), &mut storage);
    assert!(!engine.state.local_reset_unconfirmed);
    engine.on_logout_timeout();
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::LocalRequestedLogoutTimeout)
    );
    assert!(!engine.has_admin_output());
    for confirmed in [false, true] {
        let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, true);
        if confirmed {
            accept_input(&mut engine, reset_ack(1, Some(true), None), &mut storage);
            assert!(!engine.state.local_reset_unconfirmed);
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        }
        engine.on_logout_timeout();
        assert_eq!(
            engine.disconnect_reason(),
            Some(if confirmed {
                DisconnectReason::LocalRequestedLogoutTimeout
            } else {
                DisconnectReason::SeqNumResetFailed
            })
        );
        assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
        assert_eq!(
            storage.next_target_msg_seq_num().get(),
            if confirmed { 2 } else { 1 }
        );
        assert!(!engine.has_admin_output());
    }
}

#[test]
fn application_cannot_keep_reset_ack_open_after_consuming_its_number() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for action in 0..4 {
                let (mut engine, mut storage) =
                    reset_waiting_engine_with_origin(logout_sent, running);
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
                    engine.disconnect_reason(),
                    Some(DisconnectReason::SeqNumResetFailed)
                );
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
        }
    }
}

#[test]
fn reset_ack_number_guard_preserves_completed_input_outcomes() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for action in 0..5 {
                let (mut engine, mut storage) =
                    reset_waiting_engine_with_origin(logout_sent, running);
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
                let expected_reason =
                    ended.then_some(DisconnectReason::ApplicationForcedDisconnect);
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
                    assert_eq!(engine.disconnect_reason(), expected_reason);
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
                                assert_matches!(
                                    engine.state.logon_state,
                                    LogonState::LogoutSent { .. }
                                );
                                assert!(engine.logout_deadline().is_some());
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
                        engine.disconnect_reason(),
                        Some(DisconnectReason::LocalRequestedLogout)
                    );
                    assert!(!engine.has_admin_output());
                }
            }
        }
    }
}

#[test]
fn header_rejected_ack_fails_the_reset() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for wrong_comp_id in [false, true] {
                let (mut engine, mut storage) =
                    reset_waiting_engine_with_origin(logout_sent, running);
                let mut ack = reset_ack(1, Some(true), None);
                ack.header.poss_dup_flag = Some(true);
                if wrong_comp_id {
                    ack.header.sender_comp_id = fix_str!("WRONG").to_owned();
                }
                assert_matches!(
                    engine.on_input(ack, &mut storage).unwrap(),
                    InputResult::Handled
                );
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::SeqNumResetFailed)
                );
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
        }
    }
}

#[test]
fn garbled_reset_response_leaves_confirmation_and_numbering_unchanged() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, running);
            engine
                .on_deserialize_error(
                    DeserializeErrorKind::Garbled(GarbledReason::MessageNotWellFormed).into(),
                    &mut storage,
                )
                .unwrap();
            engine.end_session_if_reset_ack_number_consumed(&storage);
            assert!(engine.state.local_reset_unconfirmed);
            assert!(engine.disconnect_reason().is_none());
            assert_eq!(storage.next_target_msg_seq_num().get(), 1);
            assert!(!engine.has_admin_output());
            if running {
                assert_matches!(
                    engine
                        .on_input(test_helpers::heartbeat(2, None), &mut storage)
                        .unwrap(),
                    InputResult::Handled
                );
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::SeqNumResetFailed)
                );
                assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                if !logout_sent {
                    let logout = commit_reset_admin(&mut engine, &mut storage);
                    assert_matches!(as_admin(&logout), AdminBase::Logout(l) if l.text.as_deref() == Some(fix_str!("Unexpected Heartbeat(0) during sequence number reset")));
                }
                assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
                assert!(!engine.has_admin_output());
            }
        }
    }
}

#[test]
fn reset_ack_during_logout_preserves_the_deadline_and_clears_negotiation() {
    for running in [false, true] {
        for second_logon in [false, true] {
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
                        .on_input(reset_ack(1, Some(true), None), &mut storage)
                        .unwrap(),
                    InputResult::Handled
                );
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::InvalidLogonState)
                );
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            } else {
                accept_input(&mut engine, test_helpers::logout(2), &mut storage);
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::LocalRequestedLogout)
                );
                assert_eq!(storage.next_target_msg_seq_num().get(), 3);
            }
            assert!(!engine.has_admin_output());
            assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
        }
    }
}

#[test]
fn non_ack_traffic_during_logout_cannot_consume_the_reset_ack_number() {
    for running in [false, true] {
        for decoded in [false, true] {
            let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, running);
            if decoded {
                assert_matches!(
                    engine
                        .on_input(test_helpers::heartbeat(1, None), &mut storage)
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
                engine.disconnect_reason(),
                Some(DisconnectReason::SeqNumResetFailed)
            );
            assert!(engine.state.local_reset_unconfirmed);
            assert_eq!(storage.next_target_msg_seq_num().get(), 1);
            assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
            assert!(!engine.has_admin_output());
        }
    }
}
