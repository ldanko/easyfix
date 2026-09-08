//! Connection task fault tolerance.
//!
//! A connection task must survive hostile input (e.g. a burst of rejected
//! messages saturating the events channel) and, when it dies anyway, it must
//! release the session so the peer can reconnect and the acceptor API remains
//! safe to call.

use std::{
    cell::Cell,
    future::pending,
    io,
    iter::empty,
    net::SocketAddr,
    ops::RangeInclusive,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};

use chrono::NaiveTime;
use easyfix_macros::fix_str;
use easyfix_messages::{
    fields::{DefaultApplVerId, EncryptMethod, FixStr, SeqNum, SessionStatus, Utc, UtcTimestamp},
    messages::{FixtMessage, Header, Logon, Message, SequenceReset, TestRequest, Trailer},
};
use easyfix_session::{
    DisconnectReason, Sender,
    acceptor::{Acceptor, Connection},
    application::{AsEvent, FixEvent},
    messages_storage::{InMemoryStorage, MessagesStorage},
    session_id::SessionId,
    settings::{SessionSettings, Settings},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, WriteHalf},
    runtime::Builder,
    sync::{Notify, mpsc},
    task::LocalSet,
    time::{sleep, timeout},
};
use tokio_stream::StreamExt;

const BEGIN_STRING: &FixStr = fix_str!("FIXT.1.1");
const SERVER_COMP_ID: &FixStr = fix_str!("server");
const CLIENT_COMP_ID: &FixStr = fix_str!("client");

fn session_id() -> SessionId {
    SessionId::new(
        BEGIN_STRING.to_owned(),
        SERVER_COMP_ID.to_owned(),
        CLIENT_COMP_ID.to_owned(),
    )
}

fn settings() -> Settings {
    Settings {
        sender_comp_id: SERVER_COMP_ID.to_owned(),
        sender_sub_id: None,
        heartbeat_interval: Some(10),
        auto_disconnect_after_no_logon_received: Duration::from_secs(3),
        auto_disconnect_after_no_heartbeat: 3,
        auto_disconnect_after_no_logout: Duration::from_secs(5),
    }
}

fn session_settings() -> SessionSettings {
    SessionSettings {
        session_id: session_id(),
        session_time: NaiveTime::from_hms_opt(0, 0, 0).unwrap()
            ..=NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
        logon_time: NaiveTime::from_hms_opt(0, 0, 0).unwrap()
            ..=NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
        send_redundant_resend_requests: false,
        check_comp_id: true,
        max_latency: Some(Duration::from_secs(60)),
        reset_on_logon: false,
        reset_on_logout: false,
        reset_on_disconnect: true,
        sender_default_appl_ver_id: fix_str!("9").to_owned(),
        target_default_appl_ver_id: fix_str!("9").to_owned(),
        persist: false,
        refresh_on_logon: false,
        enable_next_expected_msg_seq_num: false,
        verify_logout: true,
        verify_test_request_id: true,
    }
}

/// Test double for `TcpConnection` - hands out in-memory streams pushed
/// through a channel by the test body.
struct TestConnection {
    incoming: mpsc::UnboundedReceiver<DuplexStream>,
}

impl Connection for TestConnection {
    async fn accept(
        &mut self,
    ) -> Result<
        (
            impl AsyncRead + Unpin + 'static,
            impl AsyncWrite + Unpin + 'static,
            SocketAddr,
        ),
        io::Error,
    > {
        match self.incoming.recv().await {
            Some(stream) => {
                let (reader, writer) = tokio::io::split(stream);
                Ok((reader, writer, "127.0.0.1:1".parse().unwrap()))
            }
            None => pending().await,
        }
    }
}

fn serialize_msg(msg: Message, msg_seq_num: SeqNum, sending_time: UtcTimestamp) -> Vec<u8> {
    FixtMessage {
        header: Box::new(Header {
            begin_string: BEGIN_STRING.to_owned(),
            msg_type: msg.msg_type(),
            sender_comp_id: CLIENT_COMP_ID.to_owned(),
            target_comp_id: SERVER_COMP_ID.to_owned(),
            msg_seq_num,
            sending_time,
            ..Default::default()
        }),
        body: Box::new(msg),
        trailer: Box::new(Trailer::default()),
    }
    .serialize()
}

fn logon() -> Message {
    Message::Logon(Logon {
        encrypt_method: EncryptMethod::NoneOther,
        heart_bt_int: 10,
        default_appl_ver_id: DefaultApplVerId::Fix50Sp2,
        ..Default::default()
    })
}

fn test_request(id: &FixStr) -> Message {
    Message::TestRequest(TestRequest {
        test_req_id: id.to_owned(),
    })
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| w == &needle)
        .count()
}

async fn pump_until_logon<S: MessagesStorage + 'static>(acceptor: &mut Acceptor<S>) {
    loop {
        let mut entry = acceptor.next().await.expect("event stream closed");
        if matches!(entry.as_event(), FixEvent::Logon(..)) {
            break;
        }
    }
}

/// Reads from `reader` until `needle` occurs `count` times, pumping acceptor
/// events on the side. Outgoing messages are written to the wire only after
/// their `AdmMsgOut`/`AppMsgOut` event is consumed (dropping the event sends
/// the default response), so reading without pumping would deadlock.
async fn read_until_pumping<S: MessagesStorage + 'static, R: AsyncRead + Unpin>(
    acceptor: &mut Acceptor<S>,
    reader: &mut R,
    buf: &mut Vec<u8>,
    needle: &[u8],
    count: usize,
) {
    let mut chunk = [0u8; 4096];
    while count_occurrences(buf, needle) < count {
        tokio::select! {
            entry = acceptor.next() => {
                let mut entry = entry.expect("event stream closed");
                let _ = entry.as_event();
            }
            read_result = reader.read(&mut chunk) => {
                let n = read_result.expect("read failed");
                assert_ne!(n, 0, "connection closed while waiting for {needle:?}");
                buf.extend_from_slice(&chunk[..n]);
            }
        }
    }
}

fn run_local_test(test: impl Future<Output = ()>) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let runtime = Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    let local_set = LocalSet::new();
    local_set
        .block_on(&runtime, async {
            timeout(Duration::from_secs(10), test).await
        })
        .expect("test timed out");
}

/// A batch of messages rejected in one go (here: stale SendingTime<52>) must
/// not kill the connection task, even when the events channel overflows
/// mid-batch while the event consumer is busy.
///
/// Before the fix the input task held a session state borrow across
/// `Emitter::send().await`; once the events channel (capacity 16) filled up,
/// the await yielded to the output stream, which panicked on
/// `state.borrow_mut()` ("RefCell already borrowed").
#[test]
fn reject_flood_does_not_kill_connection_task() {
    const FLOOD_LEN: usize = 20;

    run_local_test(async {
        let mut acceptor = Acceptor::new(settings(), Box::new(|_| InMemoryStorage::new()));
        acceptor.register_session(session_id(), session_settings());

        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        acceptor.start(TestConnection { incoming: conn_rx });

        let (client, server) = tokio::io::duplex(1024 * 1024);
        conn_tx.send(server).expect("connection refused");
        let (mut client_rx, mut client_tx) = tokio::io::split(client);

        client_tx
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .expect("logon write failed");
        pump_until_logon(&mut acceptor).await;

        // Wait until the Logon<A> response reaches the wire: the output
        // stream must be parked on its input queue, not inside
        // `on_message_out`, for the flood to hit the borrow-across-await
        // window.
        let mut buf = Vec::new();
        read_until_pumping(&mut acceptor, &mut client_rx, &mut buf, b"\x0135=A\x01", 1).await;

        // Flood with messages older than max_latency, written as one batch.
        let stale = UtcTimestamp::with_millis(Utc::now() - chrono::Duration::seconds(600));
        let mut batch = Vec::new();
        for seq_num in 2..2 + FLOOD_LEN as SeqNum {
            batch.extend_from_slice(&serialize_msg(
                test_request(fix_str!("flood")),
                seq_num,
                stale,
            ));
        }
        client_tx
            .write_all(&batch)
            .await
            .expect("flood write failed");

        // Let the connection task chew through the batch while no one
        // consumes events, so the events channel fills up mid-batch.
        sleep(Duration::from_millis(200)).await;

        let mut deserialize_errors = 0;
        while deserialize_errors < FLOOD_LEN {
            let mut entry = acceptor.next().await.expect("event stream closed");
            if matches!(entry.as_event(), FixEvent::DeserializeError(..)) {
                deserialize_errors += 1;
            }
        }

        // Every flooded message must be answered with Reject<3>.
        let mut buf = Vec::new();
        read_until_pumping(
            &mut acceptor,
            &mut client_rx,
            &mut buf,
            b"\x0135=3\x01",
            FLOOD_LEN,
        )
        .await;
    });
}

/// Storage which can be armed to panic on the next `store()` call, simulating
/// any unexpected panic inside the connection task.
struct PanickingStorage {
    inner: InMemoryStorage,
    panic_armed: Rc<Cell<bool>>,
}

impl MessagesStorage for PanickingStorage {
    fn fetch_range(&mut self, range: RangeInclusive<SeqNum>) -> impl Iterator<Item = &[u8]> {
        self.inner.fetch_range(range)
    }

    fn store(&mut self, seq_num: SeqNum, data: &[u8]) {
        if self.panic_armed.replace(false) {
            panic!("injected storage failure");
        }
        self.inner.store(seq_num, data);
    }

    fn next_sender_msg_seq_num(&self) -> SeqNum {
        self.inner.next_sender_msg_seq_num()
    }

    fn next_target_msg_seq_num(&self) -> SeqNum {
        self.inner.next_target_msg_seq_num()
    }

    fn set_next_sender_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.inner.set_next_sender_msg_seq_num(seq_num);
    }

    fn set_next_target_msg_seq_num(&mut self, seq_num: SeqNum) {
        self.inner.set_next_target_msg_seq_num(seq_num);
    }

    fn incr_next_sender_msg_seq_num(&mut self) {
        self.inner.incr_next_sender_msg_seq_num();
    }

    fn incr_next_target_msg_seq_num(&mut self) {
        self.inner.incr_next_target_msg_seq_num();
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// A panicking connection task must not leave the session behind as an
/// "active" zombie.
///
/// Before the fix the session stayed in the active sessions map forever:
/// every reconnect attempt was rejected with "Session already active" and
/// `disable_with_logout` panicked the whole process trying to send Logout<5>
/// into the closed output channel.
#[test]
fn panicked_connection_task_releases_session() {
    run_local_test(async {
        let panic_armed = Rc::new(Cell::new(false));
        let storage_panic_armed = panic_armed.clone();
        let mut acceptor = Acceptor::new(
            settings(),
            Box::new(move |_| PanickingStorage {
                inner: InMemoryStorage::new(),
                panic_armed: storage_panic_armed.clone(),
            }),
        );
        acceptor.register_session(session_id(), session_settings());

        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        acceptor.start(TestConnection { incoming: conn_rx });

        let (client, server) = tokio::io::duplex(1024 * 1024);
        conn_tx.send(server).expect("connection refused");
        let (_client_rx, mut client_tx) = tokio::io::split(client);

        client_tx
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .expect("logon write failed");
        pump_until_logon(&mut acceptor).await;

        // Kill the connection task: storage panics while the Heartbeat
        // response to this TestRequest is stored for resend.
        panic_armed.set(true);
        client_tx
            .write_all(&serialize_msg(
                test_request(fix_str!("boom")),
                2,
                UtcTimestamp::now(),
            ))
            .await
            .expect("test request write failed");

        // The dead task must clean up after itself; keep the event stream
        // drained while waiting. The cleanup must also deliver the Logout
        // event the dead output loop never emitted, so the application can
        // release its own per-connection state (login status, senders,
        // disconnect notification for other services).
        let mut logout_seen = false;
        while acceptor
            .is_session_active(&session_id())
            .expect("unknown session")
        {
            if let Ok(Some(mut entry)) = timeout(Duration::from_millis(20), acceptor.next()).await
                && matches!(entry.as_event(), FixEvent::Logout(..))
            {
                logout_seen = true;
            }
        }
        while !logout_seen {
            let mut entry = acceptor.next().await.expect("event stream closed");
            if matches!(entry.as_event(), FixEvent::Logout(..)) {
                logout_seen = true;
            }
        }

        // Reconnect must succeed (no "Session already active").
        let (client2, server2) = tokio::io::duplex(1024 * 1024);
        conn_tx.send(server2).expect("connection refused");
        let (mut client2_rx, mut client2_tx) = tokio::io::split(client2);

        client2_tx
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .expect("logon write failed");
        pump_until_logon(&mut acceptor).await;

        // Disabling the acceptor with logout must survive the earlier panic
        // and deliver Logout<5> to the connected client.
        acceptor.disable_with_logout(Some(SessionStatus::SessionLogoutComplete), None);

        let mut buf = Vec::new();
        read_until_pumping(&mut acceptor, &mut client2_rx, &mut buf, b"\x0135=5\x01", 1).await;
    });
}

// A third-party storage implementation with the unchanged, infallible API.
// Counters are observable by tests but only the session mutates them.
struct Counters {
    sender: Cell<SeqNum>,
    target: Cell<SeqNum>,
    stores: Cell<usize>,
    target_changed: Notify,
}

struct TrackingStorage(Rc<Counters>);

impl MessagesStorage for TrackingStorage {
    fn fetch_range(&mut self, _: RangeInclusive<SeqNum>) -> impl Iterator<Item = &[u8]> {
        empty()
    }

    fn store(&mut self, _: SeqNum, _: &[u8]) {
        self.0.stores.set(self.0.stores.get() + 1);
    }

    fn next_sender_msg_seq_num(&self) -> SeqNum {
        self.0.sender.get()
    }

    fn next_target_msg_seq_num(&self) -> SeqNum {
        self.0.target.get()
    }

    fn set_next_sender_msg_seq_num(&mut self, value: SeqNum) {
        self.0.sender.set(value);
    }

    fn set_next_target_msg_seq_num(&mut self, value: SeqNum) {
        self.0.target.set(value);
    }

    fn incr_next_sender_msg_seq_num(&mut self) {
        self.0.sender.set(self.0.sender.get() + 1);
    }

    fn incr_next_target_msg_seq_num(&mut self) {
        self.0.target.set(self.0.target.get() + 1);
        self.0.target_changed.notify_one();
    }

    fn reset(&mut self) {
        self.0.sender.set(1);
        self.0.target.set(1);
    }
}

fn tracked_acceptor(sender: SeqNum, target: SeqNum) -> (Acceptor<TrackingStorage>, Rc<Counters>) {
    let counters = Rc::new(Counters {
        sender: Cell::new(sender),
        target: Cell::new(target),
        stores: Cell::new(0),
        target_changed: Notify::new(),
    });
    let storage_counters = counters.clone();
    let mut acceptor = Acceptor::new(
        settings(),
        Box::new(move |_| TrackingStorage(storage_counters.clone())),
    );
    acceptor.register_session(session_id(), session_settings());
    (acceptor, counters)
}

async fn connected(acceptor: &mut Acceptor<TrackingStorage>) -> (DuplexStream, Sender) {
    let (client, server) = tokio::io::duplex(65536);
    let (tx, incoming) = mpsc::unbounded_channel();
    acceptor.start(TestConnection { incoming });
    tx.send(server).unwrap();
    let mut client = client;
    client
        .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
        .await
        .unwrap();
    let sender = loop {
        let mut entry = acceptor.next().await.unwrap();
        if let FixEvent::Logon(_, sender) = entry.as_event() {
            break sender;
        }
    };
    let mut bytes = Vec::new();
    read_until_pumping(acceptor, &mut client, &mut bytes, b"\x0135=A\x01", 1).await;
    (client, sender)
}

fn sequence_reset(new_seq_no: SeqNum, gap_fill: bool) -> Message {
    Message::SequenceReset(SequenceReset {
        new_seq_no,
        gap_fill_flag: Some(gap_fill),
    })
}

#[test]
fn responder_abort_vetoes_reset_and_discards_queued_input() {
    run_local_test(async {
        const LIMIT: SeqNum = (1 << 28) - 1;
        for gap_fill in [false, true] {
            let (mut acceptor, counters) = tracked_acceptor(1, 1);
            let (mut client, sender) = connected(&mut acceptor).await;
            // Both messages are deferred until the missing sequence 2 arrives.
            // GapFill is then checked by the existing responder, before NewSeqNo is applied.
            let reset_seq = if gap_fill { 3 } else { 2 };
            let mut batch = serialize_msg(
                sequence_reset(LIMIT + 1, gap_fill),
                reset_seq,
                UtcTimestamp::now(),
            );
            batch.extend(serialize_msg(
                test_request(fix_str!("must-not-run")),
                reset_seq + 1,
                UtcTimestamp::now(),
            ));
            if gap_fill {
                batch.extend(serialize_msg(
                    test_request(fix_str!("fill-gap")),
                    2,
                    UtcTimestamp::now(),
                ));
            }
            client.write_all(&batch).await.unwrap();
            loop {
                let mut entry = acceptor.next().await.unwrap();
                if let FixEvent::AdmMsgIn(msg, responder) = entry.as_event()
                    && matches!(*msg.body, Message::SequenceReset(_))
                {
                    responder.abort();
                    break;
                }
            }
            let stores = counters.stores.get();
            let outgoing = counters.sender.get();
            let mut remaining = Vec::new();
            client.read_to_end(&mut remaining).await.unwrap();
            assert_eq!(count_occurrences(&remaining, b"\x0135=5\x01"), 0);
            assert_eq!(counters.target.get(), reset_seq);
            assert_eq!(counters.sender.get(), outgoing);
            assert_eq!(counters.stores.get(), stores);
            assert!(
                sender
                    .send(Box::new(test_request(fix_str!("late"))))
                    .is_err()
            );
            assert!(!acceptor.is_session_active(&session_id()).unwrap());
        }
    });
}

#[test]
fn abort_closes_transport_with_output_responder_held() {
    run_local_test(async {
        let (mut acceptor, counters) = tracked_acceptor(1, 1);
        let (mut client, sender) = connected(&mut acceptor).await;
        for _ in 0..32 {
            sender
                .send(Box::new(test_request(fix_str!("backlog"))))
                .unwrap();
        }
        let mut held = acceptor.next().await.unwrap();
        assert!(matches!(held.as_event(), FixEvent::AdmMsgOut(_)));
        let next = counters.sender.get();
        let stores = counters.stores.get();
        acceptor.abort(&session_id()).unwrap();
        acceptor.abort(&session_id()).unwrap();
        // Do not drop the output event or consume Logout until the transport is closed.
        let mut remaining = Vec::new();
        client.read_to_end(&mut remaining).await.unwrap();
        assert!(remaining.is_empty());
        assert_eq!(counters.sender.get(), next);
        assert_eq!(counters.stores.get(), stores);
        assert!(!acceptor.is_session_active(&session_id()).unwrap());
        drop(held); // A late output response must not panic.
        let mut logout = acceptor.next().await.unwrap();
        assert!(matches!(
            logout.as_event(),
            FixEvent::Logout(_, DisconnectReason::ApplicationForcedDisconnect)
        ));
    });
}

#[test]
fn abort_interrupts_first_logon_and_stale_responder_cannot_abort_reconnect() {
    run_local_test(async {
        let (mut acceptor, _) = tracked_acceptor(1, 1);
        let (mut client, server) = tokio::io::duplex(65536);
        let (tx, incoming) = mpsc::unbounded_channel();
        acceptor.start(TestConnection { incoming });
        tx.send(server).unwrap();
        client
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .unwrap();
        let mut held = acceptor.next().await.unwrap();
        let FixEvent::AdmMsgIn(_, old_responder) = held.as_event() else {
            panic!("expected Logon input");
        };
        acceptor.abort(&session_id()).unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
        assert!(!acceptor.is_session_active(&session_id()).unwrap());
        let (mut new_client, sender) = connected(&mut acceptor).await;
        old_responder.abort();
        sender
            .send(Box::new(test_request(fix_str!("still-alive"))))
            .unwrap();
        read_until_pumping(
            &mut acceptor,
            &mut new_client,
            &mut bytes,
            b"still-alive",
            1,
        )
        .await;
        acceptor.abort(&session_id()).unwrap();
    });
}

#[test]
fn numbering_exhaustion_aborts_and_requires_explicit_inactive_reset() {
    run_local_test(async {
        for sender_exhausted in [false, true] {
            let (mut acceptor, counters) = tracked_acceptor(1, 1);
            let (mut client, sender) = connected(&mut acceptor).await;
            if sender_exhausted {
                acceptor
                    .set_next_sender_msg_seq_num(&session_id(), SeqNum::MAX - 1)
                    .unwrap();
                sender
                    .send(Box::new(test_request(fix_str!("exhaust"))))
                    .unwrap();
            } else {
                client
                    .write_all(&serialize_msg(
                        sequence_reset(SeqNum::MAX, false),
                        2,
                        UtcTimestamp::now(),
                    ))
                    .await
                    .unwrap();
                let mut entry = acceptor.next().await.unwrap();
                assert!(matches!(entry.as_event(), FixEvent::AdmMsgIn(..)));
                drop(entry);
            }
            let mut remaining = Vec::new();
            client.read_to_end(&mut remaining).await.unwrap();
            assert!(remaining.is_empty());
            assert_eq!(
                if sender_exhausted {
                    counters.sender.get()
                } else {
                    counters.target.get()
                },
                SeqNum::MAX
            );
            assert!(!acceptor.is_session_active(&session_id()).unwrap());
            let mut logout = acceptor.next().await.unwrap();
            assert!(matches!(
                logout.as_event(),
                FixEvent::Logout(_, DisconnectReason::SequenceNumberExhausted)
            ));
            drop(logout);
            acceptor.reset(&session_id()).unwrap();
            assert_eq!(counters.sender.get(), 1);
            assert_eq!(counters.target.get(), 1);
            let (_client, _) = connected(&mut acceptor).await;
            acceptor.abort(&session_id()).unwrap();
        }
    });
}

#[test]
fn stored_maximum_aborts_before_first_logon_callback() {
    run_local_test(async {
        for (sender, target) in [(SeqNum::MAX, 1), (1, SeqNum::MAX)] {
            let (acceptor, counters) = tracked_acceptor(sender, target);
            let (mut client, server) = tokio::io::duplex(65536);
            let (tx, incoming) = mpsc::unbounded_channel();
            acceptor.start(TestConnection { incoming });
            tx.send(server).unwrap();
            client
                .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
                .await
                .unwrap();
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await.unwrap();
            assert!(bytes.is_empty());
            assert_eq!(counters.sender.get(), sender);
            assert_eq!(counters.target.get(), target);
            assert_eq!(counters.stores.get(), 0);
            assert!(!acceptor.is_session_active(&session_id()).unwrap());
        }
    });
}

struct WriteProbe {
    blocked: Cell<bool>,
    calls: Cell<usize>,
    dropped: Cell<bool>,
    pending: Notify,
}

struct BlockedWriter {
    inner: WriteHalf<DuplexStream>,
    probe: Rc<WriteProbe>,
}

impl Drop for BlockedWriter {
    fn drop(&mut self) {
        self.probe.dropped.set(true);
    }
}

impl AsyncWrite for BlockedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.probe.calls.set(self.probe.calls.get() + 1);
        if self.probe.blocked.get() {
            self.probe.pending.notify_one();
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

struct BlockedConnection {
    stream: Option<DuplexStream>,
    probe: Rc<WriteProbe>,
}

impl Connection for BlockedConnection {
    async fn accept(
        &mut self,
    ) -> Result<
        (
            impl AsyncRead + Unpin + 'static,
            impl AsyncWrite + Unpin + 'static,
            SocketAddr,
        ),
        io::Error,
    > {
        let Some(stream) = self.stream.take() else {
            return pending().await;
        };
        let (reader, writer) = tokio::io::split(stream);
        Ok((
            reader,
            BlockedWriter {
                inner: writer,
                probe: self.probe.clone(),
            },
            "127.0.0.1:1".parse().unwrap(),
        ))
    }
}

#[test]
fn abort_cancels_a_pending_write_without_polling_it_again() {
    run_local_test(async {
        let (mut acceptor, counters) = tracked_acceptor(1, 1);
        let (mut client, server) = tokio::io::duplex(65536);
        let probe = Rc::new(WriteProbe {
            blocked: Cell::new(false),
            calls: Cell::new(0),
            dropped: Cell::new(false),
            pending: Notify::new(),
        });
        acceptor.start(BlockedConnection {
            stream: Some(server),
            probe: probe.clone(),
        });
        client
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .unwrap();
        let sender = loop {
            let mut entry = acceptor.next().await.unwrap();
            if let FixEvent::Logon(_, sender) = entry.as_event() {
                break sender;
            }
        };
        let mut bytes = Vec::new();
        read_until_pumping(&mut acceptor, &mut client, &mut bytes, b"\x0135=A\x01", 1).await;
        probe.blocked.set(true);
        sender
            .send(Box::new(test_request(fix_str!("pending-write"))))
            .unwrap();
        let mut entry = acceptor.next().await.unwrap();
        assert!(matches!(entry.as_event(), FixEvent::AdmMsgOut(_)));
        drop(entry);
        probe.pending.notified().await;
        let calls = probe.calls.get();
        let stores = counters.stores.get();
        acceptor.abort(&session_id()).unwrap();
        probe.blocked.set(false); // A further poll_write would now succeed.
        let mut remaining = Vec::new();
        client.read_to_end(&mut remaining).await.unwrap();
        assert!(remaining.is_empty());
        assert_eq!(probe.calls.get(), calls);
        assert!(probe.dropped.get());
        assert_eq!(counters.stores.get(), stores);
    });
}

#[test]
fn abort_closes_transport_even_when_event_channel_is_full() {
    run_local_test(async {
        let (mut acceptor, counters) = tracked_acceptor(1, 1);
        let (mut client, _) = connected(&mut acceptor).await;
        let stale = UtcTimestamp::with_millis(Utc::now() - chrono::Duration::seconds(600));
        let mut batch = Vec::new();
        for seq in 2..66 {
            batch.extend(serialize_msg(
                test_request(fix_str!("full-events")),
                seq,
                stale,
            ));
        }
        client.write_all(&batch).await.unwrap();
        // Each rejected input advances target before attempting to emit an
        // event. At least 16 attempts fill the bounded channel; do not drain it.
        while counters.target.get() < 18 {
            counters.target_changed.notified().await;
        }
        acceptor.abort(&session_id()).unwrap();
        let target = counters.target.get();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
        assert_eq!(counters.target.get(), target);
        assert!(!acceptor.is_session_active(&session_id()).unwrap());
        loop {
            let mut entry = acceptor.next().await.unwrap();
            if let FixEvent::Logout(_, reason) = entry.as_event() {
                assert!(matches!(
                    reason,
                    DisconnectReason::ApplicationForcedDisconnect
                ));
                break;
            }
        }
    });
}
