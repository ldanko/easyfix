use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    fix_str,
    message::{HeaderAccess, SessionMessage},
};

use super::{
    resend_support::{assert_resend_request, drain_queued},
    support::assert_msg_type,
};
use crate::{
    application::{DisconnectReason, InputAction},
    engine::InputResult,
    initiator::SessionStart,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, drain_all_admin, nz_seq, take_admin},
};

#[test]
fn send_resend_request_produces_resend_request_in_admin_output() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    engine.send_resend_request(1, 5);
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::ResendRequest);
    let AdminBase::ResendRequest(rr) = as_admin(&msg) else {
        panic!("expected ResendRequest");
    };
    assert_eq!(rr.begin_seq_no, 1);
    assert_eq!(rr.end_seq_no, 5);
}

// --- ResendRequest ---

#[test]
fn on_resend_request_normal() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // Set sender seq higher so the range is valid
    storage.set_next_sender_msg_seq_num(nz_seq(10)).unwrap();

    let msg = test_helpers::resend_request(1, 1, 5);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);

    assert!(engine.has_pending_resends());
    let range = engine.take_pending_resend().unwrap();
    assert_eq!(range, 1..=5);
}

#[test]
fn on_resend_request_end_zero_normalized() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(10)).unwrap();

    let msg = test_helpers::resend_request(1, 1, 0);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    // end_seq_no=0 normalized to next_sender-1=9
    let range = engine.take_pending_resend().unwrap();
    assert_eq!(range, 1..=9);
}

#[test]
fn on_resend_request_too_high_seq() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(10)).unwrap();

    // msg seq=5, but target expects 1 -> too high (manual check)
    let msg = test_helpers::resend_request(5, 1, 3);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    assert_eq!(engine.queued_count(), 1);

    // Range still pushed to pending_resends
    assert!(engine.has_pending_resends());

    // ResendRequest for the gap in admin_output
    let rr_msg = take_admin(&mut engine);
    assert_msg_type(&rr_msg, MsgTypeBase::ResendRequest);
}

#[test]
fn on_resend_request_second_while_pending() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(20)).unwrap();

    let msg1 = test_helpers::resend_request(1, 1, 5);
    accept_input(&mut engine, msg1, &mut storage);

    let msg2 = test_helpers::resend_request(2, 10, 15);
    accept_input(&mut engine, msg2, &mut storage);

    let r1 = engine.take_pending_resend().unwrap();
    let r2 = engine.take_pending_resend().unwrap();
    assert_eq!(r1, 1..=5);
    assert_eq!(r2, 10..=15);
}

/// A ResendRequest whose own MsgSeqNum is too low (below NextNumIn, no
/// PossDup) is a fatal desync like any inbound message - Logout + disconnect.
#[test]
fn on_resend_request_too_low_logs_out_and_disconnects() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    storage.set_next_sender_msg_seq_num(nz_seq(10)).unwrap();
    // ResendRequest MsgSeqNum=1 << expected 5.
    let msg = test_helpers::resend_request(1, 1, 3);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::MsgSeqNumTooLow)
    );
    assert!(engine.should_disconnect());
    let logout = take_admin(&mut engine);
    assert_msg_type(&logout, MsgTypeBase::Logout);
}

/// A range naming no message draws Reject 373=5 on tag 7 and still advances
/// NextNumIn (Testcases Section 4.5.13 Scenario 14(e)). `BeginSeqNo(7)=0` names none
/// because sequence numbers start at 1 (Session Layer Section 4.1) - queueing it
/// would put a gap-fill stamped `MsgSeqNum(34)=0` on the wire. An inverted
/// range is none of the three forms Section 4.8.2 admits - unchecked it queues an
/// empty range and the peer gets silence.
#[test]
fn on_resend_request_with_a_range_naming_no_message_rejects() {
    for (begin, end) in [(0, 0), (10, 5)] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        storage.set_next_sender_msg_seq_num(nz_seq(20)).unwrap();

        let msg = test_helpers::resend_request(1, begin, end);
        let result = accept_input(&mut engine, msg, &mut storage);
        assert_matches!(result, InputResult::Handled, "({begin}, {end})");

        let reject_msg = take_admin(&mut engine);
        assert_msg_type(&reject_msg, MsgTypeBase::Reject);
        let AdminBase::Reject(reject) = as_admin(&reject_msg) else {
            panic!("expected Reject ({begin}, {end})");
        };
        assert_eq!(
            reject.session_reject_reason,
            Some(SessionRejectReasonBase::ValueIsIncorrect.into()),
            "({begin}, {end})"
        );
        assert_eq!(reject.ref_tag_id, Some(7), "({begin}, {end}): BeginSeqNo");
        assert_eq!(reject.ref_seq_num, 1);

        assert!(!engine.has_pending_resends(), "({begin}, {end})");
        // Scenario 14(e) step 2: a rejected message still advances NextNumIn.
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    }
}

/// Boundary the range check must not swallow: `begin == end` requests a
/// single message (Section 4.8.2).
#[test]
fn on_resend_request_single_message_range_accepted() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(10)).unwrap();

    let msg = test_helpers::resend_request(1, 5, 5);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    assert!(engine.take_admin_output().is_none());
    assert_eq!(engine.take_pending_resend().unwrap(), 5..=5);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// `EndSeqNo(16)=0` is infinity, so it is exempt from the `begin > end`
/// check: open-ended from a seq num we have not reached is well-formed and
/// must not draw a Reject.
///
/// Silence here is a decision, not an oversight. Unlike a zero or inverted
/// range, this one is judged against our counters rather than against the
/// value itself, and a peer that outlived our storage sends it legitimately,
/// so Rejecting would blame the wrong side. The engine logs a warning
/// instead (not asserted here; no log-capture harness in this suite).
#[test]
fn on_resend_request_begin_above_next_sender_with_end_zero_accepted() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(3)).unwrap();

    let msg = test_helpers::resend_request(1, 5, 0);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);

    assert!(engine.take_admin_output().is_none());
    // Normalized to next_sender-1=2, leaving an empty range. Spelled out
    // because clippy denies a `5..=2` literal.
    let range = engine.take_pending_resend().unwrap();
    assert_eq!(*range.start(), 5);
    assert_eq!(*range.end(), 2);
    assert!(range.is_empty());
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

/// A second sequence gap after the first one was fully recovered must
/// trigger a fresh ResendRequest (Session Layer 4.8.1; Scenario 2(b)).
/// Guards against suppression-forever: the `resend_range` left over from
/// the first gap is stale once NextNumIn has moved past its end and must
/// not suppress requests for later gaps.
#[test]
fn second_gap_after_recovery_sends_fresh_resend_request() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // First gap: heartbeat seq 3 while expecting 1 -> RR 1..=2, 3 parked.
    let result = accept_input(&mut engine, test_helpers::heartbeat(3, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 1, 2);
    // Peer redelivers 1 and 2; the parked 3 is then drained.
    accept_input(&mut engine, test_helpers::heartbeat(1, None), &mut storage);
    accept_input(&mut engine, test_helpers::heartbeat(2, None), &mut storage);
    drain_queued(&mut engine, &mut storage);
    assert_eq!(storage.next_target_msg_seq_num().get(), 4);
    assert!(engine.take_admin_output().is_none());
    // Second gap: heartbeat 6 -> RR 4..=5.
    let result = accept_input(&mut engine, test_helpers::heartbeat(6, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 4, 5);
}

/// While the first gap is still outstanding, another too-high message
/// whose implied request is covered by the outstanding range must NOT
/// produce a duplicate ResendRequest (`send_redundant_resend_requests`
/// disabled).
#[test]
fn covered_gap_request_stays_suppressed_while_outstanding() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let result = accept_input(&mut engine, test_helpers::heartbeat(5, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 1, 4);
    // Second too-high message: the implied request is still 1..=4
    // (clamped to the parked 5) - covered, so suppressed.
    let result = accept_input(&mut engine, test_helpers::heartbeat(7, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(
        engine.take_admin_output().is_none(),
        "covered request must stay suppressed"
    );
    assert_eq!(engine.queued_count(), 2);
}

/// With `send_redundant_resend_requests` the covered request goes out again -
/// the same clamped range - for peers known to drop a `ResendRequest<2>`.
#[test]
fn covered_gap_request_repeats_with_send_redundant_resend_requests() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .send_redundant_resend_requests(true)
        .logged_on()
        .build();
    let result = accept_input(&mut engine, test_helpers::heartbeat(5, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 1, 4);

    let result = accept_input(&mut engine, test_helpers::heartbeat(7, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 1, 4);
    assert_eq!(engine.queued_count(), 2);
}

/// Drop-and-request for an unparseable too-high message extends the
/// request THROUGH its seq num even when a parseable message is already
/// parked in the queue - the clamp to the lowest parked message applies
/// only to the park-and-request strategy (Session Layer 4.8.2 sanctions
/// both; redundant redelivery of a parked seq num is ignored as a
/// PossDup duplicate).
#[test]
fn broken_too_high_requests_through_even_with_queued_messages() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();
    let result = accept_input(&mut engine, test_helpers::heartbeat(7, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 5, 6);
    // Broken-body message at 9: cannot be parked - request through 9.
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        test_helpers::inbound_header(9),
        Some(44),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    assert_resend_request(&mut engine, 5, 9);
}

/// The full drop-and-request loop across TWO gaps: a broken-body too-high
/// message arriving after an earlier gap was recovered must still trigger
/// its ResendRequest - otherwise the dropped message is never redelivered
/// and inbound processing stalls.
#[test]
fn broken_too_high_after_recovered_gap_requests_resend() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // First gap recovered: 3 parked, 1..=2 redelivered, 3 drained.
    accept_input(&mut engine, test_helpers::heartbeat(3, None), &mut storage);
    assert_resend_request(&mut engine, 1, 2);
    accept_input(&mut engine, test_helpers::heartbeat(1, None), &mut storage);
    accept_input(&mut engine, test_helpers::heartbeat(2, None), &mut storage);
    drain_queued(&mut engine, &mut storage);
    assert_eq!(storage.next_target_msg_seq_num().get(), 4);
    // Broken-body message at 8 -> drop-and-request 4..=8.
    let error = test_helpers::body_reject_error(
        fix_str!("D"),
        test_helpers::inbound_header(8),
        Some(44),
        SessionRejectReasonBase::ValueIsIncorrect,
    );
    let result = engine.on_deserialize_error(error, &mut storage).unwrap();
    assert_matches!(result, InputResult::Error(..));
    assert_resend_request(&mut engine, 4, 8);
    assert_eq!(storage.next_target_msg_seq_num().get(), 4);
}

/// Initiator sent NextExpectedMsgSeqNum(789); the peer's too-high Logon
/// response records an implicit-resend suppression range. That range must
/// be finite: once the implicit recovery completes, later gaps must get
/// their own ResendRequest.
#[test]
fn gap_after_tag_789_implicit_resend_is_requested() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .enable_next_expected_msg_seq_num()
        .build();
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    drain_all_admin(&mut engine);
    // Our Logon consumed seq 1 on the wire.
    storage.set_next_sender_msg_seq_num(nz_seq(2)).unwrap();
    // Peer's Logon response is too high (seq 5, expecting 1) and carries
    // 789=2 == our NextNumOut, so no outbound resend is implied. The
    // explicit ResendRequest for our inbound gap is suppressed - the
    // resend is implied by our own tag 789.
    let msg = test_helpers::logon_with_options(
        5,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        None,
        Some(2),
    );
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(
        engine.take_admin_output().is_none(),
        "explicit ResendRequest is implied by tag 789"
    );
    // Implicit recovery completes: 1..=4 redelivered, parked Logon drained.
    for seq in 1..=4 {
        accept_input(
            &mut engine,
            test_helpers::heartbeat(seq, None),
            &mut storage,
        );
    }
    drain_queued(&mut engine, &mut storage);
    assert_eq!(storage.next_target_msg_seq_num().get(), 6);
    // A later gap must be requested despite the recorded 789 range.
    let result = accept_input(&mut engine, test_helpers::heartbeat(9, None), &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 6, 8);
}

/// Scenario 10a: a `SequenceReset-GapFill` whose own MsgSeqNum is ABOVE
/// NextNumIn signals a gap before it - the session issues a ResendRequest
/// to fill the gap and does NOT advance NextNumIn.
#[test]
fn on_sequence_reset_gap_fill_too_high_triggers_resend() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_target_msg_seq_num(nz_seq(2)).unwrap();
    // GapFill MsgSeqNum=5 > NextNumIn=2 -> gap before this message.
    let msg = test_helpers::sequence_reset(5, 10, true);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    let rr = take_admin(&mut engine);
    assert_msg_type(&rr, MsgTypeBase::ResendRequest);
    // The too-high GapFill is parked, not processed - NextNumIn unchanged.
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

// --- next_queued_message ---

#[test]
fn next_queued_message_empty_queue() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    assert!(engine.next_queued_message(&mut storage).unwrap().is_none());
}

#[test]
fn next_queued_message_not_ready() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // Queue a message at seq=5, but target expects 1
    let msg = test_helpers::new_order_single(5);
    engine.on_input(msg, &mut storage).unwrap();
    // Drain the ResendRequest
    let _ = take_admin(&mut engine);

    // next_target is still 1, queued msg is at 5 -> not ready
    assert!(engine.next_queued_message(&mut storage).unwrap().is_none());
}

#[test]
fn next_queued_message_ready_app() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // Queue app message at seq=3 via too-high
    let msg = test_helpers::new_order_single(3);
    engine.on_input(msg, &mut storage).unwrap();
    let _ = take_admin(&mut engine); // drain ResendRequest

    // Advance target to 3 (simulating gap fill)
    storage.set_next_target_msg_seq_num(nz_seq(3)).unwrap();

    let result = engine.next_queued_message(&mut storage).unwrap();
    assert_matches!(result, Some(InputResult::AppMsg(_)));
    assert_eq!(engine.queued_count(), 0);
}

#[test]
fn next_queued_message_queued_logon() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    // Process a too-high logon: validation passes, app callback fires
    // (here auto-Accept), process_logon enqueues the message and emits
    // a ResendRequest for the gap.
    let msg = test_helpers::logon(5, fix_str!("TARGET"), fix_str!("SENDER"));
    accept_input(&mut engine, msg, &mut storage);
    drain_all_admin(&mut engine);

    // Advance target to 5 (simulating gap fill)
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();

    // Queued Logon: just increments seq, returns Handled
    let result = engine.next_queued_message(&mut storage).unwrap();
    assert_matches!(result, Some(InputResult::Handled));
    assert_eq!(storage.next_target_msg_seq_num().get(), 6);
    assert_eq!(engine.queued_count(), 0);
}

#[test]
fn next_queued_message_multi_drain() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // Feed seq 5, 4, 3 - all too high (target expects 1), all enqueued
    for seq in [5, 4, 3] {
        let msg = test_helpers::new_order_single(seq);
        engine.on_input(msg, &mut storage).unwrap();
    }
    // Drain all ResendRequests from admin_output
    drain_all_admin(&mut engine);
    assert_eq!(engine.queued_count(), 3);

    // Advance target to 3 (simulating gap fill for seq 1 and 2)
    storage.set_next_target_msg_seq_num(nz_seq(3)).unwrap();

    // Should drain 3, 4, 5 in order. Each pop emits AppMsg from the
    // queue; we mirror the IO loop by feeding `Accept` back through
    // `process_app_input` to advance the target sequence number.
    for expected_seq in [3, 4, 5] {
        let popped = engine
            .next_queued_message(&mut storage)
            .unwrap()
            .expect("queued message available");
        let InputResult::AppMsg(msg) = popped else {
            panic!("expected AppMsg, got {popped:?}");
        };
        assert_eq!(msg.msg_seq_num(), expected_seq);
        let result = engine
            .process_app_input(
                msg.msg_seq_num(),
                SessionMessage::msg_type(&*msg),
                InputAction::Accept,
                &mut storage,
            )
            .unwrap();
        assert_matches!(result, InputResult::Handled);
        assert_eq!(storage.next_target_msg_seq_num().get(), expected_seq + 1);
    }

    // Queue is empty now
    assert!(engine.next_queued_message(&mut storage).unwrap().is_none());
}
