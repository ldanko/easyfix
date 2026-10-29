//! FIX initiator - client side of the FIX session layer.
//!
//! The [`Initiator`] manages a single outbound session and spawns a
//! session task on each call to [`connect`](Initiator::connect) /
//! [`run_session`](Initiator::run_session). Each task initializes a handler
//! and runs the session; `connect` also establishes the TCP connection.
//!
//! Reconnection is the caller's responsibility - one-shot per design.
//! After the session task exits, call [`connect`](Initiator::connect) to
//! continue the stored session, or
//! [`connect_with_reset`](Initiator::connect_with_reset) to request a new one.

use std::{
    borrow::Cow, cell::RefCell, error::Error as StdError, future::Future, io, marker::PhantomData,
    net::SocketAddr, rc::Rc,
};

use easyfix_core::{
    basic_types::{FixStr, SessionStatusField},
    message::SessionMessage,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpStream, ToSocketAddrs},
    sync::{Notify, mpsc},
    task::{JoinHandle, spawn_local},
};
use tracing::{Instrument, info, info_span};

use crate::{
    application::{SessionContext, SessionHandler},
    engine::{SessionEngine, supports_seq_num_reset, validate_gap_fill_fits},
    io::{
        ControlMsg, InputStream, SessionOpening, initialize_handler, sender, session_loop,
        time::TimerBackend,
    },
    messages_storage::MessagesStorage,
    session_id::SessionId,
    settings::SessionSettings,
};

// ---------------------------------------------------------------------------
// Session opening
// ---------------------------------------------------------------------------

/// How an initiator opens the FIX session on a new connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStart {
    /// Continue the stored FIX session using its sequence numbers and resend
    /// history. Gaps on either side are recovered through resend as usual.
    Resume,
    /// Reset stored numbering and history, then announce the reset in Logon.
    Reset,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from initiator operations and session startup.
#[derive(Debug, thiserror::Error)]
pub enum InitiatorError {
    /// The session handler could not be initialized.
    #[error("Handler initialization failed: {0}")]
    HandlerInit(#[source] Box<dyn StdError + 'static>),
    /// Opening or resetting the storage backend failed.
    #[error("Storage error: {0}")]
    Storage(#[source] Box<dyn StdError + 'static>),
    /// The local Logon does not preserve `ResetSeqNumFlag(141)`,
    /// required by the reset acceptance setting or requested operation.
    #[error("Logon does not support ResetSeqNumFlag(141)")]
    ResetSeqNumFlagNotSupportedInLogon,
    /// A connection attempt or session is already active. Wait for it to end
    /// before starting another.
    #[error("Session active")]
    SessionActive,
    /// No session is currently active.
    #[error("No active session")]
    NoActiveSession,
    /// Underlying I/O error while establishing the TCP connection.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The session's `max_message_size` cannot hold a `SequenceReset`-GapFill
    /// for its CompIDs, so the session could never gap-fill during resend.
    #[error("max_message_size {configured} too small: gap fill needs about {required} bytes")]
    MaxMessageSizeTooSmall { required: usize, configured: usize },
}

// ---------------------------------------------------------------------------
// Internal shared state
// ---------------------------------------------------------------------------

/// Shared state between the [`Initiator`] public API and the spawned
/// session task. Wrapped in an [`Rc`] so the task can access it after
/// the call site returns the `JoinHandle`.
struct InitiatorInner<M, S, F>
where
    M: SessionMessage,
{
    session_id: SessionId,
    session_settings: SessionSettings,
    timer_backend: TimerBackend,
    // Probe once at construction; reconnects and reset operations reuse it.
    supports_seq_num_reset: bool,
    make_handler: F,
    /// Per-session storage. `Some` when no session task is running,
    /// `None` while a task owns it.
    storage: RefCell<Option<S>>,
    /// Control channel to the currently active session; `None` while no
    /// session is running.
    current_session: RefCell<Option<mpsc::Sender<ControlMsg>>>,
    closed: Notify,
    _phantom: PhantomData<M>,
}

// ---------------------------------------------------------------------------
// Initiator
// ---------------------------------------------------------------------------

/// FIX initiator (client) - manages a single outbound session.
pub struct Initiator<M, S, F>
where
    M: SessionMessage,
    S: MessagesStorage,
{
    inner: Rc<InitiatorInner<M, S, F>>,
}

impl<M, S, F, H> Initiator<M, S, F>
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    F: AsyncFn(&SessionContext<'_>) -> Result<H, Box<dyn StdError + 'static>> + 'static,
    H: SessionHandler<M> + 'static,
{
    /// Create a new `Initiator` for the session identified by `session_id`.
    ///
    /// `make_handler` asynchronously creates a [`SessionHandler`] from the
    /// [`SessionContext`] of each connection, including reconnects.
    /// Neither the closure nor the handler needs to be `Send` or `Sync`.
    /// Errors are reported in the session task's result; see
    /// [`connect`](Self::connect).
    ///
    /// Initialization must tolerate cancellation. Failure or cancellation
    /// produces no handler callbacks. No initialization timeout is imposed;
    /// enforce any required deadline inside the closure.
    ///
    /// `build_storage` is called with the [`SessionId`] and the
    /// `max_message_size` from the supplied [`SessionSettings`].
    ///
    /// Returns [`InitiatorError::MaxMessageSizeTooSmall`] if the settings'
    /// `max_message_size` cannot hold a `SequenceReset`-GapFill for this
    /// session; `build_storage` is not called in that case.
    /// Returns [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`] before
    /// calling `build_storage` if `accept_reset_in_session` is enabled and
    /// the local message type does not preserve `ResetSeqNumFlag(141)`.
    /// `accept_reset_on_connect` does not apply to an initiator.
    /// Returns [`InitiatorError::Storage`] if `build_storage` fails. The factory
    /// must load and validate the backend's counters before returning success.
    pub fn new<B>(
        session_id: SessionId,
        session_settings: SessionSettings,
        make_handler: F,
        build_storage: B,
    ) -> Result<Self, InitiatorError>
    where
        B: FnOnce(&SessionId, usize) -> Result<S, S::Error>,
    {
        Self::with_timer_backend(
            session_id,
            session_settings,
            make_handler,
            build_storage,
            TimerBackend::Tokio,
        )
    }

    /// Create an initiator with busywait timers.
    ///
    /// Every connection uses wall-clock deadlines. Pending timers keep the
    /// executor polling until their deadlines elapse. Arguments and errors
    /// are the same as for [`new`](Self::new).
    #[doc(hidden)]
    pub fn with_busywait_timers<B>(
        session_id: SessionId,
        session_settings: SessionSettings,
        make_handler: F,
        build_storage: B,
    ) -> Result<Self, InitiatorError>
    where
        B: FnOnce(&SessionId, usize) -> Result<S, S::Error>,
    {
        Self::with_timer_backend(
            session_id,
            session_settings,
            make_handler,
            build_storage,
            TimerBackend::Busywait,
        )
    }

    fn with_timer_backend<B>(
        session_id: SessionId,
        session_settings: SessionSettings,
        make_handler: F,
        build_storage: B,
        timer_backend: TimerBackend,
    ) -> Result<Self, InitiatorError>
    where
        B: FnOnce(&SessionId, usize) -> Result<S, S::Error>,
    {
        let max_message_size = usize::from(session_settings.max_message_size.get());
        if let Err(required) = validate_gap_fill_fits::<M>(
            &session_id,
            max_message_size,
            session_settings.time_precision,
        ) {
            return Err(InitiatorError::MaxMessageSizeTooSmall {
                required,
                configured: max_message_size,
            });
        }
        let supports_seq_num_reset = supports_seq_num_reset::<M>(&session_settings);
        if session_settings.accept_reset_in_session && !supports_seq_num_reset {
            return Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon);
        }
        let storage = build_storage(&session_id, max_message_size)
            .map_err(|error| InitiatorError::Storage(Box::new(error)))?;
        Ok(Initiator {
            inner: Rc::new(InitiatorInner {
                session_id,
                session_settings,
                timer_backend,
                supports_seq_num_reset,
                make_handler,
                storage: RefCell::new(Some(storage)),
                current_session: RefCell::new(None),
                closed: Notify::new(),
                _phantom: PhantomData,
            }),
        })
    }

    /// Session identifier for the Initiator's single session.
    pub fn session_id(&self) -> SessionId {
        self.inner.session_id.clone()
    }

    /// Whether a connection attempt or session is active.
    ///
    /// Active immediately after starting a task, including before its first
    /// poll, until it completes or is dropped. This does not mean the Logon
    /// handshake has completed.
    pub fn is_session_active(&self) -> bool {
        self.inner.storage.borrow().is_none()
    }

    /// Reset the session's sequence numbers back to `1` and discard the
    /// messages retained for resend. Returns [`InitiatorError::SessionActive`]
    /// if the session task is still running - the caller must disconnect (or
    /// wait for the task to finish) first.
    ///
    /// Both peers must agree to reset before reconnecting (Session Test Cases
    /// Scenario 9). Sends nothing and does not require tag 141 support.
    /// See [offline resets](crate::session_reset#choosing-a-method).
    /// Returns [`InitiatorError::Storage`] on backend failure; the backend may
    /// be partially changed and must be recovered before reuse.
    pub fn reset_session(&self) -> Result<(), InitiatorError> {
        let mut storage = self.inner.storage.borrow_mut();
        let Some(storage) = storage.as_mut() else {
            return Err(InitiatorError::SessionActive);
        };
        storage
            .reset()
            .map_err(|error| InitiatorError::Storage(Box::new(error)))
    }

    /// Connect to the remote FIX peer via TCP and start the session.
    ///
    /// Reserves the session immediately and spawns a task that connects with
    /// `TCP_NODELAY` enabled. The address must be owned or `'static`.
    ///
    /// The task returns [`InitiatorError::SessionActive`] if already active,
    /// [`InitiatorError::Io`] on TCP failure, or [`InitiatorError::HandlerInit`]
    /// on handler initialization failure. Otherwise it returns `Ok(())`,
    /// including when logout or disconnect cancels connection setup.
    /// Session outcomes after initialization are reported to the handler.
    ///
    /// Reconnection is the caller's responsibility - after the returned
    /// `JoinHandle` resolves you may call `connect` again. Uses the stored
    /// sequence numbers and resend history. To request a new FIX session,
    /// use [`connect_with_reset`](Self::connect_with_reset).
    /// [`close`](Self::close) also cancels a pending connection attempt.
    pub fn connect(
        &self,
        addr: impl ToSocketAddrs + 'static,
    ) -> JoinHandle<Result<(), InitiatorError>> {
        self.connect_impl(addr, SessionStart::Resume)
    }

    /// Connect via TCP and start a new FIX session by requesting a sequence
    /// number reset. Requires the acceptor's agreement.
    ///
    /// The session task resets both counters to 1, discards resend history
    /// and sends Logon with `ResetSeqNumFlag(141)=Y` and `MsgSeqNum(34)=1`
    /// (FIX Session Layer Section 4.4.3). This applies only to this connection
    /// attempt; a later [`connect`](Self::connect) uses the stored state.
    /// See [session resets](crate::session_reset) for configuration and
    /// recovery after failure.
    ///
    /// The task reports the same errors as [`connect`](Self::connect).
    /// Without local tag 141 support, it returns
    /// [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`] before opening
    /// TCP or changing storage.
    pub fn connect_with_reset(
        &self,
        addr: impl ToSocketAddrs + 'static,
    ) -> JoinHandle<Result<(), InitiatorError>> {
        self.connect_impl(addr, SessionStart::Reset)
    }

    fn connect_impl(
        &self,
        addr: impl ToSocketAddrs + 'static,
        start: SessionStart,
    ) -> JoinHandle<Result<(), InitiatorError>> {
        let reserved = self.reserve_session(start);
        let inner = self.inner.clone();
        spawn_local(async move {
            let ReservedSession {
                storage_guard,
                mut control_rx,
            } = reserved?;
            let connecting = TcpStream::connect(addr);
            tokio::pin!(connecting);
            let tcp = loop {
                tokio::select! {
                    biased;
                    control = control_rx.recv() => match control {
                        Some(ControlMsg::ResetRunningSession) => {}
                        Some(ControlMsg::Logout { .. } | ControlMsg::Disconnect) | None => {
                            return Ok(());
                        }
                    },
                    result = &mut connecting => break result?,
                }
            };
            tcp.set_nodelay(true)?;
            let peer_addr = tcp.peer_addr()?;
            let (reader, writer) = tcp.into_split();
            session_future(
                inner,
                reader,
                writer,
                storage_guard,
                Some(peer_addr),
                start,
                control_rx,
            )
            .await
        })
    }

    /// Run a session on an already-established connection (custom
    /// transport, test harness, etc.). Spawns a session task and
    /// returns its [`JoinHandle`].
    /// The task's result has the same meaning as in [`connect`](Self::connect).
    ///
    /// `peer_addr` is reported to the application through
    /// [`SessionContext::peer_addr`]; pass the remote address when the
    /// transport has one (e.g. the `TcpStream` underlying a TLS session) and
    /// `None` when it does not.
    ///
    /// The task returns [`InitiatorError::SessionActive`] if a previous
    /// session task is still running. Uses the stored sequence numbers and
    /// resend history. To request a new FIX session, use
    /// [`run_session_with_reset`](Self::run_session_with_reset).
    ///
    /// [`SessionContext::peer_addr`]: crate::SessionContext::peer_addr
    pub fn run_session<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
    ) -> JoinHandle<Result<(), InitiatorError>>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        self.run_session_impl(reader, writer, peer_addr, SessionStart::Resume)
    }

    /// Spawn a session on an established connection, requesting the same
    /// sequence number reset as [`connect_with_reset`](Self::connect_with_reset).
    /// Requires the acceptor's agreement; the reset applies only to this task.
    ///
    /// Transport arguments and the returned handle have the same meaning as
    /// in [`run_session`](Self::run_session). The task returns
    /// [`InitiatorError::SessionActive`] if a previous task is still running,
    /// or [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`] without local
    /// tag 141 support, before changing storage.
    pub fn run_session_with_reset<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
    ) -> JoinHandle<Result<(), InitiatorError>>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        self.run_session_impl(reader, writer, peer_addr, SessionStart::Reset)
    }

    fn run_session_impl<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
        start: SessionStart,
    ) -> JoinHandle<Result<(), InitiatorError>>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        spawn_local(self.session_task_impl(reader, writer, peer_addr, start))
    }

    /// Build - but do not spawn - the session task future for an
    /// already-established connection. The caller spawns the returned
    /// future on its own executor (`tokio::task::spawn_local`, or any
    /// other single-threaded spawn that accepts a `!Send` future).
    ///
    /// This is the runtime-agnostic counterpart to
    /// [`run_session`](Self::run_session), which spawns on tokio and
    /// returns a [`JoinHandle`]. Use it to drive the session on an
    /// executor where tokio's `spawn_local` / `JoinHandle` are not
    /// available. `peer_addr` carries the same meaning as there.
    ///
    /// The future returns [`InitiatorError::SessionActive`] if already active.
    /// Uses the stored sequence numbers and resend history.
    /// To request a new FIX session, use
    /// [`session_task_with_reset`](Self::session_task_with_reset).
    /// The session becomes active and accepts control requests immediately.
    /// The caller must poll or drop the future for the session to close;
    /// awaiting [`close`](Self::close) does not drive this future.
    ///
    /// The future's output has the same meaning as the task result in
    /// [`connect`](Self::connect). Dropping the future cancels the session
    /// without producing a result.
    pub fn session_task<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
    ) -> impl Future<Output = Result<(), InitiatorError>> + use<M, S, F, H, R, W>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        self.session_task_impl(reader, writer, peer_addr, SessionStart::Resume)
    }

    /// Build, without spawning, a session task that requests the same sequence
    /// number reset as [`connect_with_reset`](Self::connect_with_reset).
    /// Requires the acceptor's agreement; the reset applies only to this task.
    ///
    /// Transport arguments and the returned future have the same meaning as
    /// in [`session_task`](Self::session_task). Building the future performs
    /// no transport IO and does not reset storage; the reset runs when the
    /// future is polled.
    ///
    /// The future returns [`InitiatorError::SessionActive`] if already active,
    /// or [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`] without local
    /// tag 141 support, before changing storage.
    pub fn session_task_with_reset<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
    ) -> impl Future<Output = Result<(), InitiatorError>> + use<M, S, F, H, R, W>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        self.session_task_impl(reader, writer, peer_addr, SessionStart::Reset)
    }

    fn session_task_impl<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
        start: SessionStart,
    ) -> impl Future<Output = Result<(), InitiatorError>> + use<M, S, F, H, R, W>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        let reserved = self.reserve_session(start);
        let inner = self.inner.clone();
        async move {
            let ReservedSession {
                storage_guard,
                control_rx,
            } = reserved?;
            session_future(
                inner,
                reader,
                writer,
                storage_guard,
                peer_addr,
                start,
                control_rx,
            )
            .await
        }
    }

    fn reserve_session(
        &self,
        start: SessionStart,
    ) -> Result<ReservedSession<M, S, F>, InitiatorError> {
        self.check_session_start(start)?;
        let storage = self
            .inner
            .storage
            .borrow_mut()
            .take()
            .ok_or(InitiatorError::SessionActive)?;

        // Wrapped before the future is built, so the storage is under the
        // guard from the moment it leaves the registry - see `StorageReturn`.
        let storage_guard = StorageReturn {
            inner: self.inner.clone(),
            storage: Some(storage),
        };
        // Publish control with the reservation, before the task's first poll,
        // so an immediately following logout or disconnect reaches this task.
        let (control_tx, control_rx) = mpsc::channel::<ControlMsg>(4);
        *self.inner.current_session.borrow_mut() = Some(control_tx);

        Ok(ReservedSession {
            storage_guard,
            control_rx,
        })
    }

    fn check_session_start(&self, start: SessionStart) -> Result<(), InitiatorError> {
        if start == SessionStart::Reset && !self.inner.supports_seq_num_reset {
            return Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon);
        }
        Ok(())
    }

    /// Send a Logout to the current session. Returns `Ok(())` if no session
    /// is active. Completion means the request was submitted, not that the
    /// session has closed; use [`await_session_closed`](Self::await_session_closed)
    /// to wait for closure.
    /// If `disconnect` is true, close after sending Logout without waiting
    /// for the peer's response. Otherwise wait for its response or timeout.
    /// Repeating the request sends no additional Logout. With `disconnect`
    /// true it ends the wait; otherwise the original deadline is unchanged.
    /// Closure without waiting reports
    /// [`ApplicationForcedDisconnect`](crate::SessionEndReason::ApplicationForcedDisconnect).
    /// Cancels pending TCP connection or handler initialization without callbacks.
    pub async fn logout(
        &self,
        session_status: Option<SessionStatusField>,
        text: Option<Cow<'static, FixStr>>,
        disconnect: bool,
    ) -> Result<(), InitiatorError> {
        let tx = self.inner.current_session.borrow().clone();
        if let Some(tx) = tx {
            let _ = tx
                .send(ControlMsg::Logout {
                    session_status,
                    text,
                    disconnect,
                })
                .await;
        }
        Ok(())
    }

    /// Request a sequence number reset over the active connection.
    ///
    /// Agree the timing and initiating side with the peer, which must accept
    /// in-session resets (FIX Session Layer Section 4.4.2).
    ///
    /// Once preparation succeeds, resets the counters and discards resend
    /// history. Queued outgoing application messages remain queued and are
    /// sent in order with new sequence numbers after acknowledgement.
    /// `Ok(())` does not confirm completion; requests outside an established
    /// session or during another reset are ignored.
    ///
    /// See [session resets](crate::session_reset) for preparation, time limits,
    /// callbacks and recovery after failure.
    ///
    /// Returns [`InitiatorError::NoActiveSession`] if no session is running.
    /// Otherwise returns [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`]
    /// before sending the request if the local Logon cannot preserve
    /// `ResetSeqNumFlag(141)`. That refusal leaves the session running.
    pub async fn request_running_session_reset(&self) -> Result<(), InitiatorError> {
        let tx = self
            .inner
            .current_session
            .borrow()
            .clone()
            .ok_or(InitiatorError::NoActiveSession)?;
        if !self.inner.supports_seq_num_reset {
            return Err(InitiatorError::ResetSeqNumFlagNotSupportedInLogon);
        }
        let _ = tx.send(ControlMsg::ResetRunningSession).await;
        Ok(())
    }

    /// Request an immediate disconnect of the current session. Returns
    /// `Ok(())` if no session is active. Does not wait for closure; use
    /// [`close`](Self::close) to disconnect and wait.
    /// Reports [`ApplicationForcedDisconnect`](crate::SessionEndReason::ApplicationForcedDisconnect).
    /// Cancels pending TCP connection or handler initialization without callbacks.
    pub async fn disconnect(&self) -> Result<(), InitiatorError> {
        let tx = self.inner.current_session.borrow().clone();
        if let Some(tx) = tx {
            let _ = tx.send(ControlMsg::Disconnect).await;
        }
        Ok(())
    }

    /// Await full closure of the session, including its end callback and
    /// transport teardown. Once closed, its storage can be reused or reset.
    /// Resolves immediately if no session is active.
    ///
    /// Observes whether the session is currently inactive, not the end of a
    /// particular connection. Stop reconnecting first to prevent a new task
    /// from taking its place.
    ///
    /// Must be awaited on the same single-threaded executor that drives the
    /// session, outside its own application callbacks. Awaiting from one of
    /// those callbacks deadlocks. A future returned by
    /// [`session_task`](Self::session_task) must be driven or dropped by its
    /// owner; waiting here does not run it.
    pub async fn await_session_closed(&self) {
        loop {
            // Register before inspecting the durable state. Every waiter must
            // recheck it after a wakeup: another task may already own storage.
            let notified = self.inner.closed.notified();
            if !self.is_session_active() {
                return;
            }
            notified.await;
        }
    }

    /// Force-disconnect the session and await its full closure. Returns
    /// `Ok(())` immediately if no session is active.
    ///
    /// For a graceful logout, call [`logout`](Self::logout) followed by
    /// [`await_session_closed`](Self::await_session_closed) instead. The
    /// latter's callback and reconnect caveats also apply here. Cancels a
    /// pending TCP connection attempt, but does not prevent later connections.
    pub async fn close(&self) -> Result<(), InitiatorError> {
        self.disconnect().await?;
        self.await_session_closed().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

struct ReservedSession<M: SessionMessage, S, F> {
    storage_guard: StorageReturn<M, S, F>,
    control_rx: mpsc::Receiver<ControlMsg>,
}

/// RAII return of the storage taken out of [`InitiatorInner`], plus the
/// teardown of the control handle. Mirrors the acceptor's guard of the same
/// name.
///
/// Restores `storage` and clears `current_session` on EVERY exit - normal
/// return, early return, or panic unwind - so a panicking session task can
/// never permanently lose the sequence counters and the messages retained
/// for resend, which would leave every later `connect()` returning
/// [`InitiatorError::SessionActive`] with no session running.
//
// Created by `reserve_session` and captured before the task's first poll,
// so dropping an unpolled task also releases the reservation.
//
// The acceptor takes its storage inside the future instead, because it
// cannot know which session a connection belongs to until the first Logon
// is parsed. The initiator reserves its known session immediately so control
// requests also reach tasks that have not been polled yet.
struct StorageReturn<M, S, F>
where
    M: SessionMessage,
{
    inner: Rc<InitiatorInner<M, S, F>>,
    /// `Some` for the whole life of the guard; `Option` only so `Drop` can
    /// move the storage back out. Borrow it through [`Self::storage`].
    storage: Option<S>,
}

impl<M, S, F> StorageReturn<M, S, F>
where
    M: SessionMessage,
{
    /// The guarded storage.
    fn storage(&mut self) -> &mut S {
        // Unreachable: the field is `Some` from construction until `drop`,
        // which is the only place that takes it, and `drop` runs after every
        // borrow handed out here has ended.
        self.storage
            .as_mut()
            .expect("storage present for the life of the guard")
    }
}

impl<M, S, F> Drop for StorageReturn<M, S, F>
where
    M: SessionMessage,
{
    fn drop(&mut self) {
        *self.inner.current_session.borrow_mut() = None;
        if let Some(storage) = self.storage.take() {
            *self.inner.storage.borrow_mut() = Some(storage);
        }
        // Wake every waiter only after restoring the durable inactive state
        // and releasing the borrows, including on cancellation and unwind.
        self.inner.closed.notify_waiters();
    }
}

/// Build the instrumented session task future without spawning it.
///
/// The caller must have already taken the storage out of the `Initiator` and
/// wrapped it in a [`StorageReturn`], which hands it back when the future
/// ends or is dropped.
fn session_future<M, S, F, H, R, W>(
    inner: Rc<InitiatorInner<M, S, F>>,
    reader: R,
    writer: W,
    storage_guard: StorageReturn<M, S, F>,
    peer_addr: Option<SocketAddr>,
    start: SessionStart,
    control_rx: mpsc::Receiver<ControlMsg>,
) -> impl Future<Output = Result<(), InitiatorError>> + 'static
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    F: AsyncFn(&SessionContext<'_>) -> Result<H, Box<dyn StdError + 'static>> + 'static,
    H: SessionHandler<M> + 'static,
    R: AsyncRead + Unpin + 'static,
    W: AsyncWrite + Unpin + 'static,
{
    // Root span - the session is not nested under a connection span. The
    // peer address is bound to the session id once, by the "session
    // connected" log below, instead of riding along on every event.
    let span = info_span!(parent: None, "session", id = %inner.session_id);
    initiator_session_task(
        inner,
        reader,
        writer,
        storage_guard,
        peer_addr,
        start,
        control_rx,
    )
    .instrument(span)
}

async fn initiator_session_task<M, S, F, H, R, W>(
    inner: Rc<InitiatorInner<M, S, F>>,
    reader: R,
    writer: W,
    mut storage_guard: StorageReturn<M, S, F>,
    peer_addr: Option<SocketAddr>,
    start: SessionStart,
    mut control_rx: mpsc::Receiver<ControlMsg>,
) -> Result<(), InitiatorError>
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    F: AsyncFn(&SessionContext<'_>) -> Result<H, Box<dyn StdError + 'static>>,
    H: SessionHandler<M>,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Binds the peer address to the session id (carried by the span) once,
    // at the start of the session. `peer_addr` is `None` for transports
    // without a socket address.
    info!(?peer_addr, "session connected");

    let session_id = &inner.session_id;
    let time_precision = inner.session_settings.time_precision;
    let context = SessionContext::new(session_id, peer_addr, time_precision);
    let initialized = initialize_handler((inner.make_handler)(&context), &mut control_rx).await;
    let handler = match initialized {
        Ok(Some(handler)) => handler,
        result => {
            drop(reader);
            drop(writer);
            return result.map(|_| ()).map_err(InitiatorError::HandlerInit);
        }
    };
    let engine = SessionEngine::<M>::new(
        session_id.clone(),
        inner.session_settings.clone(),
        inner.timer_backend,
    );

    let max_message_size = engine.session_settings().max_message_size;
    let (sender_tx, app_rx) = sender::channel::<M>(inner.timer_backend);
    let input = InputStream::<R, M>::new(reader, max_message_size);
    session_loop(
        SessionOpening::SendLogon(start),
        input,
        writer,
        engine,
        storage_guard.storage(),
        handler,
        sender_tx,
        app_rx,
        control_rx,
    )
    .await;

    // The control handle is cleared and the storage returned by
    // `StorageReturn::drop`, which runs on this path and on every failure
    // path alike.
    info!("session task finished");
    Ok(())
}

#[cfg(test)]
mod tests;
