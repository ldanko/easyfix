#![feature(type_alias_impl_trait)]

pub mod acceptor;
pub mod application;
mod connection;
pub mod initiator;
pub mod messages_storage;
mod session;
pub mod session_id;
mod session_state;
pub mod settings;

use std::{io, time::Duration};

use easyfix_messages::{
    fields::{FixString, MsgType, UtcTimestamp},
    messages::{FixtMessage, Header, Message, Trailer},
};
use settings::Settings;
use tokio::sync::mpsc;

const NO_INBOUND_TIMEOUT_PADDING: Duration = Duration::from_millis(250);

pub use connection::{send, send_raw, sender};
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

#[derive(Clone, Copy, Debug)]
pub enum DisconnectReason {
    /// Logout requested locally
    LocalRequestedLogout,
    /// Logout requested remotely
    RemoteRequestedLogout,
    /// Disconnect forced by User code
    UserForcedDisconnect,
    /// Received message without MsgSeqNum
    MsgSeqNumNotFound,
    /// Received message with MsgSeqNum too low
    MsgSeqNumTooLow,
    /// Invalid logon state
    InvalidLogonState,
    /// Remote side disconnected
    Disconnected,
    /// I/O Error
    IoError,
}

#[derive(Debug)]
pub(crate) enum SenderMsg {
    Msg(Box<FixtMessage>),
    Disconnect(DisconnectReason),
}

#[derive(Clone, Debug)]
pub struct Sender {
    inner: mpsc::UnboundedSender<SenderMsg>,
}

impl Sender {
    pub(crate) fn new(writer: mpsc::UnboundedSender<SenderMsg>) -> Sender {
        Sender { inner: writer }
    }

    pub fn send_raw(&self, msg: Box<FixtMessage>) {
        if let Err(msg) = self.inner.send(SenderMsg::Msg(msg)) {
            match msg.0 {
                SenderMsg::Msg(msg) => error!(
                    "failed to send {:?}<{}> message, receiver closed or dropped",
                    msg.msg_type(),
                    msg.msg_type().as_fix_str()
                ),
                SenderMsg::Disconnect(_) => unreachable!(),
            }
        }
    }

    // TOOD: Check if send_raw is faster
    pub fn send(&self, msg: Box<Message>) {
        let msg = Box::new(FixtMessage {
            header: Box::new(new_header(msg.msg_type())),
            body: msg,
            trailer: Box::new(new_trailer()),
        });
        if let Err(msg) = self.inner.send(SenderMsg::Msg(msg)) {
            match msg.0 {
                SenderMsg::Msg(msg) => error!(
                    "failed to send {:?}<{}> message, receiver closed or dropped",
                    msg.msg_type(),
                    msg.msg_type().as_fix_str()
                ),
                SenderMsg::Disconnect(_) => unreachable!(),
            }
        }
    }

    pub(crate) fn disconnect(&self, reason: DisconnectReason) {
        if self.inner.send(SenderMsg::Disconnect(reason)).is_err() {
            error!("failed to disconnect, receiver closed or dropped");
        }
    }
}

pub fn new_header(msg_type: MsgType) -> Header {
    // XXX: all required fields overwritten before serialization (if not set)
    Header {
        begin_string: FixString::new(),
        msg_type,
        sending_time: UtcTimestamp::MIN_UTC,
        ..Default::default()
    }
}

pub fn new_trailer() -> Trailer {
    // XXX: all required fields overwritten before serialization
    Trailer::default()
}
