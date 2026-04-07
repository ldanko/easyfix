use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    fix_str,
};
use tokio::time::Duration;

use super::{
    reset_peer_support::{assert_reset_refused, reset_test_session},
    support::assert_msg_type,
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
fn on_logon_peer_reset_acknowledged_with_post_reset_tag_789() {
    // The ACK echoes the reset and advertises the next incoming number
    // after consuming the peer's Logon (Session Layer 4.4.2 and 4.4.1).
    let (mut engine, mut storage) = EngineBuilder::new()
        .accept_reset_on_connect(true)
        .enable_next_expected_msg_seq_num()
        .build();

    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();

    let logon = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        Some(1),
    );

    let result = accept_input(&mut engine, logon, &mut storage);
    assert_matches!(result, InputResult::Handled);

    let ack = take_admin(&mut engine);
    assert_msg_type(&ack, MsgTypeBase::Logon);
    let AdminBase::Logon(logon_ack) = as_admin(&ack) else {
        panic!("expected Logon ACK");
    };
    assert_eq!(
        logon_ack.reset_seq_num_flag,
        Some(true),
        "ACK must echo the peer's ResetSeqNumFlag=Y"
    );
    assert_eq!(
        logon_ack.next_expected_msg_seq_num,
        Some(2),
        "tag 789 must be computed from post-reset counters"
    );

    // Counters converge: NextNumIn advances to 2 (logon processed post-reset);
    // the sender counter is back to 1 (the ACK is stamped/incremented later in
    // the IO layer, reaching NextNumOut=2).
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
}

/// A fresh reset must start at one (FIX Session Layer Section 4.4.2).
#[tokio::test]
async fn on_logon_reset_with_msg_seq_num_other_than_one_is_refused() {
    for established in [false, true] {
        for seq in [0, 5, 40] {
            let (mut engine, mut storage, history) = reset_test_session(established);
            let msg = test_helpers::logon_with_options(
                seq,
                fix_str!("TARGET"),
                fix_str!("SENDER"),
                30,
                Some(true),
                None,
            );
            assert_matches!(
                engine.on_input(msg, &mut storage).unwrap(),
                InputResult::Handled
            );
            assert_reset_refused(
                &mut engine,
                &mut storage,
                &history,
                &format!("ResetSeqNumFlag=Y requires MsgSeqNum=1, got {seq}"),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn on_logon_reset_refused_when_the_applicable_permission_is_disabled() {
    for established in [false, true] {
        let (mut engine, mut storage, history) = reset_test_session(established);
        engine.session_settings.accept_reset_on_connect = established;
        engine.session_settings.accept_reset_in_session = !established;
        let msg = test_helpers::logon_with_options(
            1,
            fix_str!("TARGET"),
            fix_str!("SENDER"),
            30,
            Some(true),
            None,
        );
        assert_matches!(
            engine.on_input(msg, &mut storage).unwrap(),
            InputResult::Handled
        );
        let text = if established {
            "Resetting the sequence number is not supported"
        } else {
            "Resetting the sequence number upon FIX connection establishment is not supported"
        };
        assert_reset_refused(&mut engine, &mut storage, &history, text).await;
    }
}

#[test]
fn on_logon_reset_acceptance_permissions_are_independent() {
    for established in [false, true] {
        let builder = EngineBuilder::new()
            .accept_reset_on_connect(!established)
            .accept_reset_in_session(established);
        let (mut engine, mut storage) = if established {
            builder.logged_on().build()
        } else {
            builder.build()
        };
        let msg = test_helpers::logon_with_options(
            1,
            fix_str!("TARGET"),
            fix_str!("SENDER"),
            30,
            Some(true),
            None,
        );
        assert_matches!(
            accept_input(&mut engine, msg, &mut storage),
            InputResult::Handled
        );
        assert!(engine.is_logged_on());
        assert!(engine.disconnect_reason().is_none());
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logon);
    }
}

#[tokio::test]
async fn on_logon_reset_in_session_with_different_heart_bt_int_is_refused() {
    for heartbeat in [0, 20] {
        let (mut engine, mut storage, history) = reset_test_session(true);
        let msg = test_helpers::logon_with_options(
            1,
            fix_str!("TARGET"),
            fix_str!("SENDER"),
            heartbeat,
            Some(true),
            None,
        );
        assert_matches!(
            accept_input(&mut engine, msg, &mut storage),
            InputResult::Handled
        );
        assert_reset_refused(
            &mut engine,
            &mut storage,
            &history,
            "Invalid HeartBtInt(108), expected value 30 seconds",
        )
        .await;
        assert_eq!(engine.heartbeat_interval(), Some(Duration::from_secs(30)));
    }
}

#[tokio::test]
async fn on_logon_reset_in_session_with_heartbeats_disabled_refuses_nonzero() {
    for heartbeat in [0, 30] {
        let (mut engine, mut storage, history) = reset_test_session(true);
        engine.state.heartbeat_interval = None;
        let msg = test_helpers::logon_with_options(
            1,
            fix_str!("TARGET"),
            fix_str!("SENDER"),
            heartbeat,
            Some(true),
            None,
        );
        assert_matches!(
            accept_input(&mut engine, msg, &mut storage),
            InputResult::Handled
        );
        if heartbeat == 0 {
            assert!(engine.disconnect_reason().is_none());
            let ack = take_admin(&mut engine);
            assert_matches!(as_admin(&ack), AdminBase::Logon(logon) if logon.heart_bt_int == 0);
        } else {
            assert_reset_refused(
                &mut engine,
                &mut storage,
                &history,
                "Invalid HeartBtInt(108)",
            )
            .await;
        }
        assert_eq!(engine.heartbeat_interval(), None);
    }
}

#[tokio::test]
async fn on_logon_reset_refused_by_application_leaves_counters() {
    let (mut engine, mut storage, history) = reset_test_session(true);
    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    let InputResult::AdminMsg(msg) = engine.on_input(msg, &mut storage).unwrap() else {
        panic!("expected Logon callback")
    };
    engine
        .process_admin_input(
            msg,
            InputAction::Logout {
                session_status: None,
                text: Some(fix_str!("Reset refused").to_owned()),
                disconnect: true,
            },
            &mut storage,
        )
        .unwrap();
    assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
    assert_eq!(storage.next_target_msg_seq_num().get(), 40);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        history.as_slice()
    );
    assert_msg_type(&take_admin(&mut engine), MsgTypeBase::Logout);
    assert!(engine.take_admin_output().is_none());
}

#[tokio::test]
async fn on_logon_unsolicited_reset_in_response_is_refused() {
    let (mut engine, mut storage, history) = reset_test_session(false);
    engine
        .send_logon_request(&mut storage, SessionStart::Resume)
        .unwrap();
    let _ = take_admin(&mut engine);
    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    assert_matches!(
        engine.on_input(msg, &mut storage).unwrap(),
        InputResult::Handled
    );
    assert_reset_refused(
        &mut engine,
        &mut storage,
        &history,
        "Unsolicited ResetSeqNumFlag=Y in Logon response",
    )
    .await;
}

#[test]
fn on_logon_acceptor_reset_allowed() {
    let (mut engine, mut storage) = EngineBuilder::new().accept_reset_on_connect(true).build();

    // Advance seq nums to verify they get reset
    storage.set_next_sender_msg_seq_num(nz_seq(5)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(5)).unwrap();

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    let result = accept_input(&mut engine, msg, &mut storage);
    assert_matches!(result, InputResult::Handled);
    assert!(engine.is_logged_on());

    // Response mirrors ResetSeqNumFlag
    let logon_msg = take_admin(&mut engine);
    let AdminBase::Logon(logon) = as_admin(&logon_msg) else {
        panic!("expected Logon");
    };
    assert_eq!(logon.reset_seq_num_flag, Some(true));

    // Seq nums reset (post-reset; the Logon itself consumed seq 1, so
    // target now sits at 2 expecting the peer's next message)
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
}

#[test]
fn on_logon_acceptor_reset_disallowed() {
    let (mut engine, mut storage) = EngineBuilder::new().accept_reset_on_connect(false).build();

    let msg = test_helpers::logon_with_options(
        1,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        Some(true),
        None,
    );
    let result = engine.on_input(msg, &mut storage).unwrap();
    assert_matches!(result, InputResult::Handled);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert!(engine.should_disconnect());

    // Logout in admin_output
    let logout_msg = take_admin(&mut engine);
    assert_msg_type(&logout_msg, MsgTypeBase::Logout);
}
