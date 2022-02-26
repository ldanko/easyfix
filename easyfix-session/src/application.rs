use std::{
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use easyfix_messages::{
    deserializer::DeserializeError,
    fields::{FixString, SeqNum, SessionRejectReason},
    messages::FixtMessage,
};
use futures::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::{session_id::SessionId, Sender};

pub struct DoNotSend {
    pub gap_fill: bool,
}

#[derive(Debug)]
pub(crate) enum InputResponderMsg {
    // RejectLogon {
    //     reason: Option<String>,
    // },
    Reject {
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReason,
        text: FixString,
        ref_tag_id: Option<i64>,
    },
    Logout {
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

    pub fn reject(
        self,
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReason,
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

    pub fn logout(self, text: Option<FixString>, disconnect: bool) {
        self.sender
            .send(InputResponderMsg::Logout { text, disconnect })
            .unwrap();
    }

    pub fn disconnect(self) {
        self.sender
            .send(InputResponderMsg::Disconnect { reason: None })
            .unwrap();
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

#[derive(Debug)]
pub(crate) enum FixEventInternal {
    Created(SessionId),
    Logon(SessionId, Option<Sender>),
    Logout(SessionId),
    AppMsgIn(
        Option<Box<FixtMessage>>,
        Option<oneshot::Sender<InputResponderMsg>>,
    ),
    AdmMsgIn(
        Option<Box<FixtMessage>>,
        Option<oneshot::Sender<InputResponderMsg>>,
    ),
    AppMsgOut(Option<Box<FixtMessage>>, Responder),
    AdmMsgOut(Option<Box<FixtMessage>>, Responder),
    DeserializeError(SessionId, DeserializeError),
}

impl Drop for FixEventInternal {
    fn drop(&mut self) {
        if let FixEventInternal::AppMsgOut(ref mut msg, ref mut responder)
        | FixEventInternal::AdmMsgOut(ref mut msg, ref mut responder) = self
        {
            if let Some(sender) = responder.sender.take() {
                if responder.change_to_gap_fill {
                    // TODO: GapFill HERE!
                    sender.send(msg.take().unwrap()).unwrap();
                } else {
                    sender.send(msg.take().unwrap()).unwrap();
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum FixEvent<'a> {
    Created(&'a SessionId),
    Logon(&'a SessionId, Sender),
    Logout(&'a SessionId),
    AppMsgIn(Box<FixtMessage>, InputResponder<'a>),
    AdmMsgIn(Box<FixtMessage>, InputResponder<'a>),
    //
    AppMsgOut(&'a mut FixtMessage, &'a mut Responder), // TODO: Try pass by value but bind named
    // ref to the struct
    AdmMsgOut(&'a mut FixtMessage),
    //
    DeserializeError(&'a SessionId, &'a DeserializeError),
}

#[derive(Debug)]
pub struct EventStream {
    receiver: ReceiverStream<FixEventInternal>,
}

#[derive(Debug)]
pub struct Emitter {
    inner: mpsc::Sender<FixEventInternal>,
}

impl Clone for Emitter {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Emitter {
    pub(crate) async fn send(&self, event: FixEventInternal) {
        if let Err(_e) = self.inner.send(event).await {
            error!("Failed to send msg")
        }
    }
}

pub(crate) fn events_channel() -> (Emitter, EventStream) {
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
            FixEventInternal::Logout(id) => FixEvent::Logout(id),
            FixEventInternal::AppMsgIn(msg, sender) => FixEvent::AppMsgIn(
                msg.take().unwrap(),
                InputResponder::new(sender.take().unwrap()),
            ),
            FixEventInternal::AdmMsgIn(msg, sender) => FixEvent::AdmMsgIn(
                msg.take().unwrap(),
                InputResponder::new(sender.take().unwrap()),
            ),
            FixEventInternal::AppMsgOut(msg, resp) => {
                FixEvent::AppMsgOut(msg.as_mut().unwrap(), resp)
            }
            FixEventInternal::AdmMsgOut(msg, _) => FixEvent::AdmMsgOut(msg.as_mut().unwrap()),
            FixEventInternal::DeserializeError(session_id, deserialize_error) => {
                FixEvent::DeserializeError(session_id, deserialize_error)
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
