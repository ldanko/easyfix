use std::{assert_matches, str::from_utf8, thread, time::Duration};

use chrono::Utc;
use easyfix_core::{message::HeaderAccess, serializer::SerializeError};
use easyfix_test_messages::Message;
use tokio::{io, task::LocalSet};

use super::{
    harness::{TestEvent, build_harness},
    wire::{build_peer_logon, logon_handshake, read_lone_message, wire_bytes},
};
use crate::{
    application::{Application, DisconnectReason, InputAction},
    io::{
        ControlMsg, drain_and_flush_admin_output, drain_and_flush_app_sends, sender,
        time::TimerBackend,
    },
    session_id::SessionId,
    test_helpers,
    test_helpers::EngineBuilder,
};

/// With history disabled every commit must be flushed before the next
/// serialization reuses scratch.
#[tokio::test]
async fn drain_and_flush_admin_preserves_batch_without_history() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .logged_on()
        .persist_messages(false)
        .build();
    let mut app = test_helpers::StubApp;

    // Two admin messages queued before any flush. Two heartbeats are a
    // minimal stand-in for the acceptor Logon-ack + ResendRequest pair.
    engine.send_heartbeat(None);
    engine.send_heartbeat(None);

    let (mut writer, reader) = io::duplex(16384);
    let written = drain_and_flush_admin_output(&mut writer, &mut storage, &mut engine, &mut app)
        .await
        .expect("drain+flush should succeed");
    assert!(written, "bytes should have been written");

    let wire = wire_bytes(writer, reader).await;
    let wire_str = from_utf8(&wire).expect("wire is ASCII");

    // No admin commit may be replaced by a SequenceReset (MsgType=4) on the
    // live path; both heartbeats (MsgType=0) must reach the wire at seq 1
    // and seq 2.
    assert!(
        !wire_str.contains("\x0135=4\x01"),
        "no admin commit may be replaced by a live SequenceReset gap-fill: {wire_str:?}"
    );
    assert_eq!(
        wire_str.matches("\x0135=0\x01").count(),
        2,
        "both batched heartbeats must reach the wire intact: {wire_str:?}"
    );
    // The invariant under test is an ordering one - each commit written before
    // the next is made - so assert the offsets, not mere presence.
    let seq_1 = wire_str
        .find("\x0134=1\x01")
        .expect("heartbeat for seq 1 must be present: {wire_str:?}");
    let seq_2 = wire_str
        .find("\x0134=2\x01")
        .expect("heartbeat for seq 2 must be present: {wire_str:?}");
    assert!(
        seq_1 < seq_2,
        "heartbeats must reach the wire in seq num order: {wire_str:?}"
    );
}

/// A batch of staged application messages must also preserve scratch until
/// each first transmission completes.
#[tokio::test]
async fn drain_and_flush_app_sends_preserves_batch_without_history() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .logged_on()
        .persist_messages(false)
        .build();
    let mut app = test_helpers::StubApp;

    let (tx, mut app_rx) = sender::channel::<Message>(TimerBackend::Tokio);
    tx.send(test_helpers::new_order_single_with_empty_header())
        .expect("send first");
    tx.send(test_helpers::new_order_single_with_empty_header())
        .expect("send second");

    let (mut writer, reader) = io::duplex(16384);
    let written = drain_and_flush_app_sends(
        &mut writer,
        &mut storage,
        &mut engine,
        &mut app,
        &mut app_rx,
    )
    .await
    .expect("drain+flush should succeed");
    assert!(written, "bytes should have been written");

    let wire = wire_bytes(writer, reader).await;
    let wire_str = from_utf8(&wire).expect("wire is ASCII");

    assert!(
        !wire_str.contains("\x0135=4\x01"),
        "no app commit may be replaced by a live SequenceReset gap-fill: {wire_str:?}"
    );
    assert_eq!(
        wire_str.matches("\x0135=D\x01").count(),
        2,
        "both staged messages must reach the wire intact: {wire_str:?}"
    );
    // Ordering, not presence: the drain's contract is that each commit is
    // written before the next is made.
    let seq_1 = wire_str
        .find("\x0134=1\x01")
        .expect("staged message at seq 1 must be present: {wire_str:?}");
    let seq_2 = wire_str
        .find("\x0134=2\x01")
        .expect("staged message at seq 2 must be present: {wire_str:?}");
    assert!(
        seq_1 < seq_2,
        "staged messages must reach the wire in seq num order: {wire_str:?}"
    );
}

/// Application that stages one more message every time it sees one going out.
/// Stands in for a producer task scheduled at one of the drain's `.await`
/// points - the drain flushes per commit, so on the single-threaded runtime a
/// producer really does get to run between iterations.
struct RestagingApp {
    tx: sender::Sender<Message>,
}

impl Application<Message> for RestagingApp {
    fn on_serialize_error(&mut self, _msg: Box<Message>, _error: &SerializeError) {}

    async fn on_session_ready(&mut self, _: &SessionId, _: sender::Sender<Message>) {}

    async fn on_session_end(&mut self, _: &SessionId, _: DisconnectReason) {}

    async fn on_app_msg_in(&mut self, _: Box<Message>) -> InputAction {
        InputAction::Accept
    }

    fn on_app_msg_out(&mut self, _: &mut Message) {
        self.tx
            .send(test_helpers::new_order_single_with_empty_header())
            .expect("restage should succeed");
    }
}

/// The drain stops at the backlog it entered with. It is used on a terminating
/// iteration, so an unbounded `while let Some(_) = try_recv()` would let a
/// producer that keeps enqueueing extend it forever and the session would never
/// reach its `break` - the same window `finalize_session` keeps shut by staying
/// synchronous.
#[tokio::test]
async fn drain_and_flush_app_sends_stops_at_the_backlog_it_entered_with() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();

    let (tx, mut app_rx) = sender::channel::<Message>(TimerBackend::Tokio);
    let mut app = RestagingApp { tx: tx.clone() };

    tx.send(test_helpers::new_order_single_with_empty_header())
        .expect("send first");
    tx.send(test_helpers::new_order_single_with_empty_header())
        .expect("send second");

    let (mut writer, reader) = io::duplex(16384);
    drain_and_flush_app_sends(
        &mut writer,
        &mut storage,
        &mut engine,
        &mut app,
        &mut app_rx,
    )
    .await
    .expect("drain+flush should succeed");

    assert_eq!(
        app_rx.len(),
        2,
        "the two messages staged during the drain must be left for `finalize_session`"
    );

    let wire = wire_bytes(writer, reader).await;
    let wire_str = from_utf8(&wire).expect("wire is ASCII");

    assert_eq!(
        wire_str.matches("\x0135=D\x01").count(),
        2,
        "only the backlog present on entry may reach the wire: {wire_str:?}"
    );
}

/// `SendingTime(52)` is the time of transmission, not of staging (Test
/// Cases Scenario 16, the note under both rows). The runtime is
/// single-threaded, so a blocking sleep between `send` and the read keeps
/// the session task off the CPU for exactly that long - the stamp on the
/// wire must postdate the staging by at least that much.
#[tokio::test]
async fn sending_time_is_stamped_at_transmission_not_at_staging() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let harness = build_harness();
            let sender = harness.sender.clone();
            let (server_io, mut client_io) = io::duplex(8192);
            let (server_reader, server_writer) = io::split(server_io);
            let first_msg = build_peer_logon(1, 30);
            let (session_task, mut events_rx, control_tx) =
                harness.spawn_acceptor(server_reader, server_writer, first_msg);

            logon_handshake(&mut client_io, &mut events_rx).await;

            let staged_at = Utc::now();
            sender
                .send(test_helpers::new_order_single_with_empty_header())
                .expect("stage");
            thread::sleep(Duration::from_millis(200));

            let order = read_lone_message(&mut client_io).await;
            assert_eq!(order.msg_seq_num(), 2);
            let held = order.sending_time().timestamp() - staged_at;
            assert!(
                held.num_milliseconds() >= 190,
                "SendingTime must be stamped at transmission, {held} after staging"
            );

            control_tx.send(ControlMsg::Disconnect).await.unwrap();
            assert_matches!(
                events_rx.recv().await.unwrap(),
                TestEvent::SessionEnd(DisconnectReason::Disconnected)
            );
            session_task.await.unwrap();
        })
        .await;
}
