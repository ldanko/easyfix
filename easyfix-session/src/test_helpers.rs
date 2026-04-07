use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io,
    num::{NonZeroU8, NonZeroU16, NonZeroU64},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, HeaderBase, HeartbeatBase, LogonBase, LogoutBase, RejectBase,
        ResendRequestBase, SequenceResetBase, SessionRejectReasonBase, TestRequestBase,
    },
    basic_types::{
        ApplVerId, DateTime, FixStr, FixString, Int, Length, MsgTypeField, NonZeroLength,
        NonZeroSeqNum, SeqNum, TagNum, TimePrecision, Utc, UtcTimestamp,
    },
    deserializer::{DeserializeErrorKind, RawMessage},
    fix_str,
    message::{DeserializeError, HeaderAccess, MsgCat, SessionMessage},
    serializer::SerializeError,
    version::Version,
};
use easyfix_test_messages::{Body, Header, Message, NewOrderSingle, Trailer};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::Notify,
    time::Duration,
};

use crate::{
    application::{Application, ApplicationFactory, DisconnectReason, InputAction, SessionContext},
    engine::{InputResult, SessionEngine},
    io::{sender::Sender, time::TimerBackend},
    messages_storage::{InMemoryStorage, MessagesStorage, StoreError},
    session_id::SessionId,
    settings::SessionSettings,
};

// The fixtures that need nothing beyond the public API live in one file
// with the integration tests; see its module doc for the ground rules.
#[path = "../tests/common/fixtures.rs"]
mod fixtures;
pub(crate) use fixtures::{
    DEFAULT_MAX_MESSAGE_SIZE, DEFAULT_MAX_MESSAGE_SIZE_BYTES, ObservedStorage, RecordingObserver,
    TEST_PEER_ADDR, build_session_settings, new_order_single_with_empty_header, read_one_message,
    serialize_message,
};

// ---------------------------------------------------------------------------
// EngineBuilder
// ---------------------------------------------------------------------------

/// A fresh [`InMemoryStorage`] sized for the tests' default max message size.
pub(crate) fn default_storage() -> InMemoryStorage {
    InMemoryStorage::new(DEFAULT_MAX_MESSAGE_SIZE_BYTES)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageOp {
    Store,
    Fetch,
    GetSender,
    GetTarget,
    SetSender,
    SetTarget,
    Reset,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FailureTiming {
    Before,
    After,
}

#[derive(Debug, thiserror::Error)]
#[error("injected storage failure: {0:?}")]
pub(crate) struct InjectedStorageError(pub StorageOp);

#[derive(Debug, Default)]
pub(crate) struct StorageTrace {
    pub calls: Vec<StorageOp>,
    pub serialized: usize,
    pub failed: bool,
    failure: Option<(StorageOp, usize, FailureTiming)>,
}

/// A backend with owned chunks and a trace that also catches getters after an error.
#[derive(Debug)]
pub(crate) struct FailingStorage {
    pub records: BTreeMap<NonZeroSeqNum, Vec<u8>>,
    pub sender: NonZeroSeqNum,
    pub target: NonZeroSeqNum,
    pub trace: Rc<RefCell<StorageTrace>>,
    pub max_message_size: usize,
}

impl FailingStorage {
    pub fn new() -> Self {
        Self {
            records: BTreeMap::new(),
            sender: nz_seq(1),
            target: nz_seq(1),
            trace: Rc::default(),
            max_message_size: DEFAULT_MAX_MESSAGE_SIZE_BYTES,
        }
    }

    /// Fail on the nth subsequent occurrence of an operation.
    pub fn fail_on(&mut self, op: StorageOp, nth: usize, timing: FailureTiming) {
        let mut trace = self.trace.borrow_mut();
        assert!(!trace.failed);
        trace.failure = Some((op, nth, timing));
    }

    #[expect(
        clippy::panic_in_result_fn,
        reason = "asserts that the session never uses failed storage"
    )]
    fn enter(&self, op: StorageOp) -> Result<Option<FailureTiming>, InjectedStorageError> {
        let mut trace = self.trace.borrow_mut();
        assert!(!trace.failed, "storage call after fatal error: {op:?}");
        trace.calls.push(op);
        let Some((expected, remaining, timing)) = trace.failure.as_mut() else {
            return Ok(None);
        };
        if *expected != op {
            return Ok(None);
        }
        *remaining -= 1;
        if *remaining > 0 {
            return Ok(None);
        }
        let timing = *timing;
        trace.failure = None;
        if matches!(timing, FailureTiming::Before) {
            trace.failed = true;
            return Err(InjectedStorageError(op));
        }
        Ok(Some(timing))
    }

    fn leave(
        &self,
        op: StorageOp,
        timing: Option<FailureTiming>,
    ) -> Result<(), InjectedStorageError> {
        if timing.is_some() {
            self.trace.borrow_mut().failed = true;
            Err(InjectedStorageError(op))
        } else {
            Ok(())
        }
    }
}

impl MessagesStorage for FailingStorage {
    type Error = InjectedStorageError;

    fn store(
        &mut self,
        seq_num: NonZeroSeqNum,
        serialize: impl FnOnce(&mut [u8]) -> Result<usize, SerializeError>,
    ) -> Result<(), StoreError<Self::Error>> {
        let timing = self.enter(StorageOp::Store).map_err(StoreError::Backend)?;
        if self.records.contains_key(&seq_num) {
            self.trace.borrow_mut().failed = true;
            return Err(StoreError::Backend(InjectedStorageError(StorageOp::Store)));
        }
        let mut chunk = vec![0; self.max_message_size];
        self.trace.borrow_mut().serialized += 1;
        let len = serialize(&mut chunk).map_err(StoreError::Serialize)?;
        chunk.truncate(len);
        self.records.insert(seq_num, chunk);
        self.leave(StorageOp::Store, timing)
            .map_err(StoreError::Backend)
    }

    async fn fetch(
        &mut self,
        seq_num: NonZeroSeqNum,
        _: NonZeroSeqNum,
    ) -> Result<&[u8], Self::Error> {
        let timing = self.enter(StorageOp::Fetch)?;
        self.leave(StorageOp::Fetch, timing)?;
        if let Some(bytes) = self.records.get(&seq_num) {
            Ok(bytes)
        } else {
            self.trace.borrow_mut().failed = true;
            Err(InjectedStorageError(StorageOp::Fetch))
        }
    }

    fn next_sender_msg_seq_num(&self) -> NonZeroSeqNum {
        self.enter(StorageOp::GetSender)
            .expect("getter is infallible");
        self.sender
    }

    fn next_target_msg_seq_num(&self) -> NonZeroSeqNum {
        self.enter(StorageOp::GetTarget)
            .expect("getter is infallible");
        self.target
    }

    fn set_next_sender_msg_seq_num(&mut self, seq_num: NonZeroSeqNum) -> Result<(), Self::Error> {
        let timing = self.enter(StorageOp::SetSender)?;
        self.sender = seq_num;
        self.leave(StorageOp::SetSender, timing)
    }

    fn set_next_target_msg_seq_num(&mut self, seq_num: NonZeroSeqNum) -> Result<(), Self::Error> {
        let timing = self.enter(StorageOp::SetTarget)?;
        self.target = seq_num;
        self.leave(StorageOp::SetTarget, timing)
    }

    fn reset(&mut self) -> Result<(), Self::Error> {
        let timing = self.enter(StorageOp::Reset)?;
        self.sender = nz_seq(1);
        self.target = nz_seq(1);
        self.records.clear();
        self.leave(StorageOp::Reset, timing)
    }
}

/// A sequence number as the storage counters take them.
///
/// `MessagesStorage` deals exclusively in [`NonZeroSeqNum`], so tests that
/// place a counter go through this instead of spelling out
/// `NonZeroSeqNum::new(n).unwrap()` at every call site.
///
/// # Panics
///
/// On `0` - not a sequence number (FIX Session Layer §4.1).
pub(crate) fn nz_seq(seq_num: SeqNum) -> NonZeroSeqNum {
    NonZeroSeqNum::new(seq_num).expect("sequence numbers start at 1")
}

pub(crate) fn default_session_id() -> SessionId {
    SessionId::new(
        Version::FIXT11,
        fix_str!("SENDER").to_owned(),
        fix_str!("TARGET").to_owned(),
    )
}

/// The shared baseline settings with a 30 s heartbeat.
pub(crate) fn default_session_settings() -> SessionSettings {
    build_session_settings(30)
}

/// Builder for constructing `SessionEngine<Message>` with sensible
/// test defaults.
pub(crate) struct EngineBuilder {
    session_id: SessionId,
    session_settings: SessionSettings,
    timer_backend: TimerBackend,
    logged_on: bool,
    heartbeat_interval_in_force: Option<Option<NonZeroU64>>,
}

impl EngineBuilder {
    pub fn new() -> Self {
        EngineBuilder {
            session_id: default_session_id(),
            session_settings: default_session_settings(),
            timer_backend: TimerBackend::Tokio,
            logged_on: false,
            heartbeat_interval_in_force: None,
        }
    }

    pub fn max_message_size(mut self, size: NonZeroLength) -> Self {
        self.session_settings.max_message_size = size;
        self
    }

    pub fn timer_backend(mut self, timer_backend: TimerBackend) -> Self {
        self.timer_backend = timer_backend;
        self
    }

    pub fn sender_default_appl_ver_id(mut self, appl_ver_id: ApplVerId) -> Self {
        self.session_settings.sender_default_appl_ver_id = appl_ver_id;
        self
    }

    pub fn heartbeat_interval(mut self, interval: Option<NonZeroU16>) -> Self {
        self.session_settings.heartbeat_interval = interval;
        self
    }

    pub fn heartbeat_interval_in_force(mut self, interval: Option<NonZeroU64>) -> Self {
        self.heartbeat_interval_in_force = Some(interval);
        self
    }

    pub fn max_latency(mut self, max_latency: Duration) -> Self {
        self.session_settings.max_latency = Some(max_latency);
        self
    }

    pub fn accept_reset_on_connect(mut self, accept: bool) -> Self {
        self.session_settings.accept_reset_on_connect = accept;
        self
    }

    pub fn accept_reset_in_session(mut self, accept: bool) -> Self {
        self.session_settings.accept_reset_in_session = accept;
        self
    }

    pub fn auto_disconnect_after_no_heartbeat(mut self, probes: u8) -> Self {
        self.session_settings.auto_disconnect_after_no_heartbeat =
            NonZeroU8::new(probes).expect("probes must be non-zero");
        self
    }

    pub fn auto_disconnect_after_no_logon_response(mut self, budget: Duration) -> Self {
        self.session_settings
            .auto_disconnect_after_no_logon_response = budget;
        self
    }

    pub fn auto_disconnect_after_no_logout(mut self, budget: Duration) -> Self {
        self.session_settings.auto_disconnect_after_no_logout = budget;
        self
    }

    pub fn verify_test_request_id(mut self, verify: bool) -> Self {
        self.session_settings.verify_test_request_id = verify;
        self
    }

    pub fn verify_logout(mut self, verify: bool) -> Self {
        self.session_settings.verify_logout = verify;
        self
    }

    pub fn send_redundant_resend_requests(mut self, send: bool) -> Self {
        self.session_settings.send_redundant_resend_requests = send;
        self
    }

    pub fn enable_next_expected_msg_seq_num(mut self) -> Self {
        self.session_settings.enable_next_expected_msg_seq_num = true;
        self
    }

    pub fn logged_on(mut self) -> Self {
        self.logged_on = true;
        self
    }

    pub fn persist_messages(mut self, persist: bool) -> Self {
        self.session_settings.persist_messages = persist;
        self
    }

    pub fn build(self) -> (SessionEngine<Message>, InMemoryStorage) {
        let storage =
            InMemoryStorage::new(usize::from(self.session_settings.max_message_size.get()));
        (self.build_engine(), storage)
    }

    fn build_engine(self) -> SessionEngine<Message> {
        let mut engine =
            SessionEngine::new(self.session_id, self.session_settings, self.timer_backend);
        if self.logged_on {
            engine.set_logged_on();
        }
        if let Some(interval) = self.heartbeat_interval_in_force {
            engine.set_heartbeat_interval_in_force(interval);
        }
        engine
    }
}

// ---------------------------------------------------------------------------
// Message factory helpers
// ---------------------------------------------------------------------------

/// Build a HeaderBase for an incoming message from the counterparty.
pub(crate) fn header(
    seq: SeqNum,
    sender: &'static FixStr,
    target: &'static FixStr,
) -> HeaderBase<'static> {
    HeaderBase {
        sender_comp_id: Cow::Borrowed(sender),
        target_comp_id: Cow::Borrowed(target),
        msg_seq_num: seq,
        sending_time: UtcTimestamp::now(TimePrecision::Nanos),
        poss_dup_flag: None,
        orig_sending_time: None,
        appl_ver_id: None,
    }
}

/// Build a HeaderBase for an incoming message from the counterparty with
/// the default test CompIDs (peer perspective: sender=TARGET,
/// target=SENDER).
pub(crate) fn inbound_header(seq: SeqNum) -> HeaderBase<'static> {
    header(seq, fix_str!("TARGET"), fix_str!("SENDER"))
}

/// Build the `DeserializeError` the generated deserializer produces for a
/// message whose header parsed but whose body (or trailer) failed: a
/// body-level Reject with the parsed header attached.
pub(crate) fn body_reject_error(
    msg_type: &FixStr,
    header: HeaderBase<'static>,
    tag: Option<TagNum>,
    reason: SessionRejectReasonBase,
) -> DeserializeError {
    DeserializeError {
        kind: DeserializeErrorKind::Reject {
            msg_type: Some(msg_type.to_owned()),
            seq_num: header.msg_seq_num,
            tag,
            reason: reason.into(),
        },
        header: Some(Box::new(header)),
    }
}

/// Build a Logon message with given seq num and comp IDs.
pub(crate) fn logon(seq: SeqNum, sender: &'static FixStr, target: &'static FixStr) -> Box<Message> {
    logon_with_options(seq, sender, target, 30, None, None)
}

/// Build a Logon message with configurable options.
pub(crate) fn logon_with_options(
    seq: SeqNum,
    sender: &'static FixStr,
    target: &'static FixStr,
    heart_bt_int: Int,
    reset_seq_num_flag: Option<bool>,
    next_expected_msg_seq_num: Option<SeqNum>,
) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, sender, target),
        AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: 0,
            heart_bt_int,
            reset_seq_num_flag,
            max_message_size: None,
            next_expected_msg_seq_num,
            default_appl_ver_id: Some(ApplVerId::Fix50Sp2),
            session_status: None,
        }),
    ))
}

/// Build a Logon message advertising `MaxMessageSize<383>`.
pub(crate) fn logon_with_max_message_size(
    seq: SeqNum,
    sender: &'static FixStr,
    target: &'static FixStr,
    max_message_size: Length,
) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, sender, target),
        AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: 0,
            heart_bt_int: 30,
            reset_seq_num_flag: None,
            max_message_size: Some(max_message_size),
            next_expected_msg_seq_num: None,
            default_appl_ver_id: Some(ApplVerId::Fix50Sp2),
            session_status: None,
        }),
    ))
}

/// Build a Heartbeat message.
pub(crate) fn heartbeat(seq: SeqNum, test_req_id: Option<FixString>) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, fix_str!("TARGET"), fix_str!("SENDER")),
        AdminBase::Heartbeat(HeartbeatBase {
            test_req_id: test_req_id.map(Cow::Owned),
        }),
    ))
}

/// Build a TestRequest message.
pub(crate) fn test_request(seq: SeqNum, test_req_id: &'static FixStr) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, fix_str!("TARGET"), fix_str!("SENDER")),
        AdminBase::TestRequest(TestRequestBase {
            test_req_id: Cow::Borrowed(test_req_id),
        }),
    ))
}

/// Build a ResendRequest message.
pub(crate) fn resend_request(seq: SeqNum, begin: SeqNum, end: SeqNum) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, fix_str!("TARGET"), fix_str!("SENDER")),
        AdminBase::ResendRequest(ResendRequestBase {
            begin_seq_no: begin,
            end_seq_no: end,
        }),
    ))
}

/// Build a Reject message.
pub(crate) fn reject(seq: SeqNum, ref_seq: SeqNum) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, fix_str!("TARGET"), fix_str!("SENDER")),
        AdminBase::Reject(RejectBase {
            ref_seq_num: ref_seq,
            ref_tag_id: None,
            ref_msg_type: None,
            session_reject_reason: None,
            text: None,
        }),
    ))
}

/// Build a SequenceReset message.
pub(crate) fn sequence_reset(seq: SeqNum, new_seq: SeqNum, gap_fill: bool) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, fix_str!("TARGET"), fix_str!("SENDER")),
        AdminBase::SequenceReset(SequenceResetBase {
            gap_fill_flag: Some(gap_fill),
            new_seq_no: new_seq,
        }),
    ))
}

/// Build a Logout message.
pub(crate) fn logout(seq: SeqNum) -> Box<Message> {
    Box::new(Message::from_admin(
        header(seq, fix_str!("TARGET"), fix_str!("SENDER")),
        AdminBase::Logout(LogoutBase {
            session_status: None,
            text: None,
        }),
    ))
}

/// Build a NewOrderSingle message (incoming from counterparty).
pub(crate) fn new_order_single(seq: SeqNum) -> Box<Message> {
    Box::new(Message {
        header: Header::from(header(seq, fix_str!("TARGET"), fix_str!("SENDER"))),
        body: Box::new(Body::NewOrderSingle(NewOrderSingle {
            cl_ord_id: fix_str!("ORD001").to_owned(),
            symbol: fix_str!("SYMBOL1").to_owned(),
            transact_time: UtcTimestamp::now(TimePrecision::Nanos),
            ..NewOrderSingle::default()
        })),
        trailer: Trailer::default(),
    })
}

/// Build an outgoing Heartbeat with a default (empty) header - simulates an
/// outgoing message before the engine fills the header. Mirrors
/// [`new_order_single_with_empty_header`].
pub(crate) fn heartbeat_with_empty_header() -> Box<Message> {
    Box::new(Message::from_admin(
        HeaderBase::default(),
        AdminBase::Heartbeat(HeartbeatBase { test_req_id: None }),
    ))
}

// ---------------------------------------------------------------------------
// Raw-bytes + misc fixtures
// ---------------------------------------------------------------------------

/// Serialize a peer-perspective Heartbeat at `seq` into raw FIX bytes.
pub(crate) fn heartbeat_bytes(seq: SeqNum) -> Vec<u8> {
    serialize_message(&heartbeat(seq, None))
}

/// Frame `body` into a complete FIX message with computed BodyLength(9)
/// and CheckSum(10). `|` in `body` is replaced with SOH first. Fixture
/// for tests that need well-framed wire bytes no serializer would emit
/// (invalid field values, broken header structure, ...).
pub(crate) fn frame_message(begin_string: &str, body: &str) -> Vec<u8> {
    let body = body.replace('|', "\x01").into_bytes();
    let mut out = Vec::new();
    out.extend_from_slice(b"8=");
    out.extend_from_slice(begin_string.as_bytes());
    out.push(b'\x01');
    out.extend_from_slice(format!("9={}\x01", body.len()).as_bytes());
    out.extend_from_slice(&body);
    let sum = out.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    out.extend_from_slice(format!("10={sum:03}\x01").as_bytes());
    out
}

/// Serialize a peer-perspective Logon (sender=TARGET, target=SENDER) at `seq`
/// with the given heartbeat interval into raw FIX bytes.
pub(crate) fn logon_bytes(seq: SeqNum, heart_bt_int: Int) -> Vec<u8> {
    serialize_message(&logon_with_options(
        seq,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        heart_bt_int,
        None,
        None,
    ))
}

/// Frame a peer Logon with a raw EncryptMethod value, including invalid values.
pub(crate) fn logon_bytes_with_encrypt_method(
    seq: SeqNum,
    encrypt_method: &str,
    reset: bool,
) -> Vec<u8> {
    let sending_time = Utc::now().format("%Y%m%d-%H:%M:%S%.3f");
    let reset = if reset { "141=Y|" } else { "" };
    frame_message(
        "FIXT.1.1",
        &format!(
            "35=A|49=TARGET|56=SENDER|34={seq}|52={sending_time}|98={encrypt_method}|108=30|{reset}1137=9|"
        ),
    )
}

/// Serialize a peer-perspective Logout at `seq` into raw FIX bytes.
pub(crate) fn logout_bytes(seq: SeqNum) -> Vec<u8> {
    serialize_message(&logout(seq))
}

/// Serialize a peer-perspective `SequenceReset<4>` into raw FIX bytes.
pub(crate) fn sequence_reset_bytes(seq: SeqNum, new_seq: SeqNum, gap_fill: bool) -> Vec<u8> {
    serialize_message(&sequence_reset(seq, new_seq, gap_fill))
}

/// Well-framed peer-perspective Logon (sender=TARGET, target=SENDER, so
/// it resolves to [`default_session_id`]) that fails decoding: 98=QQ
/// (EncryptMethod not an integer).
pub(crate) fn invalid_logon_bytes() -> Vec<u8> {
    // SendingTime(52) must be current: on the deserialize-error path the
    // recovered header runs the full clean-path validation, and a stale
    // timestamp would trip the SendingTime-accuracy verdict (Scenario
    // 2(o)) before the invalid-body escalation this fixture exists for.
    let sending_time = Utc::now().format("%Y%m%d-%H:%M:%S%.3f");
    frame_message(
        "FIXT.1.1",
        &format!("35=A|49=TARGET|56=SENDER|34=1|52={sending_time}|98=QQ|108=30|"),
    )
}

/// Reader whose every poll fails with `ConnectionReset` - drives IO-error
/// paths.
pub(crate) struct FailingReader;

impl AsyncRead for FailingReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "connection reset",
        )))
    }
}

/// Create a `UtcTimestamp` offset from now by the given number of seconds.
/// Negative values go into the past.
pub(crate) fn timestamp_offset_secs(secs: i64) -> UtcTimestamp {
    let dt: DateTime<Utc> = Utc::now() + chrono::Duration::seconds(secs);
    UtcTimestamp::with_precision(dt, TimePrecision::Nanos)
}

// ---------------------------------------------------------------------------
// Engine / IO-loop setup helpers
//
// Preparation only - no assertions. The `expect`/`panic` calls are structural
// extraction guards (drain-or-panic); every test asserts its actual verdict at
// the call site.
// ---------------------------------------------------------------------------

/// Drain exactly one admin message from the engine's admin output queue.
pub(crate) fn take_admin(engine: &mut SessionEngine<Message>) -> Box<Message> {
    engine
        .take_admin_output()
        .expect("expected admin message in output")
}

/// Drain and discard the engine's entire admin output queue.
pub(crate) fn drain_all_admin(engine: &mut SessionEngine<Message>) {
    while engine.take_admin_output().is_some() {}
}

/// Unwrap the `AdminBase` view of a message. The `expect` is a structural
/// guard (the message under test is known-admin); the *which-admin* verdict
/// stays at the call site.
pub(crate) fn as_admin(msg: &Message) -> AdminBase<'_> {
    msg.try_as_admin().expect("expected admin message")
}

/// Drive an inbound message through the standard "validate -> callback ->
/// accept" flow used by the IO loop. Validation outcomes (`Handled`, `Error`)
/// pass through unchanged; the caller inspects the returned `InputResult`,
/// and a session-ending verdict through `engine.disconnect_reason()`.
pub(crate) fn accept_input(
    engine: &mut SessionEngine<Message>,
    msg: Box<Message>,
    storage: &mut impl MessagesStorage,
) -> InputResult<Message> {
    match engine.on_input(msg, storage).expect("input succeeds") {
        InputResult::AdminMsg(m) => engine
            .process_admin_input(m, InputAction::Accept, storage)
            .expect("admin input succeeds"),
        InputResult::AppMsg(m) => {
            let ref_seq_num = m.msg_seq_num();
            let ref_msg_type = SessionMessage::msg_type(&*m);
            engine
                .process_app_input(ref_seq_num, ref_msg_type, InputAction::Accept, storage)
                .expect("app input succeeds")
        }
        other => other,
    }
}

/// Run `on_input` expecting an `AppMsg` and extract the
/// `(ref_seq_num, ref_msg_type)` pair a later `process_app_input` needs. The
/// `InputAction` under test stays at the call site.
pub(crate) fn dispatch_app_for_action(
    engine: &mut SessionEngine<Message>,
    msg: Box<Message>,
    storage: &mut impl MessagesStorage,
) -> (SeqNum, MsgTypeField) {
    let InputResult::AppMsg(m) = engine.on_input(msg, storage).expect("input succeeds") else {
        panic!("expected AppMsg from on_input");
    };
    (m.msg_seq_num(), SessionMessage::msg_type(&*m))
}

/// Send a heartbeat, fill its header, commit it to `storage` at the current
/// sender seq num, and return the serialized bytes so the test can compare
/// them against what later appears on the wire. Generic over storage. Side effect:
/// advances the sender seq num and writes to `storage`.
pub(crate) fn commit_heartbeat<S: MessagesStorage>(
    engine: &mut SessionEngine<Message>,
    storage: &mut S,
) -> Vec<u8> {
    engine.send_heartbeat(None);
    let mut msg = engine.take_admin_output().unwrap();
    assert!(
        engine
            .fill_header(&mut msg, storage)
            .expect("header succeeds")
    );
    let expected_bytes = serialize_message(&msg);
    engine
        .commit_send(msg, storage)
        .expect("commit should succeed");
    expected_bytes
}

/// Minimal no-op [`Application`] for tests that only need to satisfy the trait
/// parameter without exercising any callback. Accepts everything; asserts
/// nothing.
pub(crate) struct StubApp;

impl<M: SessionMessage> Application<M> for StubApp {
    fn on_serialize_error(&mut self, _msg: Box<M>, _error: &SerializeError) {}

    async fn on_session_ready(&mut self, _: &SessionId, _: Sender<M>) {}

    async fn on_session_end(&mut self, _: &SessionId, _: DisconnectReason) {}

    async fn on_app_msg_in(&mut self, _: Box<M>) -> InputAction {
        InputAction::Accept
    }
}

/// [`ApplicationFactory`] producing [`StubApp`] - for tests that construct an
/// `Acceptor` / `Initiator` but never exercise the application callbacks.
pub(crate) struct StubAppFactory;

impl<M: SessionMessage> ApplicationFactory<M> for StubAppFactory {
    type App = StubApp;

    fn create(&self, _: &SessionContext<'_>) -> StubApp {
        StubApp
    }
}

/// Signals entry to `on_session_end` and holds it until explicitly released.
#[derive(Clone, Default)]
pub(crate) struct GatedSessionEnd {
    pub entered: Rc<Notify>,
    pub release: Rc<Notify>,
}

impl Application<Message> for GatedSessionEnd {
    fn on_serialize_error(&mut self, _: Box<Message>, _: &SerializeError) {}

    async fn on_session_ready(&mut self, _: &SessionId, _: Sender<Message>) {}

    async fn on_session_end(&mut self, _: &SessionId, _: DisconnectReason) {
        self.entered.notify_one();
        self.release.notified().await;
    }

    async fn on_app_msg_in(&mut self, _: Box<Message>) -> InputAction {
        InputAction::Accept
    }
}

impl ApplicationFactory<Message> for GatedSessionEnd {
    type App = Self;

    fn create(&self, _: &SessionContext<'_>) -> Self::App {
        self.clone()
    }
}

/// [`Application`] that panics in `on_admin_msg_in` - arbitrary user code
/// unwinding the session task while it owns the session's storage.
pub(crate) struct PanicApp;

impl Application<Message> for PanicApp {
    fn on_serialize_error(&mut self, _msg: Box<Message>, _error: &SerializeError) {}

    async fn on_session_ready(&mut self, _: &SessionId, _: Sender<Message>) {}

    async fn on_session_end(&mut self, _: &SessionId, _: DisconnectReason) {}

    async fn on_app_msg_in(&mut self, _: Box<Message>) -> InputAction {
        InputAction::Accept
    }

    async fn on_admin_msg_in(&mut self, _: &Message) -> InputAction {
        panic!("user callback panicked");
    }
}

/// [`ApplicationFactory`] producing [`PanicApp`].
pub(crate) struct PanicAppFactory;

impl ApplicationFactory<Message> for PanicAppFactory {
    type App = PanicApp;

    fn create(&self, _: &SessionContext<'_>) -> PanicApp {
        PanicApp
    }
}

thread_local! {
    static RESET_SUPPORT_PROBES: Cell<usize> = const { Cell::new(0) };
}

pub(crate) fn reset_support_probe_count() -> usize {
    RESET_SUPPORT_PROBES.get()
}

/// Drops the reset flag during Logon conversion and counts explicit reset
/// requests. All other message operations delegate to the generated message.
#[derive(Debug)]
pub(crate) struct CountedResetMessage(pub Message);

impl SessionMessage for CountedResetMessage {
    fn from_raw_message(raw: RawMessage<'_>) -> Result<Box<Self>, DeserializeError> {
        Message::from_raw_message(raw).map(|msg| Box::new(Self(*msg)))
    }

    fn serialize(&self, buf: &mut [u8]) -> Result<usize, SerializeError> {
        self.0.serialize(buf)
    }

    fn header(&self) -> HeaderBase<'_> {
        self.0.header()
    }

    fn try_as_admin(&self) -> Option<AdminBase<'_>> {
        self.0.try_as_admin()
    }

    fn msg_type(&self) -> MsgTypeField {
        SessionMessage::msg_type(&self.0)
    }

    fn msg_cat(&self) -> MsgCat {
        self.0.msg_cat()
    }

    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn from_admin(header: HeaderBase<'static>, mut admin: AdminBase<'static>) -> Self {
        if let AdminBase::Logon(logon) = &mut admin {
            if logon.reset_seq_num_flag == Some(true) {
                RESET_SUPPORT_PROBES.set(RESET_SUPPORT_PROBES.get() + 1);
            }
            logon.reset_seq_num_flag = None;
        }
        Self(Message::from_admin(header, admin))
    }
}

impl HeaderAccess for CountedResetMessage {
    fn version(&self) -> Version {
        self.0.version()
    }

    fn sender_comp_id(&self) -> &FixStr {
        self.0.sender_comp_id()
    }

    fn target_comp_id(&self) -> &FixStr {
        self.0.target_comp_id()
    }

    fn msg_seq_num(&self) -> SeqNum {
        self.0.msg_seq_num()
    }

    fn sending_time(&self) -> UtcTimestamp {
        self.0.sending_time()
    }

    fn poss_dup_flag(&self) -> Option<bool> {
        self.0.poss_dup_flag()
    }

    fn orig_sending_time(&self) -> Option<UtcTimestamp> {
        self.0.orig_sending_time()
    }

    fn appl_ver_id(&self) -> Option<ApplVerId> {
        self.0.appl_ver_id()
    }

    fn set_sender_comp_id(&mut self, value: FixString) {
        self.0.set_sender_comp_id(value);
    }

    fn set_target_comp_id(&mut self, value: FixString) {
        self.0.set_target_comp_id(value);
    }

    fn set_msg_seq_num(&mut self, value: SeqNum) {
        self.0.set_msg_seq_num(value);
    }

    fn set_sending_time(&mut self, value: UtcTimestamp) {
        self.0.set_sending_time(value);
    }

    fn set_poss_dup_flag(&mut self, value: Option<bool>) {
        self.0.set_poss_dup_flag(value);
    }

    fn set_orig_sending_time(&mut self, value: Option<UtcTimestamp>) {
        self.0.set_orig_sending_time(value);
    }

    fn set_appl_ver_id(&mut self, value: Option<ApplVerId>) {
        self.0.set_appl_ver_id(value);
    }
}
