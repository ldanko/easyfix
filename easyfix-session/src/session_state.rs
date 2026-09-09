use std::{
    collections::{BTreeMap, HashSet},
    ops::RangeInclusive,
    time::Instant,
};

use easyfix_messages::{
    fields::{FixString, SeqNum},
    messages::FixtMessage,
};
use tracing::{instrument, trace};

use crate::{Abort, DisconnectReason, messages_storage::MessagesStorage};

#[derive(Debug)]
struct Messages(BTreeMap<SeqNum, Box<FixtMessage>>);

impl Messages {
    fn new() -> Messages {
        Messages(BTreeMap::new())
    }

    fn first_seq(&self) -> Option<SeqNum> {
        self.0.keys().next().copied()
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
pub struct State<S> {
    enabled: bool,
    received_logon: bool,
    logon_sent: bool,
    logout_sent_time: Option<Instant>,
    reset_sent: bool,
    reset_received: bool,
    initiate: bool,
    resend_range: Option<RangeInclusive<SeqNum>>,
    last_sent_time: Instant,
    last_received_time: Instant,

    disconnected: bool,
    seq_num_numbering_closed: bool,
    abort: Option<Abort>,

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
        let seq_num_numbering_closed = messages_storage.next_sender_msg_seq_num() == SeqNum::MAX
            || messages_storage.next_target_msg_seq_num() == SeqNum::MAX;
        State {
            enabled: true,
            received_logon: false,
            logon_sent: false,
            logout_sent_time: None,
            reset_sent: false,
            reset_received: false,
            initiate: false,
            resend_range: None,
            last_sent_time: Instant::now(),
            last_received_time: Instant::now(),
            disconnected: true,
            seq_num_numbering_closed,
            abort: None,
            next_expected_msg_seq_num: 0,
            queue: Messages::new(),
            messages_storage,
            grace_period_test_req_ids: HashSet::new(),
        }
    }

    ////

    pub fn messages_storage(&self) -> &S {
        &self.messages_storage
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn logon_received(&self) -> bool {
        self.received_logon
    }

    pub fn set_logon_received(&mut self, logon_received: bool) {
        self.received_logon = logon_received;
    }

    pub fn logon_sent(&self) -> bool {
        self.logon_sent
    }

    pub fn set_logon_sent(&mut self, logon_sent: bool) {
        self.logon_sent = logon_sent;
    }

    pub fn logout_sent_time(&self) -> Option<Instant> {
        self.logout_sent_time
    }

    pub fn set_logout_sent_time(&mut self, logout_sent: bool) {
        if logout_sent {
            self.logout_sent_time = Some(Instant::now());
        } else {
            self.logout_sent_time = None;
        }
    }

    pub fn reset_received(&self) -> bool {
        self.reset_received
    }

    pub fn set_reset_received(&mut self, reset_received: bool) {
        self.reset_received = reset_received;
    }

    pub fn reset_sent(&self) -> bool {
        self.reset_sent
    }

    pub fn set_reset_sent(&mut self, reset_sent: bool) {
        self.reset_sent = reset_sent;
    }

    pub fn initiate(&self) -> bool {
        self.initiate
    }

    pub fn set_resend_range(&mut self, resend_range: RangeInclusive<SeqNum>) {
        self.resend_range = Some(resend_range);
    }

    pub fn reset_resend_range(&mut self) {
        self.resend_range = None;
    }

    pub fn resend_range(&self) -> Option<RangeInclusive<SeqNum>> {
        self.resend_range.clone()
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
        self.set_resend_range(self.next_expected_msg_seq_num..=0);
        // clean up the variable (not really needed)
        self.next_expected_msg_seq_num = 0;
    }

    pub fn set_last_expected_logon_next_seq_num(&mut self, seq_num: SeqNum) {
        self.next_expected_msg_seq_num = seq_num;
    }

    pub fn is_expected_logon_next_seq_num_sent(&self) -> bool {
        self.next_expected_msg_seq_num != 0
    }

    #[instrument(skip_all)]
    pub fn enqueue_msg(&mut self, msg: Box<FixtMessage>) {
        trace!(msg_seq_num = msg.header.msg_seq_num, msg_type = ?msg.msg_type());
        if !self.aborted() {
            self.queue.enqueue(msg.header.msg_seq_num, msg);
        }
    }

    pub fn lowest_queued_seq_num(&self) -> Option<SeqNum> {
        self.queue.first_seq()
    }

    pub fn retrieve_msg(&mut self) -> Option<Box<FixtMessage>> {
        if self.aborted() {
            return None;
        }
        self.queue.retrieve(self.next_target_msg_seq_num())
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }

    pub fn fetch_range(&mut self, range: RangeInclusive<SeqNum>) -> impl Iterator<Item = &[u8]> {
        self.messages_storage.fetch_range(range)
    }

    pub fn store(&mut self, seq_num: SeqNum, data: &[u8]) {
        if !self.aborted() {
            self.messages_storage.store(seq_num, data);
        }
    }

    pub fn next_sender_msg_seq_num(&self) -> SeqNum {
        self.messages_storage.next_sender_msg_seq_num()
    }

    pub fn next_target_msg_seq_num(&self) -> SeqNum {
        self.messages_storage.next_target_msg_seq_num()
    }

    pub(crate) fn attach_abort(&mut self, abort: Abort) {
        self.abort = Some(abort);
        self.check_numbering();
    }

    fn aborted(&self) -> bool {
        self.abort
            .as_ref()
            .is_some_and(|abort| abort.reason().is_some())
    }

    fn check_numbering(&mut self) {
        self.seq_num_numbering_closed |= self.next_sender_msg_seq_num() == SeqNum::MAX
            || self.next_target_msg_seq_num() == SeqNum::MAX;
        if self.seq_num_numbering_closed
            && let Some(abort) = &self.abort
        {
            abort.request(DisconnectReason::SequenceNumberExhausted);
        }
    }

    pub fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum) {
        if self.aborted() && !self.disconnected {
            return;
        }
        self.messages_storage.set_next_sender_msg_seq_num(seq_num);
        self.check_numbering();
    }

    pub fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum) {
        if self.aborted() {
            return;
        }
        self.messages_storage.set_next_target_msg_seq_num(seq_num);
        self.check_numbering();
    }

    pub fn incr_next_sender_msg_seq_num(&mut self) {
        self.check_numbering();
        if self.seq_num_numbering_closed || self.aborted() {
            return;
        }
        self.messages_storage.incr_next_sender_msg_seq_num();
        self.check_numbering();
    }

    pub fn incr_next_target_msg_seq_num(&mut self) {
        self.check_numbering();
        if self.seq_num_numbering_closed || self.aborted() {
            return;
        }
        self.messages_storage.incr_next_target_msg_seq_num();
        self.check_numbering();
    }

    pub fn reset(&mut self) {
        if self.aborted() || self.seq_num_numbering_closed {
            return;
        }
        self.messages_storage.reset();
        self.check_numbering();
    }

    /// Only the acceptor's explicit reset of an inactive session may reopen numbering.
    pub(crate) fn reset_numbering(&mut self) {
        self.messages_storage.reset();
        self.seq_num_numbering_closed = false;
        self.abort = None;
        self.check_numbering();
    }

    pub fn disconnect(&mut self, reset: bool) {
        self.set_disconnected(true);

        self.set_logout_sent_time(false);
        self.set_reset_received(false);
        self.set_reset_sent(false);
        self.set_last_expected_logon_next_seq_num(0);
        if reset && !self.aborted() {
            self.reset();
        }

        self.reset_resend_range();
        self.clear_queue();
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

#[cfg(test)]
mod tests {
    use easyfix_messages::fields::SeqNum;
    use tokio::sync::mpsc;

    use super::State;
    use crate::{DisconnectReason, Sender, messages_storage::InMemoryStorage};

    #[test]
    fn exhausted_counter_latches_until_explicit_reset() {
        for outgoing in [false, true] {
            let mut state = State::new(InMemoryStorage::new());
            let (tx, _rx) = mpsc::unbounded_channel();
            let sender = Sender::new(tx);
            state.attach_abort(sender.abort.clone());
            state.set_disconnected(false);
            if outgoing {
                state.set_next_sender_msg_seq_num(SeqNum::MAX - 1);
                state.incr_next_sender_msg_seq_num();
            } else {
                state.set_next_target_msg_seq_num(SeqNum::MAX - 1);
                state.incr_next_target_msg_seq_num();
            }
            assert!(state.seq_num_numbering_closed);
            assert!(matches!(
                sender.abort.reason(),
                Some(DisconnectReason::SequenceNumberExhausted)
            ));
            let counters = (
                state.next_sender_msg_seq_num(),
                state.next_target_msg_seq_num(),
            );
            state.incr_next_sender_msg_seq_num();
            state.incr_next_target_msg_seq_num();
            state.reset();
            state.disconnect(true);
            state.reset();
            assert_eq!(
                (
                    state.next_sender_msg_seq_num(),
                    state.next_target_msg_seq_num()
                ),
                counters
            );
            assert!(state.seq_num_numbering_closed);
            state.reset_numbering();
            assert!(!state.seq_num_numbering_closed);
            assert_eq!(
                (
                    state.next_sender_msg_seq_num(),
                    state.next_target_msg_seq_num()
                ),
                (1, 1)
            );
            // Reset does not revive an old connection's terminal handle.
            assert!(sender.abort.reason().is_some());
        }
    }

    #[test]
    fn ordinary_disconnect_reset_keeps_live_abort_handle() {
        let mut state = State::new(InMemoryStorage::new());
        let (tx, _rx) = mpsc::unbounded_channel();
        let sender = Sender::new(tx);
        state.attach_abort(sender.abort.clone());
        state.set_disconnected(false);
        state.disconnect(true);
        // Output can still be draining after an ordinary disconnect.
        state.set_next_sender_msg_seq_num(SeqNum::MAX);
        assert!(matches!(
            sender.abort.reason(),
            Some(DisconnectReason::SequenceNumberExhausted)
        ));
        state.reset();
        assert!(state.seq_num_numbering_closed);
    }
}
