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

use std::{future::Future, time::Duration};

use easyfix_messages::{
    fields::{FixString, MsgType, UtcTimestamp},
    messages::{FixtMessage, Header, Message, Trailer},
};
use settings::Settings;
use tokio::sync::{mpsc, watch};

const NO_INBOUND_TIMEOUT_PADDING: Duration = Duration::from_millis(250);
const TEST_REQUEST_THRESHOLD: f32 = 1.2;

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
    /// Disconnect forced by Application code
    ApplicationForcedDisconnect,
    /// Sequence numbering has reached the limit of its representation.
    SequenceNumberExhausted,
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
pub(crate) enum SenderMsg {
    Msg(Box<FixtMessage>),
    Disconnect(DisconnectReason),
}

/// Connection-local terminal signal, independent of the message queues.
#[derive(Clone, Debug)]
pub(crate) struct Abort {
    reason: watch::Sender<Option<DisconnectReason>>,
}

impl Abort {
    fn new() -> Self {
        Self {
            reason: watch::channel(None).0,
        }
    }

    pub(crate) fn request(&self, reason: DisconnectReason) {
        self.reason.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(reason);
            true
        });
    }

    pub(crate) fn reason(&self) -> Option<DisconnectReason> {
        *self.reason.borrow()
    }

    pub(crate) async fn run(&self, work: impl Future<Output = ()>) {
        tokio::select! {
            biased;
            _ = self.cancelled() => {},
            _ = work => {},
        }
    }

    pub(crate) async fn cancelled(&self) {
        let mut receiver = self.reason.subscribe();
        let _ = receiver.wait_for(Option::is_some).await;
    }
}

#[derive(Clone, Debug)]
pub struct Sender {
    inner: mpsc::UnboundedSender<SenderMsg>,
    pub(crate) abort: Abort,
}

impl Sender {
    /// Create new `Sender` instance.
    pub(crate) fn new(writer: mpsc::UnboundedSender<SenderMsg>) -> Sender {
        Sender {
            inner: writer,
            abort: Abort::new(),
        }
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
        if self.abort.reason().is_some() {
            return Err(msg);
        }
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

    pub(crate) fn same_connection(&self, other: &Self) -> bool {
        self.inner.same_channel(&other.inner)
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

#[cfg(test)]
mod tests {
    use std::{cell::Cell, future::poll_fn, task::Poll};

    use futures_util::{pin_mut, poll};
    use tokio::runtime::Builder;

    use super::{Abort, DisconnectReason};

    #[test]
    fn abort_in_one_join_branch_prevents_repolling_the_other() {
        Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let abort = Abort::new();
                let trigger = Cell::new(false);
                let writes = Cell::new(0);
                let input = poll_fn(|_| {
                    if trigger.get() {
                        abort.request(DisconnectReason::SequenceNumberExhausted);
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                });
                let output = poll_fn(|_| {
                    writes.set(writes.get() + 1);
                    Poll::<()>::Pending
                });
                let connection = async {
                    tokio::join!(biased; abort.run(input), abort.run(output));
                };
                pin_mut!(connection);
                assert!(poll!(&mut connection).is_pending());
                assert_eq!(writes.get(), 1);
                trigger.set(true);
                assert!(poll!(&mut connection).is_ready());
                assert_eq!(writes.get(), 1);
            });
    }
}
