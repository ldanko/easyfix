#![feature(type_alias_impl_trait)]

pub mod acceptor;
pub mod application;
pub mod async_cell;
mod connection;
pub mod messages_storage;
mod session;
pub mod session_id;
mod session_state;
pub mod settings;

use std::{io, time::Duration};

use easyfix_messages::messages::FixtMessage;
use settings::Settings;
use tokio::sync::{
    broadcast,
    mpsc,
};

const NO_INBOUND_TIMEOUT_PADDING: Duration = Duration::from_millis(250);

pub use connection::sender;
use tracing::error;

// TODO:
// 1. Don't use tokio codec
// 2. Try tu write messages directly to output stream
// 3. Examples: FIX Monitor app, logon, maintain session and print incoming
//    messages
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Never received logon from new connection.")]
    LogonNeverReceived,
    #[error("Message does not point to any session.")]
    UnknownSession,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Session error: {0}")]
    SessionError(SessionError),
}

#[derive(Debug)]
pub(crate) enum SenderMsg {
    Msg(Box<FixtMessage>),
    Disconnect,
}

#[derive(Clone, Debug)]
pub struct Sender {
    inner: mpsc::Sender<SenderMsg>,
}

impl Sender {
    pub(crate) fn new(writer: mpsc::Sender<SenderMsg>) -> Sender {
        Sender { inner: writer }
    }

    pub async fn send(&self, msg: Box<FixtMessage>) {
        if let Err(_) = self.inner.send(SenderMsg::Msg(msg)).await {
            panic!("Internal error: failed to send message, receiver closed or dropped");
        }
    }

    pub(crate) async fn disconnect(&self) {
        if let Err(_) = self.inner.send(SenderMsg::Disconnect).await {
            panic!("Internal error: failed to disconnect, receiver closed or dropped");
        }
    }
}

#[derive(Debug)]
pub(crate) struct Shutdown {
    shutdown: bool,
    notify: broadcast::Receiver<()>,
}

impl Shutdown {
    /// Create a new `Shutdown` backed by the given `broadcast::Receiver`.
    pub(crate) fn new(notify: broadcast::Receiver<()>) -> Shutdown {
        Shutdown {
            shutdown: false,
            notify,
        }
    }

    /// Returns `true` if the shutdown signal has been received.
    pub(crate) fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Receive the shutdown notice, waiting if necessary.
    pub(crate) async fn recv(&mut self) {
        // If the shutdown signal has already been received, then return
        // immediately.
        if self.shutdown {
            return;
        }

        // Cannot receive a "lag error" as only one value is ever sent.
        let _ = self.notify.recv().await;

        // Remember that the signal has been received.
        self.shutdown = true;
    }
}

pub struct Initiator {}

impl Initiator {
    fn _register_session() {}
}
