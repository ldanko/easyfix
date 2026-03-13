use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, HeaderBase, HeartbeatBase, LogonBase, LogoutBase,
        MsgTypeBase, RejectBase, ResendRequestBase, SequenceResetBase, SessionRejectReasonBase,
        SessionStatusBase, TestRequestBase,
    },
    basic_types::{
        FixStr, FixString, Int, MsgTypeField, SeqNum, SessionRejectReasonField, SessionStatusField,
        TagNum, Utc, UtcTimestamp,
    },
    deserializer::{DeserializeError, raw_message},
    fix_str,
    message::{MsgCat, SessionMessage},
};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::{
    DisconnectReason, Sender,
    application::{Emitter, FixEventInternal, InputResponderMsg, Responder},
    messages_storage::MessagesStorage,
    session_id::SessionId,
    session_state::State,
    settings::{SessionSettings, Settings},
};

// TODO: should be configurable per session, not hardcoded.
const DEFAULT_APPL_VER_ID: &FixStr = fix_str!("9");

// Tag numbers used by session-level validation.
const TAG_NEW_SEQ_NO: TagNum = 36;
const TAG_SENDER_COMP_ID: TagNum = 49;
const TAG_SENDING_TIME: TagNum = 52;
const TAG_TARGET_COMP_ID: TagNum = 56;
const TAG_HEART_BT_INT: TagNum = 108;
const TAG_ORIG_SENDING_TIME: TagNum = 122;

#[derive(Debug, thiserror::Error)]
enum VerifyError {
    #[error("Message already received")]
    Duplicate,
    #[error("Too high target sequence number {msg_seq_num}")]
    ResendRequest { msg_seq_num: SeqNum },
    #[error("Reject due to {reason:?} (tag={tag:?}, disconnect_reason={disconnect_reason:?})")]
    Reject {
        reason: SessionRejectReasonBase,
        tag: Option<TagNum>,
        disconnect_reason: Option<DisconnectReason>,
    },
    #[error("Invalid logon state")]
    InvalidLogonState,
    #[error("MsgSeqNum too low, expected {next_target_msg_seq_num}, got {msg_seq_num}")]
    SeqNumTooLow {
        msg_seq_num: SeqNum,
        next_target_msg_seq_num: SeqNum,
    },
    #[error("Rejected by application ({reason:?}: {text})")]
    ApplicationForcedReject {
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReasonField,
        text: FixString,
        ref_tag_id: Option<Int>,
    },
    #[error("Rejected by application with Logout<5> ({})", .text.as_ref().map(FixString::as_utf8).unwrap_or_default())]
    ApplicationForcedLogout {
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
        disconnect: bool,
    },
    #[error("Disconnected by application: {reason:?}")]
    ApplicationForcedDisconnect { reason: Option<String> },
    #[error("Message processing aborted by application")]
    ApplicationAbortedProcessing,
}

impl VerifyError {
    fn invalid_time() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReasonBase::SendingTimeAccuracyProblem,
            tag: Some(TAG_SENDING_TIME),
            disconnect_reason: None,
        }
    }

    fn invalid_comp_id(tag: TagNum) -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReasonBase::CompIdProblem,
            tag: Some(tag),
            disconnect_reason: Some(DisconnectReason::InvalidCompId),
        }
    }

    fn target_seq_num_too_high(msg_seq_num: SeqNum) -> VerifyError {
        VerifyError::ResendRequest { msg_seq_num }
    }

    fn missing_orig_time() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReasonBase::RequiredTagMissing,
            tag: Some(TAG_ORIG_SENDING_TIME),
            disconnect_reason: None,
        }
    }

    fn invalid_orig_time() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReasonBase::SendingTimeAccuracyProblem,
            tag: Some(TAG_ORIG_SENDING_TIME),
            disconnect_reason: Some(DisconnectReason::InvalidOrigSendingTime),
        }
    }
}

impl From<InputResponderMsg> for VerifyError {
    fn from(msg: InputResponderMsg) -> VerifyError {
        match msg {
            InputResponderMsg::Ignore => VerifyError::ApplicationAbortedProcessing,
            InputResponderMsg::Reject {
                ref_msg_type,
                ref_seq_num,
                reason,
                text,
                ref_tag_id,
            } => VerifyError::ApplicationForcedReject {
                ref_msg_type,
                ref_seq_num,
                reason,
                text,
                ref_tag_id,
            },
            InputResponderMsg::Logout {
                session_status,
                text,
                disconnect,
            } => VerifyError::ApplicationForcedLogout {
                session_status,
                text,
                disconnect,
            },
            InputResponderMsg::Disconnect { reason } => {
                VerifyError::ApplicationForcedDisconnect { reason }
            }
        }
    }
}

trait MessageExt {
    fn resend_as_gap_fill(&self) -> bool;
}

impl<M: SessionMessage> MessageExt for M {
    fn resend_as_gap_fill(&self) -> bool {
        matches!(self.msg_cat(), MsgCat::Admin) && self.msg_type() != MsgTypeBase::Reject
    }
}

#[derive(Debug)]
pub(crate) struct Session<M: SessionMessage, S> {
    // XXX: To avoid borrow errors, borrow state only in async fn,
    //      and in regular fn pass it by ref as argument.
    state: Rc<RefCell<State<M, S>>>,
    sender: Sender<M>,
    settings: Settings,
    session_settings: SessionSettings,
    emitter: Emitter<M>,
    // Not in SessionState as I/O layer asks for this value often
    heartbeat_interval: Cell<u64>,
    disconnect_notify: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl<M: SessionMessage, S: MessagesStorage> Session<M, S> {
    pub(crate) fn new(
        settings: Settings,
        session_settings: SessionSettings,
        state: Rc<RefCell<State<M, S>>>,
        sender: Sender<M>,
        emitter: Emitter<M>,
        disconnect_notify_tx: tokio::sync::oneshot::Sender<()>,
    ) -> Session<M, S> {
        let heartbeat_interval = settings
            .heartbeat_interval
            .unwrap_or(settings.auto_disconnect_after_no_logout.as_secs());
        Session {
            state,
            settings,
            session_settings,
            sender,
            emitter,
            heartbeat_interval: Cell::new(heartbeat_interval),
            disconnect_notify: RefCell::new(Some(disconnect_notify_tx)),
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_settings.session_id
    }

    pub(crate) fn state(&self) -> &Rc<RefCell<State<M, S>>> {
        &self.state
    }

    pub fn is_logged_on(state: &State<M, S>) -> bool {
        state.logon_received() && state.logon_sent()
    }

    pub fn is_logon_time(&self, time: UtcTimestamp) -> bool {
        self.session_settings
            .logon_time
            .contains(&time.timestamp().time())
    }

    fn check_sending_time(&self, sending_time: UtcTimestamp) -> Result<(), VerifyError> {
        let Some(max_latency) = self.session_settings.max_latency else {
            return Ok(());
        };
        let max_latency = chrono::Duration::from_std(max_latency).expect("duration");

        // neg implementation for chrono::Duration modifies secs value,
        // so abs value has to be calculated manually
        let now = Utc::now();
        let sending_timestamp = sending_time.timestamp();
        let abs_time_diff = if now > sending_timestamp {
            now - sending_timestamp
        } else {
            sending_timestamp - now
        };
        if abs_time_diff > max_latency {
            warn!(
                ?abs_time_diff,
                ?max_latency,
                "SendingTime<52> verification failed"
            );
            Err(VerifyError::invalid_time())
        } else {
            Ok(())
        }
    }

    fn is_target_too_high(state: &State<M, S>, msg_seq_num: SeqNum) -> bool {
        msg_seq_num > state.next_target_msg_seq_num()
    }

    fn is_target_too_low(state: &State<M, S>, msg_seq_num: SeqNum) -> bool {
        msg_seq_num < state.next_target_msg_seq_num()
    }

    fn check_comp_id(
        &self,
        sender_comp_id: &FixStr,
        target_comp_id: &FixStr,
    ) -> Result<(), VerifyError> {
        if !self.session_settings.check_comp_id {
            Ok(())
        } else if self.session_settings.session_id.sender_comp_id() != target_comp_id {
            Err(VerifyError::invalid_comp_id(TAG_TARGET_COMP_ID))
        } else if self.session_settings.session_id.target_comp_id() != sender_comp_id {
            Err(VerifyError::invalid_comp_id(TAG_SENDER_COMP_ID))
        } else {
            Ok(())
        }
    }

    fn should_send_reset(&self, state: &State<M, S>) -> bool {
        (self.session_settings.reset_on_logon
            || self.session_settings.reset_on_logout
            || self.session_settings.reset_on_disconnect)
            && state.next_target_msg_seq_num() == 1
            && state.next_sender_msg_seq_num() == 1
    }

    #[instrument(skip_all, err)]
    fn check_logon_state(state: &State<M, S>, msg_type: MsgTypeField) -> Result<(), VerifyError> {
        if (msg_type == MsgTypeBase::Logon && state.reset_sent()) || state.reset_received() {
            trace!("Allowed: Logon with ResetSeqNumFlag(141)=Y sent or received");
            Ok(())
        } else if msg_type == MsgTypeBase::Logon && !state.logon_received() {
            trace!("Allowed: First Logon in session (Logon not received yet)");
            Ok(())
        } else if msg_type != MsgTypeBase::Logon && state.logon_received() {
            trace!("Allowed: Message after Logon received");
            Ok(())
        } else if msg_type == MsgTypeBase::Logout && state.logon_sent() {
            trace!("Allowed: Logout after Logon sent");
            Ok(())
        } else if msg_type != MsgTypeBase::Logout && state.logout_sent_time().is_some() {
            trace!("Allowed: Message after Logout sent");
            Ok(())
        } else if msg_type == MsgTypeBase::SequenceReset {
            trace!("Allowed: SequenceReset<4>");
            Ok(())
        } else if msg_type == MsgTypeBase::Reject {
            trace!("Allowed: Reject<3>");
            Ok(())
        } else {
            warn!(
                state.reset_sent = state.reset_sent(),
                state.reset_received = state.reset_received(),
                state.logon_received = state.logon_received(),
                state.logon_sent = state.logon_sent(),
                state.logout_sent = ?state.logout_sent_time(),
                "Not allowed: Invalid session state",
            );
            Err(VerifyError::InvalidLogonState)
        }
    }

    #[instrument(skip_all, err)]
    #[expect(clippy::await_holding_refcell_ref)]
    // Make sure `state` is dropped before await points, see
    // https://github.com/rust-lang/rust-clippy/issues/6353
    async fn verify(
        &self,
        msg: Box<M>,
        check_too_high: bool,
        check_too_low: bool,
    ) -> Result<(), VerifyError> {
        let msg_type = msg.msg_type();
        let sender_comp_id = msg.sender_comp_id();
        let target_comp_id = msg.target_comp_id();
        let sending_time = msg.sending_time();
        let msg_seq_num = msg.msg_seq_num();
        let poss_dup_flag = msg.poss_dup_flag();
        let orig_sending_time = msg.orig_sending_time();

        let mut state = self.state.borrow_mut();
        let reset_received = state.reset_received();

        Self::check_logon_state(&state, msg_type)?;
        self.check_sending_time(sending_time)?;
        self.check_comp_id(sender_comp_id, target_comp_id)?;

        if check_too_high && !reset_received && Self::is_target_too_high(&state, msg_seq_num) {
            warn!(
                "Target MsgSeqNum too high, expected {}, got {msg_seq_num}",
                state.next_target_msg_seq_num()
            );
            state.enqueue_msg(msg);
            Err(VerifyError::target_seq_num_too_high(msg_seq_num))
        } else if check_too_low && !reset_received && Self::is_target_too_low(&state, msg_seq_num) {
            if poss_dup_flag.unwrap_or(false) {
                if msg_type != MsgTypeBase::SequenceReset {
                    let Some(orig_sending_time) = orig_sending_time else {
                        warn!("Target too low (orig sending time missing)");
                        return Err(VerifyError::missing_orig_time());
                    };
                    if orig_sending_time.timestamp() > sending_time.timestamp() {
                        error!("Target too low (invalid orig sending time)");
                        return Err(VerifyError::invalid_orig_time());
                    }
                }
                warn!("Target too low (duplicate)");
                Err(VerifyError::Duplicate)
            } else {
                error!(
                    expected_msg_seq_num = state.next_target_msg_seq_num(),
                    "Target too low"
                );
                Err(VerifyError::SeqNumTooLow {
                    msg_seq_num,
                    next_target_msg_seq_num: state.next_target_msg_seq_num(),
                })
            }
        } else {
            if let Some(resend_range) = state.resend_range()
                && check_too_high
            {
                let begin_seq_num = *resend_range.start();
                let end_seq_num = *resend_range.end();

                if msg_seq_num >= end_seq_num {
                    info!(
                        begin_seq_num,
                        end_seq_num, "Resend request has been satisfied"
                    );
                    state.reset_resend_range();
                }
            }
            drop(state);

            let (sender, receiver) = tokio::sync::oneshot::channel();
            match msg.msg_cat() {
                MsgCat::Admin => {
                    self.emitter
                        .send(FixEventInternal::AdmMsgIn(Some(msg), Some(sender)))
                        .await
                }
                MsgCat::App => {
                    self.emitter
                        .send(FixEventInternal::AppMsgIn(Some(msg), Some(sender)))
                        .await
                }
            }
            if let Ok(input_responder_message) = receiver.await {
                return Err(input_responder_message.into());
            }

            Ok(())
        }
    }

    pub(crate) fn send_logon_request(&self, state: &mut State<M, S>) {
        if self.session_settings.reset_on_logon {
            state.reset();
        }

        let next_expected_msg_seq_num = if self.session_settings.enable_next_expected_msg_seq_num {
            let next_expected_msg_seq_num = state.next_sender_msg_seq_num();
            state.set_last_expected_logon_next_seq_num(next_expected_msg_seq_num);
            Some(next_expected_msg_seq_num)
        } else {
            None
        };

        self.send(AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: EncryptMethodBase::None as Int,
            heart_bt_int: self.heartbeat_interval.get().try_into().unwrap_or(Int::MAX),
            reset_seq_num_flag: self.should_send_reset(state).then_some(true),
            next_expected_msg_seq_num,
            // TODO: should be conditional on FIXT version
            default_appl_ver_id: Some(Cow::Borrowed(DEFAULT_APPL_VER_ID)),
            session_status: None,
        }));
    }

    fn send_logon_response(
        &self,
        state: &mut State<M, S>,
        next_expected_msg_seq_num: Option<SeqNum>,
    ) {
        if self.session_settings.reset_on_logon {
            state.reset();
        }

        self.send(AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: EncryptMethodBase::None as Int,
            heart_bt_int: self.heartbeat_interval.get().try_into().unwrap_or(Int::MAX),
            reset_seq_num_flag: self.should_send_reset(state).then_some(true),
            next_expected_msg_seq_num,
            // TODO: should be conditional on FIXT version
            default_appl_ver_id: Some(Cow::Borrowed(DEFAULT_APPL_VER_ID)),
            session_status: None,
        }));

        state.set_last_received_time(Instant::now());
        state.set_logon_sent(true);
    }

    pub(crate) fn send_logout(
        &self,
        state: &mut State<M, S>,
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    ) {
        self.send(AdminBase::Logout(LogoutBase {
            session_status,
            text: text.map(Cow::Owned),
        }));
        state.set_logout_sent_time(true);
    }

    fn send_reject(
        &self,
        state: &mut State<M, S>,
        ref_msg_type: Option<FixString>,
        ref_seq_num: SeqNum,
        reason: SessionRejectReasonField,
        text: FixString,
        ref_tag_id: Option<Int>,
    ) {
        let is_logon_or_seq_reset = ref_msg_type
            .as_deref()
            .and_then(|s| MsgTypeField::from_bytes(s.as_bytes()).ok())
            .is_some_and(|mt| mt == MsgTypeBase::Logon || mt == MsgTypeBase::SequenceReset);
        if !is_logon_or_seq_reset && ref_seq_num == state.next_target_msg_seq_num() {
            state.incr_next_target_msg_seq_num();
        }

        info!("Message {ref_seq_num} Rejected: {reason:?} (tag={ref_tag_id:?})");

        self.send(AdminBase::Reject(RejectBase {
            ref_seq_num,
            ref_tag_id,
            ref_msg_type: ref_msg_type.map(Cow::Owned),
            session_reject_reason: Some(reason.into()),
            text: Some(Cow::Owned(text)),
        }));
    }

    fn send_sequence_reset(&self, seq_num: SeqNum, new_seq_num: SeqNum) {
        let mut sequence_reset = Box::new(M::from_admin(
            HeaderBase::default(),
            AdminBase::SequenceReset(SequenceResetBase {
                gap_fill_flag: Some(true),
                new_seq_no: new_seq_num,
            }),
        ));

        sequence_reset.set_msg_seq_num(seq_num);
        sequence_reset.set_poss_dup_flag(Some(true));
        let now = UtcTimestamp::now();
        sequence_reset.set_sending_time(now);
        sequence_reset.set_orig_sending_time(Some(now));

        info!(seq_num, new_seq_num, "SequenceReset sent (gap fill)");
        self.send_raw(sequence_reset);
    }

    #[instrument(level = "trace", skip_all, fields(too_high_msg_seq_num))]
    fn send_resend_request(&self, state: &mut State<M, S>, too_high_msg_seq_num: SeqNum) {
        let begin_seq_no = state.next_target_msg_seq_num();
        let mut end_seq_no = too_high_msg_seq_num.saturating_sub(1);

        if let Some(queued_lowest) = state.lowest_queued_seq_num()
            && queued_lowest > begin_seq_no
        {
            let new_end_seq_no = queued_lowest.saturating_sub(1);
            if new_end_seq_no < end_seq_no {
                trace!(
                    new_end_seq_no = queued_lowest,
                    prev_end_seq_no = end_seq_no,
                    "clamping resend request upper bound to queued gap"
                );
                end_seq_no = new_end_seq_no;
            }
        }

        if begin_seq_no > end_seq_no {
            trace!(
                begin_seq_no,
                "ResendRequest suppressed; queued messages cover the gap"
            );
            return;
        }

        trace!(begin_seq_no, end_seq_no);

        self.send(AdminBase::ResendRequest(ResendRequestBase {
            begin_seq_no,
            end_seq_no,
        }));

        state.set_resend_range(begin_seq_no..=end_seq_no);
    }

    /// Send admin message constructed from base types.
    fn send(&self, admin: AdminBase<'static>) {
        self.send_raw(Box::new(M::from_admin(HeaderBase::default(), admin)));
    }

    fn message_from_bytes(
        bytes: &[u8],
    ) -> Result<Box<M>, easyfix_core::deserializer::DeserializeError> {
        let (_, raw) = raw_message(bytes)?;
        Ok(Box::new(M::from_raw_message(raw)?))
    }

    /// Send a fully constructed message.
    fn send_raw(&self, msg: Box<M>) {
        if let Err(msg) = self.sender.send_raw(msg) {
            // This should never happen.
            // See `fn input_loop()` and `fn output_loop()` in connection.rs
            // Output loop always waits for input loop to finish, so it's not
            // possible that output queue is closed when input message is still
            // being processed.
            unreachable!(
                "Can't send message {:?}/{} - output stream is closed",
                msg.msg_type(),
                msg.msg_seq_num()
            );
        }
    }

    #[expect(clippy::await_holding_refcell_ref)]
    // Make sure `state` is dropped before await points, see
    // https://github.com/rust-lang/rust-clippy/issues/6353
    #[instrument(skip_all)]
    pub(crate) async fn emit_logout(&self, reason: DisconnectReason) {
        info!(?reason);

        let mut state = self.state.borrow_mut();

        if state.logon_received() || state.logon_sent() {
            state.set_logon_received(false);
            state.set_logon_sent(false);
            drop(state);

            self.emitter
                .send(FixEventInternal::Logout(
                    self.session_settings.session_id.clone(),
                    reason,
                ))
                .await;
        } else {
            info!(
                "Logout not emitted: session was never established \
                (neither Logon received nor Logon sent)"
            );
        }
    }

    #[instrument(
        skip_all,
        fields(?reason, reset = self.session_settings.reset_on_disconnect),
        ret
    )]
    pub(crate) fn disconnect(&self, state: &mut State<M, S>, reason: DisconnectReason) {
        if state.disconnected() {
            info!("already disconnected");
            return;
        }

        // XXX: Emit logout in connection handler instead of here,
        //      so `Logout` event will be delivered after Logout
        //      message instead of randomly before or after.
        // self.emit_logout().await;

        state.disconnect(self.session_settings.reset_on_disconnect);

        self.sender.disconnect(reason);

        // Notify input loop to exit immediately
        if let Some(tx) = self.disconnect_notify.borrow_mut().take() {
            let _ = tx.send(());
        }
    }

    #[instrument(level = "trace", skip_all)]
    fn resend_range(
        &self,
        state: &mut State<M, S>,
        begin_seq_num: SeqNum,
        mut end_seq_num: SeqNum,
    ) {
        info!("resend range: ({begin_seq_num}, {end_seq_num})");
        let next_sender_msg_seq_num = state.next_sender_msg_seq_num();
        if end_seq_num == 0 || end_seq_num >= next_sender_msg_seq_num {
            end_seq_num = next_sender_msg_seq_num - 1;
            info!("adjust end_seq_num to {end_seq_num}");
        }

        // Just do a gap fill when messages aren't persisted
        if !self.session_settings.persist {
            let next_sender_msg_seq_num = state.next_sender_msg_seq_num();
            end_seq_num += 1;
            if end_seq_num > next_sender_msg_seq_num {
                end_seq_num = next_sender_msg_seq_num;
            }
            self.send_sequence_reset(begin_seq_num, end_seq_num);
            return;
        }

        let mut gap_fill_range = None;
        info!("fetch messages range from {begin_seq_num} to {end_seq_num}");
        for msg_str in state.fetch_range(begin_seq_num..=end_seq_num) {
            // TODO: log error! and resend as gap fill instead of unwrap
            let mut msg = match Self::message_from_bytes(msg_str) {
                Ok(msg) => msg,
                Err(err) => {
                    error!(%err, "Failed to decode message bytes");
                    continue;
                }
            };
            if msg.resend_as_gap_fill() {
                trace!(
                    "Message {:?}/{} changed to gap fill",
                    msg.msg_type(),
                    msg.msg_seq_num()
                );
                let seq_num = msg.msg_seq_num();
                gap_fill_range.get_or_insert((seq_num, seq_num - 1)).1 += 1;
            } else {
                if let Some((begin_seq_num, end_seq_num)) = gap_fill_range.take() {
                    trace!("Resending messages from {begin_seq_num} to {end_seq_num} as gap fill");
                    self.send_sequence_reset(begin_seq_num, end_seq_num + 1);
                }
                trace!(
                    "Resending message {:?}/{}",
                    msg.msg_type(),
                    msg.msg_seq_num()
                );
                msg.set_orig_sending_time(Some(msg.sending_time()));
                // Reset sending_time so that the sending task assigns a new timestamp before serialization
                msg.set_sending_time(UtcTimestamp::MIN_UTC);
                msg.set_poss_dup_flag(Some(true));
                // TODO: emit event!
                self.send_raw(msg);
            }
        }
        if let Some((begin_seq_num, end_seq_num)) = gap_fill_range {
            info!("Resending messages from {begin_seq_num} to {end_seq_num} as gap fill");
            self.send_sequence_reset(begin_seq_num, end_seq_num + 1);
        }
    }

    async fn on_heartbeat(&self, message: Box<M>) -> Result<(), VerifyError> {
        // Got Heartbeat, verify against grace period test requests.
        trace!("got heartbeat");

        let Some(AdminBase::Heartbeat(heartbeat)) = message.try_as_admin() else {
            unreachable!();
        };
        let test_req_id = if self.session_settings.verify_test_request_id {
            heartbeat.test_req_id.map(Cow::into_owned)
        } else {
            None
        };

        self.verify(message, true, true).await?;

        let mut state = self.state.borrow_mut();

        if let Some(ref test_req_id) = test_req_id {
            state.validate_grace_period_test_req_id(test_req_id);
        }

        state.incr_next_target_msg_seq_num();

        Ok(())
    }

    /// Got TestRequest, answer with Heartbeat and return.
    async fn on_test_request(&self, message: Box<M>) -> Result<(), VerifyError> {
        trace!("on_test_request");

        let Some(AdminBase::TestRequest(test_request)) = message.try_as_admin() else {
            unreachable!();
        };

        let test_req_id = test_request.test_req_id.into_owned();

        self.verify(message, true, true).await?;

        trace!("Send Heartbeat");
        self.send(AdminBase::Heartbeat(HeartbeatBase {
            test_req_id: Some(Cow::Owned(test_req_id)),
        }));

        self.state.borrow_mut().incr_next_target_msg_seq_num();

        Ok(())
    }

    async fn on_resend_request(&self, msg: Box<M>) -> Result<(), VerifyError> {
        trace!("on_resend_request");

        let Some(AdminBase::ResendRequest(resend_request)) = msg.try_as_admin() else {
            unreachable!();
        };

        let begin_seq_no = resend_request.begin_seq_no;
        let end_seq_no = resend_request.end_seq_no;

        let msg_seq_num = msg.msg_seq_num();

        // XXX: Do not check if message is too high here - in such case message
        //      would be enqueued for later processing. This might lead to
        //      dedclock when both sides would wait for resended messages.
        //
        //      Instead just resend requested messages and after that, send
        //      ResendRequest for missing messages.
        self.verify(msg, false, true).await?;

        info!("Received ResendRequest FROM: {begin_seq_no} TO: {end_seq_no}");

        let mut state = self.state.borrow_mut();

        self.resend_range(&mut state, begin_seq_no, end_seq_no);

        if Self::is_target_too_high(&state, msg_seq_num) {
            // XXX: This message will be ignored during queued messages
            //      processing, it's enqueued only to maintain proper sequence
            //      numbers.
            let mut placeholder = Box::new(M::from_admin(
                HeaderBase::default(),
                AdminBase::ResendRequest(ResendRequestBase {
                    begin_seq_no,
                    end_seq_no,
                }),
            ));
            placeholder.set_msg_seq_num(msg_seq_num);
            state.enqueue_msg(placeholder);
            return Err(VerifyError::ResendRequest { msg_seq_num });
        } else if state.next_target_msg_seq_num() == msg_seq_num {
            state.incr_next_target_msg_seq_num();
        }

        Ok(())
    }

    async fn on_reject(&self, message: Box<M>) -> Result<(), VerifyError> {
        trace!("on_reject");

        self.verify(message, false, true).await?;

        self.state.borrow_mut().incr_next_target_msg_seq_num();

        Ok(())
    }

    async fn on_sequence_reset(&self, message: Box<M>) -> Result<(), VerifyError> {
        trace!("on_sequence_reset");

        let Some(AdminBase::SequenceReset(sequence_reset)) = message.try_as_admin() else {
            unreachable!();
        };

        let ref_msg_type = message.msg_type().as_fix_str().to_owned();
        let ref_seq_num = message.msg_seq_num();
        let is_gap_fill = sequence_reset.gap_fill_flag.unwrap_or(false);
        let new_seq_no = sequence_reset.new_seq_no;

        self.verify(message, is_gap_fill, is_gap_fill).await?;

        let mut state = self.state().borrow_mut();

        if new_seq_no > state.next_target_msg_seq_num() {
            info!("Set next target MsgSeqNo to {new_seq_no}");
            state.set_next_target_msg_seq_num(new_seq_no);
        } else if new_seq_no < state.next_sender_msg_seq_num() {
            let reject_reason = SessionRejectReasonBase::ValueIsIncorrect;
            let tag = Int::from(TAG_NEW_SEQ_NO);
            let text = format!("{reject_reason:?} (tag={tag}) - NewSeqNum too low");
            self.send_reject(
                &mut state,
                Some(ref_msg_type),
                ref_seq_num,
                reject_reason.into(),
                FixString::from_ascii_lossy(text.into_bytes()),
                Some(tag),
            );
        }

        Ok(())
    }

    async fn on_logout(&self, message: Box<M>) -> Result<DisconnectReason, VerifyError> {
        if self.session_settings.verify_logout {
            self.verify(message, true, true).await?;
        } else if let Err(e) = self.verify(message, false, false).await {
            // Nothing more we can do as client is disconnecting anyway
            error!("logout failed: {e}");
        }

        let mut state = self.state.borrow_mut();
        let disconnect_reason = if state.logout_sent_time().is_some() {
            info!("received logout response");
            DisconnectReason::LocalRequestedLogout
        } else {
            info!("received logout request");
            self.send_logout(
                &mut state,
                Some(SessionStatusBase::SessionLogoutComplete.into()),
                Some(FixString::from_ascii_lossy(b"Responding".to_vec())),
            );
            info!("sending logout response");
            DisconnectReason::RemoteRequestedLogout
        };

        state.incr_next_target_msg_seq_num();
        if self.session_settings.reset_on_logout {
            state.reset();
        }

        Ok(disconnect_reason)
    }

    #[instrument(level = "trace", skip_all, err, ret)]
    #[expect(clippy::await_holding_refcell_ref)]
    // Make sure `state` is dropped before await points, see
    // https://github.com/rust-lang/rust-clippy/issues/6353
    async fn on_logon(&self, message: Box<M>) -> Result<Option<DisconnectReason>, VerifyError> {
        let (
            enabled,
            initiate,
            should_send_logon,
            reset_received,
            reset_sent,
            reset_seq_num_flag,
            heart_bt_int,
            next_expected_msg_seq_num,
        ) = {
            let state = self.state.borrow_mut();

            let Some(AdminBase::Logon(logon)) = message.try_as_admin() else {
                unreachable!()
            };
            (
                state.enabled(),
                state.initiate(),
                state.should_send_logon(),
                state.reset_received(),
                state.reset_sent(),
                logon.reset_seq_num_flag.unwrap_or(false),
                logon.heart_bt_int,
                logon.next_expected_msg_seq_num,
            )
        };

        if !enabled {
            error!("Session is not enabled for logon");
            return Ok(Some(DisconnectReason::InvalidLogonState));
        }

        if !self.is_logon_time(message.sending_time()) {
            error!("Received logon outside of valid logon time");
            return Ok(Some(DisconnectReason::InvalidLogonState));
        }

        if reset_seq_num_flag {
            self.state.borrow_mut().set_reset_received(true);
        }

        let msg_seq_num = message.msg_seq_num();

        self.verify(message, false, true).await?;

        let enable_next_expected_msg_seq_num =
            self.session_settings.enable_next_expected_msg_seq_num
                && next_expected_msg_seq_num.is_some();

        if reset_seq_num_flag {
            let mut state = self.state.borrow_mut();
            state.set_reset_received(true);
            info!("Logon contains ResetSeqNumFlag=Y, reseting sequence numbers to 1");
            if !state.reset_sent() {
                state.reset();
            }
        } else if reset_sent && msg_seq_num == 1 {
            info!("Inferring ResetSeqNumFlag as sequence number is 1 in response to reset request");
            self.state.borrow_mut().set_reset_received(true);
        }

        if should_send_logon && !reset_received {
            error!("Received logon response before sending request");
            return Ok(Some(DisconnectReason::InvalidLogonState));
        }

        if !initiate && self.session_settings.reset_on_logon {
            self.state.borrow_mut().reset();
        }

        let mut state = self.state.borrow_mut();

        state.set_logon_received(true);

        let next_sender_msg_num_at_logon_received = state.next_sender_msg_seq_num();

        if enable_next_expected_msg_seq_num
            && let Some(next_expected_msg_seq_num) = next_expected_msg_seq_num
        {
            let next_sender_msg_seq_num = state.next_sender_msg_seq_num();
            // Is the 789 we received too high ??
            if next_expected_msg_seq_num > next_sender_msg_seq_num {
                // can't resend what we never sent! something unrecoverable has happened.
                let error_msg = format!(
                    "NextExpectedMsgSeqNum<789> too high \
                            (expected {next_sender_msg_seq_num}, \
                             got {next_expected_msg_seq_num})",
                );
                error!(error_msg);
                let err = FixString::from_ascii_lossy(error_msg.into_bytes());
                self.send_logout(
                    &mut state,
                    Some(SessionStatusBase::ReceivedNextExpectedMsgSeqNumTooHigh.into()),
                    Some(err),
                );
                return Ok(Some(DisconnectReason::InvalidLogonState));
            }
        }

        // Test here that it's not too high (which would result in a resend)
        // and that it's not resetting on logon 34=1
        let is_logon_in_normal_sequence =
            !Self::is_target_too_high(&state, msg_seq_num) || self.session_settings.reset_on_logon;

        if !state.initiate() || (state.reset_received() && !state.reset_sent()) {
            info!("Received logon request");
            if self.settings.heartbeat_interval.is_none() {
                if heart_bt_int <= 0 {
                    return Err(VerifyError::Reject {
                        reason: SessionRejectReasonBase::ValueIsIncorrect,
                        tag: Some(TAG_HEART_BT_INT),
                        disconnect_reason: None,
                    });
                }
                self.heartbeat_interval.set(heart_bt_int as u64);
            }

            if enable_next_expected_msg_seq_num {
                let mut next_expected_target_num = state.next_target_msg_seq_num();
                // we increment for the logon later (after Logon response sent) in this method if and only if in sequence
                if is_logon_in_normal_sequence {
                    // logon was fine take account of it in 789
                    next_expected_target_num += 1;
                }

                info!("Responding to Logon request with tag 789={next_expected_target_num}");
                state.set_last_expected_logon_next_seq_num(next_expected_target_num);
                self.send_logon_response(&mut state, Some(next_expected_target_num));
            } else {
                info!("Responding to Logon request");
                self.send_logon_response(&mut state, None);
            }
        } else {
            info!("Received logon response");
        }

        state.set_reset_sent(false);
        state.set_reset_received(false);

        let mut ret = Ok(None);

        if !is_logon_in_normal_sequence {
            // if 789 was sent then we effectively have already sent a resend request
            if state.is_expected_logon_next_seq_num_sent() {
                // Mark state as if we have already sent a resend request from the logon's 789 (we sent) to infinity.
                // This will supress the resend request in doTargetTooHigh ...
                state.set_reset_range_from_last_expected_logon_next_seq_num();
                info!("Required resend will be suppressed as we are setting tag 789");
            }

            warn!(
                "Target MsgSeqNum too high, expected {}, got {msg_seq_num}",
                state.next_target_msg_seq_num()
            );

            state.enqueue_msg(
                // No need to clone input message. Pass empty message
                // as it will be skipped during enqueued messages processing.
                {
                    let mut placeholder = Box::new(M::from_admin(
                        HeaderBase::default(),
                        AdminBase::Logon(LogonBase {
                            encrypt_method: EncryptMethodBase::None,
                            encrypt_method_raw: 0,
                            heart_bt_int: 0,
                            reset_seq_num_flag: None,
                            next_expected_msg_seq_num: None,
                            default_appl_ver_id: None,
                            session_status: None,
                        }),
                    ));
                    placeholder.set_msg_seq_num(msg_seq_num);
                    placeholder
                },
            );
            ret = Err(VerifyError::ResendRequest { msg_seq_num });
        } else {
            state.incr_next_target_msg_seq_num();
        }

        if enable_next_expected_msg_seq_num
            && let Some(next_expected_msg_seq_num) = next_expected_msg_seq_num
        {
            // is the 789 lower (we checked for higher previously) than our next message after receiving the logon
            if next_expected_msg_seq_num != next_sender_msg_num_at_logon_received {
                let mut end_seq_no = next_sender_msg_num_at_logon_received;

                // TODO: self.resend_range() will handle this !!!
                if !self.session_settings.persist {
                    end_seq_no += 1;
                    let next = state.next_sender_msg_seq_num();
                    if end_seq_no > next {
                        end_seq_no = next;
                    }
                    info!(
                        "Received implicit ResendRequest via Logon FROM: {next_expected_msg_seq_num}, \
                             TO: {next_sender_msg_num_at_logon_received} will be reset"
                    );
                    self.send_sequence_reset(next_expected_msg_seq_num, end_seq_no);
                } else {
                    // resend missed messages
                    info!(
                        "Received implicit ResendRequest via Logon FROM: {next_expected_msg_seq_num} \
                             TO: {next_sender_msg_num_at_logon_received} will be resent"
                    );
                    self.resend_range(&mut state, next_expected_msg_seq_num, end_seq_no)
                }
            }
        }

        if Self::is_logged_on(&state) {
            state.reset_grace_period();
            drop(state);
            self.emitter
                .send(FixEventInternal::Logon(
                    self.session_settings.session_id.clone(),
                    Some(self.sender.clone()),
                ))
                .await;
        }

        ret
    }

    // TODO: restore #[instrument] once msg no longer borrows through the span
    // #[instrument(
    //     name = "on_msg",
    //     level = "trace",
    //     skip_all,
    //     fields(
    //         msg_seq_num = msg.header.msg_seq_num,
    //         msg_type = ?msg.msg_type()
    //         )
    //     )]
    #[expect(clippy::await_holding_refcell_ref)]
    // Make sure `state` is dropped before await points, see
    // https://github.com/rust-lang/rust-clippy/issues/6353
    async fn on_message_in_impl(&self, msg: Box<M>) -> Option<DisconnectReason> {
        let msg_type = msg.msg_type();
        let msg_seq_num = msg.msg_seq_num();
        let name = msg.name();
        let _span = tracing::trace_span!("on_msg", msg_type = name, msg_seq_num).entered();
        trace!(msg_type = format!("{name}<{}>", msg_type.as_fix_str()));

        let result = if let Some(admin) = msg.try_as_admin() {
            match admin {
                AdminBase::Heartbeat(_) => self.on_heartbeat(msg).await,
                AdminBase::TestRequest(_) => self.on_test_request(msg).await,
                AdminBase::ResendRequest(_) => self.on_resend_request(msg).await,
                AdminBase::Reject(_) => self.on_reject(msg).await,
                AdminBase::SequenceReset(_) => self.on_sequence_reset(msg).await,
                AdminBase::Logout(_) => match self.on_logout(msg).await {
                    Ok(disconnect_reason) => return Some(disconnect_reason),
                    Err(e) => Err(e),
                },
                AdminBase::Logon(_) => match self.on_logon(msg).await {
                    Ok(Some(disconnect_reason)) => {
                        return Some(disconnect_reason);
                    }
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                },
            }
        } else {
            self.verify(msg, true, true)
                .await
                .map(|_| self.state.borrow_mut().incr_next_target_msg_seq_num())
        };

        match result {
            Ok(()) => return None,
            Err(VerifyError::Duplicate) => {
                // Duplicate can be ignored
            }
            Err(VerifyError::ResendRequest { msg_seq_num }) => {
                if let Some(resend_range) = self.state.borrow().resend_range() {
                    let begin_seq_num = *resend_range.start();
                    let end_seq_num = *resend_range.end();

                    if !self.session_settings.send_redundant_resend_requests
                        && msg_seq_num >= begin_seq_num
                    {
                        warn!(
                            begin_seq_num,
                            end_seq_num,
                            too_high_msg_seq_num = msg_seq_num,
                            "ResendRequest already sent, suppressing another attempt"
                        );
                        return None;
                    }
                }

                self.send_resend_request(&mut self.state.borrow_mut(), msg_seq_num);
            }
            Err(VerifyError::Reject {
                reason,
                tag,
                disconnect_reason,
            }) => {
                let mut state = self.state().borrow_mut();
                let tag_as_int = tag.map(Int::from);
                self.send_reject(
                    &mut state,
                    Some(msg_type.as_fix_str().to_owned()),
                    msg_seq_num,
                    reason.into(),
                    if let Some(tag) = tag_as_int {
                        FixString::from_ascii_lossy(format!("{reason:?} (tag={tag})").into_bytes())
                    } else {
                        FixString::from_ascii_lossy(format!("{reason:?}").into_bytes())
                    },
                    tag_as_int,
                );

                self.emitter
                    .send(FixEventInternal::DeserializeError(
                        self.session_id().clone(),
                        DeserializeError::Reject {
                            msg_type: Some(msg_type.as_fix_str().to_owned()),
                            seq_num: msg_seq_num,
                            tag,
                            reason: reason.into(),
                        },
                    ))
                    .await;

                if let Some(disconnect_reason) = disconnect_reason {
                    self.send_logout(&mut state, None, None);
                    return Some(disconnect_reason);
                }
            }
            Err(e @ VerifyError::SeqNumTooLow { .. }) => {
                let mut state = self.state.borrow_mut();
                self.send_logout(
                    &mut state,
                    Some(SessionStatusBase::ReceivedMsgSeqNumTooLow.into()),
                    Some(FixString::from_ascii_lossy(e.to_string().into_bytes())),
                );
                return Some(DisconnectReason::MsgSeqNumTooLow);
            }
            Err(VerifyError::InvalidLogonState) => {
                error!("disconnecting because of invalid logon state");
                return Some(DisconnectReason::InvalidLogonState);
            }
            Err(VerifyError::ApplicationForcedReject {
                ref_msg_type,
                ref_seq_num,
                reason,
                text,
                ref_tag_id,
            }) => {
                warn!("Rejected by application ({reason:?}: {text})");
                self.send_reject(
                    &mut self.state().borrow_mut(),
                    Some(ref_msg_type),
                    ref_seq_num,
                    reason,
                    text,
                    ref_tag_id,
                );
            }
            Err(VerifyError::ApplicationForcedLogout {
                session_status,
                text,
                disconnect,
            }) => {
                error!(
                    "Rejected by application with Logout<5> ({})",
                    text.as_ref().map(FixString::as_utf8).unwrap_or_default()
                );
                let mut state = self.state.borrow_mut();
                self.send_logout(&mut state, session_status, text);
                if disconnect {
                    return Some(DisconnectReason::ApplicationForcedDisconnect);
                }
            }
            Err(VerifyError::ApplicationForcedDisconnect { reason }) => {
                error!("Disconnected by application: {reason:?}");
                return Some(DisconnectReason::ApplicationForcedDisconnect);
            }
            Err(VerifyError::ApplicationAbortedProcessing) => {
                self.state().borrow_mut().incr_next_target_msg_seq_num();
            }
        }

        None
    }

    pub async fn on_message_in(&self, msg: Box<M>) -> Option<DisconnectReason> {
        if !self.session_settings.verify_test_request_id {
            self.state.borrow_mut().reset_grace_period();
        }

        if let Some(disconnect_reason) = self.on_message_in_impl(msg).await {
            return Some(disconnect_reason);
        }
        loop {
            let Some(msg) = self.state.borrow_mut().retrieve_msg() else {
                break;
            };

            debug!("Processing queued message {}", msg.msg_seq_num());

            if msg.msg_type() == MsgTypeBase::Logon || msg.msg_type() == MsgTypeBase::ResendRequest
            {
                debug!(msg_type = ?msg.msg_type(), "message already processed");
                // Logon and ResendRequest processing has already been done,
                // just increment the target sequence nummber.
                self.state.borrow_mut().incr_next_target_msg_seq_num();
            } else if let Some(disconnect_reason) = self.on_message_in_impl(msg).await {
                return Some(disconnect_reason);
            }
        }
        None
    }

    pub async fn on_message_out(&self, msg: Box<M>) -> Option<Box<M>> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        match msg.msg_cat() {
            MsgCat::Admin => {
                self.emitter
                    .send(FixEventInternal::AdmMsgOut(
                        Some(msg),
                        Responder::new(sender),
                    ))
                    .await;
                // TODO: maybe change unwrap() to None ?
                Some(receiver.await.unwrap())
            }
            MsgCat::App => {
                self.emitter
                    .send(FixEventInternal::AppMsgOut(
                        Some(msg),
                        Responder::new(sender),
                    ))
                    .await;
                #[expect(clippy::manual_ok_err)]
                match receiver.await {
                    Ok(msg) => Some(msg),
                    Err(_gap_fill) => {
                        // TODO: GAP FILL!
                        // let mut header = self.new_header(MsgType::SequenceReset);
                        // header.msg_seq_num = msg.header.msg_seq_num;
                        // Ok(Some(Box::new(Message {
                        //     header,
                        //     body: Message::SequenceReset(SequenceReset {
                        //         gap_fill_flag: Some(true),
                        //         new_seq_no: msg.header.msg_seq_num + 1,
                        //     }),
                        //     trailer: self.new_trailer(),
                        // })))
                        None
                    } // TODO: Err(_no_gap_fill) => None,
                }
            }
        }
    }

    pub async fn on_deserialize_error(&self, error: DeserializeError) -> Option<DisconnectReason> {
        trace!("on_deserialize_error");

        // TODO: if msg_type is logon, handle missing CompId separately (disconnect)

        // Failed to parse the message. Discard the message.
        // Processing of the next valid FIX message will cause detection of
        // a sequence gap and a ResendRequest<2> will be generated.
        // TODO: Make sure infinite resend loop is not possible
        let text = FixString::from_ascii_lossy(error.to_string().into_bytes());
        error!(deserialize_error = %text);

        match &error {
            DeserializeError::GarbledMessage(reason) => error!("Garbled message: {reason}"),
            DeserializeError::Logout => {
                let mut state = self.state.borrow_mut();
                self.send_logout(
                    &mut state,
                    None,
                    Some(FixString::from_ascii_lossy(
                        b"MsgSeqNum(34) not found".to_vec(),
                    )),
                );
                return Some(DisconnectReason::MsgSeqNumNotFound);
            }
            DeserializeError::Reject {
                msg_type,
                seq_num,
                tag,
                reason,
            } => self.send_reject(
                &mut self.state().borrow_mut(),
                msg_type.clone(),
                *seq_num,
                *reason,
                text,
                tag.map(Int::from),
            ),
        }

        self.emitter
            .send(FixEventInternal::DeserializeError(
                self.session_id().clone(),
                error,
            ))
            .await;

        None
    }

    pub async fn on_in_timeout(self: &Rc<Self>) -> bool {
        trace!("on_in_timeout");

        let mut state = self.state().borrow_mut();
        let timeout_cnt_limit = self.settings.auto_disconnect_after_no_heartbeat;
        if timeout_cnt_limit > 0 && state.input_timeout_cnt() >= timeout_cnt_limit as usize {
            warn!("Grace period is over");
            return true;
        }

        // Use current time as TestReqId as recommended in FIX Session
        // Protocol (FIX) Version 1.1 Errata March 2008
        let test_req_id = FixString::from_ascii_lossy(
            format!("{}", Utc::now().format("%Y%m%d-%H:%M:%S.%f")).into_bytes(),
        );
        state.register_grace_period_test_req_id(test_req_id.clone());

        self.send(AdminBase::TestRequest(TestRequestBase {
            test_req_id: Cow::Owned(test_req_id),
        }));

        false
    }

    pub async fn on_out_timeout(self: &Rc<Self>) {
        trace!("on_out_timeout");
        self.send(AdminBase::Heartbeat(HeartbeatBase { test_req_id: None }));
    }

    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.heartbeat_interval.get())
    }

    pub fn logout_deadline(&self) -> Option<Instant> {
        let logout_sent_time = self.state.borrow().logout_sent_time()?;

        Some(
            logout_sent_time
                .checked_add(self.settings.auto_disconnect_after_no_logout)
                // better disconnect immediately than panic on overflow
                .unwrap_or(logout_sent_time),
        )
    }
}
