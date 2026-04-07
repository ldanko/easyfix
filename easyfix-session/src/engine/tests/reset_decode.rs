use std::{assert_matches, borrow::Cow};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    deserializer::{DeserializeErrorKind, raw_message},
    fix_str,
    message::SessionMessage,
};
use easyfix_test_messages::Message;

use super::{
    reset_support::{broken_reset_response, reset_ack, reset_waiting_engine_with_origin},
    support::assert_msg_type,
};
use crate::{
    application::DisconnectReason,
    engine::{InputResult, LogonState},
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{accept_input, as_admin, nz_seq, serialize_message, take_admin},
};

#[tokio::test]
async fn malformed_reset_ack_is_refused_without_another_logout() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for (seq, flag, next_expected) in [
                (2, Some(true), None),
                (0, Some(true), None),
                (1, None, None),
                (1, Some(false), None),
                (1, Some(true), Some(40)),
            ] {
                let (mut engine, mut storage) =
                    reset_waiting_engine_with_origin(logout_sent, running);
                let initial_sender = if logout_sent { 3 } else { 2 };
                let initial_logon = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
                let initial_logout = storage
                    .fetch(nz_seq(2), nz_seq(2))
                    .await
                    .map(<[u8]>::to_vec);
                assert_eq!(initial_logout.is_ok(), logout_sent);
                engine.session_settings.enable_next_expected_msg_seq_num = true;
                assert_matches!(
                    engine
                        .on_input(reset_ack(seq, flag, next_expected), &mut storage)
                        .unwrap(),
                    InputResult::Handled
                );
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::SeqNumResetFailed)
                );
                assert!(engine.state.local_reset_unconfirmed);
                assert_eq!(storage.next_sender_msg_seq_num().get(), initial_sender);
                assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                assert_eq!(
                    storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                    initial_logon.as_slice()
                );
                assert_eq!(
                    storage.fetch(nz_seq(2), nz_seq(2)).await.as_deref(),
                    initial_logout.as_deref()
                );
                if !logout_sent {
                    let expected = if next_expected.is_some() {
                        fix_str!("NextExpectedMsgSeqNum(789) too high (expected 2, got 40)")
                    } else {
                        fix_str!(
                            "Sequence number reset acknowledgement requires ResetSeqNumFlag=Y and MsgSeqNum=1"
                        )
                    };
                    let mut reply = take_admin(&mut engine);
                    assert_matches!(as_admin(&reply), AdminBase::Logout(logout) if logout.text.as_deref() == Some(expected));
                    assert!(engine.fill_header(&mut reply, &mut storage).unwrap());
                    assert_eq!(reply.header.msg_seq_num, 2);
                    assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
                    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                    assert_eq!(
                        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                        initial_logon.as_slice()
                    );
                    assert!(storage.fetch(nz_seq(2), nz_seq(2)).await.is_err());
                    let reply_bytes = serialize_message(&reply);
                    assert!(engine.commit_send(reply, &mut storage).is_ok());
                    assert_eq!(
                        storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap(),
                        reply_bytes.as_slice()
                    );
                } else {
                    assert_eq!(
                        storage.fetch(nz_seq(2), nz_seq(2)).await.as_deref(),
                        initial_logout.as_deref()
                    );
                }
                assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
                assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                assert_eq!(
                    storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                    initial_logon.as_slice()
                );
                assert!(!engine.has_admin_output());
            }
        }
    }
}

#[test]
fn retransmitted_logon_cannot_acknowledge_a_local_reset() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for seq in [1, 2] {
                let (mut engine, mut storage) =
                    reset_waiting_engine_with_origin(logout_sent, running);
                let mut ack = reset_ack(seq, Some(true), None);
                ack.header.poss_dup_flag = Some(true);
                ack.header.orig_sending_time = Some(ack.header.sending_time);
                assert_matches!(
                    engine.on_input(ack, &mut storage).unwrap(),
                    InputResult::Handled
                );
                assert!(engine.state.local_reset_unconfirmed);
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::SeqNumResetFailed)
                );
                assert_eq!(
                    storage.next_target_msg_seq_num().get(),
                    if seq == 1 { 2 } else { 1 }
                );
                if !logout_sent {
                    let mut logout = take_admin(&mut engine);
                    assert_matches!(as_admin(&logout), AdminBase::Logout(lo)
                    if lo.text.as_deref() == Some(fix_str!("Retransmitted Logon cannot acknowledge sequence number reset")));
                    assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
                    assert!(engine.commit_send(logout, &mut storage).is_ok());
                }
                assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
                assert!(!engine.has_admin_output());
                assert!(!engine.has_pending_resends());
            }
        }
    }
}

#[tokio::test]
async fn refusing_logout_is_accepted_without_sequence_checks() {
    for running in [false, true] {
        for seq in if running {
            &[40, 501, 600][..]
        } else {
            &[40][..]
        } {
            for logout_sent in [false, true] {
                let (mut engine, mut storage) =
                    reset_waiting_engine_with_origin(logout_sent, running);
                let initial_logon = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
                if running {
                    assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());
                }
                assert_matches!(
                    accept_input(&mut engine, test_helpers::logout(*seq), &mut storage),
                    InputResult::Handled
                );
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                assert!(engine.state.local_reset_unconfirmed);
                assert!(!engine.has_pending_resends());
                assert!(engine.state.queue.is_empty());
                if logout_sent {
                    assert_eq!(
                        engine.disconnect_reason(),
                        Some(DisconnectReason::SeqNumResetFailed)
                    );
                    assert!(!engine.has_admin_output());
                } else {
                    assert_matches!(
                        engine.state.logon_state,
                        LogonState::LogoutAcknowledged { .. }
                    );
                    assert!(engine.disconnect_reason().is_none());
                    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
                    let mut logout = take_admin(&mut engine);
                    assert_msg_type(&logout, MsgTypeBase::Logout);
                    assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
                    assert_eq!(logout.header.msg_seq_num, 2);
                    let logout_bytes = serialize_message(&logout);
                    assert!(storage.fetch(nz_seq(2), nz_seq(2)).await.is_err());
                    assert!(engine.commit_send(logout, &mut storage).is_ok());
                    assert_eq!(
                        storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap(),
                        logout_bytes.as_slice()
                    );
                    engine.end_session_if_reset_ack_number_consumed(&storage);
                    assert!(engine.disconnect_reason().is_none());
                    engine.begin_disconnect(DisconnectReason::RemoteRequestedLogout);
                    assert_eq!(
                        engine.disconnect_reason(),
                        Some(DisconnectReason::SeqNumResetFailed)
                    );
                }
                assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                assert_eq!(
                    storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                    initial_logon.as_slice()
                );
                assert!(!engine.has_admin_output());
            }
        }
    }
}

#[test]
fn reset_refusal_still_validates_headers_when_logout_verification_is_disabled() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for wrong_comp_id in [false, true] {
                let (mut engine, mut storage) =
                    reset_waiting_engine_with_origin(logout_sent, running);
                engine.session_settings.verify_logout = false;
                let mut logout = test_helpers::logout(1);
                logout.header.poss_dup_flag = Some(true);
                if wrong_comp_id {
                    logout.header.sender_comp_id = fix_str!("WRONG").to_owned();
                }
                assert_matches!(
                    engine.on_input(logout, &mut storage).unwrap(),
                    InputResult::Handled
                );
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::SeqNumResetFailed)
                );
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                assert!(engine.state.local_reset_unconfirmed);
                assert_matches!(as_admin(&take_admin(&mut engine)), AdminBase::Reject(reject)
                if reject.ref_tag_id == Some(if wrong_comp_id { 49 } else { 122 }));
                if !logout_sent {
                    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
                }
                assert!(!engine.has_admin_output());
            }
        }
    }
}

#[tokio::test]
async fn undecodable_reset_response_is_terminal_with_or_without_a_header() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for msg_type in [Some(fix_str!("A")), Some(fix_str!("5")), None] {
                for with_header in [false, true] {
                    for seq in [1, 40] {
                        let (mut engine, mut storage) =
                            reset_waiting_engine_with_origin(logout_sent, running);
                        let initial_logon =
                            storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
                        let initial_sender = if logout_sent { 3 } else { 2 };
                        let expected_text = match msg_type {
                            Some(mt) if mt == "A" => {
                                fix_str!("SessionRejectReasonField(6) (tag=108)")
                            }
                            Some(_) => fix_str!("SessionRejectReasonField(6) (tag=1409)"),
                            None => fix_str!("SessionRejectReasonField(11)"),
                        };
                        engine
                            .on_deserialize_error(
                                broken_reset_response(msg_type, seq, with_header),
                                &mut storage,
                            )
                            .unwrap();
                        assert_eq!(
                            engine.disconnect_reason(),
                            Some(DisconnectReason::SeqNumResetFailed)
                        );
                        assert_eq!(
                            storage.next_target_msg_seq_num().get(),
                            if seq == 1 { 2 } else { 1 }
                        );
                        assert!(engine.state.local_reset_unconfirmed);
                        assert_eq!(storage.next_sender_msg_seq_num().get(), initial_sender);
                        let mut reject = take_admin(&mut engine);
                        assert_matches!(as_admin(&reject), AdminBase::Reject(reject)
                        if reject.session_reject_reason == Some(if msg_type.is_none() {
                            SessionRejectReasonBase::InvalidMsgType
                        } else { SessionRejectReasonBase::IncorrectDataFormatForValue }.into())
                        && reject.text.as_deref() == Some(expected_text));
                        assert!(engine.fill_header(&mut reject, &mut storage).unwrap());
                        assert_eq!(reject.header.msg_seq_num, initial_sender);
                        assert!(engine.commit_send(reject, &mut storage).is_ok());
                        if !logout_sent {
                            let mut logout = take_admin(&mut engine);
                            assert_matches!(as_admin(&logout), AdminBase::Logout(logout) if logout.text.as_deref() == Some(expected_text));
                            assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
                            assert_eq!(logout.header.msg_seq_num, 3);
                            assert!(engine.commit_send(logout, &mut storage).is_ok());
                        }
                        assert_eq!(storage.next_sender_msg_seq_num().get(), 4);
                        assert_eq!(
                            storage.next_target_msg_seq_num().get(),
                            if seq == 1 { 2 } else { 1 }
                        );
                        assert_eq!(
                            storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                            initial_logon.as_slice()
                        );
                        assert!(!engine.has_admin_output());
                        assert!(!engine.has_pending_resends());
                        assert!(engine.state.queue.is_empty());
                        assert!(engine.state.resend_range.is_none());
                        assert!(!matches!(
                            engine.state.logon_state,
                            LogonState::LogoutAcknowledged { .. }
                        ));
                    }
                }
            }
        }
    }
}

#[test]
fn unknown_wire_message_type_preserves_its_decoder_reject_during_reset() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            let (mut engine, mut storage) = reset_waiting_engine_with_origin(logout_sent, running);
            let bytes = test_helpers::frame_message(
                "FIXT.1.1",
                "35=ZZ|49=TARGET|56=SENDER|34=1|52=20260909-12:00:00.000|",
            );
            let (_, raw) = raw_message(&bytes).unwrap();
            let error = Message::from_raw_message(raw).unwrap_err();
            assert!(error.header.is_none());
            assert_matches!(&error.kind, DeserializeErrorKind::Reject { msg_type, reason, seq_num, .. }
            if msg_type.as_deref() == Some(fix_str!("ZZ"))
            && *reason == SessionRejectReasonBase::InvalidMsgType && *seq_num == 1);
            engine.on_deserialize_error(error, &mut storage).unwrap();
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::SeqNumResetFailed)
            );
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            assert_matches!(as_admin(&take_admin(&mut engine)), AdminBase::Reject(reject)
            if reject.session_reject_reason == Some(SessionRejectReasonBase::InvalidMsgType.into()));
            if !logout_sent {
                assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
            }
            assert!(!engine.has_admin_output());
        }
    }
}

#[tokio::test]
async fn known_wire_type_without_a_header_preserves_the_reset_state_verdict() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for seq in [1, 501] {
                for (msg_type, expected_text) in [
                    (
                        "0",
                        fix_str!("Unexpected Heartbeat(0) during sequence number reset"),
                    ),
                    (
                        "1",
                        fix_str!("Unexpected TestRequest(1) during sequence number reset"),
                    ),
                    (
                        "2",
                        fix_str!("Unexpected ResendRequest(2) during sequence number reset"),
                    ),
                    (
                        "3",
                        fix_str!("Unexpected Reject(3) during sequence number reset"),
                    ),
                    (
                        "4",
                        fix_str!("Unexpected SequenceReset(4) during sequence number reset"),
                    ),
                    (
                        "D",
                        fix_str!("Unexpected MsgType(D) during sequence number reset"),
                    ),
                ] {
                    let (mut engine, mut storage) =
                        reset_waiting_engine_with_origin(logout_sent, running);
                    let initial_state = engine.state.logon_state;
                    let initial_sender = storage.next_sender_msg_seq_num().get();
                    let initial_logon = storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap().to_vec();
                    let initial_logout = storage
                        .fetch(nz_seq(2), nz_seq(2))
                        .await
                        .map(<[u8]>::to_vec);
                    assert_eq!(initial_sender, if logout_sent { 3 } else { 2 });
                    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                    assert!(storage.fetch(nz_seq(40), nz_seq(40)).await.is_err());

                    let bytes = test_helpers::frame_message(
                        "FIXT.1.1",
                        &format!(
                            "35={msg_type}|49=TARGET|56=SENDER|34={seq}|49=TARGET|52=20260909-12:00:00.000|"
                        ),
                    );
                    let (remaining, raw) = raw_message(&bytes).unwrap();
                    assert!(remaining.is_empty());
                    let error = Message::from_raw_message(raw).unwrap_err();
                    assert!(error.header.is_none());
                    assert_matches!(&error.kind, DeserializeErrorKind::Reject {
                        msg_type: recovered, seq_num, tag: Some(49), reason,
                    } if recovered.as_deref().unwrap().as_bytes() == msg_type.as_bytes()
                        && *seq_num == seq
                        && *reason == SessionRejectReasonBase::TagAppearsMoreThanOnce);
                    assert_matches!(
                        engine.on_deserialize_error(error, &mut storage).unwrap(),
                        InputResult::Error(_)
                    );
                    assert_eq!(
                        engine.disconnect_reason(),
                        Some(DisconnectReason::SeqNumResetFailed)
                    );
                    assert!(engine.state.local_reset_unconfirmed);
                    assert_eq!(engine.state.logon_state, initial_state);
                    assert_eq!(storage.next_sender_msg_seq_num().get(), initial_sender);
                    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                    assert!(engine.state.queue.is_empty());
                    assert!(engine.state.resend_range.is_none());
                    assert!(!engine.has_pending_resends());

                    if running && !logout_sent {
                        let mut logout = take_admin(&mut engine);
                        assert_matches!(as_admin(&logout), AdminBase::Logout(logout)
                            if logout.text.as_deref() == Some(expected_text)
                                && logout.session_status.is_none());
                        assert_eq!(logout.header.msg_seq_num, 0);
                        assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
                        assert_eq!(logout.header.msg_seq_num, 2);
                        assert!(storage.fetch(nz_seq(2), nz_seq(2)).await.is_err());
                        let logout_bytes = serialize_message(&logout);
                        assert!(engine.commit_send(logout, &mut storage).is_ok());
                        assert_eq!(
                            storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap(),
                            logout_bytes.as_slice()
                        );
                    } else {
                        assert_eq!(
                            storage.fetch(nz_seq(2), nz_seq(2)).await.as_deref(),
                            initial_logout.as_deref()
                        );
                    }
                    assert!(!engine.has_admin_output());
                    assert_eq!(
                        storage.next_sender_msg_seq_num().get(),
                        if running || logout_sent { 3 } else { 2 }
                    );
                    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                    assert_eq!(
                        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                        initial_logon.as_slice()
                    );
                    assert!(storage.fetch(nz_seq(3), nz_seq(3)).await.is_err());
                }
            }
        }
    }
}

#[test]
fn header_reject_with_broken_reset_response_body_is_terminal() {
    for running in [false, true] {
        for logout_sent in [false, true] {
            for msg_type in [fix_str!("A"), fix_str!("5")] {
                for wrong_comp_id in [false, true] {
                    let (mut engine, mut storage) =
                        reset_waiting_engine_with_origin(logout_sent, running);
                    let mut error = broken_reset_response(Some(msg_type), 1, true);
                    let header = error.header.as_mut().unwrap();
                    header.poss_dup_flag = Some(true);
                    if wrong_comp_id {
                        header.sender_comp_id = Cow::Borrowed(fix_str!("WRONG"));
                    }
                    engine.on_deserialize_error(error, &mut storage).unwrap();
                    assert_eq!(
                        engine.disconnect_reason(),
                        Some(DisconnectReason::SeqNumResetFailed)
                    );
                    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                    let reject = take_admin(&mut engine);
                    assert_matches!(as_admin(&reject), AdminBase::Reject(reject)
                    if reject.ref_tag_id == Some(if wrong_comp_id { 49 } else { 122 })
                    && reject.session_reject_reason == Some(if wrong_comp_id {
                        SessionRejectReasonBase::CompIdProblem
                    } else { SessionRejectReasonBase::RequiredTagMissing }.into()));
                    if !logout_sent {
                        assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
                    }
                    assert!(!engine.has_admin_output());
                }
            }
        }
    }
}
