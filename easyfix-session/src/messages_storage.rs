use std::{collections::BTreeMap, convert::Infallible, ops::RangeInclusive};

use easyfix_messages::fields::SeqNum;

pub trait MessagesStorage {
    type Error: std::error::Error;

    fn fetch(&mut self, seq_num: SeqNum) -> Result<Vec<u8>, Self::Error>;
    fn fetch_range(&mut self, range: RangeInclusive<SeqNum>) -> Result<Vec<Vec<u8>>, Self::Error>;
    fn store(&mut self, seq_num: SeqNum, data: &[u8]) -> Result<(), Self::Error>;

    //bool set( int s, const std::string& m ) EXCEPT ( IOException )
    //{ Locker l( m_mutex ); return m_pStore->set( s, m ); }

    //void get( int b, int e, std::vector < std::string > &m ) const
    //EXCEPT ( IOException )
    //{ Locker l( m_mutex ); m_pStore->get( b, e, m ); }

    fn next_sender_msg_seq_num(&self) -> SeqNum;
    fn next_target_msg_seq_num(&self) -> SeqNum;

    fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum);
    fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum);

    fn incr_next_sender_msg_seq_num(&mut self);
    fn incr_next_target_msg_seq_num(&mut self);

    //UtcTimeStamp getCreationTime() const EXCEPT ( IOException )
    //{ Locker l( m_mutex ); return m_pStore->getCreationTime(); }

    fn reset(&mut self) -> Result<(), Self::Error>;
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

impl MessagesStorage for NullStorage {
    type Error = Infallible;

    fn fetch(&mut self, _seq_num: SeqNum) -> Result<Vec<u8>, Self::Error> {
        Ok(Vec::new())
    }

    // TODO: Stream!
    fn fetch_range(&mut self, _range: RangeInclusive<SeqNum>) -> Result<Vec<Vec<u8>>, Self::Error> {
        Ok(Vec::new())
    }

    fn store(&mut self, _seq_num: SeqNum, _data: &[u8]) -> Result<(), Self::Error> {
        Ok(())
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

    fn reset(&mut self) -> Result<(), Self::Error> {
        self.next_sender_msg_seq_num = 1;
        self.next_target_msg_seq_num = 1;
        Ok(())
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

impl MessagesStorage for InMemoryStorage {
    type Error = Infallible;

    fn fetch(&mut self, seq_num: SeqNum) -> Result<Vec<u8>, Self::Error> {
        Ok(self.mem[&seq_num].clone())
    }

    // TODO: Stream!
    fn fetch_range(&mut self, _range: RangeInclusive<SeqNum>) -> Result<Vec<Vec<u8>>, Self::Error> {
        Ok(Vec::new())
    }

    fn store(&mut self, seq_num: SeqNum, data: &[u8]) -> Result<(), Self::Error> {
        self.mem.insert(seq_num, data.to_vec());
        Ok(())
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

    fn reset(&mut self) -> Result<(), Self::Error> {
        self.next_sender_msg_seq_num = 1;
        self.next_target_msg_seq_num = 1;
        self.mem.clear();
        Ok(())
    }
}
