use std::{assert_matches, time::Duration};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    fix_str,
    message::{HeaderAccess, SessionMessage},
};
use easyfix_test_messages::{Body, Message};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc::error::TryRecvError,
    task,
    task::LocalSet,
    time,
};

use super::{
    harness::{TestEvent, build_harness_with_settings},
    reset_peer::running_reset_peer,
    reset_transport::{gated_reset_harness, gated_reset_peer},
};
use crate::{
    application::DisconnectReason, io::ControlMsg, messages_storage::MessagesStorage, test_helpers,
    test_helpers::nz_seq,
};

#[tokio::test(start_paused = true)]
async fn reset_preparation_keeps_heartbeats_alive_behind_an_old_probe() {
    LocalSet::new().run_until(async {
        let mut settings = test_helpers::default_session_settings();
        settings.heartbeat_interval = Some(1.try_into().unwrap());
        let mut peer = running_reset_peer(settings).await;
        time::advance(Duration::from_secs(1)).await;
        let heartbeat = peer.read().await;
        assert_eq!(heartbeat.msg_seq_num(), 2);
        assert_matches!(heartbeat.try_as_admin(), Some(AdminBase::Heartbeat(h)) if h.test_req_id.is_none());
        time::advance(Duration::from_millis(200)).await;
        let keepalive = peer.read().await;
        assert_eq!(keepalive.msg_seq_num(), 3);
        let Some(AdminBase::TestRequest(request)) = keepalive.try_as_admin() else { panic!("expected keepalive probe"); };
        let old_id = request.test_req_id.into_owned();
        peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
        task::yield_now().await;
        peer.assert_silent();
        peer.send(&test_helpers::test_request(2, fix_str!("PEER"))).await;
        let response = peer.read().await;
        assert_eq!(response.msg_seq_num(), 4);
        assert_matches!(response.try_as_admin(), Some(AdminBase::Heartbeat(h)) if h.test_req_id.as_deref() == Some(fix_str!("PEER")));
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::TestRequest));
        for seq in 5..=7 {
            time::advance(Duration::from_secs(1)).await;
            let heartbeat = peer.read().await;
            assert_eq!(heartbeat.msg_seq_num(), seq);
            assert_matches!(heartbeat.try_as_admin(), Some(AdminBase::Heartbeat(h)) if h.test_req_id.is_none());
            peer.assert_silent(); assert!(!peer.task.is_finished());
        }
        let before = time::Instant::now();
        peer.send(&test_helpers::heartbeat(3, Some(old_id.clone()))).await;
        let probe = peer.read().await;
        assert_eq!(probe.msg_seq_num(), 8);
        let Some(AdminBase::TestRequest(request)) = probe.try_as_admin() else { panic!("expected reset probe after keepalive response"); };
        let id = request.test_req_id.into_owned();
        assert_ne!(id, old_id); assert_eq!(time::Instant::now(), before);
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
        peer.send(&test_helpers::heartbeat(4, Some(id))).await;
        let logon = peer.read().await;
        assert_eq!(logon.msg_seq_num(), 1);
        assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true) && l.heart_bt_int == 1);
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
        peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 1, Some(true), None)).await;
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
        peer.control.send(ControlMsg::Disconnect).await.unwrap();
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
        let mut storage = peer.task.await.unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 2); assert_eq!(storage.next_target_msg_seq_num().get(), 2);
        assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(test_helpers::serialize_message(&logon).as_slice()));
        for seq in 2..=8 { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_err()); }
        assert!(peer.buffer.is_empty());
        let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
    }).await;
}

#[tokio::test(start_paused = true)]
async fn queued_probe_answer_waits_for_replay_and_the_trailing_gap_fill() {
    LocalSet::new().run_until(async {
        for expire_in_trailing_gap in [false, true] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.write_timeout = Duration::from_secs(60);
            let (mut peer, mut gate) = gated_reset_peer(settings).await;
            for seq in 2..=6 {
                peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
                assert_eq!(peer.read().await.msg_seq_num(), seq);
            }
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            peer.send(&test_helpers::heartbeat(3, Some(id.clone()))).await;
            let request = peer.read().await;
            assert_eq!(request.msg_seq_num(), 8);
            assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r)) if r.begin_seq_no == 2 && r.end_seq_no == 2);
            let mut replay_request = test_helpers::resend_request(2, 2, 7);
            replay_request.header.poss_dup_flag = Some(true);
            replay_request.header.orig_sending_time = Some(replay_request.header.sending_time);
            gate.armed.set(true);
            peer.send(&replay_request).await;
            for seq in 2..=7 {
                let pending = Message::from_bytes(&gate.entered.recv().await.unwrap()).unwrap();
                assert_eq!(pending.msg_seq_num(), seq);
                assert_eq!(pending.poss_dup_flag(), Some(true));
                if seq < 7 { assert_matches!(&*pending.body, Body::NewOrderSingle(_)); }
                else {
                    assert_matches!(pending.try_as_admin(), Some(AdminBase::SequenceReset(r)) if r.gap_fill_flag == Some(true) && r.new_seq_no == 8);
                    if expire_in_trailing_gap { time::advance(Duration::from_secs(31)).await; }
                    gate.armed.set(false);
                }
                gate.release.send(Ok(())).unwrap();
                let actual = peer.read().await;
                assert_eq!(test_helpers::serialize_message(&actual), test_helpers::serialize_message(&pending));
            }
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest));
            if expire_in_trailing_gap {
                let logout = peer.read().await;
                assert_eq!(logout.msg_seq_num(), 9);
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::ResetPreparationTimeout));
                let mut storage = peer.task.await.unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 10); assert_eq!(storage.next_target_msg_seq_num().get(), 3);
                for seq in 1..=9 { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_ok()); }
            } else {
                let before = time::Instant::now();
                let next = peer.read().await;
                assert_eq!(next.msg_seq_num(), 9);
                let Some(AdminBase::TestRequest(probe)) = next.try_as_admin() else { panic!("expected fresh probe after the full replay"); };
                let next_id = probe.test_req_id.into_owned();
                assert_ne!(next_id, id); assert_eq!(time::Instant::now(), before);
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                peer.send(&test_helpers::heartbeat(4, Some(next_id))).await;
                let logon = peer.read().await;
                assert_eq!(logon.msg_seq_num(), 1);
                assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None)).await;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
                peer.control.send(ControlMsg::Disconnect).await.unwrap();
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
                let mut storage = peer.task.await.unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 2); assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(test_helpers::serialize_message(&logon).as_slice()));
                for seq in 2..=9 { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_err()); }
            }
            assert!(peer.buffer.is_empty());
            let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
            assert!(peer.events.try_recv().is_err());
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn queued_admin_flush_precedes_reprobe_and_checks_the_original_budget() {
    LocalSet::new().run_until(async {
        for expire_during_flush in [false, true] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.queued_batch_size = 2.try_into().unwrap();
            settings.write_timeout = Duration::from_secs(60);
            let mut harness = build_harness_with_settings(settings);
            harness.storage.set_next_sender_msg_seq_num(nz_seq(98)).unwrap();
            harness.storage.set_next_target_msg_seq_num(nz_seq(501)).unwrap();
            let (mut peer, mut gate) = gated_reset_harness(harness).await;
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            let mut garbled = test_helpers::serialize_message(&test_helpers::new_order_single(502));
            let checksum_digit = garbled.len() - 2;
            garbled[checksum_digit] = if garbled[checksum_digit] == b'0' { b'1' } else { b'0' };
            peer.wire.write_all(&garbled).await.unwrap();
            peer.send(&test_helpers::test_request(503, fix_str!("PEER"))).await;
            peer.send(&test_helpers::heartbeat(504, Some(id.clone()))).await;
            let request = peer.read().await;
            assert_eq!(request.msg_seq_num(), 100);
            assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r)) if r.begin_seq_no == 502 && r.end_seq_no == 502);
            assert_matches!(peer.events.try_recv(), Err(TryRecvError::Empty));
            if expire_during_flush { time::advance(Duration::from_secs(29)).await; }
            gate.armed.set(true);
            let mut replay = test_helpers::new_order_single(502);
            replay.header.poss_dup_flag = Some(true);
            replay.header.orig_sending_time = Some(replay.header.sending_time);
            peer.send(&replay).await;
            let pending = Message::from_bytes(&gate.entered.recv().await.unwrap()).unwrap();
            assert_eq!(pending.msg_seq_num(), 101);
            assert_matches!(pending.try_as_admin(), Some(AdminBase::Heartbeat(h)) if h.test_req_id.as_deref() == Some(fix_str!("PEER")));
            if expire_during_flush { time::advance(Duration::from_secs(2)).await; }
            gate.armed.set(false); gate.release.send(Ok(())).unwrap();
            let heartbeat = peer.read().await;
            assert_eq!(test_helpers::serialize_message(&heartbeat), test_helpers::serialize_message(&pending));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AppMsgIn);
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::TestRequest));
            if expire_during_flush {
                let logout = peer.read().await;
                assert_eq!(logout.msg_seq_num(), 102);
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::ResetPreparationTimeout));
                let mut storage = peer.task.await.unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 103);
                assert_eq!(storage.next_target_msg_seq_num().get(), 504);
                for seq in 98..=102 { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_ok()); }
                assert_eq!(storage.fetch(nz_seq(101), nz_seq(101)).await, Ok(test_helpers::serialize_message(&heartbeat).as_slice()));
            } else {
                let before = time::Instant::now();
                let next = peer.read().await;
                assert_eq!(next.msg_seq_num(), 102);
                let Some(AdminBase::TestRequest(probe)) = next.try_as_admin() else { panic!("expected fresh probe"); };
                let next_id = probe.test_req_id.into_owned();
                assert_ne!(next_id, id); assert_eq!(time::Instant::now(), before);
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                peer.send(&test_helpers::heartbeat(505, Some(next_id))).await;
                let logon = peer.read().await;
                assert_eq!(logon.msg_seq_num(), 1);
                assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None)).await;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
                peer.control.send(ControlMsg::Disconnect).await.unwrap();
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
                let mut storage = peer.task.await.unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 2); assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                for seq in 98..=102 { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_err()); }
            }
            assert!(peer.buffer.is_empty());
            let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
            assert!(peer.events.try_recv().is_err());
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn running_reset_replay_barriers_and_budget_check_every_completed_batch() {
    LocalSet::new().run_until(async {
        for reset_before_replay in [false, true] {
            for expire_after in [None, Some(3), Some(6)] {
                let mut settings = test_helpers::default_session_settings();
                settings.heartbeat_interval = None;
                settings.resend_batch_size = 1.try_into().unwrap();
                settings.write_timeout = Duration::from_secs(60);
                let (mut peer, mut gate) = gated_reset_peer(settings).await;
                for seq in 2..=6 {
                    peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
                    assert_eq!(peer.read().await.msg_seq_num(), seq);
                }
                let first_id = if reset_before_replay {
                    peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
                    Some(peer.probe().await)
                } else { None };
                gate.armed.set(true);
                peer.send(&test_helpers::resend_request(2, 2, 6)).await;
                if let Some(id) = &first_id { peer.send(&test_helpers::heartbeat(3, Some(id.clone()))).await; }
                for seq in 2..=6 {
                    let pending = Message::from_bytes(&gate.entered.recv().await.unwrap()).unwrap();
                    assert_eq!(pending.msg_seq_num(), seq);
                    assert_eq!(pending.poss_dup_flag(), Some(true));
                    assert_matches!(&*pending.body, Body::NewOrderSingle(_));
                    if !reset_before_replay && seq == 2 {
                        peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
                    }
                    time::advance(Duration::from_secs(if expire_after == Some(seq) { 31 } else { 1 })).await;
                    if seq == 6 || expire_after == Some(seq) { gate.armed.set(false); }
                    gate.release.send(Ok(())).unwrap();
                    let replay = peer.read().await;
                    assert_eq!(replay.msg_seq_num(), seq);
                    assert_eq!(replay.poss_dup_flag(), Some(true));
                    if expire_after == Some(seq) { break; }
                }
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest));
                if expire_after.is_some() {
                    let logout = peer.read().await;
                    assert_eq!(logout.msg_seq_num(), if reset_before_replay { 8 } else { 7 });
                    assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
                    assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::ResetPreparationTimeout));
                    let mut storage = peer.task.await.unwrap();
                    assert_eq!(storage.next_sender_msg_seq_num().get(), if reset_before_replay { 9 } else { 8 });
                    assert_eq!(storage.next_target_msg_seq_num().get(), 3);
                    for seq in [2, 6] { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_ok()); }
                    assert!(peer.buffer.is_empty());
                    let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
                } else {
                    let before_probe = time::Instant::now();
                    let id = peer.probe().await;
                    assert_eq!(time::Instant::now(), before_probe);
                    if let Some(first_id) = first_id {
                        assert_ne!(id, first_id);
                        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                    }
                    peer.send(&test_helpers::heartbeat(if reset_before_replay { 4 } else { 3 }, Some(id))).await;
                    let logon = peer.read().await;
                    assert_eq!(logon.msg_seq_num(), 1);
                    assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
                    assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                    peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None)).await;
                    assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
                    peer.control.send(ControlMsg::Disconnect).await.unwrap();
                    assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
                    let mut storage = peer.task.await.unwrap();
                    assert_eq!(storage.next_sender_msg_seq_num().get(), 2); assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                    assert!(storage.fetch(nz_seq(6), nz_seq(6)).await.is_err());
                    assert!(peer.buffer.is_empty());
                }
            }
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn repeated_replays_and_controls_share_the_original_preparation_budget() {
    LocalSet::new().run_until(async {
        let mut settings = test_helpers::default_session_settings();
        settings.heartbeat_interval = None;
        settings.resend_batch_size = 1.try_into().unwrap();
        let (mut peer, mut gate) = gated_reset_peer(settings).await;
        for seq in 2..=6 {
            peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
            assert_eq!(peer.read().await.msg_seq_num(), seq);
        }
        let started = time::Instant::now();
        peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
        let mut id = peer.probe().await;
        for cycle in 0..6 {
            gate.armed.set(true);
            peer.send(&test_helpers::resend_request(2 + 2 * cycle, 2, 6)).await;
            peer.send(&test_helpers::heartbeat(3 + 2 * cycle, Some(id.clone()))).await;
            for seq in 2..=6 {
                let pending = Message::from_bytes(&gate.entered.recv().await.unwrap()).unwrap();
                assert_eq!(pending.msg_seq_num(), seq);
                assert_eq!(pending.poss_dup_flag(), Some(true));
                if seq == 2 { peer.control.send(ControlMsg::ResetRunningSession).await.unwrap(); }
                time::advance(Duration::from_secs(1)).await;
                if seq == 6 { gate.armed.set(false); }
                gate.release.send(Ok(())).unwrap();
                let replay = peer.read().await;
                assert_eq!(replay.msg_seq_num(), seq); assert_eq!(replay.poss_dup_flag(), Some(true));
            }
            assert_eq!(started.elapsed(), Duration::from_secs(5 * u64::from(cycle + 1)));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest));
            if cycle < 5 {
                let next = peer.probe().await;
                assert_ne!(next, id); id = next;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            } else {
                let logout = peer.read().await;
                assert_eq!(logout.msg_seq_num(), 13);
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
            }
        }
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::ResetPreparationTimeout));
        let mut storage = peer.task.await.unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 14);
        assert_eq!(storage.next_target_msg_seq_num().get(), 13);
        for seq in [2, 6] { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_ok()); }
        assert!(peer.buffer.is_empty());
        let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
    }).await;
}
