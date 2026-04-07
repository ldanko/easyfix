use std::{assert_matches, time::Duration};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    fix_str,
    message::{HeaderAccess, SessionMessage},
};
use tokio::{io::AsyncReadExt, sync::mpsc, task, task::LocalSet, time};

use super::{
    harness::{AdminGate, TestEvent, build_harness_with_settings},
    reset_peer::running_reset_harness,
};
use crate::{
    application::{DisconnectReason, InputAction},
    io::ControlMsg,
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::nz_seq,
};

#[tokio::test(start_paused = true)]
async fn confirmed_ack_callback_finishes_before_its_expired_budget_is_considered() {
    LocalSet::new().run_until(async {
        for (decision, expired) in [(0, true), (1, true), (2, true), (3, true), (4, true), (4, false)] {
            let mut settings = test_helpers::default_session_settings(); settings.heartbeat_interval = None;
            settings.auto_disconnect_after_no_logon_response = if expired { Duration::from_secs(10) } else { Duration::MAX };
            let mut harness = build_harness_with_settings(settings);
            let (entered, mut started) = mpsc::unbounded_channel(); let (release, action) = mpsc::unbounded_channel();
            harness.app.admin_gate = Some(AdminGate { kind: MsgTypeBase::Logon, skip: 1, entered, action });
            let mut peer = running_reset_harness(harness).await;
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await; peer.send(&test_helpers::heartbeat(2, Some(id))).await;
            let logon = peer.read().await;
            assert_eq!(logon.msg_seq_num(), 1); assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            time::advance(Duration::from_secs(9)).await;
            peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None)).await;
            started.recv().await.unwrap();
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
            if expired { time::advance(Duration::from_secs(2)).await; }
            let decided_at = time::Instant::now();
            release.send(match decision {
                0 => InputAction::Accept,
                1 => InputAction::Disconnect,
                2 | 3 => InputAction::Logout { session_status: None, text: None, disconnect: decision == 2 },
                _ => InputAction::Reject { reason: SessionRejectReasonBase::ValueIsIncorrect.into(), tag: None, text: None },
            }).unwrap();
            let mut sender = 2; let mut target = 2;
            let reason = match decision {
                0 => {
                    task::yield_now().await; peer.assert_silent(); assert!(!peer.task.is_finished());
                    peer.control.send(ControlMsg::Disconnect).await.unwrap(); DisconnectReason::Disconnected
                }
                1 => DisconnectReason::ApplicationForcedDisconnect,
                2 | 3 => {
                    let logout = peer.read().await; assert_eq!(logout.msg_seq_num(), 2); assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(_))); sender = 3;
                    if decision == 3 {
                        assert!(!peer.task.is_finished()); peer.assert_silent();
                        peer.send(&test_helpers::logout(2)).await;
                        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logout)); target = 3; DisconnectReason::LocalRequestedLogout
                    } else { DisconnectReason::ApplicationForcedDisconnect }
                }
                _ => {
                    let reject = peer.read().await; assert_eq!(reject.msg_seq_num(), 2);
                    assert_matches!(reject.try_as_admin(), Some(AdminBase::Reject(r)) if r.session_reject_reason == Some(SessionRejectReasonBase::ValueIsIncorrect.into()));
                    let logout = peer.read().await; assert_eq!(logout.msg_seq_num(), 3);
                    assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Reset Logon acknowledgement rejected by application")));
                    sender = 4; DisconnectReason::ApplicationForcedDisconnect
                }
            };
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(actual) if actual == reason);
            let mut storage = peer.task.await.unwrap();
            if !expired { assert_eq!(decided_at.elapsed(), Duration::ZERO); }
            assert_eq!(storage.next_sender_msg_seq_num().get(), sender); assert_eq!(storage.next_target_msg_seq_num().get(), target);
            assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(test_helpers::serialize_message(&logon).as_slice()));
            assert!(storage.fetch(nz_seq(sender), nz_seq(sender)).await.is_err());
            assert!(peer.buffer.is_empty()); let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
            assert!(peer.events.try_recv().is_err());
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn reset_probe_callback_finishes_before_the_new_ack_budget_is_checked() {
    LocalSet::new().run_until(async {
        for (unlimited, delay, acknowledge) in [(false, 2, true), (false, 2, false), (false, 0, true), (true, 2, true)] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.running_session_reset_timeout = if unlimited { Duration::MAX } else { Duration::from_secs(30) };
            settings.auto_disconnect_after_no_logon_response = Duration::from_secs(10);
            let mut harness = build_harness_with_settings(settings);
            let (entered, mut started_rx) = mpsc::unbounded_channel();
            let (release, action) = mpsc::unbounded_channel();
            harness.app.admin_gate = Some(AdminGate { kind: MsgTypeBase::Heartbeat, skip: 0, entered, action });
            let mut peer = running_reset_harness(harness).await;
            let started = time::Instant::now();
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            time::advance(Duration::from_secs(29)).await;
            peer.send(&test_helpers::heartbeat(2, Some(id))).await;
            started_rx.recv().await.unwrap();
            assert_eq!(started.elapsed(), Duration::from_secs(29));
            time::advance(Duration::from_secs(delay)).await;
            release.send(InputAction::Accept).unwrap();
            let logon = peer.read().await;
            let sent_at = time::Instant::now();
            assert_eq!(started.elapsed(), Duration::from_secs(29 + delay));
            assert_eq!(logon.msg_seq_num(), 1);
            assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            if acknowledge {
                peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None)).await;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
                peer.control.send(ControlMsg::Disconnect).await.unwrap();
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
            } else {
                time::advance(Duration::from_secs(9)).await;
                task::yield_now().await;
                peer.assert_silent();
                assert!(!peer.task.is_finished());
                time::advance(Duration::from_secs(1)).await;
                let logout = peer.read().await;
                assert_eq!(sent_at.elapsed(), Duration::from_secs(10));
                assert_eq!(logout.msg_seq_num(), 2);
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::SeqNumResetFailed));
            }
            let mut storage = peer.task.await.unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), if acknowledge { 2 } else { 3 });
            assert_eq!(storage.next_target_msg_seq_num().get(), if acknowledge { 2 } else { 1 });
            assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(test_helpers::serialize_message(&logon).as_slice()));
            if acknowledge { assert!(storage.fetch(nz_seq(2), nz_seq(2)).await.is_err()); }
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn reset_probe_callback_outcomes_precede_the_expired_preparation_budget() {
    LocalSet::new().run_until(async {
        for outcome in 0..5 {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            let mut harness = build_harness_with_settings(settings);
            let (entered, mut entered_rx) = mpsc::unbounded_channel();
            let (release, action) = mpsc::unbounded_channel();
            harness.app.admin_gate = Some(AdminGate { kind: MsgTypeBase::Heartbeat, skip: 0, entered, action });
            let mut peer = running_reset_harness(harness).await;
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            let stale = outcome == 4;
            if stale {
                peer.send(&test_helpers::resend_request(2, 1, 2)).await;
                let gap = peer.read().await;
                assert_eq!(gap.msg_seq_num(), 1);
                assert_eq!(gap.poss_dup_flag(), Some(true));
                assert_matches!(gap.try_as_admin(), Some(AdminBase::SequenceReset(s)) if s.new_seq_no == 3 && s.gap_fill_flag == Some(true));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest));
            }
            time::advance(Duration::from_secs(29)).await;
            peer.send(&test_helpers::heartbeat(if stale { 3 } else { 2 }, Some(id))).await;
            entered_rx.recv().await.unwrap();
            time::advance(Duration::from_secs(2)).await;
            let action = match outcome {
                0 => InputAction::Disconnect,
                1 | 2 => InputAction::Logout { session_status: None, text: Some(fix_str!("Application logout").to_owned()), disconnect: outcome == 1 },
                3 => InputAction::Reject { reason: SessionRejectReasonBase::ValueIsIncorrect.into(), tag: None, text: Some(fix_str!("Application reject").to_owned()) },
                _ => InputAction::Accept,
            };
            release.send(action).unwrap();
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            if outcome == 3 {
                let reject = peer.read().await;
                assert_eq!(reject.msg_seq_num(), 3);
                assert_matches!(reject.try_as_admin(), Some(AdminBase::Reject(r)) if r.text.as_deref() == Some(fix_str!("Application reject")));
            }
            if outcome != 0 {
                let logout = peer.read().await;
                assert_eq!(logout.msg_seq_num(), if outcome == 3 { 4 } else { 3 });
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(if outcome < 3 { fix_str!("Application logout") } else { fix_str!("Sequence number reset not acknowledged") }));
            }
            if outcome == 2 {
                assert!(!peer.task.is_finished());
                peer.send(&test_helpers::logout(3)).await;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logout));
            }
            let expected = match outcome { 0 | 1 => DisconnectReason::ApplicationForcedDisconnect, 2 => DisconnectReason::LocalRequestedLogout, _ => DisconnectReason::ResetPreparationTimeout };
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(reason) if reason == expected);
            let mut storage = peer.task.await.unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), match outcome { 0 => 3, 3 => 5, _ => 4 });
            assert_eq!(storage.next_target_msg_seq_num().get(), if outcome == 2 || stale { 4 } else { 3 });
            for seq in [1, 2] { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_ok()); }
            let mut remainder = Vec::new(); peer.wire.read_to_end(&mut remainder).await.unwrap();
            assert!(remainder.is_empty()); assert!(peer.buffer.is_empty());
        }
    }).await;
}
