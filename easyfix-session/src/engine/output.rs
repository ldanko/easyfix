//! Outgoing header preparation, serialization, and pending output.

use std::{borrow::Cow, mem};

use easyfix_core::{
    base_messages::{AdminBase, HeaderBase, MsgTypeBase},
    basic_types::{FixStr, FixString, NonZeroSeqNum, SeqNum, UtcTimestamp},
    message::SessionMessage,
};
use tracing::{error, warn};

use super::{FatalError, HeaderFill, PendingOutput, SendFailure, SerializeFailure, SessionEngine};
use crate::{
    application::DisconnectReason,
    messages_storage::{MessagesStorage, StoreError},
};

/// Encode an optional `Text(58)` on `msg_type`, dropping an empty string with
/// a warning.
//
// A FIX field must carry a value - Session Layer reserves
// `SessionRejectReason(373) = 4, Tag specified without a value` for a peer that
// sends one anyway - so an empty `Text` has no legal encoding and the
// serializer refuses it (`SerializeError::EmptyValue`). Omitting the field is
// therefore the only way to say "no text", and `Text(58)` is optional on both
// `Reject<3>` and `Logout<5>` (FIX Transport §5.5). Without this an empty
// string would fail serialization and cost the whole message, not just the
// field.
//
// Dropping it is a repair, not a normalization: every engine-internal caller
// supplies a real diagnostic, so an empty string can only have come from an
// `InputAction` and is an application bug. Hence the warning, and hence no
// `debug_assert` - the message is worth sending without its `Text` (for
// `Logout` it is the mandatory half of the Scenario 1S(d) escalation), and
// killing the session over a lost diagnostic field would cost more than the
// bug does. `msg_type` is a parameter because `push_admin` runs outside the
// `msg` span, so the log has no other way to say which message lost the field.
pub(super) fn text_field(
    text: Option<FixString>,
    msg_type: MsgTypeBase,
) -> Option<Cow<'static, FixStr>> {
    match text {
        Some(text) if text.is_empty() => {
            warn!(
                ?msg_type,
                "empty Text(58) dropped - a FIX field cannot carry an empty value; pass None"
            );
            None
        }
        other => other.map(Cow::Owned),
    }
}

impl<M: SessionMessage> SessionEngine<M> {
    /// Mark the scratch buffer as free for the next writer.
    pub(crate) fn mark_scratch_clean(&mut self) {
        self.scratch.mark_clean();
    }

    pub(crate) fn scratch(&self) -> &[u8] {
        self.scratch.as_slice()
    }

    /// Mutable access to scratch buffer. Used by tests to set up
    /// transient message data for flush_output testing.
    #[cfg(test)]
    pub(crate) fn scratch_mut(&mut self) -> &mut [u8] {
        self.scratch.as_mut_slice()
    }

    /// Push a pending output entry directly. Used by tests to set up
    /// PendingOutput entries for flush_output testing.
    #[cfg(test)]
    pub(crate) fn push_pending(&mut self, pending: PendingOutput) {
        self.output.push_back(pending);
    }

    pub(crate) fn take_pending(&mut self) -> Option<PendingOutput> {
        let entry = self.output.pop_front();
        // A `Transient` references the shared scratch buffer; the IO loop
        // will write it to TCP immediately after this call. Mark scratch
        // clean now so the next scratch writer can reuse the buffer.
        if matches!(entry, Some(PendingOutput::Transient { .. })) {
            self.mark_scratch_clean();
        }
        entry
    }

    pub(crate) fn take_admin_output(&mut self) -> Option<Box<M>> {
        self.admin_output.pop_front()
    }

    pub(crate) fn has_admin_output(&self) -> bool {
        !self.admin_output.is_empty()
    }

    /// Build an admin message and push to admin_output.
    pub(super) fn push_admin(&mut self, admin: AdminBase<'static>) {
        self.admin_output
            .push_back(Box::new(M::from_admin(HeaderBase::default(), admin)));
    }

    /// Fill header fields on an outgoing message. Preserves pre-set fields.
    /// Allocates and consumes a sequence number from the storage if msg_seq_num
    /// is zero (not pre-set). The allocation is provisional until the commit
    /// that follows: a commit that fails to serialize releases the number
    /// again (see [`Self::commit_send`]).
    ///
    /// Returns `Ok(false)` at the outgoing limit, including a producer-supplied
    /// `SeqNum::MAX`. The caller must drop the message without committing it.
    ///
    /// An `Ok(true)` return does not mean the session continues. Stamping
    /// `SeqNum::MAX - 1` succeeds and exhausts the numbering, so the caller
    /// sends that message and then finds `should_disconnect` set.
    ///
    /// A storage failure is fatal; the caller must not commit the message.
    pub(crate) fn fill_header<S: MessagesStorage>(
        &mut self,
        msg: &mut M,
        storage: &mut S,
    ) -> Result<bool, FatalError> {
        self.ensure_healthy()?;

        // SenderCompID
        if msg.sender_comp_id().is_empty() {
            msg.set_sender_comp_id(self.session_id.sender_comp_id().to_owned());
        }
        // TargetCompID
        if msg.target_comp_id().is_empty() {
            msg.set_target_comp_id(self.session_id.target_comp_id().to_owned());
        }
        // MAX is reserved for the exhausted counter, never a fresh message.
        // Gate pre-numbered messages too, before they can overwrite storage.
        let seq = storage.next_sender_msg_seq_num();
        let Some(next) = seq
            .checked_add(1)
            .filter(|_| msg.msg_seq_num() < SeqNum::MAX)
        else {
            self.begin_disconnect(DisconnectReason::SeqNumExhausted);
            return Ok(false);
        };
        let mut fill = HeaderFill::default();
        // Allocate from storage if not pre-set.
        if msg.msg_seq_num() == 0 {
            msg.set_msg_seq_num(seq.get());
            fill.seq_num = Some(seq);
            storage.set_next_sender_msg_seq_num(next).map_err(|error| {
                error!(%seq, %next, "Failed to reserve outgoing sequence number");
                self.fail_storage("set_next_sender_msg_seq_num", &error)
            })?;
            if next.get() == SeqNum::MAX {
                error!("Outgoing sequence numbers exhausted; a sequence number reset is required");
                self.begin_disconnect(DisconnectReason::SeqNumExhausted);
            }
        }
        // SendingTime - set to now if not pre-set (MIN_UTC means not set)
        if msg.sending_time() == UtcTimestamp::MIN_UTC {
            msg.set_sending_time(UtcTimestamp::now(self.session_settings.time_precision));
            fill.sending_time = true;
        }
        self.state.last_fill = fill;
        Ok(true)
    }

    /// Release the number allocated by [`Self::fill_header`] and clear its
    /// stamps after serialization fails. Preserve producer-supplied fields.
    /// A failed counter rollback is fatal and leaves the stamps uncleared.
    fn undo_header_fill<S: MessagesStorage>(
        &mut self,
        msg: &mut M,
        storage: &mut S,
    ) -> Result<(), FatalError> {
        self.ensure_healthy()?;

        let fill = mem::take(&mut self.state.last_fill);
        if let Some(seq) = fill.seq_num
            && msg.msg_seq_num() == seq.get()
        {
            // Restore the pre-allocation counter, including MAX - 1. Any
            // disconnect already latched stays: first writer wins.
            storage.set_next_sender_msg_seq_num(seq).map_err(|error| {
                error!(%seq, "Failed to release outgoing sequence number");
                self.fail_storage("rollback_sender_msg_seq_num", &error)
            })?;
            msg.set_msg_seq_num(0);
        }
        if fill.sending_time {
            msg.set_sending_time(UtcTimestamp::MIN_UTC);
        }
        Ok(())
    }

    /// Return the message's nonzero sequence number.
    /// Requires a preceding `Ok(true)` from [`Self::fill_header`].
    fn stamped_seq_num(msg: &M) -> NonZeroSeqNum {
        NonZeroSeqNum::new(msg.msg_seq_num()).expect("fill_header stamped a sequence number")
    }

    /// Serialize an outgoing message after its output callback and queue
    /// its bytes for TCP. History is stored only when configured.
    ///
    /// Requires a preceding `Ok(true)` from [`Self::fill_header`]. Flush pending
    /// output before another commit when history is disabled.
    /// Serialization failure rolls back the allocated number and returns the
    /// message with the engine's stamps cleared. Backend or rollback failure
    /// is fatal; no output is queued on either error path.
    pub(crate) fn commit_send<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        storage: &mut S,
    ) -> Result<(), SendFailure<M>> {
        self.ensure_healthy()?;
        let seq_num = Self::stamped_seq_num(&msg);
        let pending = if self.session_settings.persist_messages {
            match storage.store(seq_num, |buf| msg.serialize(buf)) {
                Ok(()) => PendingOutput::Stored(seq_num),
                Err(error) => return Err(self.send_failure(msg, storage, error)),
            }
        } else {
            match self.scratch.write(|buf| msg.serialize(buf)) {
                Ok(len) => PendingOutput::Transient { len },
                Err(error) => {
                    return Err(self.send_failure(msg, storage, StoreError::Serialize(error)));
                }
            }
        };
        self.state.last_fill = HeaderFill::default();
        self.output.push_back(pending);
        Ok(())
    }

    /// Serialize and store a message without pushing to the output queue.
    /// Requires enabled history and a preceding `Ok(true)` from
    /// [`Self::fill_header`]. Failure handling matches [`Self::commit_send`].
    pub(crate) fn store_for_resend<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        storage: &mut S,
    ) -> Result<(), SendFailure<M>> {
        self.ensure_healthy()?;
        let seq_num = Self::stamped_seq_num(&msg);
        match storage.store(seq_num, |buf| msg.serialize(buf)) {
            Ok(()) => {
                self.state.last_fill = HeaderFill::default();
                Ok(())
            }
            Err(error) => Err(self.send_failure(msg, storage, error)),
        }
    }

    fn send_failure<S: MessagesStorage>(
        &mut self,
        mut msg: Box<M>,
        storage: &mut S,
        error: StoreError<S::Error>,
    ) -> SendFailure<M> {
        let seq_num = msg.msg_seq_num();
        match error {
            StoreError::Backend(error) => {
                error!(seq_num, "Failed to store outgoing message");
                SendFailure::Fatal(self.fail_storage("store", &error))
            }
            StoreError::Serialize(error) => {
                if let Err(fatal) = self.undo_header_fill(&mut msg, storage) {
                    error!(seq_num, %error, "Serialization failed before sequence rollback failed");
                    return SendFailure::Fatal(fatal);
                }
                SendFailure::Serialize(SerializeFailure { msg, error })
            }
        }
    }
}
