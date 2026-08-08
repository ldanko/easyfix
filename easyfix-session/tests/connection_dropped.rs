//! Connections that never become sessions are invisible to the application -
//! they are registered nowhere, so `Acceptor::peer_addr` cannot reach them.
//! They must therefore be reported as events, which is what lets an
//! application recognize CompID enumeration.

use std::{io, net::SocketAddr, time::Duration};

use chrono::NaiveTime;
use easyfix_macros::fix_str;
use easyfix_messages::{
    fields::{DefaultApplVerId, EncryptMethod, FixStr, SeqNum, UtcTimestamp},
    messages::{FixtMessage, Header, Logon, Message, Trailer},
};
use easyfix_session::{
    acceptor::{Acceptor, Connection},
    application::{AsEvent, ConnectionDropReason, FixEvent},
    messages_storage::{InMemoryStorage, MessagesStorage},
    session_id::SessionId,
    settings::{SessionSettings, Settings},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream},
    runtime::Builder,
    sync::mpsc,
    task::LocalSet,
    time::timeout,
};
use tokio_stream::StreamExt;

const BEGIN_STRING: &FixStr = fix_str!("FIXT.1.1");
const SERVER_COMP_ID: &FixStr = fix_str!("server");
const CLIENT_COMP_ID: &FixStr = fix_str!("client");
const PEER_ADDR: &str = "10.20.30.40:5678";

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

/// Test double for `TcpConnection`, reporting a fixed peer address.
struct TestConnection {
    incoming: mpsc::UnboundedReceiver<DuplexStream>,
    peer_addr: SocketAddr,
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
                Ok((reader, writer, self.peer_addr))
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

/// Drain the event stream until a connection drop is reported, returning it in
/// owned form (the event borrows).
async fn pump_until_dropped<S: MessagesStorage + 'static>(
    acceptor: &mut Acceptor<S>,
) -> (SocketAddr, ConnectionDropReason) {
    loop {
        let mut entry = acceptor.next().await.expect("event stream closed");
        if let FixEvent::ConnectionDropped(peer_addr, reason) = entry.as_event() {
            return (peer_addr, reason.clone());
        }
    }
}

fn run_local_test(test: impl Future<Output = ()>) {
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

/// A Logon<A> naming an unconfigured identity is dropped without a reply, and
/// reported with that identity. Many *distinct* unknown ids from one address
/// is what distinguishes CompID enumeration from a misconfigured peer, so the
/// id - not just the address - has to reach the application.
#[test]
fn unknown_session_id_is_reported_with_identity() {
    run_local_test(async {
        let peer_addr: SocketAddr = PEER_ADDR.parse().unwrap();
        // Deliberately NOT registered.
        let mut acceptor = Acceptor::new(settings(), Box::new(|_| InMemoryStorage::new()));

        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        acceptor.start(TestConnection {
            incoming: conn_rx,
            peer_addr,
        });

        let (client, server) = tokio::io::duplex(1024 * 1024);
        conn_tx.send(server).expect("connection refused");
        let (_client_rx, mut client_tx) = tokio::io::split(client);
        client_tx
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .expect("logon write failed");

        let (reported_addr, reason) = pump_until_dropped(&mut acceptor).await;
        assert_eq!(reported_addr, peer_addr);
        match reason {
            ConnectionDropReason::UnknownSession(id) => assert_eq!(id, session_id()),
            other => panic!("expected UnknownSession, got {other:?}"),
        }

        // The session never existed, so nothing is registered for it.
        assert_eq!(acceptor.peer_addr(&session_id()), None);
    });
}

/// A peer that connects and closes without a usable Logon<A> is reported too.
/// It carries no identity - `first_msg` collapses silence, timeout, EOF and
/// undecodable input into one condition - but repeated from one address it is
/// still a signal worth counting.
#[test]
fn connection_without_logon_is_reported_without_identity() {
    run_local_test(async {
        let peer_addr: SocketAddr = PEER_ADDR.parse().unwrap();
        let mut acceptor = Acceptor::new(settings(), Box::new(|_| InMemoryStorage::new()));
        acceptor.register_session(session_id(), session_settings());

        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        acceptor.start(TestConnection {
            incoming: conn_rx,
            peer_addr,
        });

        let (client, server) = tokio::io::duplex(1024 * 1024);
        conn_tx.send(server).expect("connection refused");
        // Close without sending anything - the acceptor sees end of stream
        // rather than waiting out the logon timeout.
        drop(client);

        let (reported_addr, reason) = pump_until_dropped(&mut acceptor).await;
        assert_eq!(reported_addr, peer_addr);
        match reason {
            ConnectionDropReason::LogonNotReceived => {}
            other => panic!("expected LogonNotReceived, got {other:?}"),
        }
    });
}

/// A second connection for an already-established session is dropped without a
/// Logout<5>, which would consume a MsgSeqNum(34) and disturb the live session
/// (FIX Session Layer 4.6.4). The drop must still be observable.
#[test]
fn duplicate_connection_is_reported_while_session_stays_up() {
    run_local_test(async {
        let peer_addr: SocketAddr = PEER_ADDR.parse().unwrap();
        let mut acceptor = Acceptor::new(settings(), Box::new(|_| InMemoryStorage::new()));
        acceptor.register_session(session_id(), session_settings());

        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        acceptor.start(TestConnection {
            incoming: conn_rx,
            peer_addr,
        });

        // First connection logs on normally.
        let (client, server) = tokio::io::duplex(1024 * 1024);
        conn_tx.send(server).expect("connection refused");
        let (_client_rx, mut client_tx) = tokio::io::split(client);
        client_tx
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .expect("logon write failed");
        loop {
            let mut entry = acceptor.next().await.expect("event stream closed");
            if matches!(entry.as_event(), FixEvent::Logon(..)) {
                break;
            }
        }

        // Second connection claims the same identity.
        let (client2, server2) = tokio::io::duplex(1024 * 1024);
        conn_tx.send(server2).expect("connection refused");
        let (_client2_rx, mut client2_tx) = tokio::io::split(client2);
        client2_tx
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .expect("logon write failed");

        let (reported_addr, reason) = pump_until_dropped(&mut acceptor).await;
        assert_eq!(reported_addr, peer_addr);
        match reason {
            ConnectionDropReason::SessionAlreadyActive(id) => assert_eq!(id, session_id()),
            other => panic!("expected SessionAlreadyActive, got {other:?}"),
        }

        // The original session must be untouched by the refused duplicate.
        assert_eq!(acceptor.peer_addr(&session_id()), Some(peer_addr));
    });
}
