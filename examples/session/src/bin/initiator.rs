//! FIX initiator (client) example.
//!
//! Connects to `127.0.0.1:10050` as `client_1`, sends a single
//! `NewOrderSingle` after logon, and logs the `ExecutionReport` reply.
//! `Ctrl-C` triggers graceful logout.
//!
//! ```text
//! RUST_LOG=info cargo run -p session-example --bin initiator
//! ```
//!
//! The order is staged from `on_session_ready` - the first moment the
//! `Sender` exists. An initiator is still waiting for the Logon response
//! there, so the order leaves the staging queue only once the session is
//! established. The reply is handled in `on_app_msg_in`. Only ctrl-c is left
//! outside the session.

use easyfix_session::{
    Application, ApplicationFactory, DisconnectReason, InMemoryStorage, Initiator, InputAction,
    Sender, SerializeError, SessionContext, SessionId, SessionMessage,
    basic_types::{FixStr, TimePrecision},
    fix_str,
};
use session_example::{
    Body, Message, build_new_order_single, build_session_settings, create_session_id,
    run_on_local_set,
};
use tokio::signal;
use tracing::{error, info, warn};

const INITIATOR_COMP_ID: &FixStr = fix_str!("client_1");
const ACCEPTOR_COMP_ID: &FixStr = fix_str!("easyfix_test_server");
const ACCEPTOR_ADDR: &str = "127.0.0.1:10050";
const CL_ORD_ID: &FixStr = fix_str!("ORD001");

// ---- Application ---------------------------------------------------------

/// Per-session handler: sends one order on logon and logs what comes back.
struct OrderApp {
    /// Captured from [`SessionContext::time_precision`] - the session stamps
    /// only `SendingTime<52>`, so body timestamps such as
    /// `TransactTime<60>` are ours to render.
    time_precision: TimePrecision,
}

impl Application<Message> for OrderApp {
    async fn on_session_ready(&mut self, session_id: &SessionId, sender: Sender<Message>) {
        info!(%session_id, "session ready");

        let order = build_new_order_single(CL_ORD_ID, self.time_precision);
        match sender.send(order) {
            Ok(()) => info!(cl_ord_id = %CL_ORD_ID, "staged NewOrderSingle"),
            Err(err) => warn!(?err, "failed to stage NewOrderSingle"),
        }
        // This application sends nothing else, so it lets the Sender drop
        // here. Keep it in a field (as the acceptor does) to send later.
    }

    async fn on_session_end(&mut self, session_id: &SessionId, reason: DisconnectReason) {
        info!(%session_id, ?reason, "session end");
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        match *msg.body {
            Body::ExecutionReport(ref report) => info!(
                order_id = %report.order_id,
                exec_id = %report.exec_id,
                ord_status = ?report.ord_status,
                "received ExecutionReport"
            ),
            // The peer accepted the message at the session level and refused it
            // at the business level - the order never reached its book.
            Body::BusinessMessageReject(ref reject) => warn!(
                ref_seq_num = ?reject.ref_seq_num,
                ref_msg_type = %reject.ref_msg_type,
                reason = ?reject.business_reject_reason,
                text = ?reject.text,
                "order rejected"
            ),
            _ => info!(name = SessionMessage::name(msg.as_ref()), "app msg in"),
        }
        InputAction::Accept
    }

    fn on_serialize_error(&mut self, msg: Box<Message>, error: &SerializeError) {
        // The message was never sent and the session will not retry it. This
        // application has nothing to fall back on, so it only records the
        // loss; a real one would escalate or re-stage a corrected message.
        error!(
            name = SessionMessage::name(msg.as_ref()),
            %error,
            "dropping message that failed to serialize"
        );
    }
}

/// Builds the handler for the Initiator's single session.
struct OrderAppFactory;

impl ApplicationFactory<Message> for OrderAppFactory {
    type App = OrderApp;

    fn create(&self, ctx: &SessionContext<'_>) -> OrderApp {
        OrderApp {
            time_precision: ctx.time_precision(),
        }
    }
}

// ---- Setup ---------------------------------------------------------------

async fn run_initiator() {
    let session_id = create_session_id(INITIATOR_COMP_ID, ACCEPTOR_COMP_ID);

    let initiator = match Initiator::<Message, InMemoryStorage, _>::new(
        session_id,
        build_session_settings(),
        OrderAppFactory,
        |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
    ) {
        Ok(initiator) => initiator,
        Err(err) => {
            error!(%err, "invalid session configuration");
            return;
        }
    };

    // Connect to the acceptor and spawn the session task.
    let mut session_handle = match initiator.connect(ACCEPTOR_ADDR).await {
        Ok(h) => h,
        Err(err) => {
            error!(%err, "failed to connect to acceptor");
            return;
        }
    };
    info!(%ACCEPTOR_ADDR, "connected");

    let interrupted = tokio::select! {
        biased;

        _ = signal::ctrl_c() => {
            info!("ctrl-c received; requesting logout");
            if let Err(err) = initiator.logout(None, None).await {
                warn!(?err, "logout request failed (session may already be closed)");
            }
            true
        }

        res = &mut session_handle => {
            if let Err(err) = res {
                warn!(?err, "session task panicked");
            }
            false
        }
    };

    // A Logout request only starts the teardown; the session task lives until
    // the peer confirms the Logout (or the logout timeout expires).
    if interrupted && let Err(err) = session_handle.await {
        warn!(?err, "session task panicked");
    }
    info!("initiator shutdown complete");
}

fn main() {
    run_on_local_set(run_initiator());
}
