use std::{cell::RefCell, collections::HashMap, net::SocketAddr, rc::Rc};

use pin_project::pin_project;
use tokio::net::TcpStream;
use tracing::{info, info_span, Instrument};

use crate::{
    application::{events_channel, Emitter, EventStream},
    io::initiator_connection,
    messages_storage::MessagesStorage,
    session::Session,
    session_id::SessionId,
    session_state::State,
    settings::{SessionSettings, Settings},
    Error,
};

// TODO: Same as in Acceptor, not need for duplicate
pub(crate) type ActiveSessionsMap<S> = HashMap<SessionId, Rc<Session<S>>>;

#[pin_project]
pub struct Initiator<S: MessagesStorage> {
    id: SessionId,
    settings: Settings,
    session_settings: SessionSettings,
    state: Rc<RefCell<State<S>>>,
    active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
    emitter: Emitter,
    #[pin]
    event_stream: EventStream,
}

impl<S: MessagesStorage + 'static> Initiator<S> {
    pub fn new(
        settings: Settings,
        session_settings: SessionSettings,
        messages_storage: S,
    ) -> Initiator<S> {
        let (emitter, event_stream) = events_channel();
        Initiator {
            id: session_settings.session_id.clone(),
            settings,
            session_settings,
            state: Rc::new(RefCell::new(State::new(messages_storage))),
            active_sessions: Rc::new(RefCell::new(HashMap::new())),
            emitter,
            event_stream,
        }
    }

    pub async fn connect(&self) -> Result<(), Error> {
        info!("Initiator started");

        let addr = SocketAddr::from((self.settings.host, self.settings.port));
        let tcp_stream = TcpStream::connect(addr).await?;
        tcp_stream.set_nodelay(true)?;
        let emitter = self.emitter.clone();
        let settings = self.settings.clone();
        let session_settings = self.session_settings.clone();
        let active_sessions = self.active_sessions.clone();
        let state = self.state.clone();

        let connection_span = info_span!("connection", %addr);

        tokio::task::spawn_local(async move {
            initiator_connection(
                tcp_stream,
                settings,
                session_settings,
                state,
                active_sessions,
                emitter,
            )
            .instrument(connection_span.clone())
            .await;
            connection_span.in_scope(|| {
                info!("Connection closed");
            });
        });
        Ok(())
    }
}
