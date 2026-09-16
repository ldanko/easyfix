//! Inbound message validation and the session's response to failures.

use std::borrow::Cow;

use chrono::Utc;
use easyfix_core::{
    base_messages::{
        HeaderBase, MsgTypeBase, ResendRequestBase, SequenceResetBase, SessionStatusBase,
    },
    basic_types::{
        FixStr, FixString, MsgTypeField, SeqNum, SessionStatusField, TagNum, UtcTimestamp,
    },
    fix_str,
    message::SessionMessage,
};
use tracing::{error, warn};

use super::{
    FatalError, HandlerResult, LogonState, SessionEngine, logout::unexpected_reset_logout_text,
};
use crate::{
    application::{DisconnectReason, ValidationError},
    messages_storage::MessagesStorage,
};

const TAG_SENDER_COMP_ID: TagNum = 49;
const TAG_TARGET_COMP_ID: TagNum = 56;

/// A refusal waiting for its application notification.
#[derive(Debug)]
pub(crate) struct ValidationFailure {
    pub(crate) error: ValidationError,
    reaction: ValidationReaction,
}

/// A validation outcome that stops normal message delivery.
#[derive(Debug)]
pub(super) enum ValidationResult {
    /// Notify the application before executing the refusal.
    Failure(ValidationFailure),
    /// The message needs no further processing.
    Handled,
    /// Defer the message until the preceding sequence gap is filled.
    Enqueue,
}

/// How a rejected message participates in incoming sequence processing.
#[derive(Debug)]
pub(super) enum SequenceAction {
    /// Leave the incoming counter and recovery queue unchanged.
    Preserve,
    /// Consume an expected number, except on SequenceReset.
    Consume,
    /// Queue the message and request the preceding sequence gap.
    Enqueue,
}

/// Protocol work performed after the validation-error notification.
#[derive(Debug)]
enum ValidationReaction {
    Reply {
        text: Option<Cow<'static, FixStr>>,
        disconnect: Option<DisconnectReason>,
        sequence: SequenceAction,
        session_status: Option<SessionStatusField>,
    },
    Disconnect(DisconnectReason),
}

/// Failure of header verification.
///
/// Pure observation - no message ownership, no state mutation. Each
/// variant is a specific check that did not pass; the downstream policy
/// (silent ignore, recovery, reject, disconnect) is the caller's
/// concern via [`SessionEngine::validate`].
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
        error: ValidationError,
        text: Cow<'static, FixStr>,
    },
    /// Sequence number too low without PossDupFlag - caller sends Logout
    /// carrying `text` and disconnects.
    TooLow { text: Cow<'static, FixStr> },
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
    /// (via [`SessionEngine::validate`]).
    ///
    /// `reset_pending` is set by [`Self::validate_logon`] when the incoming
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
                error: ValidationError::MissingOrigSendingTime,
                text: Cow::Borrowed(fix_str!("Required tag missing: OrigSendingTime(122)")),
            });
        };
        if orig_sending_time.timestamp() > header.sending_time.timestamp() {
            error!("OrigSendingTime<122> after SendingTime<52>");
            return Err(VerifyError::Reject {
                error: ValidationError::OrigSendingTimeAfterSendingTime,
                text: Cow::Borrowed(fix_str!("OrigSendingTime(122) after SendingTime(52)")),
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
            text: Cow::Owned(FixString::from_ascii_lossy(text.into_bytes())),
        })
    }

    /// Check if the message type is allowed in the current logon state.
    ///
    /// `reset_pending` is forwarded from [`Self::verify_header`] - true
    /// only when [`Self::validate_logon`] is processing a Logon with
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
        let Ok(threshold) = chrono::Duration::from_std(max_latency) else {
            return Ok(());
        };

        let now = Utc::now();
        let sending_timestamp = sending_time.timestamp();
        let abs_time_diff = (now - sending_timestamp).abs();
        if abs_time_diff > threshold {
            warn!(
                ?abs_time_diff,
                ?max_latency,
                "SendingTime<52> verification failed"
            );
            Err(VerifyError::Reject {
                error: ValidationError::SendingTimeAccuracy { max_latency },
                text: Cow::Borrowed(fix_str!("SendingTime accuracy problem")),
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
                error: ValidationError::CompIdMismatch {
                    tag: TAG_TARGET_COMP_ID,
                    expected: self.session_id.sender_comp_id().to_owned(),
                },
                text: Cow::Borrowed(fix_str!("TargetCompID does not match")),
            })
        } else if self.session_id.target_comp_id() != sender_comp_id {
            Err(VerifyError::Reject {
                error: ValidationError::CompIdMismatch {
                    tag: TAG_SENDER_COMP_ID,
                    expected: self.session_id.target_comp_id().to_owned(),
                },
                text: Cow::Borrowed(fix_str!("SenderCompID does not match")),
            })
        } else {
            Ok(())
        }
    }

    /// Check the header and return a deferred refusal, a recovery outcome,
    /// or `None` when message-specific validation can continue.
    pub(super) fn validate<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        msg_type: MsgTypeField,
        storage: &mut S,
        check_too_high: bool,
        check_too_low: bool,
        reset_pending: bool,
    ) -> Option<ValidationResult> {
        let error = self
            .verify_header(
                header,
                msg_type,
                storage,
                check_too_high,
                check_too_low,
                reset_pending,
            )
            .err()?;

        Some(match error {
            VerifyError::TooHigh => ValidationResult::Enqueue,

            VerifyError::Duplicate => ValidationResult::Handled,

            VerifyError::SeqNumExhausted => {
                self.end_session_if_target_numbering_exhausted(storage);
                ValidationResult::Handled
            }

            VerifyError::Reject { error, text } => ValidationResult::Failure(
                self.validation_failure(error, Some(text), SequenceAction::Consume, None),
            ),

            VerifyError::TooLow { text } => ValidationResult::Failure(self.validation_failure(
                ValidationError::MsgSeqNumTooLow {
                    expected: storage.next_target_msg_seq_num().get(),
                },
                Some(text),
                SequenceAction::Preserve,
                Some(SessionStatusBase::ReceivedMsgSeqNumTooLow.into()),
            )),

            VerifyError::InvalidLogonState => ValidationResult::Failure(self.validation_failure(
                ValidationError::UnexpectedMessage,
                None,
                SequenceAction::Preserve,
                None,
            )),
            VerifyError::UnexpectedMessageDuringReset { msg_type } => {
                ValidationResult::Failure(self.validation_failure(
                    ValidationError::UnexpectedMessageDuringReset,
                    Some(Cow::Owned(unexpected_reset_logout_text(msg_type))),
                    SequenceAction::Preserve,
                    None,
                ))
            }
        })
    }

    /// Validate the header and NewSeqNo before delivering a SequenceReset.
    /// A Reset ignores its own MsgSeqNum; a GapFill follows normal ordering.
    pub(super) fn validate_sequence_reset<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        sequence_reset: SequenceResetBase,
        storage: &mut S,
    ) -> Option<ValidationResult> {
        let gap_fill_flag = sequence_reset.gap_fill_flag.unwrap_or(false);
        const MSG_TYPE: MsgTypeField = MsgTypeBase::SequenceReset.raw_value();
        if let Some(result) = self.validate(
            header,
            MSG_TYPE,
            storage,
            gap_fill_flag,
            gap_fill_flag,
            false,
        ) {
            return Some(result);
        }

        // A GapFill reaches this check in sequence. Equality is invalid
        // there (Test Cases Scenario 10(e)), but a Reset with the same
        // NewSeqNo is accepted (Scenario 11(b)). Neither refusal consumes
        // an incoming number (Scenario 11(c)).
        let new_seq_no = sequence_reset.new_seq_no;
        let next_target = storage.next_target_msg_seq_num().get();
        if new_seq_no < next_target || (gap_fill_flag && new_seq_no == next_target) {
            return Some(ValidationResult::Failure(self.validation_failure(
                ValidationError::InvalidNewSeqNo {
                    expected: next_target,
                },
                Some(Cow::Borrowed(fix_str!(
                    "ValueIsIncorrect (tag=36) - attempt to lower sequence number"
                ))),
                SequenceAction::Preserve,
                None,
            )));
        }

        None
    }

    /// Validate the header and requested range before delivering a ResendRequest.
    /// A valid request's too-high MsgSeqNum is handled after application acceptance.
    pub(super) fn validate_resend_request<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        resend_request: ResendRequestBase,
        storage: &mut S,
    ) -> Option<ValidationResult> {
        const MSG_TYPE: MsgTypeField = MsgTypeBase::ResendRequest.raw_value();
        if let Some(result) = self.validate(header, MSG_TYPE, storage, false, true, false) {
            return Some(result);
        }

        let begin_seq_no = resend_request.begin_seq_no;
        let end_seq_no = resend_request.end_seq_no;
        // BeginSeqNo starts at 1; EndSeqNo=0 means infinity (Session Layer
        // Sections 4.1, 4.8.2). An invalid range draws Reject 373=5
        // (Test Cases Scenario 14(e)), even when its MsgSeqNum is too high:
        // a queued ResendRequest is not processed again after gap recovery.
        if begin_seq_no == 0 || (end_seq_no != 0 && begin_seq_no > end_seq_no) {
            let text = format!(
                "ValueIsIncorrect (tag=7) - invalid resend range {begin_seq_no}..{end_seq_no}"
            );
            return Some(ValidationResult::Failure(self.validation_failure(
                ValidationError::InvalidResendRange,
                Some(Cow::Owned(FixString::from_ascii_lossy(text.into_bytes()))),
                if header.msg_seq_num > storage.next_target_msg_seq_num().get() {
                    SequenceAction::Enqueue
                } else {
                    SequenceAction::Consume
                },
                None,
            )));
        }

        None
    }

    /// Prepare a refusal with the session's termination policy.
    pub(super) fn validation_failure(
        &self,
        error: ValidationError,
        text: Option<Cow<'static, FixStr>>,
        sequence: SequenceAction,
        session_status: Option<SessionStatusField>,
    ) -> ValidationFailure {
        let disconnect = match &error {
            ValidationError::CompIdMismatch { .. } => Some(DisconnectReason::InvalidCompId),
            ValidationError::SendingTimeAccuracy { .. } => {
                Some(DisconnectReason::SendingTimeAccuracyProblem)
            }
            ValidationError::OrigSendingTimeAfterSendingTime => {
                Some(DisconnectReason::InvalidOrigSendingTime)
            }
            // Rejecting the reset ACK number cannot leave it waiting for reuse.
            ValidationError::MissingOrigSendingTime => self
                .state
                .local_reset_unconfirmed
                .then_some(DisconnectReason::InvalidLogonState),
            ValidationError::InvalidNewSeqNo { .. } | ValidationError::InvalidResendRange => None,
            ValidationError::MsgSeqNumTooLow { .. } => Some(DisconnectReason::MsgSeqNumTooLow),
            ValidationError::UnexpectedMessage => {
                return ValidationFailure {
                    error,
                    reaction: ValidationReaction::Disconnect(DisconnectReason::InvalidLogonState),
                };
            }
            ValidationError::UnexpectedMessageDuringReset
            | ValidationError::ResetAcknowledgementRetransmitted
            | ValidationError::InvalidResetAcknowledgement
            | ValidationError::InvalidResetSequenceNumber
            | ValidationError::UnsolicitedReset
            | ValidationError::ResetNotAllowedOnConnect
            | ValidationError::ResetNotAllowedInSession
            | ValidationError::UnsupportedEncryptMethod
            | ValidationError::InvalidHeartBtInt
            | ValidationError::HeartBtIntMismatch { .. }
            | ValidationError::InvalidNextExpectedMsgSeqNum { .. } => {
                Some(DisconnectReason::InvalidLogonState)
            }
        };
        ValidationFailure {
            error,
            reaction: ValidationReaction::Reply {
                text,
                disconnect,
                sequence,
                session_status,
            },
        }
    }

    /// Execute a refusal after notification, or directly for a failed decode.
    pub(super) fn apply_validation_reaction<S: MessagesStorage>(
        &mut self,
        msg_type: MsgTypeField,
        seq_num: SeqNum,
        failure: ValidationFailure,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;
        Ok(match failure.reaction {
            ValidationReaction::Reply {
                text,
                disconnect,
                sequence,
                session_status,
            } => {
                if matches!(sequence, SequenceAction::Consume) {
                    self.consume_seq_num(msg_type, seq_num, storage)?;
                }
                if let Some(reason) = failure.error.reject_reason() {
                    self.send_reject(
                        Some(Cow::Owned(msg_type.as_fix_str().to_owned())),
                        seq_num,
                        reason,
                        failure.error.tag(),
                        text.clone(),
                    );
                }
                if let Some(reason) = disconnect {
                    // Preserve the original diagnostic on both replies
                    // (Test Cases Scenarios 1S(d), 2(k), and 2(o)).
                    self.push_logout(session_status, text);
                    HandlerResult::Disconnect(reason)
                } else if matches!(sequence, SequenceAction::Enqueue) {
                    HandlerResult::Enqueue
                } else {
                    HandlerResult::Handled
                }
            }
            ValidationReaction::Disconnect(reason) => HandlerResult::Disconnect(reason),
        })
    }
}
