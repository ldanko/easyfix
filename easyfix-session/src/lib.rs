#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]

pub mod acceptor;
pub mod application;
pub mod initiator;
pub mod io;
pub mod messages_storage;
mod session;
pub mod session_id;
mod session_state;
pub mod settings;

use std::time::Duration;

use easyfix_messages::{
    fields::{FixString, MsgType, UtcTimestamp},
    messages::{FixtMessage, Header, Message, Trailer},
};
use settings::Settings;
use tokio::sync::mpsc;

const NO_INBOUND_TIMEOUT_PADDING: Duration = Duration::from_millis(250);

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
    Io(#[from] std::io::Error),
    #[error("Session error: {0}")]
    SessionError(SessionError),
}

/// Disconnection reasons.
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
    /// Create new `Sender` instance.
    pub(crate) fn new(writer: mpsc::UnboundedSender<SenderMsg>) -> Sender {
        Sender { inner: writer }
    }

    /// Send FIXT message.
    ///
    /// All header and trailer fields can be also adjusted when handing
    /// `FixEvent::AppMsgOut` and `FixEvent::AdmMsgOut`.
    ///
    /// Before serialziation following header fields will be filled:
    /// - begin_string (if not empty)
    /// - msg_type
    /// - sender_comp_id (if not empty)
    /// - target_comp_id (if not empty)
    /// - sending_time (if eq UtcTimestamp::MIN_UTC)
    /// - msg_seq_num (if eq 0)
    ///
    /// The checksum(10) field value is always ignored - it is computed and set
    /// after serialziation.
    pub fn send_raw(&self, msg: Box<FixtMessage>) -> Result<(), Box<FixtMessage>> {
        if let Err(msg) = self.inner.send(SenderMsg::Msg(msg)) {
            match msg.0 {
                SenderMsg::Msg(msg) => {
                    error!(
                        "failed to send {:?}<{}> message, receiver closed or dropped",
                        msg.msg_type(),
                        msg.msg_type().as_fix_str()
                    );
                    Err(msg)
                }
                SenderMsg::Disconnect(_) => unreachable!(),
            }
        } else {
            Ok(())
        }
    }

    /// Send FIX message.
    ///
    /// FIXT message will be constructed internally using default values
    /// for Header and Trailer.
    ///
    /// All header and trailer fields can be also adjusted when handing
    /// `FixEvent::AppMsgOut` and `FixEvent::AdmMsgOut`.
    pub fn send(&self, msg: Box<Message>) -> Result<(), Box<FixtMessage>> {
        let msg = Box::new(FixtMessage {
            header: Box::new(new_header(msg.msg_type())),
            body: msg,
            trailer: Box::new(new_trailer()),
        });
        self.send_raw(msg)
    }

    /// Send disconnect message.
    ///
    /// Output stream will close output queue so no more message can be send
    /// after this one.
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
