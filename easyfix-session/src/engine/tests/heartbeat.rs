use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    fix_str,
};

use super::support::assert_msg_type;
use crate::{
    engine::InputResult,
    initiator::SessionStart,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, take_admin},
};

#[test]
fn send_heartbeat_produces_heartbeat_in_admin_output() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    engine.send_heartbeat(None);
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Heartbeat);
    let AdminBase::Heartbeat(hb) = as_admin(&msg) else {
        panic!("expected Heartbeat");
    };
    assert!(hb.test_req_id.is_none());
}

#[test]
fn send_heartbeat_with_test_req_id() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    let test_req_id = fix_str!("TEST123").to_owned();
    engine.send_heartbeat(Some(test_req_id));
    let msg = take_admin(&mut engine);
    let AdminBase::Heartbeat(hb) = as_admin(&msg) else {
        panic!("expected Heartbeat");
    };
    assert_eq!(hb.test_req_id.as_deref(), Some(fix_str!("TEST123")));
}

#[test]
fn send_test_request_produces_test_request_in_admin_output() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    let test_req_id = fix_str!("REQ001").to_owned();
    engine.send_test_request(test_req_id);
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::TestRequest);
    let AdminBase::TestRequest(tr) = as_admin(&msg) else {
        panic!("expected TestRequest");
    };
    assert_eq!(&*tr.test_req_id, fix_str!("REQ001"));
}

// --- Heartbeat ---

#[test]
fn on_heartbeat_normal() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::heartbeat(1, None);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

#[test]
fn on_heartbeat_matching_grace_period() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // Register a grace period TestReqID
    engine.on_input_timeout();
    assert_eq!(engine.grace_period_count(), 1);

    // Extract the generated TestReqID from the TestRequest in admin_output
    let tr_msg = take_admin(&mut engine);
    let AdminBase::TestRequest(ref tr) = as_admin(&tr_msg) else {
        panic!("expected TestRequest");
    };
    let msg = test_helpers::heartbeat(1, Some(tr.test_req_id.clone().into_owned()));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(engine.grace_period_count(), 0);
}

#[test]
fn on_heartbeat_non_matching_grace_period() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    engine.on_input_timeout();
    assert_eq!(engine.grace_period_count(), 1);
    // Drain the TestRequest from admin_output
    let _ = take_admin(&mut engine);

    // Send heartbeat with a different TestReqID
    let msg = test_helpers::heartbeat(1, Some(fix_str!("WRONG_ID").to_owned()));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    // Grace period not cleared
    assert_eq!(engine.grace_period_count(), 1);
}

// --- TestRequest ---

#[test]
fn on_test_request_normal() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::test_request(1, fix_str!("REQ001"));
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);

    // Should have a Heartbeat with matching TestReqID in admin_output
    let hb_msg = take_admin(&mut engine);
    assert_msg_type(&hb_msg, MsgTypeBase::Heartbeat);
    let AdminBase::Heartbeat(hb) = as_admin(&hb_msg) else {
        panic!("expected Heartbeat");
    };
    assert_eq!(hb.test_req_id.as_deref(), Some(fix_str!("REQ001")));
}

#[test]
fn on_input_timeout_first_call_sends_test_request() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    engine.on_input_timeout();

    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::TestRequest);
    let AdminBase::TestRequest(tr) = as_admin(&msg) else {
        panic!("expected TestRequest");
    };
    // TestReqID should be non-empty
    assert!(!tr.test_req_id.is_empty());
    // Grace period count should be 1
    assert_eq!(engine.grace_period_count(), 1);
    // Should NOT disconnect
    assert!(!engine.should_disconnect());
}

#[test]
fn on_input_timeout_grace_period_exceeded_sends_logout() {
    // Default posture = the spec's letter (FIX Session Layer 4.5.5): a
    // single unanswered TestRequest probe terminates the session with
    // Logout + Text.
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();

    // First timeout: sends TestRequest
    engine.on_input_timeout();
    let _ = take_admin(&mut engine);
    assert_eq!(engine.grace_period_count(), 1);
    assert!(!engine.should_disconnect());

    // Second timeout: probe unanswered (count=1 >= limit=1), sends Logout
    engine.on_input_timeout();
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Logout);
    let AdminBase::Logout(logout) = as_admin(&msg) else {
        panic!("expected Logout");
    };
    assert!(
        logout.text.is_some(),
        "Logout should contain text explaining the timeout"
    );
    assert!(engine.should_disconnect());
}

#[test]
fn on_input_timeout_extra_probe_when_configured() {
    // Operator-configured leniency: two probes before giving up.
    let (mut engine, _store) = EngineBuilder::new()
        .auto_disconnect_after_no_heartbeat(2)
        .logged_on()
        .build();

    // First timeout: sends TestRequest
    engine.on_input_timeout();
    let _ = take_admin(&mut engine);
    assert_eq!(engine.grace_period_count(), 1);
    assert!(!engine.should_disconnect());

    // Second timeout: sends another TestRequest (count=2, limit=2, not yet exceeded)
    engine.on_input_timeout();
    let _ = take_admin(&mut engine);
    assert_eq!(engine.grace_period_count(), 2);
    assert!(!engine.should_disconnect());

    // Third timeout: grace period exceeded (count=2 >= limit=2), sends Logout
    engine.on_input_timeout();
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Logout);
    assert!(engine.should_disconnect());
}

#[test]
fn on_input_timeout_then_matching_heartbeat_then_timeout_sends_test_request() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();

    // First timeout: sends TestRequest
    engine.on_input_timeout();
    let tr_msg = take_admin(&mut engine);
    let AdminBase::TestRequest(ref tr) = as_admin(&tr_msg) else {
        panic!("expected TestRequest");
    };
    let test_req_id = tr.test_req_id.clone().into_owned();
    assert_eq!(engine.grace_period_count(), 1);

    // Matching Heartbeat clears grace period
    let hb = test_helpers::heartbeat(1, Some(test_req_id));
    let result = accept_input(&mut engine, hb, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(engine.grace_period_count(), 0);

    // Next timeout: sends TestRequest again (not Logout)
    engine.on_input_timeout();
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::TestRequest);
    assert!(!engine.should_disconnect());
    assert_eq!(engine.grace_period_count(), 1);
}

#[test]
fn on_input_timeout_unverified_heartbeat_keeps_session_alive() {
    // Nothing but `process_heartbeat` and `process_logon` shrinks the grace
    // set. A session that skipped it with verification off would escalate to
    // Logout at the next timeout even though the peer had answered - and
    // repeat that after every reconnect.
    let (mut engine, mut storage) = EngineBuilder::new()
        .verify_test_request_id(false)
        .logged_on()
        .build();

    engine.on_input_timeout();
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::TestRequest);
    assert_eq!(engine.grace_period_count(), 1);

    // The peer answers with an id matching nothing, which is the case this
    // mode exists for.
    let hb = test_helpers::heartbeat(1, Some(fix_str!("WRONG_ID").to_owned()));
    assert_matches!(
        accept_input(&mut engine, hb, &mut storage),
        InputResult::Handled
    );
    assert_eq!(engine.grace_period_count(), 0);

    // Next timeout probes again instead of logging out.
    engine.on_input_timeout();
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::TestRequest);
    assert!(!engine.should_disconnect());
}

#[test]
fn on_output_timeout_sends_heartbeat_without_test_req_id() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    engine.on_output_timeout();

    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Heartbeat);
    let AdminBase::Heartbeat(hb) = as_admin(&msg) else {
        panic!("expected Heartbeat");
    };
    assert!(hb.test_req_id.is_none());
    assert!(engine.take_admin_output().is_none());
}

/// Neither keep-alive timer may put a message on the wire before the handshake
/// completes. A `TestRequest<1>` or `Heartbeat<0>` sent while the initiator is
/// still awaiting its Logon response asks a peer to answer for a session that
/// does not exist yet - and if our own `Logon<A>` never reached that peer, the
/// probe arrives as its first message, which puts us straight on its disconnect
/// path (FIX Session Layer Section 4.3.1; Test Cases Scenario 2S). An unanswered Logon
/// is `logon_deadline`'s job.
#[test]
fn keep_alive_timers_are_silent_while_awaiting_the_logon_response() {
    let (mut engine, mut store) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut store, SessionStart::Resume)
        .unwrap();
    let logon = take_admin(&mut engine);
    assert_msg_type(&logon, MsgTypeBase::Logon);

    engine.on_input_timeout();
    engine.on_output_timeout();

    assert!(
        engine.take_admin_output().is_none(),
        "nothing may follow the Logon request until the peer answers it"
    );
    // No probe was registered either, so the grace-period escalation cannot
    // start counting against a session that never got off the ground.
    assert_eq!(engine.grace_period_count(), 0);
    assert!(!engine.should_disconnect());
}
