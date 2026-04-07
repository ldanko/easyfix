//! Synchronous session engine: shared state and coordination.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fmt::Display,
    num::NonZeroU64,
    ops::RangeInclusive,
    time::Instant,
};

use easyfix_core::{
    base_messages::MsgTypeBase,
    basic_types::{FixString, MsgTypeField, NonZeroSeqNum, SeqNum},
    fix_str,
    message::{DeserializeError, SessionMessage},
    serializer::SerializeError,
};
use thiserror::Error;
use tracing::{error, info, warn};

use crate::{
    application::DisconnectReason,
    io::{ControlMsg, time::TimerBackend},
    messages_storage::MessagesStorage,
    session_id::SessionId,
    settings::SessionSettings,
};

mod heartbeat;
mod input;
mod logon;
mod logout;
mod output;
mod recovery;
mod scratch_buffer;
mod validation;

#[cfg(test)]
mod tests;

pub(crate) use logon::supports_seq_num_reset;
pub(crate) use recovery::validate_gap_fill_fits;
use scratch_buffer::ScratchBuffer;
use validation::VerifyError;

/// Outcome of a per-variant handler. The dispatcher converts it to the
/// public [`InputResult`] via [`SessionEngine::apply_result`].
///
/// Handlers don't see `Box<M>` directly - the dispatcher in
/// [`SessionEngine::on_input`] owns the box and applies the side effects
/// that need ownership (queue insert, callback dispatch, disconnect).
#[derive(Debug)]
enum HandlerResult {
    /// Engine handled the message internally; nothing more to do.
    Handled,
    /// Sequence number was too high. Dispatcher enqueues the original
    /// message into the out-of-order queue and sends a ResendRequest
    /// for the gap.
    Enqueue,
    /// Application message - dispatcher returns `InputResult::AppMsg(msg)`.
    AppMsg,
    /// Admin message that needs an `on_admin_msg_in` callback -
    /// dispatcher returns `InputResult::AdminMsg(msg)`.
    AdminMsg,
    /// Engine wants to terminate the session. Dispatcher latches the reason
    /// via [`SessionEngine::begin_disconnect`] and returns
    /// `InputResult::Handled`.
    Disconnect(DisconnectReason),
}

/// Output entry produced by engine methods.
#[derive(Debug)]
pub(crate) enum PendingOutput {
    /// Message serialized into storage slot.
    Stored(NonZeroSeqNum),
    /// Bytes in scratch: a resend, gap fill, or first send without history.
    Transient { len: usize },
}

/// Result of processing an inbound message.
#[derive(Debug)]
pub(crate) enum InputResult<M> {
    /// Engine handled internally, no callback needed.
    Handled,
    /// Application message - call on_app_msg_in.
    AppMsg(Box<M>),
    /// Admin message - call on_admin_msg_in.
    AdminMsg(Box<M>),
    /// Deserialization error - call on_deserialize_error.
    Error(DeserializeError),
}

/// Serialization failure - returned by commit methods.
///
/// The sequence number and `SendingTime` that [`SessionEngine::fill_header`]
/// stamped on `msg` are cleared again (see [`SessionEngine::commit_send`]),
/// so the message can be fixed and staged again without a duplicate number
/// or a stale time. Whatever the producer set itself stays.
#[derive(Debug)]
pub(crate) struct SerializeFailure<M> {
    pub msg: Box<M>,
    pub error: SerializeError,
}

/// The session has latched an unrecoverable storage or replay failure.
#[derive(Debug, Error)]
#[error("fatal storage or replay failure")]
pub(crate) struct FatalError;

#[derive(Debug)]
pub(crate) enum SendFailure<M> {
    Serialize(SerializeFailure<M>),
    Fatal(FatalError),
}

impl<M> From<FatalError> for SendFailure<M> {
    fn from(error: FatalError) -> Self {
        Self::Fatal(error)
    }
}

/// What the last [`SessionEngine::fill_header`] stamped on its message, so a
/// commit that fails can undo exactly that and nothing else.
///
/// `fill_header` and the commit it prepares are always paired in the same
/// synchronous stretch, and `fill_header` overwrites this unconditionally, so
/// the record a commit reads is always the one for its own message.
#[derive(Default)]
struct HeaderFill {
    /// The sequence number allocated from the storage counter - `None` when
    /// the producer numbered the message itself and the counter was not
    /// touched.
    seq_num: Option<NonZeroSeqNum>,
    /// Whether `SendingTime(52)` was stamped (it was `MIN_UTC`).
    sending_time: bool,
}

/// Logon-protocol lifecycle state.
///
/// Termination is tracked separately via `SessionState::disconnect` - that
/// signal is orthogonal to the protocol state and can fire from any variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogonState {
    /// Pre-logon. Acceptor: waiting for inbound Logon. Initiator:
    /// briefly here before [`SessionEngine::send_logon_request`] runs.
    Idle,
    /// Initiator only: Logon sent, awaiting peer's response.
    LogonSent,
    /// Both directions logged on. Normal traffic flows.
    Established,
    /// A running reset was requested; hold application sends while waiting
    /// for recovery and old probes. Shares the budget with `ResetProbe`.
    ResetPending,
    /// A running reset probe is outstanding; application sends are held.
    /// Recovery makes the probe stale; a new one follows completed recovery.
    ResetProbe,
    /// Our reset Logon is outstanding; only its ACK or a refusal is accepted
    /// while the reset remains unconfirmed. Uses the separate Logon-response
    /// budget; application sends and normal keep-alive output stay held.
    ResetSent,
    /// We've sent a Logout and are waiting for the peer's response (or
    /// for [`SessionEngine::logout_deadline`] to fire). `sent_at` drives
    /// the deadline computation.
    LogoutSent { sent_at: Instant },
    /// We've acknowledged the peer's Logout request and are waiting for it
    /// to close the connection (or for [`SessionEngine::awaiting_peer_close`]
    /// to expire). Terminal for the protocol: the IO loop stops feeding input
    /// here, so no handler runs in this state. `sent_at` drives the deadline.
    LogoutAcknowledged { sent_at: Instant },
}

/// Phase of a locally requested running reset, for the IO time budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResetPhase {
    Pending,
    Probe,
    Sent,
}

/// Internal session state.
struct SessionState<M> {
    /// Logon-protocol lifecycle position. See [`LogonState`] for the
    /// per-variant semantics.
    logon_state: LogonState,
    /// A locally initiated reset has not yet been confirmed by the peer.
    /// Cleared only by validation of a fresh, correctly formed reset ACK.
    local_reset_unconfirmed: bool,
    reset_probe_id: Option<FixString>,
    reset_barrier_ids: HashSet<FixString>,
    probe_stale: bool,
    // Never reset with the FIX counters: a late Heartbeat must not match a
    // later probe, even when the clock has not advanced.
    reset_probe_count: u64,
    /// `Some` once the session has decided to end the connection, carrying
    /// why. Read by the IO loop via [`SessionEngine::should_disconnect`];
    /// the TCP socket isn't actually closed until the loop breaks and the
    /// writer is dropped. First writer wins - see
    /// [`SessionEngine::begin_disconnect`].
    disconnect: Option<DisconnectReason>,
    // The public reason may already name an earlier cause or an unconfirmed
    // reset. This latch independently forbids further storage work and output.
    fatal_error: bool,
    /// TestReqIDs of outstanding grace period TestRequests.
    grace_period_test_req_ids: HashSet<FixString>,
    /// Value of tag 789 (NextExpectedMsgSeqNum) from the last Logon we sent.
    /// Used to determine if the peer has enough information to avoid a
    /// ResendRequest. `None` means no tag 789 was sent (the type makes the
    /// old `0` sentinel unrepresentable - a wire `SeqNum` is always >= 1).
    next_expected_msg_seq_num: Option<NonZeroSeqNum>,
    /// Effective heartbeat interval - the single source of truth for both the
    /// output-heartbeat and input-timeout (TestRequest) deadlines. `Some(n)` is
    /// `n` seconds; `None` means heartbeats are disabled (negotiated
    /// `HeartBtInt=0`, FIX Transport §5.1). Seeded from the settings value in
    /// [`SessionEngine::new`] and overwritten on the acceptor path with the
    /// initiator's adopted `HeartBtInt`.
    //
    // Wider than the settings field: the peer's value is any non-negative
    // `Int`, adopted as-is (no upper bound, see `check_heart_bt_int`), and
    // `u64` is what `Duration::from_secs` takes.
    heartbeat_interval: Option<NonZeroU64>,
    /// Range of sequence numbers we've requested the peer to resend
    /// (via ResendRequest). Used for suppression of redundant requests.
    resend_range: Option<RangeInclusive<SeqNum>>,
    /// Messages received out of sequence, held until the gap is filled.
    /// Keyed by MsgSeqNum for retrieval by sequence number.
    queue: BTreeMap<SeqNum, Box<M>>,
    /// Header stamps of the message most recently passed through
    /// [`SessionEngine::fill_header`], awaiting its commit.
    last_fill: HeaderFill,
    /// When this session began, driving [`SessionEngine::logon_deadline`].
    //
    // The whole handshake is measured from here, the write of our own
    // `Logon<A>` included, rather than from the moment that write completes:
    // one anchor covers both roles, and `LogonState` never returns to a
    // pre-handshake variant, so it cannot go stale.
    started_at: Instant,
}

impl<M> SessionState<M> {
    fn new(started_at: Instant) -> Self {
        SessionState {
            logon_state: LogonState::Idle,
            local_reset_unconfirmed: false,
            reset_probe_id: None,
            reset_barrier_ids: HashSet::new(),
            probe_stale: false,
            reset_probe_count: 0,
            disconnect: None,
            fatal_error: false,
            grace_period_test_req_ids: HashSet::new(),
            next_expected_msg_seq_num: None,
            heartbeat_interval: None,
            resend_range: None,
            queue: BTreeMap::new(),
            last_fill: HeaderFill::default(),
            started_at,
        }
    }
}

/// Synchronous FIX session protocol engine.
///
/// Pure protocol logic with `&mut self` methods - no async, no `Rc`, no
/// channels. The IO loop drives this engine and handles async operations
/// (TCP I/O, handler callbacks, timers).
pub(crate) struct SessionEngine<M> {
    /// Internal protocol state.
    state: SessionState<M>,
    /// Session identity (SenderCompID / TargetCompID pair).
    session_id: SessionId,
    /// Static configuration.
    session_settings: SessionSettings,
    timer_backend: TimerBackend,
    /// Serialized messages ready for TCP write.
    output: VecDeque<PendingOutput>,
    /// Admin messages awaiting on_admin_msg_out callback.
    admin_output: VecDeque<Box<M>>,
    /// Pending resend ranges from incoming ResendRequests and from implicit
    /// requests via Logon NextExpectedMsgSeqNum(789).
    pending_resends: VecDeque<RangeInclusive<SeqNum>>,
    /// Current accumulating gap-fill range (begin, end) for resend processing.
    gap_fill_range: Option<(SeqNum, SeqNum)>,
    /// Scratch buffer for transient message serialization. Encapsulates
    /// the shared-buffer aliasing invariant - see [`ScratchBuffer`].
    scratch: ScratchBuffer,
}

impl<M: SessionMessage> SessionEngine<M> {
    pub(crate) fn new(
        session_id: SessionId,
        session_settings: SessionSettings,
        timer_backend: TimerBackend,
    ) -> Self {
        let max_message_size = usize::from(session_settings.max_message_size.get());
        let mut state = SessionState::new(timer_backend.now());
        // Seed the effective heartbeat interval from settings; the acceptor
        // overwrites it with the initiator's adopted value during Logon.
        state.heartbeat_interval = session_settings.heartbeat_interval.map(NonZeroU64::from);
        SessionEngine {
            state,
            session_id,
            session_settings,
            timer_backend,
            output: VecDeque::new(),
            admin_output: VecDeque::new(),
            pending_resends: VecDeque::new(),
            gap_fill_range: None,
            scratch: ScratchBuffer::new(max_message_size),
        }
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub(crate) fn session_settings(&self) -> &SessionSettings {
        &self.session_settings
    }

    pub(crate) fn timer_backend(&self) -> TimerBackend {
        self.timer_backend
    }

    pub(crate) fn is_logged_on(&self) -> bool {
        matches!(
            self.state.logon_state,
            LogonState::Established
                | LogonState::ResetPending
                | LogonState::ResetProbe
                | LogonState::ResetSent
                | LogonState::LogoutSent { .. }
        )
    }

    /// Gate both normal sends and the terminating drain. Being logged on alone
    /// is insufficient while a running reset or its unconfirmed Logout waits.
    pub(crate) fn accepts_app_sends(&self) -> bool {
        self.state.logon_state == LogonState::Established
            || (matches!(self.state.logon_state, LogonState::LogoutSent { .. })
                && !self.state.local_reset_unconfirmed)
    }

    pub(crate) fn reset_phase(&self) -> Option<ResetPhase> {
        match self.state.logon_state {
            LogonState::ResetPending => Some(ResetPhase::Pending),
            LogonState::ResetProbe => Some(ResetPhase::Probe),
            LogonState::ResetSent => Some(ResetPhase::Sent),
            _ => None,
        }
    }

    /// Check both directions' recovery and the strict pre-reset probe barrier.
    /// The IO caller supplies whether its active replay and trailing gap are
    /// finished; pending engine replay and parked input must be empty too.
    pub(crate) fn reset_ready<S: MessagesStorage>(
        &self,
        active_resend_none: bool,
        storage: &S,
    ) -> bool {
        !self.has_fatal_error()
            && self.state.logon_state == LogonState::ResetPending
            && active_resend_none
            && !self.has_pending_resends()
            && self.inbound_recovery_finished(storage)
            && self.state.queue.is_empty()
            && self.state.reset_barrier_ids.is_empty()
    }

    /// Start a new probe after the caller has checked the readiness barrier.
    pub(crate) fn start_reset_probe(&mut self) {
        if self.state.logon_state != LogonState::ResetPending {
            return;
        }
        let Some(count) = self.state.reset_probe_count.checked_add(1) else {
            error!("reset probe identifiers exhausted");
            self.begin_disconnect(DisconnectReason::InvalidLogonState);
            return;
        };
        self.state.reset_probe_count = count;
        let id = FixString::from_ascii_lossy(format!("RESET-{count}").into_bytes());
        self.state.reset_probe_id = Some(id.clone());
        self.state.probe_stale = false;
        self.send_test_request(id);
        self.state.logon_state = LogonState::ResetProbe;
    }

    /// Mark an outstanding probe stale before doing replay or queued work.
    pub(crate) fn mark_probe_stale(&mut self) {
        if self.state.logon_state == LogonState::ResetProbe {
            self.state.probe_stale = true;
        }
    }

    /// Whether the session has decided to end the connection; the reason is
    /// in [`Self::disconnect_reason`].
    pub(crate) fn should_disconnect(&self) -> bool {
        self.state.disconnect.is_some()
    }

    /// Whether the IO loop should stop dispatching: the connection is ending
    /// ([`Self::should_disconnect`]), or the peer's Logout has been
    /// acknowledged and only its close remains
    /// ([`Self::awaiting_peer_close`]).
    pub(crate) fn should_leave_loop(&self) -> bool {
        self.should_disconnect() || self.awaiting_peer_close().is_some()
    }

    /// Why the session is ending - `None` while it is not.
    pub(crate) fn disconnect_reason(&self) -> Option<DisconnectReason> {
        self.state.disconnect
    }

    /// Decide to end the connection, recording why. A reason already recorded
    /// stands: the first call is what starts the teardown, and later ones
    /// describe consequences of it. While our local reset is unconfirmed,
    /// the first reason becomes `SeqNumResetFailed`.
    //
    // First writer wins because a later caller is describing a consequence,
    // not a cause, and saying so would mislead: a grace-period timeout stages
    // its farewell `Logout<5>` and latches `HeartbeatTimeout`, and if stamping
    // that Logout happens to consume the last sequence number,
    // `SeqNumExhausted` would displace the timeout that actually ended the
    // session. Likewise a write failure while flushing an application's
    // farewell Logout does not turn `ApplicationForcedDisconnect` into `IoError`.
    // The caller's specific cause remains in the log when an unconfirmed
    // local reset normalizes the public reason; callers need the recovery
    // obligation more than another transport or protocol symptom.
    pub(crate) fn begin_disconnect(&mut self, reason: DisconnectReason) {
        if self.state.disconnect.is_none() {
            self.state.disconnect = Some(if self.state.local_reset_unconfirmed {
                error!(
                    ?reason,
                    "connection ended before the peer confirmed our sequence number reset"
                );
                DisconnectReason::SeqNumResetFailed
            } else {
                reason
            });
        }
    }

    /// Set the engine into a logged-on state. Used by test helpers to
    /// construct an engine that skips the logon handshake.
    #[cfg(test)]
    pub(crate) fn set_logged_on(&mut self) {
        self.state.logon_state = LogonState::Established;
    }

    /// Mutable access to session settings. Used by tests to adjust timeouts.
    #[cfg(test)]
    pub(crate) fn session_settings_mut(&mut self) -> &mut SessionSettings {
        &mut self.session_settings
    }

    pub(crate) fn has_fatal_error(&self) -> bool {
        self.state.fatal_error
    }

    fn ensure_healthy(&self) -> Result<(), FatalError> {
        if self.has_fatal_error() {
            Err(FatalError)
        } else {
            Ok(())
        }
    }

    pub(crate) fn fail_storage(
        &mut self,
        operation: &'static str,
        error: &impl Display,
    ) -> FatalError {
        error!(operation, %error, "Fatal storage or replay failure");
        self.state.fatal_error = true;
        self.begin_disconnect(DisconnectReason::StorageError);
        FatalError
    }

    /// Consume the incoming sequence number just processed.
    // The dispatch tail handles reaching MAX, including via SequenceReset.
    fn advance_target<S: MessagesStorage>(&mut self, storage: &mut S) -> Result<(), FatalError> {
        self.ensure_healthy()?;
        let seq_num = storage.next_target_msg_seq_num();
        if let Some(next) = seq_num.checked_add(1) {
            storage.set_next_target_msg_seq_num(next).map_err(|error| {
                error!(%seq_num, %next, "Failed to advance incoming sequence number");
                self.fail_storage("set_next_target_msg_seq_num", &error)
            })?;
        }
        Ok(())
    }

    fn reset_storage<S: MessagesStorage>(&mut self, storage: &mut S) -> Result<(), FatalError> {
        self.ensure_healthy()?;
        storage
            .reset()
            .map_err(|error| self.fail_storage("reset", &error))
    }

    /// Consume the seq num of an in-sequence message whose protocol handler
    /// does not run - because the session rejects it or the application
    /// refuses it. SequenceReset does not consume its own number (Session
    /// Layer 4.8.8; Test Cases 11(c)). The caller must skip this for a
    /// silently refused initial Logon request.
    fn consume_seq_num<S: MessagesStorage>(
        &mut self,
        msg_type: MsgTypeField,
        seq_num: SeqNum,
        storage: &mut S,
    ) -> Result<(), FatalError> {
        self.ensure_healthy()?;

        if msg_type != MsgTypeBase::SequenceReset
            && seq_num == storage.next_target_msg_seq_num().get()
        {
            self.advance_target(storage)?;
        }
        Ok(())
    }

    /// End a running reset whose current phase exceeded its time budget.
    pub(crate) fn on_reset_timeout(&mut self) {
        let reason = match self.reset_phase() {
            Some(ResetPhase::Pending | ResetPhase::Probe) => {
                DisconnectReason::ResetPreparationTimeout
            }
            Some(ResetPhase::Sent) => DisconnectReason::LogonTimeout,
            None => return,
        };
        self.push_logout(
            None,
            Some(fix_str!("Sequence number reset not acknowledged").to_owned()),
        );
        self.begin_disconnect(reason);
    }

    pub(crate) fn on_control(&mut self, msg: ControlMsg) {
        match msg {
            ControlMsg::ResetRunningSession => {
                if self.state.logon_state == LogonState::Established && !self.should_disconnect() {
                    self.state.reset_barrier_ids = self.state.grace_period_test_req_ids.clone();
                    self.state.probe_stale = false;
                    self.state.logon_state = LogonState::ResetPending;
                } else {
                    warn!("running reset requested outside established state, ignoring");
                }
            }
            ControlMsg::Logout {
                session_status,
                text,
            } => {
                if matches!(
                    self.state.logon_state,
                    LogonState::LogoutSent { .. } | LogonState::LogoutAcknowledged { .. }
                ) {
                    // Preserve the first Logout and its original deadline.
                    info!("logout requested while already logging out, ignoring");
                } else {
                    self.send_logout(session_status, text);
                }
            }
            ControlMsg::Disconnect => {
                self.begin_disconnect(DisconnectReason::Disconnected);
            }
        }
    }

    /// End the session when the incoming counter reaches `SeqNum::MAX`.
    ///
    /// Called once per processed input, after the protocol handlers have run.
    /// A `Logout<5>` goes out unless one already has - the session then ends
    /// with a `Text(58)` saying why, rather than a bare transport close.
    //
    // Deliberately not inlined at the ten sites that advance the counter.
    // Three of them emit a Logout of their own in the same call - the peer
    // would see "Logout, Reject, Logout" - and a fourth runs after the Logon
    // acknowledgement. One reaction per input keeps the wire honest.
    //
    // The `LogoutSent` arm is load-bearing, not belt-and-braces: a Logout can
    // be staged without latching `should_disconnect` at all - `ControlMsg::
    // Logout` and `InputAction::Logout { disconnect: false }` both do - so the
    // early return above does not cover it. An application asking for a
    // graceful logout, followed by a peer message that uses up the incoming
    // numbering, is exactly the case that arm catches.
    //
    // The state is deliberately NOT narrowed to `Established`. `check_poss_dup`
    // yields the one `VerifyError::Reject` on the Logon path that carries no
    // disconnect of its own, and it advances the counter on the way out, so it
    // can be the message that uses up the numbering - leaving this method as
    // the only thing left to end a handshake that never completed. Test Cases
    // §4.4.1 Scenario 1S(d) step 3 (and §4.3.1 Scenario 1B(d) step 3 for the
    // initiator) makes the Logout mandatory there, unlike the Reject in step 2
    // which is marked optional.
    //
    // §4.3.1's bare-drop mandate is not weakened by that: a non-Logon first
    // message never reaches here, because `check_logon_state` turns it into an
    // `InvalidLogonState` disconnect that latches above. Neither do Scenario
    // 1S(b)/(c) - duplicate or unauthenticated identity is settled before the
    // engine sees a message, which is what keeps their "would consume a
    // MsgSeqNum" warning satisfied.
    //
    // The counter itself preserves exhaustion across reconnects. This also
    // handles SequenceReset raising NextNumIn directly to MAX.
    pub(crate) fn end_session_if_target_numbering_exhausted<S: MessagesStorage>(
        &mut self,
        storage: &S,
    ) {
        if self.should_disconnect() || storage.next_target_msg_seq_num().get() < SeqNum::MAX {
            return;
        }
        // The peer's Logout took the last number and is already answered; the
        // connection is now the peer's to close. The exhausted counter still
        // prevents resuming the old numbering on the next connection.
        if let LogonState::LogoutAcknowledged { .. } = self.state.logon_state {
            return;
        }
        error!("Incoming sequence numbers exhausted; a sequence number reset is required");
        if !matches!(self.state.logon_state, LogonState::LogoutSent { .. }) {
            // `push_logout`, not `send_logout`: the connection ends in this
            // same iteration, so there is no response to wait for, and
            // entering `LogoutSent` from `LogonSent` would open the loop's
            // app-send drain on an unacknowledged handshake (§4.3.10).
            self.push_logout(
                None,
                Some(fix_str!("Incoming sequence numbers exhausted").to_owned()),
            );
        }
        self.begin_disconnect(DisconnectReason::SeqNumExhausted);
    }

    /// Stop an unconfirmed reset if a completed input used up the number
    /// reserved for its acknowledgement. Call only after the callback and
    /// its decision have been fully applied.
    pub(crate) fn end_session_if_reset_ack_number_consumed<S: MessagesStorage>(
        &mut self,
        storage: &S,
    ) {
        if self.should_disconnect()
            || !self.state.local_reset_unconfirmed
            || !matches!(
                self.state.logon_state,
                LogonState::LogonSent | LogonState::ResetSent | LogonState::LogoutSent { .. }
            )
            || storage.next_target_msg_seq_num().get() == 1
        {
            return;
        }
        if !self
            .admin_output
            .iter()
            .any(|msg| msg.msg_type() == MsgTypeBase::Logout)
        {
            self.push_logout(
                None,
                Some(
                    fix_str!("Sequence number expected for reset acknowledgement already consumed")
                        .to_owned(),
                ),
            );
        }
        self.begin_disconnect(DisconnectReason::InvalidLogonState);
    }
}
