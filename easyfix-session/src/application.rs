use std::{
    net::SocketAddr,
    num::{NonZeroU16, NonZeroU64},
};

use easyfix_core::{
    basic_types::{
        FixString, Length, SessionRejectReasonField, SessionStatusField, TagNum, TimePrecision,
    },
    message::{DeserializeError, SessionMessage},
    serializer::SerializeError,
};

use crate::{io::sender::Sender, session_id::SessionId};

/// Application's response to an inbound message.
#[derive(Debug)]
pub enum InputAction {
    /// Accept the message.
    Accept,
    /// Reject with session-level `Reject<3>`.
    /// On an acknowledgement of our reset, also send Logout (unless already
    /// sent) and end the connection with [`DisconnectReason::ApplicationForcedDisconnect`].
    Reject {
        /// Sent as `SessionRejectReason(373)`.
        reason: SessionRejectReasonField,
        /// Sent as `Text(58)`, which `Reject<3>` marks optional
        /// (FIX Transport §5.5). A diagnostic is strongly recommended
        /// (FIX Session Layer §4.5.4), so prefer one where the reason
        /// alone does not identify the problem. An empty string is
        /// treated as `None` - a FIX field cannot carry an empty value.
        text: Option<FixString>,
        /// Tag that triggered the reject, sent as `RefTagID(371)`.
        tag: Option<TagNum>,
    },
    /// Send `Logout<5>` and optionally disconnect.
    Logout {
        /// Sent as `SessionStatus(1409)`.
        session_status: Option<SessionStatusField>,
        /// Sent as `Text(58)`. An empty string is treated as `None` - a FIX
        /// field cannot carry an empty value.
        text: Option<FixString>,
        /// If `true`, disconnect after sending the Logout.
        disconnect: bool,
    },
    /// Force immediate disconnect without sending a response.
    /// When an acceptor refuses the initial Logon request, preserve the
    /// session's sequence numbers and message history, including when the
    /// request carries `ResetSeqNumFlag(141)=Y`. Other in-sequence messages
    /// consume their number, except SequenceReset.
    Disconnect,
}

/// `Text(58)` for refusing a `Logon<A>` whose `HeartBtInt(108)` is not the
/// single value this acceptor requires: `"Invalid HeartBtInt(108), expected
/// value N seconds"`, the wording FIX Session Layer §4.3.5.1 prescribes.
///
/// Pass it as the `text` of an [`InputAction::Logout`] returned from
/// [`Application::on_admin_msg_in`]. `expected` is the spec's N, which
/// Section 4.3.5.1 requires to be larger than zero.
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
/// Pass it as the `text` of an [`InputAction::Logout`] returned from
/// [`Application::on_admin_msg_in`]. `min` and `max` are the spec's N and M;
/// §4.3.5.2 requires M to be larger than N, which the types cannot express.
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
/// Pass it as the `text` of an [`InputAction::Logout`] returned from
/// [`Application::on_admin_msg_in`]. `announced` is the peer's value, taken
/// from its `Logon<A>`; `supported` is the largest this side can process.
/// The session does not judge the peer's value itself - either peer may end
/// the connection over it, and which sizes are workable is the application's
/// to know.
pub fn max_message_size_exceeded_text(announced: Length, supported: Length) -> FixString {
    FixString::from_ascii_lossy(
        format!("MaxMessageSize(383)={announced} exceeds maximum message size of {supported}")
            .into_bytes(),
    )
}

/// Why a session ended.
///
/// Passed to [`Application::on_session_end`] as the `reason`; covers orderly
/// logout completions as well as protocol violations and transport failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The connection ended after a locally initiated sequence-number reset
    /// whose execution by the peer was not confirmed. Coordinate numbering
    /// with the peer before resuming; see the
    /// [recovery instructions](crate::session_reset#recovery-after-an-unconfirmed-reset).
    SeqNumResetFailed,
    /// Preparation for an in-band reset did not complete within
    /// [`running_session_reset_timeout`](crate::SessionSettings::running_session_reset_timeout).
    /// The local counters and history were not reset; normal resumption from
    /// storage is available.
    ResetPreparationTimeout,
    /// Locally requested logout completed - the peer's `Logout<5>` response
    /// was received.
    LocalRequestedLogout,
    /// Logout requested remotely. The session confirms with its own
    /// `Logout<5>` (`SessionStatus(1409)=4`), then waits for the peer to
    /// close the connection (FIX Session Layer §4.6, Test Cases Scenario
    /// 13(b)); this is the reason once it does.
    RemoteRequestedLogout,
    /// Logout requested remotely and acknowledged, but the peer did not close
    /// the connection within `auto_disconnect_after_no_logout`; the session
    /// drops it (Test Cases Scenario 13(b)).
    RemoteRequestedLogoutTimeout,
    /// The application requested termination through an inbound-message
    /// callback. See [`InputAction`] for the actions that end the connection.
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
    /// The remote side closed the connection, or the session ended without a
    /// more specific reason (e.g. a disconnect requested through the session
    /// handle).
    Disconnected,
    /// A TCP read or write failed; the session ends without a `Logout<5>`
    /// exchange.
    IoError,
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
    /// The peer did not acknowledge this side's `Logout<5>` within
    /// `auto_disconnect_after_no_logout`; the connection is dropped
    /// (FIX Session Layer §4.6.2).
    LocalRequestedLogoutTimeout,
    /// The Logon handshake did not complete within
    /// [`auto_disconnect_after_no_logon_response`](crate::SessionSettings::auto_disconnect_after_no_logon_response).
    /// The connection is dropped without a Logout exchange.
    LogonTimeout,
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

/// Per-connection application handler. [`ApplicationFactory::create`] builds
/// a fresh instance for every connection a session makes, so the callbacks
/// below describe the life of one connection.
///
/// All callbacks execute inline in the session's IO loop. While a callback
/// runs, the session cannot process input, send heartbeats, or drain channels.
/// Callbacks should return promptly - if a callback takes longer than
/// `heartbeat_interval`, the peer may disconnect.
#[expect(async_fn_in_trait, reason = "single-threaded runtime, Send not needed")]
pub trait Application<M: SessionMessage> {
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
    /// See [`DisconnectReason`] for the outcome.
    ///
    /// This can be the first and only callback a connection's `Application`
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
    ///   connection started ([`DisconnectReason::SeqNumExhausted`]).
    ///
    /// No [`Sender`] was handed over in those cases. An implementation that
    /// pairs the two callbacks must tolerate the missing first half.
    ///
    /// Otherwise the [`Sender`] provided by `on_session_ready` is no longer
    /// usable after this callback returns - its underlying channel is closed.
    /// A `send()` after the session ends returns [`SendError::Closed`],
    /// handing the unsent message back. A `send()` issued from *within* this
    /// callback also returns [`SendError::Closed`]. On reconnect, a new
    /// `Application` instance is created via [`ApplicationFactory::create`]
    /// and receives a fresh `Sender` through a new `on_session_ready` call.
    ///
    /// [`SendError::Closed`]: crate::SendError::Closed
    async fn on_session_end(&mut self, session_id: &SessionId, reason: DisconnectReason);

    /// Inbound application message.
    ///
    /// The session delivers every message that parses against the dictionary
    /// (a MsgType that is not in the dictionary is rejected earlier with a
    /// session-level `Reject<3>`, `SessionRejectReason=InvalidMsgType`).
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

    /// Inbound admin message after header and protocol-state validation.
    /// Return [`InputAction`] to accept or refuse it (e.g. invalid Logon
    /// credentials); acceptance applies the remaining message-specific logic.
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

    /// Deserialization error on inbound message. The error carries the
    /// parsed header of the failed message when header parsing succeeded
    /// before the failure.
    fn on_deserialize_error(&mut self, _error: &DeserializeError) {}

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
    fn on_serialize_error(&mut self, msg: Box<M>, error: &SerializeError);
}

/// Connection-scoped facts available when a session's [`Application`] is
/// built, handed to [`ApplicationFactory::create`].
///
/// A read-only view: it is constructed by the library and only ever passed
/// *to* application code, never accepted back by any session API.
//
// New attributes are exposed as additional accessor methods rather than as
// extra `create` parameters, so the factory signature stays stable as the
// crate grows.
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

/// Factory for creating per-session [`Application`] instances.
///
/// The Acceptor/Initiator holds the factory and calls
/// [`create`](ApplicationFactory::create) when a session is established.
pub trait ApplicationFactory<M: SessionMessage> {
    /// Handler type produced by [`create`](Self::create).
    type App: Application<M>;

    /// Create an application handler for a new session.
    ///
    /// Called once per connection, before the session's first message is
    /// surfaced to the handler - so an inbound `Logon<A>` reaches
    /// [`on_admin_msg_in`](Application::on_admin_msg_in) on a handler that
    /// already carries everything read out of `ctx`.
    fn create(&self, ctx: &SessionContext<'_>) -> Self::App;
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
