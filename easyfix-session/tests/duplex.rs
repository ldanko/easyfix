//! End-to-end duplex integration tests for `easyfix-session`.
//!
//! These tests exercise the public [`Acceptor`] / [`Initiator`] API over
//! paired [`tokio::io::duplex`] streams.

use std::{
    assert_matches,
    borrow::Cow,
    cell::{Cell, RefCell},
    fmt, mem,
    num::NonZeroUsize,
    rc::Rc,
    time::Duration,
};

use easyfix_core::{
    base_messages::{
        AdminBase, EncryptMethodBase, HeartbeatBase, LogonBase, MsgTypeBase, ResendRequestBase,
        SequenceResetBase, SessionRejectReasonBase,
    },
    basic_types::{ApplVerId, FixStr, Int, NonZeroSeqNum, SeqNum},
    fix_str,
    message::{HeaderAccess, SessionMessage},
    version::Version,
};
use easyfix_session::{
    Acceptor, AcceptorError, Application, ApplicationFactory, DisconnectReason, InMemoryStorage,
    Initiator, InputAction, MessagesStorage, Sender, SerializeError, SessionContext, SessionId,
    SessionSettings, ShutdownMode,
};
use easyfix_test_messages::{Body, Message};
use futures_util::FutureExt;
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf, split},
    sync::mpsc::{self, error::TryRecvError},
    task::{JoinHandle, LocalSet, spawn_local, yield_now},
    time::{self, timeout},
};

#[path = "common/fixtures.rs"]
mod common;
use common::{
    ObservedStorage, RecordingObserver, StorageSnapshot, TEST_PEER_ADDR, TEST_TIMEOUT,
    build_session_settings, new_order_single_with_empty_header, peer_header, peer_logon_bytes,
    peer_logout_bytes, peer_test_request_bytes, read_one_message, serialize_message,
    try_read_one_message,
};

fn reset_settings() -> SessionSettings {
    let mut settings = build_session_settings(1);
    settings.heartbeat_interval = None;
    settings
}

struct ResetApp {
    inner: TestApp,
    refuse: Rc<Cell<bool>>,
}

impl Application<Message> for ResetApp {
    fn on_serialize_error(&mut self, msg: Box<Message>, error: &SerializeError) {
        self.inner.on_serialize_error(msg, error);
    }

    async fn on_session_ready(&mut self, id: &SessionId, sender: Sender<Message>) {
        self.inner.on_session_ready(id, sender).await;
    }

    async fn on_session_end(&mut self, id: &SessionId, reason: DisconnectReason) {
        self.inner.on_session_end(id, reason).await;
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        self.inner.on_app_msg_in(msg).await
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        let action = self.inner.on_admin_msg_in(msg).await;
        if self.refuse.get()
            && matches!(msg.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true))
        {
            InputAction::Logout {
                session_status: None,
                text: Some(fix_str!("Peer rejected reset").to_owned()),
                disconnect: true,
            }
        } else {
            action
        }
    }
}

struct ResetFactory {
    events: mpsc::UnboundedSender<TestEvent>,
    refuse: Rc<Cell<bool>>,
}

impl ApplicationFactory<Message> for ResetFactory {
    type App = ResetApp;

    fn create(&self, ctx: &SessionContext<'_>) -> ResetApp {
        ResetApp {
            inner: TestApp {
                session_id: ctx.session_id().clone(),
                events_tx: self.events.clone(),
            },
            refuse: self.refuse.clone(),
        }
    }
}

struct ResetPair {
    acceptor: Acceptor<Message, ObservedStorage, ResetFactory>,
    initiator: Initiator<Message, ObservedStorage, ResetFactory>,
    acc_events: mpsc::UnboundedReceiver<TestEvent>,
    ini_events: mpsc::UnboundedReceiver<TestEvent>,
    acc_storage: Rc<RefCell<StorageSnapshot>>,
    ini_storage: Rc<RefCell<StorageSnapshot>>,
    refuse: Rc<Cell<bool>>,
}

fn reset_pair() -> ResetPair {
    let (acc_events_tx, acc_events) = mpsc::unbounded_channel();
    let (ini_events_tx, ini_events) = mpsc::unbounded_channel();
    let refuse = Rc::new(Cell::new(false));
    let acceptor = Acceptor::new(ResetFactory {
        events: acc_events_tx,
        refuse: refuse.clone(),
    });
    let (acc_storage, acc_snapshot) = ObservedStorage::new(4096);
    acceptor
        .register_session(acceptor_session_id(), reset_settings(), |_, _| {
            Ok(acc_storage)
        })
        .unwrap();
    let (ini_storage, ini_snapshot) = ObservedStorage::new(4096);
    let initiator = Initiator::new(
        initiator_session_id(),
        reset_settings(),
        ResetFactory {
            events: ini_events_tx,
            refuse: Rc::new(Cell::new(false)),
        },
        |_, _| Ok(ini_storage),
    )
    .unwrap();
    ResetPair {
        acceptor,
        initiator,
        acc_events,
        ini_events,
        acc_storage: acc_snapshot,
        ini_storage: ini_snapshot,
        refuse,
    }
}

fn assert_snapshot_counts(snapshot: &Rc<RefCell<StorageSnapshot>>, sender: SeqNum, target: SeqNum) {
    let state = snapshot.borrow();
    assert_eq!(state.sender.get(), sender);
    assert_eq!(state.target.get(), target);
}

async fn expect_admin(
    events: &mut mpsc::UnboundedReceiver<TestEvent>,
    kind: MsgTypeBase,
) -> Box<Message> {
    let TestEvent::AdminMsgIn(_, msg) = recv_event(events).await else {
        panic!("expected admin callback");
    };
    assert_eq!(SessionMessage::msg_type(&*msg), kind);
    msg
}

struct RelayWire {
    wire: DuplexStream,
    buffer: Vec<u8>,
}

impl RelayWire {
    async fn read(&mut self) -> Box<Message> {
        timeout(
            TEST_TIMEOUT,
            read_one_message(&mut self.wire, &mut self.buffer),
        )
        .await
        .unwrap()
    }

    async fn send(&mut self, msg: &Message) {
        self.wire.write_all(&serialize_message(msg)).await.unwrap();
    }

    async fn close_input(&mut self) {
        self.wire.shutdown().await.unwrap();
    }

    async fn assert_closed_without_output(&mut self) {
        self.wire.read_to_end(&mut self.buffer).await.unwrap();
        assert!(self.buffer.is_empty());
    }
}

async fn connect_relay_pair(
    pair: &mut ResetPair,
    seq: SeqNum,
) -> (JoinHandle<()>, JoinHandle<()>, RelayWire, RelayWire) {
    let (acc_wire, acc_peer) = io::duplex(65536);
    let (ini_wire, ini_peer) = io::duplex(65536);
    let (acc_r, acc_w) = split(acc_wire);
    let (ini_r, ini_w) = split(ini_wire);
    let acc_task = pair.acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
    let ini_task = pair.initiator.run_session(ini_r, ini_w, None).unwrap();
    let mut acc = RelayWire {
        wire: acc_peer,
        buffer: Vec::new(),
    };
    let mut ini = RelayWire {
        wire: ini_peer,
        buffer: Vec::new(),
    };
    let logon = ini.read().await;
    assert_eq!(logon.msg_seq_num(), seq);
    assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag.is_none());
    acc.send(&logon).await;
    let ack = acc.read().await;
    assert_eq!(ack.msg_seq_num(), seq);
    assert_matches!(ack.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag.is_none());
    assert_eq!(logon.sender_comp_id(), ack.target_comp_id());
    assert_eq!(logon.target_comp_id(), ack.sender_comp_id());
    ini.send(&ack).await;
    assert_matches!(
        recv_event(&mut pair.ini_events).await,
        TestEvent::SessionReady(..)
    );
    expect_admin(&mut pair.ini_events, MsgTypeBase::Logon).await;
    expect_admin(&mut pair.acc_events, MsgTypeBase::Logon).await;
    assert_matches!(
        recv_event(&mut pair.acc_events).await,
        TestEvent::SessionReady(..)
    );
    (acc_task, ini_task, acc, ini)
}

#[tokio::test(start_paused = true)]
async fn graceful_shutdown_during_a_reset_leaves_both_sides_consistent() {
    LocalSet::new().run_until(async {
        let mut pair = reset_pair();
        let (acc_task, ini_task, mut acc, mut ini) = connect_relay_pair(&mut pair, 1).await;
        pair.acceptor.request_running_session_reset(&acceptor_session_id()).await.unwrap();
        let probe = acc.read().await; ini.send(&probe).await;
        let answer = ini.read().await; acc.send(&answer).await;
        assert_matches!((probe.try_as_admin(), answer.try_as_admin()), (Some(AdminBase::TestRequest(t)), Some(AdminBase::Heartbeat(h))) if h.test_req_id.as_deref() == Some(t.test_req_id.as_ref()));
        let logon = acc.read().await;
        assert_eq!(logon.msg_seq_num(), 1); assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
        ini.send(&logon).await;
        let ack = ini.read().await;
        assert_eq!(ack.msg_seq_num(), 1); assert_matches!(ack.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
        assert_snapshot_counts(&pair.acc_storage, 2, 1); assert_snapshot_counts(&pair.ini_storage, 2, 2);
        let stopping = pair.acceptor.clone();
        let shutdown = spawn_local(async move { stopping.shutdown(ShutdownMode::GracefulLogout { session_status: None, text: None }).await; });
        let logout = acc.read().await;
        assert_eq!(logout.msg_seq_num(), 2); assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(_)));
        acc.send(&ack).await;
        expect_admin(&mut pair.acc_events, MsgTypeBase::Heartbeat).await;
        let callback = expect_admin(&mut pair.acc_events, MsgTypeBase::Logon).await;
        assert_eq!(serialize_message(&callback), serialize_message(&ack));
        assert_snapshot_counts(&pair.acc_storage, 3, 2);
        ini.send(&logout).await;
        let logout_ack = ini.read().await;
        assert_eq!(logout_ack.msg_seq_num(), 2); assert_matches!(logout_ack.try_as_admin(), Some(AdminBase::Logout(_)));
        acc.send(&logout_ack).await;
        expect_admin(&mut pair.acc_events, MsgTypeBase::Logout).await;
        assert_matches!(recv_event(&mut pair.acc_events).await, TestEvent::SessionEnd(_, DisconnectReason::LocalRequestedLogout));
        shutdown.await.unwrap(); acc_task.await.unwrap();
        ini.close_input().await;
        expect_admin(&mut pair.ini_events, MsgTypeBase::TestRequest).await;
        expect_admin(&mut pair.ini_events, MsgTypeBase::Logon).await;
        expect_admin(&mut pair.ini_events, MsgTypeBase::Logout).await;
        assert_matches!(recv_event(&mut pair.ini_events).await, TestEvent::SessionEnd(_, DisconnectReason::RemoteRequestedLogout));
        ini_task.await.unwrap(); acc.assert_closed_without_output().await; ini.assert_closed_without_output().await;
        assert_snapshot_counts(&pair.acc_storage, 3, 3); assert_snapshot_counts(&pair.ini_storage, 3, 3);
        for (snapshot, first, second) in [(&pair.acc_storage, &logon, &logout), (&pair.ini_storage, &ack, &logout_ack)] {
            let state = snapshot.borrow(); assert_eq!(state.messages.len(), 2);
            assert_eq!(state.messages[&NonZeroSeqNum::new(1).unwrap()], serialize_message(first));
            assert_eq!(state.messages[&NonZeroSeqNum::new(2).unwrap()], serialize_message(second));
        }
        let saved = pair.acceptor.remove_session(&acceptor_session_id()).unwrap();
        let (events, received) = mpsc::unbounded_channel();
        pair.acceptor = Acceptor::new(ResetFactory { events, refuse: pair.refuse.clone() });
        pair.acceptor.register_session(acceptor_session_id(), reset_settings(), |_, _| Ok(saved)).unwrap();
        pair.acc_events = received;
        let (acc_task, ini_task, mut acc, mut ini) = connect_relay_pair(&mut pair, 3).await;
        assert_snapshot_counts(&pair.acc_storage, 4, 4); assert_snapshot_counts(&pair.ini_storage, 4, 4);
        pair.initiator.disconnect().await.unwrap();
        assert_matches!(recv_event(&mut pair.ini_events).await, TestEvent::SessionEnd(_, DisconnectReason::Disconnected));
        ini_task.await.unwrap(); acc.close_input().await;
        assert_matches!(recv_event(&mut pair.acc_events).await, TestEvent::SessionEnd(_, DisconnectReason::Disconnected));
        acc_task.await.unwrap(); acc.assert_closed_without_output().await; ini.assert_closed_without_output().await;
        assert!(pair.acc_events.try_recv().is_err()); assert!(pair.ini_events.try_recv().is_err());
    }).await;
}

#[tokio::test(start_paused = true)]
async fn in_session_reset_refused_by_peer() {
    LocalSet::new().run_until(async {
        let mut pair = reset_pair();
        let (acc_task, ini_task, mut acc, mut ini) = connect_relay_pair(&mut pair, 1).await;
        pair.refuse.set(true);
        pair.initiator.request_running_session_reset().await.unwrap();
        let probe = ini.read().await; acc.send(&probe).await;
        let answer = acc.read().await; ini.send(&answer).await;
        assert_matches!((probe.try_as_admin(), answer.try_as_admin()), (Some(AdminBase::TestRequest(t)), Some(AdminBase::Heartbeat(h))) if h.test_req_id.as_deref() == Some(t.test_req_id.as_ref()));
        let logon = ini.read().await;
        assert_eq!(logon.msg_seq_num(), 1); assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
        acc.send(&logon).await;
        let refusal = acc.read().await;
        assert_eq!(refusal.msg_seq_num(), 3);
        assert_matches!(refusal.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Peer rejected reset")));
        ini.send(&refusal).await;
        let logout_ack = ini.read().await;
        assert_eq!(logout_ack.msg_seq_num(), 2); assert_matches!(logout_ack.try_as_admin(), Some(AdminBase::Logout(_)));
        expect_admin(&mut pair.acc_events, MsgTypeBase::TestRequest).await;
        expect_admin(&mut pair.acc_events, MsgTypeBase::Logon).await;
        assert_matches!(recv_event(&mut pair.acc_events).await, TestEvent::SessionEnd(_, DisconnectReason::ApplicationForcedDisconnect));
        expect_admin(&mut pair.ini_events, MsgTypeBase::Heartbeat).await;
        let callback = expect_admin(&mut pair.ini_events, MsgTypeBase::Logout).await;
        assert_eq!(serialize_message(&callback), serialize_message(&refusal));
        assert!(!ini_task.is_finished());
        ini.close_input().await;
        assert_matches!(recv_event(&mut pair.ini_events).await, TestEvent::SessionEnd(_, DisconnectReason::SeqNumResetFailed));
        acc_task.await.unwrap(); ini_task.await.unwrap();
        acc.assert_closed_without_output().await; ini.assert_closed_without_output().await;
        assert_snapshot_counts(&pair.ini_storage, 3, 2); assert_snapshot_counts(&pair.acc_storage, 4, 3);
        let state = pair.ini_storage.borrow(); assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[&NonZeroSeqNum::new(1).unwrap()], serialize_message(&logon));
        assert_eq!(state.messages[&NonZeroSeqNum::new(2).unwrap()], serialize_message(&logout_ack));
        assert!(pair.acc_events.try_recv().is_err()); assert!(pair.ini_events.try_recv().is_err());
    }).await;
}

/// One intentional collision interleaving: the first reset request is held
/// while the other engine sends its own probe, and its ACK is written before
/// the failed requester's close is delivered. Other IO ordering can make
/// the answering engine observe IoError instead of RemoteRequestedLogout.
#[tokio::test(start_paused = true)]
async fn an_initiator_collision_can_end_with_an_unconfirmed_reset_failure() {
    LocalSet::new().run_until(async {
        let mut pair = reset_pair();
        let (acc_task, ini_task, mut acc, mut ini) = connect_relay_pair(&mut pair, 1).await;
        pair.acceptor.request_running_session_reset(&acceptor_session_id()).await.unwrap();
        let probe_b = acc.read().await; ini.send(&probe_b).await;
        let answer = ini.read().await; acc.send(&answer).await;
        assert_matches!((probe_b.try_as_admin(), answer.try_as_admin()), (Some(AdminBase::TestRequest(t)), Some(AdminBase::Heartbeat(h))) if h.test_req_id.as_deref() == Some(t.test_req_id.as_ref()));
        let logon_b = acc.read().await;
        assert_eq!(logon_b.msg_seq_num(), 1); assert_matches!(logon_b.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
        assert_snapshot_counts(&pair.acc_storage, 2, 1); assert_snapshot_counts(&pair.ini_storage, 3, 3);
        pair.initiator.request_running_session_reset().await.unwrap();
        let probe_a = ini.read().await;
        assert_eq!(probe_a.msg_seq_num(), 3); assert_matches!(probe_a.try_as_admin(), Some(AdminBase::TestRequest(_)));
        assert_snapshot_counts(&pair.ini_storage, 4, 3);
        acc.send(&probe_a).await;
        let logout_b = acc.read().await;
        assert_eq!(logout_b.msg_seq_num(), 2);
        assert_matches!(logout_b.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Unexpected TestRequest(1) during sequence number reset")));
        ini.send(&logon_b).await;
        let ack_a = ini.read().await;
        assert_eq!(ack_a.msg_seq_num(), 1); assert_matches!(ack_a.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
        assert_eq!(ack_a.sender_comp_id(), logon_b.target_comp_id()); assert_eq!(ack_a.target_comp_id(), logon_b.sender_comp_id());
        assert_snapshot_counts(&pair.ini_storage, 2, 2);
        ini.send(&logout_b).await;
        let logout_ack_a = ini.read().await;
        assert_eq!(logout_ack_a.msg_seq_num(), 2); assert_matches!(logout_ack_a.try_as_admin(), Some(AdminBase::Logout(_)));
        expect_admin(&mut pair.ini_events, MsgTypeBase::TestRequest).await;
        let ack_callback = expect_admin(&mut pair.ini_events, MsgTypeBase::Logon).await;
        assert_eq!(serialize_message(&ack_callback), serialize_message(&logon_b));
        expect_admin(&mut pair.ini_events, MsgTypeBase::Logout).await;
        expect_admin(&mut pair.acc_events, MsgTypeBase::Heartbeat).await;
        assert_matches!(recv_event(&mut pair.acc_events).await, TestEvent::SessionEnd(_, DisconnectReason::SeqNumResetFailed));
        assert!(!ini_task.is_finished()); ini.close_input().await;
        assert_matches!(recv_event(&mut pair.ini_events).await, TestEvent::SessionEnd(_, DisconnectReason::RemoteRequestedLogout));
        acc_task.await.unwrap(); ini_task.await.unwrap();
        acc.assert_closed_without_output().await; ini.assert_closed_without_output().await;
        assert_snapshot_counts(&pair.acc_storage, 3, 1); assert_snapshot_counts(&pair.ini_storage, 3, 3);
        for (snapshot, first, second) in [(&pair.acc_storage, &logon_b, &logout_b), (&pair.ini_storage, &ack_a, &logout_ack_a)] {
            let state = snapshot.borrow(); assert_eq!(state.messages.len(), 2);
            assert_eq!(state.messages[&NonZeroSeqNum::new(1).unwrap()], serialize_message(first));
            assert_eq!(state.messages[&NonZeroSeqNum::new(2).unwrap()], serialize_message(second));
        }
        assert!(pair.acc_events.try_recv().is_err()); assert!(pair.ini_events.try_recv().is_err());
    }).await;
}

#[tokio::test(start_paused = true)]
async fn in_session_reset_converges_both_engines() {
    LocalSet::new().run_until(async {
        let mut pair = reset_pair();
        let (acc_wire, ini_wire) = io::duplex(65536);
        let (acc_r, acc_w) = split(acc_wire); let (ini_r, ini_w) = split(ini_wire);
        let acc_task = pair.acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
        let ini_task = pair.initiator.run_session(ini_r, ini_w, None).unwrap();
        let TestEvent::SessionReady(_, ini_sender) = recv_event(&mut pair.ini_events).await else { panic!("expected sender"); };
        expect_admin(&mut pair.acc_events, MsgTypeBase::Logon).await;
        let TestEvent::SessionReady(_, acc_sender) = recv_event(&mut pair.acc_events).await else { panic!("expected sender"); };
        expect_admin(&mut pair.ini_events, MsgTypeBase::Logon).await;
        // The initiator sender remains the same across every running reset.
        for from_acceptor in [true, false] {
            if from_acceptor { pair.acceptor.request_running_session_reset(&acceptor_session_id()).await.unwrap(); }
            else { pair.initiator.request_running_session_reset().await.unwrap(); }
            let (requester_events, peer_events) = if from_acceptor { (&mut pair.acc_events, &mut pair.ini_events) } else { (&mut pair.ini_events, &mut pair.acc_events) };
            let probe = expect_admin(peer_events, MsgTypeBase::TestRequest).await;
            let answer = expect_admin(requester_events, MsgTypeBase::Heartbeat).await;
            assert_matches!((probe.try_as_admin(), answer.try_as_admin()), (Some(AdminBase::TestRequest(t)), Some(AdminBase::Heartbeat(h))) if h.test_req_id.as_deref() == Some(t.test_req_id.as_ref()));
            let request = expect_admin(peer_events, MsgTypeBase::Logon).await;
            let ack = expect_admin(requester_events, MsgTypeBase::Logon).await;
            for msg in [&request, &ack] {
                assert_eq!(msg.msg_seq_num(), 1);
                assert_matches!(msg.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true) && l.heart_bt_int == 0);
            }
            assert_eq!(request.sender_comp_id(), ack.target_comp_id()); assert_eq!(request.target_comp_id(), ack.sender_comp_id());
            yield_now().await;
            assert_snapshot_counts(&pair.acc_storage, 2, 2); assert_snapshot_counts(&pair.ini_storage, 2, 2);
            acc_sender.send(new_order_single_with_empty_header()).unwrap(); ini_sender.send(new_order_single_with_empty_header()).unwrap();
            for (events, sid) in [(&mut pair.acc_events, initiator_session_id()), (&mut pair.ini_events, acceptor_session_id())] {
                let TestEvent::AppMsgIn(_, msg) = recv_event(events).await else { panic!("expected order after reset"); };
                assert_eq!(msg.msg_seq_num(), 2); assert!(is_new_order_single(&msg));
                assert_eq!(msg.sender_comp_id(), sid.sender_comp_id()); assert_eq!(msg.target_comp_id(), sid.target_comp_id());
            }
            assert_snapshot_counts(&pair.acc_storage, 3, 3); assert_snapshot_counts(&pair.ini_storage, 3, 3);
        }
        pair.initiator.disconnect().await.unwrap();
        assert_matches!(recv_event(&mut pair.ini_events).await, TestEvent::SessionEnd(_, DisconnectReason::Disconnected));
        assert_matches!(recv_event(&mut pair.acc_events).await, TestEvent::SessionEnd(_, DisconnectReason::Disconnected));
        ini_task.await.unwrap(); acc_task.await.unwrap();
        for snapshot in [&pair.acc_storage, &pair.ini_storage] {
            let state = snapshot.borrow(); assert_eq!(state.messages.len(), 2);
            let order = Message::from_bytes(&state.messages[&NonZeroSeqNum::new(2).unwrap()]).unwrap(); assert!(is_new_order_single(&order));
        }
        assert!(pair.acc_events.try_recv().is_err()); assert!(pair.ini_events.try_recv().is_err());
    }).await;
}

async fn echo_reset_probe(peer: &mut RawPeerSession, seq: SeqNum) {
    let probe = peer.read().await;
    let Some(AdminBase::TestRequest(request)) = probe.try_as_admin() else {
        panic!("expected reset probe, got {probe:?}");
    };
    let response = Message::from_admin(
        peer_header(&peer.peer_sid, seq),
        AdminBase::Heartbeat(HeartbeatBase {
            test_req_id: Some(Cow::Owned(request.test_req_id.into_owned())),
        }),
    );
    peer.write(&serialize_message(&response)).await;
}

/// A timer Heartbeat sent before the peer reads the reset Logon is legal
/// ordinary traffic (Session Test Cases 4(a)). Its arrival in the reset
/// window is a timing race; it does not imply a peer protocol violation.
#[tokio::test(start_paused = true)]
async fn unexpected_application_or_peer_timer_heartbeat_ends_the_reset_window() {
    LocalSet::new().run_until(async {
        for heartbeat in [false, true] {
            let (events_tx, mut events) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory { events_tx });
            let sid = acceptor_session_id();
            register_in_memory_session(&acceptor, sid.clone(), reset_settings());
            let mut peer = connect_raw_peer(&acceptor, sid.clone(), 65536);
            peer.logon(1, 0).await;
            assert_matches!(recv_event(&mut events).await, TestEvent::AdminMsgIn(_, m) if matches!(m.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag.is_none()));
            assert_matches!(recv_event(&mut events).await, TestEvent::SessionReady(..));
            acceptor.request_running_session_reset(&sid).await.unwrap();
            echo_reset_probe(&mut peer, 2).await;
            let unexpected = if heartbeat {
                Message::from_admin(peer_header(&peer.peer_sid, 501), AdminBase::Heartbeat(HeartbeatBase { test_req_id: None }))
            } else {
                let mut order = new_order_single_with_empty_header();
                order.header = peer_header(&peer.peer_sid, 501).into(); *order
            };
            peer.write(&serialize_message(&unexpected)).await;
            let logon = peer.read().await;
            assert_eq!(logon.msg_seq_num(), 1);
            assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
            let logout = peer.read().await;
            assert_eq!(logout.msg_seq_num(), 2);
            let expected = if heartbeat { fix_str!("Unexpected Heartbeat(0) during sequence number reset") } else { fix_str!("Unexpected MsgType(D) during sequence number reset") };
            assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(expected));
            assert_matches!(recv_event(&mut events).await, TestEvent::AdminMsgIn(_, m) if matches!(m.try_as_admin(), Some(AdminBase::Heartbeat(h)) if h.test_req_id.is_some()));
            assert_matches!(recv_event(&mut events).await, TestEvent::SessionEnd(_, DisconnectReason::SeqNumResetFailed));
            assert!(peer.read_to_close().await.is_empty()); peer.handle.await.unwrap();
            let mut storage = acceptor.remove_session(&sid).unwrap();
            assert_eq!(storage.next_sender_msg_seq_num().get(), 3); assert_eq!(storage.next_target_msg_seq_num().get(), 1);
            assert_eq!(storage.fetch(NonZeroSeqNum::new(1).unwrap(), NonZeroSeqNum::new(1).unwrap()).await.unwrap(), serialize_message(&logon).as_slice());
            assert_eq!(storage.fetch(NonZeroSeqNum::new(2).unwrap(), NonZeroSeqNum::new(2).unwrap()).await.unwrap(), serialize_message(&logout).as_slice());
            assert!(events.try_recv().is_err());
        }
    }).await;
}

#[tokio::test(start_paused = true)]
async fn acceptor_disconnect_shutdown_stores_sends_held_by_an_unconfirmed_reset() {
    LocalSet::new().run_until(async {
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory { events_tx });
        let sid = acceptor_session_id();
        register_in_memory_session(&acceptor, sid.clone(), reset_settings());
        let mut peer = connect_raw_peer(&acceptor, sid.clone(), 65536);
        peer.logon(1, 0).await;
        assert_matches!(recv_event(&mut events).await, TestEvent::AdminMsgIn(_, m) if matches!(m.try_as_admin(), Some(AdminBase::Logon(_))));
        let TestEvent::SessionReady(_, sender) = recv_event(&mut events).await else { panic!("expected sender"); };
        acceptor.request_running_session_reset(&sid).await.unwrap();
        echo_reset_probe(&mut peer, 2).await;
        let logon = peer.read().await;
        assert_eq!(logon.msg_seq_num(), 1);
        assert_matches!(logon.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag == Some(true));
        assert_matches!(recv_event(&mut events).await, TestEvent::AdminMsgIn(_, m) if matches!(m.try_as_admin(), Some(AdminBase::Heartbeat(_))));
        sender.send(new_order_single_with_empty_header()).unwrap();
        acceptor.shutdown(ShutdownMode::Disconnect).await;
        assert_matches!(recv_event(&mut events).await, TestEvent::SessionEnd(_, DisconnectReason::SeqNumResetFailed));
        assert!(peer.read_to_close().await.is_empty()); peer.handle.await.unwrap();
        let mut storage = acceptor.remove_session(&sid).unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 3); assert_eq!(storage.next_target_msg_seq_num().get(), 1);
        assert_eq!(storage.fetch(NonZeroSeqNum::new(1).unwrap(), NonZeroSeqNum::new(1).unwrap()).await.unwrap(), serialize_message(&logon).as_slice());
        let order = Message::from_bytes(storage.fetch(NonZeroSeqNum::new(2).unwrap(), NonZeroSeqNum::new(2).unwrap()).await.unwrap()).unwrap();
        assert_eq!(order.msg_seq_num(), 2); assert!(is_new_order_single(&order));
        assert_eq!(order.sender_comp_id(), sid.sender_comp_id()); assert_eq!(order.target_comp_id(), sid.target_comp_id());
        assert!(storage.fetch(NonZeroSeqNum::new(3).unwrap(), NonZeroSeqNum::new(3).unwrap()).await.is_err());
        assert!(events.try_recv().is_err());
    }).await;
}

#[tokio::test(start_paused = true)]
async fn a_peer_without_matching_probe_ids_cannot_finish_a_reset() {
    LocalSet::new().run_until(async {
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory { events_tx });
        let sid = acceptor_session_id(); let mut settings = reset_settings();
        settings.verify_test_request_id = false; settings.running_session_reset_timeout = Duration::from_secs(30);
        register_in_memory_session(&acceptor, sid.clone(), settings);
        let mut peer = connect_raw_peer(&acceptor, sid.clone(), 65536);
        peer.logon(1, 0).await;
        assert_matches!(recv_event(&mut events).await, TestEvent::AdminMsgIn(_, m) if matches!(m.try_as_admin(), Some(AdminBase::Logon(_))));
        assert_matches!(recv_event(&mut events).await, TestEvent::SessionReady(..));
        acceptor.request_running_session_reset(&sid).await.unwrap();
        let probe = peer.read().await;
        assert_eq!(probe.msg_seq_num(), 2);
        assert_matches!(probe.try_as_admin(), Some(AdminBase::TestRequest(r)) if r.test_req_id.as_ref() != fix_str!("WRONG"));
        for (seq, id) in [(2, None), (3, Some(fix_str!("WRONG")))] {
            peer.write(&common::peer_heartbeat_bytes(&peer.peer_sid, seq, id)).await;
            assert_matches!(recv_event(&mut events).await, TestEvent::AdminMsgIn(_, m) if matches!(m.try_as_admin(), Some(AdminBase::Heartbeat(h)) if h.test_req_id.as_deref() == id));
            assert!(try_read_one_message(&mut peer.peer_r, &mut peer.peer_buf).now_or_never().is_none());
        }
        time::advance(Duration::from_secs(30)).await;
        let logout = peer.read().await;
        assert_eq!(logout.msg_seq_num(), 3);
        assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(l)) if l.text.as_deref() == Some(fix_str!("Sequence number reset not acknowledged")));
        assert_matches!(recv_event(&mut events).await, TestEvent::SessionEnd(_, DisconnectReason::ResetPreparationTimeout));
        assert!(peer.read_to_close().await.is_empty()); peer.handle.await.unwrap();
        let mut storage = acceptor.remove_session(&sid).unwrap();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 4); assert_eq!(storage.next_target_msg_seq_num().get(), 4);
        let original = Message::from_bytes(storage.fetch(NonZeroSeqNum::new(1).unwrap(), NonZeroSeqNum::new(1).unwrap()).await.unwrap()).unwrap();
        assert_matches!(original.try_as_admin(), Some(AdminBase::Logon(l)) if l.reset_seq_num_flag.is_none());
        assert_eq!(storage.fetch(NonZeroSeqNum::new(2).unwrap(), NonZeroSeqNum::new(2).unwrap()).await.unwrap(), serialize_message(&probe).as_slice());
        assert!(events.try_recv().is_err());
    }).await;
}

/// Acceptor-side session id with configurable TargetCompID. Our comp is
/// always "SENDER".
fn acceptor_session_id_for(target: &'static FixStr) -> SessionId {
    SessionId::new(
        Version::FIXT11,
        fix_str!("SENDER").to_owned(),
        target.to_owned(),
    )
}

/// Initiator-side session id with configurable own SenderCompID, mirrored
/// to the acceptor side (target = "SENDER").
fn initiator_session_id_for(own_sender: &'static FixStr) -> SessionId {
    SessionId::new(
        Version::FIXT11,
        own_sender.to_owned(),
        fix_str!("SENDER").to_owned(),
    )
}

/// Default acceptor-side session id (TargetCompID = "TARGET"). Used by
/// single-session tests.
fn acceptor_session_id() -> SessionId {
    acceptor_session_id_for(fix_str!("TARGET"))
}

/// Default initiator-side session id (SenderCompID = "TARGET").
fn initiator_session_id() -> SessionId {
    initiator_session_id_for(fix_str!("TARGET"))
}

fn is_new_order_single(msg: &Message) -> bool {
    matches!(&*msg.body, Body::NewOrderSingle(_))
}

// ---------------------------------------------------------------------------
// Test application / factory
// ---------------------------------------------------------------------------

enum TestEvent {
    SessionReady(SessionId, Sender<Message>),
    SessionEnd(SessionId, DisconnectReason),
    AppMsgIn(SessionId, Box<Message>),
    /// The admin message as the application saw it, so a test can assert on
    /// its fields (a Reject's reason, a Logout's Text) and not just its type.
    AdminMsgIn(SessionId, Box<Message>),
}

/// Whether `event` is an inbound admin message of type `msg_type`.
fn is_admin_msg_in(event: &TestEvent, msg_type: MsgTypeBase) -> bool {
    matches!(event, TestEvent::AdminMsgIn(_, msg) if SessionMessage::msg_type(msg.as_ref()) == msg_type)
}

// `Sender<M>` intentionally does not implement `Debug`, so we hand-roll a
// `Debug` impl that only references fields that do.
impl fmt::Debug for TestEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestEvent::SessionReady(id, _) => f.debug_tuple("SessionReady").field(id).finish(),
            TestEvent::SessionEnd(id, reason) => {
                f.debug_tuple("SessionEnd").field(id).field(reason).finish()
            }
            TestEvent::AppMsgIn(id, msg) => f
                .debug_tuple("AppMsgIn")
                .field(id)
                .field(&SessionMessage::name(msg.as_ref()))
                .finish(),
            TestEvent::AdminMsgIn(id, msg) => f
                .debug_tuple("AdminMsgIn")
                .field(id)
                .field(&SessionMessage::name(msg.as_ref()))
                .finish(),
        }
    }
}

struct TestApp {
    session_id: SessionId,
    events_tx: mpsc::UnboundedSender<TestEvent>,
}

impl Application<Message> for TestApp {
    fn on_serialize_error(&mut self, _msg: Box<Message>, _error: &SerializeError) {}

    async fn on_session_ready(&mut self, session_id: &SessionId, sender: Sender<Message>) {
        let _ = self
            .events_tx
            .send(TestEvent::SessionReady(session_id.clone(), sender));
    }

    async fn on_session_end(&mut self, session_id: &SessionId, reason: DisconnectReason) {
        let _ = self
            .events_tx
            .send(TestEvent::SessionEnd(session_id.clone(), reason));
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        let _ = self
            .events_tx
            .send(TestEvent::AppMsgIn(self.session_id.clone(), msg));
        InputAction::Accept
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        let _ = self.events_tx.send(TestEvent::AdminMsgIn(
            self.session_id.clone(),
            Box::new(msg.clone()),
        ));
        InputAction::Accept
    }
}

#[derive(Clone)]
struct TestAppFactory {
    events_tx: mpsc::UnboundedSender<TestEvent>,
}

impl ApplicationFactory<Message> for TestAppFactory {
    type App = TestApp;

    fn create(&self, ctx: &SessionContext<'_>) -> TestApp {
        TestApp {
            session_id: ctx.session_id().clone(),
            events_tx: self.events_tx.clone(),
        }
    }
}

/// [`TestApp`] that panics in `on_app_msg_in` while `armed` is set - once,
/// since the panic disarms it. Arbitrary user code unwinding the session
/// task; the session that reconnects afterwards gets the same message again
/// on resend and must be able to take it.
struct PanicOnceApp {
    inner: TestApp,
    armed: Rc<Cell<bool>>,
}

impl Application<Message> for PanicOnceApp {
    fn on_serialize_error(&mut self, msg: Box<Message>, error: &SerializeError) {
        self.inner.on_serialize_error(msg, error);
    }

    async fn on_session_ready(&mut self, session_id: &SessionId, sender: Sender<Message>) {
        self.inner.on_session_ready(session_id, sender).await;
    }

    async fn on_session_end(&mut self, session_id: &SessionId, reason: DisconnectReason) {
        self.inner.on_session_end(session_id, reason).await;
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        if self.armed.replace(false) {
            panic!("injected application panic");
        }
        self.inner.on_app_msg_in(msg).await
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        self.inner.on_admin_msg_in(msg).await
    }
}

/// Factory for [`PanicOnceApp`]; `armed` is shared with the test so it can
/// arm the panic once the session is up.
#[derive(Clone)]
struct PanicOnceAppFactory {
    inner: TestAppFactory,
    armed: Rc<Cell<bool>>,
}

impl ApplicationFactory<Message> for PanicOnceAppFactory {
    type App = PanicOnceApp;

    fn create(&self, ctx: &SessionContext<'_>) -> PanicOnceApp {
        PanicOnceApp {
            inner: self.inner.create(ctx),
            armed: self.armed.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Event-drain helpers
// ---------------------------------------------------------------------------

async fn recv_event(rx: &mut mpsc::UnboundedReceiver<TestEvent>) -> TestEvent {
    timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("timeout waiting for TestEvent")
        .expect("events channel closed")
}

async fn wait_until<F>(rx: &mut mpsc::UnboundedReceiver<TestEvent>, mut predicate: F) -> TestEvent
where
    F: FnMut(&TestEvent) -> bool,
{
    loop {
        let event = recv_event(rx).await;
        if predicate(&event) {
            return event;
        }
    }
}

async fn wait_for_session_ready(rx: &mut mpsc::UnboundedReceiver<TestEvent>) -> Sender<Message> {
    match wait_until(rx, |e| matches!(e, TestEvent::SessionReady(..))).await {
        TestEvent::SessionReady(_, sender) => sender,
        _ => unreachable!(),
    }
}

async fn wait_for_session_end(rx: &mut mpsc::UnboundedReceiver<TestEvent>) -> DisconnectReason {
    match wait_until(rx, |e| matches!(e, TestEvent::SessionEnd(..))).await {
        TestEvent::SessionEnd(_, reason) => reason,
        _ => unreachable!(),
    }
}

/// Drain events until a `SessionReady` for the given session id arrives.
async fn wait_for_session_ready_of(
    rx: &mut mpsc::UnboundedReceiver<TestEvent>,
    expected: &SessionId,
) -> Sender<Message> {
    loop {
        let event = recv_event(rx).await;
        if let TestEvent::SessionReady(sid, sender) = event
            && sid == *expected
        {
            return sender;
        }
    }
}

/// Drain events until a `SessionEnd` for the given session id arrives.
async fn wait_for_session_end_of(
    rx: &mut mpsc::UnboundedReceiver<TestEvent>,
    expected: &SessionId,
) -> DisconnectReason {
    loop {
        let event = recv_event(rx).await;
        if let TestEvent::SessionEnd(sid, reason) = event
            && sid == *expected
        {
            return reason;
        }
    }
}

// ---------------------------------------------------------------------------
// Test 1: Logon -> message exchange -> logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logon_message_exchange_logout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Paired duplex streams: what server_stream writes, client_stream
            // reads, and vice versa.
            let (server_stream, client_stream) = io::duplex(8192);
            let (server_r, server_w) = split(server_stream);
            let (client_r, client_w) = split(client_stream);

            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let (ini_events_tx, mut ini_events_rx) = mpsc::unbounded_channel();

            // Build Acceptor and register a single session.
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            register_in_memory_session(
                &acceptor,
                acceptor_session_id(),
                build_session_settings(30),
            );

            // Build Initiator with the mirrored session id.
            let initiator = Initiator::<Message, InMemoryStorage, _>::new(
                initiator_session_id(),
                build_session_settings(30),
                TestAppFactory {
                    events_tx: ini_events_tx,
                },
                |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
            )
            .expect("initiator settings valid");

            // Spawn both session tasks.
            let acc_handle = acceptor.run_session(server_r, server_w, TEST_PEER_ADDR);
            let ini_handle = initiator
                .run_session(client_r, client_w, None)
                .expect("initiator run_session");

            // Both sides should hit SessionReady after the Logon handshake
            // completes.
            let ini_sender = wait_for_session_ready(&mut ini_events_rx).await;
            let _acc_sender = wait_for_session_ready(&mut acc_events_rx).await;

            // Initiator stages a NewOrderSingle. `send` is synchronous and may
            // be called from any context.
            ini_sender
                .send(new_order_single_with_empty_header())
                .expect("sender.send");

            // Acceptor should observe an inbound app message that is a
            // NewOrderSingle. Drain any admin events (e.g. Logon) first.
            let event =
                wait_until(&mut acc_events_rx, |e| matches!(e, TestEvent::AppMsgIn(..))).await;
            assert_matches!(event, TestEvent::AppMsgIn(_, msg) if is_new_order_single(&msg));

            // Initiator-originated graceful logout.
            initiator.logout(None, None).await.expect("logout");

            // Both sides should fire SessionEnd. The initiator requested the
            // logout; the acceptor observes it as a remote request.
            let ini_reason = wait_for_session_end(&mut ini_events_rx).await;
            let acc_reason = wait_for_session_end(&mut acc_events_rx).await;

            assert_eq!(ini_reason, DisconnectReason::LocalRequestedLogout);
            assert_eq!(acc_reason, DisconnectReason::RemoteRequestedLogout);

            // Both tasks should complete cleanly (no panic).
            acc_handle.await.expect("acceptor task");
            ini_handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 1b: Rejected first Logon must receive a Logout before disconnect
// ---------------------------------------------------------------------------

/// Regression: the acceptor's pre-loop processes the first (Logon)
/// message, and a rejection handler stages a `Logout(35=5)`/`Reject` into
/// `admin_output` and sets `should_disconnect`. The pre-loop used to early-return
/// on `should_disconnect` *before* draining and flushing, so the staged Logout
/// was silently dropped and the peer saw a bare TCP close. Scenario 1S(d)
/// mandates the Logout(35=5) with Text(58) be sent, then disconnect
/// (FIX Session Layer §4.3.10).
#[tokio::test]
async fn acceptor_rejected_first_logon_sends_logout_before_disconnect() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            let mut settings = build_session_settings(30);
            settings.accept_reset_on_connect = false;
            acceptor.register_session(acceptor_session_id(), settings, |_, max| {
                let mut storage = InMemoryStorage::new(max);
                storage.set_next_sender_msg_seq_num(40.try_into().unwrap()).unwrap();
                storage.set_next_target_msg_seq_num(40.try_into().unwrap()).unwrap();
                Ok(storage)
            }).unwrap();
            let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 8192);

            // The acceptor refuses a reset using its persisted numbering.
            let invalid_logon = peer_logon_reset_bytes(&peer.peer_sid, 1, 30);
            peer.write(&invalid_logon).await;

            // The peer must receive the mandated Logout, not a bare TCP close.
            let msg = timeout(
                TEST_TIMEOUT,
                try_read_one_message(&mut peer.peer_r, &mut peer.peer_buf),
            )
            .await
            .expect("timed out waiting for Logout")
            .expect("acceptor closed the connection without sending Logout");
            assert_eq!(
                SessionMessage::msg_type(&*msg),
                MsgTypeBase::Logout,
                "rejected first Logon must be answered with a Logout(35=5)"
            );
            let AdminBase::Logout(ref logout) =
                SessionMessage::try_as_admin(&*msg).expect("admin message")
            else {
                panic!("expected Logout");
            };
            assert_eq!(msg.header.msg_seq_num, 40);
            assert_eq!(logout.text.as_deref(), Some(fix_str!("Resetting the sequence number upon FIX connection establishment is not supported")));

            // Then the session ends with the logon-rejection reason.
            let acc_reason = wait_for_session_end(&mut acc_events_rx).await;
            assert_eq!(acc_reason, DisconnectReason::InvalidLogonState);
            peer.handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Raw-peer helpers (used by tests that drive one side with raw FIX bytes)
// ---------------------------------------------------------------------------

/// A Logon carrying `ResetSeqNumFlag(141)=Y`.
fn peer_logon_reset_bytes(peer_sid: &SessionId, seq: SeqNum, heart_bt_int: Int) -> Vec<u8> {
    serialize_message(&Message::from_admin(
        peer_header(peer_sid, seq),
        AdminBase::Logon(LogonBase {
            encrypt_method: EncryptMethodBase::None,
            encrypt_method_raw: 0,
            heart_bt_int,
            reset_seq_num_flag: Some(true),
            max_message_size: None,
            next_expected_msg_seq_num: None,
            default_appl_ver_id: Some(ApplVerId::Fix50Sp2),
            session_status: None,
        }),
    ))
}

fn peer_resend_request_bytes(
    peer_sid: &SessionId,
    seq: SeqNum,
    begin_seq_no: SeqNum,
    end_seq_no: SeqNum,
) -> Vec<u8> {
    serialize_message(&Message::from_admin(
        peer_header(peer_sid, seq),
        AdminBase::ResendRequest(ResendRequestBase {
            begin_seq_no,
            end_seq_no,
        }),
    ))
}

fn peer_sequence_reset_bytes(
    peer_sid: &SessionId,
    seq: SeqNum,
    new_seq_no: SeqNum,
    gap_fill_flag: Option<bool>,
) -> Vec<u8> {
    serialize_message(&Message::from_admin(
        peer_header(peer_sid, seq),
        AdminBase::SequenceReset(SequenceResetBase {
            gap_fill_flag,
            new_seq_no,
        }),
    ))
}

/// Everything a raw-peer initiator test interacts with: the initiator and its
/// running session task, the test-event stream, and the peer's end of the
/// duplex pipe (`peer_sid` is the initiator's session id reversed).
struct RawPeerInitiator {
    initiator: Initiator<Message, InMemoryStorage, TestAppFactory>,
    handle: JoinHandle<()>,
    events_rx: mpsc::UnboundedReceiver<TestEvent>,
    peer_r: ReadHalf<DuplexStream>,
    peer_w: WriteHalf<DuplexStream>,
    peer_sid: SessionId,
    peer_buf: Vec<u8>,
}

/// Spawn an initiator session over a duplex pipe of `pipe_size` bytes,
/// leaving the peer side raw (driven with the `peer_*_bytes` builders).
fn spawn_initiator_with_raw_peer(pipe_size: usize, settings: SessionSettings) -> RawPeerInitiator {
    let (ini_stream, peer_stream) = io::duplex(pipe_size);
    let (ini_r, ini_w) = split(ini_stream);
    let (peer_r, peer_w) = split(peer_stream);
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let initiator = Initiator::<Message, InMemoryStorage, _>::new(
        initiator_session_id(),
        settings,
        TestAppFactory { events_tx },
        |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
    )
    .expect("initiator settings valid");
    let handle = initiator
        .run_session(ini_r, ini_w, None)
        .expect("initiator run_session");
    RawPeerInitiator {
        initiator,
        handle,
        events_rx,
        peer_r,
        peer_w,
        peer_sid: initiator_session_id().reverse_route(),
        peer_buf: Vec::new(),
    }
}

impl RawPeerInitiator {
    /// Drive the peer side of the logon handshake: read the initiator's
    /// Logon request (seq 1), answer it with a peer Logon carrying
    /// `heart_bt_int`, and wait for the initiator's `SessionReady`.
    async fn peer_logon_handshake(&mut self, heart_bt_int: Int) -> Sender<Message> {
        let logon_req = read_one_message(&mut self.peer_r, &mut self.peer_buf).await;
        assert_eq!(SessionMessage::msg_type(&*logon_req), MsgTypeBase::Logon);
        assert_eq!(logon_req.msg_seq_num(), 1);
        self.peer_w
            .write_all(&peer_logon_bytes(&self.peer_sid, 1, heart_bt_int))
            .await
            .expect("write peer logon response");
        wait_for_session_ready(&mut self.events_rx).await
    }
}

#[tokio::test]
async fn poss_resend_passes_through_send_and_application_callback() {
    LocalSet::new()
        .run_until(async {
            let mut peer = spawn_initiator_with_raw_peer(8192, build_session_settings(30));
            let sender = peer.peer_logon_handshake(30).await;
            let order = new_order_single_with_empty_header();

            // Section 4.9: application retries consume fresh sequence numbers.
            // Scenario 19: only the application decides whether the repeated
            // business ID is a duplicate. The session delivers every retry.
            for (seq, poss_resend) in (2..).zip([None, Some(false), Some(true), Some(true)]) {
                let mut outgoing = order.clone();
                outgoing.header.poss_resend = poss_resend;
                sender.send(outgoing).unwrap();

                let sent = read_one_message(&mut peer.peer_r, &mut peer.peer_buf).await;
                assert_matches!(
                    &*sent.body,
                    Body::NewOrderSingle(order) if order.cl_ord_id == fix_str!("ORD001")
                );
                assert_eq!(sent.msg_seq_num(), seq);
                assert_eq!(sent.header.poss_resend, poss_resend);
                assert_eq!(sent.poss_dup_flag(), None);
                assert_eq!(sent.orig_sending_time(), None);

                let mut incoming = order.clone();
                incoming.header = peer_header(&peer.peer_sid, seq).into();
                incoming.header.poss_resend = poss_resend;
                peer.peer_w
                    .write_all(&serialize_message(&incoming))
                    .await
                    .unwrap();

                let event = wait_until(&mut peer.events_rx, |event| {
                    matches!(event, TestEvent::AppMsgIn(..))
                })
                .await;
                let TestEvent::AppMsgIn(_, received) = event else {
                    unreachable!();
                };
                assert_matches!(
                    &*received.body,
                    Body::NewOrderSingle(order) if order.cl_ord_id == fix_str!("ORD001")
                );
                assert_eq!(received.msg_seq_num(), seq);
                assert_eq!(received.header.poss_resend, poss_resend);
            }

            peer.initiator.disconnect().await.unwrap();
            timeout(TEST_TIMEOUT, peer.handle).await.unwrap().unwrap();
        })
        .await;
}

/// Register `sid` on `acceptor` under `settings`, backed by an
/// `InMemoryStorage` sized by the acceptor.
fn register_in_memory_session<A: ApplicationFactory<Message> + 'static>(
    acceptor: &Acceptor<Message, InMemoryStorage, A>,
    sid: SessionId,
    settings: SessionSettings,
) {
    acceptor
        .register_session(sid, settings, |_id, max_message_size| {
            Ok(InMemoryStorage::new(max_message_size))
        })
        .expect("register session");
}

/// One acceptor-side session driven by a raw peer: the session id on both
/// sides, the peer's end of the pipe with its read buffer, and the session
/// task's handle.
struct RawPeerSession {
    sid: SessionId,
    peer_sid: SessionId,
    peer_r: ReadHalf<DuplexStream>,
    peer_w: WriteHalf<DuplexStream>,
    peer_buf: Vec<u8>,
    handle: JoinHandle<()>,
}

/// Open a duplex pipe of `pipe_size` bytes to `acceptor`, spawn the session
/// task for it and hand back the peer's end. Nothing has been exchanged yet;
/// `sid` must already be registered.
fn connect_raw_peer<A: ApplicationFactory<Message> + 'static>(
    acceptor: &Acceptor<Message, InMemoryStorage, A>,
    sid: SessionId,
    pipe_size: usize,
) -> RawPeerSession {
    let (acc_stream, peer_stream) = io::duplex(pipe_size);
    let (acc_r, acc_w) = split(acc_stream);
    let (peer_r, peer_w) = split(peer_stream);
    let handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
    RawPeerSession {
        peer_sid: sid.clone().reverse_route(),
        sid,
        peer_r,
        peer_w,
        peer_buf: Vec::new(),
        handle,
    }
}

impl RawPeerSession {
    /// Write raw bytes to the acceptor.
    async fn write(&mut self, bytes: &[u8]) {
        self.peer_w
            .write_all(bytes)
            .await
            .expect("write to the acceptor");
    }

    /// Log on with `seq` and `heart_bt_int` and return the acceptor's Logon
    /// acknowledgement for the test to inspect.
    async fn logon(&mut self, seq: SeqNum, heart_bt_int: Int) -> Box<Message> {
        let logon = peer_logon_bytes(&self.peer_sid, seq, heart_bt_int);
        self.write(&logon).await;
        let ack = self.read().await;
        assert_eq!(SessionMessage::msg_type(&*ack), MsgTypeBase::Logon);
        ack
    }

    /// Read the next message the acceptor sent this peer.
    async fn read(&mut self) -> Box<Message> {
        timeout(
            TEST_TIMEOUT,
            read_one_message(&mut self.peer_r, &mut self.peer_buf),
        )
        .await
        .expect("timeout waiting for a message from the acceptor")
    }

    /// Everything the acceptor still writes before closing the connection -
    /// empty when it closes without another word.
    async fn read_to_close(&mut self) -> Vec<u8> {
        let mut rest = mem::take(&mut self.peer_buf);
        timeout(TEST_TIMEOUT, self.peer_r.read_to_end(&mut rest))
            .await
            .expect("the acceptor must close the connection")
            .expect("read");
        rest
    }
}

/// Register `target` under the default settings, connect a raw peer to it and
/// drive the logon handshake through to the acceptor's `SessionReady`.
async fn spawn_raw_peer_session(
    acceptor: &Acceptor<Message, InMemoryStorage, TestAppFactory>,
    acc_events_rx: &mut mpsc::UnboundedReceiver<TestEvent>,
    target: &'static FixStr,
) -> RawPeerSession {
    let sid = acceptor_session_id_for(target);
    register_in_memory_session(acceptor, sid.clone(), build_session_settings(30));
    let mut peer = connect_raw_peer(acceptor, sid, 8192);
    peer.logon(1, 30).await;
    let _ = wait_for_session_ready_of(acc_events_rx, &peer.sid).await;
    peer
}

/// One initiator connection to a live acceptor: both session tasks and the
/// initiator's event stream. The `Initiator` handle itself is not kept - the
/// session task owns everything it needs.
struct InitiatorConnection {
    acc_handle: JoinHandle<()>,
    ini_handle: JoinHandle<()>,
    events_rx: mpsc::UnboundedReceiver<TestEvent>,
}

/// Connect a fresh initiator under the default settings to `acceptor` over
/// a duplex pipe and spawn both session tasks.
fn connect_initiator<A: ApplicationFactory<Message> + 'static>(
    acceptor: &Acceptor<Message, InMemoryStorage, A>,
) -> InitiatorConnection {
    let (acc_stream, ini_stream) = io::duplex(8192);
    let (acc_r, acc_w) = split(acc_stream);
    let (ini_r, ini_w) = split(ini_stream);
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let initiator = Initiator::<Message, InMemoryStorage, _>::new(
        initiator_session_id(),
        build_session_settings(30),
        TestAppFactory { events_tx },
        |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
    )
    .expect("initiator settings valid");
    let acc_handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
    let ini_handle = initiator
        .run_session(ini_r, ini_w, None)
        .expect("initiator run_session");
    InitiatorConnection {
        acc_handle,
        ini_handle,
        events_rx,
    }
}

// ---------------------------------------------------------------------------
// Test 2: Inbound ResendRequest covering a large stored range
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resend_request_large_gap() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Generous buffer: 25 stored messages + 25 resends must fit without
            // ever blocking the writer.
            let mut p = spawn_initiator_with_raw_peer(65536, build_session_settings(30));
            let ini_sender = p.peer_logon_handshake(30).await;
            let RawPeerInitiator {
                initiator,
                handle: ini_handle,
                events_rx: mut ini_events_rx,
                mut peer_r,
                mut peer_w,
                peer_sid,
                mut peer_buf,
            } = p;

            // Initiator sends N NewOrderSingles (sender seq 2..=N+1).
            const N: SeqNum = 25;
            for _ in 0..N {
                ini_sender
                    .send(new_order_single_with_empty_header())
                    .expect("sender.send");
            }

            // Peer consumes all of them - first send, no PossDupFlag.
            for expected in 2..=(N + 1) {
                let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
                assert!(
                    is_new_order_single(&msg),
                    "seq {expected} should be a NewOrderSingle on first send"
                );
                assert_eq!(msg.msg_seq_num(), expected);
                assert_ne!(
                    msg.poss_dup_flag(),
                    Some(true),
                    "first-send message at seq {expected} must not have PossDupFlag=Y"
                );
            }

            // Peer asks for a resend covering the full stored range.
            peer_w
                .write_all(&peer_resend_request_bytes(&peer_sid, 2, 2, N + 1))
                .await
                .expect("write resend request");

            // Peer consumes the N resent messages. Each must carry
            // PossDupFlag=Y and preserve the original sequence number.
            for expected in 2..=(N + 1) {
                let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
                assert!(
                    is_new_order_single(&msg),
                    "resend at seq {expected} should be a NewOrderSingle, got {}",
                    SessionMessage::name(&*msg)
                );
                assert_eq!(msg.msg_seq_num(), expected);
                assert_eq!(
                    msg.poss_dup_flag(),
                    Some(true),
                    "resend message at seq {expected} must have PossDupFlag=Y"
                );
            }

            // Clean teardown. Disconnect is simpler than a full Logout echo
            // dance for the raw-peer case and doesn't require peer cooperation.
            initiator.disconnect().await.expect("disconnect");
            assert_eq!(
                wait_for_session_end(&mut ini_events_rx).await,
                DisconnectReason::Disconnected
            );
            ini_handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 2b: ResendRequest whose range ends on a gap-fill candidate
// ---------------------------------------------------------------------------

/// Regression: when the *last* sequence number of a resend range is a
/// gap-fill candidate (an admin message), `process_one_resend` accumulates it
/// and exhausts the range in the same call, returning `true`. Both exits of
/// the batch loop once left that trailing `SequenceReset`-GapFill orphaned in
/// engine state and never written to TCP - recovery deadlocked because the
/// peer's NextNumIn never advanced past the gap (FIX Session Layer §4.8.5;
/// Scenario 8):
///
/// - with the default `resend_batch_size = 1` the loop ends by count and
///   never takes the flush-on-`false` branch;
/// - with a batch larger than the range (3 against two slots) the third call
///   finds `active_resend` already cleared and breaks - and that branch used
///   to flush the gap itself, consuming `gap_fill_range` and thereby
///   disarming the guard on the block that is the only one to write it to
///   TCP.
///
/// The fix flushes any residual gap once the range is exhausted, on either
/// exit.
#[tokio::test]
async fn resend_range_ending_on_gap_fill_flushes_trailing_gap() {
    let local = LocalSet::new();
    local
        .run_until(async {
            for resend_batch_size in [1, 3] {
                let mut settings = build_session_settings(30);
                settings.resend_batch_size = NonZeroUsize::new(resend_batch_size).unwrap();
                let mut p = spawn_initiator_with_raw_peer(65536, settings);
                let ini_sender = p.peer_logon_handshake(30).await;
                let RawPeerInitiator {
                    initiator,
                    handle: ini_handle,
                    events_rx: mut ini_events_rx,
                    mut peer_r,
                    mut peer_w,
                    peer_sid,
                    mut peer_buf,
                } = p;

                // Initiator sends one app message (sender seq 2).
                ini_sender
                    .send(new_order_single_with_empty_header())
                    .expect("sender.send");
                let nos = read_one_message(&mut peer_r, &mut peer_buf).await;
                assert!(is_new_order_single(&nos));
                assert_eq!(nos.msg_seq_num(), 2);

                // Peer TestRequest (peer seq 2) -> initiator replies Heartbeat
                // (sender seq 3), an admin message that is a gap-fill candidate
                // and becomes the LAST slot of the resend range below.
                peer_w
                    .write_all(&peer_test_request_bytes(&peer_sid, 2, fix_str!("TR1")))
                    .await
                    .expect("write test request");
                let hb = read_one_message(&mut peer_r, &mut peer_buf).await;
                assert_eq!(SessionMessage::msg_type(&*hb), MsgTypeBase::Heartbeat);
                assert_eq!(hb.msg_seq_num(), 3);

                // Peer requests a resend of seq 2..=3 (peer seq 3). seq 2 is a
                // real app resend; seq 3 (the Heartbeat) is a gap-fill
                // candidate sitting at the range end - the trailing-gap-fill
                // case.
                peer_w
                    .write_all(&peer_resend_request_bytes(&peer_sid, 3, 2, 3))
                    .await
                    .expect("write resend request");

                // seq 2 comes back as a real NewOrderSingle resend
                // (PossDupFlag=Y).
                let resent = read_one_message(&mut peer_r, &mut peer_buf).await;
                assert!(
                    is_new_order_single(&resent),
                    "seq 2 resend must be a NewOrderSingle (batch {resend_batch_size})"
                );
                assert_eq!(resent.msg_seq_num(), 2);
                assert_eq!(resent.poss_dup_flag(), Some(true));

                // The trailing gap-fill for seq 3 must be flushed. Before the
                // fix this read deadlocks - the gap-fill is orphaned in engine
                // state and never written, so the peer waits forever.
                let gap = timeout(TEST_TIMEOUT, read_one_message(&mut peer_r, &mut peer_buf))
                    .await
                    .unwrap_or_else(|_| {
                        panic!(
                            "trailing SequenceReset-GapFill must be sent (batch \
                             {resend_batch_size})"
                        )
                    });
                assert_eq!(
                    SessionMessage::msg_type(&*gap),
                    MsgTypeBase::SequenceReset,
                    "trailing slot must be a SequenceReset-GapFill (batch {resend_batch_size})"
                );
                let AdminBase::SequenceReset(ref sr) =
                    SessionMessage::try_as_admin(&*gap).expect("admin message")
                else {
                    panic!("expected SequenceReset");
                };
                assert_eq!(sr.gap_fill_flag, Some(true), "must be a GapFill (123=Y)");
                assert_eq!(gap.msg_seq_num(), 3, "gap-fill begins at the skipped seq 3");
                assert_eq!(sr.new_seq_no, 4, "NewSeqNo advances past the gap");

                initiator.disconnect().await.expect("disconnect");
                assert_eq!(
                    wait_for_session_end(&mut ini_events_rx).await,
                    DisconnectReason::Disconnected
                );
                ini_handle.await.expect("initiator task");
            }
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 3: SequenceReset-Reset advances target seq num
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sequence_reset_advances_target_seq_num() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            register_in_memory_session(
                &acceptor,
                acceptor_session_id(),
                build_session_settings(30),
            );
            let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 8192);

            // Peer drives the logon handshake; the acceptor answers with
            // its Logon.
            peer.logon(1, 30).await;

            // Waiting for SessionReady also drains the AdminMsgIn(Logon)
            // that precedes it, so the next event the test waits on is
            // unambiguously caused by the reset + heartbeat sequence below.
            let _ = wait_for_session_ready(&mut acc_events_rx).await;

            // Peer sends SequenceReset-Reset: "reset your expected
            // incoming seq num to 10". gap_fill_flag=None -> Reset form.
            // The reset's own MsgSeqNum is 2 (next after Logon=1), but
            // SequenceReset-Reset is explicitly exempt from the
            // MsgSeqNum check in verify_header when gap_fill_flag=false.
            let reset = peer_sequence_reset_bytes(
                &peer.peer_sid,
                /* seq */ 2,
                /* new_seq_no */ 10,
                /* gap_fill_flag */ None,
            );
            peer.write(&reset).await;

            // Peer sends a TestRequest at the *new* expected seq. The
            // engine's `on_test_request` replies with an outbound
            // Heartbeat carrying the matching `TestReqID` - this is the
            // positive confirmation that the reset was honored.
            //
            // (Heartbeat / TestRequest are both `InputResult::Handled`
            // by the engine, so no `AdminMsgIn` event fires on the
            // acceptor side - we have to verify via the outbound
            // response read from the peer stream.)
            //
            // If the reset had been ignored, the engine would see
            // seq=10 as too-high (expected 2), queue the TestRequest,
            // and emit a `ResendRequest` instead of the Heartbeat
            // response. We assert on message type to distinguish the
            // two outcomes cleanly.
            let test_request = peer_test_request_bytes(&peer.peer_sid, 10, fix_str!("RESET1"));
            peer.write(&test_request).await;

            let response = peer.read().await;
            assert_eq!(
                SessionMessage::msg_type(&*response),
                MsgTypeBase::Heartbeat,
                "expected Heartbeat response after SequenceReset-Reset + TestRequest, \
                 got {} - this means the reset was NOT applied and the engine \
                 saw seq=10 as too-high",
                SessionMessage::name(&*response)
            );
            match SessionMessage::try_as_admin(&*response) {
                Some(AdminBase::Heartbeat(hb)) => {
                    assert_eq!(
                        hb.test_req_id.as_deref(),
                        Some(fix_str!("RESET1")),
                        "Heartbeat response did not echo our TestReqID"
                    );
                }
                _ => unreachable!("already asserted this is a Heartbeat"),
            }

            // Teardown.
            acceptor
                .disconnect(&acceptor_session_id())
                .await
                .expect("acceptor disconnect");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::Disconnected
            );
            peer.handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 4: Heartbeat timeout -> TestRequest -> grace period -> disconnect
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn heartbeat_timeout_sends_test_request_and_disconnects() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            // Small heartbeat; with the fixture's grace limit of 1 only
            // one TestRequest needs to go unanswered before the engine
            // escalates to Logout. Keeps the read stream short.
            register_in_memory_session(&acceptor, acceptor_session_id(), build_session_settings(1));
            let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 8192);

            // Logon handshake.
            peer.logon(1, 1).await;
            let _ = wait_for_session_ready(&mut acc_events_rx).await;

            // Peer goes silent. Step tokio time past output and input
            // deadlines twice over - 3 s is comfortably past the
            // expected Logout emission at t~2.4 s.
            time::advance(Duration::from_millis(3000)).await;

            // Read messages in order until we see the Logout; filter
            // out Heartbeats (their exact count depends on the select
            // interleaving under start_paused). Assert we saw a
            // TestRequest on the way. Each read is bounded so a session
            // that stops escalating fails here instead of hanging.
            let mut saw_test_request = false;
            loop {
                let msg = peer.read().await;
                let msg_type = SessionMessage::msg_type(&*msg);
                if msg_type == MsgTypeBase::Heartbeat {
                    continue;
                } else if msg_type == MsgTypeBase::TestRequest {
                    saw_test_request = true;
                } else if msg_type == MsgTypeBase::Logout {
                    break;
                } else {
                    panic!(
                        "unexpected message on wire during heartbeat timeout: {}",
                        SessionMessage::name(&*msg)
                    );
                }
            }
            assert!(
                saw_test_request,
                "expected at least one TestRequest before the Logout during grace period"
            );

            // Session should terminate on its own - no teardown call needed.
            let reason = wait_for_session_end(&mut acc_events_rx).await;
            assert_eq!(reason, DisconnectReason::HeartbeatTimeout);
            peer.handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 5: Multiple sessions on the same Acceptor are isolated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_sessions_isolated() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let sid_a_acc = acceptor_session_id_for(fix_str!("TARGET_A"));
            let sid_b_acc = acceptor_session_id_for(fix_str!("TARGET_B"));

            // --- Acceptor with two registered sessions ---
            let (acc_tx, mut acc_rx) = mpsc::unbounded_channel();
            let acceptor =
                Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory { events_tx: acc_tx });
            register_in_memory_session(&acceptor, sid_a_acc.clone(), build_session_settings(30));
            register_in_memory_session(&acceptor, sid_b_acc.clone(), build_session_settings(30));

            // --- Initiator A ---
            let (ini_a_stream, acc_a_stream) = io::duplex(8192);
            let (acc_a_r, acc_a_w) = split(acc_a_stream);
            let (ini_a_r, ini_a_w) = split(ini_a_stream);
            let (ini_a_tx, mut ini_a_rx) = mpsc::unbounded_channel();
            let initiator_a = Initiator::<Message, InMemoryStorage, _>::new(
                initiator_session_id_for(fix_str!("TARGET_A")),
                build_session_settings(30),
                TestAppFactory {
                    events_tx: ini_a_tx,
                },
                |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
            )
            .expect("initiator settings valid");

            // --- Initiator B ---
            let (ini_b_stream, acc_b_stream) = io::duplex(8192);
            let (acc_b_r, acc_b_w) = split(acc_b_stream);
            let (ini_b_r, ini_b_w) = split(ini_b_stream);
            let (ini_b_tx, mut ini_b_rx) = mpsc::unbounded_channel();
            let initiator_b = Initiator::<Message, InMemoryStorage, _>::new(
                initiator_session_id_for(fix_str!("TARGET_B")),
                build_session_settings(30),
                TestAppFactory {
                    events_tx: ini_b_tx,
                },
                |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
            )
            .expect("initiator settings valid");

            // --- Spawn all four tasks ---
            let acc_a_handle = acceptor.run_session(acc_a_r, acc_a_w, TEST_PEER_ADDR);
            let acc_b_handle = acceptor.run_session(acc_b_r, acc_b_w, TEST_PEER_ADDR);
            let ini_a_handle = initiator_a
                .run_session(ini_a_r, ini_a_w, None)
                .expect("initiator A run_session");
            let ini_b_handle = initiator_b
                .run_session(ini_b_r, ini_b_w, None)
                .expect("initiator B run_session");

            // --- Wait for both sessions to be fully up, on both sides ---
            let sender_a = wait_for_session_ready(&mut ini_a_rx).await;
            let sender_b = wait_for_session_ready(&mut ini_b_rx).await;
            let _ = wait_for_session_ready_of(&mut acc_rx, &sid_a_acc).await;
            let _ = wait_for_session_ready_of(&mut acc_rx, &sid_b_acc).await;

            // --- Each initiator sends one NewOrderSingle ---
            sender_a
                .send(new_order_single_with_empty_header())
                .expect("sender A send");
            sender_b
                .send(new_order_single_with_empty_header())
                .expect("sender B send");

            // --- Verify routing: each AppMsgIn arrives on the matching session id ---
            let mut seen_a = false;
            let mut seen_b = false;
            while !(seen_a && seen_b) {
                let event = recv_event(&mut acc_rx).await;
                if let TestEvent::AppMsgIn(sid, msg) = &event {
                    assert!(
                        is_new_order_single(msg),
                        "unexpected non-NOS app message on session {sid}"
                    );
                    if *sid == sid_a_acc {
                        assert!(!seen_a, "session A received a duplicate message");
                        seen_a = true;
                    } else if *sid == sid_b_acc {
                        assert!(!seen_b, "session B received a duplicate message");
                        seen_b = true;
                    } else {
                        panic!("AppMsgIn on unknown session: {sid}");
                    }
                }
            }

            // --- Disconnect session A only ---
            initiator_a.disconnect().await.expect("disconnect A");

            // Both sides of session A should fire SessionEnd: the requesting
            // side by decision, the other on the transport going away.
            assert_eq!(
                wait_for_session_end(&mut ini_a_rx).await,
                DisconnectReason::Disconnected
            );
            assert_eq!(
                wait_for_session_end_of(&mut acc_rx, &sid_a_acc).await,
                DisconnectReason::Disconnected
            );
            acc_a_handle.await.expect("acceptor A task");
            ini_a_handle.await.expect("initiator A task");

            // --- Verify session B is still fully functional ---
            sender_b
                .send(new_order_single_with_empty_header())
                .expect("sender B second send");
            let event = wait_until(
                &mut acc_rx,
                |e| matches!(e, TestEvent::AppMsgIn(sid, _) if *sid == sid_b_acc),
            )
            .await;
            assert_matches!(
                event,
                TestEvent::AppMsgIn(sid, msg) if sid == sid_b_acc && is_new_order_single(&msg)
            );

            // --- Teardown B ---
            initiator_b.disconnect().await.expect("disconnect B");
            assert_eq!(
                wait_for_session_end(&mut ini_b_rx).await,
                DisconnectReason::Disconnected
            );
            assert_eq!(
                wait_for_session_end_of(&mut acc_rx, &sid_b_acc).await,
                DisconnectReason::Disconnected
            );
            acc_b_handle.await.expect("acceptor B task");
            ini_b_handle.await.expect("initiator B task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 6: Non-blocking send preserves order under TCP write-side backpressure
// ---------------------------------------------------------------------------

/// `Sender::send` is synchronous and never blocks: a burst of sends stages
/// every message instantly into the unbounded staging queue. Delivery order
/// and sequence numbers must still be correct when the *only* backpressure is
/// the TCP write side - here a deliberately tight 1024-byte duplex pipe that
/// cannot hold all N serialized messages at once. The session task flushes as
/// the peer drains, so all N arrive in order with no `PossDupFlag`.
#[tokio::test]
async fn nonblocking_send_preserves_order_under_write_backpressure() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Intentionally tight: a 1024-byte duplex pipe cannot hold all N
            // serialized NewOrderSingles (~100 B each) at once, so the
            // session's `flush_output` blocks on the write side until the peer
            // drains. `send` itself never blocks - it stages synchronously.
            const N: SeqNum = 20;

            let mut p = spawn_initiator_with_raw_peer(1024, build_session_settings(30));
            let sender = p.peer_logon_handshake(30).await;
            let RawPeerInitiator {
                initiator,
                handle: ini_handle,
                events_rx: mut ini_events_rx,
                mut peer_r,
                peer_w: _peer_w,
                peer_sid: _peer_sid,
                mut peer_buf,
            } = p;

            // Stage all N synchronously - no awaits, no blocking. The 1024-byte
            // pipe cannot absorb them all, so they back up in the staging queue
            // and in `flush_output` until the peer reads below.
            for _ in 0..N {
                sender
                    .send(new_order_single_with_empty_header())
                    .expect("sender.send");
            }
            assert_eq!(
                sender.backlog_len(),
                N as usize,
                "send stages, nothing more"
            );

            // Let the session task drain until the pipe fills and its write
            // parks. Some of the burst must still be staged then - that is the
            // backpressure this test is about; a pipe that swallowed the whole
            // burst would leave nothing to order under it.
            for _ in 0..(N as usize * 4) {
                yield_now().await;
            }
            assert!(
                sender.backlog_len() > 0,
                "the write side must have parked before the burst drained"
            );

            // Read N messages in order. Draining `peer_r` unblocks the
            // session's write_all one message at a time.
            for expected in 2..=(N + 1) {
                let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
                assert!(
                    is_new_order_single(&msg),
                    "seq {expected}: expected NewOrderSingle, got {}",
                    SessionMessage::name(&*msg)
                );
                assert_eq!(msg.msg_seq_num(), expected);
                assert_ne!(
                    msg.poss_dup_flag(),
                    Some(true),
                    "first-send message at seq {expected} must not have PossDupFlag=Y"
                );
            }

            // Teardown.
            initiator.disconnect().await.expect("disconnect");
            assert_eq!(
                wait_for_session_end(&mut ini_events_rx).await,
                DisconnectReason::Disconnected
            );
            ini_handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 7: Count cap evicts a slow consumer (SlowConsumer)
// ---------------------------------------------------------------------------

/// When the outbound staging queue exceeds `max_outbound_queued_messages`, the
/// session is hard-disconnected with `DisconnectReason::SlowConsumer`. `send`
/// is non-blocking, so the application can stage well past the cap in one
/// burst; the cap check - run after each flush & admin drain, once logged on -
/// then trips and evicts. The still-queued messages are drained to storage for
/// resend on reconnect (continuity preserved).
#[tokio::test]
async fn count_cap_evicts_slow_consumer() {
    let local = LocalSet::new();
    local
        .run_until(async {
            const CAP: usize = 3;
            const N: SeqNum = 20;

            let mut session_settings = build_session_settings(30);
            session_settings.max_outbound_queued_messages = Some(NonZeroUsize::new(CAP).unwrap());

            // Peer completes the logon handshake but then stops reading app
            // messages - they pile up in the staging queue past the cap.
            let mut p = spawn_initiator_with_raw_peer(8192, session_settings);
            let sender = p.peer_logon_handshake(30).await;
            let RawPeerInitiator {
                initiator: _initiator,
                handle: ini_handle,
                events_rx: mut ini_events_rx,
                peer_r: _peer_r,
                peer_w: _peer_w,
                peer_sid: _peer_sid,
                peer_buf: _peer_buf,
            } = p;

            // Stage N >> CAP messages synchronously in one burst.
            for _ in 0..N {
                sender
                    .send(new_order_single_with_empty_header())
                    .expect("sender.send");
            }

            // The session must hard-disconnect as a slow consumer.
            let reason = wait_for_session_end(&mut ini_events_rx).await;
            assert_eq!(reason, DisconnectReason::SlowConsumer);

            ini_handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 8: Lag cap evicts a slow-but-steady consumer (SlowConsumer)
// ---------------------------------------------------------------------------

/// The lag cap catches the consumer that the protocol's liveness mechanisms
/// miss: one that keeps the connection alive but drains slower than we
/// produce, so the staging queue's head ages without bound. With a tight pipe
/// the session flushes a few messages, then `flush_output` blocks on the full
/// write side while the bulk of the burst sits in the staging queue aging.
/// Once the peer reads enough to unblock the flush, the post-flush cap check
/// sees `head_age > max_outbound_lag` and evicts.
///
/// Real wall-clock timing (the staging `Instant` is `std::time::Instant`, so
/// `start_paused` cannot age it). The margin (cap 100 ms vs a 300 ms wait) is
/// wide and the staged head's age is fixed at enqueue time, so this is robust.
#[tokio::test]
async fn lag_cap_evicts_slow_consumer() {
    let local = LocalSet::new();
    local
        .run_until(async {
            const N: SeqNum = 50;
            let lag = Duration::from_millis(100);

            let mut session_settings = build_session_settings(30);
            session_settings.max_outbound_lag = Some(lag);

            // Tight pipe: a 50-message burst (~140 B each) cannot fit, so the
            // session flushes a few then blocks on the write side with the rest
            // still staged and aging.
            let mut p = spawn_initiator_with_raw_peer(512, session_settings);
            let sender = p.peer_logon_handshake(30).await;
            let RawPeerInitiator {
                initiator: _initiator,
                handle: ini_handle,
                events_rx: mut ini_events_rx,
                mut peer_r,
                peer_w: _peer_w,
                peer_sid: _peer_sid,
                mut peer_buf,
            } = p;

            // Stage the whole burst at once. The session drains a few into the
            // pipe and then blocks on `flush_output`; the head of the queue is
            // now pinned at this enqueue instant.
            for _ in 0..N {
                sender
                    .send(new_order_single_with_empty_header())
                    .expect("sender.send");
            }

            // Let the head age well past the lag cap while the peer reads
            // nothing (the session stays blocked on the full pipe).
            time::sleep(Duration::from_millis(300)).await;

            // One read frees the pipe; the flush completes and the very next
            // cap check sees an over-aged head and evicts.
            let _ = read_one_message(&mut peer_r, &mut peer_buf).await;

            let reason = wait_for_session_end(&mut ini_events_rx).await;
            assert_eq!(reason, DisconnectReason::SlowConsumer);

            ini_handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 9: Slow-consumer eviction drains the backlog; reconnect recovers it
// ---------------------------------------------------------------------------

/// End-to-end durability: when a slow consumer is evicted (count cap here),
/// the still-staged messages are drained to storage with their assigned
/// sequence numbers. On reconnect (same session id - the acceptor caches
/// storage per session id), the peer recovers the whole backlog via a normal
/// `ResendRequest`, with original sequence numbers and `PossDupFlag=Y`. This
/// is the "no `send()`-accepted message is dropped" durability property.
#[tokio::test]
async fn slow_consumer_eviction_recovers_backlog_on_reconnect() {
    let local = LocalSet::new();
    local
        .run_until(async {
            const CAP: usize = 3;
            const K: SeqNum = 20;

            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let mut settings = build_session_settings(30);
            settings.max_outbound_queued_messages = Some(NonZeroUsize::new(CAP).unwrap());

            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            register_in_memory_session(&acceptor, acceptor_session_id(), settings);

            // --- Connection #1: logon, acceptor app stages K, peer never reads
            //     them, count cap evicts, backlog drained to storage. ---
            {
                let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 8192);
                // Peer (client) drives logon with seq 1.
                peer.logon(1, 30).await;

                let sender = wait_for_session_ready(&mut acc_events_rx).await;

                // Acceptor's app stages K app messages (seq 2..=K+1); the peer
                // never reads them.
                for _ in 0..K {
                    sender
                        .send(new_order_single_with_empty_header())
                        .expect("acceptor app send");
                }

                let reason = wait_for_session_end(&mut acc_events_rx).await;
                assert_eq!(reason, DisconnectReason::SlowConsumer);
                peer.handle.await.expect("acceptor task #1");
                // Storage (seq 2..=K+1 stored, next_sender = K+2) is returned to
                // the registry when the task exits.
            }

            // --- Connection #2: reconnect, recover the drained backlog. ---
            {
                let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 65536);
                // Peer continues its own seq counter: it sent seq 1 on conn #1,
                // so its reconnect Logon is seq 2 (no ResetSeqNumFlag). The
                // acceptor's sender counter persisted: its Logon response is
                // seq K+2.
                let logon_resp = peer.logon(2, 30).await;
                assert_eq!(logon_resp.msg_seq_num(), K + 2);

                let _ = wait_for_session_ready(&mut acc_events_rx).await;

                // Peer requests the evicted backlog (its next seq is 3).
                let resend_request = peer_resend_request_bytes(&peer.peer_sid, 3, 2, K + 1);
                peer.write(&resend_request).await;

                // Recover all K messages with original seq nums and PossDupFlag=Y.
                for expected in 2..=(K + 1) {
                    let msg = peer.read().await;
                    assert!(
                        is_new_order_single(&msg),
                        "recovered seq {expected} should be a NewOrderSingle, got {}",
                        SessionMessage::name(&*msg)
                    );
                    assert_eq!(msg.msg_seq_num(), expected);
                    assert_eq!(
                        msg.poss_dup_flag(),
                        Some(true),
                        "recovered message at seq {expected} must have PossDupFlag=Y"
                    );
                }

                acceptor
                    .disconnect(&acceptor_session_id())
                    .await
                    .expect("acceptor disconnect");
                assert_eq!(
                    wait_for_session_end(&mut acc_events_rx).await,
                    DisconnectReason::Disconnected
                );
                peer.handle.await.expect("acceptor task #2");
            }
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 10: Lag cap is suspended during a resend and for one grace window after
// ---------------------------------------------------------------------------

/// The lag cap must not evict a *healthy* peer during a resend, nor in the
/// grace window immediately after it. During a resend the app-drain arm
/// is gated off, so a message staged *during* the resend ages for reasons
/// unrelated to consumer speed; evicting on that would be a false positive and
/// would only provoke a larger `ResendRequest` on reconnect.
///
/// Setup: a tight pipe keeps a resend "in progress" (blocked on the full write
/// side). A message staged once the resend is active then ages well past
/// `max_outbound_lag` while the resend holds. The peer eventually drains the
/// resend; the staged message must NOT have caused a `SlowConsumer` eviction
/// (suppressed during the resend, then covered by the grace window as it
/// drains), and must still be delivered as a normal first-send.
#[tokio::test(start_paused = true)]
async fn lag_cap_suspended_during_resend_and_grace_window() {
    let local = LocalSet::new();
    local
        .run_until(async {
            const M: SeqNum = 6;
            let lag = Duration::from_millis(200);

            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let mut settings = build_session_settings(30);
            settings.max_outbound_lag = Some(lag);

            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            register_in_memory_session(&acceptor, acceptor_session_id(), settings);
            // Tight pipe so the resend stalls on the write side while we let the
            // staged-during-resend message age.
            let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 512);

            // Logon (peer seq 1).
            peer.logon(1, 30).await;
            let sender = wait_for_session_ready(&mut acc_events_rx).await;

            // Acceptor app sends M messages (seq 2..=M+1); peer reads them all
            // promptly so they are stored and do not age out the lag cap.
            for _ in 0..M {
                sender
                    .send(new_order_single_with_empty_header())
                    .expect("initial send");
            }
            for expected in 2..=(M + 1) {
                let msg = peer.read().await;
                assert_eq!(msg.msg_seq_num(), expected);
            }

            // Peer requests a resend of the whole range (peer seq 2).
            let resend_request = peer_resend_request_bytes(&peer.peer_sid, 2, 2, M + 1);
            peer.write(&resend_request).await;

            // Read the first resent message - this confirms the resend is
            // active (`active_resend` is set), so the message we stage next
            // cannot be drained until the resend completes.
            let first_resend = peer.read().await;
            assert_eq!(first_resend.msg_seq_num(), 2);
            assert_eq!(first_resend.poss_dup_flag(), Some(true));

            // Stage one fresh message DURING the resend. It will be transmitted
            // as seq M+2 once the resend clears.
            sender
                .send(new_order_single_with_empty_header())
                .expect("stage during resend");

            // Let it age well past `max_outbound_lag` while the resend holds the
            // drain arm and the tight pipe blocks further resend output. A bare
            // `!resend_in_progress` suspension alone is not enough - the grace
            // window must also cover the post-resend drain. On the paused clock
            // the session task is parked on the full pipe, so this is an exact
            // jump, not a race against the cap.
            time::sleep(Duration::from_millis(450)).await;

            // Drain the rest of the resend (seq 3..=M+1), all PossDup=Y. The
            // reads are bounded: an evicted session closes the pipe, and a
            // stalled one would otherwise hang the test instead of failing it.
            for expected in 3..=(M + 1) {
                let msg = peer.read().await;
                assert_eq!(msg.msg_seq_num(), expected);
                assert_eq!(msg.poss_dup_flag(), Some(true));
            }

            // The staged-during-resend message must survive: delivered as a
            // first-send (seq M+2, no PossDupFlag), proving it was neither
            // evicted during the resend nor in the grace window after it.
            let fresh = peer.read().await;
            assert_eq!(fresh.msg_seq_num(), M + 2);
            assert_ne!(
                fresh.poss_dup_flag(),
                Some(true),
                "staged-during-resend message must go out as a first-send"
            );
            assert!(is_new_order_single(&fresh));

            // Clean teardown: the session ends on this request, not on a
            // SlowConsumer eviction.
            acceptor
                .disconnect(&acceptor_session_id())
                .await
                .expect("acceptor disconnect");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::Disconnected,
                "healthy peer must not be evicted during/after a resend"
            );
            peer.handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 11: First Logon carries a valid NextExpectedMsgSeqNum(789) when enabled
// ---------------------------------------------------------------------------

/// When `enable_next_expected_msg_seq_num` is set, the initiator's first
/// Logon must carry a valid `NextExpectedMsgSeqNum(789)` value (>= 1) equal
/// to `storage.next_target_msg_seq_num().get()` (1 on a fresh session).
///
/// Regression: the engine once emitted the uninitialised sentinel `Some(0)`,
/// which fails serialization (zero `SeqNum`), so the IO flush phase
/// substituted a `SequenceReset(35=4)`-GapFill and the peer never saw the
/// Logon at all.
#[tokio::test]
async fn initiator_first_logon_emits_valid_next_expected_msg_seq_num_when_enabled() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut settings = build_session_settings(30);
            settings.enable_next_expected_msg_seq_num = true;

            let RawPeerInitiator {
                initiator,
                handle: ini_handle,
                events_rx: _ini_events_rx,
                mut peer_r,
                peer_w: _peer_w,
                peer_sid: _peer_sid,
                mut peer_buf,
            } = spawn_initiator_with_raw_peer(8192, settings);

            // Read the first message the initiator emits - must be Logon.
            let logon_req = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert_eq!(SessionMessage::msg_type(&*logon_req), MsgTypeBase::Logon);
            assert_eq!(logon_req.msg_seq_num(), 1);

            let admin = SessionMessage::try_as_admin(&*logon_req).expect("admin payload");
            let AdminBase::Logon(lg) = admin else {
                panic!("expected Logon admin payload");
            };

            // Tag 789 must equal storage's `next_target_msg_seq_num().get()`
            // (1 on a fresh session) - and never the `Some(0)` sentinel.
            let next_expected = lg
                .next_expected_msg_seq_num
                .expect("tag 789 must be present when enable_next_expected_msg_seq_num=true");
            assert_eq!(
                next_expected, 1,
                "tag 789 on a fresh-session initiator Logon should equal next_target_msg_seq_num (1)"
            );

            // Tear down - drop peer end so the initiator's IO closes.
            initiator.disconnect().await.expect("disconnect");
            ini_handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Session-close / reconfiguration API
// ---------------------------------------------------------------------------

/// `remove_session` is refused while the session task is running, and the
/// `close` -> `await_session_closed` path makes the storage recoverable. The
/// recovered storage carries the live sequence counters (the basis of the
/// reconfiguration flow).
#[tokio::test]
async fn remove_session_refused_while_active_then_recoverable_after_close() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (server_stream, client_stream) = io::duplex(8192);
            let (server_r, server_w) = split(server_stream);
            let (client_r, client_w) = split(client_stream);

            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let (ini_events_tx, mut ini_events_rx) = mpsc::unbounded_channel();

            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            register_in_memory_session(
                &acceptor,
                acceptor_session_id(),
                build_session_settings(30),
            );

            let initiator = Initiator::<Message, InMemoryStorage, _>::new(
                initiator_session_id(),
                build_session_settings(30),
                TestAppFactory {
                    events_tx: ini_events_tx,
                },
                |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
            )
            .expect("initiator settings valid");

            let acc_handle = acceptor.run_session(server_r, server_w, TEST_PEER_ADDR);
            let ini_handle = initiator
                .run_session(client_r, client_w, None)
                .expect("initiator run_session");

            wait_for_session_ready(&mut ini_events_rx).await;
            wait_for_session_ready(&mut acc_events_rx).await;

            // The running task owns the storage - removal must be refused.
            assert_matches!(
                acceptor.remove_session(&acceptor_session_id()).map(|_| ()),
                Err(AcceptorError::SessionActive)
            );

            // Force-close and await full closure: the only point at which the
            // storage is back in the registry and removable.
            acceptor.close(&acceptor_session_id()).await.expect("close");
            // `close` parks on the session's close signal, and the signal
            // fires only after the registry entry is settled - so the very
            // next call sees the session inactive, with nothing awaited in
            // between that could mask an early wake-up.
            assert_matches!(
                acceptor.is_session_active(&acceptor_session_id()),
                Ok(false),
                "close must not resolve before the registry shows the session inactive"
            );

            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::Disconnected
            );
            assert_eq!(
                wait_for_session_end(&mut ini_events_rx).await,
                DisconnectReason::Disconnected
            );
            acc_handle.await.expect("acceptor task");
            let _ = ini_handle.await;

            let storage = acceptor
                .remove_session(&acceptor_session_id())
                .expect("remove after close");
            // The one inbound message, the Logon, advanced the counter to 2,
            // so the recovered storage carries the live session state.
            assert_eq!(
                storage.next_target_msg_seq_num().get(),
                2,
                "recovered storage must preserve the live sequence counters"
            );
        })
        .await;
}

/// Test Cases Scenario 9, the peer's disaster recovery: after losing its
/// session state it reconnects with `NextNumOut` set arbitrarily high and
/// `NextNumIn` back at 1, sends a `SequenceReset<4>` (`GapFillFlag(123)=N`)
/// to a larger number still, and recovers what it missed by `ResendRequest<2>`.
/// From our side: the far-ahead Logon is answered and parked behind a
/// ResendRequest, the Reset moves `NextNumIn` past it (dropping the parked
/// Logon, Session Layer §4.8.8), and the peer's request from 1 replays the
/// old session's traffic - the Logon acknowledgement gap-filled, the orders as
/// duplicates - after which the session is in step.
#[tokio::test]
async fn peer_resynchronizes_after_losing_its_session_state() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            register_in_memory_session(
                &acceptor,
                acceptor_session_id(),
                build_session_settings(30),
            );

            // --- Session 1: three orders go out (our seq 2..=4), then the
            // peer's process dies, taking its counters with it. ---
            let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 8192);
            let logon_resp = peer.logon(1, 30).await;
            assert_eq!(logon_resp.msg_seq_num(), 1);
            let sender = wait_for_session_ready(&mut acc_events_rx).await;
            for _ in 0..3 {
                sender.send(new_order_single_with_empty_header()).expect("send");
            }
            for expected in 2..=4 {
                let msg = peer.read().await;
                assert_eq!(msg.msg_seq_num(), expected);
            }

            peer.peer_w.shutdown().await.expect("peer close");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::Disconnected
            );
            peer.handle.await.expect("acceptor task 1");

            // --- Session 2: the peer comes back numbering from 1000 and
            // expecting 1. ---
            let mut peer = connect_raw_peer(&acceptor, acceptor_session_id(), 8192);
            // The Logon is far ahead of the expected 2: answered (our seq 5,
            // the counters survived the disconnect) and followed by a request
            // for everything up to it.
            let logon_resp = peer.logon(1000, 30).await;
            assert_eq!(logon_resp.msg_seq_num(), 5);
            let request = peer.read().await;
            assert_matches!(
                SessionMessage::try_as_admin(&*request),
                Some(AdminBase::ResendRequest(rr)) if rr.begin_seq_no == 2 && rr.end_seq_no == 999
            );
            wait_for_session_ready(&mut acc_events_rx).await;

            // Instead of a replay it does not have, the peer resets our
            // NextNumIn past everything it will ever number...
            let reset = peer_sequence_reset_bytes(&peer.peer_sid, 1001, 1002, Some(false));
            peer.write(&reset).await;
            // ...and asks for what it missed since its own counter went to 1.
            let resend_request = peer_resend_request_bytes(&peer.peer_sid, 1002, 1, 4);
            peer.write(&resend_request).await;

            // Seq 1 was the old Logon acknowledgement - gap-filled; 2..=4 the
            // orders, as duplicates.
            let gap = peer.read().await;
            assert_eq!(gap.msg_seq_num(), 1);
            assert_eq!(gap.poss_dup_flag(), Some(true));
            assert_matches!(
                SessionMessage::try_as_admin(&*gap),
                Some(AdminBase::SequenceReset(sr)) if sr.gap_fill_flag == Some(true) && sr.new_seq_no == 2
            );
            for expected in 2..=4 {
                let msg = peer.read().await;
                assert_eq!(msg.msg_seq_num(), expected);
                assert_eq!(msg.poss_dup_flag(), Some(true));
                assert!(is_new_order_single(&msg));
            }

            // In step: the next peer message is in sequence and answered with
            // the next number of ours after the two the handshake consumed.
            let test_request = peer_test_request_bytes(&peer.peer_sid, 1003, fix_str!("sync"));
            peer.write(&test_request).await;
            let hb = peer.read().await;
            assert_eq!(hb.msg_seq_num(), 7);
            assert_matches!(
                SessionMessage::try_as_admin(&*hb),
                Some(AdminBase::Heartbeat(hb)) if hb.test_req_id.as_deref() == Some(fix_str!("sync"))
            );

            acceptor
                .disconnect(&acceptor_session_id())
                .await
                .expect("acceptor disconnect");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::Disconnected
            );
            peer.handle.await.expect("acceptor task 2");
        })
        .await;
}

/// A session task that panics in a user callback must not leave the session
/// behind as an "active" zombie: the acceptor reports it inactive, the peer
/// can reconnect (it recovers the message that caused the panic through the
/// usual resend, FIX Session Layer §4.8), and a graceful Logout still works
/// on the new connection.
#[tokio::test]
async fn panicked_session_task_releases_the_session_for_reconnect() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let (ini_events_tx, mut ini_events_rx) = mpsc::unbounded_channel();
            let armed = Rc::new(Cell::new(false));

            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(PanicOnceAppFactory {
                inner: TestAppFactory {
                    events_tx: acc_events_tx,
                },
                armed: armed.clone(),
            });
            register_in_memory_session(
                &acceptor,
                acceptor_session_id(),
                build_session_settings(30),
            );
            let initiator = Initiator::<Message, InMemoryStorage, _>::new(
                initiator_session_id(),
                build_session_settings(30),
                TestAppFactory {
                    events_tx: ini_events_tx,
                },
                |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
            )
            .expect("initiator settings valid");

            // --- Connection 1: the acceptor's app panics on the first order ---
            let (server_stream, client_stream) = io::duplex(8192);
            let (server_r, server_w) = split(server_stream);
            let (client_r, client_w) = split(client_stream);
            let acc_handle = acceptor.run_session(server_r, server_w, TEST_PEER_ADDR);
            let ini_handle = initiator
                .run_session(client_r, client_w, None)
                .expect("initiator run_session");
            let ini_sender = wait_for_session_ready(&mut ini_events_rx).await;
            wait_for_session_ready(&mut acc_events_rx).await;
            assert_matches!(acceptor.is_session_active(&acceptor_session_id()), Ok(true));

            armed.set(true);
            ini_sender
                .send(new_order_single_with_empty_header())
                .expect("sender.send");

            let join = timeout(TEST_TIMEOUT, acc_handle)
                .await
                .expect("acceptor task must end");
            assert_matches!(join, Err(e) if e.is_panic(), "the callback unwinds the task");
            assert!(!armed.get(), "the panic fired in on_app_msg_in");

            // The dead task cleaned up after itself: the session is inactive
            // and its storage is back, so the id is free to reconnect.
            timeout(TEST_TIMEOUT, acceptor.await_session_closed(&acceptor_session_id()))
                .await
                .expect("session must close after the panic")
                .expect("registered session");
            assert_matches!(acceptor.is_session_active(&acceptor_session_id()), Ok(false));

            // The initiator saw the transport go away.
            assert_eq!(
                wait_for_session_end(&mut ini_events_rx).await,
                DisconnectReason::Disconnected
            );
            ini_handle.await.expect("initiator task");

            // --- Connection 2: reconnect, recover the order, log out ---
            let (server_stream, client_stream) = io::duplex(8192);
            let (server_r, server_w) = split(server_stream);
            let (client_r, client_w) = split(client_stream);
            let acc_handle = acceptor.run_session(server_r, server_w, TEST_PEER_ADDR);
            let ini_handle = initiator
                .run_session(client_r, client_w, None)
                .expect("reconnect after the panic must not be refused as a duplicate");
            wait_for_session_ready(&mut ini_events_rx).await;
            wait_for_session_ready(&mut acc_events_rx).await;

            // The order never reached the application on connection 1 (the
            // counter did not advance past it), so it comes back on resend.
            let event =
                wait_until(&mut acc_events_rx, |e| matches!(e, TestEvent::AppMsgIn(..))).await;
            assert_matches!(
                event,
                TestEvent::AppMsgIn(_, msg) if is_new_order_single(&msg) && msg.poss_dup_flag() == Some(true)
            );

            acceptor
                .logout(&acceptor_session_id(), None, None)
                .await
                .expect("logout");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::LocalRequestedLogout
            );
            assert_eq!(
                wait_for_session_end(&mut ini_events_rx).await,
                DisconnectReason::RemoteRequestedLogout
            );
            acc_handle.await.expect("acceptor task");
            ini_handle.await.expect("initiator task");
        })
        .await;
}

/// A suspended session rejects new inbound connections (no `SessionReady`),
/// and resuming it accepts them again.
#[tokio::test]
async fn suspended_session_rejects_inbound_logon_until_resumed() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            register_in_memory_session(
                &acceptor,
                acceptor_session_id(),
                build_session_settings(30),
            );

            // --- Connection #1: establish, then suspend and close. ---
            let mut conn1 = connect_initiator(&acceptor);
            wait_for_session_ready(&mut conn1.events_rx).await;
            wait_for_session_ready(&mut acc_events_rx).await;

            acceptor
                .suspend_session(&acceptor_session_id())
                .expect("suspend");
            acceptor.close(&acceptor_session_id()).await.expect("close");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::Disconnected
            );
            conn1.acc_handle.await.expect("acceptor task 1");
            let _ = conn1.ini_handle.await;

            // Reset seq state so a fresh initiator can later log on cleanly once
            // resumed (the rejected attempt below never reaches seq validation).
            acceptor
                .reset_session(&acceptor_session_id())
                .expect("reset");

            // --- Connection #2: while suspended -> rejected (no SessionReady). ---
            let conn2 = connect_initiator(&acceptor);

            // The acceptor drops the suspended connection before building the
            // application, so its task finishes without any SessionReady.
            timeout(TEST_TIMEOUT, conn2.acc_handle)
                .await
                .expect("rejected acceptor task should finish")
                .expect("acceptor task 2");
            assert_matches!(
                acc_events_rx.try_recv(),
                Err(TryRecvError::Empty),
                "a suspended session must not emit SessionReady"
            );
            let _ = conn2.ini_handle.await;

            // --- Connection #3: after resume -> accepted. ---
            acceptor
                .resume_session(&acceptor_session_id())
                .expect("resume");
            let mut conn3 = connect_initiator(&acceptor);
            wait_for_session_ready(&mut conn3.events_rx).await;
            wait_for_session_ready(&mut acc_events_rx).await;

            // Clean up.
            acceptor.close(&acceptor_session_id()).await.expect("close");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::Disconnected
            );
            conn3.acc_handle.await.expect("acceptor task 3");
            let _ = conn3.ini_handle.await;
        })
        .await;
}

// ---------------------------------------------------------------------------
// Logon response timeout
// ---------------------------------------------------------------------------

/// An initiator whose `Logon<A>` is never answered must give up on its own.
///
/// `heartbeat_interval: None` proposes `HeartBtInt=0` (FIX Transport §5.1), so
/// every heartbeat-derived deadline in the session loop is unarmed; the logout
/// deadline needs a `Logout<5>` we never sent, and the app-send arm is gated on
/// being logged on. Without a deadline of its own the loop has nothing left to
/// wake it, so a peer that completes the TCP connection and then says nothing
/// parks the session task forever - holding the initiator's storage, so every
/// later `connect()` is refused as `SessionActive`, and never running
/// `on_session_end`, so the application cannot even observe it.
#[tokio::test(start_paused = true)]
async fn initiator_gives_up_on_an_unanswered_logon() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut settings = build_session_settings(30);
            settings.heartbeat_interval = None;
            settings.auto_disconnect_after_no_logon_response = Duration::from_millis(50);

            let mut p = spawn_initiator_with_raw_peer(65536, settings);

            // Read the Logon request but never answer it.
            let logon_req = timeout(
                TEST_TIMEOUT,
                read_one_message(&mut p.peer_r, &mut p.peer_buf),
            )
            .await
            .expect("Logon request");
            assert_eq!(SessionMessage::msg_type(&*logon_req), MsgTypeBase::Logon);

            let reason = wait_for_session_end(&mut p.events_rx).await;
            assert_eq!(reason, DisconnectReason::LogonTimeout);

            // Nothing follows the Logon on the wire: an unestablished session
            // has no Logout to send, and no TestRequest or Heartbeat may
            // overtake a Logon the peer might not have seen (Scenario 2S).
            assert_matches!(
                try_read_one_message(&mut p.peer_r, &mut p.peer_buf).await,
                None
            );

            p.handle.await.expect("initiator task");
        })
        .await;
}

/// No keep-alive traffic may reach the wire before the handshake completes,
/// however long the peer takes to answer. A `Heartbeat<0>` or `TestRequest<1>`
/// that overtakes a `Logon<A>` the peer never saw arrives as its first message,
/// and a first message that is not a Logon puts us on its disconnect path
/// (FIX Session Layer §4.3.1; Test Cases Scenario 2S).
///
/// The paused clock auto-advances to the earliest armed timer, so the session
/// jumps straight to its 3s Logon deadline - past the 1s heartbeat interval and
/// the 1.2s input timeout that would otherwise have fired twice over.
#[tokio::test(start_paused = true)]
async fn no_keep_alive_traffic_before_the_logon_response() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut settings = build_session_settings(1);
            settings.auto_disconnect_after_no_logon_response = Duration::from_secs(3);

            let mut p = spawn_initiator_with_raw_peer(65536, settings);

            let logon_req = timeout(
                TEST_TIMEOUT,
                read_one_message(&mut p.peer_r, &mut p.peer_buf),
            )
            .await
            .expect("Logon request");
            assert_eq!(SessionMessage::msg_type(&*logon_req), MsgTypeBase::Logon);

            // Read before checking the event: with nothing to read this
            // returns only at the EOF the Logon deadline produces, so a leaked
            // Heartbeat or TestRequest is reported as itself rather than as a
            // missing session-end event. The guard is what keeps a regression
            // that arms no deadline at all a failure rather than a hang - the
            // paused clock would otherwise have nothing to advance to.
            let leaked = timeout(
                TEST_TIMEOUT,
                try_read_one_message(&mut p.peer_r, &mut p.peer_buf),
            )
            .await
            .expect("session task parked with no deadline armed");
            assert_matches!(
                leaked,
                None,
                "keep-alive traffic leaked out before the session was established"
            );
            assert_eq!(
                wait_for_session_end(&mut p.events_rx).await,
                DisconnectReason::LogonTimeout
            );

            p.handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Duplicate connections (Test Cases Scenario 1S(b))
// ---------------------------------------------------------------------------

/// A second connection presenting the identity of a session that is already
/// running is dropped without a byte in reply - a Reject or Logout would
/// consume a `MsgSeqNum` and put the live session out of step (Test Cases
/// §4.4.1 Scenario 1S(b); Session Layer §4.6.4) - and reported to the
/// observer as exactly that, so a policy can tell it from an unknown peer.
/// The live session is not disturbed: it stays active and carries an order
/// afterwards in sequence, with no recovery. Driven through two real
/// connections on the public API, rather than by pre-emptying the registry.
#[tokio::test]
async fn duplicate_connection_for_a_live_session_is_dropped_without_disturbing_it() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let (ini_events_tx, mut ini_events_rx) = mpsc::unbounded_channel();
            let observer = Rc::new(RecordingObserver::default());

            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            acceptor.set_connection_observer(observer.clone());
            register_in_memory_session(
                &acceptor,
                acceptor_session_id(),
                build_session_settings(30),
            );
            let initiator = Initiator::<Message, InMemoryStorage, _>::new(
                initiator_session_id(),
                build_session_settings(30),
                TestAppFactory {
                    events_tx: ini_events_tx,
                },
                |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
            )
            .expect("initiator settings valid");

            // --- Connection 1: the live session ---
            let (server_stream, client_stream) = io::duplex(8192);
            let (server_r, server_w) = split(server_stream);
            let (client_r, client_w) = split(client_stream);
            let acc_handle = acceptor.run_session(server_r, server_w, TEST_PEER_ADDR);
            let ini_handle = initiator
                .run_session(client_r, client_w, None)
                .expect("initiator run_session");
            let ini_sender = wait_for_session_ready(&mut ini_events_rx).await;
            wait_for_session_ready(&mut acc_events_rx).await;

            // --- Connection 2: the same identity again, from a raw peer ---
            let (dup_stream, mut dup_peer) = io::duplex(8192);
            let (dup_r, dup_w) = split(dup_stream);
            let dup_handle = acceptor.run_session(dup_r, dup_w, TEST_PEER_ADDR);
            dup_peer
                .write_all(&peer_logon_bytes(
                    &acceptor_session_id().reverse_route(),
                    1,
                    30,
                ))
                .await
                .expect("write duplicate logon");

            timeout(TEST_TIMEOUT, dup_handle)
                .await
                .expect("the duplicate connection must be dropped")
                .expect("duplicate connection task");
            let mut written = Vec::new();
            timeout(TEST_TIMEOUT, dup_peer.read_to_end(&mut written))
                .await
                .expect("the duplicate connection must be closed")
                .expect("read");
            assert!(
                written.is_empty(),
                "1S(b): nothing may be sent on a duplicate connection"
            );
            assert_eq!(
                observer.summaries(),
                vec![format!("active:{}", acceptor_session_id())]
            );

            // --- The live session is untouched ---
            assert_matches!(acceptor.is_session_active(&acceptor_session_id()), Ok(true));
            ini_sender
                .send(new_order_single_with_empty_header())
                .expect("sender.send");
            // The very next acceptor event is the order itself - not a
            // SessionEnd, and not a message recovered on resend.
            let event = recv_event(&mut acc_events_rx).await;
            assert_matches!(
                event,
                TestEvent::AppMsgIn(_, msg)
                    if is_new_order_single(&msg) && msg.poss_dup_flag() != Some(true)
            );

            acceptor
                .logout(&acceptor_session_id(), None, None)
                .await
                .expect("logout");
            assert_eq!(
                wait_for_session_end(&mut acc_events_rx).await,
                DisconnectReason::LocalRequestedLogout
            );
            assert_eq!(
                wait_for_session_end(&mut ini_events_rx).await,
                DisconnectReason::RemoteRequestedLogout
            );
            acc_handle.await.expect("acceptor task");
            ini_handle.await.expect("initiator task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Acceptor shutdown modes, with live sessions
// ---------------------------------------------------------------------------

/// Drain events until `count` sessions have ended, returning who ended why.
async fn collect_session_ends(
    rx: &mut mpsc::UnboundedReceiver<TestEvent>,
    count: usize,
) -> Vec<(SessionId, DisconnectReason)> {
    let mut ends = Vec::new();
    while ends.len() < count {
        if let TestEvent::SessionEnd(sid, reason) = recv_event(rx).await {
            ends.push((sid, reason));
        }
    }
    ends
}

/// `ShutdownMode::Disconnect` drops every live session on the spot: no
/// Logout on either wire, both sessions ending as `Disconnected`, and
/// `shutdown` itself returning without waiting on peers that were never
/// asked anything. On return the registry is settled - the sessions are
/// removable at once, which is what a supervisor replacing the acceptor
/// needs.
#[tokio::test]
async fn shutdown_disconnect_drops_every_live_session_without_a_farewell() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            let mut a =
                spawn_raw_peer_session(&acceptor, &mut acc_events_rx, fix_str!("TARGET_A")).await;
            let mut b =
                spawn_raw_peer_session(&acceptor, &mut acc_events_rx, fix_str!("TARGET_B")).await;

            timeout(TEST_TIMEOUT, acceptor.shutdown(ShutdownMode::Disconnect))
                .await
                .expect("shutdown must not wait on peers it never addressed");

            for sid in [&a.sid, &b.sid] {
                acceptor
                    .remove_session(sid)
                    .expect("every session is back in the registry when shutdown returns");
            }

            let mut ends = collect_session_ends(&mut acc_events_rx, 2).await;
            ends.sort_by_key(|(sid, _)| sid.to_string());
            assert_eq!(
                ends,
                vec![
                    (a.sid.clone(), DisconnectReason::Disconnected),
                    (b.sid.clone(), DisconnectReason::Disconnected),
                ]
            );

            for peer in [&mut a, &mut b] {
                assert!(
                    peer.read_to_close().await.is_empty(),
                    "Disconnect sends nothing - not even a Logout"
                );
            }
            a.handle.await.expect("session task A");
            b.handle.await.expect("session task B");
        })
        .await;
}

/// `ShutdownMode::LogoutAndDisconnect` puts a Logout - with the caller's
/// `Text(58)` - on every live session's wire and closes without waiting for
/// an answer: the peers here never reply, and `shutdown` still returns. The
/// sessions end as `Disconnected`, the transport verdict, since the logout
/// exchange was never completed.
#[tokio::test]
async fn shutdown_logout_and_disconnect_sends_logout_then_closes_without_waiting() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            let mut a =
                spawn_raw_peer_session(&acceptor, &mut acc_events_rx, fix_str!("TARGET_A")).await;
            let mut b =
                spawn_raw_peer_session(&acceptor, &mut acc_events_rx, fix_str!("TARGET_B")).await;

            timeout(
                TEST_TIMEOUT,
                acceptor.shutdown(ShutdownMode::LogoutAndDisconnect {
                    session_status: None,
                    text: Some(fix_str!("maintenance").to_owned()),
                }),
            )
            .await
            .expect("shutdown must not wait for a Logout response");

            for sid in [&a.sid, &b.sid] {
                acceptor
                    .remove_session(sid)
                    .expect("every session is back in the registry when shutdown returns");
            }

            for peer in [&mut a, &mut b] {
                let logout = peer.read().await;
                let AdminBase::Logout(ref logout) =
                    SessionMessage::try_as_admin(&*logout).expect("admin message")
                else {
                    panic!("expected Logout on {}", peer.sid);
                };
                assert_eq!(logout.text.as_deref(), Some(fix_str!("maintenance")));
                assert!(
                    peer.read_to_close().await.is_empty(),
                    "nothing follows the Logout"
                );
            }

            let mut ends = collect_session_ends(&mut acc_events_rx, 2).await;
            ends.sort_by_key(|(sid, _)| sid.to_string());
            assert_eq!(
                ends,
                vec![
                    (a.sid.clone(), DisconnectReason::Disconnected),
                    (b.sid.clone(), DisconnectReason::Disconnected),
                ]
            );
            a.handle.await.expect("session task A");
            b.handle.await.expect("session task B");
        })
        .await;
}

/// `ShutdownMode::GracefulLogout` is the FIX logout exchange on every live
/// session at once (Session Layer §4.6): a Logout goes out to each peer and
/// `shutdown` stays pending until each has answered - it is still waiting
/// with one response in and one outstanding - after which the sessions end
/// as `LocalRequestedLogout`, the exchange having completed on our request.
#[tokio::test]
async fn shutdown_graceful_logout_waits_for_every_peers_logout_response() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
            let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(TestAppFactory {
                events_tx: acc_events_tx,
            });
            let mut a =
                spawn_raw_peer_session(&acceptor, &mut acc_events_rx, fix_str!("TARGET_A")).await;
            let mut b =
                spawn_raw_peer_session(&acceptor, &mut acc_events_rx, fix_str!("TARGET_B")).await;

            let shutdown = spawn_local({
                let acceptor = acceptor.clone();
                async move {
                    acceptor
                        .shutdown(ShutdownMode::GracefulLogout {
                            session_status: None,
                            text: Some(fix_str!("end of day").to_owned()),
                        })
                        .await;
                }
            });

            for peer in [&mut a, &mut b] {
                let logout = peer.read().await;
                let AdminBase::Logout(ref logout) =
                    SessionMessage::try_as_admin(&*logout).expect("admin message")
                else {
                    panic!("expected Logout on {}", peer.sid);
                };
                assert_eq!(logout.text.as_deref(), Some(fix_str!("end of day")));
            }
            // Both Logouts are on the wire and nobody has answered: the
            // shutdown is waiting.
            for _ in 0..4 {
                yield_now().await;
            }
            assert!(
                !shutdown.is_finished(),
                "graceful shutdown must wait for the Logout responses"
            );

            // Peer A answers; B is still outstanding, so the wait goes on.
            a.peer_w
                .write_all(&peer_logout_bytes(&a.peer_sid, 2))
                .await
                .expect("write logout response A");
            assert_eq!(
                wait_for_session_end_of(&mut acc_events_rx, &a.sid).await,
                DisconnectReason::LocalRequestedLogout
            );
            for _ in 0..4 {
                yield_now().await;
            }
            assert!(
                !shutdown.is_finished(),
                "graceful shutdown must wait for every session, not just the first"
            );

            // Peer B answers; now the shutdown can complete.
            b.peer_w
                .write_all(&peer_logout_bytes(&b.peer_sid, 2))
                .await
                .expect("write logout response B");
            assert_eq!(
                wait_for_session_end_of(&mut acc_events_rx, &b.sid).await,
                DisconnectReason::LocalRequestedLogout
            );
            timeout(TEST_TIMEOUT, shutdown)
                .await
                .expect("shutdown must return once every peer has answered")
                .expect("shutdown task");

            for sid in [&a.sid, &b.sid] {
                acceptor
                    .remove_session(sid)
                    .expect("every session is back in the registry when shutdown returns");
            }
            for peer in [&mut a, &mut b] {
                assert!(
                    peer.read_to_close().await.is_empty(),
                    "nothing follows the completed logout exchange"
                );
            }
            a.handle.await.expect("session task A");
            b.handle.await.expect("session task B");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Application actions end-to-end
// ---------------------------------------------------------------------------

/// What an [`ActionApp`] answers an inbound application message with.
#[derive(Clone, Copy)]
enum AppAction {
    Accept,
    Reject,
    /// `InputAction::Logout { disconnect: false }`: start the logout exchange
    /// and wait for the peer's response.
    LogoutAndWait,
    Disconnect,
}

/// [`TestApp`] whose `on_app_msg_in` answers with the action currently set
/// in `action` - shared with the test, which flips it once the session is up.
struct ActionApp {
    inner: TestApp,
    action: Rc<Cell<AppAction>>,
}

impl Application<Message> for ActionApp {
    fn on_serialize_error(&mut self, msg: Box<Message>, error: &SerializeError) {
        self.inner.on_serialize_error(msg, error);
    }

    async fn on_session_ready(&mut self, session_id: &SessionId, sender: Sender<Message>) {
        self.inner.on_session_ready(session_id, sender).await;
    }

    async fn on_session_end(&mut self, session_id: &SessionId, reason: DisconnectReason) {
        self.inner.on_session_end(session_id, reason).await;
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        // Records the event; the verdict below is ours.
        let _ = self.inner.on_app_msg_in(msg).await;
        match self.action.get() {
            AppAction::Accept => InputAction::Accept,
            AppAction::Reject => InputAction::Reject {
                reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
                text: Some(fix_str!("not today").to_owned()),
                tag: None,
            },
            AppAction::LogoutAndWait => InputAction::Logout {
                session_status: None,
                text: None,
                disconnect: false,
            },
            AppAction::Disconnect => InputAction::Disconnect,
        }
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        self.inner.on_admin_msg_in(msg).await
    }
}

#[derive(Clone)]
struct ActionAppFactory {
    inner: TestAppFactory,
    action: Rc<Cell<AppAction>>,
}

impl ApplicationFactory<Message> for ActionAppFactory {
    type App = ActionApp;

    fn create(&self, ctx: &SessionContext<'_>) -> ActionApp {
        ActionApp {
            inner: self.inner.create(ctx),
            action: self.action.clone(),
        }
    }
}

/// An established acceptor/initiator pair whose applications both start out
/// accepting everything; each side's action can be changed through its cell.
struct ActionPair {
    acceptor: Acceptor<Message, InMemoryStorage, ActionAppFactory>,
    acc_events_rx: mpsc::UnboundedReceiver<TestEvent>,
    acc_action: Rc<Cell<AppAction>>,
    acc_sender: Sender<Message>,
    acc_handle: JoinHandle<()>,
    ini_events_rx: mpsc::UnboundedReceiver<TestEvent>,
    ini_action: Rc<Cell<AppAction>>,
    ini_sender: Sender<Message>,
    ini_handle: JoinHandle<()>,
}

async fn spawn_action_pair() -> ActionPair {
    let (acc_events_tx, mut acc_events_rx) = mpsc::unbounded_channel();
    let (ini_events_tx, mut ini_events_rx) = mpsc::unbounded_channel();
    let acc_action = Rc::new(Cell::new(AppAction::Accept));
    let ini_action = Rc::new(Cell::new(AppAction::Accept));

    let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(ActionAppFactory {
        inner: TestAppFactory {
            events_tx: acc_events_tx,
        },
        action: acc_action.clone(),
    });
    register_in_memory_session(&acceptor, acceptor_session_id(), build_session_settings(30));
    let initiator = Initiator::<Message, InMemoryStorage, _>::new(
        initiator_session_id(),
        build_session_settings(30),
        ActionAppFactory {
            inner: TestAppFactory {
                events_tx: ini_events_tx,
            },
            action: ini_action.clone(),
        },
        |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
    )
    .expect("initiator settings valid");

    let (server_stream, client_stream) = io::duplex(8192);
    let (server_r, server_w) = split(server_stream);
    let (client_r, client_w) = split(client_stream);
    let acc_handle = acceptor.run_session(server_r, server_w, TEST_PEER_ADDR);
    let ini_handle = initiator
        .run_session(client_r, client_w, None)
        .expect("initiator run_session");
    let ini_sender = wait_for_session_ready(&mut ini_events_rx).await;
    let acc_sender = wait_for_session_ready(&mut acc_events_rx).await;
    // The initiator's `SessionReady` fires in `LogonSent`, ahead of the
    // acceptor's Logon response; drain that response's event so the next
    // initiator event a test sees is the one its action caused.
    let _ = wait_until(&mut ini_events_rx, |e| {
        is_admin_msg_in(e, MsgTypeBase::Logon)
    })
    .await;

    ActionPair {
        acceptor,
        acc_events_rx,
        acc_action,
        acc_sender,
        acc_handle,
        ini_events_rx,
        ini_action,
        ini_sender,
        ini_handle,
    }
}

/// `InputAction::Disconnect` from a callback drops the transport with no
/// farewell: the acceptor ends as `ApplicationForcedDisconnect`, and the
/// peer's very next event is the transport going away - no Logout in
/// between.
#[tokio::test]
async fn application_disconnect_drops_the_transport_without_a_farewell() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut p = spawn_action_pair().await;
            p.acc_action.set(AppAction::Disconnect);

            p.ini_sender
                .send(new_order_single_with_empty_header())
                .expect("sender.send");

            assert_matches!(
                recv_event(&mut p.acc_events_rx).await,
                TestEvent::AppMsgIn(..)
            );
            assert_matches!(
                recv_event(&mut p.acc_events_rx).await,
                TestEvent::SessionEnd(_, DisconnectReason::ApplicationForcedDisconnect)
            );
            assert_matches!(
                recv_event(&mut p.ini_events_rx).await,
                TestEvent::SessionEnd(_, DisconnectReason::Disconnected),
                "the peer must see the transport close, with no Logout before it"
            );
            p.acc_handle.await.expect("acceptor task");
            p.ini_handle.await.expect("initiator task");
        })
        .await;
}

/// `InputAction::Logout { disconnect: false }` from a callback runs the full
/// logout exchange (Session Layer §4.6): the peer receives the Logout and
/// answers it, and only then does the connection close - the acceptor as the
/// requesting side, the initiator as the one asked.
#[tokio::test]
async fn application_logout_without_disconnect_completes_the_logout_exchange() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut p = spawn_action_pair().await;
            p.acc_action.set(AppAction::LogoutAndWait);

            p.ini_sender
                .send(new_order_single_with_empty_header())
                .expect("sender.send");

            assert_matches!(
                recv_event(&mut p.acc_events_rx).await,
                TestEvent::AppMsgIn(..)
            );
            let event = recv_event(&mut p.ini_events_rx).await;
            assert!(is_admin_msg_in(&event, MsgTypeBase::Logout), "{event:?}");
            assert_matches!(
                recv_event(&mut p.ini_events_rx).await,
                TestEvent::SessionEnd(_, DisconnectReason::RemoteRequestedLogout)
            );
            // The peer's response is in sequence - the refused order's
            // number was consumed - so it reaches the application as the
            // Logout it is, and only then does the session end.
            let event = recv_event(&mut p.acc_events_rx).await;
            assert!(is_admin_msg_in(&event, MsgTypeBase::Logout), "{event:?}");
            assert_matches!(
                recv_event(&mut p.acc_events_rx).await,
                TestEvent::SessionEnd(_, DisconnectReason::LocalRequestedLogout)
            );
            p.acc_handle.await.expect("acceptor task");
            p.ini_handle.await.expect("initiator task");
        })
        .await;
}

/// The initiator's application is not a passive party: an
/// `InputAction::Reject` from its callback reaches the acceptor as a
/// `Reject<3>`, and the session stays up on both sides.
#[tokio::test]
async fn initiator_application_reject_reaches_the_acceptor() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let mut p = spawn_action_pair().await;
            p.ini_action.set(AppAction::Reject);

            p.acc_sender
                .send(new_order_single_with_empty_header())
                .expect("sender.send");

            assert_matches!(
                recv_event(&mut p.ini_events_rx).await,
                TestEvent::AppMsgIn(..)
            );
            // The Reject carries what the callback put in it.
            let event = recv_event(&mut p.acc_events_rx).await;
            let TestEvent::AdminMsgIn(_, msg) = event else {
                panic!("expected AdminMsgIn, got {event:?}");
            };
            assert_matches!(
                SessionMessage::try_as_admin(&*msg),
                Some(AdminBase::Reject(reject))
                    if reject.session_reject_reason
                        == Some(SessionRejectReasonBase::ValueIsIncorrect.into())
                        && reject.text.as_deref() == Some(fix_str!("not today"))
            );

            // Still up: the acceptor's logout exchange runs to completion.
            p.acceptor
                .logout(&acceptor_session_id(), None, None)
                .await
                .expect("logout");
            assert_eq!(
                wait_for_session_end(&mut p.acc_events_rx).await,
                DisconnectReason::LocalRequestedLogout
            );
            assert_eq!(
                wait_for_session_end(&mut p.ini_events_rx).await,
                DisconnectReason::RemoteRequestedLogout
            );
            p.acc_handle.await.expect("acceptor task");
            p.ini_handle.await.expect("initiator task");
        })
        .await;
}
