use std::{collections::BTreeMap, ops::RangeInclusive};

use easyfix_messages::fields::SeqNum;

pub trait MessagesStorage {
    fn fetch_range(&mut self, range: RangeInclusive<SeqNum>) -> Vec<Vec<u8>>;
    fn store(&mut self, seq_num: SeqNum, data: &[u8]);

    fn next_sender_msg_seq_num(&self) -> SeqNum;
    fn next_target_msg_seq_num(&self) -> SeqNum;

    fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum);
    fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum);

    fn incr_next_sender_msg_seq_num(&mut self);
    fn incr_next_target_msg_seq_num(&mut self);

    fn reset(&mut self);
}

pub struct NullStorage {
    next_sender_msg_seq_num: SeqNum,
    next_target_msg_seq_num: SeqNum,
}

impl NullStorage {
    pub fn new() -> NullStorage {
        NullStorage {
            next_sender_msg_seq_num: 1,
            next_target_msg_seq_num: 1,
        }
    }
}

impl Default for NullStorage {
    fn default() -> NullStorage {
        NullStorage::new()
    }
}

impl MessagesStorage for NullStorage {
    // TODO: Iterator or Stream! Returned Vec can be extremly big (and slow to create)
    fn fetch_range(&mut self, _range: RangeInclusive<SeqNum>) -> Vec<Vec<u8>> {
        Vec::new()
    }

    fn store(&mut self, _seq_num: SeqNum, _data: &[u8]) {}

    fn next_sender_msg_seq_num(&self) -> SeqNum {
        self.next_sender_msg_seq_num
    }

    fn next_target_msg_seq_num(&self) -> SeqNum {
        self.next_target_msg_seq_num
    }

    fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.next_sender_msg_seq_num = seq_num;
    }

    fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.next_target_msg_seq_num = seq_num;
    }

    fn incr_next_sender_msg_seq_num(&mut self) {
        self.next_sender_msg_seq_num += 1;
    }

    fn incr_next_target_msg_seq_num(&mut self) {
        self.next_target_msg_seq_num += 1;
    }

    fn reset(&mut self) {
        self.next_sender_msg_seq_num = 1;
        self.next_target_msg_seq_num = 1;
    }
}

pub struct InMemoryStorage {
    next_sender_msg_seq_num: SeqNum,
    next_target_msg_seq_num: SeqNum,
    mem: BTreeMap<SeqNum, Vec<u8>>,
}

impl InMemoryStorage {
    pub fn new() -> InMemoryStorage {
        InMemoryStorage {
            next_sender_msg_seq_num: 1,
            next_target_msg_seq_num: 1,
            mem: BTreeMap::new(),
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> InMemoryStorage {
        InMemoryStorage::new()
    }
}

impl MessagesStorage for InMemoryStorage {
    // TODO: Stream!
    fn fetch_range(&mut self, _range: RangeInclusive<SeqNum>) -> Vec<Vec<u8>> {
        Vec::new()
    }

    fn store(&mut self, seq_num: SeqNum, data: &[u8]) {
        self.mem.insert(seq_num, data.to_vec());
    }

    fn next_sender_msg_seq_num(&self) -> SeqNum {
        self.next_sender_msg_seq_num
    }

    fn next_target_msg_seq_num(&self) -> SeqNum {
        self.next_target_msg_seq_num
    }

    fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.next_sender_msg_seq_num = seq_num;
    }

    fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.next_target_msg_seq_num = seq_num;
    }

    fn incr_next_sender_msg_seq_num(&mut self) {
        self.next_sender_msg_seq_num += 1;
    }

    fn incr_next_target_msg_seq_num(&mut self) {
        self.next_target_msg_seq_num += 1;
    }

    fn reset(&mut self) {
        self.next_sender_msg_seq_num = 1;
        self.next_target_msg_seq_num = 1;
        self.mem.clear();
    }
}
