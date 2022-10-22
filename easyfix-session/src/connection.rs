use std::{
    cell::RefCell,
    collections::{hash_map::Entry, HashMap},
    io,
    rc::Rc,
};

use easyfix_messages::{deserializer::DeserializeError, messages::FixtMessage};
use futures::{pin_mut, SinkExt, Stream};
use once_cell::unsync::Lazy;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::mpsc::{self, Receiver},
    time::{timeout, Duration},
};
use tokio_stream::{wrappers::ReceiverStream, Elapsed, StreamExt};
use tokio_util::codec::{FramedRead, FramedWrite};
use tracing::{debug, error, info, info_span, Instrument, warn};

use crate::{
    acceptor::SessionsMap,
    application::{Emitter, FixEventInternal},
    codec::{FixDecoder, FixEncoder, FixEncoderError},
    messages_storage::MessagesStorage,
    session::Session,
    session_id::SessionId,
    session_state::State as SessionState,
    settings::{SessionSettings, Settings},
    Error, Sender, SenderMsg, SessionError, NO_INBOUND_TIMEOUT_PADDING,
};

pub struct Disconnect;

// TODO: cfg(mt) on mt build
static mut SENDERS: Lazy<HashMap<SessionId, Sender>> = Lazy::new(|| HashMap::new());

fn senders() -> &'static HashMap<SessionId, Sender> {
    Lazy::force(unsafe { &SENDERS })
}

fn senders_mut() -> &'static mut HashMap<SessionId, Sender> {
    Lazy::force_mut(unsafe { &mut SENDERS })
}

pub fn register_sender(session_id: SessionId, sender: Sender) {
    if let Entry::Vacant(entry) = senders_mut().entry(session_id) {
        entry.insert(sender);
    }
}

pub fn unregister_sender(session_id: &SessionId) {
    if senders_mut().remove(session_id).is_none() {
        // TODO: ERROR?
    }
}

pub fn sender(session_id: &SessionId) -> Option<&Sender> {
    senders().get(session_id)
}

pub async fn send(msg: Box<FixtMessage>) {
    sender(&SessionId::from_input_msg(&msg))
        .unwrap()
        .send(msg)
        .await;
}

enum InputEvent {
    Message(Box<FixtMessage>),
    DeserializeError(DeserializeError),
    IoError(io::Error),
    Timeout,
}

// TODO: Match proper error messages, don't use generics here
// T may be tokio_stream::Elapsed or tokio::time::error::Elapsed
impl<T> From<Result<Result<Result<Box<FixtMessage>, DeserializeError>, io::Error>, T>>
    for InputEvent
{
    fn from(
        result: Result<Result<Result<Box<FixtMessage>, DeserializeError>, io::Error>, T>,
    ) -> InputEvent {
        match result {
            Ok(Ok(Ok(msg))) => InputEvent::Message(msg),
            Ok(Ok(Err(error))) => InputEvent::DeserializeError(error),
            Ok(Err(error)) => InputEvent::IoError(error),
            Err(_) => InputEvent::Timeout,
        }
    }
}

async fn first_msg(
    stream: &mut FramedRead<impl AsyncRead + Unpin, FixDecoder>,
    logon_timeout: Duration,
) -> Result<Box<FixtMessage>, Error> {
    // Result<Option<Result<Result<FIXT Message>, io::Error>>>
    //    ▲      ▲      ▲      ▲
    //    │      │      │      └ Result from decoder (FIX Parser)
    //    │      │      └ Result from stream (IO error)
    //    │      └ Is end of stream
    //    └ Is timeout
    // TODO: How quickfix handles similar errors?
    match timeout(logon_timeout, stream.next())
        .await
        .transpose()
        .map(InputEvent::from)
    {
        Some(InputEvent::Message(msg)) => Ok(msg),
        Some(InputEvent::DeserializeError(_error)) => {
            Err(Error::SessionError(SessionError::LogonNeverReceived))
        }
        Some(InputEvent::IoError(error)) => Err(error.into()),
        Some(InputEvent::Timeout) => Err(Error::SessionError(SessionError::LogonNeverReceived)),
        None => Err(Error::SessionError(SessionError::LogonNeverReceived)),
    }
}

fn input_stream(
    timeout_duration: Duration,
    input: FramedRead<impl AsyncRead + Unpin, FixDecoder>,
) -> impl Stream<Item = InputEvent> {
    input.timeout(timeout_duration).map(InputEvent::from)
}

fn output_stream(
    timeout_duration: Duration,
    output: Receiver<SenderMsg>,
) -> impl Stream<Item = OutputEvent> {
    ReceiverStream::new(output)
        .timeout(timeout_duration)
        .map(OutputEvent::from)
}

enum OutputEvent {
    Message(Box<FixtMessage>),
    Timeout,
    Disconnect,
}

impl From<Result<SenderMsg, Elapsed>> for OutputEvent {
    fn from(result: Result<SenderMsg, Elapsed>) -> Self {
        match result {
            Ok(SenderMsg::Msg(msg)) => OutputEvent::Message(msg),
            Ok(SenderMsg::Disconnect) => OutputEvent::Disconnect,
            Err(Elapsed { .. }) => OutputEvent::Timeout,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Connection<S> {
    settings: Settings,
    session: Rc<Session<S>>,
}

pub(crate) async fn new_connection<S>(
    tcp_stream: TcpStream,
    settings: Settings,
    sessions: Rc<RefCell<SessionsMap<S>>>,
    emitter: Emitter,
) -> Result<(), Error>
where
    S: MessagesStorage,
{
    let (source, sink) = tcp_stream.into_split();
    let mut stream = FramedRead::new(source, FixDecoder::new());
    let logon_timeout = settings.heartbeat_interval + NO_INBOUND_TIMEOUT_PADDING;
    let msg = first_msg(&mut stream, logon_timeout).await?;
    let session_id = SessionId::from_input_msg(&msg);
    debug!("first_msg: {msg:?}");

    let (sender, receiver) = mpsc::channel(10);
    let sender = Sender::new(sender);

    let (session_settings, session_state) = sessions
        .borrow()
        .get_session(&session_id)
        .ok_or(Error::SessionError(SessionError::UnknownSession))?;
    register_sender(session_id.clone(), sender.clone());
    let session = Rc::new(Session::new(
        settings.clone(),
        session_settings,
        session_state,
        sender,
        emitter.clone(),
    ));

    let session_span = info_span!(
        "session",
        // begin_string = %msg.header.begin_string,
        // sender_comp_id = %msg.header.target_comp_id,
        // target_comp_id = %msg.header.sender_comp_id,
        id = %session_id
    );

    let input_loop_span = info_span!(parent: &session_span, "in");
    let output_loop_span = info_span!(parent: &session_span, "out");

    session
        .on_message_in(msg)
        .instrument(input_loop_span.clone())
        .await;
    // TODO: Not here!, send this event when SessionState is created!
    emitter
        .send(FixEventInternal::Created(session_id.clone()))
        .await;

    let input_stream = input_stream(
        session.heartbeat_interval() + NO_INBOUND_TIMEOUT_PADDING,
        stream,
    );
    pin_mut!(input_stream);

    let output_stream = output_stream(session.heartbeat_interval(), receiver);
    pin_mut!(output_stream);

    let connection = Connection::new(settings, session);

    let ret = tokio::try_join!(
        connection
            .input_loop(input_stream)
            .instrument(input_loop_span),
        connection
            .output_loop(sink, output_stream)
            .instrument(output_loop_span),
    );
    info!("connection closed");
    // TODO: error here?
    connection.session.on_disconnect().await;
    unregister_sender(&session_id);
    ret.map(|_| ())
}

impl<S: MessagesStorage> Connection<S> {
    pub(crate) fn new(settings: Settings, session: Rc<Session<S>>) -> Connection<S> {
        Connection { settings, session }
    }

    async fn on_input_event(&mut self, event: InputEvent) -> Result<Option<Disconnect>, Error> {
        match event {
            InputEvent::Message(msg) => return Ok(self.session.on_message_in(msg).await),
            InputEvent::DeserializeError(error) => self.session.on_deserialize_error(error).await,
            InputEvent::IoError(error) => self.session.on_io_error(error).await.map(|_| ())?,
            InputEvent::Timeout => self.session.on_in_timeout().await,
        }
        Ok(None)
    }

    async fn input_loop(
        &self,
        mut input_stream: impl Stream<Item = InputEvent> + Unpin,
    ) -> Result<(), Error> {
        while let Some(event) = input_stream.next().await {
            match event {
                InputEvent::Message(msg) => {
                    if let Some(Disconnect) = self.session.on_message_in(msg).await {
                        error!("DISCONNECT");
                        break;
                    }
                }
                InputEvent::DeserializeError(error) => {
                    self.session.on_deserialize_error(error).await
                }
                InputEvent::IoError(error) => return self.session.on_io_error(error).await,
                InputEvent::Timeout => self.session.on_in_timeout().await,
            }
        }
        // TODO: Err(disconnected)
        Ok(())
    }

    async fn output_loop(
        &self,
        sink: impl AsyncWrite + Unpin,
        //mut receiver: Receiver<Box<FixtMessage>>,
        mut output_stream: impl Stream<Item = OutputEvent> + Unpin,
    ) -> Result<(), Error> {
        let mut sink = FramedWrite::new(sink, FixEncoder::new(self.session.clone()));

        while let Some(event) = output_stream.next().await {
            match event {
                OutputEvent::Message(msg) => match self.session.on_message_out(msg).await {
                    // TODO: handle replay msg_seq_num
                    Ok(Some(msg)) => match sink.send(msg.into()).await {
                        Ok(()) => {warn!("msg sent");}
                        Err(FixEncoderError::Io(error)) => {
                            return self.session.on_io_error(error).await
                        }
                        Err(FixEncoderError::OutboundMsgSeqNumMaxExceeded) => return Ok(()),
                    },
                    Ok(None) => {}
                    Err(_) => break,
                },
                OutputEvent::Timeout => self.session.on_out_timeout().await,
                OutputEvent::Disconnect => {
                    error!("DISCONNECT");
                    return Ok(());
                }
            }
        }
        // TODO: Internal Error here?
        //Err(Error::SessionError(SessionError::InternalError)
        Ok(())
    }

    //pub async fn run(
    //    self,
    //    source: impl AsyncRead + Unpin,
    //    sink: impl AsyncWrite + Unpin,
    //) -> Result<(), Error> {
    //    let (sender, receiver) = mpsc::channel(10);
    //    tokio::select!(
    //        ret = self.input_loop(source, sender) => ret,
    //        ret = self.output_loop(sink, receiver) => ret,
    //    )
    //}
}
