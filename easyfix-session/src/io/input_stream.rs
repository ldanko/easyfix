use std::{
    io,
    marker::PhantomData,
    num::NonZeroUsize,
    pin::Pin,
    task::{Context, Poll, ready},
};

use bytes::{Buf, BytesMut};
use easyfix_core::{
    basic_types::{FixString, NonZeroLength, TagNum},
    deserializer::{DeserializeErrorKind, Deserializer, RawMessageError, frame_len, raw_message},
    message::{DeserializeError, SessionMessage},
    version::Version,
};
use futures_util::Stream;
use memchr::memmem;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::io::poll_read_buf;
use tracing::{info, trace, warn};

use crate::session_id::SessionId;

#[cfg(test)]
mod tests;

/// Events yielded by [`InputStream`].
///
/// Timer-related events (input timeout, logout timeout) are managed by
/// the session loop, not the input stream.
#[derive(Debug)]
pub(crate) enum InputEvent<M> {
    Message(Box<M>),
    DeserializeError(DeserializeError),
    IoError(io::Error),
    /// The frame at the front of the input declares a length above the
    /// stream's limit. Reported from the framing fields alone, before the
    /// body is read; the bytes stay buffered.
    TooLarge {
        frame_len: usize,
    },
}

/// The declared length of the frame at the front of `buf` when it exceeds
/// `limit`. `None` while the framing fields are still arriving, when the
/// bytes are not a frame at all (`parse_message` reports that), or when the
/// frame fits.
fn oversized_frame(buf: &[u8], limit: usize) -> Option<usize> {
    frame_len(buf).ok().filter(|&len| len > limit)
}

/// Result of [`InputStream::first_message`] - like [`InputEvent`], but a
/// decode failure of a Logon(35=A) also carries the best-effort identity
/// recovered from the failed message's bytes, so the acceptor can answer it
/// per Test Cases Scenario 1S(d). That identity is `None` when the input was
/// garbled (no message span exists), the message was not a Logon, or the
/// identity could not be established beyond doubt.
#[derive(Debug)]
pub(crate) enum FirstMessageEvent<M> {
    Message(Box<M>),
    DeserializeError {
        error: DeserializeError,
        invalid_logon_identity: Option<SessionId>,
    },
    IoError(io::Error),
    /// The frame at the front of the input declares a length above the
    /// stream's limit. Reported from the framing fields alone, before the
    /// body is read; the bytes stay buffered.
    TooLarge {
        frame_len: usize,
    },
}

/// Start of every FIX message: `BeginString(8)` with its value prefix.
const MESSAGE_START: &[u8] = b"8=FIX";

/// Drops the garbled bytes at the front of `buf`, up to the next plausible
/// message start. Called after `raw_message` rejected offset 0.
fn process_garbled_data(buf: &mut BytesMut) {
    let len = buf.len();
    // Search for "8=FIX" - the start of any valid FIX message.
    // Uses SIMD-accelerated search instead of byte-by-byte raw_message() calls.
    // The search starts at index 1: offset 0 was just parsed and failed, so
    // it cannot be a valid message start.
    for offset in memmem::find_iter(&buf[1..], MESSAGE_START) {
        let offset = offset + 1;
        if let Ok(_) | Err(RawMessageError::Incomplete) = raw_message(&buf[offset..]) {
            buf.advance(offset);
            info!("dropped {offset} bytes of garbled message");
            return;
        }
    }
    // No complete needle - but the next message may start in the tail with
    // its `8=FIX` cut by the read boundary. Keep the longest tail that is a
    // proper prefix of the needle so the next read can complete it; every
    // such prefix is `Incomplete` to `raw_message`, never garbled again.
    // Offset 0 is never kept (it was just rejected), so this always makes
    // progress and `poll_next` cannot spin on the same bytes.
    let kept = (1..MESSAGE_START.len())
        .rev()
        .filter(|&n| n < len)
        .find(|&n| buf.ends_with(&MESSAGE_START[..n]))
        .unwrap_or(0);
    let dropped = len - kept;
    buf.advance(dropped);
    info!("dropped {dropped} bytes of garbled message");
}

fn parse_message<M: SessionMessage>(
    bytes: &mut BytesMut,
) -> Result<Option<Box<M>>, DeserializeError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    // The `trace!` macro evaluates its arguments lazily - the allocation
    // below happens only when a subscriber enables TRACE for this target,
    // e.g. RUST_LOG=easyfix_session::io::input_stream=trace.
    trace!(
        "Raw data input :: {}",
        String::from_utf8_lossy(bytes).replace('\x01', "|")
    );

    let src_len = bytes.len();

    match raw_message(bytes) {
        Ok((leftover, raw_msg)) => {
            let result = M::from_raw_message(raw_msg).map(Some);
            bytes.advance(src_len - leftover.len());
            result
        }
        Err(RawMessageError::Incomplete) => Ok(None),
        // The frame is complete and its extent is known, so the whole
        // message is skipped in one step. Scanning for the next `8=FIX`
        // instead could land on a look-alike inside the rejected body (a
        // Text(58) carrying one, say) and stall the stream on a phantom
        // frame until enough bytes arrive to disprove it.
        Err(err @ RawMessageError::InvalidChecksum { frame_len }) => {
            bytes.advance(frame_len);
            info!("dropped {frame_len} bytes of message with invalid checksum");
            Err(err.into())
        }
        Err(err) => {
            process_garbled_data(bytes);
            Err(err.into())
        }
    }
}

/// Best-effort identity recovery from a framed Logon(35=A) that failed
/// decoding - Test Cases Scenario 1S(d) needs the peer identity of an
/// invalid first Logon to answer it. Scans the standard-header region
/// for SenderCompID(49) and TargetCompID(56) with the same
/// [`Deserializer`] primitives the real decoder uses, and derives the
/// `SessionId` like `SessionId::from_inbound` (CompIDs swapped).
///
/// The scan is deliberately conservative: any ambiguity yields `None`, and
/// the caller then falls back to the silent drop (Scenarios 1S(b)/(c)).
//
// Answering on a misidentified connection is worse than that silent drop, so
// every doubtful case bails out. What the scan does, and where it gives up:
//
// - BeginString(8) unknown - no `Version`, so no identity;
// - MsgType(35) must be the third tag on the wire, as the TagValue encoding
//   mandates, and must be a Logon: a first message of any other type is
//   dropped without a response (Scenario 2S), so no identity is recovered
//   for it;
// - the walk is confined to the closed set of standard-header tags; the first
//   tag outside it marks the message body and ends the scan, so Data-typed
//   body fields under arbitrary (including custom) tags are never interpreted
//   as tag soup. Known limitation: a dictionary may extend the header with
//   custom tags, and those cannot be safely skipped either (a custom Data
//   field's content would become tag soup) - a custom header tag placed
//   before 49/56 therefore ends the scan with `None`, the safe direction.
//   CompIDs conventionally follow 35 directly, so the window is narrow;
// - header Data fields must honor the TagValue pairing rule (length field
//   immediately before its data field: 90/91, 212/213) - their content is
//   skipped by the declared length; a data field with broken pairing cannot
//   be delimited and aborts the scan;
// - a duplicated 35/49/56 aborts the scan;
// - 49 or 56 missing from the walked region yields `None`.
fn invalid_logon_identity(bytes: &[u8]) -> Option<SessionId> {
    let (_leftover, raw_msg) = raw_message(bytes).ok()?;
    let version: Version = raw_msg.begin_string.as_utf8().parse().ok()?;
    let mut de = Deserializer::from_raw_message(raw_msg);

    if !matches!(de.deserialize_tag_num(), Ok(Some(35))) {
        return None;
    }
    let msg_type_range = de.deserialize_msg_type().ok()?;
    if de.range_to_fixstr(msg_type_range).as_bytes() != b"A" {
        return None;
    }

    let mut sender_comp_id: Option<FixString> = None;
    let mut target_comp_id: Option<FixString> = None;

    loop {
        let tag = match de.deserialize_tag_num() {
            Ok(Some(tag)) => tag,
            // End of body - the whole message was walked.
            Ok(None) => break,
            // A malformed tag cannot be a standard-header field - treat
            // it like any out-of-domain tag: stop, keeping what was
            // collected; nothing past it is interpreted.
            Err(_) => break,
        };
        match tag {
            tag @ (49 | 56) => {
                let slot = if tag == 49 {
                    &mut sender_comp_id
                } else {
                    &mut target_comp_id
                };
                // A duplicated identity tag marks the message as too
                // damaged to trust.
                if slot
                    .replace(de.deserialize_str().ok()?.to_owned())
                    .is_some()
                {
                    return None;
                }
            }
            // A second MsgType(35) marks the message as too damaged to
            // trust.
            35 => return None,
            // Length-prefixed pairs (TagValue encoding: the length field
            // stands immediately before its data field). The data
            // content is skipped by the declared length, never scanned
            // for tags.
            90 | 212 => {
                let len = de.deserialize_length().ok()? as usize;
                let data_tag = if tag == 90 { 91 } else { 213 };
                if !matches!(de.deserialize_tag_num(), Ok(Some(t)) if t == data_tag) {
                    return None;
                }
                if tag == 90 {
                    de.deserialize_data(len).ok()?;
                } else {
                    de.deserialize_xml(len).ok()?;
                }
            }
            // A data field without its length field directly before it -
            // the content cannot be delimited, and scanning into it
            // could fabricate tags. Give up.
            91 | 213 => return None,
            tag if is_standard_header_tag(tag) => {
                // Any other header field - skip its value.
                de.deserialize_str().ok()?;
            }
            // First tag outside the standard header - the body starts
            // here, nothing past this point is walkable.
            _ => break,
        }
    }

    Some(SessionId::new(version, target_comp_id?, sender_comp_id?))
}

/// Tags of the FIXT 1.1 / FIX 4.x standard header. BeginString(8) and
/// BodyLength(9) are absent - `raw_message` framing consumes them before
/// the scan in [`invalid_logon_identity`] starts.
fn is_standard_header_tag(tag: TagNum) -> bool {
    matches!(
        tag,
        35 | 49 | 56 | 34 | 52          // MsgType, Sender/TargetCompID, MsgSeqNum, SendingTime
            | 43 | 97 | 122             // PossDupFlag, PossResend, OrigSendingTime
            | 50 | 142 | 57 | 143       // Sender/Target SubID + LocationID
            | 115 | 116 | 144 | 370     // OnBehalfOf CompID/SubID/LocationID/SendingTime
            | 128 | 129 | 145           // DeliverTo CompID/SubID/LocationID
            | 90 | 91 | 212 | 213       // SecureDataLen/SecureData, XmlDataLen/XmlData
            | 347 | 369                 // MessageEncoding, LastMsgSeqNumProcessed
            | 627 | 628 | 629 | 630     // NoHops group
            | 1128 | 1129 | 1156 // ApplVerID, CstmApplVerID, ApplExtID
    )
}

/// Cancel-safe input stream over an `AsyncRead` source.
///
/// Buffers incoming bytes in a `BytesMut` and yields complete FIX messages
/// as they become available. Garbled data is detected and skipped.
pub(crate) struct InputStream<S, M> {
    buffer: BytesMut,
    source: S,
    /// Largest frame the stream lets through. A frame declaring more is
    /// reported as `TooLarge` from its framing fields, before its body is
    /// read - so this, not the framing ceiling, is what bounds the buffer.
    max_message_size: usize,
    _message: PhantomData<fn() -> M>,
}

/// The input buffer starts with room for this many messages of
/// `max_message_size` bytes, so a burst of them lands in one read.
///
/// The buffer never grows past that. A read is issued only when the buffer
/// holds no complete message, that is at most the front of one frame - and
/// that frame is under `max_message_size`, or it would have been refused
/// from its framing fields (`oversized_frame`), or dropped as garbled by
/// `raw_message` before those fields could grow past their own bounds. So
/// the buffer holds less than one message's worth of bytes whenever it
/// reads, and the read adds at most the spare capacity.
const BUFFER_CAPACITY_FACTOR: usize = 16;

impl<S, M> InputStream<S, M> {
    /// Stream for an identified session: the buffer starts large enough to
    /// take a burst of `max_message_size` messages in one read.
    pub(crate) fn new(source: S, max_message_size: NonZeroLength) -> Self {
        let max_message_size = usize::from(max_message_size.get());
        InputStream {
            buffer: BytesMut::with_capacity(BUFFER_CAPACITY_FACTOR * max_message_size),
            source,
            max_message_size,
            _message: PhantomData,
        }
    }

    /// Stream for the first message of a connection whose session - and so
    /// whose `max_message_size` - is not known yet: `limit` stands in for it.
    /// The buffer starts at exactly that limit rather than a burst's worth:
    /// nothing is known about the peer, and `BUFFER_CAPACITY_FACTOR` times
    /// the limit per unauthenticated connection would pay for a burst that a
    /// single first message never is.
    pub(crate) fn for_first_message(source: S, limit: NonZeroUsize) -> Self {
        InputStream {
            buffer: BytesMut::with_capacity(limit.get()),
            source,
            max_message_size: limit.get(),
            _message: PhantomData,
        }
    }

    /// Construct an [`InputStream`] with a pre-existing buffer. Used by
    /// the acceptor when it reads the first (Logon) message with a
    /// small buffer, then continues with the session's full
    /// `max_message_size` while preserving any bytes already buffered past
    /// the first message.
    pub(crate) fn from_parts(
        source: S,
        mut buffer: BytesMut,
        max_message_size: NonZeroLength,
    ) -> Self {
        let max_message_size = usize::from(max_message_size.get());
        let target = BUFFER_CAPACITY_FACTOR * max_message_size;
        if buffer.capacity() < target {
            // `reserve` guarantees `capacity >= len + additional`, so the
            // top-up must be computed from `len`, not `capacity`.
            buffer.reserve(target - buffer.len());
        }
        InputStream {
            buffer,
            source,
            max_message_size,
            _message: PhantomData,
        }
    }

    /// Decompose the stream, returning the reader and any buffered
    /// bytes. Used by the acceptor to hand off the underlying TCP
    /// connection to a freshly-sized [`InputStream`] after the first
    /// message has identified the session.
    pub(crate) fn into_parts(self) -> (S, BytesMut) {
        (self.source, self.buffer)
    }
}

impl<S, M> InputStream<S, M>
where
    S: AsyncRead + Unpin,
    M: SessionMessage,
{
    /// Read the acceptor's first message. Behaves like the `Stream` impl
    /// (`None` on EOF), except that a decode failure of a Logon also
    /// hands out the identity recovered from the failed message - see
    /// [`FirstMessageEvent`]. Cancel-safe: partial reads stay buffered.
    ///
    /// A frame declaring more than the stream's limit stops the read with
    /// [`FirstMessageEvent::TooLarge`] as soon as its framing fields are in.
    pub(crate) async fn first_message(&mut self) -> Option<FirstMessageEvent<M>> {
        loop {
            if let Some(frame_len) = oversized_frame(&self.buffer, self.max_message_size) {
                return Some(FirstMessageEvent::TooLarge { frame_len });
            }
            // Copy the framed message aside - `parse_message` consumes the
            // failed message's bytes before returning the error, and
            // identity recovery needs them. Only a complete message can
            // fail to decode, so nothing is copied until one is framed,
            // and then only the message itself: at most once per
            // connection, bounded by the framing ceiling.
            //
            // Framing an incomplete buffer stops at BodyLength, so this
            // check costs nothing per read. Snapshotting the whole buffer
            // unconditionally - as this did - copied it once per socket
            // read instead, which a peer drip-feeding the first message
            // turns into quadratic work before any identity is known.
            let framed = match raw_message(&self.buffer) {
                Ok((leftover, _)) => {
                    let len = self.buffer.len() - leftover.len();
                    Some(self.buffer[..len].to_vec())
                }
                Err(_) => None,
            };
            match parse_message::<M>(&mut self.buffer) {
                Ok(Some(msg)) => return Some(FirstMessageEvent::Message(msg)),
                Ok(None) => {}
                // Garbled input: the consumed span is dropped garbage,
                // not a message - see `process_garbled_data`. There is
                // nothing to recover an identity from.
                Err(error) if matches!(error.kind, DeserializeErrorKind::Garbled(_)) => {
                    return Some(FirstMessageEvent::DeserializeError {
                        error,
                        invalid_logon_identity: None,
                    });
                }
                Err(error) => {
                    return Some(FirstMessageEvent::DeserializeError {
                        error,
                        invalid_logon_identity: framed.as_deref().and_then(invalid_logon_identity),
                    });
                }
            }

            // Not enough buffered data - read more from the socket.
            match self.source.read_buf(&mut self.buffer).await {
                Ok(0) => {
                    info!("Stream closed before first message");
                    return None;
                }
                Ok(_) => continue,
                Err(err) => return Some(FirstMessageEvent::IoError(err)),
            }
        }
    }

    /// Read until the peer closes the connection, discarding everything -
    /// bytes already buffered included. For the wait after a Logout exchange,
    /// where nothing the peer sends is owed a look and only the close matters.
    /// Cancel-safe: a partial read is dropped like everything else.
    pub(crate) async fn discard_until_closed(&mut self) -> io::Result<()> {
        let mut discarded = self.buffer.len();
        loop {
            self.buffer.clear();
            match self.source.read_buf(&mut self.buffer).await? {
                0 => break,
                n => discarded += n,
            }
        }
        if discarded > 0 {
            warn!(
                bytes = discarded,
                "peer kept sending after the Logout exchange, discarded"
            );
        }
        Ok(())
    }
}

impl<S, M> Stream for InputStream<S, M>
where
    S: AsyncRead + Unpin,
    M: SessionMessage,
{
    type Item = InputEvent<M>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            if let Some(frame_len) = oversized_frame(&this.buffer, this.max_message_size) {
                return Poll::Ready(Some(InputEvent::TooLarge { frame_len }));
            }
            match parse_message::<M>(&mut this.buffer) {
                Ok(Some(msg)) => {
                    return Poll::Ready(Some(InputEvent::Message(msg)));
                }
                Ok(None) => {}
                Err(error) => {
                    return Poll::Ready(Some(InputEvent::DeserializeError(error)));
                }
            }

            // Not enough buffered data - read more from the socket.
            match ready!(poll_read_buf(
                Pin::new(&mut this.source),
                cx,
                &mut this.buffer
            )) {
                Ok(0) => {
                    if this.buffer.is_empty() {
                        info!("Stream closed");
                    } else {
                        warn!(
                            buffered_bytes = this.buffer.len(),
                            "Stream closed with partial message in buffer"
                        );
                    }
                    return Poll::Ready(None);
                }
                Ok(_) => continue,
                Err(err) => return Poll::Ready(Some(InputEvent::IoError(err))),
            }
        }
    }
}
