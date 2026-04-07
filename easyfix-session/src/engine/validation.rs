//! Header verification and the session's response to validation failures.

use chrono::Utc;
use easyfix_core::{
    base_messages::{HeaderBase, MsgTypeBase, SessionRejectReasonBase, SessionStatusBase},
    basic_types::{
        FixStr, FixString, MsgTypeField, SeqNum, SessionRejectReasonField, TagNum, UtcTimestamp,
    },
    fix_str,
    message::SessionMessage,
};
use tracing::{error, warn};

use super::{FatalError, HandlerResult, LogonState, SessionEngine};
use crate::{application::DisconnectReason, messages_storage::MessagesStorage};

const TAG_SENDER_COMP_ID: TagNum = 49;
const TAG_SENDING_TIME: TagNum = 52;
const TAG_TARGET_COMP_ID: TagNum = 56;
const TAG_ORIG_SENDING_TIME: TagNum = 122;

/// Failure of header verification.
///
/// Pure observation - no message ownership, no state mutation. Each
/// variant is a specific check that did not pass; the downstream policy
/// (silent ignore, recovery, reject, disconnect) is the caller's
/// concern via [`SessionEngine::validate_impl`].
#[derive(Debug)]
pub(super) enum VerifyError {
    /// Sequence number too high - caller enqueues the message and sends
    /// ResendRequest.
    TooHigh,
    /// Duplicate (PossDupFlag=Y, seq < expected). Silently ignored
    /// downstream - but still a failed seq-num check at this layer.
    Duplicate,
    /// Reject the message (CompID mismatch, SendingTime invalid, etc.).
    Reject {
        reason: SessionRejectReasonField,
        tag: Option<TagNum>,
        text: FixString,
        disconnect: Option<DisconnectReason>,
    },
    /// Sequence number too low without PossDupFlag - caller sends Logout
    /// carrying `text` and disconnects.
    TooLow { text: FixString },
    /// The persisted incoming counter has reached the implementation limit.
    SeqNumExhausted,
    /// Message not allowed in current logon state - caller disconnects.
    InvalidLogonState,
    /// Traffic other than the acknowledgement or refusal during a reset.
    UnexpectedMessageDuringReset { msg_type: MsgTypeField },
}

impl<M: SessionMessage> SessionEngine<M> {
    /// Verify an incoming message's header fields.
    ///
    /// Checks (in order): logon state, SendingTime accuracy, CompID,
    /// sequence number too-high/too-low. Pure observation: returns
    /// `Result<(), VerifyError>`; queueing the message and sending
    /// ResendRequest/Reject/Logout are the caller's responsibility
    /// (typically via [`SessionEngine::validate_impl`] or one of its
    /// role-named wrappers).
    ///
    /// `reset_pending` is set by [`Self::on_logon`] when the incoming
    /// Logon carries `ResetSeqNumFlag=Y` - it bypasses the seq-num
    /// checks (the peer is legitimately renumbering) and tells
    /// [`Self::check_logon_state`] to permit a re-Logon while
    /// established. All other callers pass `false`.
    pub(super) fn verify_header(
        &self,
        header: &HeaderBase<'_>,
        msg_type: MsgTypeField,
        storage: &impl MessagesStorage,
        check_too_high: bool,
        check_too_low: bool,
        reset_pending: bool,
    ) -> Result<(), VerifyError> {
        let sending_time = header.sending_time;
        let msg_seq_num = header.msg_seq_num;

        // 1. Logon state check
        self.check_logon_state(msg_type, reset_pending)?;

        // 2. SendingTime accuracy
        self.check_sending_time(sending_time)?;

        // 3. CompID validation
        self.check_comp_id(&header.sender_comp_id, &header.target_comp_id)?;

        let next_target = storage.next_target_msg_seq_num().get();

        // An authorized reset Logon may start new numbering. Ordinary input,
        // including SequenceReset, cannot resume an exhausted direction.
        if !reset_pending && next_target == SeqNum::MAX {
            return Err(VerifyError::SeqNumExhausted);
        }

        // 4. Sequence number too high
        if check_too_high && !reset_pending && msg_seq_num > next_target {
            warn!("Target MsgSeqNum too high, expected {next_target}, got {msg_seq_num}");
            return Err(VerifyError::TooHigh);
        }

        // 5. PossDupFlag / OrigSendingTime
        Self::check_poss_dup(header, msg_type)?;

        // 6. Sequence number too low
        if check_too_low && !reset_pending {
            Self::check_seq_num_too_low(header, next_target)?;
        }

        Ok(())
    }

    /// Validate the `OrigSendingTime(122)` that must accompany
    /// `PossDupFlag(43)=Y`: present (Test Cases §4.5.1 Scenario 2(g),
    /// `SessionRejectReason(373)=1`) and not after `SendingTime(52)`
    /// (Scenario 2(f), `373=10`). `SequenceReset<4>` is exempt - a gap fill
    /// carries `43=Y` without an original send to name.
    //
    // Ordered between the too-high and too-low checks in `verify_header`,
    // and that placement is the point of the function existing:
    //
    // - After too-high, because a PossDup message that opens a gap has to
    //   draw a ResendRequest. Rejecting it would leave the gap unrecovered,
    //   and the Reject would not even advance NextNumIn - the message is not
    //   in sequence. It is revalidated in sequence when the queue drains.
    // - Before too-low, so the duplicate path of Scenario 2(e) still decides
    //   on an OrigSendingTime that has been checked.
    //
    // Neither scenario qualifies the sequence number (2(f) says "MsgSeqNum
    // as expected", 2(g) says nothing at all), which is why this cannot stay
    // inside the too-low branch: a PossDup message in sequence never
    // reached it.
    fn check_poss_dup(header: &HeaderBase<'_>, msg_type: MsgTypeField) -> Result<(), VerifyError> {
        if !header.poss_dup_flag.unwrap_or(false) || msg_type == MsgTypeBase::SequenceReset {
            return Ok(());
        }
        let Some(orig_sending_time) = header.orig_sending_time else {
            warn!("PossDupFlag<43>=Y without OrigSendingTime<122>");
            return Err(VerifyError::Reject {
                reason: SessionRejectReasonBase::RequiredTagMissing.into(),
                tag: Some(TAG_ORIG_SENDING_TIME),
                text: fix_str!("Required tag missing: OrigSendingTime(122)").to_owned(),
                disconnect: None,
            });
        };
        if orig_sending_time.timestamp() > header.sending_time.timestamp() {
            error!("OrigSendingTime<122> after SendingTime<52>");
            return Err(VerifyError::Reject {
                reason: SessionRejectReasonBase::SendingTimeAccuracyProblem.into(),
                tag: Some(TAG_ORIG_SENDING_TIME),
                text: fix_str!("OrigSendingTime(122) after SendingTime(52)").to_owned(),
                disconnect: Some(DisconnectReason::InvalidOrigSendingTime),
            });
        }
        Ok(())
    }

    /// Check for a sequence number below `next_target`.
    /// A message marked `PossDupFlag(43)=Y` is a legitimate
    /// retransmission and yields [`VerifyError::Duplicate`]; anything else is
    /// [`VerifyError::TooLow`]. Its `OrigSendingTime(122)` was already judged
    /// by [`Self::check_poss_dup`].
    pub(super) fn check_seq_num_too_low(
        header: &HeaderBase<'_>,
        next_target: SeqNum,
    ) -> Result<(), VerifyError> {
        let msg_seq_num = header.msg_seq_num;
        if msg_seq_num >= next_target {
            return Ok(());
        }
        if header.poss_dup_flag.unwrap_or(false) {
            warn!("Target too low (duplicate)");
            return Err(VerifyError::Duplicate);
        }
        error!(next_target, "Target MsgSeqNum too low, got {msg_seq_num}");
        let text = format!("MsgSeqNum too low, expected {next_target}, got {msg_seq_num}");
        Err(VerifyError::TooLow {
            text: FixString::from_ascii_lossy(text.into_bytes()),
        })
    }

    /// Check if the message type is allowed in the current logon state.
    ///
    /// `reset_pending` is forwarded from [`Self::verify_header`] - true
    /// only when [`Self::on_logon`] is processing a Logon with
    /// `ResetSeqNumFlag=Y`. It permits a re-Logon over an already
    /// established session.
    fn check_logon_state(
        &self,
        msg_type: MsgTypeField,
        reset_pending: bool,
    ) -> Result<(), VerifyError> {
        if self.state.local_reset_unconfirmed
            && matches!(
                self.state.logon_state,
                LogonState::ResetSent | LogonState::LogoutSent { .. }
            )
            && msg_type != MsgTypeBase::Logon
            && msg_type != MsgTypeBase::Logout
        {
            return Err(VerifyError::UnexpectedMessageDuringReset { msg_type });
        }
        // No message type is exempt from the pre-logon gate below. In
        // particular SequenceReset<4> is not: its "process without regard
        // to MsgSeqNum" exemption (FIX Session Layer §4.8.8, Transport
        // §4.5) suspends the *sequence* check, never the first-message
        // rule. Letting it through in Idle would let an unauthenticated
        // peer drive `set_next_target_msg_seq_num` on a persisted store.
        let allowed = match self.state.logon_state {
            // Pre-logon: only the initial Logon is permitted. FIX Session
            // Layer §4.3.1: "If the acceptor receives anything other than
            // a valid Logon(35=A) request message, an error should be
            // logged and the transport layer connection terminated
            // without Logout(35=5) processing." (Test Cases §4.4.2
            // Scenario 2S.) `InvalidLogonState` is that silent
            // disconnect.
            LogonState::Idle => msg_type == MsgTypeBase::Logon,
            // Initiator awaiting response: peer's Logon response, plus
            // an early Logout from the peer. App / heartbeat traffic is
            // not allowed before the Established transition - Test Cases
            // §4.3.1 Scenario 1B(e) puts every non-Logon on the
            // disconnect path (Reject and Logout are optional there, so
            // disconnecting silently conforms).
            LogonState::LogonSent => {
                msg_type == MsgTypeBase::Logon || msg_type == MsgTypeBase::Logout
            }
            // Established: anything except a spurious second Logon
            // without ResetSeqNumFlag=Y.
            LogonState::Established | LogonState::ResetPending | LogonState::ResetProbe => {
                msg_type != MsgTypeBase::Logon || reset_pending
            }
            LogonState::ResetSent => {
                if self.state.local_reset_unconfirmed {
                    msg_type == MsgTypeBase::Logon || msg_type == MsgTypeBase::Logout
                } else {
                    msg_type != MsgTypeBase::Logon
                }
            }
            // Logout in flight: keep accepting the messages already on the
            // wire - heartbeats, app messages, the peer's Logout response,
            // and the ResendRequest plus retransmissions FIX Session Layer
            // §4.6.3 expects before that response. A Logon is not in-flight
            // traffic: FIX Transport §8.7 leaves Logout Pending only by
            // disconnecting or by the peer's Logout acknowledgement, never
            // back into an active session. Only an outstanding reset ACK is
            // admitted, and processing it must preserve the Logout deadline.
            LogonState::LogoutSent { .. } => {
                msg_type != MsgTypeBase::Logon || self.state.local_reset_unconfirmed
            }
            // The exchange is complete once our acknowledgement is out
            // (FIX Session Layer §4.6); the IO loop reads nothing more, so
            // this arm only keeps the match total.
            LogonState::LogoutAcknowledged { .. } => false,
        };
        if allowed {
            Ok(())
        } else {
            warn!(
                state = ?self.state.logon_state,
                reset_pending,
                "Not allowed: Invalid session state",
            );
            Err(VerifyError::InvalidLogonState)
        }
    }

    /// [`Self::check_logon_state`] for a message that failed to decode, where
    /// the MsgType may not have been recovered at all.
    ///
    /// An unresolvable MsgType cannot be the `Logon<A>` the pre-logon states
    /// require, so it fails the gate there and passes once established.
    pub(super) fn check_failed_decode_logon_state(
        &self,
        msg_type: Option<MsgTypeField>,
    ) -> Result<(), VerifyError> {
        match msg_type {
            Some(msg_type) => self.check_logon_state(msg_type, false),
            None if matches!(
                self.state.logon_state,
                LogonState::Idle | LogonState::LogonSent | LogonState::LogoutAcknowledged { .. }
            ) =>
            {
                Err(VerifyError::InvalidLogonState)
            }
            None => Ok(()),
        }
    }

    /// Check SendingTime accuracy against max_latency.
    fn check_sending_time(&self, sending_time: UtcTimestamp) -> Result<(), VerifyError> {
        let Some(max_latency) = self.session_settings.max_latency else {
            return Ok(());
        };
        // If max_latency is too large for chrono::Duration, treat it as
        // "no limit" - skip the check rather than panicking.
        let Ok(max_latency) = chrono::Duration::from_std(max_latency) else {
            return Ok(());
        };

        let now = Utc::now();
        let sending_timestamp = sending_time.timestamp();
        let abs_time_diff = (now - sending_timestamp).abs();
        if abs_time_diff > max_latency {
            warn!(
                ?abs_time_diff,
                ?max_latency,
                "SendingTime<52> verification failed"
            );
            Err(VerifyError::Reject {
                reason: SessionRejectReasonBase::SendingTimeAccuracyProblem.into(),
                tag: Some(TAG_SENDING_TIME),
                text: fix_str!("SendingTime accuracy problem").to_owned(),
                // Spec mandates Reject(373=10) "followed by a Logout(35=5)" and
                // disconnect (FIX Session Layer §4.2.3; Scenario 2(o)) - matching
                // the CompID and OrigSendingTime reject paths, not a bare Reject
                // that leaves the session established.
                disconnect: Some(DisconnectReason::SendingTimeAccuracyProblem),
            })
        } else {
            Ok(())
        }
    }

    /// Validate SenderCompID and TargetCompID against the session identity.
    fn check_comp_id(
        &self,
        sender_comp_id: &FixStr,
        target_comp_id: &FixStr,
    ) -> Result<(), VerifyError> {
        if !self.session_settings.check_comp_id {
            return Ok(());
        }
        if self.session_id.sender_comp_id() != target_comp_id {
            Err(VerifyError::Reject {
                reason: SessionRejectReasonBase::CompIdProblem.into(),
                tag: Some(TAG_TARGET_COMP_ID),
                text: fix_str!("TargetCompID does not match").to_owned(),
                disconnect: Some(DisconnectReason::InvalidCompId),
            })
        } else if self.session_id.target_comp_id() != sender_comp_id {
            Err(VerifyError::Reject {
                reason: SessionRejectReasonBase::CompIdProblem.into(),
                tag: Some(TAG_SENDER_COMP_ID),
                text: fix_str!("SenderCompID does not match").to_owned(),
                disconnect: Some(DisconnectReason::InvalidCompId),
            })
        } else {
            Ok(())
        }
    }

    /// Run [`Self::verify_header`] and react to any failure (push
    /// Reject/Logout, advance seq num where appropriate).
    ///
    /// Returns:
    /// * `None` - header passed; the caller continues with handler-specific
    ///   logic.
    /// * `Some(hr)` - caller short-circuits and returns `hr`. `Handled`
    ///   for silently-ignored failures (Duplicate, Reject without
    ///   disconnect); `Enqueue`/`Disconnect(_)` for failures that the
    ///   dispatcher must surface.
    ///
    /// Most callers use the role-named wrappers ([`Self::validate`],
    /// [`Self::validate_logon`], [`Self::validate_sequence_reset`],
    /// [`Self::validate_resend_request`], [`Self::validate_reject`])
    /// rather than calling this directly.
    pub(super) fn validate_impl<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        msg_type: MsgTypeField,
        storage: &mut S,
        check_too_high: bool,
        check_too_low: bool,
        reset_pending: bool,
    ) -> Result<Option<HandlerResult>, FatalError> {
        self.ensure_healthy()?;

        let Some(error) = self
            .verify_header(
                header,
                msg_type,
                storage,
                check_too_high,
                check_too_low,
                reset_pending,
            )
            .err()
        else {
            return Ok(None);
        };

        let msg_seq_num = header.msg_seq_num;

        Ok(Some(match error {
            VerifyError::TooHigh => HandlerResult::Enqueue,

            VerifyError::Duplicate => HandlerResult::Handled,

            VerifyError::SeqNumExhausted => {
                self.end_session_if_target_numbering_exhausted(storage);
                HandlerResult::Handled
            }

            VerifyError::Reject {
                reason,
                tag,
                text,
                disconnect,
            } => {
                // Rejecting number 1 cannot leave a locally initiated reset
                // waiting for another ACK numbered 1. Preserve specific
                // header causes when present; otherwise make it terminal.
                let disconnect = disconnect.or_else(|| {
                    self.state
                        .local_reset_unconfirmed
                        .then_some(DisconnectReason::InvalidLogonState)
                });
                self.consume_seq_num(msg_type, msg_seq_num, storage)?;

                self.send_reject(
                    Some(msg_type.as_fix_str().to_owned()),
                    msg_seq_num,
                    reason,
                    tag,
                    Some(text.clone()),
                );

                if let Some(disconnect_reason) = disconnect {
                    // The Logout carries the same Text(58) as the Reject:
                    // every Test Cases step that mandates it names what it
                    // must reference - the error condition for an invalid
                    // Logon (§4.4.1 Scenario 1S(d) step 3), the offending
                    // value for a CompID or SendingTime problem (§4.5.1
                    // Scenario 2(k) step 3, 2(o) step 3).
                    self.push_logout(None, Some(text));
                    HandlerResult::Disconnect(disconnect_reason)
                } else {
                    HandlerResult::Handled
                }
            }

            VerifyError::TooLow { text } => {
                self.push_logout(
                    Some(SessionStatusBase::ReceivedMsgSeqNumTooLow.into()),
                    Some(text),
                );
                HandlerResult::Disconnect(DisconnectReason::MsgSeqNumTooLow)
            }

            VerifyError::InvalidLogonState => {
                HandlerResult::Disconnect(DisconnectReason::InvalidLogonState)
            }
            VerifyError::UnexpectedMessageDuringReset { msg_type } => {
                self.push_unexpected_reset_logout(msg_type);
                HandlerResult::Disconnect(DisconnectReason::InvalidLogonState)
            }
        }))
    }

    /// [`Self::validate_impl`] for the common case: check both seq-num
    /// directions, no reset pending. Used by handlers and the dispatch
    /// app-message branch - anywhere validation should auto-trigger gap
    /// recovery on too-high and auto-handle Duplicate/TooLow.
    pub(super) fn validate<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        msg_type: MsgTypeField,
        storage: &mut S,
    ) -> Result<Option<HandlerResult>, FatalError> {
        self.ensure_healthy()?;

        self.validate_impl(header, msg_type, storage, true, true, false)
    }

    /// [`Self::validate_impl`] specialized for [`Self::on_logon`]. Skips
    /// the too-high check (Logon's seq-num is decided after `is_normal`
    /// branching in the handler) and forwards `reset_pending` so the
    /// logon-state check permits a re-Logon when ResetSeqNumFlag=Y.
    ///
    /// An unconfirmed local reset skips too-low validation: its ACK
    /// number and reset flag are checked together by [`Self::on_logon`].
    pub(super) fn validate_logon<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        storage: &mut S,
        reset_pending: bool,
    ) -> Result<Option<HandlerResult>, FatalError> {
        self.ensure_healthy()?;

        self.validate_impl(
            header,
            MsgTypeField::from(MsgTypeBase::Logon),
            storage,
            false,
            !self.state.local_reset_unconfirmed,
            reset_pending,
        )
    }

    /// [`Self::validate_impl`] specialized for
    /// [`Self::on_sequence_reset`]. Both seq-num checks are gated by
    /// `gap_fill_flag`: a SequenceReset without GapFill is a reset
    /// directive whose own seq-num is irrelevant.
    pub(super) fn validate_sequence_reset<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        storage: &mut S,
        gap_fill_flag: bool,
    ) -> Result<Option<HandlerResult>, FatalError> {
        self.ensure_healthy()?;

        self.validate_impl(
            header,
            MsgTypeField::from(MsgTypeBase::SequenceReset),
            storage,
            gap_fill_flag,
            gap_fill_flag,
            false,
        )
    }

    /// [`Self::validate_impl`] specialized for
    /// [`Self::on_resend_request`]. Skips the too-high check; a
    /// ResendRequest with too-high seq must not auto-trigger another
    /// ResendRequest (would cascade). The handler does its own too-high
    /// enqueue after queueing the peer's resend range.
    pub(super) fn validate_resend_request<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        storage: &mut S,
    ) -> Result<Option<HandlerResult>, FatalError> {
        self.ensure_healthy()?;

        self.validate_impl(
            header,
            MsgTypeField::from(MsgTypeBase::ResendRequest),
            storage,
            false,
            true,
            false,
        )
    }

    /// [`Self::validate_impl`] specialized for [`Self::on_reject`].
    /// Skips the too-high check; a Reject during gap recovery must not
    /// trigger another resend cycle.
    pub(super) fn validate_reject<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        storage: &mut S,
    ) -> Result<Option<HandlerResult>, FatalError> {
        self.ensure_healthy()?;

        self.validate_impl(
            header,
            MsgTypeField::from(MsgTypeBase::Reject),
            storage,
            false,
            true,
            false,
        )
    }
}
