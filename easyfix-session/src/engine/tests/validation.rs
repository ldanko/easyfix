use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase, SessionStatusBase},
    basic_types::{TimePrecision, UtcTimestamp},
    fix_str,
    message::{HeaderAccess, SessionMessage},
};
use easyfix_test_messages::Message;
use tokio::time::Duration;

use super::support::{assert_msg_type, verify_test};
use crate::{
    application::DisconnectReason,
    engine::{InputResult, VerifyError},
    initiator::SessionStart,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{
        EngineBuilder, accept_input, as_admin, drain_all_admin, nz_seq, take_admin,
        timestamp_offset_secs,
    },
};

#[test]
fn verify_header_normal_seq_num() {
    let (engine, storage) = EngineBuilder::new().logged_on().build();
    // storage.next_target == 1, msg seq == 1 -> Ok
    let msg = test_helpers::heartbeat(1, None);
    let result = verify_test(&engine, &msg, &storage, true, true);
    assert_matches!(result, Ok(()));
}

#[test]
fn verify_header_too_high_check_enabled() {
    let (engine, storage) = EngineBuilder::new().logged_on().build();
    // storage.next_target == 1, msg seq == 5 -> TooHigh
    let msg = test_helpers::heartbeat(5, None);
    let result = verify_test(&engine, &msg, &storage, true, true);
    assert_matches!(result, Err(VerifyError::TooHigh));
    // `verify_header` is pure observation - queue insertion is the
    // dispatcher's job (`apply_result` on `HandlerResult::TooHigh`). The
    // end-to-end behavior is exercised in the `on_input_*` tests.
    assert_eq!(engine.queued_count(), 0);
}

#[test]
fn verify_header_too_high_check_disabled() {
    let (engine, storage) = EngineBuilder::new().logged_on().build();
    // check_too_high=false -> Ok even with high seq
    let msg = test_helpers::heartbeat(5, None);
    let result = verify_test(&engine, &msg, &storage, false, true);
    assert_matches!(result, Ok(()));
    assert_eq!(engine.queued_count(), 0);
}

#[test]
fn verify_header_too_low_no_poss_dup() {
    let (engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // msg seq == 1, no PossDupFlag -> TooLow
    let msg = test_helpers::heartbeat(1, None);
    let result = verify_test(&engine, &msg, &storage, true, true);
    assert_matches!(result, Err(VerifyError::TooLow { .. }));
}

#[test]
fn verify_header_too_low_check_disabled() {
    let (engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // check_too_low=false -> Ok even with low seq
    let msg = test_helpers::heartbeat(1, None);
    let result = verify_test(&engine, &msg, &storage, true, false);
    assert_matches!(result, Ok(()));
}

#[test]
fn verify_header_too_low_poss_dup_valid_orig_time() {
    let (engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let mut msg = test_helpers::heartbeat(1, None);
    // PossDupFlag=Y with OrigSendingTime <= SendingTime -> Duplicate
    let past = timestamp_offset_secs(-10);
    msg.set_sending_time(UtcTimestamp::now(TimePrecision::Nanos));
    msg.set_poss_dup_flag(Some(true));
    msg.set_orig_sending_time(Some(past));
    let result = verify_test(&engine, &msg, &storage, true, true);
    assert_matches!(result, Err(VerifyError::Duplicate));
}

#[test]
fn verify_header_too_low_poss_dup_missing_orig_time() {
    let (engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_poss_dup_flag(Some(true));
    // No OrigSendingTime set -> Reject with RequiredTagMissing
    let result = verify_test(&engine, &msg, &storage, true, true);
    match result {
        Err(VerifyError::Reject {
            reason,
            tag,
            disconnect,
            ..
        }) => {
            assert_eq!(reason, SessionRejectReasonBase::RequiredTagMissing);
            assert_eq!(tag, Some(122)); // TAG_ORIG_SENDING_TIME
            assert!(disconnect.is_none());
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[test]
fn verify_header_too_low_poss_dup_orig_time_after_sending() {
    let (engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let mut msg = test_helpers::heartbeat(1, None);
    let now = UtcTimestamp::now(TimePrecision::Nanos);
    let future = timestamp_offset_secs(10);
    msg.set_sending_time(now);
    msg.set_poss_dup_flag(Some(true));
    msg.set_orig_sending_time(Some(future)); // orig > sending -> invalid
    let result = verify_test(&engine, &msg, &storage, true, true);
    match result {
        Err(VerifyError::Reject {
            reason,
            tag,
            disconnect,
            ..
        }) => {
            assert_eq!(reason, SessionRejectReasonBase::SendingTimeAccuracyProblem);
            assert_eq!(tag, Some(122)); // TAG_ORIG_SENDING_TIME
            assert_eq!(disconnect, Some(DisconnectReason::InvalidOrigSendingTime));
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

/// Test Cases Section 4.5.1 Scenario 2(g) puts no sequence-number qualifier on
/// `PossDupFlag(43)=Y` without `OrigSendingTime(122)`: Reject with
/// `SessionRejectReason(373)=1` and increment NextNumIn. A message in
/// sequence never reached the too-low branch these checks used to live in.
#[test]
fn poss_dup_in_sequence_missing_orig_time_rejects_and_advances() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_poss_dup_flag(Some(true));

    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    let reject = take_admin(&mut engine);
    assert_msg_type(&reject, MsgTypeBase::Reject);
    let AdminBase::Reject(ref rej) = as_admin(&reject) else {
        panic!("expected Reject");
    };
    assert_eq!(
        rej.session_reject_reason,
        Some(SessionRejectReasonBase::RequiredTagMissing.into())
    );
    assert_eq!(rej.ref_tag_id, Some(122));
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(!engine.should_disconnect());
}

/// Scenario 2(f): `PossDupFlag(43)=Y` with `OrigSendingTime(122)` after
/// `SendingTime(52)` and MsgSeqNum as expected - Reject with `373=10` and
/// increment NextNumIn. The Logout + disconnect that follow are the
/// scenario's optional step 3.
#[test]
fn poss_dup_in_sequence_orig_time_after_sending_rejects_and_advances() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_sending_time(UtcTimestamp::now(TimePrecision::Nanos));
    msg.set_poss_dup_flag(Some(true));
    msg.set_orig_sending_time(Some(timestamp_offset_secs(10)));

    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidOrigSendingTime)
    );

    let reject = take_admin(&mut engine);
    assert_msg_type(&reject, MsgTypeBase::Reject);
    let AdminBase::Reject(ref rej) = as_admin(&reject) else {
        panic!("expected Reject");
    };
    assert_eq!(
        rej.session_reject_reason,
        Some(SessionRejectReasonBase::SendingTimeAccuracyProblem.into())
    );
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
}

/// The PossDup check must run AFTER the too-high check. A PossDup message
/// that opens a gap has to draw a ResendRequest: a Reject would leave the
/// gap unrecovered and could not even advance NextNumIn, the message not
/// being in sequence. It is revalidated in sequence when the queue drains.
#[test]
fn poss_dup_too_high_requests_resend_instead_of_rejecting() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let mut msg = test_helpers::heartbeat(5, None);
    msg.set_poss_dup_flag(Some(true));

    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::ResendRequest);
    assert!(engine.take_admin_output().is_none());
    assert_eq!(engine.queued_count(), 1);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
}

/// State matrix (FIX Session Layer Section 4.10): a too-low MsgSeqNum WITHOUT
/// PossDupFlag is a fatal desync - the dispatched message produces a
/// `Logout(SessionStatus=ReceivedMsgSeqNumTooLow)` + disconnect, and NextNumIn
/// is NOT advanced. The `verify_header_*` tests only pin the intermediate
/// `VerifyError::TooLow`; this pins the protocol effect.
#[test]
fn too_low_no_poss_dup_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    // seq 1 << 5, no PossDupFlag.
    let msg = test_helpers::heartbeat(1, None);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::MsgSeqNumTooLow)
    );
    assert!(engine.should_disconnect());

    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    let AdminBase::Logout(lo) = as_admin(&logout) else {
        panic!("expected Logout");
    };
    assert_eq!(
        lo.session_status,
        Some(SessionStatusBase::ReceivedMsgSeqNumTooLow.into())
    );
    // A too-low message must not advance NextNumIn.
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

/// Scenario 2(e): a too-low message WITH `PossDupFlag=Y` and a valid
/// `OrigSendingTime` is a benign duplicate - silently discarded (`Handled`),
/// not dispatched, no admin reply, and NextNumIn unchanged. The
/// `verify_header_*` test only pins the intermediate `VerifyError::Duplicate`.
#[test]
fn too_low_poss_dup_duplicate_is_discarded() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_sending_time(UtcTimestamp::now(TimePrecision::Nanos));
    msg.set_poss_dup_flag(Some(true));
    msg.set_orig_sending_time(Some(timestamp_offset_secs(-10)));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(!engine.should_disconnect());
    assert!(engine.take_admin_output().is_none());
    // A duplicate must not advance NextNumIn.
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
}

#[test]
fn verify_header_wrong_sender_comp_id() {
    let (engine, storage) = EngineBuilder::new().logged_on().build();
    // Incoming message sender should be "TARGET" (our target), use "WRONG"
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_sender_comp_id(fix_str!("WRONG").to_owned());
    let result = verify_test(&engine, &msg, &storage, true, true);
    match result {
        Err(VerifyError::Reject {
            reason,
            tag,
            disconnect,
            ..
        }) => {
            assert_eq!(reason, SessionRejectReasonBase::CompIdProblem);
            assert_eq!(tag, Some(49)); // TAG_SENDER_COMP_ID
            assert_eq!(disconnect, Some(DisconnectReason::InvalidCompId));
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[test]
fn verify_header_wrong_target_comp_id() {
    let (engine, storage) = EngineBuilder::new().logged_on().build();
    // Incoming message target should be "SENDER" (our sender), use "WRONG"
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_target_comp_id(fix_str!("WRONG").to_owned());
    let result = verify_test(&engine, &msg, &storage, true, true);
    match result {
        Err(VerifyError::Reject {
            reason,
            tag,
            disconnect,
            ..
        }) => {
            assert_eq!(reason, SessionRejectReasonBase::CompIdProblem);
            assert_eq!(tag, Some(56)); // TAG_TARGET_COMP_ID
            assert_eq!(disconnect, Some(DisconnectReason::InvalidCompId));
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

/// Scenario 14: an in-sequence message
/// with a wrong CompID is Rejected (`SessionRejectReason=CompIdProblem`),
/// followed by a Logout + disconnect (CompID problems are fatal), and the
/// in-sequence rejected message still advances NextNumIn. The `verify_header_*`
/// tests only pin the intermediate `VerifyError::Reject`.
#[test]
fn wrong_comp_id_rejects_then_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_sender_comp_id(fix_str!("WRONG").to_owned());
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidCompId)
    );
    assert!(engine.should_disconnect());

    let reject = take_admin(&mut engine);
    assert_msg_type(&reject, MsgTypeBase::Reject);
    let AdminBase::Reject(rj) = as_admin(&reject) else {
        panic!("expected Reject");
    };
    assert_eq!(
        rj.session_reject_reason,
        Some(SessionRejectReasonBase::CompIdProblem.into())
    );
    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
    // The in-sequence rejected message still advances NextNumIn.
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

#[test]
fn verify_header_stale_sending_time() {
    let (engine, storage) = EngineBuilder::new()
        .max_latency(Duration::from_secs(2))
        .logged_on()
        .build();
    let mut msg = test_helpers::heartbeat(1, None);
    // Set sending time 300 seconds in the past (well beyond 2s max_latency)
    msg.set_sending_time(timestamp_offset_secs(-300));
    let result = verify_test(&engine, &msg, &storage, true, true);
    match result {
        Err(VerifyError::Reject {
            reason,
            tag,
            disconnect,
            ..
        }) => {
            assert_eq!(reason, SessionRejectReasonBase::SendingTimeAccuracyProblem);
            assert_eq!(tag, Some(52)); // TAG_SENDING_TIME
            // FIX Session Layer Section 4.2.3 / Scenario 2(o): a SendingTime-accuracy
            // Reject(373=10) must be followed by a Logout + disconnect - the
            // VerifyError must carry a disconnect reason, mirroring the CompID
            // and OrigSendingTime reject paths.
            assert_eq!(
                disconnect,
                Some(DisconnectReason::SendingTimeAccuracyProblem)
            );
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

/// Wire-level pin for Scenario 2(o): an in-sequence message whose
/// SendingTime is outside `max_latency` must produce Reject(373=10) **followed
/// by** a Logout(35=5) and a disconnect, with NextNumIn advanced - not a bare
/// Reject that leaves the session established (FIX Session Layer Section 4.2.3).
#[test]
fn stale_sending_time_rejects_then_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .max_latency(Duration::from_secs(2))
        .logged_on()
        .build();

    // In-sequence Heartbeat (seq 1 == next_target) with a stale SendingTime.
    let mut msg = test_helpers::heartbeat(1, None);
    msg.set_sending_time(timestamp_offset_secs(-300));

    let result = engine.on_input(msg, &mut storage).unwrap();

    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SendingTimeAccuracyProblem)
    );
    assert!(engine.should_disconnect());
    // The rejected in-sequence message still advances NextNumIn.
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);

    // Reject(373=10) is emitted first, then the Logout(35=5).
    let reject = take_admin(&mut engine);
    assert_msg_type(&reject, MsgTypeBase::Reject);
    let AdminBase::Reject(ref rj) = as_admin(&reject) else {
        panic!("expected Reject");
    };
    assert_eq!(
        rj.session_reject_reason.expect("reject reason must be set"),
        SessionRejectReasonBase::SendingTimeAccuracyProblem
    );
    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
}

#[test]
fn verify_header_before_logon_not_logon_msg() {
    // Fresh engine (not logged on), non-Logon message -> InvalidLogonState
    let (engine, storage) = EngineBuilder::new().build();
    let msg = test_helpers::heartbeat(1, None);
    let result = verify_test(&engine, &msg, &storage, true, true);
    assert_matches!(result, Err(VerifyError::InvalidLogonState));
}

#[test]
fn verify_header_logon_before_logon() {
    // Fresh engine (not logged on), Logon message -> Ok
    let (engine, storage) = EngineBuilder::new().build();
    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    let result = verify_test(&engine, &msg, &storage, true, true);
    assert_matches!(result, Ok(()));
}

/// SequenceReset<4> and Reject<3> are not special-cased out of the logon
/// gate. FIX Session Layer Section 4.3.1 / Test Cases Section 4.4.2 Scenario 2S (acceptor,
/// `Idle`) and Test Cases Section 4.3.1 Scenario 1B(e) (initiator, `LogonSent`) put
/// every non-Logon frame on the disconnect path. The "process without regard
/// to MsgSeqNum" licence a SequenceReset-Reset enjoys (Section 4.8.8, Transport
/// Section 4.5) suspends the *sequence* checks only - which makes it the sharpest
/// case: as the first frame of a connection it would otherwise drive
/// `set_next_target_msg_seq_num` on a persisted store and leave the session
/// unusable across reconnects, and a Reject would bump NextNumIn for a peer
/// that never logged on.
///
/// Scenario 2S: log an error and disconnect - Section 4.3.1 says "without
/// Logout(35=5) processing", so nothing goes on the wire and the counters
/// stay where they were.
///
/// The rest of the table is every other type a peer could open with - the
/// keep-alive pair, a ResendRequest, an application message - so the gate is
/// pinned as a whitelist, not as a list of known offenders. A Logout is the
/// one type whose verdict depends on the state: in `LogonSent` it is the
/// acceptor's way to refuse the connection, so only `Idle` treats it as a
/// non-Logon first message.
///
/// The positive controls at the end pin what the gate must keep admitting:
/// in `LogonSent` the peer's Logon acknowledgement and the Logout an acceptor
/// sends to refuse the connection (Scenario 1B, 1S(d)); in `LogoutSent` the
/// messages already on the wire, SequenceReset and Reject included.
#[test]
fn pre_logon_gate_exempts_no_message_type() {
    fn gated_inputs(logon_sent: bool) -> impl Iterator<Item = Box<Message>> {
        [
            test_helpers::sequence_reset(1, 4_000_000, false),
            test_helpers::reject(1, 1),
            test_helpers::heartbeat(1, None),
            test_helpers::test_request(1, fix_str!("probe")),
            test_helpers::resend_request(1, 1, 0),
            test_helpers::new_order_single(1),
        ]
        .into_iter()
        .chain((!logon_sent).then(|| test_helpers::logout(1)))
    }

    for logon_sent in [false, true] {
        for msg in gated_inputs(logon_sent) {
            let (mut engine, mut storage) = EngineBuilder::new().build();
            if logon_sent {
                engine
                    .send_logon_request(&mut storage, SessionStart::Resume)
                    .unwrap();
                drain_all_admin(&mut engine);
            }
            let msg_type = SessionMessage::msg_type(&*msg);

            let result = accept_input(&mut engine, msg, &mut storage);

            assert_matches!(
                result,
                InputResult::Handled,
                "{msg_type} (logon_sent={logon_sent})"
            );
            assert_eq!(
                engine.disconnect_reason(),
                Some(DisconnectReason::InvalidLogonState),
                "{msg_type} (logon_sent={logon_sent})"
            );
            assert_eq!(storage.next_target_msg_seq_num().get(), 1);
            assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
            assert!(
                engine.take_admin_output().is_none(),
                "{msg_type} (logon_sent={logon_sent}): nothing goes on the wire"
            );
        }
    }

    let (mut engine, mut storage) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    drain_all_admin(&mut engine);
    for msg in [
        test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER")),
        test_helpers::logout(1),
    ] {
        assert_matches!(verify_test(&engine, &msg, &storage, true, true), Ok(()));
    }

    let (mut engine, storage) = EngineBuilder::new().logged_on().build();
    engine.send_logout(None, None);
    drain_all_admin(&mut engine);
    for msg in [
        test_helpers::sequence_reset(1, 100, false),
        test_helpers::reject(1, 1),
    ] {
        assert_matches!(verify_test(&engine, &msg, &storage, true, true), Ok(()));
    }
}

/// Negative control for the same arm: a `Logon<A>` is not traffic already
/// on the wire. FIX Transport Section 8.7 leaves Logout Pending only by
/// disconnecting or by the peer's Logout acknowledgement, so admitting one
/// would revive a session whose Logout has already been sent - and disarm
/// `logout_deadline` with it, leaving nothing to end the connection. The
/// disconnect is silent: the peer already has our Logout, and a second one
/// would only consume a `MsgSeqNum` (Section 4.6.4).
#[test]
fn on_logon_while_logout_in_flight_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    engine.send_logout(None, None);
    drain_all_admin(&mut engine);

    let msg = test_helpers::logon(1, fix_str!("TARGET"), fix_str!("SENDER"));
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.should_disconnect());
    assert!(engine.take_admin_output().is_none());
    assert!(engine.logout_deadline().is_some());
}

/// A well-formed second `Logon<A>` over an established session, without
/// `ResetSeqNumFlag(141)=Y`: the only mid-session re-Logon FIX Session
/// Layer Section 4.4.2 admits is the reset one, so this is a state violation, not a
/// resync. The spec covers a duplicate *connection* (Section 4.6.4: terminate
/// without sending a message, since a Reject or Logout would consume a
/// `MsgSeqNum`) rather than a duplicate Logon on the same connection; the
/// engine takes the same silent path, and both counters stay put. Clean-path
/// companion to the broken-body variant
/// `on_deserialize_error_with_header_logon_when_established_disconnects`.
#[test]
fn on_logon_second_logon_without_reset_flag_disconnects_silently() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    storage.set_next_sender_msg_seq_num(nz_seq(5)).unwrap();

    let msg = test_helpers::logon(5, fix_str!("TARGET"), fix_str!("SENDER"));
    assert_matches!(
        accept_input(&mut engine, msg, &mut storage),
        InputResult::Handled
    );

    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.take_admin_output().is_none(), "silent disconnect");
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 5);
}

/// `MsgSeqNum(34)=0` names no message - numbering starts at 1 (FIX Session
/// Layer Section 4.1) - and the codec lets the zero through on purpose: which zeros
/// are legal is a per-tag question (`EndSeqNo(16)=0` is one), so the session
/// layer is where tag 34 has to be judged. Zero is below every possible
/// `NextNumIn`, so it takes the too-low path of Test Cases Section 4.5.1 Scenario
/// 2(c): `Logout<5>` naming both numbers, no `Reject<3>`, then disconnect,
/// with `NextNumIn` untouched. The same verdict whether the zero arrives
/// mid-session or on the very first Logon, where Scenario 1S(d) asks for the
/// same Logout-with-Text farewell.
#[test]
fn msg_seq_num_zero_is_too_low() {
    for (logged_on, msg) in [
        (true, test_helpers::heartbeat(0, None)),
        (
            false,
            test_helpers::logon(0, fix_str!("TARGET"), fix_str!("SENDER")),
        ),
    ] {
        let mut builder = EngineBuilder::new();
        if logged_on {
            builder = builder.logged_on();
        }
        let (mut engine, mut storage) = builder.build();

        assert_matches!(
            accept_input(&mut engine, msg, &mut storage),
            InputResult::Handled,
            "logged_on={logged_on}"
        );

        assert_eq!(
            engine.disconnect_reason(),
            Some(DisconnectReason::MsgSeqNumTooLow),
            "logged_on={logged_on}"
        );
        let logout = take_admin(&mut engine);
        let AdminBase::Logout(lo) = as_admin(&logout) else {
            panic!("expected Logout (logged_on={logged_on})");
        };
        assert_eq!(
            lo.session_status,
            Some(SessionStatusBase::ReceivedMsgSeqNumTooLow.into())
        );
        assert_eq!(
            lo.text.as_deref(),
            Some(fix_str!("MsgSeqNum too low, expected 1, got 0"))
        );
        assert!(
            engine.take_admin_output().is_none(),
            "no Reject accompanies the Logout (logged_on={logged_on})"
        );
        assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    }
}
