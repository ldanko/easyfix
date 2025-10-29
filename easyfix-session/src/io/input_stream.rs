use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};

use bytes::BytesMut;
use easyfix_messages::{
    deserializer::{self, RawMessageError, raw_message},
    messages::FixtMessage,
};
use futures_util::Stream;
use pin_project::pin_project;
use tokio::io::AsyncRead;
use tokio_util::io::poll_read_buf;
use tracing::{debug, info, warn};

use crate::application::DeserializeError;

#[derive(Debug)]
pub enum InputEvent {
    Message(Box<FixtMessage>),
    DeserializeError(DeserializeError),
    IoError(io::Error),
    Timeout,
    LogoutTimeout,
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

#[pin_project]
pub struct InputStream<S> {
    buffer: BytesMut,
    #[pin]
    source: S,
}

impl<S> Stream for InputStream<S>
where
    S: AsyncRead + Unpin,
{
    type Item = InputEvent;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            // Attempt to parse a message from the buffered data.
            // If enough data has been buffered, the message is returned.
            match parse_message(this.buffer) {
                Ok(Some(msg)) => {
                    return Poll::Ready(Some(InputEvent::Message(msg)));
                }
                Ok(None) => {}
                // Convert `deserializer::DeserializeError` to `application::DeserializeError`
                // to prevent leaking ParseRejectReason to user code.
                Err(error) => {
                    return Poll::Ready(Some(InputEvent::DeserializeError(error.into())));
                }
            }

            // There is not enough buffered data to read a message.
            // Attempt to read more data from the socket.
            //
            // On success, the number of bytes is returned. `0` indicates "end
            // of stream".
            let future = poll_read_buf(Pin::new(&mut this.source), cx, this.buffer);
            match ready!(future) {
                Ok(0) => {
                    // The remote closed the connection. For this to be a clean
                    // shutdown, there should be no data in the read buffer. If
                    // there is, this means that the peer closed the socket while
                    // sending a frame.
                    if this.buffer.is_empty() {
                        info!("Stream closed");
                        return Poll::Ready(None);
                    } else {
                        warn!("Connection reset by peer");
                        return Poll::Ready(None);
                    }
                }
                Ok(_n) => continue,
                Err(err) => return Poll::Ready(Some(InputEvent::IoError(err))),
            }
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
