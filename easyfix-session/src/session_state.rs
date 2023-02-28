use std::{collections::BTreeMap, ops::RangeInclusive};

use easyfix_messages::{
    fields::{FixStr, FixString, Int, SeqNum},
    messages::FixtMessage,
};
use tokio::time::{Duration, Instant};
use tracing::info;

use crate::messages_storage::MessagesStorage;

#[derive(Debug)]
struct Messages(BTreeMap<SeqNum, Box<FixtMessage>>);

impl Messages {
    fn new() -> Messages {
        Messages(BTreeMap::new())
    }

    fn enqueue(&mut self, seq_num: SeqNum, msg: Box<FixtMessage>) {
        self.0.insert(seq_num, msg);
    }

    fn retrieve(&mut self, seq_num: SeqNum) -> Option<Box<FixtMessage>> {
        self.0.remove(&seq_num)
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

#[derive(Debug)]
pub(crate) struct State<S> {
    enabled: bool,
    received_logon: bool,
    sent_logout: bool,
    sent_logon: bool,
    sent_reset: bool,
    reset_received: bool,
    initiate: bool,
    logon_timeout: Duration,
    logout_timeout: Duration,
    test_request: Int,
    resend_range: Option<RangeInclusive<SeqNum>>,
    heart_bt_int: Int,
    last_sent_time: Instant,
    last_received_time: Instant,
    // TODO: enum
    logout_reason: Option<FixString>,

    /// If this is anything other than zero it's the value of
    /// the 789/NextExpectedMsgSeqNum tag in the last Logon message sent.
    /// It is used to determine if the recipient has enough information
    /// (assuming they support 789) to avoid the need for a resend request i.e.
    /// they should be resending any necessary missing messages already.
    /// This value is used to populate the resendRange if necessary.
    next_expected_msg_seq_num: SeqNum,

    queue: Messages,
    messages_storage: S,
}

impl<S: MessagesStorage> State<S> {
    pub(crate) fn new(messages_storage: S) -> State<S> {
        State {
            enabled: true,
            received_logon: false,
            sent_logout: false,
            sent_logon: false,
            sent_reset: false,
            reset_received: false,
            initiate: false,
            logon_timeout: Duration::from_secs(10),
            logout_timeout: Duration::from_secs(2),
            test_request: 0,
            resend_range: None,
            heart_bt_int: 10,
            last_sent_time: Instant::now(),
            last_received_time: Instant::now(),
            // TODO: enum
            logout_reason: None,
            next_expected_msg_seq_num: 0,
            queue: Messages::new(),
            messages_storage,
        }
    }

    ////

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn logon_received(&self) -> bool {
        self.received_logon
    }

    pub fn set_logon_received(&mut self, logon_received: bool) {
        self.received_logon = logon_received;
    }

    pub fn logout_sent(&self) -> bool {
        self.sent_logout
    }

    pub fn set_logout_sent(&mut self, logout_sent: bool) {
        self.sent_logout = logout_sent;
    }

    pub fn logon_sent(&self) -> bool {
        self.sent_logon
    }

    pub fn set_logon_sent(&mut self, logon_sent: bool) {
        self.sent_logon = logon_sent;
    }

    pub fn reset_received(&self) -> bool {
        self.reset_received
    }

    pub fn set_reset_received(&mut self, reset_received: bool) {
        self.reset_received = reset_received;
    }

    pub fn reset_sent(&self) -> bool {
        self.sent_reset
    }

    pub fn set_reset_sent(&mut self, reset_sent: bool) {
        self.sent_reset = reset_sent;
    }

    pub fn initiate(&self) -> bool {
        return self.initiate;
    }

    pub fn set_initiate(&mut self, initiate: bool) {
        self.initiate = initiate;
    }

    pub fn logon_timeout(&self) -> Duration {
        self.logon_timeout
    }

    pub fn set_logon_timeout(&mut self, logon_timeout: Duration) {
        self.logon_timeout = logon_timeout;
    }

    pub fn logout_timeout(&self) -> Duration {
        self.logout_timeout
    }

    pub fn set_logout_timeout(&mut self, logout_timeout: Duration) {
        self.logout_timeout = logout_timeout;
    }

    pub fn test_request(&self) -> Int {
        self.test_request
    }

    pub fn set_test_request(&mut self, test_request: Int) {
        self.test_request = test_request;
    }

    pub fn resend_requested(&self) -> bool {
        self.resend_range.is_some()
    }

    pub fn resend_range(&self) -> Option<&RangeInclusive<SeqNum>> {
        self.resend_range.as_ref()
    }

    pub fn set_resend_range(&mut self, resend_range: Option<RangeInclusive<SeqNum>>) {
        self.resend_range = resend_range;
    }

    /*
    MessageStore* store() { return m_pStore; }
    void store( MessageStore* pValue ) { m_pStore = pValue; }
    Log* log() { return m_pLog ? m_pLog : &m_nullLog; }
    void log( Log* pValue ) { m_pLog = pValue; }
    */

    pub fn heart_bt_int(&self) -> Int {
        self.heart_bt_int
    }

    pub fn set_heart_bt_int(&mut self, heart_bt_int: Int) {
        self.heart_bt_int = heart_bt_int;
    }

    pub fn last_sent_time(&self) -> Instant {
        self.last_sent_time
    }

    pub fn set_last_sent_time(&mut self, last_sent_time: Instant) {
        self.last_sent_time = last_sent_time;
    }

    pub fn last_received_time(&self) -> Instant {
        self.last_received_time
    }

    pub fn set_last_received_time(&mut self, last_received_time: Instant) {
        self.last_received_time = last_received_time;
    }

    pub fn should_send_logon(&self) -> bool {
        self.initiate() && !self.logon_sent()
    }

    pub fn logon_already_sent(&self) -> bool {
        self.initiate() && self.logon_sent()
    }

    pub fn logon_timed_out(&self) -> bool {
        Instant::now() - self.last_received_time() >= self.logon_timeout()
    }

    pub fn logout_timed_out(&self) -> bool {
        self.logout_sent() && (Instant::now() - self.last_sent_time() >= self.logout_timeout())
    }

    // TODO: remove AS
    pub fn within_heartbeat(&self) -> bool {
        let now = Instant::now();
        (now - self.last_sent_time() < Duration::from_secs(self.heart_bt_int() as u64))
            && (now - self.last_received_time() < Duration::from_secs(self.heart_bt_int() as u64))
    }

    pub fn timed_out(&self) -> bool {
        (Instant::now() - self.last_received_time())
            >= Duration::from_secs(self.heart_bt_int() as u64).mul_f64(2.4)
    }

    pub fn need_heartbeat(&self) -> bool {
        (Instant::now() - self.last_sent_time() >= Duration::from_secs(self.heart_bt_int() as u64))
            && self.test_request() == 0
    }

    pub fn need_test_request(&self) -> bool {
        Instant::now() - self.last_received_time()
            >= Duration::from_secs(
                (10 * (self.test_request() + 1) * self.heart_bt_int()) as u64 / 12,
            )
    }

    pub fn logout_reason(&self) -> Option<&FixStr> {
        self.logout_reason.as_deref()
    }

    pub fn set_logout_reason(&mut self, logout_reason: Option<FixString>) {
        self.logout_reason = logout_reason;
    }

    /// No actual resend request has occurred but at logon we populated tag
    /// 789 so that the other side knows we are missing messages without
    /// an explicit resend request and should immediately reply with
    /// the missing messages.
    ///
    /// This is expected to be called only in the scenario where target is too
    /// high on logon and tag 789 is supported.
    pub fn set_reset_range_from_last_expected_logon_next_seq_num(&mut self) {
        // we have already requested all msgs from nextExpectedMsgSeqNum to infinity
        self.set_resend_range(Some(self.next_expected_msg_seq_num..=0));
        // clean up the variable (not really needed)
        self.next_expected_msg_seq_num = 0;
    }

    pub fn set_last_expected_logon_next_seq_num(&mut self, seq_num: SeqNum) {
        self.next_expected_msg_seq_num = seq_num;
    }

    pub fn get_last_expected_logon_next_seq_num(&self) -> SeqNum {
        self.next_expected_msg_seq_num
    }

    pub fn is_expected_logon_next_seq_num_sent(&self) -> bool {
        self.next_expected_msg_seq_num != 0
    }

    pub fn enqueue_msg(&mut self, msg: Box<FixtMessage>) {
        self.queue.enqueue(msg.header.msg_seq_num, msg);
    }

    pub fn retrieve_msg(&mut self) -> Option<Box<FixtMessage>> {
        self.queue.retrieve(self.next_target_msg_seq_num())
    }

    pub fn clear_queue(&mut self, seq_num: SeqNum) {
        self.queue.clear();
    }

    // TODO: change API to retrieve message data as output parameter, to save
    //       on Vec allocations
    pub fn fetch(&mut self, seq_num: SeqNum) -> Result<Vec<u8>, S::Error> {
        self.messages_storage.fetch(seq_num)
    }

    pub fn store(&mut self, seq_num: SeqNum, data: &[u8]) -> Result<(), S::Error> {
        self.messages_storage.store(seq_num, data)
    }

    pub fn next_sender_msg_seq_num(&self) -> SeqNum {
        self.messages_storage.next_sender_msg_seq_num()
    }

    pub fn next_target_msg_seq_num(&self) -> SeqNum {
        self.messages_storage.next_target_msg_seq_num()
    }

    pub fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.messages_storage.set_next_sender_msg_seq_num(seq_num);
    }

    pub fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.messages_storage.set_next_target_msg_seq_num(seq_num);
    }

    pub fn incr_next_sender_msg_seq_num(&mut self) {
        self.messages_storage.incr_next_sender_msg_seq_num();
    }

    pub fn incr_next_target_msg_seq_num(&mut self) {
        info!(
            "old target msg seq num: {}",
            self.messages_storage.next_target_msg_seq_num()
        );
        self.messages_storage.incr_next_target_msg_seq_num();
        info!(
            "current target msg seq num: {}",
            self.messages_storage.next_target_msg_seq_num()
        );
    }

    //UtcTimeStamp getCreationTime() const EXCEPT ( IOException )
    //{ Locker l( m_mutex ); return m_pStore->getCreationTime(); }

    pub fn reset(&mut self) {
        self.messages_storage.reset();
    }

    pub fn refresh(&mut self) {
        unimplemented!();
        //{ Locker l( m_mutex ); m_pStore->refresh(); }
    }

    /*
    void clear()
    { if ( !m_pLog ) return ; Locker l( m_mutex ); m_pLog->clear(); }
    void backup()
    { if ( !m_pLog ) return ; Locker l( m_mutex ); m_pLog->backup(); }
    void onIncoming( const std::string& string )
    { if ( !m_pLog ) return ; Locker l( m_mutex ); m_pLog->onIncoming( string ); }
    void onOutgoing( const std::string& string )
    { if ( !m_pLog ) return ; Locker l( m_mutex ); m_pLog->onOutgoing( string ); }
    void onEvent( const std::string& string )
    { if ( !m_pLog ) return ; Locker l( m_mutex ); m_pLog->onEvent( string ); }
      */
}