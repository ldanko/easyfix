use std::{
    fmt,
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

use easyfix_messages::{
    deserializer,
    fields::{
        FixString, SeqNum, SessionRejectReason, SessionStatus, TagNum,
        parse_reject_reason_to_session_reject_reason,
    },
    messages::FixtMessage,
};
use futures::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::{Abort, DisconnectReason, Sender, session_id::SessionId};

//
#[derive(Debug)]
pub enum DeserializeError {
    // TODO: enum maybe?
    GarbledMessage(String),
    Logout,
    Reject {
        msg_type: Option<FixString>,
        seq_num: SeqNum,
        tag: Option<TagNum>,
        reason: SessionRejectReason,
    },
}

impl fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeserializeError::GarbledMessage(reason) => write!(f, "garbled message: {}", reason),
            DeserializeError::Logout => write!(f, "MsgSeqNum missing"),
            DeserializeError::Reject {
                tag: Some(tag),
                reason,
                ..
            } => write!(f, "{reason:?} (tag={tag})"),
            DeserializeError::Reject {
                tag: None, reason, ..
            } => write!(f, "{reason:?}"),
        }
    }
}

impl std::error::Error for DeserializeError {}

impl From<deserializer::DeserializeError> for DeserializeError {
    fn from(error: deserializer::DeserializeError) -> Self {
        use deserializer::DeserializeError as DeError;
        match error {
            DeError::GarbledMessage(reason) => DeserializeError::GarbledMessage(reason),
            DeError::Logout => DeserializeError::Logout,
            DeError::Reject {
                msg_type,
                seq_num,
                tag,
                reason,
            } => DeserializeError::Reject {
                msg_type,
                seq_num,
                tag,
                reason: parse_reject_reason_to_session_reject_reason(reason),
            },
        }
    }
}

pub struct DoNotSend {
    pub gap_fill: bool,
}

#[derive(Debug)]
pub(crate) enum InputResponderMsg {
    Ignore,
    Reject {
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReason,
        text: FixString,
        ref_tag_id: Option<i64>,
    },
    Logout {
        session_status: Option<SessionStatus>,
        text: Option<FixString>,
        disconnect: bool,
    },
    Disconnect {
        reason: Option<String>,
    },
}

#[derive(Debug)]
pub struct InputResponder<'a> {
    sender: oneshot::Sender<InputResponderMsg>,
    abort: Abort,
    phantom_ref: PhantomData<&'a ()>,
}

impl<'a> InputResponder<'a> {
    pub(crate) fn new(
        sender: oneshot::Sender<InputResponderMsg>,
        abort: Abort,
    ) -> InputResponder<'a> {
        InputResponder {
            sender,
            abort,
            phantom_ref: PhantomData,
        }
    }

    pub fn ignore(self) {
        let _ = self.sender.send(InputResponderMsg::Ignore);
    }

    pub fn reject(
        self,
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReason,
        text: FixString,
        ref_tag_id: Option<i64>,
    ) {
        let _ = self.sender.send(InputResponderMsg::Reject {
            ref_msg_type,
            ref_seq_num,
            reason,
            text,
            ref_tag_id,
        });
    }

    pub fn logout(
        self,
        session_status: Option<SessionStatus>,
        text: Option<FixString>,
        disconnect: bool,
    ) {
        let _ = self.sender.send(InputResponderMsg::Logout {
            session_status,
            text,
            disconnect,
        });
    }

    /// Stop this connection immediately, without Logout or draining queues.
    /// Already written bytes and reserved sequence numbers cannot be undone.
    /// Unlike ordinary disconnect, this preserves counters even with reset_on_disconnect.
    pub fn abort(self) {
        self.abort
            .request(DisconnectReason::ApplicationForcedDisconnect);
    }

    /// Disconnect after draining queued output. Use `abort` for an emergency stop.
    pub fn disconnect(self) {
        let _ = self
            .sender
            .send(InputResponderMsg::Disconnect { reason: None });
    }
}

#[derive(Debug)]
pub struct Responder {
    sender: Option<oneshot::Sender<Box<FixtMessage>>>,
    change_to_gap_fill: bool,
}

impl Responder {
    pub(crate) fn new(sender: oneshot::Sender<Box<FixtMessage>>) -> Responder {
        Responder {
            sender: Some(sender),
            change_to_gap_fill: false,
        }
    }

    pub fn do_not_send(&mut self) {
        // Sender is `Option::None` now so message can't be send back
        self.sender.take();
    }

    pub fn change_to_gap_fill(&mut self) {
        self.change_to_gap_fill = true;
    }
}

/// Why an inbound connection ended before it became a FIX session.
///
/// Reported through [`FixEvent::ConnectionDropped`]. Distinct from
/// [`DisconnectReason`], which describes the end of an *established* session:
/// here no session was ever registered and nothing was sent on the wire.
///
/// The variants carrying a [`SessionId`] are those where the peer's identity
/// was already derived from its `Logon<A>`; `LogonNotReceived` occurs before
/// any identity is known.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ConnectionDropReason {
    /// No usable `Logon<A>` arrived - the peer stayed silent until the logon
    /// timeout, closed the connection, sent something undecodable, or the
    /// transport failed. These causes are not distinguished.
    LogonNotReceived,
    /// The `Logon<A>` resolved to a [`SessionId`] that is not configured.
    /// Dropped without a reply so as not to reveal which identities are valid
    /// (FIX Session Layer 4.6.4).
    ///
    /// A burst of *distinct* unknown ids from one address is the signature of
    /// CompID enumeration; a single id repeating is more often a misconfigured
    /// counterparty.
    UnknownSession(SessionId),
    /// A session is already running for this [`SessionId`]. Dropped without a
    /// reply, since a `Logout<5>` would consume a `MsgSeqNum(34)` and disturb
    /// the live session (FIX Session Layer 4.6.4).
    SessionAlreadyActive(SessionId),
    /// The acceptor is disabled (see `Acceptor::disable`), so inbound
    /// connections are refused until it is enabled again. Carries the peer's
    /// identity when the `Logon<A>` had already been read.
    AcceptorDisabled(Option<SessionId>),
}

#[derive(Debug)]
pub(crate) enum FixEventInternal {
    Created(SessionId),
    Logon(SessionId, Option<Sender>),
    Logout(SessionId, DisconnectReason),
    AppMsgIn(
        Option<Box<FixtMessage>>,
        Option<oneshot::Sender<InputResponderMsg>>,
        Abort,
    ),
    AdmMsgIn(
        Option<Box<FixtMessage>>,
        Option<oneshot::Sender<InputResponderMsg>>,
        Abort,
    ),
    AppMsgOut(Option<Box<FixtMessage>>, Responder),
    AdmMsgOut(Option<Box<FixtMessage>>, Responder),
    DeserializeError(SessionId, DeserializeError),
    ConnectionDropped(SocketAddr, ConnectionDropReason),
}

impl Drop for FixEventInternal {
    fn drop(&mut self) {
        if let &mut FixEventInternal::AppMsgOut(ref mut msg, ref mut responder)
        | &mut FixEventInternal::AdmMsgOut(ref mut msg, ref mut responder) = self
            && let Some(sender) = responder.sender.take()
        {
            // TODO: implement change_to_gap_fill. A cancelled connection no
            // longer owns the receiver; dropping a stale event is harmless.
            if let Some(msg) = msg.take() {
                let _ = sender.send(msg);
            }
        }
    }
}

/// FIX protolol events.
#[derive(Debug)]
pub enum FixEvent<'a> {
    /// Session created.
    Created(&'a SessionId),

    /// Successfull Logon<A> messages exchange.
    ///
    /// Use `Sender` to send messages to connected peer.
    Logon(&'a SessionId, Sender),

    /// Session disconnected.
    Logout(&'a SessionId, DisconnectReason),

    /// New application message received.
    ///
    /// Use `InputResponder` to reject the message or to force logut or
    /// disconnection.
    AppMsgIn(Box<FixtMessage>, InputResponder<'a>),

    /// New administration message received.
    ///
    /// Use `InputResponder` to reject the message or to force logut or
    /// disconnection.
    AdmMsgIn(Box<FixtMessage>, InputResponder<'a>),

    /// Application message is ready to be send.
    ///
    /// Use `Responder` to change the message to GapFill or to discard it.
    ///
    /// This event may happen after session disconnection when output queue
    /// still has messages to send. In such case all messages will be stored
    /// and will be available thorough ResendRequest<2>.
    AppMsgOut(&'a mut FixtMessage, &'a mut Responder), // TODO: Try pass by value but bind named

    /// Administration message is ready to be send.
    ///
    /// Use `Responder` to change the message to GapFill or to discard it.
    ///
    /// This event may happen after session disconnection when output queue
    /// still has messages to send. In such case all messages will be stored
    /// and will be available thorough ResendRequest<2>.
    AdmMsgOut(&'a mut FixtMessage),

    /// Failed to deserialize input message.
    DeserializeError(&'a SessionId, &'a DeserializeError),

    /// An inbound connection ended before it became a session, so no
    /// [`Created`](FixEvent::Created) / [`Logon`](FixEvent::Logon) will
    /// follow for it.
    ///
    /// Reported for every connection the acceptor handles, whether it arrived
    /// through a `Connection` listener or `Acceptor::run_session_task`. The
    /// library reports and does not classify: an address worth refusing is
    /// refused in your `Connection` implementation, which drops it before any
    /// reply - as the spec requires for an unrecognized identity
    /// (FIX Session Layer 4.6.4).
    ConnectionDropped(SocketAddr, &'a ConnectionDropReason),
}

#[derive(Debug)]
pub struct EventStream {
    receiver: ReceiverStream<FixEventInternal>,
}

#[derive(Debug)]
pub struct Emitter {
    inner: mpsc::Sender<FixEventInternal>,
    abort: Option<Abort>,
}

impl Clone for Emitter {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            abort: self.abort.clone(),
        }
    }
}

impl Emitter {
    pub(crate) fn with_abort(mut self, abort: Abort) -> Self {
        self.abort = Some(abort);
        self
    }

    pub(crate) async fn send(&self, event: FixEventInternal) {
        let send = self.inner.send(event);
        let result = if let Some(abort) = &self.abort {
            tokio::select! {
                biased;
                _ = abort.cancelled() => return,
                result = send => result,
            }
        } else {
            send.await
        };
        if result.is_err() {
            error!("Failed to send msg");
        }
    }
}

pub(crate) fn events_channel() -> (Emitter, EventStream) {
    let (sender, receiver) = mpsc::channel(16);

    (
        Emitter {
            inner: sender,
            abort: None,
        },
        EventStream {
            receiver: receiver.into(),
        },
    )
}

mod private {
    pub trait Sealed {}

    impl Sealed for super::FixEventInternal {}
}

/// This trait is sealed and not meant to be implemented outside of the current crate.
pub trait AsEvent: private::Sealed {
    fn as_event(&mut self) -> FixEvent<'_>;
}

impl AsEvent for FixEventInternal {
    fn as_event(&mut self) -> FixEvent<'_> {
        match self {
            FixEventInternal::Created(id) => FixEvent::Created(id),
            FixEventInternal::Logon(id, sender) => FixEvent::Logon(id, sender.take().unwrap()),
            FixEventInternal::Logout(id, reason) => FixEvent::Logout(id, *reason),
            FixEventInternal::AppMsgIn(msg, sender, abort) => FixEvent::AppMsgIn(
                msg.take().unwrap(),
                InputResponder::new(sender.take().unwrap(), abort.clone()),
            ),
            FixEventInternal::AdmMsgIn(msg, sender, abort) => FixEvent::AdmMsgIn(
                msg.take().unwrap(),
                InputResponder::new(sender.take().unwrap(), abort.clone()),
            ),
            FixEventInternal::AppMsgOut(msg, resp) => {
                FixEvent::AppMsgOut(msg.as_mut().unwrap(), resp)
            }
            FixEventInternal::AdmMsgOut(msg, _) => FixEvent::AdmMsgOut(msg.as_mut().unwrap()),
            FixEventInternal::DeserializeError(session_id, deserialize_error) => {
                FixEvent::DeserializeError(session_id, deserialize_error)
            }
            FixEventInternal::ConnectionDropped(peer_addr, reason) => {
                FixEvent::ConnectionDropped(*peer_addr, reason)
            }
        }
    }
}

impl Stream for EventStream {
    type Item = impl AsEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.receiver.size_hint()
    }
}
