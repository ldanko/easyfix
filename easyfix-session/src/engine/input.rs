//! Inbound dispatch, application decisions, and decode failures.

use std::borrow::Cow;

use easyfix_core::{
    base_messages::{AdminBase, HeaderBase, MsgTypeBase, RejectBase, SessionRejectReasonBase},
    basic_types::{FixString, Int, MsgTypeField, SeqNum, SessionRejectReasonField, TagNum},
    deserializer::{DeserializeErrorKind, LogoutReason},
    fix_str,
    message::{DeserializeError, SessionMessage},
};
use tracing::{error, info, warn};

use super::{
    FatalError, HandlerResult, InputResult, LogonState, SessionEngine, VerifyError,
    output::text_field,
};
use crate::{
    application::{DisconnectReason, InputAction},
    messages_storage::MessagesStorage,
};

impl<M: SessionMessage> SessionEngine<M> {
    /// Push a Reject<3> to admin_output.
    pub(super) fn send_reject(
        &mut self,
        ref_msg_type: Option<FixString>,
        ref_seq_num: SeqNum,
        reason: SessionRejectReasonField,
        ref_tag_id: Option<TagNum>,
        text: Option<FixString>,
    ) {
        info!("Message {ref_seq_num} Rejected: {reason:?} (tag={ref_tag_id:?})");
        self.push_admin(AdminBase::Reject(RejectBase {
            ref_seq_num,
            ref_tag_id: ref_tag_id.map(Int::from),
            ref_msg_type: ref_msg_type.map(Cow::Owned),
            session_reject_reason: Some(reason),
            text: text_field(text, MsgTypeBase::Reject),
        }));
    }

    fn on_reject<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        _reject: RejectBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        Ok(self
            .validate_reject(header, storage)?
            .unwrap_or(HandlerResult::AdminMsg))
    }

    fn process_reject<S: MessagesStorage>(
        &mut self,
        _header: &HeaderBase<'_>,
        _reject: RejectBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        self.advance_target(storage)?;
        Ok(HandlerResult::Handled)
    }

    /// An inbound frame declares `frame_len` bytes, more than
    /// `max_message_size`. The session answers the way FIX Session Layer
    /// §4.3.6 has a peer answer a size it cannot process: a `Logout<5>`
    /// naming the sizes in `Text(58)`, then a disconnect. `NextNumIn` stays
    /// where it is - the message was never read, so its `MsgSeqNum(34)` is
    /// unknown - and the peer offers the message again on reconnect, where
    /// it meets the same answer until one side changes its limit.
    //
    // The Text does not name `MaxMessageSize(383)`: the limit is enforced
    // whether or not the Logon announced it, and the field is optional - a
    // dictionary may not carry it at all. The phrase echoes the §4.3.6
    // wording instead.
    pub(crate) fn on_oversized_message(&mut self, frame_len: usize) {
        // The shared gate suppresses another Logout in both ending states;
        // the disconnect and unchanged input counter still apply.
        let limit = self.session_settings.max_message_size;
        warn!(
            frame_len,
            %limit,
            "inbound message exceeds max message size, logging out"
        );
        let text = FixString::from_ascii_lossy(
            format!("Message size {frame_len} exceeds maximum message size of {limit}")
                .into_bytes(),
        );
        self.push_logout(None, Some(text));
        self.begin_disconnect(DisconnectReason::MessageTooLarge);
    }

    /// Process an incoming deserialized message. Top-level dispatcher.
    ///
    /// 1. [`Self::dispatch`] borrows the message, extracts the
    ///    [`AdminBase`] variant once, and routes to the per-variant
    ///    `on_X` validation handler (or the app-message header validator).
    ///    On success `on_X` returns `HandlerResult::AdminMsg` (admin) or
    ///    `HandlerResult::AppMsg` (app); on failure it returns the
    ///    appropriate `Handled` / `Enqueue` / `Disconnect`.
    /// 2. [`Self::apply_result`] consumes `msg` according to the
    ///    [`HandlerResult`] - enqueueing on `Enqueue`, surfacing
    ///    `AppMsg` / `AdminMsg` for the application callback, or setting
    ///    the disconnect flag.
    ///
    /// Protocol logic (sending heartbeats, transitioning logon state,
    /// handling resend ranges, etc.) does **not** run during this call.
    /// It is deferred to [`Self::process_admin_input`] /
    /// [`Self::process_app_input`], which the IO loop calls after the
    /// `on_admin_msg_in` / `on_app_msg_in` callback returns. This guarantees
    /// the application sees every inbound message after validation but
    /// before the engine reacts on the wire.
    pub(crate) fn on_input<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        storage: &mut S,
    ) -> Result<InputResult<M>, FatalError> {
        self.ensure_healthy()?;

        let result = self.dispatch(&msg, storage)?;
        Ok(self.apply_result(msg, result, storage))
    }

    /// Borrow `msg`, extract its admin variant once, and dispatch to the
    /// matching `on_X` validation handler. Handlers receive the borrowed
    /// header and the typed `*Base` variant.
    fn dispatch<S: MessagesStorage>(
        &mut self,
        msg: &M,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let header = msg.header();
        Ok(match msg.try_as_admin() {
            Some(AdminBase::Heartbeat(hb)) => self.on_heartbeat(&header, hb, storage)?,
            Some(AdminBase::TestRequest(tr)) => self.on_test_request(&header, tr, storage)?,
            Some(AdminBase::ResendRequest(rr)) => self.on_resend_request(&header, rr, storage)?,
            Some(AdminBase::Reject(rj)) => self.on_reject(&header, rj, storage)?,
            Some(AdminBase::SequenceReset(sr)) => self.on_sequence_reset(&header, sr, storage)?,
            Some(AdminBase::Logout(lo)) => self.on_logout(&header, lo, storage)?,
            Some(AdminBase::Logon(lg)) => self.on_logon(&header, lg, storage)?,
            None => {
                // App message: validate header; AppMsg if it passed, otherwise
                // the failure-mapped HandlerResult.
                let msg_type = msg.msg_type();
                self.validate(&header, msg_type, storage)?
                    .unwrap_or(HandlerResult::AppMsg)
            }
        })
    }

    /// Re-borrow `msg`, extract its admin variant, and run the matching
    /// `process_X` post-callback handler. Mirror of [`Self::dispatch`] for
    /// the second phase: the engine has just returned from the
    /// `on_admin_msg_in` callback with `InputAction::Accept`, so it is now
    /// safe to mutate state (transition logon, send responses, advance
    /// seq num, queue resend ranges).
    fn dispatch_process<S: MessagesStorage>(
        &mut self,
        msg: &M,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let header = msg.header();
        // TODO: this `try_as_admin()` re-projects the same AdminBase variant
        // that `dispatch` already built during validation. The construction
        // is zero-allocation (Cow::Borrowed projections), but doing it
        // twice per inbound admin message is structurally redundant.
        // Two viable fixes:
        // 1. Carry the typed body forward in `HandlerResult::AdminMsg`
        //    via an owned `AdminPayload` enum (cost: small allocations
        //    when fields are non-Copy - Heartbeat test_req_id, etc.).
        // 2. Move the validate->callback->process flow into a single async
        //    helper in io.rs so the borrowed body stays in scope across
        //    the `.await`. Engine itself stays sync.
        // Deferred - current cost sits below the noise floor of session traffic.
        Ok(match msg.try_as_admin() {
            Some(AdminBase::Heartbeat(hb)) => self.process_heartbeat(&header, hb, storage)?,
            Some(AdminBase::TestRequest(tr)) => self.process_test_request(&header, tr, storage)?,
            Some(AdminBase::ResendRequest(rr)) => {
                self.process_resend_request(&header, rr, storage)?
            }
            Some(AdminBase::Reject(rj)) => self.process_reject(&header, rj, storage)?,
            Some(AdminBase::SequenceReset(sr)) => {
                self.process_sequence_reset(&header, sr, storage)?
            }
            Some(AdminBase::Logout(lo)) => self.process_logout(&header, lo, storage)?,
            Some(AdminBase::Logon(lg)) => self.process_logon(&header, lg, storage)?,
            None => unreachable!("dispatch_process called for non-admin message"),
        })
    }

    /// Convert a handler outcome into the public [`InputResult`],
    /// applying the side effects that need ownership of the original
    /// message (queue insert, app/admin callback dispatch, disconnect).
    fn apply_result<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        result: HandlerResult,
        storage: &mut S,
    ) -> InputResult<M> {
        match result {
            HandlerResult::Handled => InputResult::Handled,
            HandlerResult::Enqueue => {
                let seq = msg.msg_seq_num();
                // Park the original incoming message in the out-of-order
                // queue under its sequence number. For most types it is
                // re-dispatched through `on_input` once the gap is filled
                // (see `next_queued_message`). For Logon and ResendRequest
                // the body was processed or deliberately ignored at receipt - those
                // are recognized by `next_queued_message` from
                // `msg.msg_type()` alone, which only advances the target
                // sequence and does not re-process the body. Storing the
                // original message - rather than synthesizing a
                // placeholder - is simpler, costs nothing, and makes the
                // queue contents truthful for any future reader.
                self.state.queue.insert(seq, msg);
                self.request_resend(seq, storage);
                InputResult::Handled
            }
            HandlerResult::AppMsg => InputResult::AppMsg(msg),
            HandlerResult::AdminMsg => InputResult::AdminMsg(msg),
            HandlerResult::Disconnect(reason) => {
                self.begin_disconnect(reason);
                InputResult::Handled
            }
        }
    }

    /// Feed back the application's response to an inbound admin message.
    ///
    /// Called by the IO loop after `on_admin_msg_in` returns. On
    /// [`InputAction::Accept`] the engine runs the protocol logic
    /// associated with the message via [`Self::dispatch_process`]; for
    /// the other variants the engine emits a Reject<3> / Logout / sets
    /// the disconnect flag without invoking `process_X`. The original
    /// message ownership flows back through `msg` so the engine can
    /// extract `ref_seq_num` / `ref_msg_type` for Reject<3> and route
    /// to the right `process_X` for Accept.
    pub(crate) fn process_admin_input<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        action: InputAction,
        storage: &mut S,
    ) -> Result<InputResult<M>, FatalError> {
        self.ensure_healthy()?;

        let ref_seq_num = msg.msg_seq_num();
        let ref_msg_type = msg.msg_type();
        let rejected_reset_ack = matches!(action, InputAction::Reject { .. })
            && !self.answers_logon()
            && matches!(msg.try_as_admin(), Some(AdminBase::Logon(logon))
                if logon.reset_seq_num_flag == Some(true));
        let result = self.handle_input_action(
            action,
            ref_seq_num,
            ref_msg_type,
            storage,
            move |this, storage| {
                let result = this.dispatch_process(&msg, storage)?;
                Ok(this.apply_result(msg, result, storage))
            },
        )?;
        if rejected_reset_ack {
            // The peer's renumbering was confirmed before the callback. A
            // rejected acknowledgement cannot complete the exchange, so end
            // it now without reporting an unconfirmed reset or waiting for
            // another Logon.
            if !matches!(self.state.logon_state, LogonState::LogoutSent { .. }) {
                self.push_logout(
                    None,
                    Some(
                        fix_str!("Reset Logon acknowledgement rejected by application").to_owned(),
                    ),
                );
            }
            self.begin_disconnect(DisconnectReason::ApplicationForcedDisconnect);
        }
        Ok(result)
    }

    /// Feed back the application's response to an inbound app message.
    ///
    /// Called by the IO loop after `on_app_msg_in` returns. App messages
    /// have no engine-level protocol logic beyond advancing the target
    /// sequence number on accept; the IO loop captures `ref_seq_num` and
    /// `ref_msg_type` before passing the boxed message to the callback so
    /// the engine still has them here for the Reject branch.
    pub(crate) fn process_app_input<S: MessagesStorage>(
        &mut self,
        ref_seq_num: SeqNum,
        ref_msg_type: MsgTypeField,
        action: InputAction,
        storage: &mut S,
    ) -> Result<InputResult<M>, FatalError> {
        self.ensure_healthy()?;

        self.handle_input_action(
            action,
            ref_seq_num,
            ref_msg_type,
            storage,
            |this, storage| {
                this.advance_target(storage)?;
                Ok(InputResult::Handled)
            },
        )
    }

    /// Branch on the application's `InputAction` response. The caller
    /// supplies an `on_accept` closure that owns the per-flow Accept
    /// logic (admin: dispatch_process + apply_result; app: increment
    /// target seq num). Other actions consume the message's seq num via
    /// [`Self::consume_seq_num`], except a silent refusal of the acceptor's
    /// initial Logon request.
    ///
    /// For `Reject`: emit a session-level Reject<3> referencing the
    /// original message - consistent with the [`Self::validate_impl`]
    /// Reject path.
    ///
    /// For `Logout`: stage a Logout and enter `LogoutSent` when waiting, or
    /// flag immediate disconnect when requested. In either ending state the
    /// shared gate suppresses another Logout and preserves its deadline;
    /// sequence consumption and the disconnect decision still apply.
    ///
    /// For `Disconnect`: set the disconnect flag without any outbound message.
    //
    // A Logout response arrives one past the refused message. Consuming its
    // number avoids requesting a resend from a peer already closing down.
    fn handle_input_action<S, F>(
        &mut self,
        action: InputAction,
        ref_seq_num: SeqNum,
        ref_msg_type: MsgTypeField,
        storage: &mut S,
        on_accept: F,
    ) -> Result<InputResult<M>, FatalError>
    where
        S: MessagesStorage,
        F: FnOnce(&mut Self, &mut S) -> Result<InputResult<M>, FatalError>,
    {
        self.ensure_healthy()?;

        Ok(match action {
            InputAction::Accept => on_accept(self, storage)?,
            InputAction::Reject { reason, text, tag } => {
                self.consume_seq_num(ref_msg_type, ref_seq_num, storage)?;
                self.send_reject(
                    Some(ref_msg_type.as_fix_str().to_owned()),
                    ref_seq_num,
                    reason,
                    tag,
                    text,
                );
                InputResult::Handled
            }
            InputAction::Logout {
                session_status,
                text,
                disconnect,
            } => {
                self.consume_seq_num(ref_msg_type, ref_seq_num, storage)?;
                if disconnect {
                    self.push_logout(session_status, text);
                    self.begin_disconnect(DisconnectReason::ApplicationForcedDisconnect);
                } else {
                    self.send_logout(session_status, text);
                }
                InputResult::Handled
            }
            InputAction::Disconnect => {
                // An unauthenticated Logon must not advance the persisted
                // counter: repeated attempts could lock out the real peer.
                let refused_logon = ref_msg_type == MsgTypeBase::Logon
                    && matches!(self.state.logon_state, LogonState::Idle);
                if !refused_logon {
                    self.consume_seq_num(ref_msg_type, ref_seq_num, storage)?;
                }
                self.begin_disconnect(DisconnectReason::ApplicationForcedDisconnect);
                InputResult::Handled
            }
        })
    }

    /// Process a deserialization error.
    pub(crate) fn on_deserialize_error<S: MessagesStorage>(
        &mut self,
        error: DeserializeError,
        storage: &mut S,
    ) -> Result<InputResult<M>, FatalError> {
        self.ensure_healthy()?;

        let text = FixString::from_ascii_lossy(error.to_string().into_bytes());
        error!(deserialize_error = %text);

        match &error.kind {
            DeserializeErrorKind::Garbled(reason) => {
                error!("Garbled message: {reason}");
            }
            DeserializeErrorKind::Logout(reason) => {
                // Both causes are well-formed messages the spec answers with a
                // Logout(35=5) + disconnect: missing MsgSeqNum (§4.5.3) and a
                // recognized-but-wrong BeginString (Scenario 2(i)). The
                // acceptor's first-Logon BeginString mismatch is handled
                // separately (silent drop, §4.6.4) before reaching here.
                let (text, disconnect_reason) = match reason {
                    LogoutReason::MsgSeqNumMissing => (
                        fix_str!("MsgSeqNum(34) not found"),
                        DisconnectReason::MsgSeqNumNotFound,
                    ),
                    LogoutReason::BeginStringMismatch => (
                        fix_str!("Invalid BeginString(8)"),
                        DisconnectReason::InvalidBeginString,
                    ),
                };
                self.push_logout(None, Some(text.to_owned()));
                self.begin_disconnect(disconnect_reason);
            }
            DeserializeErrorKind::Reject {
                msg_type,
                seq_num,
                tag,
                reason,
            } => {
                // Header verdicts take precedence over the body-level error
                // (FIX Session Layer 4.8.1/4.8.2; Scenario 2): when the
                // failed message's header was recovered, run the same
                // validation as cleanly-parsed messages. Without a header,
                // or with an unresolvable MsgType, fall back to the
                // conservative header-less handling.
                let msg_type_field = msg_type
                    .as_deref()
                    .and_then(|mt| MsgTypeField::from_bytes(mt.as_bytes()).ok());
                if let Some(msg_type_field) = msg_type_field
                    && error.header.is_some()
                {
                    self.reject_error_with_header(&error, msg_type_field, text, storage)?;
                } else if self.state.local_reset_unconfirmed
                    && (msg_type_field.is_none()
                        || *reason == SessionRejectReasonBase::InvalidMsgType)
                {
                    // Unknown types still get the decoder's Reject, but must
                    // not consume the reset ACK number and leave it reusable.
                    // The decoder may preserve a syntactically valid raw type
                    // even when that type is absent from the dictionary.
                    self.send_reject(
                        msg_type.clone(),
                        *seq_num,
                        *reason,
                        *tag,
                        Some(text.clone()),
                    );
                    if *seq_num == storage.next_target_msg_seq_num().get() {
                        self.advance_target(storage)?;
                    }
                    self.push_logout(None, Some(text));
                    self.begin_disconnect(DisconnectReason::InvalidLogonState);
                } else if let Err(error) = self.check_failed_decode_logon_state(msg_type_field) {
                    // A decode failure does not waive the logon-state gate:
                    // a message type not permitted in this state must not
                    // draw a Reject or advance NextNumIn just because it was
                    // too damaged to validate. Preserve the reset-window
                    // Logout verdict as on the recovered-header path.
                    if let VerifyError::UnexpectedMessageDuringReset { msg_type } = error {
                        self.push_unexpected_reset_logout(msg_type);
                    }
                    self.begin_disconnect(DisconnectReason::InvalidLogonState);
                } else {
                    self.send_reject(
                        msg_type.clone(),
                        *seq_num,
                        *reason,
                        *tag,
                        Some(text.clone()),
                    );
                    // A rejected message must advance NextNumIn
                    // (FIX Session Layer §4.5.4; Scenario 14 a-j), but only when it is
                    // in sequence - a too-high/too-low message leaves the gap to be
                    // recovered by ResendRequest, mirroring the `verify_header`
                    // Reject path.
                    if *seq_num == storage.next_target_msg_seq_num().get() {
                        self.advance_target(storage)?;
                    }
                    // An invalid Logon(35=A) while the logon exchange is still in
                    // progress can never lead to an established session - Test
                    // Cases Scenario 1S(d) mandates escalation after the
                    // (optional) Reject: Logout with Text(58) referencing the
                    // error condition, then disconnect. (This header-less
                    // fallback cannot check the header, so the escalation is
                    // keyed on the recovered MsgType alone.)
                    if self.state.local_reset_unconfirmed
                        || (msg_type.as_deref() == Some(fix_str!("A"))
                            && matches!(
                                self.state.logon_state,
                                LogonState::Idle | LogonState::LogonSent
                            ))
                    {
                        self.push_logout(None, Some(text));
                        self.begin_disconnect(DisconnectReason::InvalidLogonState);
                    }
                }
            }
        }

        Ok(InputResult::Error(error))
    }

    /// Failed-decode handling when the message's header was recovered
    /// (`DeserializeError::header`): header verdicts run through
    /// [`Self::validate_impl`] exactly like a cleanly-parsed message and
    /// take precedence over the body-level Reject (FIX Session Layer
    /// 4.8.1/4.8.2; Scenario 2(b)/(c)/(e); DESIGN-parse-error-header.md).
    ///
    /// The caller guarantees `error.kind` is `Reject` and `error.header`
    /// is `Some`.
    fn reject_error_with_header<S: MessagesStorage>(
        &mut self,
        error: &DeserializeError,
        msg_type: MsgTypeField,
        text: FixString,
        storage: &mut S,
    ) -> Result<(), FatalError> {
        self.ensure_healthy()?;

        let (
            DeserializeErrorKind::Reject {
                seq_num,
                tag,
                reason,
                ..
            },
            Some(header),
        ) = (&error.kind, &error.header)
        else {
            return Ok(());
        };
        let (seq_num, tag, reason) = (*seq_num, *tag, *reason);

        // Seq-num check gating mirrors the clean-path role wrappers:
        // Logon skips too-high (the tag-789 logic needs the parsed body;
        // the pre-established escalation below covers a broken Logon) and
        // SequenceReset skips both (Reset mode ignores its own MsgSeqNum,
        // 4.8.8, and the GapFillFlag is unknowable from a broken body).
        let (check_too_high, check_too_low) = if msg_type == MsgTypeBase::Logon {
            (false, !self.state.local_reset_unconfirmed)
        } else if msg_type == MsgTypeBase::SequenceReset
            || (msg_type == MsgTypeBase::Logout && self.state.local_reset_unconfirmed)
        {
            (false, false)
        } else {
            (true, true)
        };

        match self.validate_impl(
            header,
            msg_type,
            storage,
            check_too_high,
            check_too_low,
            false,
        )? {
            // Header fully valid and in sequence: the body-level Reject
            // stands and consumes the seq num (4.5.4) - same advance rule
            // as every other Reject path (SequenceReset excluded, see
            // `consume_seq_num`).
            None => {
                self.consume_seq_num(msg_type, seq_num, storage)?;
                self.send_reject(
                    Some(msg_type.as_fix_str().to_owned()),
                    seq_num,
                    reason,
                    tag,
                    Some(text.clone()),
                );
                // An invalid Logon(35=A) while the logon exchange is still
                // in progress can never lead to an established session -
                // Test Cases Scenario 1S(d) mandates escalation after the
                // (optional) Reject: Logout with Text(58) referencing the
                // error condition, then disconnect.
                if self.state.local_reset_unconfirmed
                    || (msg_type == MsgTypeBase::Logon
                        && matches!(
                            self.state.logon_state,
                            LogonState::Idle | LogonState::LogonSent
                        ))
                {
                    self.push_logout(None, Some(text));
                    self.begin_disconnect(DisconnectReason::InvalidLogonState);
                }
            }
            // Too high: the unparseable message cannot be queued, so the
            // gap request extends THROUGH its seq num (drop-and-request,
            // 4.8.2 Figure 12); the redelivered copy arrives in sequence
            // and is rejected there. No Reject now - a message above the
            // gap must not be processed before the gap is filled (4.8.2).
            Some(HandlerResult::Enqueue) => {
                self.request_resend_through(seq_num, storage);
            }
            // Duplicate (silently ignored, Scenario 2(e)) or a
            // header-level Reject that validate_impl already sent in
            // place of the body-level one.
            Some(HandlerResult::Handled) => {}
            // Session-ending header verdicts. validate_impl already staged
            // the output where the spec mandates one: TooLow -> Logout
            // (ReceivedMsgSeqNumTooLow), CompID / SendingTime accuracy ->
            // Reject + Logout. InvalidLogonState disconnects SILENTLY -
            // no Reject, no Logout - matching the clean-path reaction.
            // Latch the disconnect like the clean-path dispatcher does.
            Some(HandlerResult::Disconnect(reason)) => {
                self.begin_disconnect(reason);
            }
            // validate_impl never produces message-dispatch results.
            Some(HandlerResult::AppMsg | HandlerResult::AdminMsg) => {}
        }
        Ok(())
    }
}
