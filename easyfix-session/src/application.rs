use std::{
    borrow::Cow,
    io,
    net::SocketAddr,
    num::{NonZeroU16, NonZeroU64},
    time::Duration,
};

use easyfix_core::{
    base_messages::SessionRejectReasonBase,
    basic_types::{
        FixStr, FixString, Length, SeqNum, SessionRejectReasonField, SessionStatusField, TagNum,
        TimePrecision,
    },
    message::{DeserializeError, SessionMessage},
    serializer::SerializeError,
};
use thiserror::Error;

use crate::{io::sender::Sender, session_id::SessionId};

/// An incoming message failed decoding or session validation.
#[derive(Debug)]
pub enum InputError<'a, M> {
    /// Decoding failed. The error includes the parsed header when available.
    /// Session state may already have changed when this is reported, but
    /// any resulting protocol response has not yet been sent.
    Deserialize(&'a DeserializeError),
    /// A decoded message failed session validation. Reported once before its
    /// protocol response or sequence-number update. Deferred messages are
    /// reported only if validation later fails; ignored duplicates are not.
    Validation {
        /// The decoded message that failed validation.
        msg: &'a M,
        /// The detected validation failure.
        error: &'a ValidationError,
    },
}

/// Why a decoded incoming message failed session validation.
///
/// The offending values are available in the message passed to
/// [`SessionHandler::on_input_error`]. A failure describes the validation
/// result; it does not guarantee that a protocol response was sent.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    /// A component identifier does not match the configured counterparty.
    #[error("component identifier at tag {tag} does not match {expected}")]
    CompIdMismatch {
        /// `SenderCompID(49)` or `TargetCompID(56)`.
        tag: TagNum,
        /// The configured value required for this tag.
        expected: FixString,
    },
    /// SendingTime falls outside the configured clock tolerance.
    #[error("SendingTime(52) exceeds the allowed clock tolerance {max_latency:?}")]
    SendingTimeAccuracy {
        /// Maximum permitted difference from the receiver's clock.
        max_latency: Duration,
    },
    /// A retransmitted message requires an OrigSendingTime value.
    #[error("OrigSendingTime(122) is required")]
    MissingOrigSendingTime,
    /// OrigSendingTime is later than SendingTime.
    #[error("OrigSendingTime(122) is later than SendingTime(52)")]
    OrigSendingTimeAfterSendingTime,
    /// MsgSeqNum is below the expected number without duplicate permission.
    #[error("MsgSeqNum(34) is too low; expected {expected}")]
    MsgSeqNumTooLow {
        /// Next incoming sequence number expected by the session.
        expected: SeqNum,
    },
    /// The message is not allowed in the current session state.
    #[error("message is not allowed in the current session state")]
    UnexpectedMessage,
    /// The message is not an acknowledgement or refusal of a pending reset.
    #[error("unexpected message while awaiting sequence number reset confirmation")]
    UnexpectedMessageDuringReset,
    /// A retransmitted Logon cannot confirm a locally initiated reset.
    #[error("retransmitted Logon cannot acknowledge sequence number reset")]
    ResetAcknowledgementRetransmitted,
    /// A reset acknowledgement lacks ResetSeqNumFlag=Y or MsgSeqNum=1.
    #[error("reset acknowledgement requires ResetSeqNumFlag=Y and MsgSeqNum=1")]
    InvalidResetAcknowledgement,
    /// A reset request has a MsgSeqNum other than 1.
    #[error("ResetSeqNumFlag=Y requires MsgSeqNum=1")]
    InvalidResetSequenceNumber,
    /// A Logon response requests a reset that was not initiated locally.
    #[error("unsolicited ResetSeqNumFlag=Y in Logon response")]
    UnsolicitedReset,
    /// The session does not permit a reset during connection establishment.
    #[error("sequence number reset on connection establishment is not allowed")]
    ResetNotAllowedOnConnect,
    /// The session does not permit a reset while established.
    #[error("sequence number reset in an established session is not allowed")]
    ResetNotAllowedInSession,
    /// EncryptMethod requests an unsupported encryption method.
    #[error("unsupported EncryptMethod(98)")]
    UnsupportedEncryptMethod,
    /// HeartBtInt is negative.
    #[error("HeartBtInt(108) must be non-negative")]
    InvalidHeartBtInt,
    /// HeartBtInt differs from the interval required by the session.
    #[error("HeartBtInt(108) does not match the required {expected} seconds")]
    HeartBtIntMismatch {
        /// Required interval in seconds; zero disables regular heartbeats.
        expected: u64,
    },
    /// NextExpectedMsgSeqNum is zero or exceeds the sender's next number.
    #[error("NextExpectedMsgSeqNum(789) must be between 1 and {next_sender}")]
    InvalidNextExpectedMsgSeqNum {
        /// Sender counter against which the offered value was checked.
        /// A peer reset request is checked against its planned value of 1.
        next_sender: SeqNum,
    },
    /// A ResendRequest names an invalid sequence-number range.
    #[error("invalid ResendRequest sequence-number range")]
    InvalidResendRange,
    /// NewSeqNo would lower the incoming counter, or a GapFill would not advance it.
    #[error("NewSeqNo(36) is invalid for the expected incoming number {expected}")]
    InvalidNewSeqNo {
        /// Current expected incoming number. A Reset may preserve it;
        /// a GapFill must advance it.
        expected: SeqNum,
    },
}

impl ValidationError {
    /// The field responsible for the failure, when one field identifies it.
    pub fn tag(&self) -> Option<TagNum> {
        match self {
            Self::CompIdMismatch { tag, .. } => Some(*tag),
            Self::SendingTimeAccuracy { .. } => Some(52),
            Self::MissingOrigSendingTime | Self::OrigSendingTimeAfterSendingTime => Some(122),
            Self::MsgSeqNumTooLow { .. } | Self::InvalidResetSequenceNumber => Some(34),
            Self::ResetAcknowledgementRetransmitted => Some(43),
            Self::UnsolicitedReset
            | Self::ResetNotAllowedOnConnect
            | Self::ResetNotAllowedInSession => Some(141),
            Self::UnsupportedEncryptMethod => Some(98),
            Self::InvalidHeartBtInt | Self::HeartBtIntMismatch { .. } => Some(108),
            Self::InvalidNextExpectedMsgSeqNum { .. } => Some(789),
            Self::InvalidResendRange => Some(7),
            Self::InvalidNewSeqNo { .. } => Some(36),
            Self::UnexpectedMessage
            | Self::UnexpectedMessageDuringReset
            | Self::InvalidResetAcknowledgement => None,
        }
    }

    /// The session reject classification, when this failure calls for Reject.
    /// This does not guarantee that a Reject was sent successfully.
    pub fn reject_reason(&self) -> Option<SessionRejectReasonField> {
        let reason = match self {
            Self::CompIdMismatch { .. } => SessionRejectReasonBase::CompIdProblem,
            Self::SendingTimeAccuracy { .. } | Self::OrigSendingTimeAfterSendingTime => {
                SessionRejectReasonBase::SendingTimeAccuracyProblem
            }
            Self::MissingOrigSendingTime => SessionRejectReasonBase::RequiredTagMissing,
            Self::InvalidHeartBtInt | Self::InvalidResendRange | Self::InvalidNewSeqNo { .. } => {
                SessionRejectReasonBase::ValueIsIncorrect
            }
            _ => return None,
        };
        Some(reason.into())
    }
}

/// Application's response to an inbound message.
#[derive(Debug)]
pub enum InputAction {
    /// Accept the message.
    Accept,
    /// Reject with session-level `Reject<3>`.
    /// On an acknowledgement of our reset, also send Logout (unless already
    /// sent) and end the connection with [`SessionEndReason::ApplicationForcedDisconnect`].
    Reject {
        /// Sent as `SessionRejectReason(373)`.
        reason: SessionRejectReasonField,
        /// Sent as `Text(58)`, which `Reject<3>` marks optional
        /// (FIX Transport §5.5). A diagnostic is strongly recommended
        /// (FIX Session Layer §4.5.4), so prefer one where the reason
        /// alone does not identify the problem. An empty string is
        /// treated as `None` - a FIX field cannot carry an empty value.
        text: Option<Cow<'static, FixStr>>,
        /// Tag that triggered the reject, sent as `RefTagID(371)`.
        tag: Option<TagNum>,
    },
    /// Send `Logout<5>` and optionally disconnect.
    /// An immediate refusal of an acceptor's initial Logon follows
    /// [`SessionSettings::preserve_seq_num_on_logon_refusal`].
    ///
    /// [`SessionSettings::preserve_seq_num_on_logon_refusal`]: crate::SessionSettings::preserve_seq_num_on_logon_refusal
    Logout {
        /// Sent as `SessionStatus(1409)`.
        session_status: Option<SessionStatusField>,
        /// Sent as `Text(58)`. An empty string is treated as `None` - a FIX
        /// field cannot carry an empty value.
        text: Option<Cow<'static, FixStr>>,
        /// If `true`, disconnect after sending the Logout.
        disconnect: bool,
    },
    /// Force immediate disconnect without sending a response.
    /// An acceptor's initial Logon refusal follows
    /// [`SessionSettings::preserve_seq_num_on_logon_refusal`]. A refused
    /// request never applies its `ResetSeqNumFlag(141)=Y` reset.
    /// Other in-sequence messages consume their number, except SequenceReset.
    ///
    /// [`SessionSettings::preserve_seq_num_on_logon_refusal`]: crate::SessionSettings::preserve_seq_num_on_logon_refusal
    Disconnect,
}

/// `Text(58)` for refusing a `Logon<A>` whose `HeartBtInt(108)` is not the
/// single value this acceptor requires: `"Invalid HeartBtInt(108), expected
/// value N seconds"`, the wording FIX Session Layer §4.3.5.1 prescribes.
///
/// Wrap in `Some(Cow::Owned(...))` and pass as the `text` of an
/// [`InputAction::Logout`] returned from [`SessionHandler::on_admin_msg_in`].
/// `expected` is the spec's N, which Section 4.3.5.1 requires to be larger
/// than zero.
pub fn invalid_heart_bt_int_text(expected: impl Into<NonZeroU64>) -> FixString {
    let expected: NonZeroU64 = expected.into();
    // The spec's wording is ASCII throughout and `expected` renders as
    // digits, so the lossy conversion has nothing to replace.
    FixString::from_ascii_lossy(
        format!("Invalid HeartBtInt(108), expected value {expected} seconds").into_bytes(),
    )
}

/// `Text(58)` for refusing a `Logon<A>` whose `HeartBtInt(108)` falls outside
/// the range this acceptor requires: `"Invalid HeartBtInt(108), expected value
/// between N and M seconds"`, the wording FIX Session Layer §4.3.5.2
/// prescribes.
///
/// Wrap in `Some(Cow::Owned(...))` and pass as the `text` of an
/// [`InputAction::Logout`] returned from [`SessionHandler::on_admin_msg_in`].
/// `min` and `max` are the spec's N and M; §4.3.5.2 requires M to be larger
/// than N, which the types cannot express.
pub fn invalid_heart_bt_int_range_text(min: NonZeroU16, max: NonZeroU16) -> FixString {
    FixString::from_ascii_lossy(
        format!("Invalid HeartBtInt(108), expected value between {min} and {max} seconds")
            .into_bytes(),
    )
}

/// `Text(58)` for terminating a session whose peer announced a
/// `MaxMessageSize(383)` this side cannot honour: `"MaxMessageSize(383)=X
/// exceeds maximum message size of Y"`, the wording FIX Session Layer §4.3.6
/// prescribes.
///
/// Wrap in `Some(Cow::Owned(...))` and pass as the `text` of an
/// [`InputAction::Logout`] returned from [`SessionHandler::on_admin_msg_in`].
/// `announced` is the peer's value, taken from its `Logon<A>`; `supported`
/// is the largest this side can process.
/// The session does not judge the peer's value itself - either peer may end
/// the connection over it, and which sizes are workable is the application's
/// to know.
pub fn max_message_size_exceeded_text(announced: Length, supported: Length) -> FixString {
    FixString::from_ascii_lossy(
        format!("MaxMessageSize(383)={announced} exceeds maximum message size of {supported}")
            .into_bytes(),
    )
}

/// The context and outcome of one session connection.
#[derive(Debug)]
pub struct SessionEnd {
    /// The procedure in progress, or the first decision to end the session.
    pub context: EndContext,
    /// The event that finally ended the connection.
    pub termination: Termination,
    /// A local sequence-number reset was not confirmed by the peer.
    /// Coordinate numbering before resuming; see the
    /// [recovery instructions](crate::session_reset#recovery-after-an-unconfirmed-reset).
    pub reset_unconfirmed: bool,
}

/// Session context preserved when its connection ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndContext {
    /// The initial Logon handshake was in progress.
    Logon,
    /// The session was exchanging ordinary traffic.
    Established,
    /// A running reset was requested but local numbering had not been reset.
    ResetPreparation,
    /// A running reset Logon was awaiting acknowledgement.
    Reset,
    /// A locally requested Logout was queued or awaiting acknowledgement.
    /// `Termination::LocalClose` means its acknowledgement was received.
    LocalLogout,
    /// The peer requested Logout. This includes sending the acknowledgement
    /// and waiting for the peer to close the connection, including expiry of
    /// that wait with `Termination::Timeout(TimeoutKind::PeerClose)`.
    RemoteLogout,
    /// The session had decided to end for this reason. A subsequent failure
    /// during teardown does not replace it.
    Closing(SessionEndReason),
}

/// The event that ended a connection, independently of its session context.
#[derive(Debug)]
pub enum Termination {
    /// This side closed the connection after completing its local work.
    LocalClose,
    /// Reading the peer's connection reached EOF.
    PeerClose,
    /// Reading or writing failed. Includes a write exceeding `write_timeout`,
    /// represented by an error with kind `io::ErrorKind::TimedOut`.
    IoError {
        /// The failed operation.
        operation: IoOperation,
        /// The original transport error, including its source when available.
        error: io::Error,
    },
    /// A protocol wait exceeded its configured time budget.
    Timeout(TimeoutKind),
    /// The Logout could not be serialized, so its exchange could not finish.
    SerializeError(SerializeError),
}

/// The transport operation that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoOperation {
    Read,
    Write,
}

/// The protocol wait whose budget expired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutKind {
    /// Waiting for the first inbound message or for a Logon acknowledgement.
    Logon,
    /// Waiting for an acknowledgement of our Logout.
    LogoutResponse,
    /// Waiting for the peer to close after we acknowledged its Logout.
    PeerClose,
    /// Waiting for recovery and probes before a running reset.
    ResetPreparation,
}

/// Why the session decided to end its connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndReason {
    /// A protocol wait expired. The cause remains visible if flushing the
    /// final output fails after that decision.
    Timeout(TimeoutKind),
    /// The application forced closure without waiting for a Logout response,
    /// with or without sending a farewell Logout. Applies to inbound-message
    /// callbacks and requests through the acceptor or initiator API,
    /// including forced acceptor shutdown.
    ApplicationForcedDisconnect,
    /// Received a message without `MsgSeqNum(34)`. The session sends
    /// `Logout<5>` and disconnects (FIX Session Layer §4.5.3).
    MsgSeqNumNotFound,
    /// Received a message whose `BeginString(8)` is a recognized FIX version
    /// other than the session's. The session sends `Logout<5>` and
    /// disconnects (FIX Session Layer Test Cases Scenario 2(i)).
    InvalidBeginString,
    /// Received `MsgSeqNum(34)` lower than expected without
    /// `PossDupFlag(43)=Y`. The session sends `Logout<5>`
    /// (`SessionStatus(1409)=9`) and disconnects (FIX Session Layer §4.8.1).
    MsgSeqNumTooLow,
    /// A message not valid in the current logon state - e.g. the first
    /// message is not a `Logon<A>`, an unexpected `Logon<A>` arrives while
    /// established, or a `Logon<A>` fails session validation.
    InvalidLogonState,
    /// `SenderCompID(49)` or `TargetCompID(56)` does not match the session
    /// configuration (checked when `check_comp_id` is enabled). The session
    /// sends `Reject<3>` (`SessionRejectReason(373)=9`) followed by
    /// `Logout<5>` and disconnects (FIX Session Layer §4.2.2).
    InvalidCompId,
    /// `OrigSendingTime(122)` later than `SendingTime(52)` on a
    /// `PossDupFlag(43)=Y` message with a too-low `MsgSeqNum(34)`. The
    /// session sends `Reject<3>` (`SessionRejectReason(373)=10`) followed by
    /// `Logout<5>` and disconnects (FIX Session Layer §4.8.4).
    InvalidOrigSendingTime,
    /// `SendingTime(52)` outside the synchronized-clock tolerance
    /// (`max_latency`). The session sends `Reject<3>`
    /// (`SessionRejectReason(373)=10`) followed by `Logout<5>` and
    /// disconnects (FIX Session Layer §4.2.3).
    SendingTimeAccuracyProblem,
    /// A storage operation or replay of a stored message failed. The
    /// connection closes without further storage access or a Logout exchange.
    /// The operation and source are logged. Restore consistent backend state
    /// before reusing it after an error with an uncertain outcome.
    StorageError,
    /// The peer's traffic stopped and it answered none of the `TestRequest<1>`
    /// probes allowed by `auto_disconnect_after_no_heartbeat`. The session
    /// sends `Logout<5>` (`Text(58)="Heartbeat timeout"`) and disconnects
    /// (FIX Session Layer §4.5.5).
    HeartbeatTimeout,
    /// A logged-on peer fell behind: its outbound staging queue exceeded
    /// `max_outbound_queued_messages` or its head-of-queue age exceeded
    /// `max_outbound_lag`. With history enabled and healthy storage, the
    /// still-queued messages are stored for resend on reconnect.
    SlowConsumer,
    /// An inbound message declared a length above `max_message_size`. The
    /// session sends `Logout<5>` naming the limit in `Text(58)` and
    /// disconnects, the answer FIX Session Layer §4.3.6 gives for a size a
    /// peer cannot process. The message is never read, so `NextNumIn` does
    /// not advance and the peer offers it again on reconnect.
    ///
    /// The limit holds whether or not the peer was told about it: the
    /// session announces it in `MaxMessageSize(383)` only where the
    /// dictionary's Logon carries that field, and the field is optional.
    MessageTooLarge,
    /// A sequence number counter reached `SeqNum::MAX`, the implementation
    /// limit. The last usable message number is `SeqNum::MAX - 1`.
    ///
    /// The condition persists across reconnects. Coordinate a
    /// [reset of the sequence numbers](crate::session_reset#choosing-a-method)
    /// with the peer before resuming.
    SeqNumExhausted,
}

/// Handles messages and events for one connection of a session.
///
/// A new handler is created for each connection, including reconnects.
/// See [`Initiator::new`](crate::Initiator::new) and
/// [`Acceptor::new`](crate::Acceptor::new) for initialization.
///
/// Callbacks delay message processing and heartbeats until they return,
/// so they should complete promptly.
#[expect(async_fn_in_trait, reason = "single-threaded runtime, Send not needed")]
pub trait SessionHandler<M: SessionMessage> {
    /// This side's part of the Logon handshake is on the wire. Provides the
    /// [`Sender`] for staging outgoing messages.
    ///
    /// What has happened by this point depends on the role:
    ///
    /// - **Initiator**: our `Logon<A>` request has been written; the peer's
    ///   response has not arrived yet. The handshake completes when that
    ///   response reaches [`on_admin_msg_in`](Self::on_admin_msg_in) as a
    ///   `Logon<A>` - that callback is where an initiator observes the
    ///   session becoming established, and where it may still refuse it.
    /// - **Acceptor**: the answer to the peer's first `Logon<A>` has been
    ///   written. Normally that is the Logon acknowledgement, which
    ///   establishes the session. If `on_admin_msg_in` answered the Logon
    ///   with [`InputAction::Reject`], the answer was that `Reject<3>` and
    ///   the handshake is still open, as on the initiator side.
    ///
    /// The `Sender` may be used right away. Application messages staged
    /// before the session is established are held back until it is, then
    /// transmitted in staging order (FIX Session Layer §4.3.10: no
    /// application message before the Logon acknowledgement). If the
    /// handshake fails instead, the held messages are stored for replay when
    /// history is enabled and storage is healthy, otherwise discarded, and
    /// [`on_session_end`](Self::on_session_end) follows.
    async fn on_session_ready(&mut self, session_id: &SessionId, sender: Sender<M>);

    /// The connection is over: disconnected, or the Logout exchange complete.
    /// The transport has been released and the sender is closed. Unsent
    /// messages have been offered to storage when history is enabled and
    /// storage is healthy; any remaining messages have been discarded.
    /// See [`SessionEnd`] for the context, transport outcome, and reset status.
    ///
    /// This can be the first and only callback a connection's `SessionHandler`
    /// receives. [`on_session_ready`](Self::on_session_ready) is skipped when
    /// the connection ends before there is a session to announce:
    ///
    /// - the acceptor refuses the peer's first `Logon<A>` - by its own
    ///   verdict (CompID, `SendingTime`, `HeartBtInt`, sequence number, an
    ///   undecodable message), or because
    ///   [`on_admin_msg_in`](Self::on_admin_msg_in) returned
    ///   [`InputAction::Logout`] or [`InputAction::Disconnect`];
    /// - writing the opening `Logon<A>` request or response failed;
    /// - the outgoing sequence numbers were already exhausted when the
    ///   connection started ([`SessionEndReason::SeqNumExhausted`]).
    ///
    /// No [`Sender`] was handed over in those cases. An implementation that
    /// pairs the two callbacks must tolerate the missing first half.
    ///
    /// Otherwise the [`Sender`] provided by `on_session_ready` is no longer
    /// usable after this callback returns - its underlying channel is closed.
    /// A `send()` after the session ends returns [`SendError::Closed`],
    /// handing the unsent message back. A `send()` issued from *within* this
    /// callback also returns [`SendError::Closed`]. On reconnect, a new
    /// `SessionHandler` instance is created by the `make_handler` closure
    /// and receives a fresh `Sender` through a new `on_session_ready` call.
    ///
    /// [`SendError::Closed`]: crate::SendError::Closed
    async fn on_session_end(&mut self, session_id: &SessionId, end: SessionEnd);

    /// Inbound application message that passed session validation.
    ///
    /// A MsgType that is not in the dictionary is rejected during decoding
    /// with a session-level `Reject<3>`, `SessionRejectReason=InvalidMsgType`.
    /// Session validation failures are reported through
    /// [`Self::on_input_error`] instead of this callback.
    ///
    /// Rejecting a message whose MsgType **is** valid but is **not supported**
    /// by this application is the application's job: send a
    /// `BusinessMessageReject<j>` (`BusinessRejectReason(380)=3` -
    /// Unsupported Message Type) and return [`InputAction::Accept`] so
    /// `NextNumIn` still advances.
    //
    // The session does not auto-generate it: only the application knows which
    // valid app MsgTypes it supports, and `BusinessMessageReject<j>` is itself
    // an application message.
    async fn on_app_msg_in(&mut self, msg: Box<M>) -> InputAction;

    /// Inbound admin message that passed all applicable session validation
    /// for its message type, the current session state, and configuration.
    /// Return [`InputAction`] to accept or refuse it (e.g. invalid Logon
    /// credentials).
    ///
    /// [`InputAction::Accept`] proceeds with message-specific processing
    /// without further header or body validation. Processing may still fail
    /// due to storage or transport errors. Messages rejected by session
    /// validation are not delivered to this callback.
    ///
    /// For reset requests and acknowledgements, see
    /// [reset callbacks](crate::session_reset#application-callbacks).
    ///
    /// Environment validation of `TestMessageIndicator(464)` on an inbound
    /// `Logon<A>` (FIX Session Layer §4.3.2) is the application's job, not the
    /// session's. An application that distinguishes environments inspects
    /// `464` here and returns [`InputAction::Logout`] (with `Text(58)`
    /// explaining the mismatch) when it does not correspond to its own.
    /// Mirrors the unsupported-MsgType case on
    /// [`on_app_msg_in`](Self::on_app_msg_in).
    //
    // The library cannot know whether it runs as a test or a production
    // instance - the same build may be deployed as either, so it has no basis
    // to compare `464` against an "environment". The mandate is "should"-level
    // and venue-configurable.
    async fn on_admin_msg_in(&mut self, _msg: &M) -> InputAction {
        InputAction::Accept
    }

    /// Outgoing admin message after session header stamping, before sending.
    /// Called for ALL admin messages - both engine-produced and user-sent.
    /// NOT called for resend messages.
    ///
    /// On `Logon<A>`, `EncryptMethod(98)` must remain `0`: FIX application-layer
    /// encryption is unsupported.
    /// `MsgSeqNum(34)`, `SendingTime(52)`, `HeartBtInt(108)`,
    /// `ResetSeqNumFlag(141)` and `NextExpectedMsgSeqNum(789)` describe session
    /// state: the hook may read them but must not change them. The session
    /// does not re-read or validate these fields after the hook; changing them
    /// can put numbering or heartbeat state on the wire that differs from
    /// the session's own state.
    fn on_admin_msg_out(&mut self, _msg: &mut M) {}

    /// Outgoing application message about to be sent. Can modify
    /// (header stamping). NOT called for resend messages.
    fn on_app_msg_out(&mut self, _msg: &mut M) {}

    /// During resend, should this message be sent as a `SequenceReset`-GapFill
    /// instead of being resent? Only called for app messages and `Reject<3>`.
    /// Skipping replay leaves the stored record unchanged. Not called when
    /// [`persist_messages`](crate::SessionSettings::persist_messages) is false.
    /// Default: false (resend the message).
    fn should_gap_fill(&mut self, _msg: &M) -> bool {
        false
    }

    /// An incoming message failed decoding or session validation.
    ///
    /// The failed message is not delivered to either input message callback.
    /// This notification may precede [`Self::on_session_ready`]. See
    /// [`InputError`] for the timing and available data for each failure kind.
    ///
    /// Application refusals and storage errors do not trigger this callback.
    /// The session chooses the response; notification cannot override it.
    fn on_input_error(&mut self, _error: InputError<'_, M>) {}

    /// An outgoing message failed to serialize and was not sent - it will not
    /// be retried, and the peer never learns of it. The sequence number it
    /// was assigned is released for the next message, so nothing changes on
    /// the wire; only a message that is sent consumes a sequence number
    /// (FIX Session Layer §4.1).
    ///
    /// The sequence number and `SendingTime` the session stamped on `msg`
    /// are cleared again, so the message can be fixed and staged again
    /// without a duplicate number or a stale time; whatever the application
    /// set itself stays. What to do is the application's call - e.g. panic,
    /// escalate, or drop with a log.
    ///
    /// If an engine-generated Logout fails to serialize, the connection
    /// closes after this callback instead of waiting for a reply.
    ///
    /// Transport and storage failures are reported through
    /// [`Self::on_session_end`], not this callback.
    fn on_output_error(&mut self, msg: Box<M>, error: &SerializeError);
}

/// Connection-scoped facts passed to the `make_handler` closure when it
/// constructs a [`SessionHandler`].
///
/// A read-only view: it is constructed by the library and only ever passed
/// *to* application code, never accepted back by any session API.
//
// New attributes are exposed as additional accessor methods rather than as
// extra closure parameters, so its signature stays stable as the crate grows.
#[derive(Debug)]
pub struct SessionContext<'a> {
    session_id: &'a SessionId,
    peer_addr: Option<SocketAddr>,
    time_precision: TimePrecision,
}

impl<'a> SessionContext<'a> {
    pub(crate) fn new(
        session_id: &'a SessionId,
        peer_addr: Option<SocketAddr>,
        time_precision: TimePrecision,
    ) -> Self {
        SessionContext {
            session_id,
            peer_addr,
            time_precision,
        }
    }

    /// Identity of the session this connection serves.
    pub fn session_id(&self) -> &'a SessionId {
        self.session_id
    }

    /// Remote address of the underlying connection.
    ///
    /// Always `Some` on the acceptor path - [`Connection::accept`] yields an
    /// address for every connection - and on an initiator session started
    /// with [`Initiator::connect`]. On an initiator session running over a
    /// caller-supplied transport ([`Initiator::run_session`] /
    /// [`Initiator::session_task`]) it is whatever the caller passed: such a
    /// transport need not be a socket, so the address cannot be derived and
    /// is supplied explicitly (e.g. from the `TcpStream` under a TLS
    /// session).
    ///
    /// Note that this is the address of the immediate transport peer: behind
    /// a proxy or load balancer it identifies that intermediary, not the
    /// counterparty.
    ///
    /// [`Connection::accept`]: crate::Connection::accept
    /// [`Initiator::connect`]: crate::Initiator::connect
    /// [`Initiator::run_session`]: crate::Initiator::run_session
    /// [`Initiator::session_task`]: crate::Initiator::session_task
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer_addr
    }

    /// The session's configured
    /// [`SessionSettings::time_precision`](crate::SessionSettings::time_precision).
    ///
    /// The session stamps only `SendingTime<52>`; timestamps in application
    /// message bodies are the handler's to render. Capture this value here, at
    /// construction, to render them at the same width - or pass a different
    /// one where the counterparty asked for a different width on that field.
    ///
    /// Only the width is shared, never the instant. A body timestamp is read
    /// when the message is built and tag 52 when it is transmitted, so the two
    /// values differ, the body one being the earlier.
    pub fn time_precision(&self) -> TimePrecision {
        self.time_precision
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use super::{
        invalid_heart_bt_int_range_text, invalid_heart_bt_int_text, max_message_size_exceeded_text,
    };

    fn nz(value: u16) -> NonZeroU16 {
        NonZeroU16::new(value).expect("non-zero")
    }

    /// The point of these helpers is that the wording matches FIX Session
    /// Layer §4.3.5.1 and §4.3.5.2 to the character - a peer may be matching
    /// on it, and a typo here is one nobody would notice on the wire.
    #[test]
    fn refusal_texts_match_the_prescribed_wording() {
        assert_eq!(
            invalid_heart_bt_int_text(nz(30)),
            "Invalid HeartBtInt(108), expected value 30 seconds"
        );
        assert_eq!(
            invalid_heart_bt_int_range_text(nz(10), nz(60)),
            "Invalid HeartBtInt(108), expected value between 10 and 60 seconds"
        );
        assert_eq!(
            max_message_size_exceeded_text(8192, 4096),
            "MaxMessageSize(383)=8192 exceeds maximum message size of 4096"
        );
    }
}
