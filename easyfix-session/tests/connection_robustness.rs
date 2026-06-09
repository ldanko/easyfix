//! Connection task fault tolerance.
//!
//! A connection task must survive hostile input (e.g. a burst of rejected
//! messages saturating the events channel) and, when it dies anyway, it must
//! release the session so the peer can reconnect and the acceptor API remains
//! safe to call.

use std::{cell::Cell, io, net::SocketAddr, ops::RangeInclusive, rc::Rc, time::Duration};

use chrono::NaiveTime;
use easyfix_macros::fix_str;
use easyfix_messages::{
    fields::{
        DefaultApplVerId, EncryptMethod, FixStr, SeqNum, SessionStatus, Utc, UtcTimestamp,
    },
    messages::{FixtMessage, Header, Logon, Message, TestRequest, Trailer},
};
use easyfix_session::{
    acceptor::{Acceptor, Connection},
    application::{AsEvent, FixEvent},
    messages_storage::{InMemoryStorage, MessagesStorage},
    session_id::SessionId,
    settings::{SessionSettings, Settings},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    runtime::Builder,
    sync::mpsc,
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
            None => std::future::pending().await,
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
    haystack.windows(needle.len()).filter(|w| w == &needle).count()
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
            batch.extend_from_slice(&serialize_msg(test_request(fix_str!("flood")), seq_num, stale));
        }
        client_tx.write_all(&batch).await.expect("flood write failed");

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
        read_until_pumping(&mut acceptor, &mut client_rx, &mut buf, b"\x0135=3\x01", FLOOD_LEN)
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
            .write_all(&serialize_msg(test_request(fix_str!("boom")), 2, UtcTimestamp::now()))
            .await
            .expect("test request write failed");

        // The dead task must clean up after itself; keep the event stream
        // drained while waiting.
        while acceptor.is_session_active(&session_id()).expect("unknown session") {
            if let Ok(Some(mut entry)) = timeout(Duration::from_millis(20), acceptor.next()).await
            {
                let _ = entry.as_event();
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
