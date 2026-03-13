use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use async_stream::stream;
use easyfix_core::{basic_types::UtcTimestamp, message::SessionMessage};
use futures_util::Stream;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::StreamExt;
use tracing::{debug, instrument};

use super::time::timeout_stream;
use crate::{DisconnectReason, SenderMsg, messages_storage::MessagesStorage, session::Session};

pub(crate) enum OutputEvent {
    Message(Vec<u8>),
    Timeout,
    Disconnect(DisconnectReason),
}

fn fill_header<M: SessionMessage, S: MessagesStorage>(message: &mut M, session: &Session<M, S>) {
    let mut state = session.state().borrow_mut();

    if message.begin_string().is_empty() {
        message.set_begin_string(session.session_id().begin_string().to_owned());
    }

    if message.sender_comp_id().is_empty() {
        message.set_sender_comp_id(session.session_id().sender_comp_id().to_owned());
    }
    if message.target_comp_id().is_empty() {
        message.set_target_comp_id(session.session_id().target_comp_id().to_owned());
    }
    if message.sending_time() == UtcTimestamp::MIN_UTC {
        message.set_sending_time(UtcTimestamp::now());
    }

    if message.msg_seq_num() == 0 {
        message.set_msg_seq_num(state.next_sender_msg_seq_num());
        state.incr_next_sender_msg_seq_num();
    }

    state.set_last_sent_time(Instant::now());
}

#[instrument(
    name = "serialize",
    level = "trace",
    skip_all,
    fields(
        msg_seq_num = message.msg_seq_num(),
        msg_type = ?message.msg_type()
    )
)]
fn output_handler<M: SessionMessage, S: MessagesStorage>(
    message: &M,
    session: &Session<M, S>,
) -> Vec<u8> {
    // TODO: fn serialize_to(&mut buf) / fn serialize_to_buf(&mut buf)
    let buffer = message.serialize();
    if !message.poss_dup_flag().unwrap_or(false) {
        session
            .state()
            .borrow_mut()
            .store(message.msg_seq_num(), &buffer);
    }

    debug!(
        "Encoded raw data: {}",
        String::from_utf8_lossy(&buffer).replace('\x01', "|")
    );
    buffer
}

pub(crate) fn output_stream<M: SessionMessage, S: MessagesStorage>(
    session: Rc<Session<M, S>>,
    timeout_duration: Duration,
    mut receiver: UnboundedReceiver<SenderMsg<M>>,
) -> impl Stream<Item = OutputEvent> {
    let stream = stream! {
        while let Some(sender_msg) = receiver.recv().await {
            match sender_msg {
                SenderMsg::Msg(mut msg) => {
                    fill_header(&mut *msg, &session);
                    if let Some(msg) = session.on_message_out(msg).await {
                        yield OutputEvent::Message(output_handler(&*msg, &session));
                    }
                }
                SenderMsg::Disconnect(reason) => {
                    // Close stream, but don't break the loop now.
                    // It's possible there are still messages inside.
                    receiver.close();
                    yield OutputEvent::Disconnect(reason);
                },
            }
        }
    };

    timeout_stream(timeout_duration, stream).map(|res| res.unwrap_or(OutputEvent::Timeout))
}
