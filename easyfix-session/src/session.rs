use std::{cell::RefCell, rc::Rc};

use easyfix_messages::{
    deserializer::DeserializeError,
    fields::{
        DefaultApplVerId, EncryptMethod, FixStr, FixString, Int, MsgType, SeqNum,
        SessionRejectReason, Utc, UtcTimestamp,
    },
    messages::{
        FieldTag, FixtMessage, Heartbeat, Logon, Logout, Message, MsgCat, Reject, ResendRequest,
        SequenceReset, TestRequest,
    },
};
use tokio::time::{Duration, Instant};
use tracing::{error, info, instrument, trace, warn};

use crate::{
    application::{Emitter, FixEventInternal, InputResponderMsg, Responder},
    messages_storage::MessagesStorage,
    new_header, new_trailer,
    session_id::SessionId,
    session_state::State,
    settings::{SessionSettings, Settings},
    DisconnectReason, Sender,
};

#[derive(Debug, thiserror::Error)]
enum VerifyError {
    #[error("Message already received")]
    Duplicate,
    #[error("Too high target sequence number {msg_seq_num}")]
    ResendRequest { msg_seq_num: SeqNum },
    #[error("Reject due to {reason:?} (tag={tag:?}, logout={logout})")]
    Reject {
        reason: SessionRejectReason,
        tag: Option<FieldTag>,
        logout: bool,
    },
    #[error("Invalid logon state")]
    InvalidLogonState,
    #[error("MsgSeqNum too low, expected {next_target_msg_seq_num}, got {msg_seq_num}")]
    SeqNumTooLow {
        msg_seq_num: SeqNum,
        next_target_msg_seq_num: SeqNum,
    },
    #[error("User rejected ({reason:?}: {text})")]
    UserForcedReject {
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReason,
        text: FixString,
        ref_tag_id: Option<i64>,
    },
    #[error("User rejected with Logout<5> ({})", .text.as_ref().map(FixString::as_utf8).unwrap_or_default())]
    UserForcedLogout {
        text: Option<FixString>,
        disconnect: bool,
    },
    #[error("User disconnected: {reason:?}")]
    UserForcedDisconnect { reason: Option<String> },
}

impl VerifyError {
    fn invalid_time() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReason::SendingtimeAccuracyProblem,
            tag: None,
            logout: false,
        }
    }

    fn invalid_comp_id() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReason::CompidProblem,
            tag: None,
            logout: true,
        }
    }

    fn target_seq_num_too_high(msg_seq_num: SeqNum) -> VerifyError {
        VerifyError::ResendRequest { msg_seq_num }
    }

    fn missing_orig_time() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReason::RequiredTagMissing,
            tag: Some(FieldTag::OrigSendingTime),
            logout: false,
        }
    }

    fn invalid_orig_time() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReason::SendingtimeAccuracyProblem,
            tag: None,
            logout: true,
        }
    }
}

trait MessageExt {
    fn resend_as_gap_fill(&self) -> bool;
}

impl MessageExt for FixtMessage {
    fn resend_as_gap_fill(&self) -> bool {
        matches!(self.msg_cat(), MsgCat::Admin) && !matches!(self.msg_type(), MsgType::Reject)
    }
}

#[derive(Debug)]
pub(crate) struct Session<S> {
    // XXX: To avoid borrow errors, borrow state only in async fn,
    //      and in regular fn pass it by ref as argument.
    state: Rc<RefCell<State<S>>>,
    sender: Sender,
    settings: Settings,
    session_settings: SessionSettings,
    emitter: Emitter,
}

impl<S: MessagesStorage> Session<S> {
    pub(crate) fn new(
        settings: Settings,
        session_settings: SessionSettings,
        state: Rc<RefCell<State<S>>>,
        sender: Sender,
        emitter: Emitter,
    ) -> Session<S> {
        Session {
            state,
            settings,
            session_settings,
            sender,
            emitter,
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_settings.session_id
    }

    pub(crate) fn state(&self) -> &Rc<RefCell<State<S>>> {
        &self.state
    }

    pub fn is_logged_on(state: &State<S>) -> bool {
        state.logon_received() && state.logon_sent()
    }

    pub fn is_logon_time(&self, time: UtcTimestamp) -> bool {
        self.session_settings
            .logon_time
            .contains(&time.timestamp().time())
    }

    fn is_good_time(&self, sending_time: UtcTimestamp) -> bool {
        if !self.session_settings.check_latency {
            return true;
        }

        // neg implementation for chrono::Duration modifies secs value,
        // so abs value has to be calculated manually
        let now = Utc::now();
        let sending_timestamp = sending_time.timestamp();
        let abs_time_diff = if now > sending_timestamp {
            now - sending_timestamp
        } else {
            sending_timestamp - now
        };
        abs_time_diff
            <= chrono::Duration::from_std(self.session_settings.max_latency).expect("duration")
    }

    fn is_target_too_high(state: &State<S>, msg_seq_num: SeqNum) -> bool {
        msg_seq_num > state.next_target_msg_seq_num()
    }

    fn is_target_too_low(state: &State<S>, msg_seq_num: SeqNum) -> bool {
        msg_seq_num < state.next_target_msg_seq_num()
    }

    fn is_correct_comp_id(&self, sender_comp_id: &FixStr, target_comp_id: &FixStr) -> bool {
        if !self.session_settings.check_comp_id {
            return true;
        }

        self.session_settings.session_id.sender_comp_id() == target_comp_id
            && self.session_settings.session_id.target_comp_id() == sender_comp_id
    }

    fn should_send_reset(&self, state: &State<S>) -> bool {
        (self.session_settings.reset_on_logon
            || self.session_settings.reset_on_logout
            || self.session_settings.reset_on_disconnect)
            && state.next_target_msg_seq_num() == 1
            && state.next_sender_msg_seq_num() == 1
    }

    fn valid_logon_state(state: &State<S>, msg_type: MsgType) -> bool {
        if (msg_type == MsgType::Logon && state.reset_sent()) || state.reset_received() {
            return true;
        }
        if (msg_type == MsgType::Logon && !state.logon_received())
            || (msg_type != MsgType::Logon && state.logon_received())
        {
            return true;
        }
        if msg_type == MsgType::Logout && state.logon_sent() {
            return true;
        }
        if msg_type != MsgType::Logout && state.logout_sent() {
            return true;
        }
        if msg_type == MsgType::SequenceReset {
            return true;
        }
        if msg_type == MsgType::Reject {
            return true;
        }

        false
    }

    #[instrument(level = "trace", skip_all, err, ret)]
    async fn verify(
        &self,
        msg: Box<FixtMessage>,
        check_too_high: bool,
        check_too_low: bool,
    ) -> Result<(), VerifyError> {
        let msg_type = msg.header.msg_type;

        let sender_comp_id = &msg.header.sender_comp_id;
        let target_comp_id = &msg.header.target_comp_id;
        let sending_time = msg.header.sending_time;
        let msg_seq_num = msg.header.msg_seq_num;

        let state = self.state.borrow();

        if !Self::valid_logon_state(&state, msg.header.msg_type) {
            error!("Invalid logon state");
            Err(VerifyError::InvalidLogonState)
        } else if !self.is_good_time(sending_time) {
            warn!("SendingTime<52> verification failed");
            Err(VerifyError::invalid_time())
        } else if !self.is_correct_comp_id(sender_comp_id, target_comp_id) {
            error!("CompID verification failed");
            Err(VerifyError::invalid_comp_id())
        } else if check_too_high && Self::is_target_too_high(&state, msg_seq_num) {
            warn!(
                "Target MsgSeqNum too high, expected {}, got {msg_seq_num}",
                state.next_target_msg_seq_num()
            );
            drop(state);
            self.state.borrow_mut().enqueue_msg(msg);
            Err(VerifyError::target_seq_num_too_high(msg_seq_num))
        } else if check_too_low && Self::is_target_too_low(&state, msg_seq_num) {
            if msg.header.poss_dup_flag.unwrap_or(false) {
                if msg_type != MsgType::SequenceReset {
                    let Some(orig_sending_time) = msg.header.orig_sending_time else {
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
                error!("Target too low");
                Err(VerifyError::SeqNumTooLow {
                    msg_seq_num,
                    next_target_msg_seq_num: state.next_target_msg_seq_num(),
                })
            }
        } else {
            let resend_range_opt = state.resend_range();
            drop(state);

            if let Some(resend_range) = resend_range_opt {
                if check_too_high {
                    let begin_seq_num = *resend_range.start();
                    let end_seq_num = *resend_range.end();

                    if msg_seq_num >= end_seq_num {
                        info!("Resend request from {begin_seq_num} to {end_seq_num} has been satisfied");
                        self.state.borrow_mut().set_resend_range(None);
                    }
                }
            }

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
            match receiver.await {
                Ok(InputResponderMsg::Reject {
                    ref_msg_type,
                    ref_seq_num,
                    reason,
                    text,
                    ref_tag_id,
                }) => {
                    return Err(VerifyError::UserForcedReject {
                        ref_msg_type,
                        ref_seq_num,
                        reason,
                        text,
                        ref_tag_id,
                    })
                }
                Ok(InputResponderMsg::Logout { text, disconnect }) => {
                    return Err(VerifyError::UserForcedLogout { text, disconnect })
                }
                Ok(InputResponderMsg::Disconnect { reason }) => {
                    return Err(VerifyError::UserForcedDisconnect { reason })
                }
                Err(_) => {}
            }

            Ok(())
        }
    }

    pub(crate) fn send_logon_request(&self, state: &mut State<S>) {
        if self.session_settings.reset_on_logon {
            state.reset();
        }

        self.send(Box::new(Message::Logon(Logon {
            // encrypt_method: EncryptMethod::None,
            encrypt_method: EncryptMethod::NoneOther,
            heart_bt_int: state.heart_bt_int(),
            reset_seq_num_flag: self.should_send_reset(state).then_some(true),
            next_expected_msg_seq_num: if self.session_settings.enable_next_expected_msg_seq_num {
                let next_expected_msg_seq_num = state.next_sender_msg_seq_num();
                state.set_last_expected_logon_next_seq_num(next_expected_msg_seq_num);
                Some(next_expected_msg_seq_num)
            } else {
                None
            },
            default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
            ..Default::default()
        })));
    }

    fn send_logon_response(&self, state: &mut State<S>, next_expected_msg_seq_num: Option<SeqNum>) {
        if self.session_settings.reset_on_logon {
            state.reset();
        }

        self.send(Box::new(Message::Logon(Logon {
            encrypt_method: EncryptMethod::NoneOther,
            // TODO: option to use predefined OR the value from Logon request
            heart_bt_int: state.heart_bt_int(),
            reset_seq_num_flag: self.should_send_reset(state).then_some(true),
            next_expected_msg_seq_num,
            // TODO: if self.session_settings.session_id().is_fixt()
            // default_appl_ver_id: self.sender_default_appl_ver_id().to_owned(),
            default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
            ..Default::default()
        })));

        state.set_last_received_time(Instant::now());
        state.set_test_request(0);
        state.set_logon_sent(true);
    }

    pub(crate) fn send_logout(&self, state: &mut State<S>, text: Option<FixString>) {
        self.send(Box::new(Message::Logout(Logout {
            text,
            ..Default::default()
        })));
        state.set_logout_sent(true);
    }

    fn send_reject(
        &self,
        state: &mut State<S>,
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReason,
        text: FixString,
        ref_tag_id: Option<i64>,
    ) {
        if !matches!(
            MsgType::from_fix_str(&ref_msg_type),
            Some(MsgType::Logon) | Some(MsgType::SequenceReset)
        ) && ref_seq_num == state.next_target_msg_seq_num()
        {
            state.incr_next_target_msg_seq_num();
        }

        info!("Message {ref_seq_num} Rejected: {reason:?} (tag={ref_tag_id:?})");

        if !state.logon_received() {
            // TODO: Error
        }

        self.send(Box::new(Message::Reject(Reject {
            ref_seq_num,
            ref_tag_id,
            ref_msg_type: Some(ref_msg_type),
            session_reject_reason: Some(reason),
            text: Some(text),
            encoded_text: None,
        })));
    }

    fn send_sequence_reset(&self, seq_num: SeqNum, new_seq_num: SeqNum) {
        let mut sequence_reset = Box::new(FixtMessage {
            header: Box::new(new_header(MsgType::SequenceReset)),
            body: Box::new(Message::SequenceReset(SequenceReset {
                gap_fill_flag: Some(true),
                new_seq_no: new_seq_num,
            })),
            trailer: Box::new(new_trailer()),
        });

        sequence_reset.header.msg_seq_num = seq_num;
        sequence_reset.header.poss_dup_flag = Some(true);
        sequence_reset.header.sending_time = UtcTimestamp::now();
        sequence_reset.header.orig_sending_time = Some(sequence_reset.header.sending_time);

        info!("SequenceReset sent (MsgSeqNum: {seq_num}, NewSeqNo: {new_seq_num})");
        self.send_raw(sequence_reset);
    }

    #[instrument(level = "trace", skip_all)]
    fn send_resend_request(&self, state: &mut State<S>, msg_seq_num: SeqNum) {
        let begin_seq_no = state.next_target_msg_seq_num();
        let end_seq_no = msg_seq_num - 1;

        self.send(Box::new(Message::ResendRequest(ResendRequest {
            begin_seq_no,
            end_seq_no,
        })));

        state.set_resend_range(Some(begin_seq_no..=msg_seq_num - 1));
    }

    /// Send FIX message.
    fn send(&self, msg: Box<Message>) {
        if let Err(msg) = self.sender.send(msg) {
            // This should never happen.
            // See `fn input_loop()` and `fn output_loop()` in connection.rs
            // Output loop always waits for input loop to finish, so it's not
            // possible that output queue is closed when input message is still
            // being processed.
            unreachable!(
                "Can't send message {:?}/{} - output stream is closed",
                msg.msg_type(),
                msg.header.msg_seq_num
            );
        }
    }

    /// Send FIXT message.
    fn send_raw(&self, msg: Box<FixtMessage>) {
        if let Err(msg) = self.sender.send_raw(msg) {
            // This should never happen.
            // See `fn input_loop()` and `fn output_loop()` in connection.rs
            // Output loop always waits for input loop to finish, so it's not
            // possible that output queue is closed when input message is still
            // being processed.
            unreachable!(
                "Can't send message {:?}/{} - output stream is closed",
                msg.msg_type(),
                msg.header.msg_seq_num
            );
        }
    }

    pub(crate) async fn emit_logout(&self, reason: DisconnectReason) {
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
        }
    }

    pub(crate) fn disconnect(&self, state: &mut State<S>, reason: DisconnectReason) {
        if self.sender.is_closed() {
            info!("already disconnected");
            return;
        }

        info!("disconnecting");

        // XXX: Emit logout in connection handler instead of here,
        //      so `Logout` event will be delivered after Logout
        //      message instead of randomly before or after.
        // self.emit_logout().await;

        state.set_logout_sent(false);
        state.set_reset_received(false);
        state.set_reset_sent(false);
        if self.session_settings.reset_on_disconnect {
            state.reset();
        }

        state.set_resend_range(None);
        state.clear_queue();
        self.sender.disconnect(reason);
    }

    #[instrument(level = "trace", skip_all)]
    fn resend_range(&self, state: &mut State<S>, begin_seq_num: SeqNum, mut end_seq_num: SeqNum) {
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
        let messages = state.fetch_range(begin_seq_num..=end_seq_num);
        info!(
            "fetch messages range from {begin_seq_num} to {end_seq_num}, found {} messages",
            messages.len()
        );
        for msg_str in messages {
            // TODO: log error! and resend as gap fill instead of unwrap
            let mut msg = FixtMessage::from_bytes(&msg_str).unwrap();
            if msg.resend_as_gap_fill() {
                info!(
                    "Message {:?}/{} changed to gap fill",
                    msg.msg_type(),
                    msg.header.msg_seq_num
                );
                gap_fill_range
                    .get_or_insert((msg.header.msg_seq_num, msg.header.msg_seq_num - 1))
                    .1 += 1;
            } else {
                if let Some((begin_seq_num, end_seq_num)) = gap_fill_range.take() {
                    info!("Resending messages from {begin_seq_num} to {end_seq_num} as gap fill");
                    self.send_sequence_reset(begin_seq_num, end_seq_num + 1);
                }
                info!(
                    "Resending message {:?}/{}",
                    msg.msg_type(),
                    msg.header.msg_seq_num
                );
                msg.header.orig_sending_time = Some(msg.header.sending_time);
                msg.header.poss_dup_flag = Some(true);
                // TODO: emit event!
                self.send_raw(msg);
            }
        }
        if let Some((begin_seq_num, end_seq_num)) = gap_fill_range {
            info!("Resending messages from {begin_seq_num} to {end_seq_num} as gap fill");
            self.send_sequence_reset(begin_seq_num, end_seq_num + 1);
        }
    }

    async fn on_heartbeat(&self, message: Box<FixtMessage>) -> Result<(), VerifyError> {
        // Got Heartbeat, nothing to do.
        // If we would like to check if this is response for specific
        // TestRequest, it should be done here.
        // TODO Check it
        trace!("got heartbeat");

        self.verify(message, false, true).await?;

        self.state.borrow_mut().incr_next_target_msg_seq_num();
        Ok(())
    }

    /// Got TestRequest, answer with Heartbeat and return.
    async fn on_test_request(&self, message: Box<FixtMessage>) -> Result<(), VerifyError> {
        trace!("on_test_request");

        let Message::TestRequest(ref test_request) = *message.body else {
            unreachable!();
        };

        let test_req_id = test_request.test_req_id.clone();

        self.verify(message, false, true).await?;

        trace!("Send Heartbeat");
        self.send(Box::new(Message::Heartbeat(Heartbeat {
            test_req_id: Some(test_req_id),
        })));

        self.state.borrow_mut().incr_next_target_msg_seq_num();

        Ok(())
    }

    async fn on_resend_request(&self, msg: Box<FixtMessage>) -> Result<(), VerifyError> {
        trace!("on_resend_request");

        let Message::ResendRequest(ref resend_request) = *msg.body else {
            // Enum is matched in on_message_in_impl
            unreachable!();
        };

        let begin_seq_no = resend_request.begin_seq_no;
        let end_seq_no = resend_request.end_seq_no;

        let msg_seq_num = msg.header.msg_seq_num;

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
            //      processing, it's enqueued only to omaintain proper
            //      sequence numbers.
            state.enqueue_msg(Box::new(FixtMessage {
                header: Box::new(new_header(MsgType::ResendRequest)),
                body: Box::new(Message::ResendRequest(ResendRequest {
                    begin_seq_no,
                    end_seq_no,
                })),
                trailer: Box::new(new_trailer()),
            }));
            self.send_resend_request(&mut state, msg_seq_num);
        } else if state.next_target_msg_seq_num() == msg_seq_num {
            state.incr_next_target_msg_seq_num();
        }

        Ok(())
    }

    async fn on_reject(&self, message: Box<FixtMessage>) -> Result<(), VerifyError> {
        trace!("on_reject");

        self.verify(message, false, true).await?;

        self.state.borrow_mut().incr_next_target_msg_seq_num();

        Ok(())
    }

    async fn on_sequence_reset(&self, message: Box<FixtMessage>) -> Result<(), VerifyError> {
        trace!("on_sequence_reset");

        let Message::SequenceReset(ref sequence_reset) = *message.body else {
            // Enum is matched in on_message_in_impl
            unreachable!();
        };

        let ref_msg_type = message.header.msg_type.as_fix_str().to_owned();
        let ref_seq_num = message.header.msg_seq_num;
        let is_gap_fill = sequence_reset.gap_fill_flag.unwrap_or(false);
        let new_seq_no = sequence_reset.new_seq_no;

        self.verify(message, is_gap_fill, is_gap_fill).await?;

        let mut state = self.state().borrow_mut();

        if new_seq_no > state.next_target_msg_seq_num() {
            info!("Set next target MsgSeqNo to {new_seq_no}");
            state.set_next_target_msg_seq_num(new_seq_no);
        } else if new_seq_no < state.next_sender_msg_seq_num() {
            let reject_reason = SessionRejectReason::ValueIsIncorrect;
            let tag = FieldTag::NewSeqNo as i64;
            let text = format!("{reject_reason:?} (tag={tag}) - NewSeqNum too low");
            self.send_reject(
                &mut state,
                ref_msg_type,
                ref_seq_num,
                reject_reason,
                FixString::from_ascii_lossy(text.into_bytes()),
                Some(tag),
            );
        }

        Ok(())
    }

    async fn on_logout(&self, message: Box<FixtMessage>) -> Result<DisconnectReason, VerifyError> {
        if self.session_settings.verify_logout {
            self.verify(message, true, true).await?;
        } else if let Err(e) = self.verify(message, false, false).await {
            // Nothing more we can do as client is disconnecting anyway
            error!("logout failed: {e}");
        }

        let mut state = self.state.borrow_mut();
        let disconnect_reason = if state.logout_sent() {
            info!("received logout response");
            DisconnectReason::LocalRequestedLogout
        } else {
            info!("received logout request");
            self.send_logout(
                &mut state,
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
    async fn on_logon(
        &self,
        message: Box<FixtMessage>,
    ) -> Result<Option<DisconnectReason>, VerifyError> {
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

            let Message::Logon(ref logon) = *message.body else { unreachable!() };
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

        if !self.is_logon_time(message.header.sending_time) {
            error!("Received logon outside of valid logon time");
            return Ok(Some(DisconnectReason::InvalidLogonState));
        }

        let msg_seq_num = message.header.msg_seq_num;

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

        self.verify(message, false, true).await?;

        let mut state = self.state.borrow_mut();

        state.set_logon_received(true);

        let next_sender_msg_num_at_logon_received = state.next_sender_msg_seq_num();

        if enable_next_expected_msg_seq_num {
            if let Some(next_expected_msg_seq_num) = next_expected_msg_seq_num {
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
                    self.send_logout(&mut state, Some(err));
                    return Ok(Some(DisconnectReason::InvalidLogonState));
                }
            }
        }

        // Test here that it's not too high (which would result in a resend)
        // and that it's not resetting on logon 34=1
        let is_logon_in_normal_sequence =
            !Self::is_target_too_high(&state, msg_seq_num) || self.session_settings.reset_on_logon;

        if !state.initiate() || (state.reset_received() && !state.reset_sent()) {
            state.set_heart_bt_int(heart_bt_int);
            info!("Received logon request");

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
                Box::new(FixtMessage {
                    header: {
                        let mut header = Box::new(new_header(MsgType::Logon));
                        header.msg_seq_num = msg_seq_num;
                        header
                    },
                    body: Box::new(Message::Logon(Logon::default())),
                    trailer: Box::new(new_trailer()),
                }),
            );
            ret = Err(VerifyError::ResendRequest { msg_seq_num });
        } else {
            state.incr_next_target_msg_seq_num();
        }

        if enable_next_expected_msg_seq_num {
            if let Some(next_expected_msg_seq_num) = next_expected_msg_seq_num {
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
        }

        if Self::is_logged_on(&state) {
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

    #[instrument(name = "on_msg", level = "trace", skip_all, fields(msg_seq_num = msg.header.msg_seq_num))]
    pub async fn on_message_in_impl(&self, msg: Box<FixtMessage>) -> Option<DisconnectReason> {
        let msg_type = msg.header.msg_type;
        let msg_seq_num = msg.header.msg_seq_num;
        trace!(msg_type = format!("{msg_type:?}<{}>", msg_type.as_fix_str()));

        let result = match *msg.body {
            Message::Heartbeat(ref _heartbeat) => self.on_heartbeat(msg).await,
            Message::TestRequest(ref _test_request) => self.on_test_request(msg).await,
            Message::ResendRequest(ref _resend_request) => self.on_resend_request(msg).await,
            Message::Reject(ref _reject) => self.on_reject(msg).await,
            Message::SequenceReset(ref _sequence_reset) => self.on_sequence_reset(msg).await,
            Message::Logout(ref _logout) => match self.on_logout(msg).await {
                Ok(disconnect_reason) => return Some(disconnect_reason),
                Err(e) => Err(e),
            },
            Message::Logon(ref _logon) => match self.on_logon(msg).await {
                Ok(Some(disconnect_reason)) => {
                    return Some(disconnect_reason);
                }
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            },
            _ => {
                let verify_result = self.verify(msg, true, true).await;
                if let Err(e) = verify_result {
                    error!("message verification failed: {e:?}",);
                    Err(e)
                } else {
                    self.state.borrow_mut().incr_next_target_msg_seq_num();
                    Ok(())
                }
            }
        };

        match result {
            Ok(()) => return None,
            Err(VerifyError::Duplicate) => {}
            Err(VerifyError::ResendRequest { msg_seq_num }) => {
                if let Some(resend_range) = self.state.borrow().resend_range() {
                    let begin_seq_num = *resend_range.start();
                    let end_seq_num = *resend_range.end();

                    if !self.session_settings.send_redundant_resend_requests
                        && msg_seq_num >= begin_seq_num
                    {
                        if end_seq_num == 0 {
                            warn!("ResendRequest from {begin_seq_num} to infinity already sent, suppressing another attempt");
                        } else {
                            warn!("ResendRequest from {begin_seq_num} to {end_seq_num} already sent, suppressing another attempt");
                        }
                        return None;
                    }
                }
                self.send_resend_request(&mut self.state.borrow_mut(), msg_seq_num);
            }
            Err(VerifyError::Reject {
                reason,
                tag,
                logout,
            }) => {
                let mut state = self.state().borrow_mut();
                let tag = tag.map(|t| t as i64);
                self.send_reject(
                    &mut state,
                    msg_type.as_fix_str().to_owned(),
                    msg_seq_num,
                    reason,
                    if let Some(tag) = tag {
                        FixString::from_ascii_lossy(format!("{reason:?} (tag={tag})").into_bytes())
                    } else {
                        FixString::from_ascii_lossy(format!("{reason:?}").into_bytes())
                    },
                    tag,
                );
                if logout {
                    self.send_logout(&mut state, None);
                }
            }
            Err(e @ VerifyError::SeqNumTooLow { .. }) => {
                let mut state = self.state.borrow_mut();
                self.send_logout(
                    &mut state,
                    Some(FixString::from_ascii_lossy(e.to_string().into_bytes())),
                );
                return Some(DisconnectReason::MsgSeqNumTooLow);
            }
            Err(VerifyError::InvalidLogonState) => {
                error!("disconnecting because of invalid logon state");
                return Some(DisconnectReason::InvalidLogonState);
            }
            Err(VerifyError::UserForcedReject {
                ref_msg_type,
                ref_seq_num,
                reason,
                text,
                ref_tag_id,
            }) => {
                warn!("User rejected ({reason:?}: {text})");
                self.send_reject(
                    &mut self.state().borrow_mut(),
                    ref_msg_type,
                    ref_seq_num,
                    reason,
                    text,
                    ref_tag_id,
                );
            }
            Err(VerifyError::UserForcedLogout { text, disconnect }) => {
                error!(
                    "User rejected with Logout<5> ({})",
                    text.as_ref().map(FixString::as_utf8).unwrap_or_default()
                );
                let mut state = self.state.borrow_mut();
                self.send_logout(&mut state, text);
                if disconnect {
                    return Some(DisconnectReason::UserForcedDisconnect);
                }
            }
            Err(VerifyError::UserForcedDisconnect { reason }) => {
                error!("User disconnected: {reason:?}");
                return Some(DisconnectReason::UserForcedDisconnect);
            }
        }

        None
    }

    pub async fn on_message_in(&self, msg: Box<FixtMessage>) -> Option<DisconnectReason> {
        if let Some(disconnect_reason) = self.on_message_in_impl(msg).await {
            return Some(disconnect_reason);
        }
        loop {
            let Some(msg) = self.state.borrow_mut().retrieve_msg() else {
                break
            };

            info!("Processing queued message {}", msg.header.msg_seq_num);

            if matches!(msg.msg_type(), MsgType::Logon | MsgType::ResendRequest) {
                // Logon and ResendRequest processing has already been done,
                // just increment the target sequence nummber.
                self.state.borrow_mut().incr_next_target_msg_seq_num();
            } else if let Some(disconnect_reason) = self.on_message_in_impl(msg).await {
                return Some(disconnect_reason);
            }
        }
        None
    }

    pub async fn on_message_out(&self, msg: Box<FixtMessage>) -> Option<Box<FixtMessage>> {
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
                match receiver.await {
                    Ok(msg) => Some(msg),
                    Err(_gap_fill) => {
                        // TODO: GAP FILL!
                        // let mut header = self.new_header(MsgType::SequenceReset);
                        // header.msg_seq_num = msg.header.msg_seq_num;
                        // Ok(Some(Box::new(FixtMessage {
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
        // TODO: comment above: add where in doc it's written
        // TODO: Make sure infinite resend loop is not possible
        // TODO: It seems that some error types from the parser should end with
        //       Reject<3> message, but now for simplicity all kinds of errors
        //       results with message discard.
        let text = FixString::from_ascii_lossy(error.to_string().into_bytes());
        error!(deserialize_error = %text);

        match &error {
            DeserializeError::GarbledMessage(reason) => error!("Garbled message: {reason}"),
            DeserializeError::Logout => {
                let mut state = self.state.borrow_mut();
                self.send_logout(
                    &mut state,
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

    pub async fn on_in_timeout(self: &Rc<Self>) {
        trace!("on_in_timeout");

        self.send(Box::new(Message::TestRequest(TestRequest {
            // Use current time as TestReqId as recommended in FIX Session
            // Protocol (FIX) Version 1.1 Errata March 2008
            test_req_id: FixString::from_ascii_lossy(
                format!("{}", Utc::now().format("%Y%m%d-%H:%M:%S.%f")).into_bytes(),
            ),
        })));
    }

    pub async fn on_out_timeout(self: &Rc<Self>) {
        trace!("on_out_timeout");
        self.send(Box::new(Message::Heartbeat(Heartbeat {
            test_req_id: None,
        })));
    }

    pub fn heartbeat_interval(&self) -> Duration {
        // TODO: logon.heartbeat_interval, value from settings is for n8 only (implement as Reject
        // on Logon)

        //let inbound_test_request_timeout_duration =
        //    self.settings.heartbeat_interval + NO_INBOUND_TIMEOUT_PADDING;
        self.settings.heartbeat_interval
    }
}
