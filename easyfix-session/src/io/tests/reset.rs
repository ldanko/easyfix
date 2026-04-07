use std::{
    assert_matches,
    cell::Cell,
    io::ErrorKind,
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
use tokio::{
    io,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf},
    sync::mpsc,
    task,
    task::LocalSet,
    time,
};

use super::{
    harness::{TestEvent, build_harness_with_settings},
    reset_peer::{running_reset_peer, running_reset_transport},
    reset_transport::{GateWriter, WriteController, gated_reset_peer},
};
use crate::{
    application::DisconnectReason,
    initiator::SessionStart,
    io::{ControlMsg, InputStream, SessionOpening, session_loop},
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{DEFAULT_MAX_MESSAGE_SIZE, nz_seq},
};

#[tokio::test(start_paused = true)]
async fn failed_reset_logon_writes_keep_the_committed_reset_and_failure_reason() {
    LocalSet::new().run_until(async {
        for at_connect in [false, true] {
            for write_timeout in [false, true] {
                let mut settings = test_helpers::default_session_settings();
                settings.heartbeat_interval = None;
                settings.write_timeout = Duration::from_secs(5);
                let (mut events, task, mut wire, mut buffer, mut gate) = if at_connect {
                    let mut harness = build_harness_with_settings(settings);
                    harness.storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
                    harness.storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
                    let (server, wire) = io::duplex(65536); let (reader, writer) = io::split(server);
                    let armed = Rc::new(Cell::new(true));
                    let (entered, observed) = mpsc::unbounded_channel();
                    let (release, actions) = mpsc::unbounded_channel();
                    let writer = GateWriter { inner: writer, armed: armed.clone(), entered, release: actions, waiting: false, approved: false };
                    let events = harness.events_rx;
                    let task = task::spawn_local(async move {
                        let mut storage = harness.storage;
                        session_loop(SessionOpening::SendLogon(SessionStart::Reset), InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE), writer, harness.engine, &mut storage, harness.app, harness.sender, harness.app_rx, harness.control_rx).await;
                        storage
                    });
                    (events, task, wire, Vec::new(), WriteController { armed, entered: observed, release })
                } else {
                    let (mut peer, gate) = gated_reset_peer(settings).await;
                    peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
                    assert_eq!(peer.read().await.msg_seq_num(), 2);
                    peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
                    let id = peer.probe().await;
                    gate.armed.set(true);
                    peer.send(&test_helpers::heartbeat(2, Some(id))).await;
                    assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
                    assert_eq!(peer.sender.backlog_len(), 0);
                    (peer.events, peer.task, peer.wire, peer.buffer, gate)
                };
                let pending_bytes = gate.entered.recv().await.unwrap();
                let pending = Message::from_bytes(&pending_bytes).unwrap();
                assert_eq!(pending.msg_seq_num(), 1);
                assert_matches!(pending.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
                if write_timeout { time::advance(Duration::from_secs(5)).await; }
                else { gate.release.send(Err(ErrorKind::BrokenPipe)).unwrap(); }
                assert_matches!(events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::SeqNumResetFailed));
                let mut storage = task.await.unwrap();
                assert_eq!(storage.next_sender_msg_seq_num().get(), 2); assert_eq!(storage.next_target_msg_seq_num().get(), 1);
                assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(pending_bytes.as_slice()));
                for seq in [2, 3, 40] { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_err()); }
                wire.read_to_end(&mut buffer).await.unwrap(); assert!(buffer.is_empty());
                assert!(events.try_recv().is_err());
            }
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn count_cap_still_wakes_after_logout_during_an_unconfirmed_reset() {
    LocalSet::new().run_until(async {
        let mut settings = test_helpers::default_session_settings(); settings.heartbeat_interval = None;
        settings.max_outbound_queued_messages = Some(1.try_into().unwrap());
        settings.auto_disconnect_after_no_logout = Duration::MAX;
        let mut peer = running_reset_peer(settings).await;
        peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
        let id = peer.probe().await; peer.send(&test_helpers::heartbeat(2, Some(id))).await;
        let logon = peer.read().await;
        assert_eq!(logon.msg_seq_num(), 1); assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
        assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
        peer.control.send(ControlMsg::Logout { session_status: None, text: None }).await.unwrap();
        let logout = peer.read().await;
        assert_eq!(logout.msg_seq_num(), 2); assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(_)));
        task::yield_now().await;
        peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
        task::yield_now().await;
        assert!(!peer.task.is_finished()); peer.assert_silent();
        peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
        let ended = time::timeout(Duration::from_secs(1), peer.events.recv()).await.expect("count cap must wake without any session timer").unwrap();
        assert_matches!(ended, TestEvent::SessionEnd(DisconnectReason::SeqNumResetFailed));
        let mut storage = peer.task.await.unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 5); assert_eq!(storage.next_target_msg_seq_num().get(), 1);
        assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(test_helpers::serialize_message(&logon).as_slice()));
        assert_eq!(storage.fetch(nz_seq(2), nz_seq(2)).await, Ok(test_helpers::serialize_message(&logout).as_slice()));
        for seq in [3, 4] {
            let order = Message::from_bytes(storage.fetch(nz_seq(seq), nz_seq(seq)).await.unwrap()).unwrap();
            assert_eq!(order.msg_seq_num(), seq); assert_matches!(&*order.body, Body::NewOrderSingle(_));
        }
        assert!(peer.buffer.is_empty()); let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
        assert!(peer.events.try_recv().is_err());
    }).await;
}

#[tokio::test(start_paused = true)]
async fn reset_send_gates_preserve_disconnect_storage_and_the_count_cap() {
    LocalSet::new().run_until(async {
        for (sent, count_cap) in [(false, true), (true, true), (true, false)] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.max_outbound_queued_messages = Some(1.try_into().unwrap());
            let mut peer = running_reset_peer(settings).await;
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            if sent {
                peer.send(&test_helpers::heartbeat(2, Some(id))).await;
                let logon = peer.read().await;
                assert_eq!(logon.msg_seq_num(), 1);
                assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            }
            let count = if count_cap { 2 } else { 1 };
            for _ in 0..count { peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap(); }
            if !count_cap { peer.control.send(ControlMsg::Disconnect).await.unwrap(); }
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(reason) if reason == if sent { DisconnectReason::SeqNumResetFailed } else { DisconnectReason::SlowConsumer });
            let mut storage = peer.task.await.unwrap();
            let first = if sent { 2 } else { 3 };
            assert_eq!(storage.next_sender_msg_seq_num().get(), first + count);
            assert_eq!(storage.next_target_msg_seq_num().get(), if sent { 1 } else { 2 });
            for seq in first..first + count {
                let order = Message::from_bytes(storage.fetch(nz_seq(seq), nz_seq(seq)).await.unwrap()).unwrap();
                assert_eq!(order.msg_seq_num(), seq); assert_matches!(&*order.body, Body::NewOrderSingle(_));
                assert_eq!(order.poss_dup_flag(), None);
            }
            let initial = Message::from_bytes(storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap()).unwrap();
            assert_matches!(initial.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == if sent { Some(true) } else { None });
            assert!(storage.fetch(nz_seq(first + count), nz_seq(first + count)).await.is_err());
            assert!(peer.buffer.is_empty());
            let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
            assert!(peer.events.try_recv().is_err());
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn eof_and_read_failure_after_reset_logon_preserve_unconfirmed_history() {
    LocalSet::new().run_until(async {
        for fail_read in [false, true] {
            let mut settings = test_helpers::default_session_settings(); settings.heartbeat_interval = None;
            let harness = build_harness_with_settings(settings);
            let (server, wire) = io::duplex(65536); let (reader, writer) = io::split(server);
            let (fault, error) = mpsc::unbounded_channel();
            let mut peer = running_reset_transport(harness, wire, FaultReader { inner: reader, error }, writer).await;
            peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
            assert_eq!(peer.read().await.msg_seq_num(), 2);
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            let id = peer.probe().await;
            peer.send(&test_helpers::heartbeat(2, Some(id))).await;
            let logon = peer.read().await;
            assert_eq!(logon.msg_seq_num(), 1);
            assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat));
            assert_eq!(peer.sender.backlog_len(), 0);
            if fail_read { fault.send(ErrorKind::ConnectionReset).unwrap(); } else { peer.wire.shutdown().await.unwrap(); }
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::SeqNumResetFailed));
            let mut storage = peer.task.await.unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), 2); assert_eq!(storage.next_target_msg_seq_num().get(), 1);
            assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await, Ok(test_helpers::serialize_message(&logon).as_slice()));
            for seq in [2, 3] { assert!(storage.fetch(nz_seq(seq), nz_seq(seq)).await.is_err()); }
            assert!(peer.buffer.is_empty());
            let mut rest = Vec::new(); peer.wire.read_to_end(&mut rest).await.unwrap(); assert!(rest.is_empty());
            assert!(peer.events.try_recv().is_err());
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn running_session_reset_holds_app_sends_and_gap_fills_the_ack_range() {
    LocalSet::new().run_until(async {
        for next_expected in [None, Some(1)] {
            let mut settings = test_helpers::default_session_settings();
            settings.heartbeat_interval = None;
            settings.enable_next_expected_msg_seq_num = true;
            settings.max_outbound_lag = Some(Duration::from_secs(1));
            let mut peer = running_reset_peer(settings).await;
            peer.send(&test_helpers::heartbeat(3, None)).await;
            let rr = peer.read().await;
            assert_eq!(rr.msg_seq_num(), 2);
            assert_matches!(rr.try_as_admin(), Some(AdminBase::ResendRequest(r)) if r.begin_seq_no == 2 && r.end_seq_no == 2);
            peer.control.send(ControlMsg::ResetRunningSession).await.unwrap();
            task::yield_now().await;
            peer.sender.send(test_helpers::new_order_single_with_empty_header()).unwrap();
            time::advance(Duration::from_secs(3)).await;
            task::yield_now().await;
            peer.assert_silent();
            assert!(!peer.task.is_finished());
            let before_probe = time::Instant::now();
            peer.send(&test_helpers::heartbeat(2, None)).await;
            let id = peer.probe().await;
            assert_eq!(time::Instant::now(), before_probe);
            peer.send(&test_helpers::heartbeat(4, Some(id))).await;
            let logon = peer.read().await;
            assert_eq!(logon.msg_seq_num(), 1);
            assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true) && l.next_expected_msg_seq_num == Some(1));
            time::advance(Duration::from_secs(2)).await;
            task::yield_now().await;
            peer.assert_silent();
            peer.send(&test_helpers::logon_with_options(1, fix_str!("TARGET"), fix_str!("SENDER"), 0, Some(true), next_expected)).await;
            if next_expected.is_some() {
                let gap = peer.read().await;
                assert_eq!(gap.msg_seq_num(), 1);
                assert_eq!(gap.poss_dup_flag(), Some(true));
                assert_matches!(gap.try_as_admin(), Some(AdminBase::SequenceReset(s)) if s.gap_fill_flag == Some(true) && s.new_seq_no == 2);
            }
            let order = peer.read().await;
            assert_eq!(order.msg_seq_num(), 2);
            assert_matches!(&*order.body, Body::NewOrderSingle(_));
            peer.assert_silent();
            for expected in [MsgTypeBase::Heartbeat, MsgTypeBase::Heartbeat, MsgTypeBase::Heartbeat, MsgTypeBase::Logon] {
                assert_matches!(peer.events.recv().await.unwrap(), TestEvent::AdminMsgIn(kind) if kind == expected);
            }
            peer.control.send(ControlMsg::Disconnect).await.unwrap();
            assert_matches!(peer.events.recv().await.unwrap(), TestEvent::SessionEnd(DisconnectReason::Disconnected));
            let mut storage = peer.task.await.unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), 3);
            assert_eq!(storage.next_target_msg_seq_num().get(), 2);
            assert_eq!(storage.fetch(nz_seq(2), nz_seq(2)).await, Ok(test_helpers::serialize_message(&order).as_slice()));
            assert!(storage.fetch(nz_seq(3), nz_seq(3)).await.is_err());
        }
    }).await;
}

struct FaultReader<R> {
    inner: R,
    error: mpsc::UnboundedReceiver<ErrorKind>,
}

impl<R: AsyncRead + Unpin> AsyncRead for FaultReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Poll::Ready(Some(error)) = self.error.poll_recv(cx) {
            return Poll::Ready(Err(error.into()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
