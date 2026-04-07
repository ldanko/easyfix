//! Logon exchange, parameter negotiation, and reset acknowledgement.

use std::{num::NonZeroU64, time::Instant};

use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, HeaderBase, LogonBase, MsgTypeBase, SessionRejectReasonBase,
        SessionStatusBase,
    },
    basic_types::{FixString, Int, Length, MsgTypeField, NonZeroSeqNum, SeqNum, TagNum},
    fix_str,
    message::SessionMessage,
};
use tracing::{error, info, warn};

use super::{FatalError, HandlerResult, LogonState, SessionEngine};
use crate::{
    application::{DisconnectReason, invalid_heart_bt_int_text},
    initiator::SessionStart,
    messages_storage::MessagesStorage,
    settings::SessionSettings,
};

const TAG_HEART_BT_INT: TagNum = 108;

/// Whether a locally constructed reset Logon survives conversion into `M`.
pub(crate) fn supports_seq_num_reset<M: SessionMessage>(settings: &SessionSettings) -> bool {
    // This is a local conversion probe only: no serialization, callbacks,
    // storage, numbering, or output queues. An omitted optional input cannot
    // distinguish a missing dictionary field from a supported unset field.
    let logon = M::from_admin(
        HeaderBase::default(),
        AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: EncryptMethodBase::None as Int,
            heart_bt_int: settings
                .heartbeat_interval
                .map_or(0, |secs| Int::from(secs.get())),
            reset_seq_num_flag: Some(true),
            max_message_size: Some(settings.max_message_size.get()),
            next_expected_msg_seq_num: settings.enable_next_expected_msg_seq_num.then_some(1),
            default_appl_ver_id: Some(settings.sender_default_appl_ver_id),
            session_status: None,
        }),
    );
    matches!(logon.try_as_admin(), Some(AdminBase::Logon(logon)) if logon.reset_seq_num_flag == Some(true))
}

impl<M: SessionMessage> SessionEngine<M> {
    /// When the session gives up on a handshake that has not completed. `None`
    /// once it has.
    //
    // Covers `Idle` as well as `LogonSent`, and not only for the acceptor
    // waiting on a first `Logon<A>`: an application that answers that Logon
    // with `InputAction::Reject` leaves the engine in `Idle` without setting
    // `should_disconnect`, so the session enters the IO loop still
    // pre-handshake. Every other deadline there is either heartbeat-derived
    // (and held back until the session is established) or needs a `Logout<5>`
    // we have not sent, so without this one a peer that then goes silent parks
    // the session task forever.
    pub(crate) fn logon_deadline(&self) -> Option<Instant> {
        matches!(
            self.state.logon_state,
            LogonState::Idle | LogonState::LogonSent
        )
        .then(|| {
            self.state.started_at.checked_add(
                self.session_settings
                    .auto_disconnect_after_no_logon_response,
            )
        })
        .flatten()
    }

    /// The `MaxMessageSize<383>` value stated on every outgoing `Logon<A>`
    /// (FIX Session Layer §4.3.6) - the session's own limit. Always
    /// `Some`: whether the field reaches the wire is the dictionary's
    /// call, not a setting (pre-FIX 4.2 Logons declare no 383 slot and
    /// the conversion drops it).
    fn advertised_max_message_size(&self) -> Option<Length> {
        Some(self.session_settings.max_message_size.get())
    }

    /// Push a Logon request to admin_output (initiator path).
    ///
    /// When `enable_next_expected_msg_seq_num` is set, tag 789 is derived
    /// from `storage.next_target_msg_seq_num()` - the next seq num we
    /// expect from the peer (>= 1 for any session, 1 on a fresh one) - and
    /// recorded back into `state.next_expected_msg_seq_num` so a later
    /// too-high Logon response can suppress the redundant ResendRequest.
    /// Reading the past-tense state field here instead would omit tag 789
    /// on a first Logon - the field stays `None` until a Logon has been sent.
    ///
    /// Tag 789 is omitted when the incoming counter has reached its limit.
    ///
    /// [`SessionStart::Reset`] discards stored numbering and history before
    /// staging the Logon, which announces the reset with tag 141.
    pub(crate) fn send_logon_request<S: MessagesStorage>(
        &mut self,
        storage: &mut S,
        start: SessionStart,
    ) -> Result<(), FatalError> {
        self.ensure_healthy()?;

        if start == SessionStart::Reset {
            // No old-numbered admin reply may survive across storage.reset().
            // The IO caller drains output before dispatch can renumber it.
            debug_assert!(self.admin_output.is_empty());
            if !self.admin_output.is_empty() {
                error!("sequence number reset reached with unflushed admin output");
            }
            self.reset_storage(storage)?;
            self.discard_recovery_state();
            self.state.local_reset_unconfirmed = true;
        }
        let next_expected_msg_seq_num = if self.session_settings.enable_next_expected_msg_seq_num
            && storage.next_target_msg_seq_num().get() < SeqNum::MAX
        {
            let next_target = storage.next_target_msg_seq_num();
            self.state.next_expected_msg_seq_num = Some(next_target);
            Some(next_target.get())
        } else {
            None
        };

        self.push_admin(AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: EncryptMethodBase::None as Int,
            // `None` configured interval means no heartbeats - propose
            // HeartBtInt=0 (FIX Transport §5.1).
            heart_bt_int: self
                .session_settings
                .heartbeat_interval
                .map_or(0, |secs| Int::from(secs.get())),
            reset_seq_num_flag: (start == SessionStart::Reset).then_some(true),
            max_message_size: self.advertised_max_message_size(),
            next_expected_msg_seq_num,
            default_appl_ver_id: Some(self.session_settings.sender_default_appl_ver_id),
            session_status: None,
        }));

        self.state.logon_state = LogonState::LogonSent;
        Ok(())
    }

    /// Push a Logon response to admin_output (acceptor path).
    ///
    /// State transitions are handled by [`Self::process_logon`]; this method
    /// only pushes the message.
    pub(super) fn send_logon_response(
        &mut self,
        heart_bt_int: Int,
        reset_seq_num_flag: bool,
        next_expected_msg_seq_num: Option<SeqNum>,
    ) {
        self.push_admin(AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: EncryptMethodBase::None as Int,
            heart_bt_int,
            reset_seq_num_flag: reset_seq_num_flag.then_some(true),
            max_message_size: self.advertised_max_message_size(),
            next_expected_msg_seq_num,
            default_appl_ver_id: Some(self.session_settings.sender_default_appl_ver_id),
            session_status: None,
        }));
    }

    /// Validate the header, reset permission, acknowledgement shape and
    /// encryption method before application input. A valid ACK confirms our
    /// local reset here; body-value checks and the application's decision
    /// follow without a second storage reset. Retransmitted resets only
    /// participate in recovery.
    pub(super) fn on_logon<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        logon: LogonBase,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let reset_seq_num_flag = logon.reset_seq_num_flag.unwrap_or(false);

        // The peer's MaxMessageSize(383) is deliberately not judged here -
        // see `SessionSettings::max_message_size`. It reaches the
        // application inside the Logon, ahead of the state transition in
        // `process_logon`, so refusing it is `InputAction::Logout` away.

        // Verify the header before judging reset permission and ACK fields.
        if let Some(hr) = self.validate_logon(header, storage, reset_seq_num_flag)? {
            return Ok(hr);
        }

        if self.state.local_reset_unconfirmed {
            if header.poss_dup_flag.unwrap_or(false) {
                self.consume_seq_num(MsgTypeBase::Logon.into(), header.msg_seq_num, storage)?;
                self.push_logout(
                    None,
                    Some(
                        fix_str!("Retransmitted Logon cannot acknowledge sequence number reset")
                            .to_owned(),
                    ),
                );
                return Ok(HandlerResult::Disconnect(
                    DisconnectReason::InvalidLogonState,
                ));
            }
            if !reset_seq_num_flag || header.msg_seq_num != 1 {
                self.push_logout(None, Some(fix_str!("Sequence number reset acknowledgement requires ResetSeqNumFlag=Y and MsgSeqNum=1").to_owned()));
                return Ok(HandlerResult::Disconnect(
                    DisconnectReason::InvalidLogonState,
                ));
            }
            // A too-high 789 contradicts the reset acknowledgement itself.
            // Zero is judged later as an invalid body value after confirmation.
            if self.session_settings.enable_next_expected_msg_seq_num
                && logon
                    .next_expected_msg_seq_num
                    .is_some_and(|seq| seq > storage.next_sender_msg_seq_num().get())
            {
                return Ok(self
                    .check_logon_next_expected_range(
                        logon.next_expected_msg_seq_num,
                        storage.next_sender_msg_seq_num().get(),
                    )
                    .unwrap_or(HandlerResult::Disconnect(
                        DisconnectReason::InvalidLogonState,
                    )));
            }
            if !self.state.queue.is_empty() {
                error!("discarding queued input after peer confirmed our sequence number reset");
            }
            self.state.queue.clear();
            self.state.resend_range = None;
            self.state.local_reset_unconfirmed = false;
            return self.check_encrypt_method(
                header.msg_seq_num,
                logon.encrypt_method_raw,
                storage,
            );
        }

        // A retransmitted reset carries old numbering, not permission to
        // discard the current session again. Only its
        // sequence number participates in recovery; its body is not applied.
        if reset_seq_num_flag && self.answers_logon() && header.poss_dup_flag.unwrap_or(false) {
            let next_target = storage.next_target_msg_seq_num().get();
            if next_target == SeqNum::MAX
                || Self::check_seq_num_too_low(header, next_target).is_err()
            {
                return Ok(HandlerResult::Handled);
            }
            if header.msg_seq_num > next_target {
                return Ok(HandlerResult::Enqueue);
            }
            self.consume_seq_num(MsgTypeBase::Logon.into(), header.msg_seq_num, storage)?;
            return Ok(HandlerResult::Handled);
        }

        if reset_seq_num_flag {
            let text = if header.msg_seq_num != 1 {
                Some(FixString::from_ascii_lossy(
                    format!(
                        "ResetSeqNumFlag=Y requires MsgSeqNum=1, got {}",
                        header.msg_seq_num
                    )
                    .into_bytes(),
                ))
            } else if matches!(self.state.logon_state, LogonState::LogonSent) {
                Some(fix_str!("Unsolicited ResetSeqNumFlag=Y in Logon response").to_owned())
            } else if matches!(self.state.logon_state, LogonState::Idle)
                && !self.session_settings.accept_reset_on_connect
            {
                Some(fix_str!("Resetting the sequence number upon FIX connection establishment is not supported").to_owned())
            } else if matches!(
                self.state.logon_state,
                LogonState::Established | LogonState::ResetPending | LogonState::ResetProbe
            ) && !self.session_settings.accept_reset_in_session
            {
                Some(fix_str!("Resetting the sequence number is not supported").to_owned())
            } else {
                None
            };
            if let Some(text) = text {
                error!("{text}");
                self.push_logout(None, Some(text));
                return Ok(HandlerResult::Disconnect(
                    DisconnectReason::InvalidLogonState,
                ));
            }
        }

        self.check_encrypt_method(header.msg_seq_num, logon.encrypt_method_raw, storage)
    }

    /// Whether the session answers an incoming Logon with an acknowledgement
    /// of its own, as opposed to reading the answer to a Logon it sent.
    //
    // We owe an acknowledgement for every Logon we did not ask for: the
    // initial one on the acceptor (Idle) and a mid-session reset from either
    // peer (Established). The connection role does not decide it - §4.4.2
    // has the counterparties agree which side initiates the daily reset, and
    // "the peer receiving the Logon(35=A) message ... should send a
    // Logon(35=A) acknowledgement". LogonSent and ResetSent await an answer
    // to our own Logon; in LogoutSent the session is going down and must not
    // be revived with an ACK.
    pub(super) fn answers_logon(&self) -> bool {
        matches!(
            self.state.logon_state,
            LogonState::Idle
                | LogonState::Established
                | LogonState::ResetPending
                | LogonState::ResetProbe
        )
    }

    /// Apply an application-accepted Logon. Check HeartBtInt and the peer's
    /// tag 789 before a requested reset can discard storage, then acknowledge
    /// a peer request or complete our exchange. In LogoutSent, preserve the
    /// existing state and deadline instead of reviving the connection.
    pub(super) fn process_logon<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        logon: LogonBase,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let msg_seq_num = header.msg_seq_num;

        let reset_seq_num_flag = logon.reset_seq_num_flag.unwrap_or(false);
        let heart_bt_int = logon.heart_bt_int;
        let next_expected_msg_seq_num = logon.next_expected_msg_seq_num;

        // Decide against the state that received this Logon, before any
        // successful handshake transitions it to Established.
        let acknowledge = self.answers_logon();
        let first_logon = matches!(self.state.logon_state, LogonState::Idle);

        // Judge the offered HeartBtInt before the reset below touches the
        // storage: an invalid Logon must not reset it. A Reject still
        // consumes the expected incoming sequence number.
        let heart_bt_secs = match self.check_heart_bt_int(msg_seq_num, heart_bt_int, storage)? {
            Ok(secs) => secs,
            Err(hr) => return Ok(hr),
        };

        let expected_heart_bt_secs = self.state.heartbeat_interval.map_or(0, NonZeroU64::get);
        if !first_logon && heart_bt_secs != expected_heart_bt_secs {
            let text = if acknowledge {
                self.state.heartbeat_interval.map_or_else(
                    || fix_str!("Invalid HeartBtInt(108)").to_owned(),
                    invalid_heart_bt_int_text,
                )
            } else {
                FixString::from_ascii_lossy(format!(
                    "HeartBtInt(108) not echoed: expected {expected_heart_bt_secs}, got {heart_bt_secs}"
                ).into_bytes())
            };
            error!("{text}");
            self.push_logout(None, Some(text));
            return Ok(HandlerResult::Disconnect(
                DisconnectReason::InvalidLogonState,
            ));
        }

        let peer_reset = reset_seq_num_flag && acknowledge;
        if peer_reset
            && self.session_settings.enable_next_expected_msg_seq_num
            && let Some(hr) = self.check_logon_next_expected_range(next_expected_msg_seq_num, 1)
        {
            return Ok(hr);
        }
        let did_reset = peer_reset;

        // 1. Apply the peer's sequence reset before reading final counters.
        if did_reset {
            // Queued admin responses must be flushed before an input can
            // renumber the store they will be committed to.
            debug_assert!(self.admin_output.is_empty());
            if !self.admin_output.is_empty() {
                error!("sequence number reset reached with unflushed admin output");
            }
            info!("Resetting sequence numbers on logon");
            self.reset_storage(storage)?;
            self.discard_recovery_state();
        }

        // 2. Transition to Established. If a later phase fails with
        // Disconnect, `should_disconnect` overrides the protocol state.
        if !matches!(self.state.logon_state, LogonState::LogoutSent { .. }) {
            self.state.logon_state = LogonState::Established;
        }
        self.state.reset_probe_id = None;
        self.state.reset_barrier_ids.clear();
        self.state.probe_stale = false;

        // Whether tag 789 is in play at all: the setting, plus the peer
        // having offered one (we mirror its capability rather than impose
        // ours).
        let enable_next_expected = self.session_settings.enable_next_expected_msg_seq_num
            && next_expected_msg_seq_num.is_some();

        // 3. Validate tag 789 if present and enabled (range check)
        if enable_next_expected
            && !peer_reset
            && let Some(hr) = self.check_logon_next_expected_range(
                next_expected_msg_seq_num,
                storage.next_sender_msg_seq_num().get(),
            )
        {
            return Ok(hr);
        }

        let next_sender_at_logon = storage.next_sender_msg_seq_num().get();

        // Whether the Logon itself is in sequence. Decided here, ahead of the
        // ACK, because the ACK's tag 789 counts the Logon only when it is.
        // An accepted peer reset starts numbering at its Logon.
        let next_target = storage.next_target_msg_seq_num().get();
        let is_normal = msg_seq_num <= next_target || did_reset;

        // 4. Acknowledge a Logon we did not ask for. Adopt HeartBtInt only on
        // the first Logon; running resets echo the validated effective value.
        if acknowledge {
            if first_logon {
                self.state.heartbeat_interval = NonZeroU64::new(heart_bt_secs);
            }
            self.send_logon_ack(
                heart_bt_int,
                did_reset,
                is_normal,
                enable_next_expected,
                storage,
            );
        } else {
            info!("Received logon response");
        }

        // 5. Handle out-of-sequence logon (too high)
        if !acknowledge && is_normal {
            self.state.next_expected_msg_seq_num = None;
        }
        if !is_normal {
            if let Some(next_expected) = self.state.next_expected_msg_seq_num {
                // Tag 789 was sent - set resend range to suppress the
                // explicit ResendRequest that `apply_result` would otherwise
                // dispatch on `HandlerResult::Enqueue`. `request_resend` reads
                // `state.resend_range` and short-circuits (unless
                // `send_redundant_resend_requests` is configured). The range
                // must be FINITE - the implied resend covers the gap below
                // this Logon, and an open-ended range would keep suppressing
                // requests for later gaps after this one is recovered.
                self.state.resend_range = Some(next_expected.get()..=msg_seq_num.saturating_sub(1));
                self.state.next_expected_msg_seq_num = None;
                info!("Required resend will be suppressed as we are setting tag 789");
            }
            warn!("Target MsgSeqNum too high, expected {next_target}, got {msg_seq_num}");
        }

        // 6. Handle implicit resend via tag 789
        if enable_next_expected
            && let Some(next_expected) = next_expected_msg_seq_num
            && next_expected != next_sender_at_logon
        {
            // Peer needs messages from next_expected to next_sender_at_logon
            let end = next_sender_at_logon.saturating_sub(1);
            info!("Received implicit ResendRequest via Logon FROM: {next_expected} TO: {end}");
            self.pending_resends.push_back(next_expected..=end);
        }

        // 7. Reset grace period if logged on
        if self.is_logged_on() {
            self.state.grace_period_test_req_ids.clear();
        }

        // Normal logon: increment seq num and signal completion. Too-high:
        // leave the original message ownership to `apply_result`, which
        // will queue it and emit a ResendRequest for the gap.
        Ok(if is_normal {
            self.advance_target(storage)?;
            HandlerResult::Handled
        } else {
            HandlerResult::Enqueue
        })
    }

    /// Process-logon step 3: validate an offered NextExpectedMsgSeqNum(789)
    /// against our own sender counter. Above `next_sender` the peer expects
    /// messages we never sent (Session Layer §4.4.1); zero names no message
    /// at all, since sequence numbers start at 1 (§4.1). Either way the
    /// Logon is invalid, so the session logs out and disconnects (Session
    /// Test Cases §4.4.1 Scenario 1S(d)).
    ///
    /// Returns `Some(Disconnect)` on failure, `None` when the check passes
    /// (or no tag 789 was offered).
    /// Pass the planned sender counter of 1 for a peer reset request,
    /// before changing storage; otherwise pass the current sender counter.
    // Runs before the one consumer of the peer's tag 789, the implicit
    // resend in step 6. Zero has to die here: step 6 would queue a resend
    // from 0 and put a gap-fill stamped MsgSeqNum(34)=0 on the wire.
    fn check_logon_next_expected_range(
        &mut self,
        next_expected_msg_seq_num: Option<SeqNum>,
        next_sender: SeqNum,
    ) -> Option<HandlerResult> {
        let next_expected = next_expected_msg_seq_num?;
        let (session_status, text) = if next_expected == 0 {
            (None, "NextExpectedMsgSeqNum(789) is zero".to_owned())
        } else if next_expected > next_sender {
            (
                Some(SessionStatusBase::ReceivedNextExpectedMsgSeqNumTooHigh.into()),
                format!(
                    "NextExpectedMsgSeqNum(789) too high \
                     (expected {next_sender}, got {next_expected})"
                ),
            )
        } else {
            return None;
        };

        error!("{text}");
        self.push_logout(
            session_status,
            Some(FixString::from_ascii_lossy(text.into_bytes())),
        );
        Some(HandlerResult::Disconnect(
            DisconnectReason::InvalidLogonState,
        ))
    }

    /// Refuse a nonzero EncryptMethod before application input.
    fn check_encrypt_method<S: MessagesStorage>(
        &mut self,
        msg_seq_num: SeqNum,
        encrypt_method: Int,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        if encrypt_method == EncryptMethodBase::None as Int {
            return Ok(HandlerResult::AdminMsg);
        }

        // Refuse the Logon without attempting encryption negotiation. The
        // optional encryption tests (Scenario 17) are outside our scope.
        // This runs before a peer reset can discard history.
        let text = FixString::from_ascii_lossy(
            format!("Unsupported EncryptMethod(98) value {encrypt_method}; only 0 is supported")
                .into_bytes(),
        );
        error!("{text}");
        self.consume_seq_num(MsgTypeBase::Logon.into(), msg_seq_num, storage)?;
        self.push_logout(None, Some(text));
        Ok(HandlerResult::Disconnect(
            DisconnectReason::InvalidLogonState,
        ))
    }

    /// Reject a negative HeartBtInt(108); zero disables regular heartbeats
    /// (FIX Transport Section 5.1). The caller separately checks consistency
    /// with the interval already in force on the connection.
    ///
    /// Returns the value as seconds when it is usable, `Err(Disconnect)`
    /// after rejecting it.
    // Runs before any reset: refusal preserves the old history, but its
    // Reject consumes an expected incoming number and its Logout an outgoing one.
    fn check_heart_bt_int<S: MessagesStorage>(
        &mut self,
        msg_seq_num: SeqNum,
        heart_bt_int: Int,
        storage: &mut S,
    ) -> Result<Result<u64, HandlerResult>, FatalError> {
        self.ensure_healthy()?;

        // The conversion is the check: every non-negative `Int` fits, and
        // nothing above zero is refused - the spec sets no ceiling.
        if let Ok(secs) = u64::try_from(heart_bt_int) {
            return Ok(Ok(secs));
        }
        error!("Invalid HeartBtInt {heart_bt_int} (must be >= 0)");
        let text = fix_str!("Invalid HeartBtInt(108)").to_owned();
        self.consume_seq_num(MsgTypeBase::Logon.into(), msg_seq_num, storage)?;
        self.send_reject(
            Some(
                MsgTypeField::from(MsgTypeBase::Logon)
                    .as_fix_str()
                    .to_owned(),
            ),
            msg_seq_num,
            SessionRejectReasonBase::ValueIsIncorrect.into(),
            Some(TAG_HEART_BT_INT),
            Some(text.clone()),
        );
        // The Reject is Test Cases §4.4.1 Scenario 1S(d)'s optional step 2;
        // the Logout with Text(58) referencing the error is its step 3, and
        // that one is not optional.
        self.push_logout(None, Some(text));
        Ok(Err(HandlerResult::Disconnect(
            DisconnectReason::InvalidLogonState,
        )))
    }

    /// Echo the validated peer HeartBtInt and stage the Logon response.
    /// The caller adopts the interval on the first Logon; a running session
    /// has already checked that the value matches its effective interval.
    ///
    /// `did_reset` says the sequence counters were renumbered for this peer
    /// request. It is what the ACK echoes as
    /// `ResetSeqNumFlag(141)`. `logon_in_sequence` says the Logon itself
    /// consumes the next target seq num, so the ACK's tag 789 counts past it.
    fn send_logon_ack<S: MessagesStorage>(
        &mut self,
        heart_bt_int: Int,
        did_reset: bool,
        logon_in_sequence: bool,
        enable_next_expected: bool,
        storage: &mut S,
    ) {
        let next_expected_target = if enable_next_expected {
            let next_target = storage.next_target_msg_seq_num();
            let adjusted = if logon_in_sequence {
                next_target.checked_add(1)
            } else {
                Some(next_target)
            };
            self.state.next_expected_msg_seq_num = adjusted;
            adjusted.map(NonZeroSeqNum::get)
        } else {
            None
        };

        // In Idle, adopt and echo the initiator's HeartBtInt verbatim. In
        // Established/ResetPending/ResetProbe, process_logon has required the
        // existing effective value. Refusal is via Logout, never a counter-
        // proposal (Session Layer Sections 4.3.4 and 4.3.5.1).
        // Rewriting tag 108 from on_admin_msg_out would disagree with timers.
        // Echo 141 only for the peer-requested renumbering; our own reset ACK
        // never comes through this path (Session Layer Section 4.4.2).
        self.send_logon_response(heart_bt_int, did_reset, next_expected_target);
    }

    /// The handshake did not complete in time.
    //
    // No `Logout<5>`: the handshake never completed, so there is no session to
    // end and nobody has agreed to read anything we number. The acceptor drops
    // an unusable handshake the same way (Session Layer §4.3.1).
    pub(crate) fn on_logon_timeout(&mut self) {
        self.begin_disconnect(DisconnectReason::LogonTimeout);
    }
}
