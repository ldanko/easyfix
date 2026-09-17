use std::{assert_matches, borrow::Cow};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase, SessionStatusBase},
    deserializer::{DeserializeErrorKind, LogoutReason},
    fix_str,
};
use tokio::time::{Duration, advance};

use super::{
    reset_peer_support::reset_test_session,
    reset_support::{reset_ack, reset_waiting_engine_with_origin},
    support::assert_msg_type,
};
use crate::{
    application::{DisconnectReason, InputAction},
    engine::InputResult,
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, take_admin},
};

#[tokio::test]
async fn invalid_ack_body_values_do_not_send_a_second_logout() {
    for running in [false, true] {
        for negative_heartbeat in [false, true] {
            let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, running);
            engine.session_settings.enable_next_expected_msg_seq_num = true;
            let ack = test_helpers::logon_with_options(
                1,
                fix_str!("TARGET"),
                fix_str!("SENDER"),
                if negative_heartbeat { -1 } else { 30 },
                Some(true),
                if negative_heartbeat { None } else { Some(0) },
            );
            assert_matches!(
                engine.process_input_without_callback(ack, &mut storage),
                Ok(InputResult::Handled)
            );
            assert!(!engine.state.local_reset_unconfirmed);
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::InvalidLogonState)
            );
            assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
            assert_eq!(
                storage.next_target_msg_seq_num().get(),
                if negative_heartbeat { 2 } else { 1 }
            );
            if negative_heartbeat {
                let mut reject = take_admin(&mut engine);
                assert_matches!(as_admin(&reject), AdminBase::Reject(reject)
                if reject.ref_tag_id == Some(108)
                && reject.session_reject_reason == Some(SessionRejectReasonBase::ValueIsIncorrect.into()));
                assert!(engine.fill_header(&mut reject, &mut storage).unwrap());
                assert!(engine.commit_send(reject, &mut storage).is_ok());
                assert_eq!(storage.next_sender_msg_seq_num().get(), 4);
            }
            assert!(!engine.has_admin_output());
        }
        for established in [false, true] {
            let (mut engine, mut storage, history) = reset_test_session(established);
            let request = test_helpers::logon_with_options(
                1,
                fix_str!("TARGET"),
                fix_str!("SENDER"),
                -1,
                Some(true),
                None,
            );
            assert_matches!(
                engine.process_input_without_callback(request, &mut storage),
                Ok(InputResult::Handled)
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
            for msg_type in [MsgTypeBase::Reject, MsgTypeBase::Logout] {
                let mut reply = take_admin(&mut engine);
                assert_msg_type(&reply, msg_type);
                assert!(engine.fill_header(&mut reply, &mut storage).unwrap());
                assert!(engine.commit_send(reply, &mut storage).is_ok());
            }
            assert_eq!(storage.next_sender_msg_seq_num().get(), 42);
            assert_eq!(storage.next_target_msg_seq_num().get(), 40);
            assert_eq!(
                storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                history.as_slice()
            );
            assert!(!engine.has_admin_output());
        }
    }
}

#[test]
fn logout_sent_error_paths_keep_the_first_logout_only() {
    for running in [false, true] {
        for unconfirmed in [false, true] {
            for case in 0..3 {
                let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, running);
                engine.state.local_reset_unconfirmed = unconfirmed;
                let reason = match case {
                    0 => {
                        engine
                            .on_deserialize_error(
                                DeserializeErrorKind::Logout(LogoutReason::MsgSeqNumMissing).into(),
                                &mut storage,
                            )
                            .unwrap();
                        DisconnectReason::MsgSeqNumNotFound
                    }
                    1 => {
                        engine
                            .on_deserialize_error(
                                DeserializeErrorKind::Logout(LogoutReason::BeginStringMismatch)
                                    .into(),
                                &mut storage,
                            )
                            .unwrap();
                        DisconnectReason::InvalidBeginString
                    }
                    _ => {
                        engine.on_oversized_message(10_000);
                        DisconnectReason::MessageTooLarge
                    }
                };
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(if unconfirmed {
                        DisconnectReason::SeqNumResetFailed
                    } else {
                        reason
                    })
                );
                assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
                assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                assert!(!engine.has_admin_output());
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn repeated_logout_keeps_the_original_message_and_deadline() {
    for running in [false, true] {
        for confirmed in [false, true] {
            let (mut engine, mut storage) = reset_waiting_engine_with_origin(false, running);
            engine.on_control(ControlMsg::Logout {
                session_status: Some(SessionStatusBase::SessionLogoutComplete.into()),
                text: Some(Cow::Borrowed(fix_str!("Original Logout"))),
            });
            let mut original_state = engine.state.logon_state;
            let mut deadline = engine.logout_deadline();
            assert!(deadline.is_none());
            for drained in [false, true] {
                advance(Duration::from_secs(1)).await;
                engine.on_control(ControlMsg::Logout {
                    session_status: None,
                    text: Some(Cow::Borrowed(fix_str!("Replacement"))),
                });
                engine.send_logout(None, Some(Cow::Borrowed(fix_str!("Replacement direct"))));
                if drained {
                    engine.mark_logout_written();
                }
                assert_eq!(engine.state.logon_state, original_state);
                assert_eq!(engine.logout_deadline(), deadline);
                if !drained {
                    let mut logout = take_admin(&mut engine);
                    assert_matches!(as_admin(&logout), AdminBase::Logout(logout)
                    if logout.text.as_deref() == Some(fix_str!("Original Logout"))
                    && logout.session_status == Some(SessionStatusBase::SessionLogoutComplete.into()));
                    assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
                    assert!(engine.commit_send(logout, &mut storage).is_ok());
                    let sent_at = engine.timer_backend().now();
                    engine.mark_logout_written();
                    deadline = engine.logout_deadline();
                    assert_eq!(
                        deadline,
                        sent_at
                            .checked_add(engine.session_settings().auto_disconnect_after_no_logout)
                    );
                    assert!(deadline.is_some());
                    original_state = engine.state.logon_state;
                    if confirmed {
                        accept_input(&mut engine, reset_ack(1, Some(true), None), &mut storage);
                        assert!(!engine.state.local_reset_unconfirmed);
                    }
                }
                assert!(!engine.has_admin_output());
                assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
            }
        }
        for acknowledged in [false, true] {
            let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
            if acknowledged {
                accept_input(&mut engine, test_helpers::logout(1), &mut storage);
            } else {
                engine.send_logout(None, None);
            }
            let mut original_state = engine.state.logon_state;
            for drained in [false, true] {
                advance(Duration::from_secs(1)).await;
                engine.on_control(ControlMsg::Logout {
                    session_status: None,
                    text: None,
                });
                engine.send_logout(None, None);
                if drained {
                    engine.mark_logout_written();
                }
                assert_eq!(engine.state.logon_state, original_state);
                if !drained {
                    let mut logout = take_admin(&mut engine);
                    assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
                    assert!(engine.commit_send(logout, &mut storage).is_ok());
                    engine.mark_logout_written();
                    if !acknowledged {
                        assert!(engine.logout_deadline().is_some());
                    }
                    original_state = engine.state.logon_state;
                }
                assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
                assert!(!engine.has_admin_output());
            }
        }
    }
}

#[test]
fn suppressing_logout_preserves_the_application_action() {
    for running in [false, true] {
        for disconnect in [false, true] {
            let (mut engine, mut storage) = reset_waiting_engine_with_origin(true, running);
            engine.state.local_reset_unconfirmed = false;
            let before = engine.state.logon_state;
            let InputResult::AdminMsg(msg) = engine
                .on_input(test_helpers::heartbeat(1, None), &mut storage)
                .unwrap()
            else {
                panic!("expected Heartbeat callback")
            };
            engine
                .process_admin_input(
                    msg,
                    InputAction::Logout {
                        session_status: None,
                        text: None,
                        disconnect,
                    },
                    &mut storage,
                )
                .unwrap();
            assert_eq!(engine.state.logon_state, before);
            assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            assert!(!engine.has_admin_output());
            assert_eq!(
                engine.disconnect_reason(),
                disconnect.then_some(DisconnectReason::ApplicationForcedDisconnect)
            );
        }
    }
}
