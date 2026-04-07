use std::{
    num::{NonZeroU8, NonZeroU16, NonZeroUsize},
    time::Duration,
};

use easyfix_core::basic_types::{ApplVerId, NonZeroLength, TimePrecision};

/// Listener-level configuration for an [`Acceptor`](crate::Acceptor).
///
/// These values gate the acceptor's behavior *before* it knows which
/// session an incoming connection belongs to - the first-message buffer
/// size and the Logon timeout. Everything that applies after session
/// identification lives in [`SessionSettings`].
#[derive(Clone, Debug)]
pub struct AcceptorSettings {
    /// Upper bound, in bytes, on the first message of an inbound connection -
    /// the `Logon<A>` that identifies its session. A first message declaring
    /// more than this in `BodyLength(9)` is refused before its body is read
    /// and the connection is dropped without a reply
    /// ([`ConnectionDropReason::FirstMessageTooLarge`]).
    ///
    /// Each session bounds its traffic with its own `max_message_size`, but
    /// until the first message is read the acceptor does not know which
    /// session a connection is for, so this one limit stands in for all of
    /// them. A `Logon<A>` is small, and the default of 1 KiB leaves it a wide
    /// margin; raise it only for a peer known to send more.
    ///
    /// [`ConnectionDropReason::FirstMessageTooLarge`]: crate::ConnectionDropReason::FirstMessageTooLarge
    pub max_first_message_size: NonZeroUsize,

    /// Timeout for the first `Logon<A>` message after accepting a TCP
    /// connection. When reached, the connection is dropped.
    pub auto_disconnect_after_no_logon_received: Duration,
}

impl Default for AcceptorSettings {
    fn default() -> Self {
        AcceptorSettings {
            max_first_message_size: const { NonZeroUsize::new(1024).unwrap() },
            auto_disconnect_after_no_logon_received: Duration::from_secs(10),
        }
    }
}

/// Per-session FIX configuration.
///
/// Pure policy - the session's identity is not part of the settings. It is
/// passed separately where sessions are created:
/// [`Acceptor::register_session`](crate::Acceptor::register_session) and
/// [`Initiator::new`](crate::Initiator::new).
///
/// Every field has a library default, so construct via functional record
/// update and spell out only what differs:
///
/// ```
/// use easyfix_session::SessionSettings;
///
/// let settings = SessionSettings {
///     manages_admin_output: true,
///     ..SessionSettings::default()
/// };
/// ```
///
/// There is no separate "global" settings layer - shared defaults across
/// sessions are a construction-time concern. Any settings value works as the
/// record-update base, so a template can be reused per counterparty.
#[derive(Clone, Debug)]
// Scheduling belongs to the application: it chooses when to open, close,
// or reset a session under its bilateral agreement with the counterparty.
pub struct SessionSettings {
    /// Heartbeat interval in **seconds**, used **only on the initiator path**
    /// to drive the `HeartBtInt<108>` proposed in the outgoing `Logon<A>`:
    /// `Some(n)` proposes `n`; `None` proposes `0`, disabling regular
    /// heartbeats (FIX Transport §5.1 - no `Heartbeat<0>` / input-timeout
    /// `TestRequest<1>` is generated).
    ///
    /// With an effective interval of zero, the engine does not detect an
    /// unresponsive peer through keep-alive timeouts. The application can
    /// still send its own `TestRequest<1>` messages through `Sender::send`,
    /// track matching `TestReqID(112)` responses in
    /// `Application::on_admin_msg_in`, and enforce its own response timeout
    /// and disconnection policy. Incoming `TestRequest<1>` messages are
    /// still answered automatically.
    ///
    /// **The acceptor ignores this field for negotiation** - it adopts and
    /// echoes the initiator's `HeartBtInt` on the initial Logon. The initiator
    /// refuses an acknowledgement that does not echo its proposal (Session
    /// Test Cases Scenario 1B(d)).
    /// Whether the peer's value is *acceptable* is
    /// the application's call on either side: return `InputAction::Logout`
    /// from `Application::on_admin_msg_in`, carrying
    /// [`invalid_heart_bt_int_text`] or [`invalid_heart_bt_int_range_text`]
    /// as its `Text(58)`.
    ///
    /// [`invalid_heart_bt_int_text`]: crate::invalid_heart_bt_int_text
    /// [`invalid_heart_bt_int_range_text`]: crate::invalid_heart_bt_int_range_text
    pub heartbeat_interval: Option<NonZeroU16>,

    /// Number of unanswered `TestRequest<1>` probes before the session
    /// gives up: it sends `Logout<5>` with an explanatory `Text<58>` and
    /// disconnects (FIX Session Layer §4.5.5). Each probe re-arms the
    /// input deadline.
    ///
    /// Default `1` - the spec's letter: a single unanswered probe is
    /// already "considered an error". Higher values grant the peer extra
    /// probes before termination. The escalation cannot be switched off.
    /// It does not apply when [`heartbeat_interval`](Self::heartbeat_interval)
    /// is `None`: without an input timeout there are no keep-alive probes.
    //
    // No "never" mode: a half-open connection would then hold the session
    // slot for good, and the acceptor would refuse the restarted peer's
    // reconnect as a duplicate.
    pub auto_disconnect_after_no_heartbeat: NonZeroU8,

    /// How long a graceful termination may wait on the peer. For a Logout of
    /// ours, the wait for its `Logout<5>` acknowledgement
    /// ([`DisconnectReason::LocalRequestedLogoutTimeout`] on expiry); for a
    /// Logout of the peer's, the wait for it to close the connection after
    /// our acknowledgement ([`DisconnectReason::RemoteRequestedLogoutTimeout`]).
    /// Test Cases Scenario 13(b) allows 10 seconds for the latter.
    ///
    /// A value too large for the clock to represent (such as
    /// [`Duration::MAX`]) means no limit.
    ///
    /// [`DisconnectReason::LocalRequestedLogoutTimeout`]: crate::DisconnectReason::LocalRequestedLogoutTimeout
    /// [`DisconnectReason::RemoteRequestedLogoutTimeout`]: crate::DisconnectReason::RemoteRequestedLogoutTimeout
    pub auto_disconnect_after_no_logout: Duration,

    /// How long the Logon handshake may take, measured from the start of the
    /// session. On expiry the connection is dropped
    /// ([`DisconnectReason::LogonTimeout`]) with no `Logout<5>` - the handshake
    /// never completed, so there is no session to end.
    ///
    /// Covers an initiator awaiting the peer's `Logon<A>` acknowledgement and
    /// an acceptor whose handshake stalled - notably when the application
    /// answered the peer's `Logon<A>` with `InputAction::Reject` and the peer
    /// never followed up. The acceptor's
    /// [`auto_disconnect_after_no_logon_received`] is a different, earlier
    /// budget: it applies before the connection is matched to a session at all.
    ///
    /// The budget includes writing our own `Logon<A>`, so it must exceed the
    /// round trip a healthy peer needs, not just its think-time. A value too
    /// large for the clock to represent (such as [`Duration::MAX`]) means no
    /// limit - and a peer that never answers then parks the session for good.
    ///
    /// Also bounds acknowledgement of an in-session reset; see
    /// [reset time limits](crate::session_reset#time-limits).
    ///
    /// [`DisconnectReason::LogonTimeout`]: crate::DisconnectReason::LogonTimeout
    /// [`auto_disconnect_after_no_logon_received`]: crate::AcceptorSettings::auto_disconnect_after_no_logon_received
    //
    // Not optional, and not derived from `heartbeat_interval`: with
    // `heartbeat_interval: None` every other deadline in the session loop is
    // unarmed, so a peer that accepts the TCP connection and then says nothing
    // would park the session task forever - holding the session's storage, with
    // no `on_session_end` for the application to observe.
    pub auto_disconnect_after_no_logon_response: Duration,

    /// Maximum duration of a single TCP write. A write that does not
    /// complete within this budget (peer not reading, send buffer full)
    /// marks the connection dead and the session disconnects.
    ///
    /// A transport-level knob, independent from the negotiated
    /// `HeartBtInt<108>`.
    //
    // Deliberately so: the peer proposes the heartbeat interval, but how long
    // the operator tolerates a stalled write is a local deployment decision.
    pub write_timeout: Duration,

    /// Largest FIX message this session sends or accepts, in bytes. The
    /// Acceptor and Initiator pass it into the storage builder closure, so
    /// the storage's slots match it without manual wiring.
    ///
    /// Announced to the counterparty as `MaxMessageSize<383>` whenever the
    /// dictionary's `Logon<A>` carries that field, and enforced on inbound
    /// messages either way: one declaring more in `BodyLength(9)` is refused
    /// before its body is read, with a `Logout<5>` naming the sizes in
    /// `Text(58)` and [`DisconnectReason::MessageTooLarge`] (FIX Session
    /// Layer §4.3.6). Before a connection is matched to a session,
    /// [`AcceptorSettings::max_first_message_size`] bounds the first message
    /// instead.
    ///
    /// The peer's own `MaxMessageSize<383>` is not judged by the session: it
    /// reaches the application in the `Logon<A>` passed to
    /// `Application::on_admin_msg_in`, which may answer `InputAction::Logout`
    /// to refuse it, as with [`heartbeat_interval`](Self::heartbeat_interval).
    ///
    /// [`DisconnectReason::MessageTooLarge`]: crate::DisconnectReason::MessageTooLarge
    /// [`AcceptorSettings::max_first_message_size`]: AcceptorSettings::max_first_message_size
    //
    // Because 383 states the *sender's own* receive capacity, a peer
    // advertising more than this limit breaks nothing - it just means the
    // counterparty can take in more than we can. The reverse, a peer
    // advertising less, is the case that matters, and it cannot be enforced
    // message-by-message: refusing a message on the resend path would mean
    // gap-filling over a real application message, i.e. silent data loss,
    // which is worse than sending one the peer may refuse.
    pub max_message_size: NonZeroLength,

    /// If `false`, suppress sending a new `ResendRequest<2>` while an earlier
    /// resend range is still outstanding and the new gap overlaps it. If
    /// `true`, always send a fresh `ResendRequest<2>` for each detected gap.
    pub send_redundant_resend_requests: bool,

    /// If `true`, validate that incoming `SenderCompID<49>` and
    /// `TargetCompID<56>` match the session identity. Mismatches reject the
    /// message with `CompIDProblem` and force a Logout.
    pub check_comp_id: bool,

    /// Maximum allowed difference between message SendingTime(52) and current
    /// time. If `None`, SendingTime is not validated.
    pub max_latency: Option<Duration>,

    /// Fractional-second digits the session puts on `SendingTime<52>`, the one
    /// timestamp it stamps itself - including the fresh tag 52 of a
    /// `PossDupFlag<43>` retransmission.
    ///
    /// Not covered:
    /// - `OrigSendingTime<122>`, which reproduces the retransmitted message's
    ///   original `SendingTime` at whatever width that was stamped with - it
    ///   has to be the same instant that went out the first time;
    /// - timestamps in application message bodies, which the application
    ///   renders itself with
    ///   [`UtcTimestamp::now`](easyfix_core::basic_types::UtcTimestamp::now) or
    ///   [`UtcTimestamp::with_precision`]. Read this setting back from
    ///   [`SessionContext::time_precision`](crate::SessionContext::time_precision)
    ///   to render them at the same width.
    ///
    /// Set it to whatever was agreed with this counterparty; the default,
    /// `Millis`, is the FIX baseline that needs no agreement (TagValue
    /// Encoding section 6.2.2). It is per session, so one process can serve
    /// two counterparties under different agreements.
    ///
    /// [`UtcTimestamp::with_precision`]: easyfix_core::basic_types::UtcTimestamp::with_precision
    pub time_precision: TimePrecision,

    /// Session default application version advertised in
    /// `DefaultApplVerID<1137>` on every outgoing `Logon<A>` (initiator
    /// request and acceptor echo alike).
    ///
    /// Inert when the dictionary's Logon declares no 1137 slot (pre-FIXT
    /// profiles) - the field is then simply not emitted.
    ///
    /// There is no matching "expected peer version" setting: the peer's
    /// 1137 reaches the application inside the Logon passed to
    /// `Application::on_admin_msg_in`, and whether it is acceptable is the
    /// application's decision (answer `InputAction::Logout` to refuse) -
    /// the same policy split as documented on
    /// [`heartbeat_interval`](Self::heartbeat_interval).
    //
    // A library-level check of 1137 alone would be wrong anyway: the session
    // default is the lowest-precedence version layer, superseded by
    // per-message-type defaults (MsgTypeGrp) and per-instance ApplVerID<1128>
    // (FIX Transport 4.1.1).
    pub sender_default_appl_ver_id: ApplVerId,

    /// Enable the next expected message sequence number (optional tag 789
    /// on Logon).
    pub enable_next_expected_msg_seq_num: bool,

    /// If `true`, run full header verification on incoming `Logout<5>`
    /// messages (CompID, SendingTime, sequence number). If `false`, accept
    /// `Logout<5>` without verification, except while a local reset awaits
    /// confirmation: a refusing Logout still undergoes header validation,
    /// but its sequence number is not checked against the reset counters.
    pub verify_logout: bool,

    /// Which inbound `Heartbeat<0>` clears an outstanding `TestRequest<1>`
    /// probe and so resets the idle-session auto-close grace counter.
    ///
    /// When enabled, only a Heartbeat whose `TestReqID(112)` matches an
    /// outstanding probe clears it (FIX Session Layer §4.5.5). When disabled,
    /// any Heartbeat does, per the weaker FIX Transport §5.1 wording - for
    /// peers that do not echo the id back.
    ///
    /// Either way only a Heartbeat clears the probe: the answer "may not be
    /// the next message received" (Test Cases §4.5.5 Scenario 6), so
    /// intervening traffic is no evidence that the peer is responsive.
    /// The probe preceding an in-session reset always requires a matching
    /// `TestReqID(112)`, regardless of this setting.
    pub verify_test_request_id: bool,

    /// If true, engine-produced admin messages are NOT sent to TCP.
    /// The application takes ownership of their delivery and, with history
    /// enabled, must supply their records for recovery. Alternatively, disable
    /// `persist_messages` to gap-fill recovery ranges without an archive.
    /// Admin messages submitted through `Sender` still use the normal send path.
    pub manages_admin_output: bool,

    /// Retain outgoing message history for replay. Defaults to `true`.
    /// When disabled, recovery ranges are gap-filled without reading history
    /// or calling [`Application::should_gap_fill`](crate::Application::should_gap_fill).
    /// Sequence counters are still stored. Unsent messages left at session end
    /// are discarded without numbering or output callbacks.
    pub persist_messages: bool,

    /// Maximum number of outbound messages allowed to sit in the per-session
    /// staging queue awaiting transmission. `None` means unlimited.
    /// When the backlog exceeds this, the
    /// session is hard-disconnected ([`DisconnectReason::SlowConsumer`]) and
    /// the still-queued messages are stored for resend on reconnect when
    /// history is enabled and storage is healthy, otherwise discarded.
    ///
    /// Counts messages, not bytes, and does not limit incoming messages.
    /// Storing the remaining backlog is synchronous and may delay shutdown.
    /// Producers must yield for the cap to be checked; see
    /// [`max_outbound_lag`](Self::max_outbound_lag).
    ///
    /// [`DisconnectReason::SlowConsumer`]: crate::DisconnectReason::SlowConsumer
    //
    // `NonZeroUsize` makes the ">= 1" invariant type-level: a `0` cap would
    // evict at the first staged message.
    pub max_outbound_queued_messages: Option<NonZeroUsize>,

    /// Maximum age of the oldest un-transmitted message before the session is
    /// hard-disconnected ([`DisconnectReason::SlowConsumer`]). `None` =
    /// disabled. Bounds *staleness* - the trading-relevant knob - directly,
    /// rather than via a sequence-number delta. It is suspended during
    /// a peer resend and for one `max_outbound_lag` grace window afterwards, so
    /// a long resend does not evict a healthy peer mid-recovery.
    /// The same exclusion and grace window apply to in-session resets;
    /// the message-count cap remains active.
    ///
    /// Producer-cooperation caveat (applies to **both** outbound caps): both
    /// are evaluated inside the session task, so they fire only when that task
    /// is scheduled. With the synchronous, `!Send` [`Sender::send`], a producer
    /// that fans out in a tight loop with no intervening `.await` starves the
    /// session task - the queue then grows unobserved and neither cap fires.
    /// The caps bound per-session memory only under a *cooperating* producer
    /// (one that yields between bursts); source-side load-shedding driven by
    /// [`Sender::backlog_len`] is the real backstop under global overload.
    ///
    /// [`DisconnectReason::SlowConsumer`]: crate::DisconnectReason::SlowConsumer
    /// [`Sender::send`]: crate::Sender::send
    /// [`Sender::backlog_len`]: crate::Sender::backlog_len
    pub max_outbound_lag: Option<Duration>,

    // Each reset method requires its own bilateral agreement. Keeping both
    // permissions off makes an unconfigured session refuse a destructive reset.
    /// Accept resets on the first Logon of a connection, resetting the
    /// counters and discarding resend history (FIX Session Layer Section 4.4.3).
    /// Acceptor only; ignored by an initiator. Off by default: enable only by
    /// agreement with the peer. When disabled, refuse with Logout and disconnect.
    /// Requires local tag 141 support at registration. See
    /// [session resets](crate::session_reset#choosing-a-method).
    pub accept_reset_on_connect: bool,

    /// Accept resets over an established connection, resetting the counters
    /// and discarding resend history (FIX Session Layer Section 4.4.2).
    /// Applies to either role. Off by default: enable only by agreement with
    /// the peer. When disabled, refuse with Logout and disconnect.
    /// Requires local tag 141 support at construction or registration. See
    /// [session resets](crate::session_reset#reset-over-an-active-connection).
    pub accept_reset_in_session: bool,

    /// Maximum preparation time for a requested in-session reset, counted
    /// from acceptance of the request. Defaults to 30 seconds. Expiry ends
    /// the connection with [`ResetPreparationTimeout`](crate::DisconnectReason::ResetPreparationTimeout)
    /// before resetting the local counters or history.
    ///
    /// A duration the clock cannot represent means no limit. Ongoing IO and
    /// callbacks are not cancelled. The subsequent Logon acknowledgement has
    /// a separate budget; see [reset time limits](crate::session_reset#time-limits).
    pub running_session_reset_timeout: Duration,

    /// Number of stored messages to process per select iteration during resend.
    pub resend_batch_size: NonZeroUsize,

    /// Number of queued out-of-order messages to process per select iteration
    /// after gap recovery.
    pub queued_batch_size: NonZeroUsize,
}

/// Library defaults: 30s heartbeat, 30s write timeout, CompID validation on,
/// peer seq-num reset refused, 4 KiB max message size, no outbound backlog
/// caps.
impl Default for SessionSettings {
    fn default() -> SessionSettings {
        SessionSettings {
            heartbeat_interval: Some(const { NonZeroU16::new(30).unwrap() }),
            auto_disconnect_after_no_heartbeat: const { NonZeroU8::new(1).unwrap() },
            auto_disconnect_after_no_logout: Duration::from_secs(10),
            auto_disconnect_after_no_logon_response: Duration::from_secs(10),
            write_timeout: Duration::from_secs(30),
            max_message_size: const { NonZeroLength::new(4096).unwrap() },
            send_redundant_resend_requests: false,
            check_comp_id: true,
            max_latency: Some(Duration::from_secs(120)),
            time_precision: TimePrecision::Millis,
            sender_default_appl_ver_id: ApplVerId::Fix50Sp2,
            enable_next_expected_msg_seq_num: false,
            verify_logout: true,
            verify_test_request_id: true,
            manages_admin_output: false,
            persist_messages: true,
            max_outbound_queued_messages: None,
            max_outbound_lag: None,
            accept_reset_on_connect: false,
            accept_reset_in_session: false,
            running_session_reset_timeout: Duration::from_secs(30),
            resend_batch_size: const { NonZeroUsize::new(1).unwrap() },
            queued_batch_size: const { NonZeroUsize::new(1).unwrap() },
        }
    }
}
