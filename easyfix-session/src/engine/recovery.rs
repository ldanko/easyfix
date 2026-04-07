//! Incoming gap recovery and outgoing retransmission.

use std::{borrow::Cow, mem, ops::RangeInclusive};

use easyfix_core::{
    base_messages::{
        AdminBase, HeaderBase, MsgTypeBase, ResendRequestBase, SequenceResetBase,
        SessionRejectReasonBase,
    },
    basic_types::{
        FixString, Int, MsgTypeField, NonZeroSeqNum, SeqNum, TagNum, TimePrecision, UtcTimestamp,
    },
    message::{MsgCat, SessionMessage},
};
use tracing::{debug, error, info, warn};

use super::{FatalError, HandlerResult, InputResult, PendingOutput, SessionEngine};
use crate::{messages_storage::MessagesStorage, session_id::SessionId};

const TAG_BEGIN_SEQ_NO: TagNum = 7;
const TAG_NEW_SEQ_NO: TagNum = 36;

/// Build the `SequenceReset`-GapFill message serialized by both the
/// runtime scratch path and the registration-time size probe
/// ([`validate_gap_fill_fits`]).
fn build_gap_fill<M: SessionMessage>(
    session_id: &SessionId,
    msg_seq_num: SeqNum,
    new_seq_no: SeqNum,
    poss_dup_flag: Option<bool>,
    time_precision: TimePrecision,
) -> Box<M> {
    // TODO: clones SenderCompID/TargetCompID because
    // `SessionMessage::from_admin` requires `HeaderBase<'static>`. Revisit
    // together with the borrowed-message concept - when the trait is
    // parameterized with `'msg`, this can become `Cow::Borrowed` from
    // `session_id` and the clones disappear.
    let header = HeaderBase {
        sender_comp_id: Cow::Owned(session_id.sender_comp_id().to_owned()),
        target_comp_id: Cow::Owned(session_id.target_comp_id().to_owned()),
        msg_seq_num,
        sending_time: UtcTimestamp::now(time_precision),
        poss_dup_flag,
        orig_sending_time: None,
        // Explicit ApplVerID is prohibited on session-level messages
        // (FIX Transport §4.1.3) - a gap-fill SequenceReset never
        // carries tag 1128.
        appl_ver_id: None,
    };
    Box::new(M::from_admin(
        header,
        AdminBase::SequenceReset(SequenceResetBase {
            gap_fill_flag: Some(true),
            new_seq_no,
        }),
    ))
}

/// Validate at registration time that `max_message_size` can hold the
/// largest possible `SequenceReset`-GapFill for this session identity.
///
/// The gap-fill's only variable-length parts are the seq num digit counts
/// (maximal at `SeqNum::MAX - 1` and `SeqNum::MAX`), the optional
/// `PossDupFlag` (included), and
/// `SendingTime`, whose width is fixed by `time_precision`. A successful
/// probe therefore proves every gap-fill the engine can emit fits the
/// scratch buffer, which is what lets `write_gap_fill_to_scratch` treat
/// serialization failure as unreachable. Keep it a real serialization
/// rather than a size calculation - that is what makes the other failure
/// modes unreachable too.
///
/// That proof holds only while `time_precision` is the value the session
/// will actually run with - pass it from the same `SessionSettings`. The
/// error is one-sided: a probe wider than the runtime merely overestimates,
/// but a narrower one (`Millis` probing for a `Nanos` session: six bytes)
/// under-measures and the unreachability claim goes with it.
///
/// On failure returns the approximate number of bytes required (measured
/// with a wider BodyLength placeholder, so it may overcount by a few
/// bytes).
pub(crate) fn validate_gap_fill_fits<M: SessionMessage>(
    session_id: &SessionId,
    max_message_size: usize,
    time_precision: TimePrecision,
) -> Result<(), usize> {
    let gap_fill = build_gap_fill::<M>(
        session_id,
        SeqNum::MAX - 1,
        SeqNum::MAX,
        Some(true),
        time_precision,
    );
    let mut buf = vec![0u8; max_message_size];
    if gap_fill.serialize(&mut buf).is_ok() {
        return Ok(());
    }
    // Measure in a roomier buffer only to report how much is needed.
    let mut buf = vec![0u8; max_message_size + 4096];
    let required = gap_fill.serialize(&mut buf).unwrap_or(buf.len());
    Err(required)
}

impl<M: SessionMessage> SessionEngine<M> {
    pub(super) fn inbound_recovery_finished<S: MessagesStorage>(&self, storage: &S) -> bool {
        self.state
            .resend_range
            .as_ref()
            .is_none_or(|range| storage.next_target_msg_seq_num().get() > *range.end())
    }

    pub(crate) fn has_pending_resends(&self) -> bool {
        !self.pending_resends.is_empty()
    }

    /// Whether `seq_num` is parked in the out-of-order queue.
    ///
    /// Passed the next expected target, this answers whether
    /// [`Self::next_queued_message`] would return `Some` - which is how the IO
    /// loop decides the queue can still make progress and keeps iterating
    /// rather than blocking on the event select mid-drain. Note it is a
    /// question about one sequence number, not "is the queue non-empty":
    /// messages parked beyond a still-open gap are not processable, and
    /// treating them as work would spin the loop.
    pub(crate) fn has_queued_message(&self, seq_num: SeqNum) -> bool {
        self.state.queue.contains_key(&seq_num)
    }

    pub(crate) fn take_pending_resend(&mut self) -> Option<RangeInclusive<SeqNum>> {
        self.pending_resends.pop_front()
    }

    /// Push a ResendRequest<2> to admin_output.
    pub(super) fn send_resend_request(&mut self, begin_seq_no: SeqNum, end_seq_no: SeqNum) {
        self.push_admin(AdminBase::ResendRequest(ResendRequestBase {
            begin_seq_no,
            end_seq_no,
        }));
    }

    /// Lowest sequence number in the out-of-order queue.
    fn lowest_queued_seq_num(&self) -> Option<SeqNum> {
        self.state.queue.first_key_value().map(|(&k, _)| k)
    }

    /// Drop parked out-of-order messages the incoming counter has jumped
    /// over. Call after every jump of `NextNumIn` that is not a plain
    /// increment.
    fn discard_queued_below(&mut self, next_target: SeqNum) {
        // The queue is drained only at exactly `NextNumIn`, and the counter
        // never goes down, so an entry below it can never be dispatched
        // again - it would only pin memory and defeat the clamp in
        // `request_resend`, which reads the lowest key in the queue.
        //
        // Reaching this with a non-empty cut means the peer's SequenceReset
        // skipped over numbers it had already transmitted, and those
        // messages are lost to the application. The spec still mandates
        // honouring the jump (Session Test Cases Scenario 10b / 11a), so
        // reporting it is the only lever left.
        let kept = self.state.queue.split_off(&next_target);
        let discarded = mem::replace(&mut self.state.queue, kept);
        if let (Some((&first, _)), Some((&last, _))) =
            (discarded.first_key_value(), discarded.last_key_value())
        {
            error!(
                count = discarded.len(),
                first,
                last,
                next_target,
                "Discarding parked out-of-order messages skipped over by the \
                 peer's SequenceReset; they were received but will never be \
                 processed"
            );
        }
    }

    /// Drop every piece of in-flight recovery state. Call wherever the
    /// sequence counters are renumbered on a live connection.
    pub(super) fn discard_recovery_state(&mut self) {
        // A reset starts "a new set of sequence numbers" on both sides
        // (Session Layer 4.4.2), so anything naming the old numbering is
        // meaningless afterwards: the ranges the peer asked us to resend
        // (the store they came from is cleared together with the counters -
        // draining them would put gap-fills stamped with discarded numbers
        // on the wire), the gap-fill accumulating over such a range, the
        // range we asked the peer for (kept, it would only suppress requests
        // for new-numbering gaps that happen to fall inside it), and the
        // messages parked behind a gap the reset closed by fiat.
        //
        // The IO loop's own `active_resend` needs no clearing here. At
        // connection start the loop has not begun. Live resets run during
        // input dispatch: an accepted peer Logon or a fresh probe Heartbeat.
        // The loop holds both fresh input and the queued drain until active
        // and pending replays finish; queued work makes a probe stale and
        // cannot start our reset. Keep those gates:
        // dispatching input during a replay could renumber the session while
        // its remaining output still names the old numbering.
        self.pending_resends.clear();
        self.gap_fill_range = None;
        self.state.resend_range = None;

        // Parked messages were received and are now lost to the application.
        // The peer was supposed to close its gaps before resetting (4.4.2
        // suggests a TestRequest/Heartbeat round trip for exactly that), so
        // a non-empty cut is the peer's doing and worth reporting, as in
        // `discard_queued_below`.
        let discarded = mem::take(&mut self.state.queue);
        if let (Some((&first, _)), Some((&last, _))) =
            (discarded.first_key_value(), discarded.last_key_value())
        {
            error!(
                count = discarded.len(),
                first,
                last,
                "Discarding parked out-of-order messages on sequence reset; they \
                 were received but will never be processed"
            );
        }
    }

    /// Build and push a ResendRequest for a detected sequence gap.
    /// Updates `resend_range` on state. Suppresses if redundant.
    ///
    /// The requested range is clamped to the lowest parked message -
    /// everything from there on already sits in the out-of-order queue
    /// (park-and-request, Session Layer 4.8.2 second strategy).
    pub(super) fn request_resend(
        &mut self,
        too_high_msg_seq_num: SeqNum,
        storage: &impl MessagesStorage,
    ) {
        let mut end_seq_no = too_high_msg_seq_num.saturating_sub(1);
        if let Some(queued_lowest) = self.lowest_queued_seq_num()
            && queued_lowest > storage.next_target_msg_seq_num().get()
        {
            let new_end = queued_lowest.saturating_sub(1);
            if new_end < end_seq_no {
                end_seq_no = new_end;
            }
        }
        self.request_resend_impl(too_high_msg_seq_num, end_seq_no, storage)
    }

    /// [`Self::request_resend`] for an unparseable too-high message: it
    /// cannot be parked in the out-of-order queue, so the requested range
    /// extends THROUGH its seq num (drop-and-request, FIX Session Layer
    /// 4.8.2 Figure 12) and the peer redelivers it in sequence. No clamp
    /// to parked messages - a redundantly redelivered parked seq num is
    /// discarded as a PossDup duplicate.
    pub(super) fn request_resend_through(
        &mut self,
        failed_seq_num: SeqNum,
        storage: &impl MessagesStorage,
    ) {
        self.request_resend_impl(failed_seq_num, failed_seq_num, storage)
    }

    fn request_resend_impl(
        &mut self,
        too_high_msg_seq_num: SeqNum,
        end_seq_no: SeqNum,
        storage: &impl MessagesStorage,
    ) {
        let begin_seq_no = storage.next_target_msg_seq_num().get();

        if begin_seq_no > end_seq_no {
            return;
        }

        // A recorded range whose end NextNumIn has already passed is fully
        // recovered - drop it so it cannot suppress requests for later
        // gaps (a stale range would otherwise suppress forever, since seq
        // nums only grow).
        if let Some(range) = &self.state.resend_range
            && begin_seq_no > *range.end()
        {
            self.state.resend_range = None;
        }

        // Suppress only a request fully covered by the still-outstanding
        // one - the peer is already resending that range.
        if let Some(ref range) = self.state.resend_range
            && !self.session_settings.send_redundant_resend_requests
            && begin_seq_no >= *range.start()
            && end_seq_no <= *range.end()
        {
            warn!(
                begin = *range.start(),
                end = *range.end(),
                too_high = too_high_msg_seq_num,
                "ResendRequest already sent, suppressing"
            );
            return;
        }

        self.send_resend_request(begin_seq_no, end_seq_no);
        self.state.resend_range = Some(begin_seq_no..=end_seq_no);
    }

    pub(super) fn on_resend_request<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        _resend_request: ResendRequestBase,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        Ok(self
            .validate_resend_request(header, storage)?
            .unwrap_or(HandlerResult::AdminMsg))
    }

    pub(super) fn process_resend_request<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        resend_request: ResendRequestBase,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let msg_seq_num = header.msg_seq_num;
        let begin_seq_no = resend_request.begin_seq_no;
        let end_seq_no = resend_request.end_seq_no;

        // BeginSeqNo(7) names a MsgSeqNum, which starts at 1 (Session Layer
        // §4.1); EndSeqNo(16)=0 is the only sanctioned zero, meaning infinity
        // (§4.8.2). Neither a zero begin nor an inverted range names a
        // message: Reject 373=5 (Testcases §4.5.13 Scenario 14(e)). Queueing
        // one would also put a gap-fill stamped MsgSeqNum(34)=0 on the wire.
        //
        // This fires before the too-high check below on purpose, so a
        // ResendRequest above the gap is Rejected rather than held back.
        // §4.8.8 exempts ResendRequest from ordered processing, and a queued
        // one is never re-processed (`next_queued_message` advances past it
        // on MsgType alone), so holding the Reject back would drop it for
        // good. `reject_error_with_header` withholds its Reject in the same
        // situation because there the message is redelivered and rejected in
        // sequence - that option does not exist here.
        if begin_seq_no == 0 || (end_seq_no != 0 && begin_seq_no > end_seq_no) {
            let msg_type = MsgTypeField::from(MsgTypeBase::ResendRequest);
            let reason = SessionRejectReasonBase::ValueIsIncorrect;
            let tag = Int::from(TAG_BEGIN_SEQ_NO);
            let text = format!(
                "{reason:?} (tag={tag}) - invalid resend range {begin_seq_no}..{end_seq_no}"
            );
            self.send_reject(
                Some(msg_type.as_fix_str().to_owned()),
                msg_seq_num,
                reason.into(),
                Some(TAG_BEGIN_SEQ_NO),
                Some(FixString::from_ascii_lossy(text.into_bytes())),
            );
        } else {
            // Normalize end_seq_no: 0 means "everything after begin"; an end
            // at or beyond our next sender seq num is clamped. Both become
            // next_sender-1.
            let next_sender = storage.next_sender_msg_seq_num().get();
            let adjusted_end = if end_seq_no == 0 || end_seq_no >= next_sender {
                next_sender - 1
            } else {
                end_seq_no
            };

            // A begin at or above our sender counter asks for messages we never
            // sent. The value is well-formed, and a peer that outlived our
            // storage produces it legitimately, so it is not a Reject - but
            // the resulting range is empty and the peer gets nothing back.
            // Say so, or the condition passes unrecorded on both sides.
            if begin_seq_no >= next_sender {
                warn!(
                    "ResendRequest BeginSeqNo<7> {begin_seq_no} at or above next sender \
                     seq num {next_sender} - nothing to retransmit"
                );
            }

            info!("Received ResendRequest FROM: {begin_seq_no} TO: {adjusted_end}");
            self.pending_resends.push_back(begin_seq_no..=adjusted_end);
        }

        // Manual too-high check - `validate_resend_request` skipped it during
        // the validation phase. `apply_result` handles enqueue + ResendRequest
        // for the gap.
        let next_target = storage.next_target_msg_seq_num().get();
        if msg_seq_num > next_target {
            return Ok(HandlerResult::Enqueue);
        }

        if next_target == msg_seq_num {
            self.advance_target(storage)?;
        }

        Ok(HandlerResult::Handled)
    }

    pub(super) fn on_sequence_reset<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        sequence_reset: SequenceResetBase,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let gap_fill_flag = sequence_reset.gap_fill_flag.unwrap_or(false);
        Ok(self
            .validate_sequence_reset(header, storage, gap_fill_flag)?
            .unwrap_or(HandlerResult::AdminMsg))
    }

    pub(super) fn process_sequence_reset<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        sequence_reset: SequenceResetBase,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let msg_type = MsgTypeField::from(MsgTypeBase::SequenceReset);
        let msg_seq_num = header.msg_seq_num;
        let new_seq_no = sequence_reset.new_seq_no;
        let gap_fill_flag = sequence_reset.gap_fill_flag.unwrap_or(false);
        let next_target = storage.next_target_msg_seq_num().get();

        // A GapFill (123=Y) reaches this point only in sequence
        // (MsgSeqNum == NextNumIn, enforced by `validate_sequence_reset`), so
        // `NewSeqNo <= NextNumIn` means `NewSeqNo <= MsgSeqNum` - an "attempt to
        // lower sequence number" that must be Rejected, *including* the degenerate
        // `NewSeqNo == NextNumIn` case (Scenario 10e).
        //
        // A Reset (123=N) ignores MsgSeqNum: `NewSeqNo > NextNumIn` advances,
        // `NewSeqNo == NextNumIn` is accepted with a warning (Scenario "Reset" b),
        // and only `NewSeqNo < NextNumIn` is Rejected (Scenario "Reset" c). The
        // GapFillFlag is therefore the discriminator for the `==` case - comparing
        // without it would silently accept the invalid GapFill.
        //
        // `NewSeqNo(36)` comes off the wire, so it may be zero. It needs no
        // arm of its own: zero is below any counter, so it falls into the
        // reject branch where an attempt to lower the sequence belongs.
        if let Some(raised_target) =
            NonZeroSeqNum::new(new_seq_no).filter(|n| n.get() > next_target)
        {
            info!("Set next target MsgSeqNo to {raised_target}");
            storage
                .set_next_target_msg_seq_num(raised_target)
                .map_err(|error| {
                    error!(%raised_target, "Failed to raise incoming sequence number");
                    self.fail_storage("set_next_target_msg_seq_num", &error)
                })?;
            self.discard_queued_below(raised_target.get());
        } else if new_seq_no < next_target || gap_fill_flag {
            let reason = SessionRejectReasonBase::ValueIsIncorrect;
            let tag = Int::from(TAG_NEW_SEQ_NO);
            let text = format!("{reason:?} (tag={tag}) - attempt to lower sequence number");
            self.send_reject(
                Some(msg_type.as_fix_str().to_owned()),
                msg_seq_num,
                reason.into(),
                Some(TAG_NEW_SEQ_NO),
                Some(FixString::from_ascii_lossy(text.into_bytes())),
            );
        } else {
            // Reset (123=N) with NewSeqNo == NextNumIn: accept, warning only.
            warn!("SequenceReset-Reset with NewSeqNo({new_seq_no}) == NextNumIn; accepting");
        }

        Ok(HandlerResult::Handled)
    }

    /// Should this message be gap-filled during resend? Returns true
    /// for admin messages except Reject.
    pub(crate) fn resend_as_gap_fill(&self, msg: &M) -> bool {
        msg.msg_cat() == MsgCat::Admin && msg.msg_type() != MsgTypeBase::Reject
    }

    /// Accumulate a seq num into the current gap-fill range.
    /// Called by the resend IO loop when [`Self::resend_as_gap_fill`] or the
    /// application's `should_gap_fill` elects gap-fill, or history is disabled.
    pub(crate) fn accumulate_resend_gap(&mut self, seq_num: SeqNum) {
        match &mut self.gap_fill_range {
            Some((_begin, end)) => *end = seq_num,
            None => self.gap_fill_range = Some((seq_num, seq_num)),
        }
    }

    /// Whether skipped messages await a gap-fill response.
    pub(crate) fn has_accumulated_resend_gap(&self) -> bool {
        self.gap_fill_range.is_some()
    }

    /// Serialize a `SequenceReset`-GapFill into the scratch buffer.
    fn write_gap_fill_to_scratch(
        &mut self,
        msg_seq_num: SeqNum,
        new_seq_no: SeqNum,
        poss_dup_flag: Option<bool>,
    ) -> Result<usize, FatalError> {
        let gap_fill = build_gap_fill::<M>(
            &self.session_id,
            msg_seq_num,
            new_seq_no,
            poss_dup_flag,
            self.session_settings.time_precision,
        );
        self.scratch
            .write(|buf| gap_fill.serialize(buf))
            .map_err(|error| {
                error!(
                    msg_seq_num,
                    new_seq_no, "Failed to serialize resend gap fill"
                );
                self.fail_storage("gap_fill_serialize", &error)
            })
    }

    /// Flush any accumulated gap-fill range as a SequenceReset-GapFill
    /// written to the scratch buffer and pushed as `PendingOutput::Transient`.
    /// No-op if no gap is accumulating.
    /// Flush pending scratch output before calling. Serialization failure is fatal.
    pub(crate) fn flush_resend_gap(&mut self) -> Result<(), FatalError> {
        self.ensure_healthy()?;
        let Some((begin, end)) = self.gap_fill_range.take() else {
            return Ok(());
        };

        // Ranges end at next_sender - 1, at most MAX - 1. MAX is a valid
        // NewSeqNo for skipping the final usable message, never a MsgSeqNum
        // allocated by this engine.
        let new_seq_no = end + 1;
        debug!("Resending messages from {begin} to {end} as gap fill (NewSeqNo={new_seq_no})");

        let len = self.write_gap_fill_to_scratch(begin, new_seq_no, Some(true))?;
        self.output.push_back(PendingOutput::Transient { len });
        Ok(())
    }

    /// Resend one stored message. Deserializes the stored bytes, sets
    /// PossDupFlag=Y and OrigSendingTime, re-serializes into scratch,
    /// and pushes `PendingOutput::Transient`.
    ///
    /// Flush pending scratch output before calling. Decoding or serialization
    /// failure is fatal. The stored bytes remain unchanged.
    pub(crate) fn process_resend_message(
        &mut self,
        seq_num: SeqNum,
        msg_bytes: &[u8],
    ) -> Result<(), FatalError> {
        self.ensure_healthy()?;
        let mut msg = M::from_bytes(msg_bytes).map_err(|error| {
            error!(seq_num, "Failed to decode stored message for resend");
            self.fail_storage("replay_decode", &error)
        })?;
        msg.set_orig_sending_time(Some(msg.sending_time()));
        msg.set_sending_time(UtcTimestamp::now(self.session_settings.time_precision));
        msg.set_poss_dup_flag(Some(true));
        let len = self
            .scratch
            .write(|buf| msg.serialize(buf))
            .map_err(|error| {
                error!(
                    seq_num,
                    %error,
                    "Failed to serialize stored message for resend"
                );
                self.fail_storage("replay_serialize", &error)
            })?;
        self.output.push_back(PendingOutput::Transient { len });
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn queued_count(&self) -> usize {
        self.state.queue.len()
    }

    /// Take the next queued out-of-order message after gap recovery.
    /// Returns None when the queue is empty or the next message's
    /// seq num hasn't been reached yet.
    pub(crate) fn next_queued_message<S: MessagesStorage>(
        &mut self,
        storage: &mut S,
    ) -> Result<Option<InputResult<M>>, FatalError> {
        self.ensure_healthy()?;

        let next_target = storage.next_target_msg_seq_num().get();
        let Some(msg) = self.state.queue.remove(&next_target) else {
            return Ok(None);
        };

        // Logon and ResendRequest were processed or deliberately ignored at
        // initial receipt (a replayed reset Logon) - just consume the number.
        let msg_type = msg.msg_type();
        if msg_type == MsgTypeBase::Logon || msg_type == MsgTypeBase::ResendRequest {
            self.advance_target(storage)?;
            return Ok(Some(InputResult::Handled));
        }

        // All other messages: re-dispatch through on_input.
        Ok(Some(self.on_input(msg, storage)?))
    }
}
