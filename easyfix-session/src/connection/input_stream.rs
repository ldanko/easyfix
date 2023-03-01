use std::io;

use async_stream::stream;
use bytes::BytesMut;
use easyfix_messages::{
    deserializer::{raw_message, DeserializeError, RawMessageError},
    messages::FixtMessage,
};
use futures_util::Stream;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_stream::StreamExt;
use tracing::{debug, error, info};

use super::Disconnect;

pub enum InputEvent {
    Message(Box<FixtMessage>),
    DeserializeError(DeserializeError),
    IoError(io::Error),
    Timeout,
}

fn process_garbled_data(buf: &mut BytesMut) {
    let len = buf.len();
    for i in 1..buf.len() {
        if let Ok(_) | Err(RawMessageError::Incomplete) = raw_message(&buf[i..]) {
            buf.split_to(i).freeze();
            info!("dropped {i} bytes of garbled message");
            return;
        }
    }
    buf.clear();
    info!("dropped {len} bytes of garbled message");
}

fn parse_message(bytes: &mut BytesMut) -> Result<Option<Box<FixtMessage>>, DeserializeError> {
    debug!(
        "Raw data input :: {}",
        String::from_utf8_lossy(bytes).replace('\x01', "|")
    );
    if bytes.is_empty() {
        debug!("Decoding stream empty");
        return Ok(None);
    }

    let src_len = bytes.len();

    match raw_message(bytes) {
        Ok((leftover, raw_msg)) => {
            let result = FixtMessage::from_raw_message(raw_msg);
            let leftover_len = leftover.len();
            bytes.split_to(src_len - leftover_len).freeze();
            match result {
                Err(e) => Err(e),
                Ok(msg) => Ok(Some(msg)),
            }
        }
        Err(RawMessageError::Incomplete) => Ok(None),
        Err(RawMessageError::Garbled) => {
            process_garbled_data(bytes);
            Err(DeserializeError::GarbledMessage(
                "Message not well formed".into(),
            ))
        }
    }
}

async fn input_handler(
    stream: &mut (impl AsyncRead + Unpin),
    buffer: &mut BytesMut,
) -> Result<Option<InputEvent>, Disconnect> {
    // Attempt to parse a frame from the buffered data. If enough data
    // has been buffered, the frame is returned.
    match parse_message(buffer) {
        Ok(Some(msg)) => return Ok(Some(InputEvent::Message(msg))),
        Ok(None) => {}
        Err(error) => return Ok(Some(InputEvent::DeserializeError(error))),
    }

    // There is not enough buffered data to read a frame. Attempt to
    // read more data from the socket.
    //
    // On success, the number of bytes is returned. `0` indicates "end
    // of stream".
    match stream.read_buf(buffer).await {
        Ok(0) => {
            // The remote closed the connection. For this to be a clean
            // shutdown, there should be no data in the read buffer. If
            // there is, this means that the peer closed the socket while
            // sending a frame.
            if buffer.is_empty() {
                return Err(Disconnect);
            } else {
                error!("Connection reset by peer");
                return Err(Disconnect);
            }
        }
        Ok(_) => {}
        Err(error) => return Ok(Some(InputEvent::IoError(error))),
    }
    Ok(None)
}

pub fn input_stream(mut source: impl AsyncRead + Unpin) -> impl Stream<Item = InputEvent> {
    let mut buffer = BytesMut::with_capacity(4096);
    stream! {
        loop {
            match input_handler(&mut source, &mut buffer).await {
                Ok(Some(event)) => yield event,
                Ok(None) => {},
                Err(_) => break,
            }
        }
    }
}
