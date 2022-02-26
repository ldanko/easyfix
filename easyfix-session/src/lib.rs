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
use tokio::sync::mpsc;

const NO_INBOUND_TIMEOUT_PADDING: Duration = Duration::from_millis(250);

pub use connection::sender;
use tracing::error;

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
        if let Err(msg) = self.inner.send(SenderMsg::Msg(msg)).await {
            match msg.0 {
                SenderMsg::Msg(msg) => error!(
                    "failed to send {:?}<{}> message, receiver closed or dropped",
                    msg.msg_type(),
                    msg.msg_type().as_fix_str()
                ),
                SenderMsg::Disconnect => unreachable!(),
            }
        }
    }

    pub(crate) async fn disconnect(&self) {
        if let Err(_) = self.inner.send(SenderMsg::Disconnect).await {
            error!("failed to disconnect, receiver closed or dropped");
        }
    }
}

pub struct Initiator {}

impl Initiator {
    fn _register_session() {}
}
