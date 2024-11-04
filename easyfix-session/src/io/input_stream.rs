use std::{
    future::Future,
    io,
    pin::pin,
    task::{ready, Context, Poll},
};

use bytes::BytesMut;
use easyfix_messages::{
    deserializer::{self, raw_message, RawMessageError},
    messages::FixtMessage,
};
use futures_util::Stream;
use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::{debug, info, warn};

use crate::application::DeserializeError;

pub enum InputEvent {
    Message(Box<FixtMessage>),
    DeserializeError(DeserializeError),
    IoError(io::Error),
    Timeout,
}

struct Disconnect;

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

fn parse_message(
    bytes: &mut BytesMut,
) -> Result<Option<Box<FixtMessage>>, deserializer::DeserializeError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    debug!(
        "Raw data input :: {}",
        String::from_utf8_lossy(bytes).replace('\x01', "|")
    );

    let src_len = bytes.len();

    match raw_message(bytes) {
        Ok((leftover, raw_msg)) => {
            let result = FixtMessage::from_raw_message(raw_msg).map(Some);
            let leftover_len = leftover.len();
            bytes.split_to(src_len - leftover_len).freeze();
            result
        }
        Err(RawMessageError::Incomplete) => Ok(None),
        Err(err) => {
            process_garbled_data(bytes);
            Err(err.into())
        }
    }
}

async fn try_read_fix_msg(
    source: &mut (impl AsyncRead + Unpin),
    buffer: &mut BytesMut,
) -> Result<Option<InputEvent>, Disconnect> {
    // Attempt to parse a frame from the buffered data. If enough data
    // has been buffered, the frame is returned.
    match parse_message(buffer) {
        Ok(Some(msg)) => return Ok(Some(InputEvent::Message(msg))),
        Ok(None) => {}
        // Convert `deserializer::DeserializeError` to `application::DeserializeError`
        // to prevent leaking ParseRejectReason to user code.
        Err(error) => return Ok(Some(InputEvent::DeserializeError(error.into()))),
    }

    // There is not enough buffered data to read a frame. Attempt to
    // read more data from the socket.
    //
    // On success, the number of bytes is returned. `0` indicates "end
    // of stream".
    match source.read_buf(buffer).await {
        Ok(0) => {
            // The remote closed the connection. For this to be a clean
            // shutdown, there should be no data in the read buffer. If
            // there is, this means that the peer closed the socket while
            // sending a frame.
            if buffer.is_empty() {
                return Err(Disconnect);
            } else {
                warn!("Connection reset by peer");
                return Err(Disconnect);
            }
        }
        Ok(_) => {}
        Err(error) => return Ok(Some(InputEvent::IoError(error))),
    }
    Ok(None)
}

pub struct InputStream<S> {
    buffer: BytesMut,
    source: S,
}

impl<S> Stream for InputStream<S>
where
    S: AsyncRead + Unpin,
{
    type Item = InputEvent;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        let future = pin!(try_read_fix_msg(&mut this.source, &mut this.buffer));
        match ready!(future.poll(cx)) {
            Ok(Some(event)) => Poll::Ready(Some(event)),
            Ok(None) => Poll::Pending,
            Err(_) => Poll::Ready(None),
        }
    }
}

pub fn input_stream<S>(source: S) -> InputStream<S>
where
    S: AsyncRead + Unpin,
{
    InputStream {
        // TODO: Max MSG size
        buffer: BytesMut::with_capacity(4096),
        source,
    }
}
