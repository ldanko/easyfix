use std::{collections::BTreeMap, error::Error as StdError};

use easyfix_core::{basic_types::NonZeroSeqNum, serializer::SerializeError};

/// Failure to serialize or commit an outgoing message.
#[derive(Debug, thiserror::Error)]
pub enum StoreError<E> {
    /// Serialization failed; no new record was committed and existing records
    /// remain unchanged.
    #[error("Message serialization failed: {0}")]
    Serialize(#[source] SerializeError),
    /// The backend failed; the write may have partially completed.
    #[error("Message storage failed: {0}")]
    Backend(#[source] E),
}

/// Storage for a single session's outgoing bytes and sequence counters.
///
/// One instance retains state across connections of the same session.
/// Committed records remain available and unchanged until a successful
/// [`reset`](Self::reset).
/// Records may contain arbitrary bytes; missing records are errors.
///
/// Compatibility of existing counters and history with a different session
/// identity, configuration or peer state is not validated by the library.
///
/// Backend errors end the current connection without further storage access.
/// A failed mutation may have partially completed: the backend/application must
/// restore a consistent state before reuse. Operations are not automatically
/// retried. There is no atomicity guarantee across counter updates, message
/// storage and transmission.
///
/// Getters return validated local counters loaded when the backend is opened;
/// they must not perform fallible IO. Both counters initially equal `1`.
/// `SeqNum::MAX` denotes exhausted numbering and must be retained like any other
/// value.
///
/// Durability across process or system failure depends on the backend.
/// Deferred backend failures are reported only by a subsequent fallible call.
pub trait MessagesStorage {
    /// The backend's own error, including missing records and occupied keys.
    type Error: StdError + 'static;

    /// Serialize and commit exactly the returned prefix under `seq_num`.
    ///
    /// The closure receives at least the configured `max_message_size` bytes
    /// and runs at most once. Returning a length exceeding that buffer is a
    /// caller logic error. An occupied key must return a backend error without
    /// changing its original record; keys need not be written in order.
    ///
    /// Success makes the exact bytes available to subsequent [`fetch`](Self::fetch)
    /// calls until reset. See [`StoreError`] for the guarantees on failure.
    fn store(
        &mut self,
        seq_num: NonZeroSeqNum,
        serialize: impl FnOnce(&mut [u8]) -> Result<usize, SerializeError>,
    ) -> Result<(), StoreError<Self::Error>>;

    /// Borrow the exact committed bytes, or return an error if unavailable.
    ///
    /// The slice must remain valid and unchanged throughout its borrow,
    /// including an asynchronous transport write.
    ///
    /// `range_end_hint` is advisory: reads may repeat, run out of order,
    /// interleave with new writes, or stop before the hinted end. A single
    /// message read uses its own sequence number as the hint.
    #[expect(async_fn_in_trait, reason = "single-threaded runtime, Send not needed")]
    async fn fetch(
        &mut self,
        seq_num: NonZeroSeqNum,
        range_end_hint: NonZeroSeqNum,
    ) -> Result<&[u8], Self::Error>;

    /// Validated local sequence number for the next outgoing message.
    fn next_sender_msg_seq_num(&self) -> NonZeroSeqNum;

    /// Validated local sequence number expected on the next incoming message.
    fn next_target_msg_seq_num(&self) -> NonZeroSeqNum;

    /// Override the outgoing counter; success makes it visible to the getter.
    fn set_next_sender_msg_seq_num(&mut self, seq_num: NonZeroSeqNum) -> Result<(), Self::Error>;

    /// Override the incoming counter; success makes it visible to the getter.
    fn set_next_target_msg_seq_num(&mut self, seq_num: NonZeroSeqNum) -> Result<(), Self::Error>;

    /// Restore both counters to `1` and discard committed message bytes.
    /// An error may leave partial changes and requires recovery before reuse.
    fn reset(&mut self) -> Result<(), Self::Error>;
}

/// Errors returned by [`InMemoryStorage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InMemoryStorageError {
    /// No committed record exists under the requested number.
    #[error("No stored message for sequence number {0}")]
    MissingMessage(NonZeroSeqNum),
    /// A committed record already occupies the requested number.
    #[error("Message already stored for sequence number {0}")]
    DuplicateSequenceNumber(NonZeroSeqNum),
}

/// In-memory session storage. Every committed record remains available until
/// reset, including across reconnects. Data does not survive process restart.
pub struct InMemoryStorage {
    next_sender_msg_seq_num: NonZeroSeqNum,
    next_target_msg_seq_num: NonZeroSeqNum,
    scratch: Box<[u8]>,
    messages: BTreeMap<NonZeroSeqNum, Vec<u8>>,
}

impl InMemoryStorage {
    /// Create storage with a serialization buffer of `max_message_size` bytes.
    pub fn new(max_message_size: usize) -> InMemoryStorage {
        InMemoryStorage {
            next_sender_msg_seq_num: NonZeroSeqNum::MIN,
            next_target_msg_seq_num: NonZeroSeqNum::MIN,
            scratch: vec![0u8; max_message_size].into_boxed_slice(),
            messages: BTreeMap::new(),
        }
    }
}

impl MessagesStorage for InMemoryStorage {
    type Error = InMemoryStorageError;

    fn store(
        &mut self,
        seq_num: NonZeroSeqNum,
        serialize: impl FnOnce(&mut [u8]) -> Result<usize, SerializeError>,
    ) -> Result<(), StoreError<Self::Error>> {
        if self.messages.contains_key(&seq_num) {
            return Err(StoreError::Backend(
                InMemoryStorageError::DuplicateSequenceNumber(seq_num),
            ));
        }
        let len = serialize(&mut self.scratch).map_err(StoreError::Serialize)?;
        self.messages.insert(seq_num, self.scratch[..len].to_vec());
        Ok(())
    }

    async fn fetch(
        &mut self,
        seq_num: NonZeroSeqNum,
        _range_end_hint: NonZeroSeqNum,
    ) -> Result<&[u8], Self::Error> {
        self.messages
            .get(&seq_num)
            .map(Vec::as_slice)
            .ok_or(InMemoryStorageError::MissingMessage(seq_num))
    }

    fn next_sender_msg_seq_num(&self) -> NonZeroSeqNum {
        self.next_sender_msg_seq_num
    }

    fn next_target_msg_seq_num(&self) -> NonZeroSeqNum {
        self.next_target_msg_seq_num
    }

    fn set_next_sender_msg_seq_num(&mut self, seq_num: NonZeroSeqNum) -> Result<(), Self::Error> {
        self.next_sender_msg_seq_num = seq_num;
        Ok(())
    }

    fn set_next_target_msg_seq_num(&mut self, seq_num: NonZeroSeqNum) -> Result<(), Self::Error> {
        self.next_target_msg_seq_num = seq_num;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), Self::Error> {
        self.next_sender_msg_seq_num = NonZeroSeqNum::MIN;
        self.next_target_msg_seq_num = NonZeroSeqNum::MIN;
        self.messages.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
