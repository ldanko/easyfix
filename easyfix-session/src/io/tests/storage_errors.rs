use std::{
    assert_matches,
    cell::RefCell,
    io::ErrorKind,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};

use easyfix_core::{
    base_messages::AdminBase,
    basic_types::{TimePrecision, UtcTimestamp},
    fix_str,
    message::{HeaderAccess, SessionMessage},
    serializer::SerializeError,
};
use easyfix_test_messages::Message;
use tokio::{
    io::{self, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
    task::{self, LocalSet},
    time,
};

use super::wire::{build_peer_logon, wire_bytes};
use crate::{
    application::{Application, DisconnectReason, InputAction},
    initiator::SessionStart,
    io::{
        ControlMsg, InputStream, SessionOpening, drain_and_flush_admin_output,
        drain_and_flush_app_sends, finalize_session, finish_staged_sends, flush_output,
        process_one_resend, sender, session_loop, time::TimerBackend,
    },
    messages_storage::MessagesStorage,
    session_id::SessionId,
    test_helpers::{
        self, DEFAULT_MAX_MESSAGE_SIZE, EngineBuilder, FailingStorage, FailureTiming, StorageOp,
        commit_heartbeat, nz_seq,
    },
};

#[derive(Default)]
struct AppTrace {
    ready: usize,
    ended: Vec<DisconnectReason>,
    app_out: usize,
    admin_out: usize,
    app_in: usize,
    admin_in: usize,
    serialize_errors: usize,
    gap_fill_calls: usize,
}

struct RecordingApp {
    trace: Rc<RefCell<AppTrace>>,
    #[expect(
        clippy::vec_box,
        reason = "staged messages use the Sender API's owned boxes"
    )]
    staged: Vec<Box<Message>>,
    end_gate: Option<oneshot::Receiver<()>>,
    gap_fill: bool,
}

impl RecordingApp {
    fn new(trace: Rc<RefCell<AppTrace>>) -> Self {
        Self {
            trace,
            staged: Vec::new(),
            end_gate: None,
            gap_fill: false,
        }
    }
}

impl Application<Message> for RecordingApp {
    fn on_serialize_error(&mut self, _: Box<Message>, _: &SerializeError) {
        self.trace.borrow_mut().serialize_errors += 1;
    }

    async fn on_session_ready(&mut self, _: &SessionId, sender: sender::Sender<Message>) {
        self.trace.borrow_mut().ready += 1;
        for msg in self.staged.drain(..) {
            sender.send(msg).unwrap();
        }
    }

    async fn on_session_end(&mut self, _: &SessionId, reason: DisconnectReason) {
        self.trace.borrow_mut().ended.push(reason);
        if let Some(gate) = self.end_gate.take() {
            gate.await.unwrap();
        }
    }

    async fn on_app_msg_in(&mut self, _: Box<Message>) -> InputAction {
        self.trace.borrow_mut().app_in += 1;
        InputAction::Accept
    }

    async fn on_admin_msg_in(&mut self, _: &Message) -> InputAction {
        self.trace.borrow_mut().admin_in += 1;
        InputAction::Accept
    }

    fn on_app_msg_out(&mut self, _: &mut Message) {
        self.trace.borrow_mut().app_out += 1;
    }

    fn on_admin_msg_out(&mut self, _: &mut Message) {
        self.trace.borrow_mut().admin_out += 1;
    }

    fn should_gap_fill(&mut self, _: &Message) -> bool {
        self.trace.borrow_mut().gap_fill_calls += 1;
        self.gap_fill
    }
}

/// The end callback waits for the test to observe EOF. This catches transport
/// halves retained across an awaiting callback, as well as post-failure work.
#[tokio::test]
async fn fatal_storage_failures_close_transport_sender_and_notify_once() {
    LocalSet::new()
        .run_until(async {
            for (op, nth) in [
                (StorageOp::SetSender, 1),
                (StorageOp::SetSender, 2),
                (StorageOp::Store, 1),
                (StorageOp::Store, 3),
                (StorageOp::Fetch, 1),
                (StorageOp::Fetch, 3),
                (StorageOp::SetTarget, 1),
            ] {
                for timing in [FailureTiming::Before, FailureTiming::After] {
                    let (engine, _) = EngineBuilder::new().build();
                    let mut storage = FailingStorage::new();
                    storage.fail_on(op, nth, timing);
                    let storage_trace = storage.trace.clone();
                    let trace = Rc::new(RefCell::new(AppTrace::default()));
                    let mut app = RecordingApp::new(trace.clone());
                    app.staged = (0..4)
                        .map(|_| test_helpers::new_order_single_with_empty_header())
                        .collect();
                    let (release, gate) = oneshot::channel();
                    app.end_gate = Some(gate);
                    let (sender, app_rx) = sender::channel(TimerBackend::Tokio);
                    let live_sender = sender.clone();
                    let (_control, control_rx) = mpsc::channel(4);
                    let (server, mut peer) = io::duplex(65536);
                    let (reader, writer) = io::split(server);
                    let task = task::spawn_local(async move {
                        session_loop(
                            SessionOpening::FirstMessage(Ok(build_peer_logon(1, 30))),
                            InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE),
                            writer,
                            engine,
                            &mut storage,
                            app,
                            sender,
                            app_rx,
                            control_rx,
                        )
                        .await;
                        storage
                    });
                    let mut bytes = Vec::new();
                    time::timeout(Duration::from_secs(2), peer.read_to_end(&mut bytes))
                        .await
                        .expect("transport must close before the end callback completes")
                        .unwrap();
                    assert!(storage_trace.borrow().failed, "{op:?} #{nth}, {timing:?}");
                    assert_eq!(live_sender.backlog_len(), 0);
                    assert_matches!(
                        live_sender.send(test_helpers::new_order_single_with_empty_header()),
                        Err(sender::SendError::Closed(_))
                    );
                    assert_eq!(trace.borrow().ended, [DisconnectReason::StorageError]);
                    assert_eq!(trace.borrow().serialize_errors, 0);
                    assert_eq!(trace.borrow().app_out, if nth == 3 { 2 } else { 0 });
                    if op == StorageOp::SetSender && nth == 1 {
                        assert_eq!(trace.borrow().admin_out, 0);
                    }
                    assert!(
                        !bytes.windows(6).any(|v| v == b"\x0135=4\x01"),
                        "failure must not generate GapFill"
                    );
                    release.send(()).unwrap();
                    let storage = time::timeout(Duration::from_secs(2), task)
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(trace.borrow().ended.len(), 1);
                    assert_eq!(storage.trace.borrow().calls.last(), Some(&op));
                }
            }
        })
        .await;
}

#[tokio::test]
async fn producer_number_collision_preserves_original_and_ends_task() {
    LocalSet::new()
        .run_until(async {
            let (engine, _) = EngineBuilder::new().build();
            let mut storage = FailingStorage::new();
            let trace = Rc::new(RefCell::new(AppTrace::default()));
            let mut app = RecordingApp::new(trace.clone());
            let mut collision = test_helpers::new_order_single_with_empty_header();
            collision.set_msg_seq_num(1);
            app.staged = vec![
                collision,
                test_helpers::new_order_single_with_empty_header(),
            ];
            let (release, gate) = oneshot::channel();
            app.end_gate = Some(gate);
            let (sender, app_rx) = sender::channel(TimerBackend::Tokio);
            let live_sender = sender.clone();
            let (_control, control_rx) = mpsc::channel(4);
            let (server, mut peer) = io::duplex(65536);
            let (reader, writer) = io::split(server);
            let task = task::spawn_local(async move {
                session_loop(
                    SessionOpening::FirstMessage(Ok(build_peer_logon(1, 30))),
                    InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE),
                    writer,
                    engine,
                    &mut storage,
                    app,
                    sender,
                    app_rx,
                    control_rx,
                )
                .await;
                storage
            });
            let mut bytes = Vec::new();
            time::timeout(Duration::from_secs(2), peer.read_to_end(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            release.send(()).unwrap();
            let storage = task.await.unwrap();
            assert_eq!(storage.records.len(), 1);
            assert_eq!(storage.records[&nz_seq(1)], bytes);
            assert_eq!(storage.sender.get(), 2);
            assert_eq!(trace.borrow().app_out, 1);
            assert_eq!(trace.borrow().serialize_errors, 0);
            assert_eq!(trace.borrow().ended, [DisconnectReason::StorageError]);
            assert_eq!(storage.trace.borrow().calls.last(), Some(&StorageOp::Store));
            assert_eq!(live_sender.backlog_len(), 0);
            assert_matches!(
                live_sender.send(test_helpers::new_order_single_with_empty_header()),
                Err(sender::SendError::Closed(_))
            );
        })
        .await;
}

#[tokio::test]
async fn no_history_sends_admin_and_app_but_never_calls_message_storage() {
    for manages_admin in [false, true] {
        for persist_messages in [false, true] {
            let (mut engine, _) = EngineBuilder::new()
                .logged_on()
                .persist_messages(persist_messages)
                .build();
            engine.session_settings_mut().manages_admin_output = manages_admin;
            let mut storage = FailingStorage::new();
            if !persist_messages {
                storage.fail_on(StorageOp::Store, 1, FailureTiming::Before);
            }
            let trace = Rc::new(RefCell::new(AppTrace::default()));
            let mut app = RecordingApp::new(trace.clone());
            let (mut writer, reader) = io::duplex(65536);
            engine.send_heartbeat(None);
            drain_and_flush_admin_output(&mut writer, &mut storage, &mut engine, &mut app)
                .await
                .unwrap();
            let (tx, mut rx) = sender::channel(TimerBackend::Tokio);
            tx.send(test_helpers::new_order_single_with_empty_header())
                .unwrap();
            tx.send(test_helpers::heartbeat(0, None)).unwrap();
            drain_and_flush_app_sends(&mut writer, &mut storage, &mut engine, &mut app, &mut rx)
                .await
                .unwrap();
            let bytes = wire_bytes(writer, reader).await;
            let text = String::from_utf8(bytes).unwrap();
            assert_eq!(
                text.matches("\x0135=0\x01").count(),
                usize::from(!manages_admin) + 1
            );
            assert_eq!(text.matches("\x0135=D\x01").count(), 1);
            assert_eq!(trace.borrow().admin_out, 2);
            assert_eq!(trace.borrow().app_out, 1);
            assert_eq!(storage.sender.get(), 4);
            if !persist_messages {
                assert!(
                    !storage
                        .trace
                        .borrow()
                        .calls
                        .iter()
                        .any(|op| matches!(op, StorageOp::Store | StorageOp::Fetch))
                );
                assert!(storage.records.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn new_message_serialization_errors_call_application_and_rollback_in_both_modes() {
    for persist_messages in [false, true] {
        let (mut engine, _) = EngineBuilder::new()
            .logged_on()
            .max_message_size(64.try_into().unwrap())
            .persist_messages(persist_messages)
            .build();
        let mut storage = FailingStorage::new();
        storage.max_message_size = 64;
        let trace = Rc::new(RefCell::new(AppTrace::default()));
        let mut app = RecordingApp::new(trace.clone());
        let (tx, mut rx) = sender::channel(TimerBackend::Tokio);
        tx.send(test_helpers::new_order_single_with_empty_header())
            .unwrap();
        let (mut writer, reader) = io::duplex(1024);
        assert!(
            !drain_and_flush_app_sends(&mut writer, &mut storage, &mut engine, &mut app, &mut rx)
                .await
                .unwrap()
        );
        assert_eq!(trace.borrow().serialize_errors, 1);
        assert_eq!(trace.borrow().app_out, 1);
        assert_eq!(storage.sender.get(), 1);
        assert!(storage.records.is_empty());
        assert!(!engine.should_disconnect());
        assert!(wire_bytes(writer, reader).await.is_empty());
    }
}

#[tokio::test]
async fn no_history_resend_ignores_even_existing_records_and_app_policy() {
    let (mut engine, _) = EngineBuilder::new()
        .logged_on()
        .persist_messages(false)
        .build();
    let mut storage = FailingStorage::new();
    storage.records.insert(
        nz_seq(1),
        b"invalid archive that must not be parsed".to_vec(),
    );
    storage.fail_on(StorageOp::Fetch, 1, FailureTiming::Before);
    let trace = Rc::new(RefCell::new(AppTrace::default()));
    let mut app = RecordingApp::new(trace.clone());
    let mut range = Some(1..=3);
    let (mut writer, reader) = io::duplex(65536);
    while process_one_resend(&mut range, &mut storage, &mut engine, &mut app)
        .await
        .unwrap()
    {
        flush_output(&mut writer, &mut storage, &mut engine)
            .await
            .unwrap();
    }
    engine.flush_resend_gap().unwrap();
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .unwrap();
    let bytes = wire_bytes(writer, reader).await;
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\x0134=1\x01"));
    assert!(text.contains("\x0136=4\x01"));
    assert!(text.contains("\x01123=Y\x01"));
    assert_eq!(trace.borrow().gap_fill_calls, 0);
    assert!(storage.trace.borrow().calls.is_empty());
}

#[tokio::test]
async fn finalization_without_history_releases_backlog_without_numbering_or_hooks() {
    let (mut engine, _) = EngineBuilder::new()
        .logged_on()
        .persist_messages(false)
        .build();
    let mut storage = FailingStorage::new();
    let trace = Rc::new(RefCell::new(AppTrace::default()));
    let mut app = RecordingApp::new(trace.clone());
    let (tx, mut rx) = sender::channel(TimerBackend::Tokio);
    for _ in 0..4 {
        tx.send(test_helpers::new_order_single_with_empty_header())
            .unwrap();
    }
    finish_staged_sends(&mut engine, &mut storage, &mut app, &mut rx);
    finalize_session(&engine, &mut app).await;
    assert_eq!(tx.backlog_len(), 0);
    assert_eq!(trace.borrow().app_out, 0);
    assert!(storage.trace.borrow().calls.is_empty());
    assert_matches!(
        tx.send(test_helpers::new_order_single_with_empty_header()),
        Err(sender::SendError::Closed(_))
    );
}

struct BrokenWriter;

impl AsyncWrite for BrokenWriter {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(ErrorKind::BrokenPipe.into()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

#[tokio::test]
async fn failure_during_final_drain_stops_and_closes_transport_before_callback() {
    LocalSet::new()
        .run_until(async {
            for timing in [FailureTiming::Before, FailureTiming::After] {
                let (engine, _) = EngineBuilder::new().build();
                let mut storage = FailingStorage::new();
                storage.fail_on(StorageOp::Store, 4, timing);
                let trace = Rc::new(RefCell::new(AppTrace::default()));
                let mut app = RecordingApp::new(trace.clone());
                let (release, gate) = oneshot::channel();
                app.end_gate = Some(gate);
                let (sender, app_rx) = sender::channel(TimerBackend::Tokio);
                for _ in 0..5 {
                    sender
                        .send(test_helpers::new_order_single_with_empty_header())
                        .unwrap();
                }
                let live_sender = sender.clone();
                let (_control, control_rx) = mpsc::channel(4);
                let (server, mut peer) = io::duplex(65536);
                // InputStream owns the whole duplex transport: dropping it is
                // necessary to let the peer see EOF, regardless of writer behavior.
                let task = task::spawn_local(async move {
                    session_loop(
                        SessionOpening::SendLogon(SessionStart::Resume),
                        InputStream::new(server, DEFAULT_MAX_MESSAGE_SIZE),
                        BrokenWriter,
                        engine,
                        &mut storage,
                        app,
                        sender,
                        app_rx,
                        control_rx,
                    )
                    .await;
                    storage
                });
                let mut bytes = Vec::new();
                time::timeout(Duration::from_secs(2), peer.read_to_end(&mut bytes))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(bytes.is_empty());
                assert_eq!(trace.borrow().ended, [DisconnectReason::IoError]);
                assert_eq!(live_sender.backlog_len(), 0);
                assert_matches!(
                    live_sender.send(test_helpers::new_order_single_with_empty_header()),
                    Err(sender::SendError::Closed(_))
                );
                release.send(()).unwrap();
                let storage = task.await.unwrap();
                assert!(storage.trace.borrow().failed);
                assert_eq!(storage.trace.borrow().calls.last(), Some(&StorageOp::Store));
                assert_eq!(
                    storage.records.len(),
                    if matches!(timing, FailureTiming::After) {
                        4
                    } else {
                        3
                    }
                );
                assert_eq!(trace.borrow().app_out, 3);
                assert_eq!(trace.borrow().ended.len(), 1);
            }
        })
        .await;
}

#[tokio::test]
async fn first_transmission_borrows_committed_chunk_across_delayed_writes() {
    let (mut engine, _) = EngineBuilder::new().build();
    let mut storage = FailingStorage::new();
    let expected = commit_heartbeat(&mut engine, &mut storage);
    let pointer = storage.records[&nz_seq(1)].as_ptr() as usize;
    struct SlowWriter {
        pointer: usize,
        bytes: Vec<u8>,
        stalled: bool,
    }
    impl AsyncWrite for SlowWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            assert_eq!(buf.as_ptr() as usize, self.pointer + self.bytes.len());
            if !self.stalled {
                self.stalled = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.stalled = false;
            self.bytes.push(buf[0]);
            Poll::Ready(Ok(1))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    let mut writer = SlowWriter {
        pointer,
        bytes: Vec::new(),
        stalled: false,
    };
    assert!(
        flush_output(&mut writer, &mut storage, &mut engine)
            .await
            .unwrap()
    );
    assert_eq!(writer.bytes, expected);
    assert_eq!(storage.records[&nz_seq(1)], expected);
    assert_eq!(storage.trace.borrow().serialized, 1);
}

#[tokio::test]
async fn gap_fill_policy_and_retransmission_preserve_original_record() {
    let (mut engine, _) = EngineBuilder::new().logged_on().build();
    let mut storage = FailingStorage::new();
    let mut original = test_helpers::new_order_single_with_empty_header();
    original.set_sending_time(UtcTimestamp::unix_epoch(TimePrecision::Millis));
    assert!(engine.fill_header(&mut original, &mut storage).unwrap());
    let original_time = original.sending_time();
    engine.commit_send(original, &mut storage).unwrap();
    let original_bytes = storage.records[&nz_seq(1)].clone();
    let (mut writer, reader) = io::duplex(65536);
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .unwrap();
    assert_eq!(wire_bytes(writer, reader).await, original_bytes);

    let trace = Rc::new(RefCell::new(AppTrace::default()));
    let mut app = RecordingApp::new(trace.clone());
    for gap_fill in [true, false] {
        app.gap_fill = gap_fill;
        let mut range = Some(1..=1);
        let (mut writer, reader) = io::duplex(65536);
        assert!(
            process_one_resend(&mut range, &mut storage, &mut engine, &mut app)
                .await
                .unwrap()
        );
        engine.flush_resend_gap().unwrap();
        flush_output(&mut writer, &mut storage, &mut engine)
            .await
            .unwrap();
        let bytes = wire_bytes(writer, reader).await;
        let msg = Message::from_bytes(&bytes).unwrap();
        if gap_fill {
            assert_matches!(msg.try_as_admin(), Some(AdminBase::SequenceReset(reset)) if reset.gap_fill_flag == Some(true));
        } else {
            assert_eq!(msg.poss_dup_flag(), Some(true));
            assert_eq!(msg.orig_sending_time(), Some(original_time));
            assert!(msg.sending_time() > original_time);
        }
        assert_eq!(
            storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
            original_bytes
        );
    }
    assert_eq!(storage.trace.borrow().serialized, 1);
    assert_eq!(trace.borrow().gap_fill_calls, 2);
}

#[tokio::test]
async fn no_history_logon_recovery_resend_request_and_reconnect_keep_counters() {
    time::timeout(Duration::from_secs(3), LocalSet::new().run_until(async {
        let mut storage = FailingStorage::new();
        storage.sender = nz_seq(5);
        storage.records.insert(nz_seq(2), b"old bytes must be ignored".to_vec());
        storage.fail_on(StorageOp::Fetch, 1, FailureTiming::Before);
        for connection in 0..2 {
            let (engine, _) = EngineBuilder::new().persist_messages(false).enable_next_expected_msg_seq_num().build();
            let trace = Rc::new(RefCell::new(AppTrace::default()));
            let app = RecordingApp::new(trace.clone());
            let (sender, app_rx) = sender::channel(TimerBackend::Tokio);
            let live_sender = sender.clone();
            let (control, control_rx) = mpsc::channel(4);
            let (server, mut peer) = io::duplex(65536);
            let (reader, writer) = io::split(server);
            let logon = test_helpers::logon_with_options(
                if connection == 0 { 1 } else { 3 }, fix_str!("TARGET"), fix_str!("SENDER"),
                30, None, Some(if connection == 0 { 2 } else { 6 }),
            );
            let task = task::spawn_local(async move {
                session_loop(SessionOpening::FirstMessage(Ok(logon)),
                    InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE), writer,
                    engine, &mut storage, app, sender, app_rx, control_rx).await;
                storage
            });
            let mut buffer = Vec::new();
            let response = test_helpers::read_one_message(&mut peer, &mut buffer).await;
            assert_eq!(response.msg_seq_num(), 5 + connection);
            assert_matches!(response.try_as_admin(), Some(AdminBase::Logon(_)));
            if connection == 0 {
                let gap = test_helpers::read_one_message(&mut peer, &mut buffer).await;
                assert_eq!(gap.msg_seq_num(), 2);
                assert_matches!(gap.try_as_admin(), Some(AdminBase::SequenceReset(reset)) if reset.gap_fill_flag == Some(true) && reset.new_seq_no == 5);
                peer.write_all(&test_helpers::serialize_message(&test_helpers::resend_request(2, 1, 5))).await.unwrap();
                let gap = test_helpers::read_one_message(&mut peer, &mut buffer).await;
                assert_eq!(gap.msg_seq_num(), 1);
                assert_matches!(gap.try_as_admin(), Some(AdminBase::SequenceReset(reset)) if reset.gap_fill_flag == Some(true) && reset.new_seq_no == 6);
            }
            control.send(ControlMsg::Disconnect).await.unwrap();
            storage = task.await.unwrap();
            assert_eq!(trace.borrow().ended, [DisconnectReason::Disconnected]);
            assert_eq!(trace.borrow().gap_fill_calls, 0);
            assert_eq!(live_sender.backlog_len(), 0);
            assert_eq!(storage.sender.get(), 6 + connection);
            assert_eq!(storage.target.get(), 3 + connection);
            assert!(!storage.trace.borrow().calls.iter().any(|op| matches!(op, StorageOp::Store | StorageOp::Fetch | StorageOp::Reset)));
        }
        assert_eq!(storage.records[&nz_seq(2)], b"old bytes must be ignored");
    })).await.expect("both connections finish without recovery timeout");
}

#[tokio::test]
async fn failures_after_incoming_callback_and_during_replay_stop_live_tasks() {
    LocalSet::new()
        .run_until(async {
            for replay in [false, true] {
                let (engine, _) = EngineBuilder::new().build();
                let mut storage = FailingStorage::new();
                storage.fail_on(
                    if replay {
                        StorageOp::Fetch
                    } else {
                        StorageOp::SetTarget
                    },
                    if replay { 5 } else { 2 },
                    FailureTiming::After,
                );
                let trace = Rc::new(RefCell::new(AppTrace::default()));
                let mut app = RecordingApp::new(trace.clone());
                if replay {
                    app.staged = (0..2)
                        .map(|_| test_helpers::new_order_single_with_empty_header())
                        .collect();
                }
                let (release, gate) = oneshot::channel();
                app.end_gate = Some(gate);
                let (sender, app_rx) = sender::channel(TimerBackend::Tokio);
                let live_sender = sender.clone();
                let (_control, control_rx) = mpsc::channel(4);
                let (server, mut peer) = io::duplex(65536);
                let (reader, writer) = io::split(server);
                let task = task::spawn_local(async move {
                    session_loop(
                        SessionOpening::FirstMessage(Ok(build_peer_logon(1, 30))),
                        InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE),
                        writer,
                        engine,
                        &mut storage,
                        app,
                        sender,
                        app_rx,
                        control_rx,
                    )
                    .await;
                    storage
                });
                let mut buffer = Vec::new();
                for expected in 1..=if replay { 3 } else { 1 } {
                    let msg = time::timeout(
                        Duration::from_secs(2),
                        test_helpers::read_one_message(&mut peer, &mut buffer),
                    )
                    .await
                    .unwrap();
                    assert_eq!(msg.msg_seq_num(), expected);
                }
                let input = if replay {
                    test_helpers::resend_request(2, 2, 3)
                } else {
                    test_helpers::new_order_single(2)
                };
                peer.write_all(&test_helpers::serialize_message(&input))
                    .await
                    .unwrap();
                time::timeout(Duration::from_secs(2), peer.read_to_end(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
                if replay {
                    let resent = Message::from_bytes(&buffer).unwrap();
                    assert_eq!(resent.msg_seq_num(), 2);
                    assert_eq!(resent.poss_dup_flag(), Some(true));
                } else {
                    assert!(buffer.is_empty());
                    assert_eq!(trace.borrow().app_in, 1);
                }
                assert_eq!(trace.borrow().ended, [DisconnectReason::StorageError]);
                assert_eq!(live_sender.backlog_len(), 0);
                assert_matches!(
                    live_sender.send(test_helpers::new_order_single_with_empty_header()),
                    Err(sender::SendError::Closed(_))
                );
                release.send(()).unwrap();
                let storage = task.await.unwrap();
                assert!(storage.trace.borrow().failed);
                assert_eq!(trace.borrow().ended.len(), 1);
            }
        })
        .await;
}

#[tokio::test]
async fn reset_storage_failures_finalize_even_with_unconfirmed_local_reset() {
    LocalSet::new()
        .run_until(async {
            for peer_reset in [false, true] {
                for fail_op in [StorageOp::Reset, StorageOp::Store] {
                    for timing in [FailureTiming::Before, FailureTiming::After] {
                        let (engine, _) =
                            EngineBuilder::new().accept_reset_on_connect(true).build();
                        let mut storage = FailingStorage::new();
                        storage.sender = nz_seq(20);
                        storage.target = nz_seq(30);
                        storage.fail_on(fail_op, 1, timing);
                        let trace = Rc::new(RefCell::new(AppTrace::default()));
                        let mut app = RecordingApp::new(trace.clone());
                        let (release, gate) = oneshot::channel();
                        app.end_gate = Some(gate);
                        let (sender, app_rx) = sender::channel(TimerBackend::Tokio);
                        sender
                            .send(test_helpers::new_order_single_with_empty_header())
                            .unwrap();
                        let live_sender = sender.clone();
                        let (_control, control_rx) = mpsc::channel(4);
                        let (server, mut peer) = io::duplex(65536);
                        let (reader, writer) = io::split(server);
                        let opening = if peer_reset {
                            SessionOpening::FirstMessage(Ok(test_helpers::logon_with_options(
                                1,
                                fix_str!("TARGET"),
                                fix_str!("SENDER"),
                                30,
                                Some(true),
                                None,
                            )))
                        } else {
                            SessionOpening::SendLogon(SessionStart::Reset)
                        };
                        let task = task::spawn_local(async move {
                            session_loop(
                                opening,
                                InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE),
                                writer,
                                engine,
                                &mut storage,
                                app,
                                sender,
                                app_rx,
                                control_rx,
                            )
                            .await;
                            storage
                        });
                        let mut bytes = Vec::new();
                        time::timeout(Duration::from_secs(2), peer.read_to_end(&mut bytes))
                            .await
                            .unwrap()
                            .unwrap();
                        assert!(bytes.is_empty());
                        let expected = if !peer_reset && fail_op == StorageOp::Store {
                            DisconnectReason::SeqNumResetFailed
                        } else {
                            DisconnectReason::StorageError
                        };
                        assert_eq!(trace.borrow().ended, [expected]);
                        assert_eq!(live_sender.backlog_len(), 0);
                        assert_matches!(
                            live_sender.send(test_helpers::new_order_single_with_empty_header()),
                            Err(sender::SendError::Closed(_))
                        );
                        release.send(()).unwrap();
                        let storage = task.await.unwrap();
                        assert!(storage.trace.borrow().failed);
                        assert_eq!(storage.trace.borrow().calls.last(), Some(&fail_op));
                        assert_eq!(trace.borrow().ended.len(), 1);
                    }
                }
            }
        })
        .await;
}
