use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, LogoutBase, MsgTypeBase, SessionStatusBase},
    basic_types::FixString,
    fix_str,
    message::SessionMessage,
};
use easyfix_test_messages::Message;
use tokio::time::Duration;

use super::{resend_support::assert_resend_request, support::assert_msg_type};
use crate::{
    application::DisconnectReason,
    engine::InputResult,
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, drain_all_admin, take_admin},
};

#[test]
fn send_logout_produces_logout_in_admin_output() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    engine.send_logout(None, None);
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Logout);
}

#[test]
fn send_logout_with_text() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    let text = FixString::from_ascii_lossy(b"Goodbye".to_vec());
    engine.send_logout(None, Some(text));
    let msg = take_admin(&mut engine);
    let AdminBase::Logout(logout) = as_admin(&msg) else {
        panic!("expected Logout");
    };
    assert_eq!(logout.text.as_deref(), Some(fix_str!("Goodbye")));
}

/// The same on `Logout<5>`, where losing the message costs more: Test Cases
/// Scenario 1S(d) makes the Logout the mandatory half of the escalation after
/// an invalid Logon, while the preceding Reject is optional.
#[test]
fn send_logout_omits_empty_text() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    engine.send_logout(None, Some(FixString::default()));
    let mut msg = take_admin(&mut engine);
    let AdminBase::Logout(logout) = as_admin(&msg) else {
        panic!("expected Logout");
    };
    assert_eq!(logout.text, None, "empty Text(58) must be omitted");

    engine.fill_header(&mut msg, &mut storage).unwrap();
    let bytes = test_helpers::serialize_message(&msg);
    assert!(
        !bytes.windows(4).any(|w| w == b"\x0158="),
        "no Text(58) field may reach the wire: {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

// --- Logout ---

/// An unsolicited peer `Logout<5>` is acknowledged, and then the connection
/// is the peer's to close: the engine leaves the loop without a disconnect
/// decision and reports a deadline for that close instead (FIX Session Layer
/// Section 4.6, Figure 9; Test Cases Scenario 13(b)). App traffic is over.
#[test]
fn on_logout_request() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let msg = test_helpers::logout(1);
    let before = engine.timer_backend().now();
    let result = accept_input(&mut engine, msg, &mut storage);
    let after = engine.timer_backend().now();
    assert_matches!(result, InputResult::Handled);
    assert!(!engine.should_disconnect());
    assert!(engine.should_leave_loop());
    assert!(!engine.is_logged_on());
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);

    let deadline = engine
        .awaiting_peer_close()
        .expect("the wait for the peer's close must be bounded");
    let budget = engine.session_settings().auto_disconnect_after_no_logout;
    assert!(deadline >= before + budget);
    assert!(deadline <= after + budget);

    // Logout acknowledgement in admin_output.
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
    let AdminBase::Logout(ref lo) = as_admin(&logout_msg) else {
        panic!("expected Logout");
    };
    assert_eq!(
        lo.session_status,
        Some(SessionStatusBase::SessionLogoutComplete.into())
    );
}

/// The wait for the peer's close ran out: Scenario 13(b) has the session
/// disconnect and report an error condition, and the reason names it.
#[test]
fn on_peer_close_timeout_ends_with_the_remote_logout_timeout() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    accept_input(&mut engine, test_helpers::logout(1), &mut storage);

    engine.on_peer_close_timeout();

    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::RemoteRequestedLogoutTimeout)
    );
}

/// A local logout request while the peer's Logout is already acknowledged
/// has nothing to add: a second Logout would only consume a sequence number.
#[test]
fn on_control_logout_while_awaiting_peer_close_is_ignored() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    accept_input(&mut engine, test_helpers::logout(1), &mut storage);
    drain_all_admin(&mut engine);

    engine.on_control(ControlMsg::Logout {
        session_status: None,
        text: None,
    });

    assert!(engine.take_admin_output().is_none());
    assert!(engine.awaiting_peer_close().is_some());
    assert!(engine.logout_deadline().is_none());
}

/// With `verify_logout` (the default) a peer Logout is a message like any
/// other: one ahead of `NextNumIn` is parked behind a `ResendRequest<2>` and
/// answered only once the gap closes (FIX Session Layer Section 4.6.3).
#[test]
fn on_logout_verified_parks_an_out_of_sequence_logout() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();

    let result = accept_input(&mut engine, test_helpers::logout(50), &mut storage);

    assert_matches!(result, InputResult::Handled);
    assert_resend_request(&mut engine, 1, 49);
    assert!(
        engine.take_admin_output().is_none(),
        "no acknowledgement yet"
    );
    assert!(engine.awaiting_peer_close().is_none());
    assert_eq!(engine.queued_count(), 1);
}

/// `verify_logout = false` skips the header checks on a peer Logout: the
/// same out-of-sequence Logout, from the wrong CompID even, is acknowledged
/// on the spot - no `ResendRequest<2>`, no `Reject<3>`, no parking.
#[test]
fn on_logout_unverified_acknowledges_whatever_arrives() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .verify_logout(false)
        .logged_on()
        .build();
    let logout = Box::new(Message::from_admin(
        test_helpers::header(50, fix_str!("WRONG"), fix_str!("SENDER")),
        AdminBase::Logout(LogoutBase {
            session_status: None,
            text: None,
        }),
    ));

    let result = accept_input(&mut engine, logout, &mut storage);

    assert_matches!(result, InputResult::Handled);
    let ack = take_admin(&mut engine);
    assert_msg_type(&ack, MsgTypeBase::Logout);
    assert!(
        engine.take_admin_output().is_none(),
        "nothing but the acknowledgement"
    );
    assert!(engine.awaiting_peer_close().is_some());
    assert_eq!(engine.queued_count(), 0);
}

#[test]
fn on_logout_response() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    // Simulate having sent Logout first
    engine.send_logout(None, None);
    let _ = take_admin(&mut engine); // drain the outgoing Logout

    let msg = test_helpers::logout(1);
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::LocalRequestedLogout)
    );
    assert!(engine.should_disconnect());

    // No new Logout in admin_output (we already sent one)
    assert!(engine.take_admin_output().is_none());
}

/// Same as [`logon_deadline_is_unarmed_when_the_budget_overflows_the_clock`],
/// for the Logout budget.
#[test]
fn logout_deadline_is_unarmed_when_the_budget_overflows_the_clock() {
    let (mut engine, _store) = EngineBuilder::new()
        .logged_on()
        .auto_disconnect_after_no_logout(Duration::MAX)
        .build();
    engine.on_control(ControlMsg::Logout {
        session_status: None,
        text: None,
    });
    // The Logout went out, so the engine is in the state that arms the
    // deadline whenever the budget is representable.
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
    assert!(engine.logout_deadline().is_none());
}

#[test]
fn on_logout_timeout_sets_disconnect_no_messages() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    assert!(!engine.should_disconnect());

    engine.on_logout_timeout();

    assert!(engine.should_disconnect());
    assert!(engine.take_admin_output().is_none());
}

#[test]
fn on_control_logout_sends_logout_and_sets_deadline() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    assert!(engine.logout_deadline().is_none());

    engine.on_control(ControlMsg::Logout {
        session_status: Some(SessionStatusBase::SessionLogoutComplete.into()),
        text: Some(fix_str!("Shutting down").to_owned()),
    });

    // Logout in admin_output
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Logout);
    let AdminBase::Logout(logout) = as_admin(&msg) else {
        panic!("expected Logout");
    };
    assert_eq!(logout.text.as_deref(), Some(fix_str!("Shutting down")));

    // logout_deadline should now be Some
    assert!(engine.logout_deadline().is_some());
}

#[test]
fn on_control_disconnect_sets_disconnect_no_messages() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    assert!(!engine.should_disconnect());

    engine.on_control(ControlMsg::Disconnect);

    assert!(engine.should_disconnect());
    assert!(engine.take_admin_output().is_none());
}
