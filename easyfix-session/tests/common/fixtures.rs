//! Fixtures shared by every test of `easyfix-session`, unit and integration
//! alike.
//!
//! One file, compiled into each consumer: every `tests/*.rs` binary includes
//! it via `#[path = "common/fixtures.rs"] mod common;`, and the crate's own
//! unit tests via `#[path = "../tests/common/fixtures.rs"]` in
//! `src/test_helpers.rs`. That is why everything here names the crate as an
//! external user would (`easyfix_session::...`, resolved inside the crate by
//! its `extern crate self` alias) and touches only the public API. A helper
//! that needs crate internals belongs in `src/test_helpers.rs` instead. Each
//! consumer uses a different subset, so unused-item warnings are suppressed
//! module-wide.
//!
//! This module holds *preparation* only - session settings, message/wire
//! fixtures, and frame I/O. Assertions and per-test verdicts (e.g.
//! `is_new_order_single`, `has_data_on_wire`, the event/app harnesses) stay in
//! each test file so each test keeps owning what it verifies.
#![allow(
    dead_code,
    reason = "every consumer compiles its own copy and uses a different subset"
)]

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::{NonZeroU8, NonZeroU16, NonZeroUsize},
    rc::Rc,
    time::Duration,
};

use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, HeaderBase, HeartbeatBase, LogonBase, LogoutBase,
        TestRequestBase,
    },
    basic_types::{
        ApplVerId, FixStr, Int, NonZeroLength, NonZeroSeqNum, SeqNum, TimePrecision, UtcTimestamp,
    },
    deserializer::{RawMessageError, raw_message},
    fix_str,
    message::SessionMessage,
};
use easyfix_session::{
    ConnectionDropReason, ConnectionObserver, InMemoryStorage, InMemoryStorageError,
    MessagesStorage, SerializeError, SessionId, SessionSettings, StoreError,
};
use easyfix_test_messages::{Body, Header, Message, NewOrderSingle, Trailer};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Max message size shared by the test harnesses.
pub const DEFAULT_MAX_MESSAGE_SIZE: NonZeroLength = const { NonZeroLength::new(4096).unwrap() };

/// [`DEFAULT_MAX_MESSAGE_SIZE`] as a buffer length, for allocation and
/// storage constructors that take a plain `usize`.
pub const DEFAULT_MAX_MESSAGE_SIZE_BYTES: usize = DEFAULT_MAX_MESSAGE_SIZE.get() as usize;

/// Generous bound for awaiting an expected event/message.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Stand-in peer address for tests that drive a session task over an
/// in-memory duplex, which has no address of its own.
pub const TEST_PEER_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9876);

/// Baseline session settings for the test harnesses. Sets
/// `manages_admin_output = false`; the managed-admin-output tests flip that
/// field on the returned value so the flag-under-test stays visible at the
/// call site.
// Deliberately a full struct literal (no `..Default::default()`): adding a
// settings field forces the compiler to demand a decision here - and this
// is the only such literal in the test code, so it demands it once.
pub fn build_session_settings(heartbeat_secs: u16) -> SessionSettings {
    SessionSettings {
        heartbeat_interval: Some(
            NonZeroU16::new(heartbeat_secs).expect("heartbeat_secs must be non-zero"),
        ),
        auto_disconnect_after_no_heartbeat: const { NonZeroU8::new(1).unwrap() },
        auto_disconnect_after_no_logout: Duration::from_secs(10),
        auto_disconnect_after_no_logon_response: Duration::from_secs(10),
        write_timeout: Duration::from_secs(30),
        max_message_size: DEFAULT_MAX_MESSAGE_SIZE,
        send_redundant_resend_requests: false,
        check_comp_id: true,
        max_latency: Some(Duration::from_secs(120)),
        time_precision: TimePrecision::Millis,
        sender_default_appl_ver_id: ApplVerId::Fix50Sp2,
        enable_next_expected_msg_seq_num: false,
        verify_logout: true,
        verify_test_request_id: true,
        manages_admin_output: false,
        persist_messages: true,
        max_outbound_queued_messages: None,
        max_outbound_lag: None,
        // Baseline reset tests explicitly permit both methods; production
        // defaults require opting in to each bilateral agreement.
        accept_reset_on_connect: true,
        accept_reset_in_session: true,
        running_session_reset_timeout: Duration::from_secs(30),
        resend_batch_size: const { NonZeroUsize::new(1).unwrap() },
        queued_batch_size: const { NonZeroUsize::new(1).unwrap() },
    }
}

/// Build a `NewOrderSingle` with an empty header, as an application stages
/// it: the engine fills the header at transmit time, so tests must not
/// pre-populate it.
pub fn new_order_single_with_empty_header() -> Box<Message> {
    Box::new(Message {
        header: Header::default(),
        body: Box::new(Body::NewOrderSingle(NewOrderSingle {
            cl_ord_id: fix_str!("ORD001").to_owned(),
            symbol: fix_str!("SYMBOL1").to_owned(),
            transact_time: UtcTimestamp::now(TimePrecision::Nanos),
            ..NewOrderSingle::default()
        })),
        trailer: Trailer::default(),
    })
}

/// Serialize a message into a freshly-allocated `Vec<u8>` sized for the test
/// buffer. Pure fixture - no assertions on the message's content.
pub fn serialize_message(msg: &Message) -> Vec<u8> {
    let mut buf = vec![0u8; DEFAULT_MAX_MESSAGE_SIZE_BYTES];
    let len = msg.serialize(&mut buf).expect("serialize failed");
    buf.truncate(len);
    buf
}

/// Build a header from the peer's perspective. `peer_sid` must already be
/// reversed relative to the local side (i.e. `local.reverse_route()`).
pub fn peer_header(peer_sid: &SessionId, seq: SeqNum) -> HeaderBase<'static> {
    HeaderBase {
        sender_comp_id: Cow::Owned(peer_sid.sender_comp_id().to_owned()),
        target_comp_id: Cow::Owned(peer_sid.target_comp_id().to_owned()),
        msg_seq_num: seq,
        sending_time: UtcTimestamp::now(TimePrecision::Nanos),
        poss_dup_flag: None,
        orig_sending_time: None,
        appl_ver_id: None,
    }
}

pub fn peer_logon_bytes(peer_sid: &SessionId, seq: SeqNum, heart_bt_int: Int) -> Vec<u8> {
    serialize_message(&Message::from_admin(
        peer_header(peer_sid, seq),
        AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: 0,
            heart_bt_int,
            reset_seq_num_flag: None,
            max_message_size: None,
            next_expected_msg_seq_num: None,
            default_appl_ver_id: Some(ApplVerId::Fix50Sp2),
            session_status: None,
        }),
    ))
}

pub fn peer_test_request_bytes(
    peer_sid: &SessionId,
    seq: SeqNum,
    test_req_id: &'static FixStr,
) -> Vec<u8> {
    serialize_message(&Message::from_admin(
        peer_header(peer_sid, seq),
        AdminBase::TestRequest(TestRequestBase {
            test_req_id: Cow::Borrowed(test_req_id),
        }),
    ))
}

pub fn peer_heartbeat_bytes(
    peer_sid: &SessionId,
    seq: SeqNum,
    test_req_id: Option<&'static FixStr>,
) -> Vec<u8> {
    serialize_message(&Message::from_admin(
        peer_header(peer_sid, seq),
        AdminBase::Heartbeat(HeartbeatBase {
            test_req_id: test_req_id.map(Cow::Borrowed),
        }),
    ))
}

pub fn peer_logout_bytes(peer_sid: &SessionId, seq: SeqNum) -> Vec<u8> {
    serialize_message(&Message::from_admin(
        peer_header(peer_sid, seq),
        AdminBase::Logout(LogoutBase {
            session_status: None,
            text: None,
        }),
    ))
}

/// [`ConnectionObserver`] that records every drop it is told about.
/// `ConnectionDropReason` borrows from the session task, so each event is
/// flattened into an owned summary, with the identity-carrying reasons
/// rendered as `<kind>:<session id>`.
#[derive(Default)]
pub struct RecordingObserver {
    seen: RefCell<Vec<(SocketAddr, String)>>,
}

impl RecordingObserver {
    /// The summaries of every drop so far, in order.
    pub fn summaries(&self) -> Vec<String> {
        self.seen
            .borrow()
            .iter()
            .map(|(_, summary)| summary.clone())
            .collect()
    }

    /// The peer address of every drop so far, in order.
    pub fn peer_addrs(&self) -> Vec<SocketAddr> {
        self.seen.borrow().iter().map(|(addr, _)| *addr).collect()
    }
}

pub fn summarize(reason: ConnectionDropReason<'_>) -> String {
    match reason {
        ConnectionDropReason::AcceptorSuspended => "acceptor_suspended".to_owned(),
        ConnectionDropReason::ClosedBeforeFirstMessage => "closed".to_owned(),
        ConnectionDropReason::LogonTimeout => "timeout".to_owned(),
        ConnectionDropReason::FirstMessageIoError(_) => "io_error".to_owned(),
        ConnectionDropReason::FirstMessageUndecodable(_) => "undecodable".to_owned(),
        ConnectionDropReason::FirstMessageTooLarge => "too_large".to_owned(),
        ConnectionDropReason::FirstMessageNotLogon => "not_logon".to_owned(),
        ConnectionDropReason::UnknownSession(id) => format!("unknown:{id}"),
        ConnectionDropReason::SessionAlreadyActive(id) => format!("active:{id}"),
        ConnectionDropReason::SessionSuspended(id) => format!("suspended:{id}"),
        // The enum is `#[non_exhaustive]`; a variant this file does not know
        // yet still gets a distinct summary. Inside the crate's own tests the
        // match is exhaustive and this arm is dead, hence `allow` rather than
        // `expect` - the lint is fulfilled on one side only.
        #[allow(
            unreachable_patterns,
            reason = "reachable only outside the defining crate"
        )]
        other => format!("{other:?}"),
    }
}

impl ConnectionObserver for RecordingObserver {
    fn on_connection_dropped(&self, peer_addr: SocketAddr, reason: ConnectionDropReason<'_>) {
        self.seen.borrow_mut().push((peer_addr, summarize(reason)));
    }
}

/// Read one complete FIX message from `reader`, using `buf` as a persistent
/// scratch buffer across calls. Any bytes past the first parsed message are
/// retained in `buf` so the next call can consume them without losing data.
pub async fn read_one_message<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Box<Message> {
    try_read_one_message(reader, buf)
        .await
        .expect("unexpected EOF while reading FIX message")
}

/// Like [`read_one_message`] but returns `None` on a clean EOF instead of
/// panicking - lets a test distinguish "peer sent a message" from "peer
/// closed the connection".
pub async fn try_read_one_message<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Option<Box<Message>> {
    loop {
        // Try parsing whatever is already buffered before reading more.
        if !buf.is_empty() {
            match raw_message(buf) {
                Ok((leftover, raw)) => {
                    let leftover_len = leftover.len();
                    let msg = Message::from_raw_message(raw).expect("parse failed");
                    let consumed = buf.len() - leftover_len;
                    buf.drain(..consumed);
                    return Some(msg);
                }
                Err(RawMessageError::Incomplete) => {}
                Err(e) => panic!("raw_message error: {e}"),
            }
        }
        let mut tmp = [0u8; 1024];
        let n = reader.read(&mut tmp).await.expect("read failed");
        if n == 0 {
            return None; // clean EOF
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StorageSnapshot {
    pub sender: NonZeroSeqNum,
    pub target: NonZeroSeqNum,
    pub messages: BTreeMap<NonZeroSeqNum, Vec<u8>>,
}

/// An in-memory backend whose actual stores and counters are observable
/// without borrowing storage out of a running session.
pub(crate) struct ObservedStorage {
    inner: InMemoryStorage,
    snapshot: Rc<RefCell<StorageSnapshot>>,
}

impl ObservedStorage {
    pub(crate) fn new(max: usize) -> (Self, Rc<RefCell<StorageSnapshot>>) {
        let inner = InMemoryStorage::new(max);
        let snapshot = Rc::new(RefCell::new(StorageSnapshot {
            sender: inner.next_sender_msg_seq_num(),
            target: inner.next_target_msg_seq_num(),
            messages: BTreeMap::new(),
        }));
        (
            Self {
                inner,
                snapshot: snapshot.clone(),
            },
            snapshot,
        )
    }
}

impl MessagesStorage for ObservedStorage {
    type Error = InMemoryStorageError;

    fn store(
        &mut self,
        seq: NonZeroSeqNum,
        serialize: impl FnOnce(&mut [u8]) -> Result<usize, SerializeError>,
    ) -> Result<(), StoreError<Self::Error>> {
        self.inner.store(seq, |buf| {
            let len = serialize(buf)?;
            self.snapshot
                .borrow_mut()
                .messages
                .insert(seq, buf[..len].to_vec());
            Ok(len)
        })
    }

    async fn fetch(
        &mut self,
        seq: NonZeroSeqNum,
        end: NonZeroSeqNum,
    ) -> Result<&[u8], Self::Error> {
        self.inner.fetch(seq, end).await
    }

    fn next_sender_msg_seq_num(&self) -> NonZeroSeqNum {
        self.inner.next_sender_msg_seq_num()
    }

    fn next_target_msg_seq_num(&self) -> NonZeroSeqNum {
        self.inner.next_target_msg_seq_num()
    }

    fn set_next_sender_msg_seq_num(&mut self, seq: NonZeroSeqNum) -> Result<(), Self::Error> {
        self.inner.set_next_sender_msg_seq_num(seq)?;
        self.snapshot.borrow_mut().sender = seq;
        Ok(())
    }

    fn set_next_target_msg_seq_num(&mut self, seq: NonZeroSeqNum) -> Result<(), Self::Error> {
        self.inner.set_next_target_msg_seq_num(seq)?;
        self.snapshot.borrow_mut().target = seq;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), Self::Error> {
        self.inner.reset()?;
        let mut snapshot = self.snapshot.borrow_mut();
        snapshot.sender = NonZeroSeqNum::new(1).unwrap();
        snapshot.target = NonZeroSeqNum::new(1).unwrap();
        snapshot.messages.clear();
        Ok(())
    }
}
