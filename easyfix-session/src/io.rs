use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::Entry},
    rc::Rc,
    sync::Mutex,
};

use easyfix_messages::{
    fields::{FixString, SessionStatus},
    messages::{FixtMessage, Message},
};
use futures_util::{Stream, pin_mut};
use tokio::{
    self,
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::Duration,
};
use tokio_stream::StreamExt;
use tracing::{Instrument, Span, debug, error, info, info_span};

use crate::{
    DisconnectReason, Error, NO_INBOUND_TIMEOUT_PADDING, Sender, SessionError,
    TEST_REQUEST_THRESHOLD,
    acceptor::{ActiveSessionsMap, SessionsMap},
    application::{Emitter, FixEventInternal},
    messages_storage::MessagesStorage,
    session::Session,
    session_id::SessionId,
    session_state::State,
    settings::{SessionSettings, Settings},
};

mod input_stream;
pub use input_stream::{InputEvent, InputStream, input_stream};

mod output_stream;
use output_stream::{OutputEvent, output_stream};

pub mod time;
use time::{timeout, timeout_stream};

static SENDERS: Mutex<Option<HashMap<SessionId, Sender>>> = Mutex::new(None);

pub fn register_sender(session_id: SessionId, sender: Sender) {
    if let Entry::Vacant(entry) = SENDERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .entry(session_id)
    {
        entry.insert(sender);
    }
}

pub fn unregister_sender(session_id: &SessionId) {
    if SENDERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .remove(session_id)
        .is_none()
    {
        // TODO: ERROR?
    }
}

pub fn sender(session_id: &SessionId) -> Option<Sender> {
    SENDERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(session_id)
        .cloned()
}

// TODO: Remove?
pub fn send(session_id: &SessionId, msg: Box<Message>) -> Result<(), Box<Message>> {
    if let Some(sender) = sender(session_id) {
        sender.send(msg).map_err(|msg| msg.body)
    } else {
        Err(msg)
    }
}

pub fn send_raw(msg: Box<FixtMessage>) -> Result<(), Box<FixtMessage>> {
    if let Some(sender) = sender(&SessionId::from_input_msg(&msg)) {
        sender.send_raw(msg)
    } else {
        Err(msg)
    }
}

async fn first_msg(
    stream: &mut (impl Stream<Item = InputEvent> + Unpin),
    logon_timeout: Duration,
) -> Result<Box<FixtMessage>, Error> {
    match timeout(logon_timeout, stream.next()).await {
        Ok(Some(InputEvent::Message(msg))) => Ok(msg),
        Ok(Some(InputEvent::IoError(error))) => Err(error.into()),
        Ok(Some(InputEvent::DeserializeError(error))) => {
            error!("failed to deserialize first message: {error}");
            Err(Error::SessionError(SessionError::LogonNeverReceived))
        }
        _ => Err(Error::SessionError(SessionError::LogonNeverReceived)),
    }
}

#[derive(Debug)]
struct Connection<S> {
    session: Rc<Session<S>>,
}

pub(crate) async fn acceptor_connection<S>(
    reader: impl AsyncRead + Unpin,
    writer: impl AsyncWrite + Unpin,
    settings: Settings,
    sessions: Rc<RefCell<SessionsMap<S>>>,
    active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
    emitter: Emitter,
) where
    S: MessagesStorage,
{
    let stream = input_stream(reader);
    let logon_timeout =
        settings.auto_disconnect_after_no_logon_received + NO_INBOUND_TIMEOUT_PADDING;
    pin_mut!(stream);
    let msg = match first_msg(&mut stream, logon_timeout).await {
        Ok(msg) => msg,
        Err(err) => {
            error!("failed to establish new session: {err}");
            return;
        }
    };
    let session_id = SessionId::from_input_msg(&msg);
    debug!("first_msg: {msg:?}");

    let (sender, receiver) = mpsc::unbounded_channel();
    let sender = Sender::new(sender);

    let Some((session_settings, session_state)) = sessions.borrow().get_session(&session_id) else {
        error!("failed to establish new session: unknown session id {session_id}");
        return;
    };
    if !session_state.borrow_mut().disconnected()
        || active_sessions.borrow().contains_key(&session_id)
    {
        error!(%session_id, "Session already active");
        return;
    }
    session_state.borrow_mut().set_disconnected(false);
    register_sender(session_id.clone(), sender.clone());
    let session = Rc::new(Session::new(
        settings,
        session_settings,
        session_state,
        sender,
        emitter.clone(),
    ));
    active_sessions
        .borrow_mut()
        .insert(session_id.clone(), session.clone());

    let session_span = info_span!(
        parent: None,
        "session",
        id = %session_id
    );
    session_span.follows_from(Span::current());

    let input_loop_span = info_span!(parent: &session_span, "in");
    let output_loop_span = info_span!(parent: &session_span, "out");

    let force_disconnection_with_reason = session
        .on_message_in(msg)
        .instrument(input_loop_span.clone())
        .await;

    // TODO: Not here!, send this event when SessionState is created!
    emitter
        .send(FixEventInternal::Created(session_id.clone()))
        .await;

    let input_timeout_duration = session.heartbeat_interval().mul_f32(TEST_REQUEST_THRESHOLD);
    let input_stream = timeout_stream(input_timeout_duration, stream)
        .map(|res| res.unwrap_or(InputEvent::Timeout));
    pin_mut!(input_stream);

    let output_stream = output_stream(session.clone(), session.heartbeat_interval(), receiver);
    pin_mut!(output_stream);

    let connection = Connection::new(session);
    let (input_closed_tx, input_closed_rx) = tokio::sync::oneshot::channel();

    tokio::join!(
        connection
            .input_loop(
                input_stream,
                input_closed_tx,
                force_disconnection_with_reason
            )
            .instrument(input_loop_span),
        connection
            .output_loop(writer, output_stream, input_closed_rx)
            .instrument(output_loop_span),
    );
    session_span.in_scope(|| {
        info!("connection closed");
    });
    unregister_sender(&session_id);
    active_sessions.borrow_mut().remove(&session_id);
}

pub(crate) async fn initiator_connection<S>(
    tcp_stream: TcpStream,
    settings: Settings,
    session_settings: SessionSettings,
    state: Rc<RefCell<State<S>>>,
    active_sessions: Rc<RefCell<ActiveSessionsMap<S>>>,
    emitter: Emitter,
) where
    S: MessagesStorage,
{
    let (source, sink) = tcp_stream.into_split();
    state.borrow_mut().set_disconnected(false);
    let session_id = session_settings.session_id.clone();

    let (sender, receiver) = mpsc::unbounded_channel();
    let sender = Sender::new(sender);

    register_sender(session_id.clone(), sender.clone());
    let session = Rc::new(Session::new(
        settings,
        session_settings,
        state,
        sender,
        emitter.clone(),
    ));
    active_sessions
        .borrow_mut()
        .insert(session_id.clone(), session.clone());

    let session_span = info_span!(
        "session",
        id = %session_id
    );

    let input_loop_span = info_span!(parent: &session_span, "in");
    let output_loop_span = info_span!(parent: &session_span, "out");

    // TODO: Not here!, send this event when SessionState is created!
    emitter
        .send(FixEventInternal::Created(session_id.clone()))
        .await;

    let input_timeout_duration = session.heartbeat_interval().mul_f32(TEST_REQUEST_THRESHOLD);
    let input_stream = timeout_stream(input_timeout_duration, input_stream(source))
        .map(|res| res.unwrap_or(InputEvent::Timeout));
    pin_mut!(input_stream);

    let output_stream = output_stream(session.clone(), session.heartbeat_interval(), receiver);
    pin_mut!(output_stream);

    // TODO: It's not so simple, add check if session time is within range,
    //       if not schedule timer to send logon at proper time
    session.send_logon_request(&mut session.state().borrow_mut());

    let connection = Connection::new(session);
    let (input_closed_tx, input_closed_rx) = tokio::sync::oneshot::channel();

    tokio::join!(
        connection
            .input_loop(input_stream, input_closed_tx, None)
            .instrument(input_loop_span),
        connection
            .output_loop(sink, output_stream, input_closed_rx)
            .instrument(output_loop_span),
    );
    info!("connection closed");
    unregister_sender(&session_id);
    active_sessions.borrow_mut().remove(&session_id);
}

impl<S: MessagesStorage> Connection<S> {
    fn new(session: Rc<Session<S>>) -> Connection<S> {
        Connection { session }
    }

    async fn input_loop(
        &self,
        mut input_stream: impl Stream<Item = InputEvent> + Unpin,
        input_closed_tx: tokio::sync::oneshot::Sender<()>,
        force_disconnection_with_reason: Option<DisconnectReason>,
    ) {
        if let Some(disconnect_reason) = force_disconnection_with_reason {
            self.session
                .disconnect(&mut self.session.state().borrow_mut(), disconnect_reason);

            // Notify output loop that all input is processed so output queue can
            // be safely closed.
            // See `fn send()` and `fn send_raw()` from session.rs.
            input_closed_tx
                .send(())
                .expect("Failed to notify about closed inpuot");

            return;
        }

        let mut disconnect_reason = DisconnectReason::Disconnected;
        let mut logout_timeout = None;

        while let Some(event) =
            timeout(logout_timeout.unwrap_or(Duration::MAX), input_stream.next())
                .await
                .unwrap_or(Some(InputEvent::LogoutTimeout))
        {
            if logout_timeout.is_none() {
                logout_timeout = self.session.logout_timeout();
            }
            // Don't accept new messages if session is disconnected.
            if self.session.state().borrow().disconnected() {
                info!("session disconnected, exit input processing");
                // Notify output loop that all input is processed so output queue can
                // be safely closed.
                // See `fn send()` and `fn send_raw()` from session.rs.
                input_closed_tx
                    .send(())
                    .expect("Failed to notify about closed inpout");
                return;
            }
            match event {
                InputEvent::Message(msg) => {
                    if let Some(reason) = self.session.on_message_in(msg).await {
                        info!(?reason, "disconnect, exit input processing");
                        disconnect_reason = reason;
                        break;
                    }
                }
                InputEvent::DeserializeError(error) => {
                    if let Some(reason) = self.session.on_deserialize_error(error).await {
                        info!(?reason, "disconnect, exit input processing");
                        disconnect_reason = reason;
                        break;
                    }
                }
                InputEvent::IoError(error) => {
                    error!(%error, "Input error");
                    disconnect_reason = DisconnectReason::IoError;
                    break;
                }
                InputEvent::Timeout => {
                    if self.session.on_in_timeout().await {
                        self.session.send_logout(
                            &mut self.session.state().borrow_mut(),
                            Some(SessionStatus::SessionLogoutComplete),
                            Some(FixString::from_ascii_lossy(
                                b"Grace period is over".to_vec(),
                            )),
                        );
                        break;
                    }
                }
                InputEvent::LogoutTimeout => {
                    info!("Logout timeout");
                    disconnect_reason = DisconnectReason::LogoutTimeout;
                    break;
                }
            }
        }
        self.session
            .disconnect(&mut self.session.state().borrow_mut(), disconnect_reason);

        // Notify output loop that all input is processed so output queue can
        // be safely closed.
        // See `fn send()` and `fn send_raw()` from session.rs.
        input_closed_tx
            .send(())
            .expect("Failed to notify about closed inpout");
    }

    async fn output_loop(
        &self,
        mut sink: impl AsyncWrite + Unpin,
        mut output_stream: impl Stream<Item = OutputEvent> + Unpin,
        input_closed_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let mut sink_closed = false;
        let mut disconnect_reason = DisconnectReason::Disconnected;
        while let Some(event) = output_stream.next().await {
            match event {
                OutputEvent::Message(msg) => {
                    if sink_closed {
                        // Sink is closed - ignore message, but do not break
                        // the loop. Output stream has to process all enqueued
                        // messages to made them available
                        // for ResendRequest<2>.
                        info!("Client disconnected, message will be stored for further resend");
                    } else if let Err(error) = sink.write_all(&msg).await {
                        sink_closed = true;
                        error!(%error, "Output write error");
                        // XXX: Don't disconnect now. If IO error happened
                        //      here, it will aslo happen in input loop
                        //      and input loop will trigger disconnection.
                        //      Disonnection from here would lead to message
                        //      loss when output queue would be closed
                        //      and input handler would try to send something.
                        //
                        // self.session.disconnect(
                        //     &mut self.session.state().borrow_mut(),
                        //     DisconnectReason::IoError,
                        // );
                    }
                }
                OutputEvent::Timeout => self.session.on_out_timeout().await,
                OutputEvent::Disconnect(reason) => {
                    // Internal channel is closed in output stream
                    // inplementation, at this point no new messages
                    // can be send.
                    info!("Client disconnected");
                    if !sink_closed && let Err(error) = sink.flush().await {
                        error!(%error, "final flush failed");
                    }
                    disconnect_reason = reason;
                }
            }
        }
        // XXX: Emit logout here instead of Session::disconnect, so `Logout`
        //      event will be delivered after Logout message instead of
        //      randomly before or after.
        self.session.emit_logout(disconnect_reason).await;

        // Don't wait for any specific value it's just notification that
        // input_loop finished, so no more messages can be added to output
        // queue.
        let _ = input_closed_rx.await;
        if let Err(error) = sink.shutdown().await {
            error!(%error, "connection shutdown failed")
        }
        info!("disconnect, exit output processing");
    }
}
