use std::{
    assert_matches,
    cell::Cell,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    fix_str,
    message::{HeaderAccess, SessionMessage},
};
use easyfix_test_messages::{Body, Message};
use futures_util::FutureExt;
use tokio::{
    io,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf},
    task,
    task::LocalSet,
    time,
};

use super::{
    harness::{TestEvent, build_harness_with_settings},
    reset_peer::{running_reset_peer, running_reset_transport},
    reset_transport::gated_reset_peer,
    wire::build_order_missing_symbol_bytes,
};
use crate::{
    application::DisconnectReason, io::ControlMsg, messages_storage::MessagesStorage, test_helpers,
    test_helpers::nz_seq,
};

#[tokio::test(start_paused = true)]
async fn unrepresentable_reset_budgets_leave_deadlines_unarmed() {
    LocalSet::new().run_until(async {
        for (unlimited_preparation, acknowledge) in [(true, true), (true, false), (false, true)] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.running_session_reset_timeout = if unlimited_preparation { Duration::MAX } else { Duration::from_secs(30) };
            settings.auto_disconnect_after_no_logon_response = if unlimited_preparation { Duration::from_secs(10) } else { Duration::MAX };
            let mut peer = running_reset_peer(settings).await;
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let mut id = peer.probe().await;
            let mut answer_seq = 2;
            if unlimited_preparation {
                peer.send(&test_helpers::resend_request(2, 1, 2)).await;
                let gap = peer.read().await;
                assert_eq!(gap.msg_seq_num(), 1); assert_eq!(gap.poss_dup_flag(), Some(true));
                assert_matches!(gap.try_as_admin(), Some(AdminBase::SequenceReset(r)) if r.gap_fill_flag == Some(true) && r.new_seq_no == 3);
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest));
                time::advance(Duration::from_secs(100)).await;
                peer.send(&test_helpers::heartbeat(3, Some(id.clone()))).await;
                let next = peer.probe().await; assert_ne!(next, id); id = next;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                answer_seq = 4;
            }
            peer.send(&test_helpers::heartbeat(answer_seq, Some(id))).await;
            let logon = peer.read().await;
            assert_eq!(logon.msg_seq_num(), 1);
            assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            if !unlimited_preparation {
                for _ in 0..3 {
                    time::advance(Duration::from_secs(40)).await;
                    peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
                    task::yield_now().await;
                    assert!(!peer.task.is_finished()); peer.assert_silent();
                }
            }
            if acknowledge {
                peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None)).await;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
                peer.control.send(ControlMsg::Disconnect).await.unwrap();
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
            } else {
                time::advance(Duration::from_secs(9)).await;
                task::yield_now().await; assert!(!peer.task.is_finished()); peer.assert_silent();
                time::advance(Duration::from_secs(1)).await;
                let logout = peer.read().await;
                assert_eq!(logout.msg_seq_num(), 2);
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::SeqNumResetFailed));
            }
            let mut storage = peer.task.await.unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), if acknowledge { 2 } else { 3 });
            assert_eq!(storage.next_target_msg_seq_num().get(), if acknowledge { 2 } else { 1 });
            assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(test_helpers::serialize_message(&logon).as_slice()));
            assert!(storage.fetch(nz_seq(3), nz_seq(3)).await.is_err());
            assert!(peer.buffer.is_empty());
            let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
            assert!(peer.events.try_recv().is_err());
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn reset_ack_budget_includes_the_logon_write_and_starts_after_preparation() {
    LocalSet::new().run_until(async {
        for (write_delay, ack_at) in [(0, Some(11)), (0, Some(20)), (9, None)] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.write_timeout = Duration::from_secs(50);
            settings.auto_disconnect_after_no_logon_response = Duration::from_secs(10);
            let (mut peer, mut gate) = gated_reset_peer(settings).await;
            let started = time::Instant::now();
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            time::advance(Duration::from_secs(9)).await;
            gate.armed.set(true);
            peer.send(&test_helpers::heartbeat(2, Some(id))).await;
            let pending = gate.entered.recv().await.unwrap();
            let pending = Message::from_bytes(&pending).unwrap();
            assert_eq!(pending.msg_seq_num(), 1);
            assert_matches!(pending.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
            time::advance(Duration::from_secs(write_delay)).await;
            gate.armed.set(false); gate.release.send(Ok(())).unwrap();
            let logon = peer.read().await;
            assert_eq!(logon.msg_seq_num(), 1);
            assert_eq!(started.elapsed(), Duration::from_secs(9 + write_delay));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            if ack_at == Some(11) {
                time::advance(Duration::from_secs(2)).await;
                peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None)).await;
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
                peer.control.send(ControlMsg::Disconnect).await.unwrap();
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
            } else {
                let before_deadline = 18 - (9 + write_delay);
                time::advance(Duration::from_secs(before_deadline)).await;
                task::yield_now().await; peer.assert_silent(); assert!(!peer.task.is_finished());
                time::advance(Duration::from_secs(1)).await;
                let logout = peer.read().await;
                assert_eq!(started.elapsed(), Duration::from_secs(19));
                assert_eq!(logout.msg_seq_num(), 2);
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::SeqNumResetFailed));
                if ack_at.is_some() {
                    time::advance(Duration::from_secs(1)).await; assert!(peer.task.is_finished());
                    let late_ack = test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), None);
                    assert!(peer.wire.write_all(&test_helpers::serialize_message(&late_ack)).await.is_err());
                    assert!(peer.events.try_recv().is_err());
                }
            }
            let storage = peer.task.await.unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), if ack_at == Some(11) { 2 } else { 3 });
            assert_eq!(storage.next_target_msg_seq_num().get(), if ack_at == Some(11) { 2 } else { 1 });
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn reset_deadline_after_read_prevents_late_message_and_decode_dispatch() {
    LocalSet::new().run_until(async {
        for late_at in [30, 31] {
            for decoded in [false, true] {
                let mut settings = test_helpers::default_session_settings();
                settings.heartbeat_interval = None;
                let harness = build_harness_with_settings(settings);
                let (server, wire) = io::duplex(65536);
                let (reader, writer) = io::split(server);
                let advance = Rc::new(Cell::new(None));
                let completed_late_reads = Rc::new(Cell::new(0));
                let reader = AdvancingReader { inner: reader, advance: advance.clone(), completed_late_reads: completed_late_reads.clone() };
                let mut peer = running_reset_transport(harness, wire, reader, writer).await;
                let started = time::Instant::now();
                peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
                let id = peer.probe().await;
                time::advance(Duration::from_secs(29)).await;
                advance.set(Some(Duration::from_secs(late_at - 29)));
                if decoded {
                    peer.send(&test_helpers::heartbeat(2, Some(id.clone()))).await;
                } else {
                    peer.wire.write_all(&build_order_missing_symbol_bytes(2)).await.unwrap();
                }
                let logout = peer.read().await;
                assert_eq!(started.elapsed(), Duration::from_secs(late_at));
                assert!(advance.get().is_none());
                assert_eq!(completed_late_reads.get(), 1);
                assert_eq!(logout.msg_seq_num(), 3);
                assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::ResetPreparationTimeout));
                let mut storage = peer.task.await.unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 4);
                assert_eq!(storage.next_target_msg_seq_num().get(), 2);
                let first = Message::from_bytes(storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap()).unwrap();
                assert_matches!(first.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag.is_none());
                let probe = Message::from_bytes(storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap()).unwrap();
                assert_matches!(probe.try_as_admin(), Some(AdminBase::TestRequest(t)) if t.test_req_id.as_ref() == &*id);
                assert_eq!(storage.fetch(nz_seq(3), nz_seq(3)).await, Ok(test_helpers::serialize_message(&logout).as_slice()));
                assert!(peer.buffer.is_empty());
                let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
                assert!(peer.events.try_recv().is_err());
            }
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn delayed_reset_ack_keeps_short_heartbeat_timers_silent() {
    LocalSet::new().run_until(async {
        let mut settings = test_helpers::default_session_settings();
        settings.heartbeat_interval = Some(1.try_into().unwrap());
        let mut peer = running_reset_peer(settings).await;
        peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
        let id = peer.probe().await;
        peer.send(&test_helpers::heartbeat(2, Some(id))).await;
        let logon = peer.read().await;
        assert_eq!(logon.msg_seq_num(), 1);
        assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true) && l.heart_bt_int == 1);
        for _ in 0..5 {
            time::advance(Duration::from_secs(1)).await;
            task::yield_now().await;
            peer.assert_silent();
            assert!(!peer.task.is_finished());
        }
        peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 1, Some(true), None)).await;
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Logon));
        peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
        let order = peer.read().await;
        assert_eq!(order.msg_seq_num(), 2);
        assert_matches!(&*order.body, Body::NewOrderSingle(_));
        peer.control.send(ControlMsg::Disconnect).await.unwrap();
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
        let storage = peer.task.await.unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
        assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    }).await;
}

#[tokio::test(start_paused = true)]
async fn running_session_reset_times_out_without_heartbeat_interval() {
    LocalSet::new().run_until(async {
        for answer_probe in [false, true] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.running_session_reset_timeout = Duration::from_secs(30);
            settings.auto_disconnect_after_no_logon_response = Duration::from_secs(10);
            let mut peer = running_reset_peer(settings).await;
            let started = time::Instant::now();
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            assert_eq!(time::Instant::now(), started);
            if answer_probe {
                peer.send(&test_helpers::heartbeat(2, Some(id))).await;
                let logon = peer.read().await;
                assert_eq!(logon.msg_seq_num(), 1);
                assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true) && l.heart_bt_int == 0);
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            }
            let budget = if answer_probe { 10 } else { 30 };
            time::advance(Duration::from_secs(budget - 1)).await;
            task::yield_now().await;
            assert!(!peer.task.is_finished());
            time::advance(Duration::from_secs(1)).await;
            let logout = peer.read().await;
            assert_eq!(logout.msg_seq_num(), if answer_probe { 2 } else { 3 });
            assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(reason) if reason == if answer_probe { DisconnectReason::SeqNumResetFailed } else { DisconnectReason::ResetPreparationTimeout });
            let mut storage = peer.task.await.unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), if answer_probe { 3 } else { 4 });
            assert_eq!(storage.next_target_msg_seq_num().get(), if answer_probe { 1 } else { 2 });
            assert!(storage.fetch(nz_seq(1), nz_seq(1)).await.is_ok());
            assert!(storage.fetch(nz_seq(2), nz_seq(2)).await.is_ok());
        }
    }).await;
}

struct AdvancingReader<R> {
    inner: R,
    advance: Rc<Cell<Option<Duration>>>,
    completed_late_reads: Rc<Cell<usize>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for AdvancingReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before
            && let Some(duration) = self.advance.take()
        {
            let before = time::Instant::now();
            // Tokio advances the paused clock on the first poll, before
            // yielding. Keep this read Ready so its select arm wins.
            assert!(time::advance(duration).now_or_never().is_none());
            assert_eq!(time::Instant::now(), before + duration);
            assert_matches!(result, Poll::Ready(Ok(())));
            self.completed_late_reads
                .set(self.completed_late_reads.get() + 1);
        }
        result
    }
}
