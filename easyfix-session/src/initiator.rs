//! FIX initiator - client side of the FIX session layer.
//!
//! The [`Initiator`] manages a single outbound session and spawns a
//! session task on each call to [`connect`](Initiator::connect) /
//! [`run_session`](Initiator::run_session). The task sends a Logon
//! request immediately, then runs the session.
//!
//! Reconnection is the caller's responsibility - one-shot per design.
//! After the session task exits, call [`connect`](Initiator::connect) to
//! continue the stored session, or
//! [`connect_with_reset`](Initiator::connect_with_reset) to request a new one.

use std::{
    cell::RefCell, error::Error as StdError, future::Future, io, marker::PhantomData,
    net::SocketAddr, rc::Rc,
};

use easyfix_core::{
    basic_types::{FixString, SessionStatusField},
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
    application::{ApplicationFactory, SessionContext},
    engine::{SessionEngine, supports_seq_num_reset, validate_gap_fill_fits},
    io::{ControlMsg, InputStream, SessionOpening, sender, session_loop, time::TimerBackend},
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

/// Errors returned by [`Initiator`] methods.
#[derive(Debug, thiserror::Error)]
pub enum InitiatorError {
    /// Opening or resetting the storage backend failed.
    #[error("Storage error: {0}")]
    Storage(#[source] Box<dyn StdError + 'static>),
    /// The local Logon does not preserve `ResetSeqNumFlag(141)`,
    /// required by the reset acceptance setting or requested operation.
    #[error("Logon does not support ResetSeqNumFlag(141)")]
    ResetSeqNumFlagNotSupportedInLogon,
    /// A session is already running. Wait for it to exit before
    /// restarting it.
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
struct InitiatorInner<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    session_id: SessionId,
    session_settings: SessionSettings,
    timer_backend: TimerBackend,
    // Probe once at construction; reconnects and reset operations reuse it.
    supports_seq_num_reset: bool,
    app_factory: A,
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
pub struct Initiator<M, S, A>
where
    M: SessionMessage,
    S: MessagesStorage,
    A: ApplicationFactory<M>,
{
    inner: Rc<InitiatorInner<M, S, A>>,
}

impl<M, S, A> Initiator<M, S, A>
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M> + 'static,
    A::App: 'static,
{
    /// Create a new `Initiator` for the session identified by `session_id`.
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
    pub fn new<F>(
        session_id: SessionId,
        session_settings: SessionSettings,
        app_factory: A,
        build_storage: F,
    ) -> Result<Self, InitiatorError>
    where
        F: FnOnce(&SessionId, usize) -> Result<S, S::Error>,
    {
        Self::with_timer_backend(
            session_id,
            session_settings,
            app_factory,
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
    pub fn with_busywait_timers<F>(
        session_id: SessionId,
        session_settings: SessionSettings,
        app_factory: A,
        build_storage: F,
    ) -> Result<Self, InitiatorError>
    where
        F: FnOnce(&SessionId, usize) -> Result<S, S::Error>,
    {
        Self::with_timer_backend(
            session_id,
            session_settings,
            app_factory,
            build_storage,
            TimerBackend::Busywait,
        )
    }

    fn with_timer_backend<F>(
        session_id: SessionId,
        session_settings: SessionSettings,
        app_factory: A,
        build_storage: F,
        timer_backend: TimerBackend,
    ) -> Result<Self, InitiatorError>
    where
        F: FnOnce(&SessionId, usize) -> Result<S, S::Error>,
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
                app_factory,
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

    /// Whether a session task currently owns the session.
    ///
    /// Active from creation of a [`session_task`](Self::session_task) until
    /// it completes or is dropped, including before its first poll. This
    /// does not mean the Logon handshake has completed. A pending TCP
    /// connection attempt alone does not make the session active.
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
    /// Opens a `TcpStream` to `addr` with `TCP_NODELAY` enabled and
    /// spawns the session task. Returns the task's [`JoinHandle`].
    ///
    /// Returns [`InitiatorError::SessionActive`] if a session is still
    /// running, or [`InitiatorError::Io`] on TCP failure.
    ///
    /// Reconnection is the caller's responsibility - after the returned
    /// `JoinHandle` resolves you may call `connect` again. Uses the stored
    /// sequence numbers and resend history. To request a new FIX session,
    /// use [`connect_with_reset`](Self::connect_with_reset).
    /// A pending connection attempt is not an active session and is not
    /// cancelled by [`close`](Self::close). Cancel pending attempts and stop
    /// reconnecting before closing the session for a scheduled break.
    pub async fn connect(
        &self,
        addr: impl ToSocketAddrs,
    ) -> Result<JoinHandle<()>, InitiatorError> {
        self.connect_impl(addr, SessionStart::Resume).await
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
    /// Returns the same errors as [`connect`](Self::connect). Without local
    /// tag 141 support, returns
    /// [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`] before opening
    /// TCP or changing storage.
    pub async fn connect_with_reset(
        &self,
        addr: impl ToSocketAddrs,
    ) -> Result<JoinHandle<()>, InitiatorError> {
        self.connect_impl(addr, SessionStart::Reset).await
    }

    async fn connect_impl(
        &self,
        addr: impl ToSocketAddrs,
        start: SessionStart,
    ) -> Result<JoinHandle<()>, InitiatorError> {
        self.check_session_start(start)?;
        let tcp = TcpStream::connect(addr).await?;
        tcp.set_nodelay(true)?;
        // Resolved before the split so the session's `SessionContext` can
        // report the address actually connected to (`ToSocketAddrs` may
        // resolve to several candidates). A failure here means the peer is
        // already gone (the connection was reset between `connect` returning
        // and this call), so fail the connect rather than start a session on
        // a dead socket - and keep `peer_addr` unconditional for this path.
        let peer_addr = tcp.peer_addr()?;
        let (reader, writer) = tcp.into_split();
        self.run_session_impl(reader, writer, Some(peer_addr), start)
    }

    /// Run a session on an already-established connection (custom
    /// transport, test harness, etc.). Spawns a session task and
    /// returns its [`JoinHandle`].
    ///
    /// `peer_addr` is reported to the application through
    /// [`SessionContext::peer_addr`]; pass the remote address when the
    /// transport has one (e.g. the `TcpStream` underlying a TLS session) and
    /// `None` when it does not.
    ///
    /// Returns [`InitiatorError::SessionActive`] if a previous
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
    ) -> Result<JoinHandle<()>, InitiatorError>
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
    /// in [`run_session`](Self::run_session). Returns
    /// [`InitiatorError::SessionActive`] if a previous task is still running,
    /// or [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`] without local
    /// tag 141 support, before changing storage or starting a task.
    pub fn run_session_with_reset<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
    ) -> Result<JoinHandle<()>, InitiatorError>
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
    ) -> Result<JoinHandle<()>, InitiatorError>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        Ok(spawn_local(
            self.session_task_impl(reader, writer, peer_addr, start)?,
        ))
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
    /// Returns [`InitiatorError::SessionActive`] if a previous session
    /// is still running. Uses the stored sequence numbers and resend history.
    /// To request a new FIX session, use
    /// [`session_task_with_reset`](Self::session_task_with_reset).
    /// The session becomes active and accepts control requests immediately.
    /// The caller must poll or drop the future for the session to close;
    /// awaiting [`close`](Self::close) does not drive this future.
    pub fn session_task<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
    ) -> Result<impl Future<Output = ()> + use<M, S, A, R, W>, InitiatorError>
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
    /// Returns [`InitiatorError::SessionActive`] if a previous task is still
    /// running, or [`InitiatorError::ResetSeqNumFlagNotSupportedInLogon`]
    /// without local tag 141 support, before changing storage or building
    /// the future.
    pub fn session_task_with_reset<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: Option<SocketAddr>,
    ) -> Result<impl Future<Output = ()> + use<M, S, A, R, W>, InitiatorError>
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
    ) -> Result<impl Future<Output = ()> + use<M, S, A, R, W>, InitiatorError>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
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

        Ok(session_future(
            self.inner.clone(),
            reader,
            writer,
            storage_guard,
            peer_addr,
            start,
            control_rx,
        ))
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
    /// Repeating the request during logout sends no additional Logout and
    /// does not change the original acknowledgement deadline.
    pub async fn logout(
        &self,
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    ) -> Result<(), InitiatorError> {
        let tx = self.inner.current_session.borrow().clone();
        if let Some(tx) = tx {
            let _ = tx
                .send(ControlMsg::Logout {
                    session_status,
                    text,
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
    /// particular connection. Stop reconnecting and cancel pending TCP
    /// connection attempts first to prevent a new task from taking its place.
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
    /// latter's callback and reconnect caveats also apply here. This does not
    /// prevent later connections or cancel a pending TCP connection attempt.
    pub async fn close(&self) -> Result<(), InitiatorError> {
        self.disconnect().await?;
        self.await_session_closed().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

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
// Built in `session_task`, before the future exists, and handed to it as an
// argument. That covers the second leak too: an `async fn` future owns the
// arguments it was called with, so dropping a returned-but-never-polled
// `session_task()` future drops this guard and gives the storage back.
//
// The acceptor takes its storage inside the future instead, because it
// cannot know which session a connection belongs to until the first Logon
// is parsed. The initiator names its session up front, which is what lets
// `session_task` report `SessionActive` synchronously - hence the earlier
// take. Different constraints, not an inconsistency.
struct StorageReturn<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    inner: Rc<InitiatorInner<M, S, A>>,
    /// `Some` for the whole life of the guard; `Option` only so `Drop` can
    /// move the storage back out. Borrow it through [`Self::storage`].
    storage: Option<S>,
}

impl<M, S, A> StorageReturn<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
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

impl<M, S, A> Drop for StorageReturn<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
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
fn session_future<M, S, A, R, W>(
    inner: Rc<InitiatorInner<M, S, A>>,
    reader: R,
    writer: W,
    storage_guard: StorageReturn<M, S, A>,
    peer_addr: Option<SocketAddr>,
    start: SessionStart,
    control_rx: mpsc::Receiver<ControlMsg>,
) -> impl Future<Output = ()> + 'static
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M> + 'static,
    A::App: 'static,
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

async fn initiator_session_task<M, S, A, R, W>(
    inner: Rc<InitiatorInner<M, S, A>>,
    reader: R,
    writer: W,
    mut storage_guard: StorageReturn<M, S, A>,
    peer_addr: Option<SocketAddr>,
    start: SessionStart,
    control_rx: mpsc::Receiver<ControlMsg>,
) where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M>,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Binds the peer address to the session id (carried by the span) once,
    // at the start of the session. `peer_addr` is `None` for transports
    // without a socket address.
    info!(?peer_addr, "session connected");

    let session_id = &inner.session_id;
    let time_precision = inner.session_settings.time_precision;
    let app = inner
        .app_factory
        .create(&SessionContext::new(session_id, peer_addr, time_precision));
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
        app,
        sender_tx,
        app_rx,
        control_rx,
    )
    .await;

    // The control handle is cleared and the storage returned by
    // `StorageReturn::drop`, which runs on this path and on every failure
    // path alike.
    info!("session task finished");
}

#[cfg(test)]
mod tests;
