use std::{assert_matches, str::from_utf8};

use easyfix_core::{
    base_messages::MsgTypeBase,
    basic_types::{MsgTypeField, SeqNum},
    message::SessionMessage,
    serializer::SerializeError,
};
use easyfix_test_messages::Message;
use tokio::io;

use super::wire::wire_bytes;
use crate::{
    application::{Application, DisconnectReason, InputAction},
    engine::PendingOutput,
    io::{
        OutputError, drain_and_flush_admin_output, drain_and_flush_app_sends, finalize_session,
        finish_staged_sends, flush_output, sender, time::TimerBackend,
    },
    messages_storage::MessagesStorage,
    session_id::SessionId,
    test_helpers,
    test_helpers::{EngineBuilder, nz_seq},
};

/// Application that records the outgoing callbacks and the session-end reason.
#[derive(Default)]
struct RecordingApp {
    app_msg_out: Vec<MsgTypeField>,
    admin_msg_out: Vec<MsgTypeField>,
    session_end: Option<DisconnectReason>,
}

impl Application<Message> for RecordingApp {
    fn on_serialize_error(&mut self, _msg: Box<Message>, _error: &SerializeError) {}

    async fn on_session_ready(&mut self, _: &SessionId, _: sender::Sender<Message>) {}

    async fn on_session_end(&mut self, _: &SessionId, reason: DisconnectReason) {
        self.session_end = Some(reason);
    }

    async fn on_app_msg_in(&mut self, _: Box<Message>) -> InputAction {
        InputAction::Accept
    }

    fn on_app_msg_out(&mut self, msg: &mut Message) {
        self.app_msg_out.push(SessionMessage::msg_type(msg));
    }

    fn on_admin_msg_out(&mut self, msg: &mut Message) {
        self.admin_msg_out.push(SessionMessage::msg_type(msg));
    }
}

/// Missing records fail even at the numbering ceiling, and queued output
/// stays blocked on repeated flush attempts.
#[tokio::test]
async fn missing_record_at_max_blocks_all_pending_output() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let (mut writer, reader) = io::duplex(1024);
    engine.push_pending(PendingOutput::Stored(nz_seq(SeqNum::MAX)));
    engine.push_pending(PendingOutput::Stored(nz_seq(1)));
    for _ in 0..2 {
        assert_matches!(
            flush_output(&mut writer, &mut storage, &mut engine).await,
            Err(OutputError::Fatal(_))
        );
    }
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::StorageError)
    );
    assert!(wire_bytes(writer, reader).await.is_empty());
}

/// The teardown drain stores what the application staged but never sent. With
/// the outgoing numbering exhausted there are no numbers left to assign, so the
/// backlog is dropped rather than half-stored under a zero sequence number.
#[tokio::test]
async fn finalize_drain_drops_what_it_cannot_stamp() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();

    let (tx, mut app_rx) = sender::channel::<Message>(TimerBackend::Tokio);
    for _ in 0..3 {
        tx.send(test_helpers::new_order_single_with_empty_header())
            .expect("staging succeeds while the channel is open");
    }
    let mut app = RecordingApp::default();

    finish_staged_sends(&mut engine, &mut storage, &mut app, &mut app_rx);
    finalize_session(&engine, &mut app).await;

    assert!(
        app.app_msg_out.is_empty(),
        "an unstampable message must not reach the outgoing callback"
    );
    assert_eq!(app.session_end, Some(DisconnectReason::SeqNumExhausted));
}

/// The reason that started the teardown is the one reported, whichever side
/// formed it: a `SeqNumExhausted` the engine forms while stamping the last
/// message neither displaces a logout the loop had already concluded nor
/// gives way to the loop's default once it stands.
#[tokio::test]
async fn finalize_reports_the_reason_that_started_the_teardown() {
    for loop_reason in [Some(DisconnectReason::RemoteRequestedLogout), None] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        storage
            .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX - 1))
            .unwrap();
        let (mut writer, _reader) = io::duplex(4096);
        let mut app = RecordingApp::default();

        if let Some(reason) = loop_reason {
            engine.begin_disconnect(reason);
        }
        // The last outgoing message consumes the last sequence number, so the
        // engine forms `SeqNumExhausted` on its own.
        engine.send_heartbeat(None);
        drain_and_flush_admin_output(&mut writer, &mut storage, &mut engine, &mut app)
            .await
            .expect("the last message still fits");
        assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);

        let (_tx, mut app_rx) = sender::channel::<Message>(TimerBackend::Tokio);
        finish_staged_sends(&mut engine, &mut storage, &mut app, &mut app_rx);
        finalize_session(&engine, &mut app).await;

        assert_eq!(
            app.session_end,
            Some(loop_reason.unwrap_or(DisconnectReason::SeqNumExhausted)),
            "loop_reason={loop_reason:?}"
        );
    }
}

/// The admin drain stops at the message it cannot stamp: no callback, no
/// commit, and nothing queued behind it goes out either.
#[tokio::test]
async fn admin_drain_stops_at_the_message_it_cannot_stamp() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();
    let (mut writer, reader) = io::duplex(1024);
    let mut app = RecordingApp::default();

    engine.send_heartbeat(None);
    engine.send_heartbeat(None);

    let wrote = drain_and_flush_admin_output(&mut writer, &mut storage, &mut engine, &mut app)
        .await
        .expect("drain should succeed");

    assert!(!wrote);
    assert!(
        app.admin_msg_out.is_empty(),
        "an unstampable message must not reach the outgoing callback"
    );
    let buf = wire_bytes(writer, reader).await;
    assert!(buf.is_empty(), "nothing may reach the wire: {buf:?}");
}

/// The message stamped `SeqNum::MAX - 1` is a normal message: it reaches the
/// callback, the storage and the wire. Only what would come after it does not.
#[tokio::test]
async fn admin_drain_sends_the_last_usable_message_then_stops() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();
    let (mut writer, reader) = io::duplex(4096);
    let mut app = RecordingApp::default();

    engine.send_heartbeat(None);
    engine.send_heartbeat(None);

    let wrote = drain_and_flush_admin_output(&mut writer, &mut storage, &mut engine, &mut app)
        .await
        .expect("drain should succeed");

    assert!(wrote);
    assert_eq!(app.admin_msg_out, vec![MsgTypeBase::Heartbeat]);
    assert!(engine.should_disconnect());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
    let buf = wire_bytes(writer, reader).await;
    let wire = from_utf8(&buf).expect("ascii");
    assert_eq!(
        wire.matches("\x0135=0\x01").count(),
        1,
        "exactly the message stamped MAX - 1: {wire:?}"
    );
    assert!(
        wire.contains(&format!("\x0134={}\x01", SeqNum::MAX - 1)),
        "stamped with the last usable number: {wire:?}"
    );
}

/// The same for the application-send drain, which routes through
/// `prepare_user_send`.
#[tokio::test]
async fn app_send_drain_stops_at_the_message_it_cannot_stamp() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();
    let (mut writer, reader) = io::duplex(4096);
    let mut app = RecordingApp::default();

    let (tx, mut app_rx) = sender::channel::<Message>(TimerBackend::Tokio);
    for _ in 0..3 {
        tx.send(test_helpers::new_order_single_with_empty_header())
            .expect("staging succeeds while the channel is open");
    }

    drain_and_flush_app_sends(
        &mut writer,
        &mut storage,
        &mut engine,
        &mut app,
        &mut app_rx,
    )
    .await
    .expect("drain should succeed");

    assert_eq!(
        app.app_msg_out.len(),
        1,
        "only the message that could be stamped MAX - 1"
    );
    assert!(engine.should_disconnect());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
    let buf = wire_bytes(writer, reader).await;
    let wire = from_utf8(&buf).expect("ascii");
    assert_eq!(
        wire.matches("\x0135=D\x01").count(),
        1,
        "exactly one application message on the wire: {wire:?}"
    );
}
