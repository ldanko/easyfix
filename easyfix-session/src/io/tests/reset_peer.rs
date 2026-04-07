use std::time::Duration;

use easyfix_core::{
    base_messages::AdminBase,
    basic_types::{FixString, Int},
    message::SessionMessage,
};
use easyfix_test_messages::Message;
use futures_util::FutureExt;
use tokio::{
    io,
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream},
    sync::mpsc,
    task,
    task::JoinHandle,
    time,
};

use super::{
    harness::{TestEvent, TestHarness, build_harness_with_settings},
    wire::{build_peer_logon, logon_handshake},
};
use crate::{
    io::{ControlMsg, InputStream, SessionOpening, sender::Sender, session_loop},
    messages_storage::{InMemoryStorage, MessagesStorage},
    settings::SessionSettings,
    test_helpers,
    test_helpers::{DEFAULT_MAX_MESSAGE_SIZE, read_one_message},
};

pub(super) struct ResetPeer {
    pub(super) wire: DuplexStream,
    pub(super) buffer: Vec<u8>,
    pub(super) events: mpsc::UnboundedReceiver<TestEvent>,
    pub(super) control: mpsc::Sender<ControlMsg>,
    pub(super) sender: Sender<Message>,
    pub(super) task: JoinHandle<InMemoryStorage>,
}

impl ResetPeer {
    pub(super) fn assert_silent(&mut self) {
        assert!(
            read_one_message(&mut self.wire, &mut self.buffer)
                .now_or_never()
                .is_none()
        );
    }

    pub(super) async fn read(&mut self) -> Box<Message> {
        time::timeout(
            Duration::from_secs(1),
            read_one_message(&mut self.wire, &mut self.buffer),
        )
        .await
        .expect("expected wire message without advancing the reset deadline")
    }

    pub(super) async fn send(&mut self, message: &Message) {
        self.wire
            .write_all(&test_helpers::serialize_message(message))
            .await
            .unwrap();
    }

    pub(super) async fn probe(&mut self) -> FixString {
        let msg = self.read().await;
        let Some(AdminBase::TestRequest(probe)) = msg.try_as_admin() else {
            panic!("expected reset probe, got {msg:?}");
        };
        probe.test_req_id.into_owned()
    }
}

pub(super) async fn running_reset_peer(settings: SessionSettings) -> ResetPeer {
    running_reset_harness(build_harness_with_settings(settings)).await
}

pub(super) async fn running_reset_harness(harness: TestHarness) -> ResetPeer {
    let (server, wire) = io::duplex(65536);
    let (reader, writer) = io::split(server);
    running_reset_transport(harness, wire, reader, writer).await
}

pub(super) async fn running_reset_transport<R, W>(
    harness: TestHarness,
    mut wire: DuplexStream,
    reader: R,
    writer: W,
) -> ResetPeer
where
    R: AsyncRead + Unpin + 'static,
    W: AsyncWrite + Unpin + 'static,
{
    let heartbeat = harness
        .engine
        .session_settings()
        .heartbeat_interval
        .map_or(0, |value| Int::from(value.get()));
    let opening_seq = harness.storage.next_target_msg_seq_num().get();
    let mut events = harness.events_rx;
    let control = harness.control_tx;
    let sender = harness.sender.clone();
    let task = task::spawn_local(async move {
        let mut storage = harness.storage;
        session_loop(
            SessionOpening::FirstMessage(Ok(build_peer_logon(opening_seq, heartbeat))),
            InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE),
            writer,
            harness.engine,
            &mut storage,
            harness.app,
            harness.sender,
            harness.app_rx,
            harness.control_rx,
        )
        .await;
        storage
    });
    logon_handshake(&mut wire, &mut events).await;
    ResetPeer {
        wire,
        buffer: Vec::new(),
        events,
        control,
        sender,
        task,
    }
}
