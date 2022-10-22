use std::{io, rc::Rc};

use bytes::BytesMut;
use easyfix_messages::{
    deserializer::DeserializeError,
    fields::{MsgType, SeqNum, UtcTimestamp},
    messages::FixtMessage,
    parser::{self, raw_message},
};
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder};
use tracing::{debug, error, trace};

use crate::{messages_storage::MessagesStorage, session::Session};

pub struct FixDecoder {}

impl Default for FixDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FixDecoder {
    pub fn new() -> FixDecoder {
        FixDecoder {}
    }
}

fn drop_broken_bytes(buf: &mut BytesMut) {
    let mut iter = buf.iter();
    let mut i = 0;

    trace!(
        "#{}: {}",
        i,
        String::from_utf8_lossy(iter.as_slice()).replace('\x01', "|")
    );
    while let Some(_) = iter.next() {
        i += 1;
        trace!(
            "#{}: {}",
            i,
            String::from_utf8_lossy(iter.as_slice()).replace('\x01', "|")
        );
        if let [b'8'] | [b'8', b'=', ..] = iter.as_slice() {
            buf.split_to(i).freeze();
            return;
        }
    }
    buf.clear();
}

impl Decoder for FixDecoder {
    type Error = io::Error;
    type Item = Result<Box<FixtMessage>, DeserializeError>;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        debug!(
            "Raw data input :: {}",
            String::from_utf8_lossy(src).replace('\x01', "|")
        );
        if src.is_empty() {
            debug!("Decoding stream empty");
            return Ok(None);
        }

        let src_len = src.len();

        let raw_msg = raw_message(src);

        match raw_msg {
            Ok((leftover, raw_msg)) => {
                let result = FixtMessage::from_raw_message(raw_msg);
                let leftover_len = leftover.len();
                src.split_to(src_len - leftover_len).freeze();
                match result {
                    Err(e) => Ok(Some(Err(e))),
                    Ok(msg) => Ok(Some(Ok(msg))),
                }
            }
            Err(parser::Err::Incomplete(_)) => Ok(None),
            Err(error) => {
                error!("Error decoding message: {}", error);
                drop_broken_bytes(src);
                Ok(Some(Err(DeserializeError::GarbledMessage(
                    "Message not well formed".into(),
                ))))
            }
        }
    }
}

pub struct EncoderMessage {
    message: Box<FixtMessage>,
    msg_seq_num: Option<SeqNum>,
}

impl From<Box<FixtMessage>> for EncoderMessage {
    fn from(message: Box<FixtMessage>) -> Self {
        EncoderMessage {
            message,
            msg_seq_num: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum FixEncoderError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("outbound MsgSeqNum max value exceeded")]
    OutboundMsgSeqNumMaxExceeded,
}

pub(crate) struct FixEncoder<S> {
    session: Rc<Session<S>>,

    out_buf: Vec<u8>,
}

impl<S: MessagesStorage> FixEncoder<S> {
    pub fn new(session: Rc<Session<S>>) -> FixEncoder<S> {
        FixEncoder {
            session,
            out_buf: Vec::new(),
        }
    }
}

impl<S: MessagesStorage> Encoder<EncoderMessage> for FixEncoder<S> {
    type Error = FixEncoderError;

    fn encode(
        &mut self,
        EncoderMessage {
            mut message,
            msg_seq_num,
        }: EncoderMessage,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        /*
        let mut state = self.state.borrow_mut();
        let header = &mut outbound_message.message.header;
        //Setup message to go out and serialize it.
        if outbound_message.auto_msg_seq_num {
          header.msg_seq_num = state.outbound_msg_seq_num;
          state
            .increment_outbound_msg_seq_num()
            .map_err(|_| FixEncoderError::OutboundMsgSeqNumMaxExceeded)?;
        }
          */

        let mut state = self.session.state().borrow_mut();
        {
            let header = &mut message.header;
            header.msg_type = message.body.msg_type();
            header.sender_comp_id = self.session.session_id().sender_comp_id().to_owned();
            header.target_comp_id = self.session.session_id().target_comp_id().to_owned();
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

        /*
        unsafe {
          verbose9!(
            "write to mem: {}",
            String::from_utf8_lossy(&buffer).replace('\x01', "|")
          );
          let len = u16::try_from(buffer.len()).expect_ext("message size");
          // TODO: Double copying! We need API to write directly to MemMut
          self.out_buf.extend_from_slice(&len.to_ne_bytes());
          self.out_buf.extend_from_slice(&buffer);
          self.mem.write_bytes(&self.out_buf)?;
          self.out_buf.clear();
        };
        */

        dst.extend_from_slice(&buffer);
        debug!(
            "Encoded raw data: {}",
            String::from_utf8_lossy(dst).replace('\x01', "|")
        );
        Ok(())
    }
}
