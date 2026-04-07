//! Integration tests for `manages_admin_output = true`.
//!
//! When this flag is set, engine-produced admin messages are NOT sent to TCP.
//! The `on_admin_msg_out` callback fires - the app reads the message fields -
//! but the message is dropped after the callback returns. The app must re-send
//! via `Sender` for the message to reach the wire.
//!
//! These tests use a raw peer (manual FIX byte construction) driving one side
//! and an Acceptor with a custom `ManagedAdminApp` on the other.

use std::{cell::RefCell, collections::HashSet, fmt, time::Duration};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::{FixStr, Int, NonZeroSeqNum, SeqNum},
    fix_str,
    message::{HeaderAccess, SessionMessage},
    version::Version,
};
use easyfix_session::{
    Acceptor, Application, ApplicationFactory, DisconnectReason, InMemoryStorage,
    InMemoryStorageError, InputAction, MessagesStorage, Sender, SerializeError, SessionContext,
    SessionId,
};
use easyfix_test_messages::{Body, Message};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, duplex, split},
    sync::mpsc,
    task::LocalSet,
    time::{advance, timeout},
};

#[path = "common/fixtures.rs"]
mod common;
use common::{
    TEST_PEER_ADDR, TEST_TIMEOUT, build_session_settings, new_order_single_with_empty_header,
    read_one_message,
};

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

/// Short timeout for asserting "nothing on the wire".
const NO_DATA_TIMEOUT: Duration = Duration::from_millis(50);

fn acceptor_session_id() -> SessionId {
    SessionId::new(
        Version::FIXT11,
        fix_str!("SENDER").to_owned(),
        fix_str!("TARGET").to_owned(),
    )
}

fn is_new_order_single(msg: &Message) -> bool {
    matches!(&*msg.body, Body::NewOrderSingle(_))
}

// ---------------------------------------------------------------------------
// Raw-peer helpers
// ---------------------------------------------------------------------------

fn peer_sid() -> SessionId {
    acceptor_session_id().reverse_route()
}

// Wrappers around the shared peer builders with the session id pinned to
// this file's single acceptor session.

fn peer_logon_bytes(seq: SeqNum, heart_bt_int: Int) -> Vec<u8> {
    common::peer_logon_bytes(&peer_sid(), seq, heart_bt_int)
}

fn peer_test_request_bytes(seq: SeqNum, test_req_id: &'static FixStr) -> Vec<u8> {
    common::peer_test_request_bytes(&peer_sid(), seq, test_req_id)
}

fn peer_heartbeat_bytes(seq: SeqNum, test_req_id: Option<&'static FixStr>) -> Vec<u8> {
    common::peer_heartbeat_bytes(&peer_sid(), seq, test_req_id)
}

fn peer_logout_bytes(seq: SeqNum) -> Vec<u8> {
    common::peer_logout_bytes(&peer_sid(), seq)
}

/// Whether the peer stream yields any bytes within a short timeout. Whatever
/// arrives is appended to `buf`, where the next `read_one_message` finds it,
/// so a positive answer costs the test nothing.
///
/// Meant for the paused clock: the timeout then elapses only once every task
/// is idle, so `false` says the session task ran out of work without writing,
/// not merely that it had not been scheduled yet.
async fn has_data_on_wire<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut Vec<u8>) -> bool {
    let mut tmp = [0u8; 1024];
    match timeout(NO_DATA_TIMEOUT, reader.read(&mut tmp)).await {
        Ok(Ok(n)) if n > 0 => {
            buf.extend_from_slice(&tmp[..n]);
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// ManagedEvent - what the test harness observes from the app
// ---------------------------------------------------------------------------

/// Events emitted by `ManagedAdminApp` for the test harness to observe.
enum ManagedEvent {
    SessionReady(SessionId, Sender<Message>),
    SessionEnd(SessionId, DisconnectReason),
    AppMsgIn(SessionId, Box<Message>),
    /// An engine-produced admin message was intercepted in `on_admin_msg_out`.
    /// The clone is ready to be re-sent via `Sender` by the test harness.
    AdminIntercepted(Box<Message>),
}

// `Sender<M>` does not implement `Debug`, so hand-roll it.
impl fmt::Debug for ManagedEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManagedEvent::SessionReady(id, _) => f.debug_tuple("SessionReady").field(id).finish(),
            ManagedEvent::SessionEnd(id, reason) => {
                f.debug_tuple("SessionEnd").field(id).field(reason).finish()
            }
            ManagedEvent::AppMsgIn(id, msg) => f
                .debug_tuple("AppMsgIn")
                .field(id)
                .field(&SessionMessage::name(msg.as_ref()))
                .finish(),
            ManagedEvent::AdminIntercepted(msg) => f
                .debug_tuple("AdminIntercepted")
                .field(&SessionMessage::name(msg.as_ref()))
                .finish(),
        }
    }
}

// ---------------------------------------------------------------------------
// Event-drain helpers
// ---------------------------------------------------------------------------

async fn recv_event(rx: &mut mpsc::UnboundedReceiver<ManagedEvent>) -> ManagedEvent {
    timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("timeout waiting for ManagedEvent")
        .expect("events channel closed")
}

async fn wait_until<F>(
    rx: &mut mpsc::UnboundedReceiver<ManagedEvent>,
    mut predicate: F,
) -> ManagedEvent
where
    F: FnMut(&ManagedEvent) -> bool,
{
    loop {
        let event = recv_event(rx).await;
        if predicate(&event) {
            return event;
        }
    }
}

async fn wait_for_session_ready(rx: &mut mpsc::UnboundedReceiver<ManagedEvent>) -> Sender<Message> {
    match wait_until(rx, |e| matches!(e, ManagedEvent::SessionReady(..))).await {
        ManagedEvent::SessionReady(_, sender) => sender,
        _ => unreachable!(),
    }
}

async fn wait_for_session_end(rx: &mut mpsc::UnboundedReceiver<ManagedEvent>) -> DisconnectReason {
    match wait_until(rx, |e| matches!(e, ManagedEvent::SessionEnd(..))).await {
        ManagedEvent::SessionEnd(_, reason) => reason,
        _ => unreachable!(),
    }
}

/// Wait for an `AdminIntercepted` event and return the cloned message.
async fn wait_for_intercepted(rx: &mut mpsc::UnboundedReceiver<ManagedEvent>) -> Box<Message> {
    match wait_until(rx, |e| matches!(e, ManagedEvent::AdminIntercepted(..))).await {
        ManagedEvent::AdminIntercepted(msg) => msg,
        _ => unreachable!(),
    }
}

/// Re-issue an intercepted admin message via the `Sender` (a plain
/// application send - not a FIX resend). Marks the seq num as confirmed so
/// the second `on_admin_msg_out` pass is a no-op.
///
/// The seq num goes out on `confirm_tx` before the send - this is how the
/// test harness communicates the "bus confirmation" to the `ManagedAdminApp`
/// instance. Since we can't reach into the app directly (it's inside the
/// session task), we use a channel for the confirmation set updates.
fn reissue_intercepted(
    sender: &Sender<Message>,
    msg: Box<Message>,
    confirm_tx: &mpsc::UnboundedSender<SeqNum>,
) {
    let seq = msg.msg_seq_num();
    let _ = confirm_tx.send(seq);
    sender.send(msg).expect("resend intercepted admin msg");
}

// ---------------------------------------------------------------------------
// ManagedAdminApp - intercepts engine-produced admin messages
// ---------------------------------------------------------------------------

/// Application that intercepts engine-produced admin messages.
///
/// On each `on_admin_msg_out` call, drains a confirmation channel to learn
/// which seq nums the test harness has already re-sent. First-pass messages
/// (not yet confirmed) are cloned and forwarded to the test harness via
/// `events_tx`. Second-pass messages (confirmed) are silently accepted.
struct ManagedAdminApp {
    session_id: SessionId,
    events_tx: mpsc::UnboundedSender<ManagedEvent>,
    confirm_rx: mpsc::UnboundedReceiver<SeqNum>,
    confirmed: HashSet<SeqNum>,
}

impl ManagedAdminApp {
    /// Drain any pending confirmations from the channel into the set.
    fn drain_confirmations(&mut self) {
        while let Ok(seq) = self.confirm_rx.try_recv() {
            self.confirmed.insert(seq);
        }
    }
}

impl Application<Message> for ManagedAdminApp {
    fn on_serialize_error(&mut self, _msg: Box<Message>, _error: &SerializeError) {}

    async fn on_session_ready(&mut self, session_id: &SessionId, sender: Sender<Message>) {
        let _ = self
            .events_tx
            .send(ManagedEvent::SessionReady(session_id.clone(), sender));
    }

    async fn on_session_end(&mut self, session_id: &SessionId, reason: DisconnectReason) {
        let _ = self
            .events_tx
            .send(ManagedEvent::SessionEnd(session_id.clone(), reason));
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        let _ = self
            .events_tx
            .send(ManagedEvent::AppMsgIn(self.session_id.clone(), msg));
        InputAction::Accept
    }

    fn on_admin_msg_out(&mut self, msg: &mut Message) {
        self.drain_confirmations();
        let seq = msg.msg_seq_num();
        if self.confirmed.remove(&seq) {
            // Second pass: re-sent by test harness after "bus confirmation".
            return;
        }
        // First pass: engine-produced. Intercept for test harness.
        let _ = self
            .events_tx
            .send(ManagedEvent::AdminIntercepted(Box::new(msg.clone())));
    }
}

struct ManagedAdminAppFactory {
    events_tx: mpsc::UnboundedSender<ManagedEvent>,
    confirm_rx_slot: RefCell<Option<mpsc::UnboundedReceiver<SeqNum>>>,
}

impl ApplicationFactory<Message> for ManagedAdminAppFactory {
    type App = ManagedAdminApp;

    fn create(&self, ctx: &SessionContext<'_>) -> ManagedAdminApp {
        let confirm_rx = self
            .confirm_rx_slot
            .borrow_mut()
            .take()
            .expect("ManagedAdminAppFactory::create called more than once");
        ManagedAdminApp {
            session_id: ctx.session_id().clone(),
            events_tx: self.events_tx.clone(),
            confirm_rx,
            confirmed: HashSet::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Test setup helper
// ---------------------------------------------------------------------------

struct TestHarness {
    acceptor: Acceptor<Message, InMemoryStorage, ManagedAdminAppFactory>,
    events_rx: mpsc::UnboundedReceiver<ManagedEvent>,
    confirm_tx: mpsc::UnboundedSender<SeqNum>,
}

fn build_harness(heartbeat_secs: u16) -> TestHarness {
    build_harness_with_storage(heartbeat_secs, |_, max_message_size| {
        Ok(InMemoryStorage::new(max_message_size))
    })
}

fn build_harness_with_storage<F>(heartbeat_secs: u16, build_storage: F) -> TestHarness
where
    F: FnOnce(&SessionId, usize) -> Result<InMemoryStorage, InMemoryStorageError> + 'static,
{
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();

    let factory = ManagedAdminAppFactory {
        events_tx,
        confirm_rx_slot: RefCell::new(Some(confirm_rx)),
    };

    let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(factory);
    // These tests are specifically about `manages_admin_output = true`; flip it
    // here so the flag-under-test stays visible at the call site.
    let mut settings = build_session_settings(heartbeat_secs);
    settings.manages_admin_output = true;
    acceptor
        .register_session(acceptor_session_id(), settings, build_storage)
        .unwrap();

    TestHarness {
        acceptor,
        events_rx,
        confirm_tx,
    }
}

/// Complete the logon handshake for a `manages_admin_output` acceptor:
///
/// 1. Peer sends Logon
/// 2. App intercepts the Logon response (nothing on wire)
/// 3. Test re-sends the Logon response via Sender
/// 4. Peer reads the Logon response
///
/// Returns the Sender for further interaction.
///
/// Setup only - the asserts here are structural guards that fail fast when
/// the handshake derails. The logon interception behaviour itself is
/// verified inline by `logon_and_logout`.
async fn do_managed_logon<R, W>(
    peer_r: &mut R,
    peer_w: &mut W,
    peer_buf: &mut Vec<u8>,
    events_rx: &mut mpsc::UnboundedReceiver<ManagedEvent>,
    confirm_tx: &mpsc::UnboundedSender<SeqNum>,
    heart_bt_int: Int,
) -> Sender<Message>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Peer sends Logon.
    peer_w
        .write_all(&peer_logon_bytes(1, heart_bt_int))
        .await
        .expect("write peer logon");

    // App intercepts the Logon response.
    let logon_resp = wait_for_intercepted(events_rx).await;
    assert_eq!(
        SessionMessage::msg_type(&*logon_resp),
        MsgTypeBase::Logon,
        "first intercepted admin message should be Logon response"
    );

    // Nothing on the wire yet.
    assert!(
        !has_data_on_wire(peer_r, peer_buf).await,
        "Logon response should NOT appear on wire before re-send"
    );

    // Get the Sender from SessionReady.
    let sender = wait_for_session_ready(events_rx).await;

    // Re-send the intercepted Logon response.
    reissue_intercepted(&sender, logon_resp, confirm_tx);

    // Peer reads the Logon response.
    let msg = read_one_message(peer_r, peer_buf).await;
    assert_eq!(
        SessionMessage::msg_type(&*msg),
        MsgTypeBase::Logon,
        "peer should receive Logon response after re-send"
    );

    sender
}

// ---------------------------------------------------------------------------
// Test 1: Logon and Logout intercepted and re-sent
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn logon_and_logout() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let TestHarness {
                acceptor,
                mut events_rx,
                confirm_tx,
            } = build_harness(30);

            let (acc_stream, peer_stream) = duplex(8192);
            let (acc_r, acc_w) = split(acc_stream);
            let (mut peer_r, mut peer_w) = split(peer_stream);

            let acc_handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
            let mut peer_buf: Vec<u8> = Vec::new();

            // --- Logon handshake (managed) ---

            // Peer sends Logon.
            peer_w
                .write_all(&peer_logon_bytes(1, 30))
                .await
                .expect("write peer logon");

            // App intercepts the Logon response.
            let logon_resp = wait_for_intercepted(&mut events_rx).await;
            assert_eq!(
                SessionMessage::msg_type(&*logon_resp),
                MsgTypeBase::Logon,
                "first intercepted admin message should be Logon response"
            );

            // Nothing on the wire yet.
            assert!(
                !has_data_on_wire(&mut peer_r, &mut peer_buf).await,
                "Logon response should NOT appear on wire before re-send"
            );

            // Get the Sender from SessionReady.
            let sender = wait_for_session_ready(&mut events_rx).await;

            // Re-send the intercepted Logon response.
            reissue_intercepted(&sender, logon_resp, &confirm_tx);

            // Peer reads the Logon response.
            let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*msg),
                MsgTypeBase::Logon,
                "peer should receive Logon response after re-send"
            );

            // --- Initiate Logout via control channel ---
            acceptor
                .logout(&acceptor_session_id(), None, None)
                .await
                .expect("logout");

            // App intercepts the Logout message.
            let logout_msg = wait_for_intercepted(&mut events_rx).await;
            assert_eq!(
                SessionMessage::msg_type(&*logout_msg),
                MsgTypeBase::Logout,
                "intercepted admin message should be Logout"
            );

            // Nothing on the wire yet.
            assert!(
                !has_data_on_wire(&mut peer_r, &mut peer_buf).await,
                "Logout should NOT appear on wire before re-send"
            );

            // Re-send the Logout.
            reissue_intercepted(&sender, logout_msg, &confirm_tx);

            // Peer reads the Logout.
            let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*msg),
                MsgTypeBase::Logout,
                "peer should receive Logout after re-send"
            );

            // Peer responds with Logout to complete the handshake.
            peer_w
                .write_all(&peer_logout_bytes(2))
                .await
                .expect("write peer logout");

            // Session should end cleanly.
            let reason = wait_for_session_end(&mut events_rx).await;
            assert_eq!(reason, DisconnectReason::LocalRequestedLogout);

            acc_handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 2: Heartbeat response to TestRequest intercepted and re-sent,
//         with app messages interleaved
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn heartbeat_response_to_test_request() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let TestHarness {
                acceptor,
                mut events_rx,
                confirm_tx,
            } = build_harness(30);

            let (acc_stream, peer_stream) = duplex(8192);
            let (acc_r, acc_w) = split(acc_stream);
            let (mut peer_r, mut peer_w) = split(peer_stream);

            let acc_handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
            let mut peer_buf: Vec<u8> = Vec::new();

            let sender = do_managed_logon(
                &mut peer_r,
                &mut peer_w,
                &mut peer_buf,
                &mut events_rx,
                &confirm_tx,
                30,
            )
            .await;

            // --- App message: should go through unaffected ---
            sender
                .send(new_order_single_with_empty_header())
                .expect("send NOS");

            let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert!(
                is_new_order_single(&msg),
                "app message should reach wire regardless of manages_admin_output"
            );

            // --- Peer sends TestRequest ---
            peer_w
                .write_all(&peer_test_request_bytes(2, fix_str!("TR1")))
                .await
                .expect("write test request");

            // App intercepts the Heartbeat response.
            let hb_msg = wait_for_intercepted(&mut events_rx).await;
            assert_eq!(
                SessionMessage::msg_type(&*hb_msg),
                MsgTypeBase::Heartbeat,
                "intercepted message should be Heartbeat response"
            );
            // Verify it carries the TestReqID.
            match SessionMessage::try_as_admin(&*hb_msg) {
                Some(AdminBase::Heartbeat(hb)) => {
                    assert_eq!(
                        hb.test_req_id.as_deref(),
                        Some(fix_str!("TR1")),
                        "Heartbeat should echo TestReqID"
                    );
                }
                _ => panic!("expected Heartbeat admin base"),
            }

            // Nothing on wire yet.
            assert!(
                !has_data_on_wire(&mut peer_r, &mut peer_buf).await,
                "Heartbeat should NOT appear on wire before re-send"
            );

            // --- Send another app message while Heartbeat is pending ---
            sender
                .send(new_order_single_with_empty_header())
                .expect("send second NOS");

            let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert!(
                is_new_order_single(&msg),
                "second app message should reach wire while admin is pending"
            );

            // --- Now re-send the Heartbeat ---
            reissue_intercepted(&sender, hb_msg, &confirm_tx);

            let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*msg),
                MsgTypeBase::Heartbeat,
                "peer should receive Heartbeat after re-send"
            );

            // --- Teardown ---
            acceptor
                .disconnect(&acceptor_session_id())
                .await
                .expect("disconnect");
            assert_eq!(
                wait_for_session_end(&mut events_rx).await,
                DisconnectReason::Disconnected
            );
            acc_handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 3: Outbound Heartbeat on idle (output timeout) intercepted
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn outbound_heartbeat_on_idle() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let TestHarness {
                acceptor,
                mut events_rx,
                confirm_tx,
            } = build_harness(1); // 1-second heartbeat for fast timeout

            let (acc_stream, peer_stream) = duplex(8192);
            let (acc_r, acc_w) = split(acc_stream);
            let (mut peer_r, mut peer_w) = split(peer_stream);

            let acc_handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
            let mut peer_buf: Vec<u8> = Vec::new();

            let sender = do_managed_logon(
                &mut peer_r,
                &mut peer_w,
                &mut peer_buf,
                &mut events_rx,
                &confirm_tx,
                1,
            )
            .await;

            // Advance time past the output deadline (1 heartbeat interval).
            // The engine should produce a Heartbeat via on_output_timeout.
            advance(Duration::from_millis(1100)).await;

            // The peer must also stay "alive" - send a Heartbeat to prevent
            // the acceptor's input timeout from firing and producing extra
            // admin messages (TestRequest).
            peer_w
                .write_all(&peer_heartbeat_bytes(2, None))
                .await
                .expect("write peer heartbeat");

            // App intercepts the outbound Heartbeat.
            let hb_msg = wait_for_intercepted(&mut events_rx).await;
            assert_eq!(
                SessionMessage::msg_type(&*hb_msg),
                MsgTypeBase::Heartbeat,
                "intercepted message should be Heartbeat from output timeout"
            );
            // Idle heartbeat has no TestReqID.
            match SessionMessage::try_as_admin(&*hb_msg) {
                Some(AdminBase::Heartbeat(hb)) => {
                    assert!(
                        hb.test_req_id.is_none(),
                        "idle Heartbeat should not carry TestReqID"
                    );
                }
                _ => panic!("expected Heartbeat admin base"),
            }

            // Re-send it.
            reissue_intercepted(&sender, hb_msg, &confirm_tx);

            let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*msg),
                MsgTypeBase::Heartbeat,
                "peer should receive Heartbeat after re-send"
            );

            // --- Teardown ---
            acceptor
                .disconnect(&acceptor_session_id())
                .await
                .expect("disconnect");
            assert_eq!(
                wait_for_session_end(&mut events_rx).await,
                DisconnectReason::Disconnected
            );
            acc_handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 4: ResendRequest on sequence gap intercepted and re-sent
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn resend_request_on_sequence_gap_is_intercepted() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let TestHarness {
                acceptor,
                mut events_rx,
                confirm_tx,
            } = build_harness(30);

            let (acc_stream, peer_stream) = duplex(8192);
            let (acc_r, acc_w) = split(acc_stream);
            let (mut peer_r, mut peer_w) = split(peer_stream);

            let acc_handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
            let mut peer_buf: Vec<u8> = Vec::new();

            let sender = do_managed_logon(
                &mut peer_r,
                &mut peer_w,
                &mut peer_buf,
                &mut events_rx,
                &confirm_tx,
                30,
            )
            .await;

            // Peer sends a message with seq=5, skipping 2-4.
            // The engine should detect the gap and produce a ResendRequest.
            peer_w
                .write_all(&peer_test_request_bytes(5, fix_str!("GAP1")))
                .await
                .expect("write gap message");

            // App intercepts the ResendRequest.
            let rr_msg = wait_for_intercepted(&mut events_rx).await;
            assert_eq!(
                SessionMessage::msg_type(&*rr_msg),
                MsgTypeBase::ResendRequest,
                "intercepted message should be ResendRequest"
            );
            // Verify range covers the gap.
            match SessionMessage::try_as_admin(&*rr_msg) {
                Some(AdminBase::ResendRequest(rr)) => {
                    assert_eq!(rr.begin_seq_no, 2, "ResendRequest should start at seq 2");
                    assert_eq!(rr.end_seq_no, 4, "ResendRequest should end at seq 4");
                }
                _ => panic!("expected ResendRequest admin base"),
            }

            // Nothing on wire yet.
            assert!(
                !has_data_on_wire(&mut peer_r, &mut peer_buf).await,
                "ResendRequest should NOT appear on wire before re-send"
            );

            // Re-send the ResendRequest.
            reissue_intercepted(&sender, rr_msg, &confirm_tx);

            // Peer reads the ResendRequest.
            let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
            assert_eq!(
                SessionMessage::msg_type(&*msg),
                MsgTypeBase::ResendRequest,
                "peer should receive ResendRequest after re-send"
            );

            // --- Teardown ---
            acceptor
                .disconnect(&acceptor_session_id())
                .await
                .expect("disconnect");
            assert_eq!(
                wait_for_session_end(&mut events_rx).await,
                DisconnectReason::Disconnected
            );
            acc_handle.await.expect("acceptor task");
        })
        .await;
}

// ---------------------------------------------------------------------------
// Test 5: App messages are unaffected by manages_admin_output
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn app_messages_unaffected() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let TestHarness {
                acceptor,
                mut events_rx,
                confirm_tx,
            } = build_harness(30);

            let (acc_stream, peer_stream) = duplex(8192);
            let (acc_r, acc_w) = split(acc_stream);
            let (mut peer_r, mut peer_w) = split(peer_stream);

            let acc_handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);
            let mut peer_buf: Vec<u8> = Vec::new();

            let sender = do_managed_logon(
                &mut peer_r,
                &mut peer_w,
                &mut peer_buf,
                &mut events_rx,
                &confirm_tx,
                30,
            )
            .await;

            // Send multiple app messages - all should go directly to TCP.
            const N: u32 = 5;
            for _ in 0..N {
                sender
                    .send(new_order_single_with_empty_header())
                    .expect("send NOS");
            }

            for i in 0..N {
                let msg = read_one_message(&mut peer_r, &mut peer_buf).await;
                assert!(
                    is_new_order_single(&msg),
                    "message {i} should be NewOrderSingle on the wire"
                );
                assert_ne!(
                    msg.poss_dup_flag(),
                    Some(true),
                    "app messages should not have PossDupFlag=Y"
                );
            }

            // Verify no extra data on wire (only app messages, no
            // leaked admin messages).
            assert!(
                !has_data_on_wire(&mut peer_r, &mut peer_buf).await,
                "no extra data should appear on wire after app messages"
            );

            // --- Teardown ---
            acceptor
                .disconnect(&acceptor_session_id())
                .await
                .expect("disconnect");
            assert_eq!(
                wait_for_session_end(&mut events_rx).await,
                DisconnectReason::Disconnected
            );
            acc_handle.await.expect("acceptor task");
        })
        .await;
}

/// `manages_admin_output` does not change where the sequence number comes
/// from: the drain stamps the header before the interception callback, so an
/// exhausted outgoing numbering drops the message on this path too - the app
/// never sees it, and there is nothing for it to re-send.
#[tokio::test(start_paused = true)]
async fn exhausted_outgoing_numbering_drops_the_admin_message_before_interception() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let TestHarness {
                acceptor,
                mut events_rx,
                confirm_tx: _confirm_tx,
            } = build_harness_with_storage(30, |_id, max_message_size| {
                let mut storage = InMemoryStorage::new(max_message_size);
                storage
                    .set_next_sender_msg_seq_num(
                        NonZeroSeqNum::new(SeqNum::MAX).expect("MAX is non-zero"),
                    )
                    .unwrap();
                Ok(storage)
            });

            let (acc_stream, peer_stream) = duplex(8192);
            let (acc_r, acc_w) = split(acc_stream);
            let (mut peer_r, mut peer_w) = split(peer_stream);
            let mut peer_buf: Vec<u8> = Vec::new();

            let acc_handle = acceptor.run_session(acc_r, acc_w, TEST_PEER_ADDR);

            peer_w
                .write_all(&peer_logon_bytes(1, 30))
                .await
                .expect("write peer logon");

            let end = timeout(TEST_TIMEOUT, wait_for_session_end(&mut events_rx))
                .await
                .expect("session must end when nothing can be stamped");
            assert_eq!(end, DisconnectReason::SeqNumExhausted);

            assert!(
                !has_data_on_wire(&mut peer_r, &mut peer_buf).await,
                "nothing may reach the wire"
            );

            acc_handle.await.expect("session task");
        })
        .await;
}
