use std::{assert_matches, num::NonZeroU16};

use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, MsgTypeBase, SessionRejectReasonBase, SessionStatusBase,
    },
    basic_types::ApplVerId,
    deserializer::raw_message,
    fix_str,
    message::SessionMessage,
};
use easyfix_test_messages::Message;
use tokio::time::Duration;

use super::{
    reset_support::reset_waiting_engine,
    support::{assert_msg_type, limit},
};
use crate::{
    application::{DisconnectReason, InputAction},
    engine::InputResult,
    initiator::SessionStart,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, take_admin},
};

#[test]
fn send_logon_request_produces_logon_and_sets_logon_sent() {
    let (mut engine, mut store) = EngineBuilder::new().build();
    assert!(!engine.is_logged_on());
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Logon);
    // logon_sent should be set after send_logon_request
    // (is_logged_on still false because logon_received is false)
    assert!(!engine.is_logged_on());
}

#[test]
fn unsupported_encrypt_method_logs_out_before_application_input() {
    for initiator in [false, true] {
        for method in ["1", "2", "3", "4", "5", "6"] {
            let (mut engine, mut storage) = EngineBuilder::new().build();
            if initiator {
                engine
                    .send_logon_request(&mut storage, SessionStart::Resume)
                    .unwrap();
                let _ = take_admin(&mut engine);
            }
            let bytes = test_helpers::logon_bytes_with_encrypt_method(1, method, false);
            let msg = Message::from_raw_message(raw_message(&bytes).unwrap().1).unwrap();
            assert_matches!(
                engine.on_input(msg, &mut storage).unwrap(),
                InputResult::Handled
            );
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::InvalidLogonState),
                "{method}"
            );
            assert!(!engine.is_logged_on());
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            let logout = take_admin(&mut engine);
            let expected =
                format!("Unsupported EncryptMethod(98) value {method}; only 0 is supported");
            assert_matches!(as_admin(&logout), AdminBase::Logout(l)
                if l.text.as_deref().is_some_and(|text| text.as_utf8() == expected));
            assert!(engine.take_admin_output().is_none());
        }
    }
}

#[test]
fn invalid_encrypt_method_keeps_parser_reasons() {
    for (method, expected) in [
        ("99", SessionRejectReasonBase::ValueIsIncorrect),
        ("-1", SessionRejectReasonBase::ValueIsIncorrect),
        (
            "9223372036854775808",
            SessionRejectReasonBase::ValueIsIncorrect,
        ),
        ("abc", SessionRejectReasonBase::IncorrectDataFormatForValue),
        ("", SessionRejectReasonBase::TagSpecifiedWithoutAValue),
        ("0|98=0", SessionRejectReasonBase::TagAppearsMoreThanOnce),
    ] {
        let (mut engine, mut storage) = EngineBuilder::new().build();
        let bytes = test_helpers::logon_bytes_with_encrypt_method(1, method, false);
        let error = Message::from_raw_message(raw_message(&bytes).unwrap().1).unwrap_err();
        engine.on_deserialize_error(error, &mut storage).unwrap();
        let reject = take_admin(&mut engine);
        assert_matches!(as_admin(&reject), AdminBase::Reject(r)
            if r.session_reject_reason == Some(expected.into()) && r.ref_tag_id == Some(98)
                && r.ref_seq_num == 1 && r.ref_msg_type.as_deref() == Some(fix_str!("A")));
        assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
        assert!(engine.take_admin_output().is_none());
        assert_eq!(
            engine.disconnect_reason(),
            Some(DisconnectReason::InvalidLogonState)
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    }
}

#[test]
fn invalid_encrypt_method_does_not_override_header_verdict() {
    for method in ["1", "99"] {
        let (mut engine, mut storage) = EngineBuilder::new().build();
        storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
        let bytes = test_helpers::logon_bytes_with_encrypt_method(1, method, false);
        match Message::from_raw_message(raw_message(&bytes).unwrap().1) {
            Ok(msg) => {
                engine.on_input(msg, &mut storage).unwrap();
            }
            Err(error) => {
                engine.on_deserialize_error(error, &mut storage).unwrap();
            }
        }
        assert_eq!(
            engine.disconnect_reason(),
            Some(DisconnectReason::MsgSeqNumTooLow)
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 5);
        assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
        assert!(engine.take_admin_output().is_none());
    }
}

#[tokio::test]
async fn unsupported_encrypt_method_reset_preserves_history() {
    for running in [false, true] {
        for method in ["1", "99"] {
            let builder = EngineBuilder::new()
                .accept_reset_on_connect(true)
                .accept_reset_in_session(true);
            let (mut engine, mut storage) = if running {
                builder.logged_on().build()
            } else {
                builder.build()
            };
            let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
            let _ = engine.take_pending();
            storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
            let bytes = test_helpers::logon_bytes_with_encrypt_method(1, method, true);
            match Message::from_raw_message(raw_message(&bytes).unwrap().1) {
                Ok(msg) => {
                    engine.on_input(msg, &mut storage).unwrap();
                }
                Err(error) => {
                    engine.on_deserialize_error(error, &mut storage).unwrap();
                }
            }
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::InvalidLogonState)
            );
            assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
            assert_eq!(
                storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                history.as_slice()
            );
            if running && method == "99" {
                // A failed body cannot establish ResetSeqNumFlag=Y. The
                // existing logon-state gate wins before the body verdict.
                assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                assert!(engine.take_admin_output().is_none());
                continue;
            }
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            if method == "99" {
                let reject = take_admin(&mut engine);
                assert_matches!(as_admin(&reject), AdminBase::Reject(r)
                    if r.session_reject_reason == Some(SessionRejectReasonBase::ValueIsIncorrect.into()));
            }
            assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
            assert!(engine.take_admin_output().is_none());
        }
    }
}

#[test]
fn unsupported_encrypt_method_respects_reset_acknowledgement_boundary() {
    for method in ["1", "99"] {
        let (mut engine, mut storage) = reset_waiting_engine(false);
        let bytes = test_helpers::logon_bytes_with_encrypt_method(1, method, true);
        match Message::from_raw_message(raw_message(&bytes).unwrap().1) {
            Ok(msg) => {
                engine.on_input(msg, &mut storage).unwrap();
            }
            Err(error) => {
                engine.on_deserialize_error(error, &mut storage).unwrap();
            }
        }
        // Only a decoded, valid acknowledgement confirms the previous reset.
        let confirmed = method == "1";
        assert_eq!(engine.state.local_reset_unconfirmed, !confirmed);
        assert_eq!(
            engine.disconnect_reason(),
            Some(if confirmed {
                DisconnectReason::InvalidLogonState
            } else {
                DisconnectReason::SeqNumResetFailed
            })
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        if !confirmed {
            let reject = take_admin(&mut engine);
            assert_matches!(as_admin(&reject), AdminBase::Reject(r)
                if r.session_reject_reason == Some(SessionRejectReasonBase::ValueIsIncorrect.into()));
        }
        assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
        assert!(engine.take_admin_output().is_none());
    }
}

#[test]
fn send_logon_response_produces_logon_and_sets_logon_sent() {
    let (mut engine, _store) = EngineBuilder::new().build();
    engine.send_logon_response(30, false, None);
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Logon);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.heart_bt_int, 30);
    assert!(logon.reset_seq_num_flag.is_none());
    // The emitted Logon must carry the mandatory EncryptMethod(98)=0.
    assert_eq!(logon.encrypt_method, EncryptMethodBase::None);
}

#[test]
fn outgoing_logon_request_carries_configured_default_appl_ver_id() {
    let (mut engine, mut store) = EngineBuilder::new()
        .sender_default_appl_ver_id(ApplVerId::Fix50)
        .build();
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.default_appl_ver_id, Some(ApplVerId::Fix50));
}

/// Wire-compatibility pin: unconfigured default settings must keep
/// emitting `1137=9` (FIX50SP2), exactly as before the ApplVerId retyping.
#[test]
fn default_settings_emit_wire_compatible_appl_ver_id() {
    let (mut engine, mut store) = EngineBuilder::new().build(); // no override
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.default_appl_ver_id, Some(ApplVerId::Fix50Sp2));
    assert_eq!(logon.default_appl_ver_id.unwrap().as_bytes(), b"9");
}

#[test]
fn outgoing_logon_response_carries_configured_default_appl_ver_id() {
    let (mut engine, _store) = EngineBuilder::new()
        .sender_default_appl_ver_id(ApplVerId::Fix50)
        .build();
    engine.send_logon_response(30, false, None);
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.default_appl_ver_id, Some(ApplVerId::Fix50));
}

#[test]
fn send_logon_response_with_reset_flag() {
    let (mut engine, _store) = EngineBuilder::new().build();
    engine.send_logon_response(30, true, None);
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.reset_seq_num_flag, Some(true));
}

#[test]
fn send_logon_response_with_next_expected_seq_num() {
    let (mut engine, _store) = EngineBuilder::new().build();
    engine.send_logon_response(30, false, Some(5));
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.next_expected_msg_seq_num, Some(5));
}

#[test]
fn on_logon_acceptor() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());

    // Logon response in admin_output
    let logon_msg = take_admin(&mut engine);
    assert_msg_type(&logon_msg, MsgTypeBase::Logon);
}

/// The session never judges the peer's `MaxMessageSize<383>` - not even a
/// value far above our own limit. Tag 383 states the *sender's* receive
/// capacity, so a peer able to take in more than us breaks nothing, and
/// whether any value is acceptable is the application's call (it gets the
/// Logon in `on_admin_msg_in`, ahead of the state transition). Pinned so
/// the check is not "restored" later.
#[test]
fn on_logon_peer_max_message_size_is_not_judged() {
    let (mut engine, mut storage) = EngineBuilder::new().max_message_size(limit(4096)).build();

    let msg = test_helpers::logon_with_max_message_size(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        u16::MAX,
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );
    assert!(engine.is_logged_on());
    assert!(!engine.should_disconnect());
}

/// A peer that states no limit is equally unremarkable - our own limit is
/// technical and stands regardless, it is simply never spoken of.
#[test]
fn on_logon_without_peer_max_message_size_is_accepted() {
    let (mut engine, mut storage) = EngineBuilder::new().max_message_size(limit(4096)).build();

    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );
    assert!(engine.is_logged_on());
}

/// The session states its own limit unconditionally - there is no setting
/// that silences tag 383, only a dictionary without the slot.
#[test]
fn send_logon_request_advertises_max_message_size() {
    let (mut engine, mut store) = EngineBuilder::new().max_message_size(limit(8192)).build();
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.max_message_size, Some(8192));
}

/// The acceptor states its own limit too - Section 4.3.6 has both peers specify
/// the maximum they support.
#[test]
fn acceptor_logon_response_advertises_max_message_size() {
    let (mut engine, mut storage) = EngineBuilder::new().max_message_size(limit(4096)).build();

    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );

    let response = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&response) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.max_message_size, Some(4096));
}

/// The acceptor adopts and echoes the *initiator's* HeartBtInt
/// and does not consult its own `heartbeat_interval` setting - whether the
/// offered value is acceptable is the application's call (`on_admin_msg_in` /
/// `on_admin_msg_out`). The setting is `Some(30)` yet the initiator offers 45,
/// which must be echoed.
#[test]
fn on_logon_acceptor_echoes_initiator_heart_bt_int() {
    // Default settings configure heartbeat_interval = Some(30).
    let (mut engine, mut storage) = EngineBuilder::new().build();

    let msg =
        test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 45, None, None);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());

    let logon_resp = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&logon_resp) else {
        panic!("expected Logon");
    };
    assert_eq!(
        logon.heart_bt_int, 45,
        "acceptor must echo the initiator's HeartBtInt, ignoring its own setting"
    );
}

/// A structurally invalid (negative) HeartBtInt is an invalid field value -
/// answered per Test Cases Section 4.4.1 Scenario 1S(d): the optional
/// `Reject(35=3)` with `SessionRejectReason=ValueIsIncorrect`, then the
/// mandatory `Logout(35=5)` with `Text(58)` referencing the error, then
/// disconnect. This is a session-layer concern, distinct from *policy*
/// refusal (acceptable-or-not), which is the application's via
/// `on_admin_msg_in`.
#[test]
fn on_logon_acceptor_negative_heart_bt_int_rejected() {
    let (mut engine, mut storage) = EngineBuilder::new().build();

    let msg =
        test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), -1, None, None);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.should_disconnect());

    let out = take_admin(&mut engine);
    assert_msg_type(&out, MsgTypeBase::Reject);
    let AdminBase::Reject(ref rj) = as_admin(&out) else {
        panic!("expected Reject");
    };
    assert_eq!(
        rj.session_reject_reason.expect("reject reason must be set"),
        SessionRejectReasonBase::ValueIsIncorrect
    );
    assert_eq!(rj.ref_tag_id, Some(108)); // HeartBtInt

    let out = take_admin(&mut engine);
    assert_msg_type(&out, MsgTypeBase::Logout);
    let AdminBase::Logout(ref lo) = as_admin(&out) else {
        panic!("expected Logout");
    };
    assert_eq!(
        lo.text.as_deref(),
        Some(fix_str!("Invalid HeartBtInt(108)"))
    );
}

/// The same Scenario 1S(d) shape for a Logon refused by the header checks
/// rather than by a body field: the Logout is not optional and must carry
/// the `Text(58)` that says why.
#[test]
fn on_logon_bad_comp_id_rejects_then_logs_out_with_text() {
    let (mut engine, mut storage) = EngineBuilder::new().build();

    let msg = test_helpers::logon(1, fix_str!("WRONG"), fix_str!("SENDER"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.should_disconnect());

    let reject = take_admin(&mut engine);
    assert_msg_type(&reject, MsgTypeBase::Reject);
    let AdminBase::Reject(ref rj) = as_admin(&reject) else {
        panic!("expected Reject");
    };
    let reject_text = rj.text.clone().expect("Reject must carry Text");

    let out = take_admin(&mut engine);
    assert_msg_type(&out, MsgTypeBase::Logout);
    let AdminBase::Logout(ref lo) = as_admin(&out) else {
        panic!("expected Logout");
    };
    assert_eq!(lo.text, Some(reject_text));
}

/// Initiator proposes its configured HeartBtInt (FIX Transport Section 5.1).
#[test]
fn send_logon_request_heart_bt_int_from_settings() {
    // Default settings configure Some(30).
    let (mut engine, mut store) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.heart_bt_int, 30);
}

/// With no configured interval the initiator proposes HeartBtInt=0, i.e. no
/// heartbeats (FIX Transport Section 5.1).
#[test]
fn send_logon_request_no_heartbeat_interval_proposes_zero() {
    let (mut engine, mut store) = EngineBuilder::new().heartbeat_interval(None).build();
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.heart_bt_int, 0);
}

/// HeartBtInt=0 is valid and disables regular heartbeats (FIX Transport Section 5.1).
/// The acceptor adopts it, echoes 0, stays logged on, and reports heartbeats as
/// disabled so the IO loop parks its deadlines.
#[test]
fn on_logon_acceptor_zero_heart_bt_int_disables_heartbeats() {
    let (mut engine, mut storage) = EngineBuilder::new().build();

    let msg =
        test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, None, None);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());
    assert!(engine.heartbeat_interval().is_none());

    let logon_resp = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&logon_resp) else {
        panic!("expected Logon");
    };
    assert_eq!(
        logon.heart_bt_int, 0,
        "acceptor echoes the disabled (0) interval"
    );
}

/// `heartbeat_interval()` is `Some` for a positive configured interval.
#[test]
fn heartbeat_interval_some_when_configured() {
    let (engine, _store) = EngineBuilder::new().build();
    assert!(engine.heartbeat_interval().is_some());
}

/// `heartbeat_interval()` is `None` when the configured interval is `None`
/// (heartbeats disabled).
#[test]
fn heartbeat_interval_none_when_disabled() {
    let (engine, _store) = EngineBuilder::new().heartbeat_interval(None).build();
    assert!(engine.heartbeat_interval().is_none());
}

#[test]
fn on_logon_initiator_response() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    // Initiator sends logon first
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    let _ = take_admin(&mut engine); // drain the outgoing Logon

    // Receive logon response
    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());

    // No additional Logon in admin_output (initiator doesn't send response)
    assert!(engine.take_admin_output().is_none());
}

/// An invalid Logon acknowledgement is refused with Logout and disconnect
/// (FIX Session Layer Test Cases Scenario 1B(d)); both peers must share the
/// HeartBtInt value within the connection (Session Layer Section 4.3.4).
#[test]
fn on_logon_initiator_refuses_an_ack_that_does_not_echo_its_heart_bt_int() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .heartbeat_interval(NonZeroU16::new(30))
        .build();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    let _ = take_admin(&mut engine);

    let msg =
        test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 5, None, None);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(!engine.is_logged_on());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    let logout = take_admin(&mut engine);
    assert_matches!(as_admin(&logout), AdminBase::Logout(logout)
        if logout.text.as_deref() == Some(fix_str!("HeartBtInt(108) not echoed: expected 30, got 5")));
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);

    assert_eq!(
        engine.heartbeat_interval(),
        Some(Duration::from_secs(30)),
        "the proposed interval stays in force; the peer's counter-proposal has \
         no standing"
    );
}

#[test]
fn on_logon_wrong_comp_id() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let msg = test_helpers::logon(1, fix_str!("WRONG"), fix_str!("SENDER"));
    let result = engine.on_input(msg, &mut storage).unwrap();
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidCompId)
    );
    assert!(engine.should_disconnect());
}

#[test]
fn on_logon_too_high_seq() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    // msg seq=5 but target expects 1
    let msg = test_helpers::logon(5, fix_str!("TARGET"), fix_str!("SENDER"));
    let result = accept_input(&mut engine, msg, &mut storage);
    // Validation passes (too-high check is skipped for Logon), so the
    // app callback fires and Accept is fed back here. process_logon
    // handles the too-high case via Enqueue -> apply_result -> Handled,
    // and the message lands in the out-of-order queue.
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());
    assert_eq!(engine.queued_count(), 1);
    // Target seq NOT incremented (handled later by next_queued_message)
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);

    // Logon response + ResendRequest in admin_output
    let logon_resp = take_admin(&mut engine);
    assert_msg_type(&logon_resp, MsgTypeBase::Logon);
    let rr_msg = take_admin(&mut engine);
    assert_msg_type(&rr_msg, MsgTypeBase::ResendRequest);
}

/// A Logon whose MsgSeqNum is too low (below NextNumIn, no reset, no PossDup)
/// is a fatal desync - Logout + disconnect, no Logon ack.
#[test]
fn on_logon_too_low_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // Logon seq 1 << expected 5; no ResetSeqNumFlag, no PossDup.
    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::MsgSeqNumTooLow)
    );
    assert!(engine.should_disconnect());
    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    // No Logon ack is sent on the abort path.
    assert!(engine.take_admin_output().is_none());
}

/// Scenario 2(k): an in-sequence Logon with a CompID mismatch is rejected
/// with `CompIDProblem`, NextNumIn is INCREMENTED (step 2 of 2(k);
/// Session Layer 4.5.4 has no Logon exception), then Logout + disconnect.
#[test]
fn on_logon_comp_id_mismatch_rejects_increments_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let msg = test_helpers::logon(1, fix_str!("EVIL"), fix_str!("SENDER"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidCompId)
    );
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    let AdminBase::Reject(ref reject) = as_admin(&reject_msg) else {
        panic!("expected Reject");
    };
    assert_eq!(
        reject.session_reject_reason,
        Some(SessionRejectReasonBase::CompIdProblem.into())
    );
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    // Scenario 2(k) step 2: the rejected in-sequence Logon consumed its
    // seq num.
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// An in-sequence Logon rejected BY THE APPLICATION (InputAction::Reject)
/// must also increment NextNumIn - Session Layer 4.5.4: "Rejected messages
/// must be logged and NextNumIn incremented by 1", with no Logon
/// exception. `process_logon` never ran on this path, so nothing else
/// consumes the seq num.
#[test]
fn app_rejected_logon_increments_next_num_in() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    let result = engine.on_input(msg, &mut storage).unwrap();
    let InputResult::AdminMsg(msg) = result else {
        panic!("expected AdminMsg, got {result:?}");
    };
    let result = engine
        .process_admin_input(
            msg,
            InputAction::Reject {
                reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
                text: Some(fix_str!("rejected by application").to_owned()),
                tag: None,
            },
            &mut storage,
        )
        .unwrap();
    assert_matches!(result, InputResult::Handled);
    let reject_msg = take_admin(&mut engine);
    assert_msg_type(&reject_msg, MsgTypeBase::Reject);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

#[tokio::test]
async fn silently_refused_logons_preserve_storage_and_allow_reconnect() {
    for next_target in [1, 40] {
        for (seq_num, reset) in [(next_target, false), (next_target + 3, false), (1, true)] {
            let (mut engine, mut storage) = EngineBuilder::new().build();
            let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
            let _ = engine.take_pending();
            storage.set_next_sender_msg_seq_num(nz_seq(20)).unwrap();
            storage
                .set_next_target_msg_seq_num(nz_seq(next_target))
                .unwrap();

            for _ in 0..3 {
                let (mut engine, _) = EngineBuilder::new().accept_reset_on_connect(true).build();
                let msg = test_helpers::logon_with_options(
                    seq_num,
                    fix_str!("TARGET"),
                    fix_str!("SENDER"),
                    30,
                    Some(reset),
                    None,
                );
                let InputResult::AdminMsg(msg) = engine.on_input(msg, &mut storage).unwrap() else {
                    panic!("expected Logon callback");
                };
                assert_matches!(
                    engine
                        .process_admin_input(msg, InputAction::Disconnect, &mut storage)
                        .unwrap(),
                    InputResult::Handled
                );
                assert_eq!(
                    engine.disconnect_reason(),
                    Some(DisconnectReason::ApplicationForcedDisconnect)
                );
                assert!(!engine.is_logged_on());
                assert!(engine.take_admin_output().is_none());
                assert!(engine.take_pending().is_none());
                assert_eq!(storage.next_target_msg_seq_num().get(), next_target);
                assert_eq!(storage.next_sender_msg_seq_num().get(), 20);
                assert_eq!(
                    storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
                    history.as_slice()
                );
            }

            let (mut engine, _) = EngineBuilder::new().build();
            accept_input(
                &mut engine,
                test_helpers::logon(next_target, fix_str!("TARGET"), fix_str!("SENDER")),
                &mut storage,
            );
            assert!(engine.is_logged_on());
            assert!(!engine.should_disconnect());
            assert_eq!(storage.next_target_msg_seq_num().get(), next_target + 1);
            assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logon);
            assert!(engine.take_admin_output().is_none());
        }
    }
}

/// An initiator that has sent its Logon and receives a Logon-ack with a
/// too-high MsgSeqNum accepts the Logon, then issues a ResendRequest to
/// recover the gap - and (unlike the acceptor) sends no Logon response.
#[test]
fn on_logon_initiator_too_high_response_triggers_resend() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    let _ = take_admin(&mut engine); // drain the outgoing Logon

    // Logon response with seq 5 while we expect 1 -> too high.
    let msg = test_helpers::logon(5, fix_str!("TARGET"), fix_str!("SENDER"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());
    assert_eq!(engine.queued_count(), 1);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);

    // Initiator emits a ResendRequest and NO Logon response.
    let rr = take_admin(&mut engine);
    assert_msg_type(&rr, MsgTypeBase::ResendRequest);
    assert!(engine.take_admin_output().is_none());
}

/// An invalid HeartBtInt must not reset storage; its Reject consumes the
/// expected number (FIX Session Layer Test Cases Scenario 14(e)).
#[tokio::test]
async fn on_logon_invalid_heart_bt_int_preserves_history_and_consumes_sequence() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
    let _ = engine.take_pending();
    storage.set_next_sender_msg_seq_num(nz_seq(5000)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(5000)).unwrap();

    let msg = test_helpers::logon_with_options(
        5000,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        -1,
        None,
        None,
    );
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Reject);
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
    assert!(engine.take_admin_output().is_none());
    assert_eq!(storage.next_sender_msg_seq_num().get(), 5000);
    assert_eq!(storage.next_target_msg_seq_num().get(), 5001);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        history.as_slice()
    );
}

#[test]
fn on_logon_with_tag_789() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .enable_next_expected_msg_seq_num()
        .build();

    // Set sender seq higher - peer says it expects seq 1 but we're at 5
    storage.set_next_sender_msg_seq_num(nz_seq(5)).unwrap();

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        None,
        Some(1), // NextExpectedMsgSeqNum=1
    );
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());

    // Implicit resend range pushed to pending_resends
    assert!(engine.has_pending_resends());
    let range = engine.take_pending_resend().unwrap();
    assert_eq!(range, 1..=4); // from 1 to next_sender-1=4
}

/// EP124: a Logon whose `NextExpectedMsgSeqNum(789)` is GREATER than our
/// `NextNumOut` claims the peer has seen messages we never sent - fatal.
/// The session sends `Logout(SessionStatus=ReceivedNextExpectedMsgSeqNumTooHigh)`
/// and disconnects.
#[test]
fn on_logon_tag_789_too_high_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .enable_next_expected_msg_seq_num()
        .build();

    // We have only sent up to seq 2 (next_sender=3); the peer claims to expect
    // seq 5 (NextExpectedMsgSeqNum=5 > next_sender=3) - impossible.
    storage.set_next_sender_msg_seq_num(nz_seq(3)).unwrap();
    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        None,
        Some(5),
    );
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.should_disconnect());

    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    let AdminBase::Logout(lo) = as_admin(&logout) else {
        panic!("expected Logout");
    };
    assert_eq!(
        lo.session_status,
        Some(SessionStatusBase::ReceivedNextExpectedMsgSeqNumTooHigh.into())
    );
}

/// `NextExpectedMsgSeqNum(789)=0` names no message - sequence numbers start
/// at 1 (Section 4.1) - so the Logon is invalid: Logout + disconnect (Testcases
/// Section 4.4.1 Scenario 1S(d)). Left through, step 6 would queue an implicit
/// resend from 0 and emit a gap-fill stamped `MsgSeqNum(34)=0`.
#[test]
fn on_logon_tag_789_zero_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .enable_next_expected_msg_seq_num()
        .build();

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        None,
        Some(0),
    );
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.should_disconnect());

    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    // No SessionStatus fits a zero 789 - the too-high code would lie.
    let AdminBase::Logout(lo) = as_admin(&logout) else {
        panic!("expected Logout");
    };
    assert_eq!(lo.session_status, None);

    // Nothing may reach the resend queue.
    assert!(!engine.has_pending_resends());
}

/// The logon deadline is armed exactly while the handshake is incomplete, so
/// the IO loop arms and disarms its timer off the state transition alone. It
/// covers `Idle` as well as `LogonSent`: an acceptor whose application answers
/// the first Logon with `InputAction::Reject` stays in `Idle` and enters the
/// session loop with no other deadline available to it.
#[test]
fn logon_deadline_covers_every_incomplete_handshake() {
    let (mut engine, mut store) = EngineBuilder::new().build();
    let idle_deadline = engine.logon_deadline().expect("armed while Idle");

    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    assert_eq!(
        engine.logon_deadline(),
        Some(idle_deadline),
        "the budget is anchored on the session start, so sending the Logon does not extend it"
    );

    engine.set_logged_on();
    assert!(engine.logon_deadline().is_none());
}

/// A budget the clock cannot add to its anchor means "no limit", so the
/// deadline is simply not armed. Falling back to the anchor instead would
/// leave a deadline in the past and end the session on the first poll.
#[test]
fn logon_deadline_is_unarmed_when_the_budget_overflows_the_clock() {
    let (engine, _store) = EngineBuilder::new()
        .auto_disconnect_after_no_logon_response(Duration::MAX)
        .build();
    assert!(engine.logon_deadline().is_none());
}

#[test]
fn on_logon_timeout_sets_disconnect_no_messages() {
    let (mut engine, mut store) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let _ = take_admin(&mut engine);

    engine.on_logon_timeout();

    assert!(engine.should_disconnect());
    // No Logout<5>: the handshake never completed, so there is no session to
    // end and no peer that agreed to read anything we number.
    assert!(engine.take_admin_output().is_none());
}
