#![feature(impl_trait_in_assoc_type)]

pub mod acceptor;
pub mod application;
pub mod initiator;
pub mod io;
pub mod messages_storage;
mod session;
pub mod session_id;
mod session_state;
pub mod settings;

use std::{fmt, time::Duration};

use easyfix_core::message::SessionMessage;
use settings::Settings;
use tokio::sync::mpsc;
use tracing::error;

const NO_INBOUND_TIMEOUT_PADDING: Duration = Duration::from_millis(250);
const TEST_REQUEST_THRESHOLD: f32 = 1.2;

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
    /// Disconnect forced by Application code
    ApplicationForcedDisconnect,
    /// Received message without MsgSeqNum
    MsgSeqNumNotFound,
    /// Received message with MsgSeqNum too low
    MsgSeqNumTooLow,
    /// Invalid logon state
    InvalidLogonState,
    /// Invalid COMP ID
    InvalidCompId,
    /// Invalid OrigSendingTime
    InvalidOrigSendingTime,
    /// Remote side disconnected
    Disconnected,
    /// I/O Error
    IoError,
    /// Logout timeout
    LogoutTimeout,
}

#[derive(Debug)]
pub(crate) enum SenderMsg<M> {
    Msg(Box<M>),
    Disconnect(DisconnectReason),
}

pub struct Sender<M> {
    inner: mpsc::UnboundedSender<SenderMsg<M>>,
}

impl<M> Clone for Sender<M> {
    fn clone(&self) -> Self {
        Sender {
            inner: self.inner.clone(),
        }
    }
}

impl<M> fmt::Debug for Sender<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<M: SessionMessage> Sender<M> {
    /// Create new `Sender` instance.
    pub(crate) fn new(writer: mpsc::UnboundedSender<SenderMsg<M>>) -> Sender<M> {
        Sender { inner: writer }
    }

    // TODO: rename send_raw to send once the old send(Box<Message>) is removed
    /// Send message.
    ///
    /// Before serialization following header fields will be filled:
    /// - begin_string (if not empty)
    /// - sender_comp_id (if not empty)
    /// - target_comp_id (if not empty)
    /// - sending_time (if eq UtcTimestamp::MIN_UTC)
    /// - msg_seq_num (if eq 0)
    ///
    /// The checksum(10) field value is always ignored - it is computed and set
    /// after serialization.
    pub fn send_raw(&self, msg: Box<M>) -> Result<(), Box<M>> {
        if let Err(msg) = self.inner.send(SenderMsg::Msg(msg)) {
            match msg.0 {
                SenderMsg::Msg(msg) => {
                    let msg_type = msg.msg_type();
                    error!(
                        "failed to send {msg_type:?}<{}> message, receiver closed or dropped",
                        msg_type.as_fix_str()
                    );
                    Err(msg)
                }
                SenderMsg::Disconnect(_) => unreachable!(),
            }
        } else {
            Ok(())
        }
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
