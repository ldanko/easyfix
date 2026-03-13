use std::{
    fmt,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use easyfix_core::{
    basic_types::{FixString, SeqNum, SessionRejectReasonField, SessionStatusField},
    deserializer::DeserializeError,
};
use futures::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::{DisconnectReason, Sender, session_id::SessionId};

pub struct DoNotSend {
    pub gap_fill: bool,
}

#[derive(Debug)]
pub(crate) enum InputResponderMsg {
    Ignore,
    Reject {
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReasonField,
        text: FixString,
        ref_tag_id: Option<i64>,
    },
    Logout {
        session_status: Option<SessionStatusField>,
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
    phantom_ref: PhantomData<&'a ()>,
}

impl<'a> InputResponder<'a> {
    pub(crate) fn new(sender: oneshot::Sender<InputResponderMsg>) -> InputResponder<'a> {
        InputResponder {
            sender,
            phantom_ref: PhantomData,
        }
    }

    pub fn ignore(self) {
        self.sender.send(InputResponderMsg::Ignore).unwrap();
    }

    pub fn reject(
        self,
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReasonField,
        text: FixString,
        ref_tag_id: Option<i64>,
    ) {
        self.sender
            .send(InputResponderMsg::Reject {
                ref_msg_type,
                ref_seq_num,
                reason,
                text,
                ref_tag_id,
            })
            .unwrap();
    }

    pub fn logout(
        self,
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
        disconnect: bool,
    ) {
        self.sender
            .send(InputResponderMsg::Logout {
                session_status,
                text,
                disconnect,
            })
            .unwrap();
    }

    pub fn disconnect(self) {
        self.sender
            .send(InputResponderMsg::Disconnect { reason: None })
            .unwrap();
    }
}

#[derive(Debug)]
pub struct Responder<M> {
    sender: Option<oneshot::Sender<Box<M>>>,
    change_to_gap_fill: bool,
}

impl<M> Responder<M> {
    pub(crate) fn new(sender: oneshot::Sender<Box<M>>) -> Responder<M> {
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

#[derive(Debug)]
pub(crate) enum FixEventInternal<M: fmt::Debug> {
    Created(SessionId),
    Logon(SessionId, Option<Sender<M>>),
    Logout(SessionId, DisconnectReason),
    AppMsgIn(Option<Box<M>>, Option<oneshot::Sender<InputResponderMsg>>),
    AdmMsgIn(Option<Box<M>>, Option<oneshot::Sender<InputResponderMsg>>),
    AppMsgOut(Option<Box<M>>, Responder<M>),
    AdmMsgOut(Option<Box<M>>, Responder<M>),
    DeserializeError(SessionId, DeserializeError),
}

impl<M: fmt::Debug> Drop for FixEventInternal<M> {
    fn drop(&mut self) {
        if let &mut FixEventInternal::AppMsgOut(ref mut msg, ref mut responder)
        | &mut FixEventInternal::AdmMsgOut(ref mut msg, ref mut responder) = self
            && let Some(sender) = responder.sender.take()
        {
            if responder.change_to_gap_fill {
                // TODO: GapFill HERE!
                sender.send(msg.take().unwrap()).unwrap();
            } else {
                sender.send(msg.take().unwrap()).unwrap();
            }
        }
    }
}

/// FIX protocol events.
#[derive(Debug)]
pub enum FixEvent<'a, M> {
    /// Session created.
    Created(&'a SessionId),

    /// Successful Logon<A> messages exchange.
    ///
    /// Use `Sender` to send messages to connected peer.
    Logon(&'a SessionId, Sender<M>),

    /// Session disconnected.
    Logout(&'a SessionId, DisconnectReason),

    /// New application message received.
    ///
    /// Use `InputResponder` to reject the message or to force logout or
    /// disconnection.
    AppMsgIn(Box<M>, InputResponder<'a>),

    /// New administration message received.
    ///
    /// Use `InputResponder` to reject the message or to force logout or
    /// disconnection.
    AdmMsgIn(Box<M>, InputResponder<'a>),

    /// Application message is ready to be send.
    ///
    /// Use `Responder` to change the message to GapFill or to discard it.
    ///
    /// This event may happen after session disconnection when output queue
    /// still has messages to send. In such case all messages will be stored
    /// and will be available thorough ResendRequest<2>.
    AppMsgOut(&'a mut M, &'a mut Responder<M>), // TODO: Try pass by value but bind named

    /// Administration message is ready to be send.
    ///
    /// Use `Responder` to change the message to GapFill or to discard it.
    ///
    /// This event may happen after session disconnection when output queue
    /// still has messages to send. In such case all messages will be stored
    /// and will be available thorough ResendRequest<2>.
    AdmMsgOut(&'a mut M),

    /// Failed to deserialize input message.
    DeserializeError(&'a SessionId, &'a DeserializeError),
}

#[derive(Debug)]
pub struct EventStream<M: fmt::Debug> {
    receiver: ReceiverStream<FixEventInternal<M>>,
}

#[derive(Debug)]
pub struct Emitter<M: fmt::Debug> {
    inner: mpsc::Sender<FixEventInternal<M>>,
}

impl<M: fmt::Debug> Clone for Emitter<M> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<M: fmt::Debug> Emitter<M> {
    pub(crate) async fn send(&self, event: FixEventInternal<M>) {
        if let Err(_e) = self.inner.send(event).await {
            error!("Failed to send msg")
        }
    }
}

pub(crate) fn events_channel<M: fmt::Debug>() -> (Emitter<M>, EventStream<M>) {
    let (sender, receiver) = mpsc::channel(16);

    (
        Emitter { inner: sender },
        EventStream {
            receiver: receiver.into(),
        },
    )
}

mod private {
    pub trait Sealed {}

    impl<M: std::fmt::Debug> Sealed for super::FixEventInternal<M> {}
}

/// This trait is sealed and not meant to be implemented outside of the current crate.
pub trait AsEvent<M: fmt::Debug>: private::Sealed {
    fn as_event(&mut self) -> FixEvent<'_, M>;
}

impl<M: fmt::Debug> AsEvent<M> for FixEventInternal<M> {
    fn as_event(&mut self) -> FixEvent<'_, M> {
        match self {
            FixEventInternal::Created(id) => FixEvent::Created(id),
            FixEventInternal::Logon(id, sender) => FixEvent::Logon(id, sender.take().unwrap()),
            FixEventInternal::Logout(id, reason) => FixEvent::Logout(id, *reason),
            FixEventInternal::AppMsgIn(msg, sender) => FixEvent::AppMsgIn(
                msg.take().unwrap(),
                InputResponder::new(sender.take().unwrap()),
            ),
            FixEventInternal::AdmMsgIn(msg, sender) => FixEvent::AdmMsgIn(
                msg.take().unwrap(),
                InputResponder::new(sender.take().unwrap()),
            ),
            FixEventInternal::AppMsgOut(msg, resp) => {
                FixEvent::AppMsgOut(msg.as_mut().unwrap().as_mut(), resp)
            }
            FixEventInternal::AdmMsgOut(msg, _) => {
                FixEvent::AdmMsgOut(msg.as_mut().unwrap().as_mut())
            }
            FixEventInternal::DeserializeError(session_id, deserialize_error) => {
                FixEvent::DeserializeError(session_id, deserialize_error)
            }
        }
    }
}

impl<M: fmt::Debug> Stream for EventStream<M> {
    type Item = impl AsEvent<M>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.receiver.size_hint()
    }
}
