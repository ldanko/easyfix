use std::{cell::RefCell, io, ops::RangeInclusive, rc::Rc};

use chrono::NaiveTime;
use easyfix_messages::{
    deserializer::DeserializeError,
    fields::{
        DefaultApplVerId, EncryptMethod, FixStr, FixString, Int, MsgType, SeqNum,
        SessionRejectReason, Utc, UtcTimestamp,
    },
    messages::{
        FieldTag, FixtMessage, Header, Heartbeat, Logon, Logout, Message, MsgCat, Reject,
        ResendRequest, SequenceReset, TestRequest, Trailer,
    },
};
use tokio::time::{Duration, Instant};
use tracing::{error, info, instrument, trace, warn};

use crate::{
    application::{
        /*events_channel,*/ Emitter, FixEventInternal,
        InputResponderMsg, /*, EventStream, Sender */
        Responder,
    },
    connection::Disconnect,
    messages_storage::MessagesStorage,
    session_id::SessionId,
    session_state::State,
    settings::{SessionSettings, Settings},
    Error, Sender,
};

//TODO:
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
    #[error("Logout: {text:?}, disconnect: {disconnect}")]
    Logout {
        text: Option<FixString>,
        disconnect: bool,
    },
    #[error("Disconnect: {0}")]
    Disconnect(String),
}

impl VerifyError {
    fn invalid_logon_state() -> VerifyError {
        VerifyError::Disconnect("invalid logon state".to_owned())
    }

    fn invalid_time() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReason::SendingTimeAccuracyProblem,
            tag: None,
            logout: false,
        }
    }

    fn invalid_comp_id() -> VerifyError {
        VerifyError::Reject {
            reason: SessionRejectReason::CompIdProblem,
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
            reason: SessionRejectReason::SendingTimeAccuracyProblem,
            tag: None,
            logout: true,
        }
    }

    fn seq_num_too_low(msg_seq_num: SeqNum, next_target_msg_seq_num: SeqNum) -> VerifyError {
        VerifyError::Logout {
            text: Some(FixString::from_ascii_lossy(
                format!(
                    "MsgSeqNum too low, expecting {}, but received {}",
                    next_target_msg_seq_num, msg_seq_num
                )
                .into_bytes(),
            )),
            disconnect: true,
        }
    }
}

trait MessageExt {
    fn resend_as_gap_fill(&self) -> bool;
}

impl MessageExt for FixtMessage {
    fn resend_as_gap_fill(&self) -> bool {
        match (self.msg_cat(), self.msg_type()) {
            (MsgCat::App, _) => false,
            (MsgCat::Admin, MsgType::Reject) => false,
            (MsgCat::Admin, _) => true,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Session<S> {
    state: Rc<RefCell<State<S>>>,
    sender: Sender,
    settings: Settings,
    session_settings: SessionSettings,
    emitter: Emitter, /*
                      events_stream: EventStream,
                      sender: Sender,
                      */
                      // SessionID m_sessionID;
                      // TimeRange m_sessionTime;
                      // TimeRange m_logonTime;

                      // bool m_resetOnLogon;
                      // bool m_resetOnLogout;
                      // bool m_resetOnDisconnect;
                      // bool m_refreshOnLogon;
                      // int m_timestampPrecision;
                      // bool m_persistMessages;
                      // bool m_validateLengthAndChecksum;

                      // SessionState m_state;
                      // DataDictionaryProvider m_dataDictionaryProvider;
                      // MessageStoreFactory& m_messageStoreFactory;
                      // LogFactory* m_pLogFactory;
                      // Responder* m_pResponder;
}

impl<S: MessagesStorage> Session<S> {
    pub(crate) fn new(
        settings: Settings,
        session_settings: SessionSettings,
        state: Rc<RefCell<State<S>>>,
        sender: Sender,
        emitter: Emitter,
    ) -> Session<S> {
        /*
        let (sender, events_stream) = events_channel();
        */
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

    pub fn logon(&self) {
        let mut state = self.state.borrow_mut();
        state.set_enabled(true);
        state.set_logout_reason(None);
    }

    pub fn logout(&self, reason: FixString) {
        let mut state = self.state.borrow_mut();
        state.set_enabled(false);
        state.set_logout_reason(Some(reason));
    }

    pub fn is_enabled(&self) -> bool {
        self.state.borrow().enabled()
    }

    pub fn logon_sent(&self) -> bool {
        self.state.borrow().logon_sent()
    }

    pub fn logout_sent(&self) -> bool {
        self.state.borrow().logout_sent()
    }

    pub fn logon_received(&self) -> bool {
        self.state.borrow().logon_received()
    }

    pub fn is_logged_on(state: &State<S>) -> bool {
        state.logon_received() && state.logon_sent()
    }

    // void reset() EXCEPT ( IOException )
    // { generateLogout(); disconnect(); m_state.reset(); }
    // void refresh() EXCEPT ( IOException )
    // { m_state.refresh(); }
    // void setNextSenderMsgSeqNum( int num ) EXCEPT ( IOException )
    // { m_state.setNextSenderMsgSeqNum( num ); }
    // void setNextTargetMsgSeqNum( int num ) EXCEPT ( IOException )
    // { m_state.setNextTargetMsgSeqNum( num ); }

    // const SessionID& getSessionID() const
    // { return m_sessionID; }
    // void setDataDictionaryProvider( const DataDictionaryProvider& dataDictionaryProvider )
    // { m_dataDictionaryProvider = dataDictionaryProvider; }
    // const DataDictionaryProvider& getDataDictionaryProvider() const
    // { return m_dataDictionaryProvider; }

    // static bool sendToTarget( Message& message,
    //                           const std::string& qualifier = "" )
    // EXCEPT ( SessionNotFound );
    // static bool sendToTarget( Message& message, const SessionID& sessionID )
    // EXCEPT ( SessionNotFound );
    // static bool sendToTarget( Message&,
    //                           const SenderCompID& senderCompID,
    //                           const TargetCompID& targetCompID,
    //                           const std::string& qualifier = "" )
    // EXCEPT ( SessionNotFound );
    // static bool sendToTarget( Message& message,
    //                           const std::string& senderCompID,
    //                           const std::string& targetCompID,
    //                           const std::string& qualifier = "" )
    // EXCEPT ( SessionNotFound );

    // static std::set<SessionID> getSessions();
    // static bool doesSessionExist( const SessionID& );
    // static Session* lookupSession( const SessionID& );
    // static Session* lookupSession( const std::string&, bool reverse = false );
    // static bool isSessionRegistered( const SessionID& );
    // static Session* registerSession( const SessionID& );
    // static void unregisterSession( const SessionID& );

    // static size_t numSessions();

    pub fn is_session_time(&self, time: UtcTimestamp) -> bool {
        self.session_settings
            .session_time
            .contains(&time.timestamp().time())
    }

    pub fn is_logon_time(&self, time: UtcTimestamp) -> bool {
        self.session_settings
            .logon_time
            .contains(&time.timestamp().time())
    }

    pub fn is_initiator(&self) -> bool {
        self.state.borrow().initiate()
    }

    pub fn is_acceptro(&self) -> bool {
        !self.state.borrow().initiate()
    }

    pub fn logon_time(&self) -> RangeInclusive<NaiveTime> {
        self.session_settings.logon_time.clone()
    }

    pub fn set_logon_time(&mut self, logon_time: RangeInclusive<NaiveTime>) {
        self.session_settings.logon_time = logon_time;
    }

    pub fn sender_default_appl_ver_id(&self) -> &FixStr {
        &self.session_settings.sender_default_appl_ver_id
    }

    pub fn set_sender_default_appl_ver_id(&mut self, default_appl_veri_id: FixString) {
        self.session_settings.sender_default_appl_ver_id = default_appl_veri_id;
    }

    pub fn target_default_appl_ver_id(&self) -> &FixStr {
        &self.session_settings.target_default_appl_ver_id
    }

    pub fn set_target_default_appl_ver_id(&mut self, default_appl_veri_id: FixString) {
        self.session_settings.target_default_appl_ver_id = default_appl_veri_id;
    }

    // const std::string& getTargetDefaultApplVerID()
    //   { return m_targetDefaultApplVerID; }
    // void setTargetDefaultApplVerID( const std::string& targetDefaultApplVerID )
    //   { m_targetDefaultApplVerID = targetDefaultApplVerID; }

    // bool getSendRedundantResendRequests()
    //   { return m_sendRedundantResendRequests; }
    // void setSendRedundantResendRequests ( bool value )
    //   { m_sendRedundantResendRequests = value; }

    // bool getCheckCompId()
    //   { return m_checkCompId; }
    // void setCheckCompId ( bool value )
    //   { m_checkCompId = value; }

    // int getMaxLatency()
    //   { return m_maxLatency; }
    // void setMaxLatency ( int value )
    //   { m_maxLatency = value; }

    // int getLogonTimeout()
    //   { return m_state.logonTimeout(); }
    // void setLogonTimeout ( int value )
    //   { m_state.logonTimeout( value ); }

    // int getLogoutTimeout()
    //   { return m_state.logoutTimeout(); }
    // void setLogoutTimeout ( int value )
    //   { m_state.logoutTimeout( value ); }

    // bool getResetOnLogon()
    //   { return m_resetOnLogon; }
    // void setResetOnLogon ( bool value )
    //   { m_resetOnLogon = value; }

    // bool getResetOnLogout()
    //   { return m_resetOnLogout; }
    // void setResetOnLogout ( bool value )
    //   { m_resetOnLogout = value; }

    // bool getResetOnDisconnect()
    //   { return m_resetOnDisconnect; }
    // void setResetOnDisconnect( bool value )
    //   { m_resetOnDisconnect = value; }

    // bool getRefreshOnLogon()
    //   { return m_refreshOnLogon; }
    // void setRefreshOnLogon( bool value )
    //   { m_refreshOnLogon = value; }

    // bool getMillisecondsInTimeStamp()
    //   { return (m_timestampPrecision == 3); }
    // void setMillisecondsInTimeStamp ( bool value )
    //   { if (value)
    //       m_timestampPrecision = 3;
    //     else
    //       m_timestampPrecision = 0;
    //   }
    // int getTimestampPrecision()
    //   { return m_timestampPrecision; }
    // void setTimestampPrecision(int precision)
    //   {
    //     if (precision < 0 || precision > 9)
    //       return;

    //     m_timestampPrecision = precision;
    //   }
    // int getSupportedTimestampPrecision()
    //   {
    //     return supportsSubSecondTimestamps(m_sessionID.getBeginString()) ? m_timestampPrecision : 0;
    //   }
    // static bool supportsSubSecondTimestamps(const std::string &beginString)
    // {
    //   if( beginString == BeginString_FIXT11 )
    //     return true;
    //   else
    //     return beginString >= BeginString_FIX42;
    // }
    //

    // bool getPersistMessages()
    //   { return m_persistMessages; }
    // void setPersistMessages ( bool value )
    //   { m_persistMessages = value; }

    // bool getValidateLengthAndChecksum()
    //   { return m_validateLengthAndChecksum; }
    // void setValidateLengthAndChecksum ( bool value )
    //   { m_validateLengthAndChecksum = value; }

    // void setResponder( Responder* pR )
    // {
    //   if( !checkSessionTime(UtcTimeStamp()) )
    //     reset();
    //   m_pResponder = pR;
    // }

    // bool send( Message& );
    // void next();
    // void next( const UtcTimeStamp& timeStamp );
    // void next( const std::string&, const UtcTimeStamp& timeStamp, bool queued = false );
    // void next( const Message&, const UtcTimeStamp& timeStamp, bool queued = false );
    // void disconnect();

    fn get_expected_sender_num(&self) -> SeqNum {
        self.state.borrow().next_sender_msg_seq_num()
    }

    fn get_expected_target_num(&self) -> SeqNum {
        self.state.borrow().next_target_msg_seq_num()
    }

    // Log* getLog() { return &m_state; }
    // const MessageStore* getStore() { return &m_state; }

    // private:
    // typedef std::map < SessionID, Session* > Sessions;
    // typedef std::set < SessionID > SessionIDs;

    // static bool addSession( Session& );
    // static void removeSession( Session& );

    // bool send( const std::string& );
    // bool sendRaw( Message&, int msgSeqNum = 0 );
    // bool resend( Message& message );
    // void persist( const Message&, const std::string& ) EXCEPT ( IOException );

    // void insertSendingTime( Header& );
    // void insertOrigSendingTime( Header&,
    //                             const UtcTimeStamp& when = UtcTimeStamp () );
    // void fill( Header& );

    fn is_good_time(&self, sending_time: UtcTimestamp) -> bool {
        if !self.session_settings.check_latency {
            return true;
        }
        //UtcTimeStamp now;
        return Utc::now() - sending_time.timestamp()
            <= chrono::Duration::from_std(self.session_settings.max_latency).expect("duration");
    }

    // bool checkSessionTime( const UtcTimeStamp& timeStamp )
    // {
    //   UtcTimeStamp creationTime = m_state.getCreationTime();
    //   return m_sessionTime.isInSameRange( timeStamp, creationTime );
    // }
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

    fn should_send_reset(&self) -> bool {
        (self.session_settings.reset_on_logon
            || self.session_settings.reset_on_logout
            || self.session_settings.reset_on_disconnect)
            && self.get_expected_target_num() == 1
            && self.get_expected_sender_num() == 1
    }

    // bool validLogonState( const MsgType& msgType );

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
        if msg_type != MsgType::Logout && state.logon_sent() {
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

    // void fromCallback( const MsgType& msgType, const Message& msg,
    //                    const SessionID& sessionID );

    // void nextQueued( const UtcTimeStamp& timeStamp );
    // bool nextQueued( int num, const UtcTimeStamp& timeStamp );

    // void nextLogon( const Message&, const UtcTimeStamp& timeStamp );
    // void nextHeartbeat( const Message&, const UtcTimeStamp& timeStamp );
    // void nextTestRequest( const Message&, const UtcTimeStamp& timeStamp );
    // void nextLogout( const Message&, const UtcTimeStamp& timeStamp );
    // void nextReject( const Message&, const UtcTimeStamp& timeStamp );
    // void nextSequenceReset( const Message&, const UtcTimeStamp& timeStamp );
    // void nextResendRequest( const Message&, const UtcTimeStamp& timeStamp );

    // void generateLogon();
    // void generateLogon( const Message& );
    // void generateResendRequest( const BeginString&, const MsgSeqNum& );
    // void generateSequenceReset( int, int );
    // void generateHeartbeat();
    // void generateHeartbeat( const Message& );
    // void generateTestRequest( const std::string& );
    // void generateReject( const Message&, int err, int field = 0 );
    // void generateReject( const Message&, const std::string& );
    // void generateBusinessReject( const Message&, int err, int field = 0 );
    // void generateLogout( const std::string& text = "" );

    // void populateRejectReason( Message&, int field, const std::string& );
    // void populateRejectReason( Message&, const std::string& );

    // bool verify( const Message& msg,
    //              bool checkTooHigh = true, bool checkTooLow = true );

    // bool set( int s, const Message& m );
    // bool get( int s, Message& m ) const;

    // Message * newMessage(const std::string & msgType) const;

    // TODO: Return ValidationError enum and outside of this function do all
    //       async stuff, so functiolns like `do_target_too_high` could
    //       get message by move
    #[instrument(
        level = "trace",
        skip_all,
        fields(msg_type = ?msg.header.msg_type),
        err, ret
    )]
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
            Err(VerifyError::invalid_logon_state())
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
                Err(VerifyError::seq_num_too_low(
                    msg_seq_num,
                    state.next_target_msg_seq_num(),
                ))
            }
        } else {
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
            match receiver.await {
                Ok(InputResponderMsg::Reject {
                    ref_msg_type,
                    ref_seq_num,
                    reason,
                    text,
                    ref_tag_id,
                }) => {
                    warn!("User rejected ({reason:?}: {text})");
                    self.send_reject(ref_msg_type, ref_seq_num, reason, text, ref_tag_id)
                        .await
                }
                Ok(InputResponderMsg::Logout { text, disconnect }) => {
                    error!(
                        "User rejected with Logout<5> ({})",
                        text.as_ref().map(FixString::as_utf8).unwrap_or_default()
                    );
                    self.send_logout(text).await;
                    if disconnect {
                        self.disconnect().await;
                    }
                }
                Ok(InputResponderMsg::Disconnect { reason }) => {
                    error!("User disconnected");
                    self.disconnect().await;
                }
                Err(_) => {}
            }

            Ok(())
        }

        /*
          if ( (checkTooHigh || checkTooLow) && m_state.resendRequested() )
          {
            SessionState::ResendRange range = m_state.resendRange();

            if ( *pMsgSeqNum >= range.second )
            {
              m_state.onEvent ("ResendRequest for messages FROM: " +
                               IntConvertor::convert (range.first) + " TO: " +
                               IntConvertor::convert (range.second) +
                               " has been satisfied.");
              m_state.resendRange (0, 0);
            }
          }

        catch ( std::exception& e )
        {
          m_state.onEvent( e.what() );
          disconnect();
          return false;
        }

        UtcTimeStamp now;
        m_state.lastReceivedTime( now );
        m_state.testRequest( 0 );

        fromCallback( pMsgType ? *pMsgType : MsgType(), msg, m_sessionID );
        return true;
        */
    }

    async fn send_logon_request(&self) {
        let mut state = self.state().borrow_mut();

        if self.session_settings.refresh_on_logon {
            state.refresh();
        }
        if self.session_settings.reset_on_logon {
            state.reset();
        }

        let logon_request = Box::new(FixtMessage {
            header: self.new_header_with_state(MsgType::Logon, &mut state),
            body: Message::Logon(Logon {
                // encrypt_method: EncryptMethod::None,
                encrypt_method: EncryptMethod::NoneOther,
                heart_bt_int: state.heart_bt_int(),
                raw_data: None,
                reset_seq_num_flag: self.should_send_reset().then_some(true),
                next_expected_msg_seq_num: if self.session_settings.enable_next_expected_msg_seq_num
                {
                    let next_expected_msg_seq_num = state.next_sender_msg_seq_num();
                    state.set_last_expected_logon_next_seq_num(next_expected_msg_seq_num);
                    Some(next_expected_msg_seq_num)
                } else {
                    None
                },
                max_message_size: None,
                test_message_indicator: None,
                username: None,
                password: None,
                // TODO: if self.session_settings.session_id().is_fixt()
                // default_appl_ver_id: self.sender_default_appl_ver_id().to_owned(),
                default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
                msg_type_grp: None,
            }),
            trailer: self.new_trailer(),
        });

        drop(state);
        self.send(logon_request).await;
    }

    async fn send_logon_response(&self, expected_target_num: SeqNum) {
        let mut state = self.state.borrow_mut();

        if self.session_settings.refresh_on_logon {
            state.refresh();
        }
        if self.session_settings.reset_on_logon {
            state.reset();
        }

        let logon_response = Box::new(FixtMessage {
            header: self.new_header_with_state(MsgType::Logon, &mut state),
            body: Message::Logon(Logon {
                // encrypt_method: EncryptMethod::None,
                encrypt_method: EncryptMethod::NoneOther,
                // TODO: option to use predefined OR the value from Logon request
                heart_bt_int: state.heart_bt_int(),
                raw_data: None,
                reset_seq_num_flag: self.should_send_reset().then_some(true),
                next_expected_msg_seq_num: if self.session_settings.enable_next_expected_msg_seq_num
                {
                    info!("Responding to Logon request with tag 789={expected_target_num}");
                    state.set_last_expected_logon_next_seq_num(expected_target_num);
                    Some(expected_target_num)
                } else {
                    info!("Responding to Logon request");
                    None
                },
                max_message_size: None,
                test_message_indicator: None,
                username: None,
                password: None,
                // TODO: if self.session_settings.session_id().is_fixt()
                // default_appl_ver_id: self.sender_default_appl_ver_id().to_owned(),
                default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
                msg_type_grp: None,
            }),
            trailer: self.new_trailer(),
        });

        state.set_last_received_time(Instant::now());
        state.set_test_request(0);
        state.set_logon_sent(true);

        drop(state);

        self.send(logon_response).await;
    }

    pub(crate) async fn send_logout(&self, text: Option<FixString>) {
        let logout_response = Box::new(FixtMessage {
            header: self.new_header(MsgType::Logout),
            body: Message::Logout(Logout {
                encoded_text: None,
                // TODO: verify arg vs state
                //
                text: text.or_else(|| self.state.borrow().logout_reason().map(FixString::from)),
            }),
            trailer: self.new_trailer(),
        });
        self.send(logout_response).await;
        self.state.borrow_mut().set_logout_sent(true);
    }

    async fn send_reject(
        &self,
        ref_msg_type: FixString,
        ref_seq_num: SeqNum,
        reason: SessionRejectReason,
        text: FixString,
        ref_tag_id: Option<i64>,
    ) {
        // reject.reverseRoute( message.getHeader() );
        //fill( reject.getHeader() );

        if !matches!(
            MsgType::from_fix_str(&ref_msg_type),
            Some(MsgType::Logon) | Some(MsgType::SequenceReset)
        ) && ref_seq_num == self.state.borrow().next_target_msg_seq_num()
        {
            self.state.borrow_mut().incr_next_target_msg_seq_num();
        }

        info!("Message {ref_seq_num} Rejected: {reason:?} (tag={ref_tag_id:?})");

        if !self.state.borrow().logon_received() {
            //throw std::runtime_error( "Tried to send a reject while not logged on" );
        }

        self.send(Box::new(FixtMessage {
            header: self.new_header(MsgType::Reject),
            body: Message::Reject(Reject {
                ref_seq_num,
                ref_tag_id,
                ref_msg_type: Some(ref_msg_type),
                session_reject_reason: Some(reason),
                text: Some(text),
                encoded_text: None,
            }),
            trailer: self.new_trailer(),
        }))
        .await;
    }

    async fn send_sequence_reset(&self, begin_seq_num: SeqNum, end_seq_num: SeqNum) {
        let mut sequence_reset = Box::new(FixtMessage {
            header: self.new_header(MsgType::SequenceReset),
            body: Message::SequenceReset(SequenceReset {
                gap_fill_flag: Some(true),
                new_seq_no: end_seq_num,
            }),
            trailer: self.new_trailer(),
        });
        // TODO: will be overwrited by Encoder to next_seq_num!
        sequence_reset.header.msg_seq_num = begin_seq_num;
        sequence_reset.header.poss_dup_flag = Some(true);
        // TODO: Make sure in GipFill mode OrigSendingTime is same as SendingTime
        sequence_reset.header.orig_sending_time = Some(sequence_reset.header.sending_time);
        self.send(sequence_reset).await;

        info!("Sent SequenceReset TO: {end_seq_num}");
    }

    async fn send_resend_request(&self, msg_seq_num: SeqNum) {
        //void Session::generateResendRequest( const BeginString& beginString, const MsgSeqNum& msgSeqNum )

        let begin_seq_no = self.get_expected_target_num();
        let end_seq_no = msg_seq_num - 1;
        let resend_request = Box::new(FixtMessage {
            header: self.new_header(MsgType::ResendRequest),
            body: Message::ResendRequest(ResendRequest {
                begin_seq_no,
                end_seq_no,
            }),
            trailer: self.new_trailer(),
        });

        // TODO: send_raw!
        self.send(resend_request).await;
        // sendRaw(resendRequest);

        self.state
            .borrow_mut()
            .set_resend_range(Some(begin_seq_no..=msg_seq_num - 1));
    }

    fn new_header_with_state(&self, msg_type: MsgType, state: &mut State<S>) -> Header {
        let msg_seq_num = {
            state.set_last_sent_time(Instant::now());
            state.next_sender_msg_seq_num()
        };
        Header {
            begin_string: self.session_settings.session_id.begin_string().to_owned(),
            // Overwriten during serialization
            body_length: 0,
            msg_type,
            sender_comp_id: self.session_settings.session_id.sender_comp_id().to_owned(),
            target_comp_id: self.session_settings.session_id.target_comp_id().to_owned(),
            on_behalf_of_comp_id: None,
            deliver_to_comp_id: None,
            secure_data: None,
            msg_seq_num,
            sender_sub_id: None,
            sender_location_id: None,
            target_sub_id: None,
            target_location_id: None,
            on_behalf_of_sub_id: None,
            on_behalf_of_location_id: None,
            deliver_to_sub_id: None,
            deliver_to_location_id: None,
            poss_dup_flag: None,
            poss_resend: None,
            sending_time: UtcTimestamp::now_with_secs(),
            orig_sending_time: None,
            xml_data: None,
            message_encoding: None,
            last_msg_seq_num_processed: None,
            hop_grp: None,
            appl_ver_id: None,
            cstm_appl_ver_id: None,
        }

        //  header.setField( MsgSeqNum( getExpectedSenderNum() ) );
        //  insertSendingTime( header );
    }

    // TODO: Pass `&mut state` as parameter!
    fn new_header(&self, msg_type: MsgType) -> Header {
        let msg_seq_num = {
            let mut state = self.state.borrow_mut();
            // TODO: make sure no one will call `new_header` without sendingTime
            //       the message
            state.set_last_sent_time(Instant::now());
            state.next_sender_msg_seq_num()
        };
        Header {
            begin_string: self.session_settings.session_id.begin_string().to_owned(),
            // Overwriten during serialization
            body_length: 0,
            msg_type,
            sender_comp_id: self.session_settings.session_id.sender_comp_id().to_owned(),
            target_comp_id: self.session_settings.session_id.target_comp_id().to_owned(),
            on_behalf_of_comp_id: None,
            deliver_to_comp_id: None,
            secure_data: None,
            msg_seq_num,
            sender_sub_id: None,
            sender_location_id: None,
            target_sub_id: None,
            target_location_id: None,
            on_behalf_of_sub_id: None,
            on_behalf_of_location_id: None,
            deliver_to_sub_id: None,
            deliver_to_location_id: None,
            poss_dup_flag: None,
            poss_resend: None,
            sending_time: UtcTimestamp::now_with_secs(),
            orig_sending_time: None,
            xml_data: None,
            message_encoding: None,
            last_msg_seq_num_processed: None,
            hop_grp: None,
            appl_ver_id: None,
            cstm_appl_ver_id: None,
        }

        //  header.setField( MsgSeqNum( getExpectedSenderNum() ) );
        //  insertSendingTime( header );
    }

    const fn new_trailer(&self) -> Trailer {
        Trailer {
            signature: None,
            // overwriteen during serialization
            check_sum: FixString::new(),
        }
    }

    fn fill(&self, header: &mut Header) {
        return;
        header.begin_string = self.session_settings.session_id.begin_string().to_owned();
        header.sender_comp_id = self.session_settings.session_id.sender_comp_id().to_owned();
        header.target_comp_id = self.session_settings.session_id.target_comp_id().to_owned();
        // header.msg_seq_num = ...
        header.sending_time = UtcTimestamp::now_with_secs();
        // TODO: decide when sending time should be updaated (last_sent_time)
    }

    async fn send_raw(&self, mut message: Box<FixtMessage>, num: Option<SeqNum>) -> bool {
        self.fill(&mut message.header);

        if let Some(num) = num {
            message.header.msg_seq_num = num;
        }

        match message.msg_cat() {
            MsgCat::Admin => {
                // TODO: send emitter event
                // self.emitter
                //     .send(FixEventInternal::AdmMsgOut(Some(message), ()))
                //     .await;

                // self.application
                //     .borrow_mut()
                //     .await
                //     .on_admin_msg_out(message)
                //     .await;

                if let Message::Logon(ref logon) = message.body {
                    if self.state.borrow().reset_received() {
                        if let Some(true) = logon.reset_seq_num_flag {
                            self.state.borrow_mut().reset();
                            message.header.msg_seq_num = self.get_expected_sender_num();
                        }
                        self.state
                            .borrow_mut()
                            .set_reset_sent(logon.reset_seq_num_flag.unwrap_or(false));
                    }
                }

                /* TODO:
                let message_string = message.to_string();

                if num.is_none() {
                    persist(message, message_string);
                }
                */

                if matches!(
                    message.msg_type(),
                    MsgType::Logon
                        | MsgType::Logout
                        | MsgType::ResendRequest
                        | MsgType::SequenceReset
                ) || Self::is_logged_on(&self.state.borrow())
                {
                    /* TODO
                    self.send(message_string).await?;
                    */
                }
            }
            MsgCat::App => {
                // do not send application messages if they will just be cleared
                if !Self::is_logged_on(&self.state.borrow()) && self.should_send_reset() {
                    return false;
                }

                // TODO: send emitter event
                // let result = self
                //     .application
                //     .borrow_mut()
                //     .await
                //     .on_app_msg_out(message)
                //     .await;

                /* TODO
                let message_string = message.to_string();

                if num.is_none() {
                    persist(message, message_string);
                }
                */

                if Self::is_logged_on(&self.state.borrow()) {
                    /* TODO
                    self.send(message_string).await?;
                    */
                }
                // if let Err(_do_not_send) = result {
                //     return Ok(false);
                // }
            }
        }

        true
    }

    async fn send(&self, msg: Box<FixtMessage>) {
        self.sender.send(msg).await;
    }

    pub(crate) async fn disconnect(&self) {
        info!("disconnecting");
        let mut state = self.state.borrow_mut();

        if state.logon_received() || state.logon_sent() {
            state.set_logon_received(false);
            state.set_logon_sent(false);
            drop(state);
            self.emitter
                .send(FixEventInternal::Logout(
                    self.session_settings.session_id.clone(),
                ))
                .await;
            state = self.state.borrow_mut();
        }

        state.set_logout_sent(false);
        state.set_reset_received(false);
        state.set_reset_sent(false);
        // state.clearQueue();
        state.set_logout_reason(None);
        if self.session_settings.reset_on_disconnect {
            state.reset();
        }

        state.set_resend_range(None);
        self.sender.disconnect().await;
    }

    async fn resend_range(&self, begin_seq_num: SeqNum, mut end_seq_num: SeqNum) {
        let next_sender_msg_seq_num = self.state.borrow().next_sender_msg_seq_num();
        if end_seq_num == 0 || end_seq_num >= next_sender_msg_seq_num {
            end_seq_num = next_sender_msg_seq_num - 1;
        }

        // Just do a gap fill when messages aren't persisted
        if !self.session_settings.persist {
            self.send_sequence_reset(begin_seq_num, end_seq_num).await;
            return;
        }

        let mut gap_fill_range = None;
        // for msg in messages {
        for msg_seq_num in begin_seq_num..=end_seq_num {
            // TODO: GapFill in case of error
            let mut msg = {
                let msg = self.state.borrow_mut().fetch(msg_seq_num).unwrap();
                FixtMessage::from_bytes(&msg).unwrap()
            };
            assert_eq!(msg_seq_num, msg.header.msg_seq_num);
            if msg.resend_as_gap_fill() {
                info!("Resending message {msg_seq_num} as gap fill");
                gap_fill_range.get_or_insert((msg_seq_num, msg_seq_num)).1 += 1;
            } else {
                if let Some((begin_seq_no, end_seq_no)) = gap_fill_range.take() {
                    self.send_sequence_reset(begin_seq_num, end_seq_num).await;
                }
                info!("Resending message {msg_seq_num}");
                msg.header.orig_sending_time = Some(msg.header.sending_time);
                msg.header.poss_dup_flag = Some(true);
                // TODO: emit event!
                self.send(msg).await;
            }
        }
        if let Some((begin_seq_no, end_seq_no)) = gap_fill_range {
            self.send_sequence_reset(begin_seq_num, end_seq_num).await;
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
        let test_req_id = {
            let Message::TestRequest(ref test_request) = message.body else { unreachable!() };
            test_request.test_req_id.clone()
        };

        self.verify(message, false, true).await?;

        let heartbeat = Box::new(FixtMessage {
            header: self.new_header(MsgType::Heartbeat),
            body: Message::Heartbeat(Heartbeat {
                test_req_id: Some(test_req_id),
            }),
            trailer: self.new_trailer(),
        });
        trace!("Send Heartbeat");
        self.send(heartbeat).await;

        self.state.borrow_mut().incr_next_target_msg_seq_num();

        Ok(())
    }

    async fn on_resend_request(&self, msg: Box<FixtMessage>) -> Result<(), VerifyError> {
        trace!("on_resend_request");

        let (begin_seq_no, end_seq_no) =
            if let Message::ResendRequest(ref resend_request) = msg.body {
                (resend_request.begin_seq_no, resend_request.end_seq_no)
            } else {
                // Enum is matched in on_message_in_impl
                unreachable!();
            };

        let msg_seq_num = msg.header.msg_seq_num;

        self.verify(msg, false, false).await?;

        info!("Received ResendRequest FROM: {begin_seq_no} TO: {end_seq_no}");

        self.resend_range(begin_seq_no, end_seq_no).await;

        {
            let mut state = self.state.borrow_mut();
            if state.next_target_msg_seq_num() == msg_seq_num {
                state.incr_next_target_msg_seq_num();
            }
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
        todo!();

        self.state.borrow_mut().incr_next_target_msg_seq_num();

        Ok(())
    }

    async fn on_logout(&self, message: Box<FixtMessage>) -> Result<(), VerifyError> {
        self.verify(message, false, false).await?;

        if self.state.borrow().logout_sent() {
            info!("received logout response");
        } else {
            info!("received logout request");
            self.send_logout(Some(FixString::from_ascii_lossy(b"Responding".to_vec())))
                .await;
            info!("sending logout response");
        }

        self.state.borrow_mut().incr_next_target_msg_seq_num();
        if self.session_settings.reset_on_logout {
            self.state.borrow_mut().reset();
        }
        self.disconnect().await;

        Ok(())
    }

    async fn on_logon(&self, message: Box<FixtMessage>) -> Result<Option<Disconnect>, VerifyError> {
        let mut state = self.state.borrow_mut();

        if !state.enabled() {
            error!("Session is not enabled for logon");
            drop(state);
            self.disconnect().await;
            return Ok(Some(Disconnect));
        }

        if !self.is_logon_time(message.header.sending_time) {
            error!("Received logon outside of valid logon time");
            drop(state);
            self.disconnect().await;
            return Ok(Some(Disconnect));
        }

        let msg_seq_num = message.header.msg_seq_num;

        let (reset_seq_num_flag, heart_bt_int, next_expected_msg_seq_num) = {
            let Message::Logon(ref logon) = message.body else { unreachable!() };
            (
                logon.reset_seq_num_flag,
                logon.heart_bt_int,
                logon.next_expected_msg_seq_num,
            )
        };

        if self.session_settings.refresh_on_logon {
            state.refresh();
        }

        // // QFJ-926 - reset session before accepting Logon
        // resetIfSessionNotCurrent(sessionID, SystemTime.currentTimeMillis());

        // TODO: add elese?
        if let Some(true) = reset_seq_num_flag {
            state.set_reset_received(true);
            info!("Logon contains ResetSeqNumFlag=Y, reseting sequence numbers to 1");
            if !state.reset_sent() {
                state.reset();
            }
        } else if state.reset_sent() && msg_seq_num == 1 {
            info!("Inferring ResetSeqNumFlag as sequence number is 1 in response to reset request");
            state.set_reset_received(true);
        }

        if state.should_send_logon() && !state.reset_received() {
            error!("Received logon response before sending request");
            self.disconnect().await;
            return Ok(Some(Disconnect));
        }

        if !state.initiate() && self.session_settings.reset_on_logon {
            state.reset();
        }

        drop(state);
        self.verify(message, false, true).await?;
        state = self.state.borrow_mut();

        state.set_logon_received(true);

        let next_sender_msg_num_at_logon_received = state.next_sender_msg_seq_num();

        if self.session_settings.enable_next_expected_msg_seq_num {
            if let Some(next_expected_msg_seq_num) = next_expected_msg_seq_num {
                let next_sender_msg_seq_num = state.next_sender_msg_seq_num();
                // Is the 789 we received too high ??
                if next_expected_msg_seq_num > next_sender_msg_seq_num {
                    // can't resend what we never sent! something unrecoverable has happened.
                    let err = FixString::from_ascii_lossy(
                        format!(
                            "NextExpectedMsgSeqNum<789> too high \
                            (expected {next_sender_msg_seq_num}, \
                             got {next_expected_msg_seq_num})",
                        )
                        .into_bytes(),
                    );
                    self.send_logout(Some(err)).await;
                    // TODO: add reason to be logged in `fn disconnect()`
                    //self.disconnect(err, true).await;
                    return Ok(Some(Disconnect));
                }
            }
        }

        // We test here that it's not too high (which would result in
        // a resend) and that we are not resetting on logon 34=1
        let is_logon_in_normal_sequence = !(Self::is_target_too_high(&state, msg_seq_num)
            && !self.session_settings.reset_on_logon);

        if !state.initiate() || (state.reset_received() && !state.reset_sent()) {
            state.set_heart_bt_int(heart_bt_int);
            info!("Received logon request");

            let mut next_expected_target_num = state.next_target_msg_seq_num();
            // we increment for the logon later (after Logon response sent) in this method if and only if in sequence
            if is_logon_in_normal_sequence {
                // logon was fine take account of it in 789
                next_expected_target_num += 1;
            }

            drop(state);
            self.send_logon_response(next_expected_target_num).await;
            state = self.state.borrow_mut();

            info!("Responding to logon request");
        } else {
            info!("Received logon response");
        }

        state.set_reset_sent(false);
        state.set_reset_received(false);

        if Self::is_target_too_high(&state, msg_seq_num) && !reset_seq_num_flag.unwrap_or(false) {
            // if 789 was sent then we effectively have already sent a resend request
            if state.is_expected_logon_next_seq_num_sent() {
                // Mark state as if we have already sent a resend request from the logon's 789 (we sent) to infinity.
                // This will supress the resend request in doTargetTooHigh ...
                state.set_reset_range_from_last_expected_logon_next_seq_num();
                info!("Required resend will be suppressed as we are setting tag 789");
            }
            // TODO!
            // self.do_target_too_high(logon).await?;
        } else {
            state.incr_next_target_msg_seq_num();
            // nextQueued(timeStamp);
        }

        if self.session_settings.enable_next_expected_msg_seq_num {
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
                        drop(state);
                        self.send_sequence_reset(next_expected_msg_seq_num, end_seq_no)
                            .await;
                        state = self.state.borrow_mut();
                    } else {
                        // resend missed messages
                        info!(
                            "Received implicit ResendRequest via Logon FROM: {next_expected_msg_seq_num} \
                             TO: {next_sender_msg_num_at_logon_received} will be resent"
                        );
                        drop(state);
                        self.resend_range(next_expected_msg_seq_num, end_seq_no)
                            .await;
                        state = self.state.borrow_mut();
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

        Ok(None)
    }

    pub async fn on_message_in_impl(&self, msg: Box<FixtMessage>) -> Option<Disconnect> {
        let msg_type = msg.header.msg_type;
        let msg_seq_num = msg.header.msg_seq_num;
        trace!(msg_type = format!("{:?}<{}>", msg_type, msg_type.as_fix_str()));

        let result = match &msg.body {
            Message::Heartbeat(heartbeat) => self.on_heartbeat(msg).await,
            Message::TestRequest(test_request) => self.on_test_request(msg).await,
            Message::ResendRequest(resend_request) => self.on_resend_request(msg).await,
            Message::Reject(reject) => self.on_reject(msg).await,
            Message::SequenceReset(sequence_reset) => self.on_sequence_reset(msg).await,
            Message::Logout(logout) => {
                self.on_logout(msg).await;
                return Some(Disconnect);
            }
            Message::Logon(logon) => match self.on_logon(msg).await {
                Ok(Some(Disconnect)) => return Some(Disconnect),
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
                self.send_resend_request(msg_seq_num).await
            }
            Err(VerifyError::Reject {
                reason,
                tag,
                logout,
            }) => {
                let tag = tag.map(|t| t as i64);
                self.send_reject(
                    msg_type.as_fix_str().to_owned(),
                    msg_seq_num,
                    reason,
                    if let Some(tag) = tag {
                        FixString::from_ascii_lossy(format!("{reason:?} (tag={tag})").into_bytes())
                    } else {
                        FixString::from_ascii_lossy(format!("{reason:?}").into_bytes())
                    },
                    tag,
                )
                .await;
                if logout {
                    self.send_logout(None).await;
                }
            }
            Err(VerifyError::Logout { text, disconnect }) => {
                self.send_logout(text).await;
                if disconnect {
                    self.disconnect().await;
                }
            }
            Err(VerifyError::Disconnect(msg)) => {
                error!("disconnecting because of {msg}");
                self.disconnect().await;
            }
        }

        //self.state.borrow_mut().last_inbound_message_time = Instant::now();
        None
    }

    // TODO: Result<(), Disconnect>
    pub async fn on_message_in(&self, msg: Box<FixtMessage>) -> Option<Disconnect> {
        if let Some(disconnect) = self.on_message_in_impl(msg).await {
            return Some(disconnect);
        }
        let mut state = self.state.borrow_mut();
        while let Some(msg) = state.retrieve_msg() {
            drop(state);
            if let Some(disconnect) = self.on_message_in_impl(msg).await {
                return Some(disconnect);
            }
            state = self.state.borrow_mut();
        }
        None
    }

    pub async fn on_message_out(
        &self,
        msg: Box<FixtMessage>,
    ) -> Result<Option<Box<FixtMessage>>, Disconnect> {
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
                Ok(Some(receiver.await.unwrap()))
            }
            MsgCat::App => {
                self.emitter
                    .send(FixEventInternal::AppMsgOut(
                        Some(msg),
                        Responder::new(sender),
                    ))
                    .await;
                match receiver.await {
                    Ok(msg) => Ok(Some(msg)),
                    Err(gap_fill) => {
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
                        Ok(None)
                    }
                    Err(no_gap_fill) => Ok(None),
                }
            }
        }
    }

    pub async fn on_deserialize_error(&self, error: DeserializeError) {
        trace!("on_deserialize_error");

        // TODO: if msg_type is logon, handle missing CompId separately (disconnect)

        // Failed to parse the message. Discard the message.
        // Processing of the next valid FIX message will cause detection of
        // a sequence gap and a ResendRequest<2> will be generated.
        // TODO: comment above: add whre in doc it's written
        // TODO: Make sure infinite resend loop is not possible
        // TODO: It seems that some error types from the parser should end with
        //       Reject<3> message, but now for simplicity all kinds of errors
        //       results with message discard.
        let text = FixString::from_ascii_lossy(error.to_string().into_bytes());
        error!(deserialize_error = %text);
        if let DeserializeError::Reject {
            msg_type,
            seq_num,
            tag,
            reason,
        } = &error
        {
            self.send_reject(
                msg_type.clone(),
                *seq_num,
                *reason,
                text,
                tag.map(Int::from),
            )
            .await;
        }

        //let id = self.state.borrow().their_comp_id.clone();
        //let sub_id = self.state.borrow().their_sub_id.clone();

        self.emitter
            .send(FixEventInternal::DeserializeError(
                self.session_id().clone(),
                error,
            ))
            .await;

        //if let Err(msg_rejected) = self
        //    .msg_handler
        //    .on_parser_error(&id, &sub_id, parse_error)
        //    .await
        //{
        //    self.message_rejected(msg_rejected).await?;
        //    self.state.borrow_mut().increment_inbound_msg_seq_num()?;
        //}
    }

    pub async fn on_disconnect(self: &Rc<Self>) {
        trace!("on_disconnect");
        //self.state.borrow_mut().status = SessionStatus::Disconnected;
    }

    pub async fn on_io_error(self: &Rc<Self>, _error: io::Error) -> Result<(), Error> {
        trace!("on_io_error");
        //self.state.borrow_mut().status = SessionStatus::Disconnected; ??
        Ok(())
    }

    pub async fn on_in_timeout(self: &Rc<Self>) {
        trace!("on_in_timeout");
        let test_request = Box::new(FixtMessage {
            header: self.new_header(MsgType::TestRequest),
            body: Message::TestRequest(TestRequest {
                // Use current time as TestReqId as recommended in FIX Session
                // Protocol (FIX) Version 1.1 Errata March 2008
                test_req_id: FixString::from_ascii_lossy(
                    format!("{}", Utc::now().format("%Y%m%d-%H:%M:%S.%f")).into_bytes(),
                ),
            }),
            trailer: self.new_trailer(),
        });

        self.send(test_request).await;
    }

    pub async fn on_out_timeout(self: &Rc<Self>) {
        trace!("on_out_timeout");
        let heartbeat = Box::new(FixtMessage {
            header: self.new_header(MsgType::Heartbeat),
            body: Message::Heartbeat(Heartbeat { test_req_id: None }),
            trailer: self.new_trailer(),
        });
        self.send(heartbeat).await;
    }

    pub fn heartbeat_interval(&self) -> Duration {
        // TODO: logon.heartbeat_interval, value from settings is for n8 only (implement as Reject
        // on Logon)

        //let inbound_test_request_timeout_duration =
        //    self.settings.heartbeat_interval + NO_INBOUND_TIMEOUT_PADDING;
        self.settings.heartbeat_interval
    }
}
