use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    message::SessionMessage,
    serializer::SerializeError,
};
use easyfix_test_messages::Message;
use tokio::{
    io::{AsyncWrite, DuplexStream, ReadHalf, WriteHalf},
    sync::mpsc,
    task,
    task::JoinHandle,
};

use crate::{
    application::{Application, DisconnectReason, InputAction},
    engine::SessionEngine,
    initiator::SessionStart,
    io::{
        ControlMsg, InputStream, SessionOpening, sender,
        sender::{Receiver, Sender},
        session_loop,
        time::TimerBackend,
    },
    messages_storage::InMemoryStorage,
    session_id::SessionId,
    settings::SessionSettings,
    test_helpers,
    test_helpers::{DEFAULT_MAX_MESSAGE_SIZE, DEFAULT_MAX_MESSAGE_SIZE_BYTES},
};

#[derive(Debug)]
pub(super) enum TestEvent {
    SessionReady,
    SessionEnd(DisconnectReason),
    AppMsgIn,
    AdminMsgIn(MsgTypeBase),
}

pub(super) struct TestApp {
    pub(super) events_tx: mpsc::UnboundedSender<TestEvent>,
    /// When set, `on_app_msg_in` stages a message through the session's
    /// `Sender` and *then* asks for a disconnecting Logout - the shape
    /// `fix_service` uses to answer an anti-flooding penalty with a
    /// BusinessMessageReject before closing.
    pub(super) send_then_logout: bool,
    /// When set, `on_app_msg_in` rejects with no `Text(58)` - the
    /// diagnostic is optional on `Reject<3>` (FIX Transport 5.5).
    pub(super) reject_without_text: bool,
    /// When set, `on_admin_msg_in` rejects every admin message. Applied to
    /// a first `Logon<A>` this leaves the engine in `Idle` without setting
    /// `should_disconnect` - the stalled-handshake shape.
    pub(super) reject_admin: bool,
    /// Captured in `on_session_ready`, so `on_app_msg_in` can stage
    /// through it.
    pub(super) sender: Option<Sender<Message>>,
    pub(super) admin_gate: Option<AdminGate>,
}

pub(super) struct AdminGate {
    pub(super) kind: MsgTypeBase,
    pub(super) skip: usize,
    pub(super) entered: mpsc::UnboundedSender<()>,
    pub(super) action: mpsc::UnboundedReceiver<InputAction>,
}

impl Application<Message> for TestApp {
    fn on_serialize_error(&mut self, _msg: Box<Message>, _error: &SerializeError) {}

    async fn on_session_ready(&mut self, _id: &SessionId, sender: Sender<Message>) {
        self.sender = Some(sender);
        self.events_tx.send(TestEvent::SessionReady).unwrap();
    }

    async fn on_session_end(&mut self, _id: &SessionId, reason: DisconnectReason) {
        self.events_tx.send(TestEvent::SessionEnd(reason)).unwrap();
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        drop(msg);
        self.events_tx.send(TestEvent::AppMsgIn).unwrap();
        if self.reject_without_text {
            return InputAction::Reject {
                reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
                text: None,
                tag: None,
            };
        }
        if self.send_then_logout {
            let sender = self
                .sender
                .as_ref()
                .expect("sender captured at session ready");
            assert!(
                sender
                    .send(test_helpers::new_order_single_with_empty_header())
                    .is_ok(),
                "staging must succeed while the session is still live"
            );
            return InputAction::Logout {
                session_status: None,
                text: None,
                disconnect: true,
            };
        }
        InputAction::Accept
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        // Map admin message variant to base enum for the event record
        let base = match SessionMessage::try_as_admin(msg) {
            Some(AdminBase::Logon(_)) => MsgTypeBase::Logon,
            Some(AdminBase::Heartbeat(_)) => MsgTypeBase::Heartbeat,
            Some(AdminBase::TestRequest(_)) => MsgTypeBase::TestRequest,
            Some(AdminBase::Logout(_)) => MsgTypeBase::Logout,
            Some(AdminBase::ResendRequest(_)) => MsgTypeBase::ResendRequest,
            Some(AdminBase::SequenceReset(_)) => MsgTypeBase::SequenceReset,
            Some(AdminBase::Reject(_)) => MsgTypeBase::Reject,
            None => unreachable!("on_admin_msg_in called with non-admin message"),
        };
        self.events_tx.send(TestEvent::AdminMsgIn(base)).unwrap();
        if self
            .admin_gate
            .as_ref()
            .is_some_and(|gate| gate.kind == base)
        {
            let mut gate = self.admin_gate.take().unwrap();
            if gate.skip > 0 {
                gate.skip -= 1;
                self.admin_gate = Some(gate);
                return InputAction::Accept;
            }
            gate.entered.send(()).unwrap();
            return gate.action.recv().await.unwrap();
        }
        if self.reject_admin {
            return InputAction::Reject {
                reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
                text: None,
                tag: None,
            };
        }
        InputAction::Accept
    }
}

pub(super) struct TestHarness {
    pub(super) engine: SessionEngine<Message>,
    pub(super) storage: InMemoryStorage,
    pub(super) app: TestApp,
    pub(super) sender: Sender<Message>,
    pub(super) app_rx: Receiver<Message>,
    pub(super) control_tx: mpsc::Sender<ControlMsg>,
    pub(super) control_rx: mpsc::Receiver<ControlMsg>,
    pub(super) events_rx: mpsc::UnboundedReceiver<TestEvent>,
}

pub(super) fn build_harness() -> TestHarness {
    build_harness_with_settings(test_helpers::default_session_settings())
}

pub(super) fn build_harness_with_settings(settings: SessionSettings) -> TestHarness {
    let engine = SessionEngine::<Message>::new(
        test_helpers::default_session_id(),
        settings,
        TimerBackend::Tokio,
    );
    let storage = InMemoryStorage::new(DEFAULT_MAX_MESSAGE_SIZE_BYTES);
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let app = TestApp {
        events_tx,
        send_then_logout: false,
        reject_without_text: false,
        reject_admin: false,
        sender: None,
        admin_gate: None,
    };
    let (sender, app_rx) = sender::channel::<Message>(engine.timer_backend());
    let (control_tx, control_rx) = mpsc::channel(4);
    TestHarness {
        engine,
        storage,
        app,
        sender,
        app_rx,
        control_tx,
        control_rx,
        events_rx,
    }
}

impl TestHarness {
    /// Spawn the acceptor session loop, consuming the loop-side fields and
    /// returning the task handle plus the test-side channels (`events_rx`,
    /// `control_tx`). Setup/wiring only - assertions stay in the test.
    pub(super) fn spawn_acceptor<W: AsyncWrite + Unpin + 'static>(
        self,
        reader: ReadHalf<DuplexStream>,
        writer: W,
        first_msg: Box<Message>,
    ) -> (
        JoinHandle<()>,
        mpsc::UnboundedReceiver<TestEvent>,
        mpsc::Sender<ControlMsg>,
    ) {
        let events_rx = self.events_rx;
        let control_tx = self.control_tx;
        let task = task::spawn_local(async move {
            let input = InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE);
            let mut storage = self.storage;
            session_loop(
                SessionOpening::FirstMessage(Ok(first_msg)),
                input,
                writer,
                self.engine,
                &mut storage,
                self.app,
                self.sender,
                self.app_rx,
                self.control_rx,
            )
            .await;
        });
        (task, events_rx, control_tx)
    }

    /// Spawn the initiator session loop (no first message). Setup/wiring
    /// only.
    pub(super) fn spawn_initiator(
        self,
        reader: ReadHalf<DuplexStream>,
        writer: WriteHalf<DuplexStream>,
    ) -> (
        JoinHandle<()>,
        mpsc::UnboundedReceiver<TestEvent>,
        mpsc::Sender<ControlMsg>,
    ) {
        let events_rx = self.events_rx;
        let control_tx = self.control_tx;
        let task = task::spawn_local(async move {
            let input = InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE);
            let mut storage = self.storage;
            session_loop(
                SessionOpening::SendLogon(SessionStart::Resume),
                input,
                writer,
                self.engine,
                &mut storage,
                self.app,
                self.sender,
                self.app_rx,
                self.control_rx,
            )
            .await;
        });
        (task, events_rx, control_tx)
    }
}
