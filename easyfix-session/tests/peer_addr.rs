//! The acceptor must expose the peer address of an active session, so the
//! application can apply source-address policies while handling Logon<A>.

use std::{io, net::SocketAddr, time::Duration};

use chrono::NaiveTime;
use easyfix_macros::fix_str;
use easyfix_messages::{
    fields::{DefaultApplVerId, EncryptMethod, FixStr, SeqNum, UtcTimestamp},
    messages::{FixtMessage, Header, Logon, Message, Trailer},
};
use easyfix_session::{
    DisconnectReason,
    acceptor::{Acceptor, Connection},
    application::{AsEvent, FixEvent},
    messages_storage::{InMemoryStorage, MessagesStorage},
    session_id::SessionId,
    settings::{SessionSettings, Settings},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    net::{TcpListener, TcpStream},
    runtime::Builder,
    sync::mpsc,
    task::{LocalSet, spawn_local},
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

async fn pump_until_logon<S: MessagesStorage + 'static>(acceptor: &mut Acceptor<S>) {
    loop {
        let mut entry = acceptor.next().await.expect("event stream closed");
        if matches!(entry.as_event(), FixEvent::Logon(..)) {
            break;
        }
    }
}

fn run_local_test(test: impl Future<Output = ()>) {
    let runtime = Builder::new_current_thread()
        .enable_io()
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

/// The address reported by `Connection::accept` must be readable through the
/// acceptor for as long as the session is active.
#[test]
fn peer_addr_of_active_session_is_exposed() {
    run_local_test(async {
        let peer_addr: SocketAddr = PEER_ADDR.parse().unwrap();
        let mut acceptor = Acceptor::new(settings(), Box::new(|_| InMemoryStorage::new()));
        acceptor.register_session(session_id(), session_settings());

        // No connection yet, so no address.
        assert_eq!(acceptor.peer_addr(&session_id()), None);

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
        pump_until_logon(&mut acceptor).await;

        assert_eq!(acceptor.peer_addr(&session_id()), Some(peer_addr));
    });
}

#[test]
fn abort_closes_tcp_without_sending_logout() {
    run_local_test(async {
        let mut acceptor = Acceptor::new(settings(), Box::new(|_| InMemoryStorage::new()));
        acceptor.register_session(session_id(), session_settings());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, peer_addr) = listener.accept().await.unwrap();
        let (reader, writer) = server.into_split();
        let connection = spawn_local(acceptor.session_task().run(peer_addr, reader, writer));

        client
            .write_all(&serialize_msg(logon(), 1, UtcTimestamp::now()))
            .await
            .unwrap();
        pump_until_logon(&mut acceptor).await;
        loop {
            let mut entry = acceptor.next().await.unwrap();
            if matches!(entry.as_event(), FixEvent::AdmMsgOut(msg) if matches!(*msg.body, Message::Logon(_)))
            {
                break;
            }
        }

        // Consume the complete Logon response before abort, so every remaining
        // byte would have been sent by the shutdown path.
        let mut response = Vec::new();
        while !response.ends_with(b"\x0110=") {
            response.push(client.read_u8().await.unwrap());
        }
        let mut checksum = [0; 4];
        client.read_exact(&mut checksum).await.unwrap();
        response.extend_from_slice(&checksum);
        let response = FixtMessage::from_bytes(&response).unwrap();
        assert!(matches!(*response.body, Message::Logon(_)));

        let next_sender = acceptor.next_sender_msg_seq_num(&session_id()).unwrap();
        acceptor.abort(&session_id()).unwrap();
        acceptor.abort(&session_id()).unwrap();

        let mut trailing_bytes = Vec::new();
        client.read_to_end(&mut trailing_bytes).await.unwrap();
        assert!(
            trailing_bytes.is_empty(),
            "abort sent bytes: {trailing_bytes:?}"
        );
        connection.await.unwrap();
        assert_eq!(acceptor.peer_addr(&session_id()), None);
        assert_eq!(
            acceptor.next_sender_msg_seq_num(&session_id()).unwrap(),
            next_sender
        );

        let mut entry = acceptor.next().await.unwrap();
        assert!(matches!(
            entry.as_event(),
            FixEvent::Logout(_, DisconnectReason::ApplicationForcedDisconnect)
        ));
    });
}
