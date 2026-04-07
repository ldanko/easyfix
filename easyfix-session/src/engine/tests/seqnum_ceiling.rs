use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::{SeqNum, TimePrecision, UtcTimestamp},
    deserializer::raw_message,
    fix_str,
    message::{HeaderAccess, SessionMessage},
};
use easyfix_test_messages::Message;

use super::support::{assert_msg_type, verify_test};
use crate::{
    application::DisconnectReason,
    engine::{InputResult, PendingOutput, VerifyError},
    initiator::SessionStart,
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{
        EngineBuilder, accept_input, as_admin, drain_all_admin, nz_seq, take_admin,
        timestamp_offset_secs,
    },
};

#[test]
fn last_usable_seq_num_is_accepted_and_exhausts_incoming_numbering() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();

    let result = accept_input(
        &mut engine,
        test_helpers::heartbeat(SeqNum::MAX - 1, None),
        &mut storage,
    );

    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), SeqNum::MAX);
    // The reaction belongs to the IO loop's dispatch tail, not to the handler.
    assert!(!engine.should_disconnect());
}

// Test Cases Scenarios 10(b) and 11(a): apply NewSeqNo when valid. Ending
// at MAX is the engine's limit policy, after the protocol update is applied.
#[test]
fn sequence_reset_to_max_applies_then_exhausts_incoming_numbering() {
    for (seq_num, gap_fill) in [(5, true), (99, false)] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();

        let result = accept_input(
            &mut engine,
            test_helpers::sequence_reset(seq_num, SeqNum::MAX, gap_fill),
            &mut storage,
        );

        assert_matches!(result, InputResult::Handled);
        assert_eq!(storage.next_target_msg_seq_num().get(), SeqNum::MAX);
        assert!(!engine.should_disconnect());
        assert!(engine.take_admin_output().is_none());

        engine.end_session_if_target_numbering_exhausted(&storage);

        assert_eq!(
            engine.disconnect_reason(),
            Some(DisconnectReason::SeqNumExhausted)
        );
        assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
        assert!(engine.take_admin_output().is_none());
    }
}

// Test Cases Scenario 10(a): fill the preceding gap before applying NewSeqNo.
#[test]
fn gap_fill_to_max_above_gap_does_not_exhaust_incoming_numbering() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();

    let result = accept_input(
        &mut engine,
        test_helpers::sequence_reset(10, SeqNum::MAX, true),
        &mut storage,
    );
    engine.end_session_if_target_numbering_exhausted(&storage);

    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
    assert!(!engine.should_disconnect());
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::ResendRequest);
    assert!(engine.take_admin_output().is_none());
}

// Test Cases Scenario 10(c): ignore a duplicate even when NewSeqNo is MAX.
#[test]
fn duplicate_gap_fill_to_max_does_not_exhaust_incoming_numbering() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let mut msg = test_helpers::sequence_reset(3, SeqNum::MAX, true);
    msg.set_poss_dup_flag(Some(true));
    msg.set_orig_sending_time(Some(timestamp_offset_secs(-10)));

    let result = accept_input(&mut engine, msg, &mut storage);
    engine.end_session_if_target_numbering_exhausted(&storage);

    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
    assert!(!engine.should_disconnect());
    assert!(engine.take_admin_output().is_none());
}

/// Whatever the state, exhausted incoming numbering ends the session with
/// exactly one `Logout<5>`. A handshake that never completed still gets one
/// when this method is the one ending it - Test Cases Section 4.4.1 Scenario 1S(d)
/// step 3 for the acceptor, Section 4.3.1 Scenario 1B(d) step 3 for the initiator.
/// The state is not what decides; a Logout already on the wire is.
///
/// That farewell must not promote a half-open handshake to "logged on":
/// `LogoutSent` makes `is_logged_on()` true, which is what the IO loop gates
/// its terminating app-send drain on - and an application message ahead of an
/// unacknowledged `Logon<A>` violates Session Layer Section 4.3.10.
#[test]
fn exhausted_incoming_numbering_ends_the_session_with_one_logout_in_every_state() {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum State {
        Idle,
        LogonSent,
        Established,
    }
    for state in [State::Idle, State::LogonSent, State::Established] {
        let builder = EngineBuilder::new();
        let (mut engine, mut storage) = if state == State::Established {
            builder.logged_on().build()
        } else {
            builder.build()
        };
        storage
            .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
            .unwrap();
        if state == State::LogonSent {
            engine
                .send_logon_request(&mut storage, SessionStart::Resume)
                .unwrap();
            drain_all_admin(&mut engine);
        }
        let logged_on = state == State::Established;
        assert_eq!(engine.is_logged_on(), logged_on);

        engine.end_session_if_target_numbering_exhausted(&storage);

        assert_eq!(
            engine.disconnect_reason(),
            Some(DisconnectReason::SeqNumExhausted),
            "{state:?}"
        );
        let logout = engine
            .take_admin_output()
            .unwrap_or_else(|| panic!("expected a Logout ({state:?})"));
        assert_msg_type(&logout, MsgTypeBase::Logout);
        assert!(
            engine.take_admin_output().is_none(),
            "exactly one Logout ({state:?})"
        );
        assert_eq!(
            engine.is_logged_on(),
            logged_on,
            "the farewell must not change the logon state ({state:?})"
        );
    }
}

#[test]
fn exhausted_incoming_numbering_sends_no_second_logout() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();
    // A Logout is already on its way out.
    engine.send_logout(None, None);
    drain_all_admin(&mut engine);

    engine.end_session_if_target_numbering_exhausted(&storage);

    assert!(engine.should_disconnect());
    assert!(
        engine.take_admin_output().is_none(),
        "LogoutSent must not produce a second Logout"
    );
}

#[test]
fn exhausted_incoming_numbering_leaves_an_already_decided_disconnect_alone() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();
    engine.on_control(ControlMsg::Disconnect);
    assert!(engine.should_disconnect());

    engine.end_session_if_target_numbering_exhausted(&storage);

    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::Disconnected)
    );
    assert!(engine.take_admin_output().is_none());
}

/// A persisted MAX counter prevents resuming exhausted numbering.
#[test]
fn reconnect_on_exhausted_incoming_numbering_logs_out() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();

    let result = accept_input(
        &mut engine,
        test_helpers::logon(SeqNum::MAX, fix_str!("TARGET"), fix_str!("SENDER")),
        &mut storage,
    );

    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
    let msg = take_admin(&mut engine);
    assert!(engine.take_admin_output().is_none());
    let AdminBase::Logout(logout) = as_admin(&msg) else {
        panic!("expected Logout");
    };
    let text = logout.text.expect("Logout carries Text(58)");
    assert!(
        !text.as_utf8().contains(&SeqNum::MAX.to_string()),
        "the counter still reads MAX, so quoting it says nothing: {text}"
    );
}

#[test]
fn permitted_logon_reset_reopens_exhausted_counters() {
    let (mut engine, mut storage) = EngineBuilder::new().accept_reset_on_connect(true).build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();
    let logon = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );

    let result = accept_input(&mut engine, logon, &mut storage);
    engine.end_session_if_target_numbering_exhausted(&storage);

    assert_matches!(result, InputResult::Handled);
    assert!(!engine.should_disconnect());
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    let mut ack = take_admin(&mut engine);
    assert_matches!(as_admin(&ack), AdminBase::Logon(logon) if logon.reset_seq_num_flag == Some(true));
    assert!(engine.fill_header(&mut ack, &mut storage).unwrap());
    assert_eq!(ack.msg_seq_num(), 1);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 2);
}

#[test]
fn exhausted_incoming_numbering_refuses_duplicates() {
    let (engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();

    let mut msg = test_helpers::heartbeat(SeqNum::MAX, None);
    msg.set_sending_time(UtcTimestamp::now(TimePrecision::Nanos));
    msg.set_poss_dup_flag(Some(true));
    msg.set_orig_sending_time(Some(timestamp_offset_secs(-10)));

    let result = verify_test(&engine, &msg, &storage, true, true);

    assert_matches!(result, Err(VerifyError::SeqNumExhausted));
}

#[test]
fn logon_ack_advertises_max_after_last_usable_incoming_number() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .enable_next_expected_msg_seq_num()
        .build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();

    let logon = test_helpers::logon_with_options(
        SeqNum::MAX - 1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        None,
        Some(1),
    );
    let result = accept_input(&mut engine, logon, &mut storage);
    assert_matches!(result, InputResult::Handled);

    let ack = take_admin(&mut engine);
    let AdminBase::Logon(logon_ack) = as_admin(&ack) else {
        panic!("expected Logon ACK");
    };
    assert_eq!(logon_ack.next_expected_msg_seq_num, Some(SeqNum::MAX));
}

#[test]
fn logon_request_omits_tag_789_when_incoming_numbering_is_exhausted() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .enable_next_expected_msg_seq_num()
        .build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();

    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();

    let msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.next_expected_msg_seq_num, None);
    assert_eq!(engine.state.next_expected_msg_seq_num, None);
}

#[test]
fn fill_header_stamps_last_usable_number_then_ends_the_session() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX - 2))
        .unwrap();

    let mut penultimate = test_helpers::heartbeat_with_empty_header();
    assert!(engine.fill_header(&mut penultimate, &mut storage).unwrap());
    assert_eq!(penultimate.msg_seq_num(), SeqNum::MAX - 2);
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX - 1);
    assert!(!engine.should_disconnect());

    let mut msg = test_helpers::heartbeat_with_empty_header();
    assert!(engine.fill_header(&mut msg, &mut storage).unwrap());

    assert_eq!(msg.msg_seq_num(), SeqNum::MAX - 1);
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);
    assert!(engine.should_disconnect());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
}

#[test]
fn fill_header_refuses_a_pre_numbered_message_once_outgoing_numbering_is_exhausted() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();

    let mut msg = test_helpers::heartbeat_with_empty_header();
    msg.set_msg_seq_num(42);
    assert!(!engine.fill_header(&mut msg, &mut storage).unwrap());

    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);
}

#[test]
fn fill_header_refuses_pre_numbered_max_with_unexhausted_counter() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let mut msg = test_helpers::heartbeat_with_empty_header();
    msg.set_msg_seq_num(SeqNum::MAX);

    assert!(!engine.fill_header(&mut msg, &mut storage).unwrap());

    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
}

#[test]
fn fill_header_refuses_to_stamp_once_outgoing_numbering_is_exhausted() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();

    let mut msg = test_helpers::heartbeat_with_empty_header();
    assert!(!engine.fill_header(&mut msg, &mut storage).unwrap());

    assert_eq!(msg.msg_seq_num(), 0, "no number was allocated");
    assert_eq!(
        msg.sending_time(),
        UtcTimestamp::MIN_UTC,
        "the refusal short-circuits before SendingTime"
    );
    // CompIDs are filled first and are not part of the refusal.
    assert_eq!(msg.sender_comp_id(), fix_str!("SENDER"));
    assert_eq!(msg.target_comp_id(), fix_str!("TARGET"));
    assert!(engine.should_disconnect());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
}

#[test]
fn resend_gap_fill_for_last_usable_number_announces_max() {
    let (mut engine, _storage) = EngineBuilder::new().logged_on().build();

    engine.accumulate_resend_gap(SeqNum::MAX - 1);
    engine.flush_resend_gap().unwrap();
    let PendingOutput::Transient { len } = engine.take_pending().unwrap() else {
        panic!("expected transient gap fill")
    };
    let (_, raw) = raw_message(&engine.scratch()[..len]).unwrap();
    let msg = Message::from_raw_message(raw).unwrap();

    assert_eq!(msg.msg_seq_num(), SeqNum::MAX - 1);
    assert_matches!(as_admin(&msg), AdminBase::SequenceReset(sr)
        if sr.gap_fill_flag == Some(true) && sr.new_seq_no == SeqNum::MAX);
    assert!(!engine.should_disconnect());
}

/// The one message that can use up the incoming numbering before the session
/// is established: a peer `Logout<5>` at `SeqNum::MAX - 1`, which `LogonSent`
/// admits. The Logout response is staged by the handler, so the peer is
/// answered rather than dropped on - the `Established` guard on the
/// exhaustion reaction never gets to suppress anything.
#[test]
fn peer_logout_at_last_usable_number_before_establishment_is_still_answered() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    drain_all_admin(&mut engine);

    let result = accept_input(
        &mut engine,
        test_helpers::logout(SeqNum::MAX - 1),
        &mut storage,
    );

    assert_matches!(result, InputResult::Handled);
    assert!(engine.awaiting_peer_close().is_some());
    assert_eq!(storage.next_target_msg_seq_num().get(), SeqNum::MAX);

    let response = take_admin(&mut engine);
    assert_msg_type(&response, MsgTypeBase::Logout);

    // The reaction finds the Logout already answered and adds nothing: the
    // connection is the peer's to close. The exhausted counter prevents a
    // subsequent reconnect from resuming the numbering.
    engine.end_session_if_target_numbering_exhausted(&storage);
    assert!(!engine.should_disconnect());
    assert!(
        engine.take_admin_output().is_none(),
        "no second Logout, and no bare drop either"
    );
}

/// Scenario 1S(d): a malformed `Logon<A>` draws an optional `Reject<3>` and a
/// mandatory `Logout<5>` with `Text(58)` before the disconnect. The Reject
/// verdict from `check_poss_dup` carries no disconnect of its own, so when it
/// is the message that uses up the incoming numbering, the exhaustion reaction
/// is the only thing left to close the session - and it owes the peer that
/// Logout.
#[test]
fn malformed_logon_that_exhausts_the_numbering_still_gets_a_logout() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    storage
        .set_next_target_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();

    // PossDupFlag=Y without OrigSendingTime(122) - RequiredTagMissing, and the
    // only Reject verdict that does not carry a disconnect.
    let mut logon = test_helpers::logon(SeqNum::MAX - 1, fix_str!("TARGET"), fix_str!("SENDER"));
    logon.set_poss_dup_flag(Some(true));

    let result = accept_input(&mut engine, logon, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        storage.next_target_msg_seq_num().get(),
        SeqNum::MAX,
        "the Reject consumed the last incoming sequence number"
    );

    engine.end_session_if_target_numbering_exhausted(&storage);

    let reject = take_admin(&mut engine);
    assert_msg_type(&reject, MsgTypeBase::Reject);
    let logout = engine
        .take_admin_output()
        .expect("Scenario 1S(d) step 3 mandates a Logout, not a bare close");
    assert_msg_type(&logout, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
}

/// Exhausting the numbering while stamping a farewell the session had already
/// decided to send is a consequence of the teardown, not its cause. Reporting
/// `SeqNumExhausted` would displace the reason that actually ended the session.
#[test]
fn exhaustion_while_stamping_a_farewell_does_not_displace_the_real_reason() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .logged_on()
        .auto_disconnect_after_no_heartbeat(1)
        .build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();

    // Grace period expires: a Logout is staged and the disconnect latched.
    engine.on_input_timeout();
    engine.on_input_timeout();
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::HeartbeatTimeout)
    );

    // Stamping that Logout consumes the last sequence number.
    let mut logout = loop {
        let msg = take_admin(&mut engine);
        if SessionMessage::msg_type(&*msg) == MsgTypeBase::Logout {
            break msg;
        }
    };
    assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
    assert_eq!(logout.msg_seq_num(), SeqNum::MAX - 1);
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);

    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::HeartbeatTimeout),
        "the heartbeat timeout ended this session, not the ceiling"
    );
}

/// The mirror image: the numbering runs out first, and only then does a peer
/// `Logout<5>` arrive. The exhaustion stands as the reason. It is also the
/// truthful one - our answering Logout cannot be numbered, so no logout
/// exchange takes place.
#[test]
fn a_peer_logout_after_exhaustion_does_not_displace_the_real_reason() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();

    // Stamping an ordinary Heartbeat consumes the last sequence number.
    engine.send_heartbeat(None);
    let mut heartbeat = take_admin(&mut engine);
    assert!(engine.fill_header(&mut heartbeat, &mut storage).unwrap());
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );

    let result = accept_input(&mut engine, test_helpers::logout(1), &mut storage);

    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted),
        "the ceiling ended this session before the peer's Logout was seen"
    );
    // The answering Logout is staged, but there is no number left for it.
    let mut logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    assert!(!engine.fill_header(&mut logout, &mut storage).unwrap());
}
