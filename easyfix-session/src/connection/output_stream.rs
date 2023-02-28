use std::{io, rc::Rc};

use async_stream::stream;
use easyfix_messages::{
    fields::{SeqNum, UtcTimestamp},
    messages::FixtMessage,
};
use futures_util::Stream;
use tokio::{sync::mpsc::Receiver, time::Duration};
use tokio_stream::{Elapsed, StreamExt};
use tracing::debug;

use crate::{messages_storage::MessagesStorage, session::Session, SenderMsg};

pub(crate) enum OutputEvent {
    Message(Vec<u8>),
    Timeout,
    Disconnect,
    Error(OutputError),
}

pub struct OutputMessage {
    message: Box<FixtMessage>,
    msg_seq_num: Option<SeqNum>,
}

impl From<Box<FixtMessage>> for OutputMessage {
    fn from(message: Box<FixtMessage>) -> Self {
        OutputMessage {
            message,
            msg_seq_num: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OutputError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("outbound MsgSeqNum max value exceeded")]
    OutboundMsgSeqNumMaxExceeded,
}

fn output_handler<S: MessagesStorage>(
    OutputMessage {
        mut message,
        msg_seq_num,
    }: OutputMessage,
    session: &Session<S>,
) -> Result<Vec<u8>, OutputError> {
    let mut state = session.state().borrow_mut();
    {
        let header = &mut message.header;
        header.msg_type = message.body.msg_type();
        header.sender_comp_id = session.session_id().sender_comp_id().to_owned();
        header.target_comp_id = session.session_id().target_comp_id().to_owned();
        header.sending_time = UtcTimestamp::now_with_secs();

        // TODO: fn serialize_to(&mut buf) / fn serialize_to_buf(&mut buf)

        if let Some(msg_seq_num) = msg_seq_num {
            header.msg_seq_num = msg_seq_num;
        } else {
            header.msg_seq_num = state.next_sender_msg_seq_num();
            state.incr_next_sender_msg_seq_num();
        }
    }
    let buffer = message.serialize();
    state.store(message.header.msg_seq_num, &buffer).unwrap();

    debug!(
        "Encoded raw data: {}",
        String::from_utf8_lossy(&buffer).replace('\x01', "|")
    );
    Ok(buffer)
}

pub(crate) fn output_stream<S: MessagesStorage>(
    session: Rc<Session<S>>,
    timeout_duration: Duration,
    mut receiver: Receiver<SenderMsg>,
) -> impl Stream<Item = OutputEvent> {
    // let stream = ReceiverStream::new(receiver).timeout(timeout_duration);
    let stream = stream! {
        while let Some(sender_msg) = receiver.recv().await {
            match sender_msg {
                SenderMsg::Msg(msg) => {
                    match session.on_message_out(msg).await {
                        Ok(Some(msg)) => yield OutputEvent::Message(output_handler(msg.into(), &session).unwrap()),
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
                SenderMsg::Disconnect => yield OutputEvent::Disconnect,
            }
        }
    };
    stream.timeout(timeout_duration).map(|res| match res {
        Ok(event) => event,
        Err(Elapsed { .. }) => OutputEvent::Timeout,
    })
}
