use std::{
    cell::RefCell,
    collections::HashMap,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use easyfix_messages::fields::{FixString, SessionStatus};
use futures::{self, Stream};
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    task::JoinHandle,
};
use tracing::{error, info, info_span, warn, Instrument};

use crate::{
    application::{events_channel, AsEvent, Emitter, EventStream},
    io::acceptor_connection,
    messages_storage::MessagesStorage,
    session::Session,
    session_id::SessionId,
    session_state::State as SessionState,
    settings::SessionSettings,
    DisconnectReason, Settings,
};

#[allow(async_fn_in_trait)]
pub trait Connection {
    async fn accept(
        &self,
    ) -> Result<
        (
            impl AsyncRead + Unpin + 'static,
            impl AsyncWrite + Unpin + 'static,
            SocketAddr,
        ),
        io::Error,
    >;
}

pub struct TcpConnection {
    listener: TcpListener,
}

impl TcpConnection {
    pub async fn new(socket_addr: impl Into<SocketAddr>) -> Result<TcpConnection, io::Error> {
        let socket_addr = socket_addr.into();
        let listener = TcpListener::bind(&socket_addr).await?;
        Ok(TcpConnection { listener })
    }
}

impl Connection for TcpConnection {
    async fn accept(
        &self,
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

type SessionMapInternal<S> = HashMap<SessionId, (SessionSettings, Rc<RefCell<SessionState<S>>>)>;

pub struct SessionsMap<S> {
    map: SessionMapInternal<S>,
    message_storage_builder: Box<dyn Fn(&SessionId) -> S>,
}

impl<S: MessagesStorage> SessionsMap<S> {
    fn new(message_storage_builder: Box<dyn Fn(&SessionId) -> S>) -> SessionsMap<S> {
        SessionsMap {
            map: HashMap::new(),
            message_storage_builder,
        }
    }

    #[rustfmt::skip]
    pub fn register_session(&mut self, session_id: SessionId, session_settings: SessionSettings) {
        self.map.insert(
            session_id.clone(),
            (
                session_settings,
                Rc::new(RefCell::new(SessionState::new(
                    (self.message_storage_builder)(&session_id),
                ))),
            ),
        );
    }

    pub(crate) fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Option<(SessionSettings, Rc<RefCell<SessionState<S>>>)> {
        self.map.get(session_id).cloned()
    }
}

pub struct SessionTask<S> {
    settings: Settings,
    sessions: Rc<RefCell<SessionsMap<S>>>,
    active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
    emitter: Emitter,
}

impl<S> Clone for SessionTask<S> {
    fn clone(&self) -> Self {
        Self {
            settings: self.settings.clone(),
            sessions: self.sessions.clone(),
            active_sessions: self.active_sessions.clone(),
            emitter: self.emitter.clone(),
        }
    }
}

impl<S: MessagesStorage + 'static> SessionTask<S> {
    fn new(
        settings: Settings,
        sessions: Rc<RefCell<SessionsMap<S>>>,
        active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
        emitter: Emitter,
    ) -> SessionTask<S> {
        SessionTask {
            settings,
            sessions,
            active_sessions,
            emitter,
        }
    }

    pub async fn run(
        self,
        peer_addr: SocketAddr,
        reader: impl AsyncRead + Unpin + 'static,
        writer: impl AsyncWrite + Unpin + 'static,
    ) {
        let span = info_span!("connection", %peer_addr);

        span.in_scope(|| {
            info!("---------------------------------------------------------");
            info!("New connection");
        });

        acceptor_connection(
            reader,
            writer,
            self.settings,
            self.sessions,
            self.active_sessions,
            self.emitter,
        )
        .instrument(span.clone())
        .await;

        span.in_scope(|| {
            info!("Connection closed");
        });
    }
}

pub(crate) type ActiveSessionsMap<S> = HashMap<SessionId, Rc<Session<S>>>;

#[pin_project]
pub struct Acceptor<S> {
    sessions: Rc<RefCell<SessionsMap<S>>>,
    active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
    session_task: SessionTask<S>,
    #[pin]
    event_stream: EventStream,
}

impl<S: MessagesStorage + 'static> Acceptor<S> {
    pub fn new(
        settings: Settings,
        message_storage_builder: Box<dyn Fn(&SessionId) -> S>,
    ) -> Acceptor<S> {
        let (emitter, event_stream) = events_channel();
        let sessions = Rc::new(RefCell::new(SessionsMap::new(message_storage_builder)));
        let active_sessions = Rc::new(RefCell::new(HashMap::new()));
        let session_task_builder =
            SessionTask::new(settings, sessions.clone(), active_sessions.clone(), emitter);

        Acceptor {
            sessions,
            active_sessions,
            session_task: session_task_builder,
            event_stream,
        }
    }

    pub fn register_session(&mut self, session_id: SessionId, session_settings: SessionSettings) {
        self.sessions
            .borrow_mut()
            .register_session(session_id, session_settings);
    }

    pub fn sessions_map(&self) -> Rc<RefCell<SessionsMap<S>>> {
        self.sessions.clone()
    }

    pub fn start(&self, connection: impl Connection + 'static) -> JoinHandle<()> {
        tokio::task::spawn_local(Self::server_task(connection, self.session_task.clone()))
    }

    pub fn logout(
        &self,
        session_id: &SessionId,
        session_status: Option<SessionStatus>,
        reason: Option<FixString>,
    ) {
        let active_sessions = self.active_sessions.borrow();
        let Some(session) = active_sessions.get(session_id) else {
            warn!("logout: session {session_id} not found");
            return;
        };

        session.send_logout(&mut session.state().borrow_mut(), session_status, reason);
    }

    pub fn disconnect(&self, session_id: &SessionId) {
        let active_sessions = self.active_sessions.borrow();
        let Some(session) = active_sessions.get(session_id) else {
            warn!("logout: session {session_id} not found");
            return;
        };

        session.disconnect(
            &mut session.state().borrow_mut(),
            DisconnectReason::UserForcedDisconnect,
        );
    }

    /// Force reset of the session
    ///
    /// Functionally equivalent to `reset_on_logon/logout/disconnect` settings,
    /// but triggered manually.
    ///
    /// You may call this after [Self::disconnect] if you want to manually reset the connection
    pub fn reset(&self, session_id: &SessionId) {
        let active_sessions = self.active_sessions.borrow();
        let Some(session) = active_sessions.get(session_id) else {
            warn!("reset: session {session_id} not found");
            return;
        };

        session.reset(&mut session.state().borrow_mut());
    }

    async fn server_task(connection: impl Connection, session_task: SessionTask<S>) {
        info!("Acceptor started");
        let connection = Rc::new(connection);
        loop {
            match connection.accept().await {
                Ok((reader, writer, peer_addr)) => {
                    tokio::task::spawn_local(session_task.clone().run(peer_addr, reader, writer));
                }
                Err(err) => error!("server task failed to accept incoming connection: {err}"),
            }
        }
    }

    pub fn session_task(&self) -> SessionTask<S> {
        self.session_task.clone()
    }

    pub fn run_session_task(
        &self,
        peer_addr: SocketAddr,
        reader: impl AsyncRead + Unpin + 'static,
        writer: impl AsyncWrite + Unpin + 'static,
    ) -> impl Future<Output = ()> {
        self.session_task.clone().run(peer_addr, reader, writer)
    }
}

impl<S: MessagesStorage> Stream for Acceptor<S> {
    type Item = impl AsEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.event_stream).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.event_stream.size_hint()
    }
}
