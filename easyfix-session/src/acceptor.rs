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

use easyfix_messages::fields::{FixString, SeqNum, SessionStatus};
use futures::{self, Stream};
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    task::JoinHandle,
};
use tracing::{Instrument, error, info, info_span, instrument};

use crate::{
    DisconnectReason, Settings,
    application::{AsEvent, Emitter, EventStream, events_channel},
    io::acceptor_connection,
    messages_storage::MessagesStorage,
    session::Session,
    session_id::SessionId,
    session_state::State as SessionState,
    settings::SessionSettings,
};

#[derive(Debug, thiserror::Error)]
pub enum AcceptorError {
    #[error("Unknown session")]
    UnknownSession,
    #[error("Session active")]
    SessionActive,
}

#[allow(async_fn_in_trait)]
pub trait Connection {
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

    pub fn register_session(&mut self, session_id: SessionId, session_settings: SessionSettings) {
        let storage = (self.message_storage_builder)(&session_id);
        self.map.insert(
            session_id.clone(),
            (
                session_settings,
                Rc::new(RefCell::new(SessionState::new(storage))),
            ),
        );
    }

    pub(crate) fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Option<(SessionSettings, Rc<RefCell<SessionState<S>>>)> {
        self.map.get(session_id).cloned()
    }

    fn contains(&self, session_id: &SessionId) -> bool {
        self.map.contains_key(session_id)
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
        let session_task =
            SessionTask::new(settings, sessions.clone(), active_sessions.clone(), emitter);

        Acceptor {
            sessions,
            active_sessions,
            session_task,
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

    pub fn is_session_active(&self, session_id: &SessionId) -> Result<bool, AcceptorError> {
        if self.active_sessions.borrow().contains_key(session_id) {
            Ok(true)
        } else if self.sessions.borrow().contains(session_id) {
            Ok(false)
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    pub fn logout(
        &self,
        session_id: &SessionId,
        session_status: Option<SessionStatus>,
        reason: Option<FixString>,
    ) -> Result<(), AcceptorError> {
        if let Some(session) = self.active_sessions.borrow().get(session_id) {
            session.send_logout(&mut session.state().borrow_mut(), session_status, reason);
            Ok(())
        } else if self.sessions.borrow().contains(session_id) {
            // Already logged out
            Ok(())
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    pub fn disconnect(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        if let Some(session) = self.active_sessions.borrow_mut().remove(session_id) {
            session.disconnect(
                &mut session.state().borrow_mut(),
                DisconnectReason::ApplicationForcedDisconnect,
            );
            Ok(())
        } else if self.sessions.borrow().contains(session_id) {
            // Already disconnected
            Ok(())
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    /// Force reset of the session
    ///
    /// Functionally equivalent to `reset_on_logon/logout/disconnect` settings,
    /// but triggered manually.
    ///
    /// Returns [`AcceptorError::SessionActive`] if the session is still active.
    /// In that case, call [Self::disconnect] or [Self::logout] first and wait
    /// for the session to fully terminate before retrying.
    #[instrument(skip_all, fields(session_id=%session_id) ret)]
    pub fn reset(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        if self.active_sessions.borrow().contains_key(session_id) {
            Err(AcceptorError::SessionActive)
        } else if let Some((_, session_state)) = self.sessions.borrow().get_session(session_id) {
            session_state.borrow_mut().reset();
            Ok(())
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    // TODO: temporary solution, remove when diconnect will be synchronized
    #[instrument(skip_all, fields(session_id=%session_id) ret)]
    pub fn force_reset(&self, session_id: &SessionId) -> Result<(), AcceptorError> {
        if let Some(session) = self.active_sessions.borrow().get(session_id) {
            session.state().borrow_mut().reset();
            Ok(())
        } else if let Some((_, session_state)) = self.sessions.borrow().get_session(session_id) {
            session_state.borrow_mut().reset();
            Ok(())
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    /// Sender seq_num getter
    #[instrument(skip_all, fields(session_id=%session_id) ret)]
    pub fn next_sender_msg_seq_num(&self, session_id: &SessionId) -> Result<SeqNum, AcceptorError> {
        if let Some(session) = self.active_sessions.borrow().get(session_id) {
            Ok(session.state().borrow().next_sender_msg_seq_num())
        } else if let Some((_, session_state)) = self.sessions.borrow().get_session(session_id) {
            Ok(session_state.borrow().next_sender_msg_seq_num())
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    /// Override sender's next seq_num
    #[instrument(skip_all, fields(session_id=%session_id, seq_num) ret)]
    pub fn set_next_sender_msg_seq_num(
        &self,
        session_id: &SessionId,
        seq_num: SeqNum,
    ) -> Result<(), AcceptorError> {
        if let Some(session) = self.active_sessions.borrow().get(session_id) {
            session
                .state()
                .borrow_mut()
                .set_next_sender_msg_seq_num(seq_num);
            Ok(())
        } else if let Some((_, session_state)) = self.sessions.borrow().get_session(session_id) {
            session_state
                .borrow_mut()
                .set_next_sender_msg_seq_num(seq_num);
            Ok(())
        } else {
            Err(AcceptorError::UnknownSession)
        }
    }

    async fn server_task(mut connection: impl Connection, session_task: SessionTask<S>) {
        info!("Acceptor started");
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
