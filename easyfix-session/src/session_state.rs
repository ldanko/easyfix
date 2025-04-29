use std::{
    collections::{BTreeMap, HashSet},
    ops::RangeInclusive,
};

use easyfix_messages::{
    fields::{FixString, Int, SeqNum},
    messages::FixtMessage,
};
use tokio::time::Instant;

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
    test_request: Int,
    resend_range: Option<RangeInclusive<SeqNum>>,
    heart_bt_int: Int,
    last_sent_time: Instant,
    last_received_time: Instant,

    disconnected: bool,

    /// If this is anything other than zero it's the value of
    /// the 789/NextExpectedMsgSeqNum tag in the last Logon message sent.
    /// It is used to determine if the recipient has enough information
    /// (assuming they support 789) to avoid the need for a resend request i.e.
    /// they should be resending any necessary missing messages already.
    /// This value is used to populate the resendRange if necessary.
    next_expected_msg_seq_num: SeqNum,

    queue: Messages,
    messages_storage: S,

    grace_period_test_req_ids: HashSet<FixString>,
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
            test_request: 0,
            resend_range: None,
            heart_bt_int: 10,
            last_sent_time: Instant::now(),
            last_received_time: Instant::now(),
            disconnected: true,
            next_expected_msg_seq_num: 0,
            queue: Messages::new(),
            messages_storage,
            grace_period_test_req_ids: HashSet::new(),
        }
    }

    ////

    pub fn enabled(&self) -> bool {
        self.enabled
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
        self.initiate
    }

    pub fn set_test_request(&mut self, test_request: Int) {
        self.test_request = test_request;
    }

    pub fn set_resend_range(&mut self, resend_range: Option<RangeInclusive<SeqNum>>) {
        self.resend_range = resend_range;
    }

    pub fn resend_range(&self) -> Option<RangeInclusive<SeqNum>> {
        self.resend_range.clone()
    }

    pub fn heart_bt_int(&self) -> Int {
        self.heart_bt_int
    }

    pub fn set_heart_bt_int(&mut self, heart_bt_int: Int) {
        self.heart_bt_int = heart_bt_int;
    }

    pub fn set_last_sent_time(&mut self, last_sent_time: Instant) {
        self.last_sent_time = last_sent_time;
    }

    pub fn set_last_received_time(&mut self, last_received_time: Instant) {
        self.last_received_time = last_received_time;
    }

    pub fn should_send_logon(&self) -> bool {
        self.initiate() && !self.logon_sent()
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

    pub fn is_expected_logon_next_seq_num_sent(&self) -> bool {
        self.next_expected_msg_seq_num != 0
    }

    pub fn enqueue_msg(&mut self, msg: Box<FixtMessage>) {
        self.queue.enqueue(msg.header.msg_seq_num, msg);
    }

    pub fn retrieve_msg(&mut self) -> Option<Box<FixtMessage>> {
        self.queue.retrieve(self.next_target_msg_seq_num())
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }

    pub fn fetch_range(&mut self, range: RangeInclusive<SeqNum>) -> impl Iterator<Item = &[u8]> {
        self.messages_storage.fetch_range(range)
    }

    pub fn store(&mut self, seq_num: SeqNum, data: &[u8]) {
        self.messages_storage.store(seq_num, data);
    }

    pub fn next_sender_msg_seq_num(&self) -> SeqNum {
        self.messages_storage.next_sender_msg_seq_num()
    }

    pub fn next_target_msg_seq_num(&self) -> SeqNum {
        self.messages_storage.next_target_msg_seq_num()
    }

    pub fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.messages_storage.set_next_sender_msg_seq_num(seq_num)
    }

    pub fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.messages_storage.set_next_target_msg_seq_num(seq_num)
    }

    pub fn incr_next_sender_msg_seq_num(&mut self) {
        self.messages_storage.incr_next_sender_msg_seq_num();
    }

    pub fn incr_next_target_msg_seq_num(&mut self) {
        self.messages_storage.incr_next_target_msg_seq_num();
    }

    pub fn reset(&mut self) {
        self.messages_storage.reset();
    }

    pub fn disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn set_disconnected(&mut self, disconnected: bool) {
        self.disconnected = disconnected;
    }

    pub fn input_timeout_cnt(&self) -> usize {
        self.grace_period_test_req_ids.len()
    }

    pub fn register_grace_period_test_req_id(&mut self, test_req_id: FixString) {
        self.grace_period_test_req_ids.insert(test_req_id);
    }

    pub fn validate_grace_period_test_req_id(&mut self, test_req_id: &FixString) {
        if self.grace_period_test_req_ids.contains(test_req_id) {
            self.reset_grace_period();
        }
    }

    pub fn reset_grace_period(&mut self) {
        self.grace_period_test_req_ids.clear();
    }
}
