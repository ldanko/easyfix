//! FIX acceptor (server) example.
//!
//! Listens on `127.0.0.1:10050`, registers three sessions (`client_1`,
//! `client_2`, `client_3`), and echoes every inbound `NewOrderSingle`
//! back as a filled `ExecutionReport`. `Ctrl-C` triggers graceful
//! logout + shutdown.
//!
//! ```text
//! RUST_LOG=info cargo run -p session-example --bin acceptor
//! ```
//!
//! The reply is built and sent from inside `on_app_msg_in`: [`Sender::send`]
//! is synchronous and legal to call from a callback, so the whole exchange
//! fits in the per-session handler. Nothing but the shutdown signal lives
//! outside the session.

use std::{cell::Cell, net::SocketAddr, rc::Rc};

use easyfix_session::{
    Acceptor, Application, ApplicationFactory, DisconnectReason, InMemoryStorage, InputAction,
    Sender, SerializeError, SessionContext, SessionId, SessionMessage, ShutdownMode, TcpConnection,
    basic_types::FixStr, fix_str,
};
use session_example::{
    Body, Message, build_business_message_reject_for, build_exec_report_for,
    build_session_settings, create_session_id, run_on_local_set,
};
use tokio::signal;
use tracing::{error, info, warn};

const ACCEPTOR_COMP_ID: &FixStr = fix_str!("easyfix_test_server");

// ---- Application ---------------------------------------------------------

/// Per-session handler: acknowledges every `NewOrderSingle` with a filled
/// `ExecutionReport`.
///
/// One instance per accepted connection, so the session's own [`Sender`] is
/// simply a field - no `SessionId` -> `Sender` map, no event channel.
struct EchoApp {
    session_id: SessionId,
    /// `Some` between `on_session_ready` and `on_session_end`.
    sender: Option<Sender<Message>>,
    /// Shared with every other session's handler - `ExecID` identifies an
    /// execution across the whole venue, not within one counterparty's
    /// session.
    next_exec_id: Rc<Cell<u64>>,
}

impl EchoApp {
    /// Enqueue a reply on this session's [`Sender`].
    ///
    /// The message is staged here and sequenced, stamped and transmitted after
    /// the current callback returns - which is the correct FIX ordering for a
    /// reply.
    fn reply(&self, msg: Box<Message>) {
        let name = SessionMessage::name(msg.as_ref());
        match &self.sender {
            Some(sender) => {
                if let Err(err) = sender.send(msg) {
                    warn!(session_id = %self.session_id, name, ?err, "reply not sent");
                }
            }
            // Unreachable in practice: application messages only arrive on an
            // established session, and `on_session_ready` runs first.
            None => warn!(session_id = %self.session_id, name, "no sender; reply dropped"),
        }
    }
}

impl Application<Message> for EchoApp {
    async fn on_session_ready(&mut self, session_id: &SessionId, sender: Sender<Message>) {
        info!(%session_id, "session ready");
        self.sender = Some(sender);
    }

    async fn on_session_end(&mut self, session_id: &SessionId, reason: DisconnectReason) {
        info!(%session_id, ?reason, "session end");
        // The Sender is dead once the session ends; a reconnect brings a
        // fresh handler with a fresh one.
        self.sender = None;
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        let Body::NewOrderSingle(ref order) = *msg.body else {
            // The MsgType parsed against the dictionary but this application
            // does not support it. That is a business-level refusal, not a
            // session-level one: answer with `BusinessMessageReject<j>` and
            // still return `Accept`, so NextNumIn advances. Returning
            // `InputAction::Reject` here would send a session-level
            // `Reject<3>`, which is reserved for session-level violations.
            info!(
                session_id = %self.session_id,
                name = SessionMessage::name(msg.as_ref()),
                "unsupported application message"
            );
            self.reply(build_business_message_reject_for(&msg));
            return InputAction::Accept;
        };

        info!(
            session_id = %self.session_id,
            cl_ord_id = %order.cl_ord_id,
            "NewOrderSingle received"
        );

        let exec_id = self.next_exec_id.get();
        self.next_exec_id.set(exec_id + 1);
        self.reply(build_exec_report_for(order, exec_id));

        InputAction::Accept
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        info!(
            session_id = %self.session_id,
            name = SessionMessage::name(msg),
            "admin msg in"
        );
        // The decision point for rejecting a session - e.g. bad credentials on
        // an inbound `Logon<A>` returns `InputAction::Logout` from right here.
        InputAction::Accept
    }

    fn on_serialize_error(&mut self, msg: Box<Message>, error: &SerializeError) {
        // The message was never sent and the session will not retry it. This
        // application has nothing to fall back on, so it only records the
        // loss; a real one would escalate or re-stage a corrected message.
        error!(
            session_id = %self.session_id,
            name = SessionMessage::name(msg.as_ref()),
            %error,
            "dropping message that failed to serialize"
        );
    }
}

/// Builds one [`EchoApp`] per accepted connection. State that is genuinely
/// venue-wide - here the `ExecID` counter - is held by the factory and shared
/// with every handler it creates.
struct EchoAppFactory {
    next_exec_id: Rc<Cell<u64>>,
}

impl EchoAppFactory {
    fn new() -> Self {
        EchoAppFactory {
            next_exec_id: Rc::new(Cell::new(1)),
        }
    }
}

impl ApplicationFactory<Message> for EchoAppFactory {
    type App = EchoApp;

    fn create(&self, ctx: &SessionContext<'_>) -> EchoApp {
        EchoApp {
            session_id: ctx.session_id().clone(),
            sender: None,
            next_exec_id: Rc::clone(&self.next_exec_id),
        }
    }
}

// ---- Setup ---------------------------------------------------------------

async fn run_acceptor() {
    let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(EchoAppFactory::new());

    // Register three sessions.
    for target in [
        fix_str!("client_1"),
        fix_str!("client_2"),
        fix_str!("client_3"),
    ] {
        let session_id = create_session_id(ACCEPTOR_COMP_ID, target);
        acceptor
            .register_session(
                session_id,
                build_session_settings(),
                |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
            )
            .expect("register session");
    }

    // Bind and start listener.
    let addr: SocketAddr = "127.0.0.1:10050".parse().unwrap();
    let connection = match TcpConnection::new(addr).await {
        Ok(c) => c,
        Err(err) => {
            error!(%err, "failed to bind TCP listener");
            return;
        }
    };
    // `shutdown` below ends the listener and waits for it, so the handle needs
    // neither abort nor await.
    let _listener_handle = acceptor.start(connection);
    info!(%addr, "acceptor listening");

    // Order handling runs inside the sessions; only the shutdown signal is
    // handled out here.
    signal::ctrl_c().await.expect("listen for ctrl-c");
    info!("ctrl-c received; beginning graceful shutdown");

    // Graceful shutdown - stop accepting, Logout every active session, wait.
    acceptor
        .shutdown(ShutdownMode::GracefulLogout {
            session_status: None,
            text: Some(fix_str!("server shutting down").to_owned()),
        })
        .await;
    info!("shutdown complete");
}

fn main() {
    run_on_local_set(run_acceptor());
}
