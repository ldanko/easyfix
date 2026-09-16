//! Inbound dispatch, application decisions, and decode failures.

use std::borrow::Cow;

use easyfix_core::{
    base_messages::{AdminBase, HeaderBase, MsgTypeBase, RejectBase, SessionRejectReasonBase},
    basic_types::{FixStr, FixString, Int, MsgTypeField, SeqNum, SessionRejectReasonField, TagNum},
    deserializer::{DeserializeErrorKind, LogoutReason},
    fix_str,
    message::{DeserializeError, SessionMessage},
};
use tracing::{error, info, warn};

use super::{
    FatalError, HandlerResult, InputResult, LogonState, SessionEngine, ValidationFailure,
    ValidationResult, VerifyError, logout::unexpected_reset_logout_text, output::text_field,
};
use crate::{
    application::{DisconnectReason, InputAction},
    messages_storage::MessagesStorage,
};

impl<M: SessionMessage> SessionEngine<M> {
    /// Push a Reject<3> to admin_output.
    pub(super) fn send_reject(
        &mut self,
        ref_msg_type: Option<Cow<'static, FixStr>>,
        ref_seq_num: SeqNum,
        reason: SessionRejectReasonField,
        ref_tag_id: Option<TagNum>,
        text: Option<Cow<'static, FixStr>>,
    ) {
        info!("Message {ref_seq_num} Rejected: {reason:?} (tag={ref_tag_id:?})");
        self.push_admin(AdminBase::Reject(RejectBase {
            ref_seq_num,
            ref_tag_id: ref_tag_id.map(Int::from),
            ref_msg_type,
            session_reject_reason: Some(reason),
            text: text_field(text, MsgTypeBase::Reject),
        }));
    }

    fn on_reject<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        _reject: RejectBase<'_>,
        storage: &mut S,
    ) -> Option<ValidationResult> {
        // A Reject during gap recovery must not trigger another resend cycle.
        self.validate(
            header,
            MsgTypeBase::Reject.into(),
            storage,
            false,
            true,
            false,
        )
    }

    fn process_reject<S: MessagesStorage>(
        &mut self,
        _header: &HeaderBase<'_>,
        _reject: RejectBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
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
        self.push_logout(None, Some(Cow::Owned(text)));
        self.begin_disconnect(DisconnectReason::MessageTooLarge);
    }

    /// Process an incoming deserialized message. Top-level dispatcher.
    ///
    /// Validate the header and administrative body, then route the original
    /// message to the appropriate callback or defer it for gap recovery.
    ///
    /// A delivered message has passed all applicable session validation.
    /// Accepted-message processing is deferred to [`Self::process_admin_input`]
    /// / [`Self::process_app_input`] after the application callback returns.
    /// Validation may already queue out-of-order input or confirm the peer's
    /// acknowledgement of a local reset. Refusals are completed by
    /// [`Self::process_validation_failure`] after the error callback.
    pub(crate) fn on_input<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        storage: &mut S,
    ) -> Result<InputResult<M>, FatalError> {
        self.ensure_healthy()?;

        let header = msg.header();
        let admin = msg.try_as_admin();
        let is_admin = admin.is_some();
        let result = match admin {
            Some(AdminBase::Heartbeat(hb)) => self.on_heartbeat(&header, hb, storage),
            Some(AdminBase::TestRequest(tr)) => self.on_test_request(&header, tr, storage),
            Some(AdminBase::ResendRequest(rr)) => self.on_resend_request(&header, rr, storage),
            Some(AdminBase::Reject(rj)) => self.on_reject(&header, rj, storage),
            Some(AdminBase::SequenceReset(sr)) => self.on_sequence_reset(&header, sr, storage),
            Some(AdminBase::Logout(lo)) => self.on_logout(&header, lo, storage),
            Some(AdminBase::Logon(lg)) => self.on_logon(&header, lg, storage)?,
            None => {
                let msg_type = msg.msg_type();
                self.validate(&header, msg_type, storage, true, true, false)
            }
        };
        Ok(match result {
            None if is_admin => InputResult::AdminMsg(msg),
            None => InputResult::AppMsg(msg),
            Some(ValidationResult::Failure(failure)) => {
                InputResult::ValidationError { msg, failure }
            }
            Some(ValidationResult::Handled) => InputResult::Handled,
            Some(ValidationResult::Enqueue) => {
                self.enqueue_input(msg, storage);
                InputResult::Handled
            }
        })
    }

    /// Re-borrow `msg`, extract its admin variant, and run the matching
    /// `process_X` post-callback handler. Mirror of [`Self::on_input`] for
    /// the second phase: the engine has just returned from the
    /// `on_admin_msg_in` callback with `InputAction::Accept`, so it is now
    /// safe to mutate state (transition logon, send responses, advance
    /// seq num, queue resend ranges).
    fn dispatch_process<S: MessagesStorage>(
        &mut self,
        msg: &M,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        let header = msg.header();
        // TODO: this `try_as_admin()` re-projects the same AdminBase variant
        // that `on_input` already built during validation. The construction
        // is zero-allocation (Cow::Borrowed projections), but doing it
        // twice per inbound admin message is structurally redundant.
        // Two viable fixes:
        // 1. Carry the typed body forward in `InputResult::AdminMsg`
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

    /// Finish processing with ownership of the original message.
    fn apply_processing_outcome<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        result: HandlerResult,
        storage: &mut S,
    ) {
        match result {
            HandlerResult::Handled => {}
            HandlerResult::Enqueue => self.enqueue_input(msg, storage),
            HandlerResult::Disconnect(reason) => {
                self.begin_disconnect(reason);
            }
        }
    }

    /// Park the original message and request the preceding sequence gap.
    fn enqueue_input<S: MessagesStorage>(&mut self, msg: Box<M>, storage: &mut S) {
        let seq = msg.msg_seq_num();
        // Most types are re-dispatched through `on_input` once the gap is
        // filled. Logon and ResendRequest were processed or deliberately
        // ignored at receipt; `next_queued_message` recognizes them by type
        // and only advances the target counter. Keep the original message
        // so the queue contents remain truthful in both cases.
        self.state.queue.insert(seq, msg);
        self.request_resend(seq, storage);
    }

    /// Complete the refusal after the application has observed its cause.
    pub(crate) fn process_validation_failure<S: MessagesStorage>(
        &mut self,
        msg: Box<M>,
        failure: ValidationFailure,
        storage: &mut S,
    ) -> Result<(), FatalError> {
        let result =
            self.apply_validation_reaction(msg.msg_type(), msg.msg_seq_num(), failure, storage)?;
        self.apply_processing_outcome(msg, result, storage);
        Ok(())
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
                this.apply_processing_outcome(msg, result, storage);
                Ok(InputResult::Handled)
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
                    Some(Cow::Borrowed(fix_str!(
                        "Reset Logon acknowledgement rejected by application"
                    ))),
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
    /// logic (admin: dispatch_process + apply_processing_outcome; app: increment
    /// target seq num). Other actions consume the message's seq num via
    /// [`Self::consume_seq_num`]. Immediate refusals of the acceptor's initial
    /// Logon follow `preserve_seq_num_on_logon_refusal`.
    ///
    /// For `Reject`: emit a session-level Reject<3> referencing the
    /// original message - consistent with the [`Self::validate`]
    /// Reject path.
    ///
    /// For `Logout`: stage a Logout and enter `LogoutSent` when waiting, or
    /// flag immediate disconnect when requested. In either ending state the
    /// shared gate suppresses another Logout and preserves its deadline;
    /// sequence consumption and the disconnect decision still apply.
    ///
    /// For `Disconnect`: set the disconnect flag without any outbound message.
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
        // Preserving the counter keeps unauthenticated attempts from locking
        // out the real peer. A waiting Logout must still consume the number
        // so the peer's acknowledgement is in sequence.
        let preserve_logon_seq_num = self.session_settings.preserve_seq_num_on_logon_refusal
            && ref_msg_type == MsgTypeBase::Logon
            && matches!(self.state.logon_state, LogonState::Idle);

        Ok(match action {
            InputAction::Accept => on_accept(self, storage)?,
            InputAction::Reject { reason, text, tag } => {
                self.consume_seq_num(ref_msg_type, ref_seq_num, storage)?;
                self.send_reject(
                    Some(Cow::Owned(ref_msg_type.as_fix_str().to_owned())),
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
                if !disconnect || !preserve_logon_seq_num {
                    self.consume_seq_num(ref_msg_type, ref_seq_num, storage)?;
                }
                if disconnect {
                    self.push_logout(session_status, text);
                    self.begin_disconnect(DisconnectReason::ApplicationForcedDisconnect);
                } else {
                    self.send_logout(session_status, text);
                }
                InputResult::Handled
            }
            InputAction::Disconnect => {
                if !preserve_logon_seq_num {
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

        let text: Cow<'static, FixStr> =
            Cow::Owned(FixString::from_ascii_lossy(error.to_string().into_bytes()));
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
                self.push_logout(None, Some(Cow::Borrowed(text)));
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
                let recovered_header = msg_type_field.zip(error.header.as_deref());
                if let Some((msg_type, header)) = recovered_header {
                    if let Some(result) =
                        self.validate_failed_decode_header(header, msg_type, storage)?
                    {
                        match result {
                            // The unparseable message cannot be queued. Request
                            // its replacement along with the gap (4.8.2).
                            HandlerResult::Enqueue => {
                                self.request_resend_through(*seq_num, storage);
                            }
                            HandlerResult::Handled => {}
                            HandlerResult::Disconnect(reason) => self.begin_disconnect(reason),
                        }
                        return Ok(InputResult::Error(error));
                    }
                    // Only a validated header gets the normal consumption
                    // rule, including its SequenceReset exception.
                    self.consume_seq_num(msg_type, *seq_num, storage)?;
                } else {
                    // Unknown types retain the decoder's Reject during a reset
                    // (Scenario 2(q)), followed by the terminal response below.
                    // A raw MsgType can be syntactically valid but absent from
                    // the dictionary, so the reject reason matters too.
                    let reject_unknown_type = self.state.local_reset_unconfirmed
                        && (msg_type_field.is_none()
                            || *reason == SessionRejectReasonBase::InvalidMsgType);
                    // A decode failure does not waive the logon-state gate:
                    // disallowed traffic gets the state verdict, not a Reject.
                    if !reject_unknown_type
                        && let Err(verdict) = self.check_failed_decode_logon_state(msg_type_field)
                    {
                        if let VerifyError::UnexpectedMessageDuringReset { msg_type } = verdict {
                            self.push_logout(
                                None,
                                Some(Cow::Owned(unexpected_reset_logout_text(msg_type))),
                            );
                        }
                        self.begin_disconnect(DisconnectReason::InvalidLogonState);
                        return Ok(InputResult::Error(error));
                    }
                }

                self.send_reject(
                    msg_type.clone().map(Cow::Owned),
                    *seq_num,
                    *reason,
                    *tag,
                    Some(text.clone()),
                );
                // Without a validated header, retain the conservative rule:
                // advance only an expected number, after staging the Reject.
                if recovered_header.is_none() && *seq_num == storage.next_target_msg_seq_num().get()
                {
                    self.advance_target(storage)?;
                }
                // Invalid Logon during establishment: Reject, then Logout
                // naming the error and disconnect (Test Cases Scenario 1S(d)).
                // A failed response to our reset is terminal as well.
                if self.state.local_reset_unconfirmed
                    || (msg_type_field == Some(MsgTypeBase::Logon.into())
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

        Ok(InputResult::Error(error))
    }

    /// Validate a recovered header before responding to its body error.
    /// `None` permits the body-level Reject; `Some` replaces it with the
    /// header verdict (FIX Session Layer 4.8.1/4.8.2; Scenario 2(b)/(c)/(e)).
    fn validate_failed_decode_header<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        msg_type: MsgTypeField,
        storage: &mut S,
    ) -> Result<Option<HandlerResult>, FatalError> {
        // Seq-num check gating mirrors the clean-path role wrappers:
        // Logon skips too-high (the tag-789 logic needs the parsed body;
        // the caller escalates a broken Logon before establishment) and
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

        let result = self.validate(
            header,
            msg_type,
            storage,
            check_too_high,
            check_too_low,
            false,
        );
        // A failed decode has no complete message to deliver. Apply the
        // same refusal immediately and report only InputError::Deserialize.
        Ok(match result {
            Some(ValidationResult::Failure(failure)) => Some(self.apply_validation_reaction(
                msg_type,
                header.msg_seq_num,
                failure,
                storage,
            )?),
            Some(ValidationResult::Handled) => Some(HandlerResult::Handled),
            Some(ValidationResult::Enqueue) => Some(HandlerResult::Enqueue),
            None => None,
        })
    }
}
