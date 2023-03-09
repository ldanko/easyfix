use std::{
    cell::RefCell,
    collections::HashMap,
    net::SocketAddr,
    panic,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use easyfix_messages::{fields::FixString, messages::FixtMessage};
use futures::{self, Stream};
use pin_project::pin_project;
use tokio::net::TcpListener;
use tracing::{error, info, info_span, warn, Instrument};

use crate::{
    application::{events_channel, AsEvent, Emitter, EventStream},
    connection::new_connection,
    messages_storage::MessagesStorage,
    session::Session,
    session_id::SessionId,
    session_state::State as SessionState,
    settings::SessionSettings,
    Error, Settings,
};

pub struct SessionsMap<S: MessagesStorage> {
    map: HashMap<SessionId, (SessionSettings, Rc<RefCell<SessionState<S>>>)>,
    message_storage_builder: Box<dyn Fn() -> S>,
}

impl<S: MessagesStorage> SessionsMap<S> {
    fn new(message_storage_builder: Box<dyn Fn() -> S>) -> SessionsMap<S> {
        SessionsMap {
            map: HashMap::new(),
            message_storage_builder,
        }
    }

    pub fn register_session(&mut self, session_id: SessionId, session_settings: SessionSettings) {
        self.map.insert(
            session_id.clone(),
            (
                session_settings,
                Rc::new(RefCell::new(SessionState::new((self
                    .message_storage_builder)(
                )))),
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

pub(crate) type ActiveSessionsMap<S> = HashMap<SessionId, Rc<Session<S>>>;

#[pin_project]
pub struct Acceptor<S: MessagesStorage> {
    settings: Settings,
    sessions: Rc<RefCell<SessionsMap<S>>>,
    active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
    emitter: Emitter,
    #[pin]
    event_stream: EventStream,
}

impl<S: MessagesStorage + 'static> Acceptor<S> {
    pub fn new(settings: Settings, message_storage_builder: Box<dyn Fn() -> S>) -> Acceptor<S> {
        let (emitter, event_stream) = events_channel();
        Acceptor {
            settings,
            sessions: Rc::new(RefCell::new(SessionsMap::new(message_storage_builder))),
            active_sessions: Rc::new(RefCell::new(HashMap::new())),
            emitter,
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

    pub fn start(&self) {
        let server_task = tokio::task::spawn_local(Self::server_task(
            self.settings.clone(),
            self.sessions.clone(),
            self.active_sessions.clone(),
            self.emitter.clone(),
        ));

        let server_error_fut = async {
            if let Err(err) = server_task.await {
                if err.is_panic() {
                    // Resume the panic on the main task
                    panic::resume_unwind(err.into_panic());
                }
            }
        };
    }

    pub fn send(&self, msg: Box<FixtMessage>) -> Result<(), Error> {
        // self.emitter.send(FixEventInternal::Created).await;
        Ok(())
    }

    pub async fn logout(&self, session_id: &SessionId, reason: Option<FixString>) {
        let session = {
            let active_sessions = self.active_sessions.borrow();
            let Some(session) = active_sessions.get(&session_id)  else {
                warn!("logout: session {session_id} not found");
                return;
            };
            session.clone()
        };

        session.send_logout(reason).await;
    }

    pub async fn disconnect(&self, session_id: &SessionId) {
        let session = {
            let active_sessions = self.active_sessions.borrow();
            let Some(session) = active_sessions.get(&session_id)  else {
                warn!("logout: session {session_id} not found");
                return;
            };
            session.clone()
        };

        session.disconnect().await;
    }

    async fn server_task(
        settings: Settings,
        sessions: Rc<RefCell<SessionsMap<S>>>,
        active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
        emitter: Emitter,
    ) -> Result<(), Error> {
        info!("Acceptor started");
        let address = SocketAddr::from((settings.host, settings.port));
        let listener = TcpListener::bind(&address).await?;
        loop {
            let (tcp_stream, peer_addr) = listener.accept().await?;

            info!("---------------------------------------------------------");
            info!("New connection from {}", peer_addr);

            let sessions = sessions.clone();
            let active_sessions = active_sessions.clone();
            let settings = settings.clone();
            let emitter = emitter.clone();
            tokio::task::spawn_local(async move {
                match new_connection(tcp_stream, settings, sessions, active_sessions, emitter)
                    .instrument(info_span!("connection", %peer_addr))
                    .await
                {
                    Ok(()) => info!("Connection closed"),
                    Err(error) => error!("Connection closed: {}", error),
                }
            });
        }
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
