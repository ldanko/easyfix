//! FIX acceptor - server side of the FIX session layer.
//!
//! The [`Acceptor`] manages a set of registered sessions and spawns one
//! task per incoming connection. Each connection reads its first message
//! (Logon), looks up the registered session by the derived
//! [`SessionId`], and constructs a per-session
//! [`Application`](crate::Application) via the user-provided
//! [`ApplicationFactory`] to serve it.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, hash_map::Entry},
    error::Error as StdError,
    future::Future,
    io,
    marker::PhantomData,
    net::SocketAddr,
    rc::Rc,
    time::Duration,
};

use easyfix_core::{
    base_messages::MsgTypeBase,
    basic_types::{FixString, SessionStatusField},
    message::{DeserializeError, SessionMessage},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    sync::{Notify, mpsc},
    task::{JoinHandle, spawn_local},
};
use tracing::{Instrument, error, info, info_span, warn};

use crate::{
    application::{ApplicationFactory, SessionContext},
    engine::{SessionEngine, supports_seq_num_reset, validate_gap_fill_fits},
    io::{
        ControlMsg, FirstMessageEvent, InputStream, SessionOpening, sender, session_loop,
        time::TimerBackend,
    },
    messages_storage::MessagesStorage,
    session_id::SessionId,
    settings::{AcceptorSettings, SessionSettings},
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Errors returned by [`Acceptor`] management methods.
#[derive(Debug, thiserror::Error)]
pub enum AcceptorError {
    /// Opening or resetting the storage backend failed.
    #[error("Storage error: {0}")]
    Storage(#[source] Box<dyn StdError + 'static>),
    /// The local Logon does not preserve `ResetSeqNumFlag(141)`,
    /// required by the reset acceptance setting or requested operation.
    #[error("Logon does not support ResetSeqNumFlag(141)")]
    ResetSeqNumFlagNotSupportedInLogon,
    /// No registered session matches the supplied [`SessionId`].
    #[error("Unknown session")]
    UnknownSession,
    /// A session with the same [`SessionId`] is already registered.
    #[error("Session already registered")]
    AlreadyRegistered,
    /// The operation requires the session to be inactive, but it is
    /// currently running.
    #[error("Session active")]
    SessionActive,
    /// The operation requires an active session, but it is not running.
    #[error("Session inactive")]
    SessionInactive,
    /// The session's `max_message_size` cannot hold a `SequenceReset`-GapFill
    /// for its CompIDs, so the session could never gap-fill during resend.
    #[error("max_message_size {configured} too small: gap fill needs about {required} bytes")]
    MaxMessageSizeTooSmall { required: usize, configured: usize },
}

/// How the [`Acceptor`] should terminate active sessions during shutdown.
#[derive(Debug)]
pub enum ShutdownMode {
    /// Immediate disconnect without sending Logout. Violates FIX protocol
    /// rules - use only in emergencies.
    Disconnect,
    /// Send Logout, then disconnect immediately without waiting for the
    /// peer's Logout response.
    LogoutAndDisconnect {
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    },
    /// Send Logout, wait for the peer's Logout response (up to each
    /// session's logout deadline), then disconnect. Standard FIX
    /// graceful logout.
    /// If logout is already in progress, sends no additional Logout and
    /// preserves the original acknowledgement deadline.
    GracefulLogout {
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    },
}

/// Why an inbound connection ended before it became a FIX session.
///
/// Reported through [`ConnectionObserver::on_connection_dropped`]. Distinct
/// from [`DisconnectReason`](crate::DisconnectReason), which describes the end
/// of an *established* session: here no session ever came into being, so no
/// `Application` exists and nothing was sent on the wire.
///
/// The variants carrying a [`SessionId`] are those where the peer's identity
/// was already derived from its `Logon<A>`; the rest occur before any identity
/// is known.
///
/// `#[non_exhaustive]`: a downstream `match` must include a wildcard arm.
//
// New variants land here as the acceptor grows; keeping the enum
// non-exhaustive is what makes that a non-breaking change.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConnectionDropReason<'a> {
    /// The acceptor is suspended via [`suspend`](Acceptor::suspend), so the
    /// connection was dropped before anything was read from it - no identity
    /// is known.
    AcceptorSuspended,
    /// The peer closed the connection without sending anything.
    ClosedBeforeFirstMessage,
    /// No message arrived within `auto_disconnect_after_no_logon_received`.
    LogonTimeout,
    /// A transport read failed before the first message was complete.
    FirstMessageIoError(&'a io::Error),
    /// The first message could not be decoded and carried no usable identity
    /// to answer to, so it was dropped silently (FIX Session Layer
    /// Test Cases Scenario 2(d)/2S).
    FirstMessageUndecodable(&'a DeserializeError),
    /// The first message declared a length above `max_first_message_size`.
    /// Dropped without a reply and without reading the message: no identity
    /// is known yet, and reading on would let an unauthenticated peer size
    /// the buffer.
    FirstMessageTooLarge,
    /// The first message decoded cleanly but was not a `Logon<A>`. Dropped
    /// without a reply and without touching the session registry, so a
    /// stray frame cannot take the storage of the session its CompIDs name
    /// (FIX Session Layer Test Cases Scenario 2S).
    FirstMessageNotLogon,
    /// The `Logon<A>` resolved to a [`SessionId`] that is not registered.
    /// Dropped without a reply so as not to reveal which identities are
    /// valid (FIX Session Layer §4.6.4).
    ///
    /// A burst of *distinct* unregistered ids from one address is the
    /// signature of CompID enumeration; a single id repeating is more often a
    /// misconfigured counterparty.
    UnknownSession(&'a SessionId),
    /// A session is already running for this [`SessionId`]. Dropped without a
    /// reply, since a `Logout<5>` would consume a `MsgSeqNum(34)` and disturb
    /// the live session (FIX Session Layer §4.6.4).
    SessionAlreadyActive(&'a SessionId),
    /// The session is registered but suspended via
    /// [`suspend_session`](Acceptor::suspend_session), so inbound connections
    /// for its identity are refused until it is resumed.
    SessionSuspended(&'a SessionId),
}

/// Notified when an inbound connection ends before becoming a FIX session.
///
/// Register with
/// [`set_connection_observer`](Acceptor::set_connection_observer). Every
/// connection the [`Acceptor`] handles is covered, whether it arrived through
/// a [`Connection`] listener or was supplied directly via
/// [`run_session`](Acceptor::run_session) /
/// [`session_task`](Acceptor::session_task).
///
/// The acceptor reports only connections dropped for reasons attributable to
/// the peer or to acceptor / session state - not those torn down by
/// [`shutdown`](Acceptor::shutdown), which the caller initiated and already
/// knows about.
///
/// Callbacks run inline in the session task. They may call back into the
/// `Acceptor`, but should return promptly - a slow observer delays the
/// connection's teardown.
pub trait ConnectionObserver {
    /// An inbound connection from `peer_addr` was dropped for `reason`.
    fn on_connection_dropped(&self, peer_addr: SocketAddr, reason: ConnectionDropReason<'_>);
}

/// Abstraction over a listening connection source.
///
/// Implemented by [`TcpConnection`] for TCP servers; implement it to
/// feed custom transports (e.g. test harnesses) into the [`Acceptor`].
#[expect(async_fn_in_trait, reason = "single-threaded runtime, Send not needed")]
pub trait Connection {
    /// Wait for the next inbound connection.
    ///
    /// **Must be cancel-safe.** [`Acceptor::shutdown`] drops the returned
    /// future where it stands, so anything an implementation does inside
    /// `accept` - a TLS handshake, reading a PROXY-protocol header, draining an
    /// internal backlog - has to survive that drop, either by being restartable
    /// or by living in `&mut self` rather than in the future.
    async fn accept(
        &mut self,
    ) -> Result<
        (
            impl AsyncRead + Unpin + 'static,
            impl AsyncWrite + Unpin + 'static,
            SocketAddr,
        ),
        io::Error,
    >;
}

/// TCP implementation of [`Connection`]. Binds a [`TcpListener`] on
/// the configured address and yields read/write halves of each
/// accepted socket with `TCP_NODELAY` enabled.
pub struct TcpConnection {
    listener: TcpListener,
}

impl TcpConnection {
    /// Bind a TCP listener on the given address.
    pub async fn new(socket_addr: impl Into<SocketAddr>) -> Result<TcpConnection, io::Error> {
        let listener = TcpListener::bind(socket_addr.into()).await?;
        Ok(TcpConnection { listener })
    }
}

impl Connection for TcpConnection {
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
        let (tcp_stream, peer_addr) = self.listener.accept().await?;
        tcp_stream.set_nodelay(true)?;
        let (reader, writer) = tcp_stream.into_split();
        Ok((reader, writer, peer_addr))
    }
}

// ---------------------------------------------------------------------------
// Internal registry
// ---------------------------------------------------------------------------

/// Per-session entry held by the [`Acceptor`], persisting for the whole
/// registered lifetime of a [`SessionId`] (across reconnects) until
/// [`remove_session`](Acceptor::remove_session).
struct RegisteredSession<S> {
    session_settings: SessionSettings,
    // Probe once per registration; reconnects and reset operations reuse it.
    supports_seq_num_reset: bool,
    /// `None` while a session task is currently running (the task owns the
    /// storage) and `Some` otherwise. `storage.is_none()` therefore doubles as
    /// the "session is active" flag - no separate active set is needed for
    /// duplicate-logon detection.
    storage: Option<S>,
    /// Per-session close signal: fired by [`StorageReturn::drop`] when the
    /// session task fully exits and awaited by
    /// [`await_session_closed`](Acceptor::await_session_closed). The same
    /// `Rc<Notify>` identity persists across reconnects.
    //
    // It lives in this PERSISTENT entry, not in `active_sessions`, because
    // clearing `active_sessions` IS the close event - the signal has to
    // outlive it.
    closed: Rc<Notify>,
    /// Gates the inbound logon path: while `true` the acceptor rejects new
    /// connections for this id (storage is never taken). Independent of
    /// [`AcceptorInner::suspended`], which gates every connection before its
    /// identity is even read.
    suspended: bool,
}

/// Shared state between the [`Acceptor`] public API and spawned session
/// tasks. Wrapped in an [`Rc`] so session tasks can outlive the call
/// site that spawned them.
struct AcceptorInner<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    settings: AcceptorSettings,
    timer_backend: TimerBackend,
    app_factory: A,
    sessions: RefCell<HashMap<SessionId, RegisteredSession<S>>>,
    active_sessions: RefCell<HashMap<SessionId, mpsc::Sender<ControlMsg>>>,
    /// Shutdown flag. `true` stops the listener from spawning new
    /// session tasks (connections arriving after the flag is set are
    /// dropped) and makes a session task racing the shutdown bail out
    /// before running the session loop. One-way: [`Acceptor::shutdown`] is
    /// terminal, so nothing ever clears it.
    stopping: Cell<bool>,
    /// Suspension flag, the reversible half of `stopping`: while `true` every
    /// new connection is dropped before its first message is read. Read once,
    /// at the start of each session task; a connection past that point
    /// completes its handshake regardless. Running sessions are untouched.
    suspended: Cell<bool>,
    /// Notified when `stopping` is set, so the two places that would
    /// otherwise block indefinitely - the listener parked in `accept()` and a
    /// connection waiting for its first message - can leave at once.
    stop_notify: Notify,
    /// Count of running tasks [`Acceptor::shutdown`] waits for: one per
    /// connection, plus the listener while it runs. See [`TaskGuard`].
    active_count: Cell<usize>,
    /// Notified when `active_count` reaches zero.
    //
    // This is how `shutdown` awaits all session tasks exiting without relying
    // on `JoinHandle`, which isn't available on every supported runtime.
    all_exited: Notify,
    /// Optional user hook for connections that die before becoming sessions.
    //
    // Not a generic parameter: it is set after construction and would
    // otherwise infect every `Acceptor` type annotation with a fourth
    // parameter.
    connection_observer: RefCell<Option<Rc<dyn ConnectionObserver>>>,
    _phantom: PhantomData<M>,
}

impl<M, S, A> AcceptorInner<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    fn new(settings: AcceptorSettings, app_factory: A, timer_backend: TimerBackend) -> Self {
        AcceptorInner {
            settings,
            timer_backend,
            app_factory,
            sessions: RefCell::new(HashMap::new()),
            active_sessions: RefCell::new(HashMap::new()),
            stopping: Cell::new(false),
            suspended: Cell::new(false),
            stop_notify: Notify::new(),
            active_count: Cell::new(0),
            all_exited: Notify::new(),
            connection_observer: RefCell::new(None),
            _phantom: PhantomData,
        }
    }

    /// Resolve once [`Acceptor::shutdown`] has been called - immediately if it
    /// already has.
    //
    // The flag check before awaiting is what makes "already has" work:
    // `notify_waiters` only wakes waiters registered at the time it runs, and
    // `run_session` / `session_task` can start a connection after the flag is
    // set. Same double-check as the `all_exited` wait in `shutdown`, and it
    // needs no loop because `stopping` is one-way - once `notified` resolves,
    // the flag is set for good.
    async fn stopped(&self) {
        if self.stopping.get() {
            return;
        }
        let notified = self.stop_notify.notified();
        if self.stopping.get() {
            return;
        }
        notified.await;
    }

    /// Report a dropped connection to the registered observer, if any.
    //
    // The handle is cloned out before the call so no `RefCell` borrow is live
    // while user code runs - the observer is documented as free to call back
    // into the `Acceptor`.
    fn notify_connection_dropped(&self, peer_addr: SocketAddr, reason: ConnectionDropReason<'_>) {
        let observer = self.connection_observer.borrow().clone();
        if let Some(observer) = observer {
            observer.on_connection_dropped(peer_addr, reason);
        }
    }
}

/// RAII guard tracking a task that [`Acceptor::shutdown`] must wait for - a
/// session task, or the listener. Constructed before the task is spawned (the
/// count is already incremented when `spawn_local` returns, so a racing
/// `shutdown` cannot miss the task), dropped when the task exits (including
/// panic). Decrements `active_count` and fires `all_exited` when the count
/// reaches zero.
//
// Constructing it at the spawn site rather than inside the task is what makes
// the listener's guard meaningful: a guard taken on the task's first poll
// would let a `shutdown` issued right after `start` see a zero count and
// return with the address still bound.
struct TaskGuard<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    inner: Rc<AcceptorInner<M, S, A>>,
}

impl<M, S, A> TaskGuard<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    fn new(inner: Rc<AcceptorInner<M, S, A>>) -> Self {
        inner.active_count.set(inner.active_count.get() + 1);
        TaskGuard { inner }
    }
}

impl<M, S, A> Drop for TaskGuard<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    fn drop(&mut self) {
        let next = self.inner.active_count.get() - 1;
        self.inner.active_count.set(next);
        if next == 0 {
            self.inner.all_exited.notify_waiters();
        }
    }
}

/// Returns a session's storage to the registry on drop - including a panic
/// unwind - so a panicking session task can never permanently lose the
/// storage (and the NextNumOut/NextNumIn counters it holds) or leave the
/// `SessionId` stuck as a phantom "active" session, locking out every
/// reconnecting peer (FIX Session Layer §4.1, "Sequence numbers"). Clears the
/// `active_sessions` control-channel entry in the same step.
///
/// The session loop borrows the storage via [`Self::storage`] for its
/// lifetime; the guard owns it and hands it back to `reg.storage`.
struct StorageReturn<M, S, A>
where
    M: SessionMessage,
    A: ApplicationFactory<M>,
{
    inner: Rc<AcceptorInner<M, S, A>>,
    session_id: SessionId,
    /// `Some` for the whole life of the guard; `Option` only so `Drop` can
    /// move the storage back out. Borrow it through [`Self::storage`].
    storage: Option<S>,
    /// Clone of the registry entry's per-session close signal, owned by the
    /// guard so [`Self::drop`] can fire it without a fresh `sessions` lookup.
    closed: Rc<Notify>,
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
        self.inner
            .active_sessions
            .borrow_mut()
            .remove(&self.session_id);
        if let Some(storage) = self.storage.take()
            && let Some(reg) = self.inner.sessions.borrow_mut().get_mut(&self.session_id)
        {
            reg.storage = Some(storage);
        }
        // Fire the close signal LAST, after every `sessions` / `active_sessions`
        // borrow above has been released and after the durable inactive flag
        // (storage restored to `Some`) is written. Order is load-bearing: a
        // waiter woken by this edge re-reads `storage.is_some()` and must
        // observe the restored value. `notify_waiters` only schedules wakeups
        // (it does not poll waiters inline), so on the single-threaded runtime
        // no waiter resumes mid-drop and re-enters a borrow. Mirrors
        // `TaskGuard::drop` firing `all_exited` after its own mutation.
        self.closed.notify_waiters();
    }
}

// ---------------------------------------------------------------------------
// Acceptor
// ---------------------------------------------------------------------------

/// FIX acceptor (server) - manages a set of registered sessions and
/// spawns one task per incoming connection.
pub struct Acceptor<M, S, A>
where
    M: SessionMessage,
    S: MessagesStorage,
    A: ApplicationFactory<M>,
{
    inner: Rc<AcceptorInner<M, S, A>>,
}

impl<M, S, A> Clone for Acceptor<M, S, A>
where
    M: SessionMessage,
    S: MessagesStorage,
    A: ApplicationFactory<M>,
{
    fn clone(&self) -> Self {
        Acceptor {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<M, S, A> Acceptor<M, S, A>
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M> + 'static,
    A::App: 'static,
{
    /// Create a new `Acceptor` with default [`AcceptorSettings`]. No
    /// sessions are registered initially - call
    /// [`register_session`](Self::register_session) to add them.
    pub fn new(app_factory: A) -> Self {
        Self::with_settings(AcceptorSettings::default(), app_factory)
    }

    /// Create a new `Acceptor` with the given listener-level settings
    /// and [`ApplicationFactory`]. No sessions are registered
    /// initially - call [`register_session`](Self::register_session)
    /// to add them.
    pub fn with_settings(settings: AcceptorSettings, app_factory: A) -> Self {
        Self::with_timer_backend(settings, app_factory, TimerBackend::Tokio)
    }

    /// Create an acceptor with the given settings and busywait timers.
    ///
    /// All connections and sessions use wall-clock deadlines. Pending timers
    /// keep the executor polling until their deadlines elapse.
    #[doc(hidden)]
    pub fn with_busywait_timers(settings: AcceptorSettings, app_factory: A) -> Self {
        Self::with_timer_backend(settings, app_factory, TimerBackend::Busywait)
    }

    fn with_timer_backend(
        settings: AcceptorSettings,
        app_factory: A,
        timer_backend: TimerBackend,
    ) -> Self {
        Acceptor {
            inner: Rc::new(AcceptorInner::new(settings, app_factory, timer_backend)),
        }
    }

    /// Register a [`ConnectionObserver`] to be notified of inbound
    /// connections that end before becoming sessions - unregistered
    /// identities, duplicate connections, undecodable first messages and the
    /// like.
    ///
    /// Replaces any previously registered observer. May be called before or
    /// after [`start`](Self::start); connections already in flight pick up
    /// the new observer at their next reported event.
    pub fn set_connection_observer(&self, observer: Rc<dyn ConnectionObserver>) {
        *self.inner.connection_observer.borrow_mut() = Some(observer);
    }

    /// Register a session under the given [`SessionId`] with its settings
    /// and a closure that constructs the per-session [`MessagesStorage`].
    ///
    /// The closure is called with the [`SessionId`] and the
    /// `max_message_size` from the session's [`SessionSettings`].
    ///
    /// Returns [`AcceptorError::AlreadyRegistered`] if a session with the same
    /// id is already registered; in that case `build_storage` is not called.
    /// To replace an existing session, [`remove_session`](Self::remove_session)
    /// it first. Returns [`AcceptorError::MaxMessageSizeTooSmall`] if the
    /// settings' `max_message_size` cannot hold a `SequenceReset`-GapFill for
    /// this session.
    /// Returns [`AcceptorError::ResetSeqNumFlagNotSupportedInLogon`] before
    /// calling `build_storage` or registering the session if either
    /// `accept_reset_on_connect` or `accept_reset_in_session` is enabled
    /// and the local message type does not preserve `ResetSeqNumFlag(141)`.
    /// Returns [`AcceptorError::Storage`] if `build_storage` fails, without
    /// registering the session. The factory must load and validate counters
    /// before returning success.
    pub fn register_session<F>(
        &self,
        session_id: SessionId,
        session_settings: SessionSettings,
        build_storage: F,
    ) -> Result<(), AcceptorError>
    where
        F: FnOnce(&SessionId, usize) -> Result<S, S::Error>,
    {
        let max_message_size = usize::from(session_settings.max_message_size.get());
        if let Err(required) = validate_gap_fill_fits::<M>(
            &session_id,
            max_message_size,
            session_settings.time_precision,
        ) {
            return Err(AcceptorError::MaxMessageSizeTooSmall {
                required,
                configured: max_message_size,
            });
        }
        match self.inner.sessions.borrow_mut().entry(session_id) {
            Entry::Occupied(_) => Err(AcceptorError::AlreadyRegistered),
            Entry::Vacant(slot) => {
                let supports_seq_num_reset = supports_seq_num_reset::<M>(&session_settings);
                if (session_settings.accept_reset_on_connect
                    || session_settings.accept_reset_in_session)
                    && !supports_seq_num_reset
                {
                    return Err(AcceptorError::ResetSeqNumFlagNotSupportedInLogon);
                }
                // `build_storage` runs only on the vacant path - never built and
                // discarded on a duplicate. It is handed only `&SessionId` and
                // `max_message_size`, so it has no handle to re-enter the
                // acceptor while this borrow is held.
                let storage = build_storage(slot.key(), max_message_size)
                    .map_err(|error| AcceptorError::Storage(Box::new(error)))?;
                slot.insert(RegisteredSession {
                    session_settings,
                    supports_seq_num_reset,
                    storage: Some(storage),
                    closed: Rc::new(Notify::new()),
                    suspended: false,
                });
                Ok(())
            }
        }
    }

    /// Remove a registered session and return its [`MessagesStorage`].
    ///
    /// Returns [`AcceptorError::SessionActive`] if the session is currently
    /// running - drain it first (see [`close`](Self::close) /
    /// [`await_session_closed`](Self::await_session_closed)) - and
    /// [`AcceptorError::UnknownSession`] if no matching session is registered.
    ///
    /// Returns the storage without resetting or validating its contents.
    pub fn remove_session(&self, session_id: &SessionId) -> Result<S, AcceptorError> {
        let mut sessions = self.inner.sessions.borrow_mut();
        let Entry::Occupied(mut entry) = sessions.entry(session_id.clone()) else {
            return Err(AcceptorError::UnknownSession);
        };
        // `storage` is `Some` only while the session is inactive. Taking it
        // yields the storage and lets us drop the whole entry; `None` means a
        // task still owns it, so refuse without disturbing the entry. Dropping
        // the entry's `closed` `Rc<Notify>` here is safe: any in-flight
        // `await_session_closed` waiter holds its own clone (so the `Notify`
        // outlives the entry by refcount), and the `SessionActive` guard means
        // no waiter can be parked-on-active when a removal succeeds.
        match entry.get_mut().storage.take() {
            Some(storage) => {
                entry.remove();
                Ok(storage)
            }
            None => Err(AcceptorError::SessionActive),
        }
    }

    /// Suspend a registered session: until [`resume_session`](Self::resume_session)
    /// the acceptor refuses every new inbound connection for this id, without
    /// a reply and without touching the session's storage.
    ///
    /// This gates new connections only. A session that is already running
    /// keeps running; ending it is a separate step
    /// ([`logout`](Self::logout) or [`disconnect`](Self::disconnect)).
    ///
    /// Independent of [`suspend`](Self::suspend), which suspends the whole
    /// acceptor.
    ///
    /// Returns [`AcceptorError::UnknownSession`] if no matching session is
    /// registered.
    pub fn suspend_session(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        let mut sessions = self.inner.sessions.borrow_mut();
        let reg = sessions
            .get_mut(session_id)
            .ok_or(AcceptorError::UnknownSession)?;
        reg.suspended = true;
        Ok(())
    }

    /// Re-enable inbound connections for a session previously suspended with
    /// [`suspend_session`](Self::suspend_session). Has no effect on a
    /// suspended acceptor - see [`resume`](Self::resume) for that.
    ///
    /// Returns [`AcceptorError::UnknownSession`] if no matching session is
    /// registered.
    pub fn resume_session(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        let mut sessions = self.inner.sessions.borrow_mut();
        let reg = sessions
            .get_mut(session_id)
            .ok_or(AcceptorError::UnknownSession)?;
        reg.suspended = false;
        Ok(())
    }

    /// Query whether the given session is suspended via
    /// [`suspend_session`](Self::suspend_session). Reports the session's own
    /// flag only; a suspended acceptor refuses its connections all the same.
    ///
    /// Returns [`AcceptorError::UnknownSession`] if no matching session is
    /// registered.
    pub fn is_session_suspended(&self, session_id: &SessionId) -> Result<bool, AcceptorError> {
        self.inner
            .sessions
            .borrow()
            .get(session_id)
            .map(|reg| reg.suspended)
            .ok_or(AcceptorError::UnknownSession)
    }

    /// Suspend the acceptor: until [`resume`](Self::resume) every new inbound
    /// connection is dropped before its first message is read, whichever
    /// session it would have belonged to. The listener keeps running and keeps
    /// its address; each dropped connection is reported to the
    /// [`ConnectionObserver`] as
    /// [`ConnectionDropReason::AcceptorSuspended`].
    ///
    /// This gates new connections only. Sessions that are already running
    /// keep running; ending them is a separate step ([`logout`](Self::logout)
    /// or [`disconnect`](Self::disconnect) per session). A connection that
    /// has already started reading its first message completes its handshake.
    ///
    /// Independent of [`suspend_session`](Self::suspend_session): the
    /// per-session gates are neither set by this call nor cleared by
    /// [`resume`](Self::resume). Unlike [`shutdown`](Self::shutdown), this is
    /// reversible.
    pub fn suspend(&self) {
        self.inner.suspended.set(true);
    }

    /// Lift a suspension set by [`suspend`](Self::suspend): new inbound
    /// connections are admitted again, subject to the per-session gates,
    /// which this call does not touch.
    pub fn resume(&self) {
        self.inner.suspended.set(false);
    }

    /// Query whether the acceptor is suspended via [`suspend`](Self::suspend).
    /// Reports the acceptor's own flag only; per-session suspensions are
    /// queried with [`is_session_suspended`](Self::is_session_suspended).
    pub fn is_suspended(&self) -> bool {
        self.inner.suspended.get()
    }

    /// Start the acceptor's listener task. Returns a [`JoinHandle`] for the
    /// task - abort it to stop accepting new connections without shutting the
    /// acceptor down.
    ///
    /// [`shutdown`](Self::shutdown) ends this task and does not return until it
    /// has exited and released the bound address, so the handle needs neither
    /// abort nor await on that path.
    ///
    /// Each accepted connection is dispatched to its own session task.
    pub fn start<C>(&self, connection: C) -> JoinHandle<()>
    where
        C: Connection + 'static,
    {
        let inner = self.inner.clone();
        // The guard is what makes `shutdown`'s wait cover the listener, and it
        // is taken here rather than inside the task for the reason `TaskGuard`
        // documents.
        let guard = TaskGuard::new(inner.clone());
        spawn_local(listener_task(connection, inner, guard))
    }

    /// Run a single session on an already-established connection
    /// (custom transport, test harness, etc.). Spawns a session task
    /// and returns its [`JoinHandle`].
    ///
    /// This bypasses the listener loop entirely - the caller is
    /// responsible for accepting connections. The returned task still
    /// participates in [`shutdown`](Self::shutdown).
    pub fn run_session<R, W>(&self, reader: R, writer: W, peer_addr: SocketAddr) -> JoinHandle<()>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        spawn_session_task(self.inner.clone(), reader, writer, peer_addr)
    }

    /// Build - but do not spawn - the session task future for an
    /// already-established connection. The caller spawns the returned
    /// future on its own executor (`tokio::task::spawn_local`, or any
    /// other single-threaded spawn that accepts a `!Send` future).
    ///
    /// This is the runtime-agnostic counterpart to
    /// [`run_session`](Self::run_session), which spawns on tokio and
    /// returns a [`JoinHandle`]. Use it to drive sessions on an executor
    /// where tokio's `spawn_local` / `JoinHandle` are not available.
    ///
    /// Sessions started this way are tracked by the acceptor, so it is
    /// safe to call [`shutdown`](Self::shutdown) concurrently - it waits
    /// for them to finish like any other session.
    pub fn session_task<R, W>(
        &self,
        reader: R,
        writer: W,
        peer_addr: SocketAddr,
    ) -> impl Future<Output = ()> + use<M, S, A, R, W>
    where
        R: AsyncRead + Unpin + 'static,
        W: AsyncWrite + Unpin + 'static,
    {
        session_future(self.inner.clone(), reader, writer, peer_addr)
    }

    /// Query whether the given session is currently active. Returns
    /// [`AcceptorError::UnknownSession`] if no matching session has
    /// been registered.
    pub fn is_session_active(&self, session_id: &SessionId) -> Result<bool, AcceptorError> {
        if self.inner.active_sessions.borrow().contains_key(session_id) {
            Ok(true)
        } else if self.inner.sessions.borrow().contains_key(session_id) {
            Ok(false)
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    /// Control-channel handle for `session_id`: `Ok(Some(tx))` if the
    /// session is active, `Ok(None)` if registered but inactive,
    /// `Err(UnknownSession)` if not registered.
    fn active_session_control(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<mpsc::Sender<ControlMsg>>, AcceptorError> {
        if let Some(tx) = self.inner.active_sessions.borrow().get(session_id) {
            Ok(Some(tx.clone()))
        } else if self.inner.sessions.borrow().contains_key(session_id) {
            Ok(None)
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    /// Send a Logout to the given active session. Returns `Ok(())` if
    /// the session is already logged out and
    /// [`AcceptorError::UnknownSession`] if no matching session has
    /// been registered.
    /// Repeating the request during logout sends no additional Logout and
    /// does not change the original acknowledgement deadline.
    pub async fn logout(
        &self,
        session_id: &SessionId,
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    ) -> Result<(), AcceptorError> {
        // `Ok(None)` = already logged out - no-op.
        if let Some(tx) = self.active_session_control(session_id)? {
            let _ = tx
                .send(ControlMsg::Logout {
                    session_status,
                    text,
                })
                .await;
        }
        Ok(())
    }

    /// Request a sequence number reset over the session's active connection.
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
    /// Returns [`AcceptorError::UnknownSession`] if the session is not
    /// registered, or [`AcceptorError::SessionInactive`] if it is not running.
    /// Otherwise returns [`AcceptorError::ResetSeqNumFlagNotSupportedInLogon`]
    /// before sending the request if the local Logon cannot preserve
    /// `ResetSeqNumFlag(141)`. That refusal leaves the session running.
    pub async fn request_running_session_reset(
        &self,
        session_id: &SessionId,
    ) -> Result<(), AcceptorError> {
        let tx = self
            .active_session_control(session_id)?
            .ok_or(AcceptorError::SessionInactive)?;
        let supports_reset = self
            .inner
            .sessions
            .borrow()
            .get(session_id)
            .ok_or(AcceptorError::UnknownSession)?
            .supports_seq_num_reset;
        if !supports_reset {
            return Err(AcceptorError::ResetSeqNumFlagNotSupportedInLogon);
        }
        let _ = tx.send(ControlMsg::ResetRunningSession).await;
        Ok(())
    }

    /// Force an immediate disconnect of the given session. Returns
    /// `Ok(())` if the session is already disconnected and
    /// [`AcceptorError::UnknownSession`] if no matching session has
    /// been registered.
    pub async fn disconnect(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        // `Ok(None)` = already disconnected - no-op.
        if let Some(tx) = self.active_session_control(session_id)? {
            let _ = tx.send(ControlMsg::Disconnect).await;
        }
        Ok(())
    }

    /// Await full closure of the given session.
    ///
    /// Resolves once the session has fully closed - its connection is torn down
    /// and it is no longer running - which is the only point at which
    /// [`remove_session`](Self::remove_session) can succeed. If the session is
    /// already registered but not running, this resolves immediately. Returns
    /// [`AcceptorError::UnknownSession`] if no matching session is registered.
    ///
    /// Semantics and caveats:
    /// - "Closed" means the session stopped running, not that a Logout message
    ///   was merely sent. After [`logout`](Self::logout) this means the logout
    ///   handshake concluded (the peer's Logout was received, or the logout
    ///   deadline elapsed and the session disconnected).
    /// - This is LEVEL-triggered on "the id is observed not currently running",
    ///   not scoped to a specific connection. With the listener still running a
    ///   peer can reconnect and re-activate the id; to drain race-free, call
    ///   [`suspend_session`](Self::suspend_session) first.
    /// - Must be awaited on the same single-threaded runtime that drives the
    ///   session tasks (the crate's `LocalSet` contract), and must NOT be
    ///   awaited from within that session's own [`Application`] callbacks: the
    ///   close signal fires only after the session stops running, so a
    ///   self-await deadlocks. To drive it from a callback, [`Clone`] the
    ///   `Acceptor` and `spawn_local` a separate task that awaits it (and runs
    ///   any follow-up such as [`remove_session`](Self::remove_session)); the
    ///   callback returns immediately and the session is free to exit.
    ///
    /// [`Application`]: crate::Application
    pub async fn await_session_closed(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        // Classify under a tight borrow, then clone out the per-session signal:
        // the loop below holds no `sessions` borrow across an `.await`, only the
        // cloned `Rc<Notify>`.
        let closed = {
            let sessions = self.inner.sessions.borrow();
            let reg = sessions
                .get(session_id)
                .ok_or(AcceptorError::UnknownSession)?;
            if reg.storage.is_some() {
                // Registered but not running - already "closed enough" to remove.
                return Ok(());
            }
            reg.closed.clone()
        };

        // Double-check loop, mirroring `shutdown` (see below), keyed on the
        // per-session `storage.is_none()` "running" flag instead of the
        // acceptor-wide `active_count`. The durable flag - not the `Notify`
        // edge - is the real guarantee; the edge only saves a needless park.
        // Each `borrow()` is an expression temporary dropped at the `;`, so no
        // borrow is ever live across the `.await`.
        loop {
            // "entry gone" (removed) and "storage restored" both mean
            // not-running => closed.
            let running = self
                .inner
                .sessions
                .borrow()
                .get(session_id)
                .is_some_and(|reg| reg.storage.is_none());
            if !running {
                break;
            }
            // Register interest BEFORE the second flag read so a
            // `notify_waiters` firing in the gap is not missed.
            let notified = closed.notified();
            let running = self
                .inner
                .sessions
                .borrow()
                .get(session_id)
                .is_some_and(|reg| reg.storage.is_none());
            if !running {
                break;
            }
            notified.await;
        }
        Ok(())
    }

    /// Convenience: force-disconnect a session and await its full closure.
    ///
    /// Equivalent to [`disconnect`](Self::disconnect) followed by
    /// [`await_session_closed`](Self::await_session_closed). For a graceful
    /// logout instead, compose [`logout`](Self::logout) with
    /// `await_session_closed` directly. The caveats on
    /// [`await_session_closed`](Self::await_session_closed) (no self-await from
    /// within the session's callbacks; reconnect gating) apply here too.
    pub async fn close(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        self.disconnect(session_id).await?;
        self.await_session_closed(session_id).await?;
        Ok(())
    }

    /// Reset a registered session's sequence numbers to `1` and discard
    /// messages retained for resend. Returns
    /// [`AcceptorError::SessionActive`] if the session is currently
    /// running - the caller must disconnect (or wait for disconnect)
    /// before resetting. Returns [`AcceptorError::UnknownSession`] if
    /// no matching session has been registered.
    ///
    /// Both peers must agree to reset before reconnecting (Session Test Cases
    /// Scenario 9). Sends nothing and does not require tag 141 support.
    /// See [offline resets](crate::session_reset#choosing-a-method).
    /// Returns [`AcceptorError::Storage`] on backend failure; the backend may
    /// be partially changed and must be recovered before reuse.
    pub fn reset_session(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        let mut sessions = self.inner.sessions.borrow_mut();
        let Some(reg) = sessions.get_mut(session_id) else {
            return Err(AcceptorError::UnknownSession);
        };
        let Some(storage) = reg.storage.as_mut() else {
            return Err(AcceptorError::SessionActive);
        };
        storage
            .reset()
            .map_err(|error| AcceptorError::Storage(Box::new(error)))
    }

    /// Shut down the acceptor, terminally.
    ///
    /// Stops new sessions from starting, terminates every currently active
    /// session according to [`ShutdownMode`], and returns once every session
    /// task *and* the listener started by [`start`](Self::start) have exited -
    /// so on return the listening address is free to rebind. Connections still
    /// waiting for their first message are dropped rather than waited out, so
    /// [`ShutdownMode::Disconnect`] is not held up by the Logon timeout.
    ///
    /// **There is no way back.** This acceptor and every clone of it stay shut
    /// down for good; build a new [`Acceptor`] to serve again. To stop
    /// admitting connections temporarily, use [`suspend`](Self::suspend) /
    /// [`resume`](Self::resume) for the whole acceptor or
    /// [`suspend_session`](Self::suspend_session) /
    /// [`resume_session`](Self::resume_session) for one session.
    ///
    /// Do **not** call this from an [`Application`](crate::Application)
    /// callback: the calling session's own task is one of those being waited
    /// for, so the wait never ends. Clone the `Acceptor` and drive the call
    /// from a task of its own. If you need a hard overall deadline, wrap the
    /// call with [`tokio::time::timeout`] and fall back to
    /// `shutdown(Disconnect)` on timeout.
    pub async fn shutdown(&self, mode: ShutdownMode) {
        self.inner.stopping.set(true);
        // Flag before signal: `stopped` re-reads the flag after registering,
        // so a waiter that arrives in between still sees the shutdown.
        self.inner.stop_notify.notify_waiters();

        // Collect control channels while borrowing, then drop the
        // borrow before awaiting any sends.
        let targets: Vec<mpsc::Sender<ControlMsg>> = self
            .inner
            .active_sessions
            .borrow()
            .values()
            .cloned()
            .collect();

        let follow_with_disconnect = matches!(mode, ShutdownMode::LogoutAndDisconnect { .. });

        for tx in targets {
            let msg = match &mode {
                ShutdownMode::Disconnect => ControlMsg::Disconnect,
                ShutdownMode::LogoutAndDisconnect {
                    session_status,
                    text,
                }
                | ShutdownMode::GracefulLogout {
                    session_status,
                    text,
                } => ControlMsg::Logout {
                    session_status: *session_status,
                    text: text.clone(),
                },
            };
            let _ = tx.send(msg).await;
            if follow_with_disconnect {
                let _ = tx.send(ControlMsg::Disconnect).await;
            }
        }

        // Wait for all session tasks to exit. The double-check around
        // `notified()` is the standard race-avoidance pattern for
        // `Notify`: getting the future before the second check ensures
        // we don't miss a notification that arrives between the two
        // reads of `active_count`.
        loop {
            if self.inner.active_count.get() == 0 {
                break;
            }
            let notified = self.inner.all_exited.notified();
            if self.inner.active_count.get() == 0 {
                break;
            }
            notified.await;
        }
    }
}

// ---------------------------------------------------------------------------
// Listener task
// ---------------------------------------------------------------------------

// Backoff bounds for a failing `accept()`. Tokio clears the listener's
// readiness only on `WouldBlock`; every other error leaves it set, so the next
// `accept()` completes synchronously with the same error and the loop never
// returns `Poll::Pending`. On the mandatory current-thread runtime that starves
// every session task - the sessions stop reading input and stop sending
// `Heartbeat<0>`, and their peers time them out. The sleep is what puts a
// yield back on that path; the growth just keeps a persistent failure
// (a descriptor limit, say) from flooding the log.
//
// A permanently broken listener (`EBADF`, `ENOTSOCK`) is left retrying rather
// than broken out of: those have no stable `io::ErrorKind`, so telling them
// apart needs raw errno values, and the failure mode a `break` trades for is a
// listener that dies silently.
const ACCEPT_BACKOFF_MIN: Duration = Duration::from_millis(10);
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(1);

async fn listener_task<M, S, A, C>(
    mut connection: C,
    inner: Rc<AcceptorInner<M, S, A>>,
    _guard: TaskGuard<M, S, A>,
) where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M> + 'static,
    A::App: 'static,
    C: Connection,
{
    info!("acceptor listener started");
    let mut backoff = ACCEPT_BACKOFF_MIN;
    loop {
        // Every wait in this loop is raced against the shutdown signal, and
        // those two races are the only place the loop observes the shutdown:
        // reaching an arm below means the flag was clear at that same poll, and
        // nothing between here and the spawn can set it (`shutdown` is `async`
        // and the runtime is single-threaded). Any new wait added here has to
        // join the race, or `shutdown` waits it out.
        //
        // Racing `accept()` is what releases the bound address promptly: parked
        // there, the loop would otherwise sit on the listener until some
        // unrelated connection happened to arrive, and a replacement acceptor
        // rebinding that address would fail meanwhile.
        let accepted = tokio::select! {
            biased;
            () = inner.stopped() => break,
            result = connection.accept() => result,
        };
        match accepted {
            Ok((reader, writer, peer_addr)) => {
                backoff = ACCEPT_BACKOFF_MIN;
                spawn_session_task(inner.clone(), reader, writer, peer_addr);
            }
            // EINTR is a restart signal, not a failure: retry at once and
            // leave the backoff where it was.
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => {
                error!(%err, "failed to accept incoming connection");
                tokio::select! {
                    biased;
                    () = inner.stopped() => break,
                    () = inner.timer_backend.sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(ACCEPT_BACKOFF_MAX);
            }
        }
    }
    info!("acceptor listener stopped");
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

/// Build the instrumented session task future without spawning it.
///
/// The [`TaskGuard`] is constructed here - so `active_count` is already
/// incremented when the future is returned - and moved into the future, where
/// it lives until the task finishes or the future is dropped.
fn session_future<M, S, A, R, W>(
    inner: Rc<AcceptorInner<M, S, A>>,
    reader: R,
    writer: W,
    peer_addr: SocketAddr,
) -> impl Future<Output = ()> + 'static
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M> + 'static,
    A::App: 'static,
    R: AsyncRead + Unpin + 'static,
    W: AsyncWrite + Unpin + 'static,
{
    let guard = TaskGuard::new(inner.clone());
    let span = info_span!("connection", %peer_addr);
    acceptor_session_task(inner, reader, writer, peer_addr, guard).instrument(span)
}

fn spawn_session_task<M, S, A, R, W>(
    inner: Rc<AcceptorInner<M, S, A>>,
    reader: R,
    writer: W,
    peer_addr: SocketAddr,
) -> JoinHandle<()>
where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M> + 'static,
    A::App: 'static,
    R: AsyncRead + Unpin + 'static,
    W: AsyncWrite + Unpin + 'static,
{
    spawn_local(session_future(inner, reader, writer, peer_addr))
}

async fn acceptor_session_task<M, S, A, R, W>(
    inner: Rc<AcceptorInner<M, S, A>>,
    reader: R,
    writer: W,
    peer_addr: SocketAddr,
    _guard: TaskGuard<M, S, A>,
) where
    M: SessionMessage + 'static,
    S: MessagesStorage + 'static,
    A: ApplicationFactory<M>,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // A suspended acceptor admits nothing: drop before reading a byte, so no
    // identity is derived and no registry entry is consulted. Checked once,
    // here - a connection that gets past this point completes its handshake
    // even if `suspend` is called meanwhile. Reported, unlike the shutdown
    // drop below: the peer knocked while the operator had the door closed,
    // which is worth knowing about.
    if inner.suspended.get() {
        warn!("acceptor suspended, dropping connection before first message");
        inner.notify_connection_dropped(peer_addr, ConnectionDropReason::AcceptorSuspended);
        return;
    }

    // Read the first (Logon) message under the acceptor-wide limit - the
    // session's own `max_message_size` is unknown until the message names
    // it. The buffer starts at exactly that limit; `from_parts` below grows
    // it to the session's burst size once the session is known.
    let first_message_limit = inner.settings.max_first_message_size;
    let mut input = InputStream::<R, M>::for_first_message(reader, first_message_limit);

    // --- Read the first message with the configured logon timeout ---

    let logon_timeout = inner.settings.auto_disconnect_after_no_logon_received;
    // Racing the read against the shutdown signal is what keeps `shutdown` from
    // waiting out the Logon timeout. `TaskGuard::new` counted this task in
    // at spawn time, but the `active_sessions` control channel only exists once
    // the first message has identified a session - so until then the task is
    // something `shutdown` waits for and cannot reach. A silent connection
    // (port scanner, load-balancer probe, a peer mid-TLS-handshake) would
    // otherwise hold up even `ShutdownMode::Disconnect`, whose whole point is
    // to be immediate.
    //
    // The drop goes unreported for the same reason the rest of the shutdown
    // teardown does: the operator asked for it (`shutdown_teardown_is_not_reported`).
    //
    // This race is also the ONLY place this task observes the shutdown. Taking
    // the message arm means the flag was clear at that same poll, and
    // everything from here to the session loop - the registry lookup, the
    // storage take, building the engine - is synchronous, so on a
    // single-threaded runtime the `async fn shutdown` cannot interleave. A
    // further flag check down there would be dead code. Adding an `.await`
    // below reopens the window and needs this race extended to cover it.
    let first = tokio::select! {
        biased;
        () = inner.stopped() => {
            warn!("acceptor stopping, dropping connection before first message");
            return;
        }
        result = inner.timer_backend.timeout(logon_timeout, input.first_message()) => result,
    };
    let (first_msg, session_id) = match first {
        // The first message must be a Logon(35=A): log an error and
        // disconnect otherwise (FIX Session Layer §4.3.1, Test Cases
        // §4.4.2 Scenario 2S). The engine's own pre-logon gate rejects it
        // too, but too late to matter here - `SessionId::from_inbound`
        // reads only the CompIDs, so any message type identifies a
        // registered session and the lookup below would take its storage
        // out of the registry, locking the real peer out with
        // `SessionAlreadyActive` for as long as this connection lives.
        Ok(Some(FirstMessageEvent::Message(msg))) if msg.msg_type() != MsgTypeBase::Logon => {
            error!(msg_type = ?msg.msg_type(), "first message not a logon");
            inner.notify_connection_dropped(peer_addr, ConnectionDropReason::FirstMessageNotLogon);
            return;
        }
        Ok(Some(FirstMessageEvent::Message(msg))) => {
            let session_id = SessionId::from_inbound(&*msg);
            (Ok(msg), session_id)
        }
        // An identified invalid Logon(35=A) from the wire is answered per
        // Test Cases Scenario 1S(d): (optional) Reject, Logout with
        // Text(58), disconnect. The registry lookup below still gates the
        // answer - identity failures mandate a disconnect without sending
        // (1S(b)/(c)). The engine runs the escalation itself once the
        // error is fed into the regular session loop.
        Ok(Some(FirstMessageEvent::DeserializeError {
            error,
            invalid_logon_identity: Some(session_id),
        })) => {
            warn!(
                %session_id, %error,
                "first message is an invalid Logon - escalating per Scenario 1S(d)"
            );
            (Err(error), session_id)
        }
        // No identity to answer to: garbled input, a non-Logon first
        // message, or an unscannable header - silent drop (Scenario
        // 2(d)/2S).
        Ok(Some(FirstMessageEvent::DeserializeError {
            error,
            invalid_logon_identity: None,
        })) => {
            error!(%error, "failed to deserialize first message");
            inner.notify_connection_dropped(
                peer_addr,
                ConnectionDropReason::FirstMessageUndecodable(&error),
            );
            return;
        }
        Ok(Some(FirstMessageEvent::TooLarge { frame_len })) => {
            warn!(
                frame_len,
                limit = first_message_limit,
                "first message exceeds max first message size, dropping connection"
            );
            inner.notify_connection_dropped(peer_addr, ConnectionDropReason::FirstMessageTooLarge);
            return;
        }
        Ok(Some(FirstMessageEvent::IoError(err))) => {
            error!(%err, "I/O error reading first message");
            inner.notify_connection_dropped(
                peer_addr,
                ConnectionDropReason::FirstMessageIoError(&err),
            );
            return;
        }
        Ok(None) => {
            info!("connection closed before first message");
            inner.notify_connection_dropped(
                peer_addr,
                ConnectionDropReason::ClosedBeforeFirstMessage,
            );
            return;
        }
        Err(_) => {
            warn!("logon timeout - no message received");
            inner.notify_connection_dropped(peer_addr, ConnectionDropReason::LogonTimeout);
            return;
        }
    };

    // --- Look up the registered session ---

    info!(%session_id, "first message received");

    // Classify under the borrow, but do NOT act on a failure here: reporting
    // it runs user code, which is free to call back into the `Acceptor` and
    // would panic on the still-live `sessions` borrow. The block therefore
    // only yields a verdict; the observer fires below, after the borrow ends.
    // Borrowing `session_id` for the reason is fine - it outlives the block
    // and is unrelated to the `RefCell`.
    let lookup = {
        let mut sessions = inner.sessions.borrow_mut();
        match sessions.get_mut(&session_id) {
            None => Err(ConnectionDropReason::UnknownSession(&session_id)),
            // A suspended session rejects new inbound connections (the gate
            // for reconfiguration). Drop the connection like the unknown /
            // duplicate cases.
            Some(reg) if reg.suspended => Err(ConnectionDropReason::SessionSuspended(&session_id)),
            Some(reg) => match reg.storage.take() {
                None => Err(ConnectionDropReason::SessionAlreadyActive(&session_id)),
                // Clone the per-session close signal under the SAME borrow
                // that takes storage (a plain field clone on the borrowed
                // `reg`, no new borrow); the guard carries it so its drop can
                // fire without a fresh lookup.
                Some(storage) => Ok((reg.session_settings.clone(), storage, reg.closed.clone())),
            },
        }
    };

    let (session_settings, storage, closed) = match lookup {
        Ok(taken) => taken,
        Err(reason) => {
            match &reason {
                ConnectionDropReason::UnknownSession(_) => {
                    warn!(%session_id, "unknown session - dropping connection");
                }
                ConnectionDropReason::SessionSuspended(_) => {
                    warn!(%session_id, "session suspended - dropping inbound connection");
                }
                _ => warn!(
                    %session_id,
                    "session already active - dropping duplicate connection"
                ),
            }
            inner.notify_connection_dropped(peer_addr, reason);
            return;
        }
    };

    // Own the storage in a guard that returns it to the registry on ANY
    // exit from here on - normal return, early return, or panic unwind - so
    // a panicking session task can never permanently lose the counters or
    // leave the SessionId locked out. The guard also clears the
    // `active_sessions` entry and fires the per-session close signal.
    let mut storage_guard = StorageReturn {
        inner: inner.clone(),
        session_id: session_id.clone(),
        storage: Some(storage),
        closed,
    };

    // --- Build per-session application, engine, and channels ---

    let time_precision = session_settings.time_precision;
    let app = inner.app_factory.create(&SessionContext::new(
        &session_id,
        Some(peer_addr),
        time_precision,
    ));
    let engine = SessionEngine::<M>::new(session_id.clone(), session_settings, inner.timer_backend);

    // Rebuild the input stream with the session's configured
    // max_message_size, preserving any bytes already buffered past the
    // first message.
    let session_max_message_size = engine.session_settings().max_message_size;
    let (reader, buffered) = input.into_parts();
    let input = InputStream::<R, M>::from_parts(reader, buffered, session_max_message_size);

    let (sender_tx, app_rx) = sender::channel::<M>(inner.timer_backend);
    let (control_tx, control_rx) = mpsc::channel::<ControlMsg>(4);

    inner
        .active_sessions
        .borrow_mut()
        .insert(session_id.clone(), control_tx);

    // --- Run the session loop (storage borrowed; `storage_guard` owns it) ---

    // Root span - deliberately not a child of the `connection` span, so the
    // peer address does not ride along on every session event. The two are
    // bound once by the "first message received" log above, which carries
    // the session id as a field while `connection` supplies the address.
    let session_span = info_span!(parent: None, "session", id = %session_id);

    session_loop(
        SessionOpening::FirstMessage(first_msg),
        input,
        writer,
        engine,
        storage_guard.storage(),
        app,
        sender_tx,
        app_rx,
        control_rx,
    )
    .instrument(session_span)
    .await;

    info!(%session_id, "session task finished");
    // `storage_guard` drops here - storage returned to the registry, the
    // `active_sessions` entry removed, and the per-session `closed` signal
    // fired; `_guard` (TaskGuard) drops AFTER it, firing `all_exited`.
    //
    // This order is load-bearing: `storage_guard` is a local declared after
    // `_guard` (a fn parameter), and Rust drops locals before parameters and in
    // reverse declaration order, so the per-session close signal fires before
    // `all_exited`. Do NOT hoist `_guard` into a local declared after
    // `storage_guard` - that would silently flip the order so an
    // `await_session_closed` waiter could observe storage before it is restored.
}

#[cfg(test)]
mod tests;
