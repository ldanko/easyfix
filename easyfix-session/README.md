# easyfix-session

FIX session layer for the [easyfix](https://github.com/ldanko/easyfix) toolkit,
supporting acceptor (server) and initiator (client) roles.

Handles session establishment, heartbeats, message sequencing and recovery.
Applications provide message types, handle session events through callbacks,
and choose a storage backend for session state and message history.

## Runtime requirements

The standard connection methods spawn local tasks. Run them inside a tokio
`LocalSet`, for example on a single-threaded runtime:

```rust
let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();
let local_set = tokio::task::LocalSet::new();
local_set.block_on(&runtime, async { /* ... */ });
```

Callbacks should return promptly; blocking code can stall all sessions on the
thread. The outgoing queue is unbounded by default. Optional slow-consumer
limits are configured through `SessionSettings`; producers must yield for
those limits to be checked.

## Usage

The snippets below show the shape of the API. A complete, runnable pair of
programs -- an [acceptor](../examples/session/src/bin/acceptor.rs) and an
[initiator](../examples/session/src/bin/initiator.rs) -- lives in
`examples/session` and is built by CI. `Message` below is your message type
implementing `SessionMessage`, generated from XML or implemented by hand.

### Session handler

Implement `SessionHandler<M>` to handle session events. Pass a `make_handler`
async closure to the initiator or acceptor constructor; it creates a fresh
handler for each connection, including reconnects. The closure receives a
`&SessionContext<'_>` with the session identity, peer address, and timestamp
precision. It can await initialization and returns
`Result<H, Box<dyn std::error::Error + 'static>>`, where `H` implements
`SessionHandler<M>`.

```rust
use easyfix_session::{
    InputAction, Sender, SerializeError, SessionContext, SessionEnd,
    SessionHandler, SessionId,
};

struct MyHandler {
    sender: Option<Sender<Message>>,
}

impl SessionHandler<Message> for MyHandler {
    async fn on_session_ready(&mut self, _id: &SessionId, sender: Sender<Message>) {
        self.sender = Some(sender);
    }

    async fn on_session_end(&mut self, _id: &SessionId, end: SessionEnd) {
        self.sender = None;
        tracing::info!(?end, "session connection ended");
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        // process incoming application message
        InputAction::Accept
    }

    fn on_output_error(&mut self, msg: Box<Message>, error: &SerializeError) {
        // the message was not sent and will not be retried - decide here
        tracing::error!(%error, "dropping message that failed to serialize");
    }
}

let make_handler = async |_ctx: &SessionContext<'_>| {
    Ok(MyHandler { sender: None })
};
```

Neither the closure nor the handler needs to be `Send` or `Sync`.

Pending initialization is cancelled by a logout, disconnect, or initiator
`close`, and by any acceptor shutdown mode. Failure or cancellation before a
handler is constructed produces no handler callbacks. Initialization must
tolerate cancellation and enforce any required timeout.

The optional `on_input_error` callback reports decoding and session validation
failures; the session chooses the response.
`on_output_error` returns ownership of a message that failed to serialize;
transport and storage failures are reported through `on_session_end`.

### Session identity and settings

Both roles take a `SessionId` (protocol version plus the CompID pair, seen
from our side) and a `SessionSettings`. The types they are built from are
re-exported by this crate.

```rust
use easyfix_session::{SessionId, SessionSettings, Version, fix_str};

let session_id = SessionId::new(
    Version::FIXT11,
    fix_str!("SERVER").to_owned(),
    fix_str!("CLIENT").to_owned(),
);
let session_settings = SessionSettings::default();
```

### Acceptor (server)

```rust
use std::net::SocketAddr;

use easyfix_session::{Acceptor, InMemoryStorage, ShutdownMode, TcpConnection};

// `Acceptor::new(make_handler)` uses `AcceptorSettings::default()`; use
// `Acceptor::with_settings(..)` to customize listener-level values.
let acceptor = Acceptor::<Message, InMemoryStorage, _>::new(make_handler);

// Register sessions before starting.
acceptor.register_session(session_id, session_settings, |_id, max_message_size| {
    Ok(InMemoryStorage::new(max_message_size))
})?;

// `shutdown` stops the listener and waits for it.
let conn = TcpConnection::new("0.0.0.0:9876".parse::<SocketAddr>()?).await?;
let _handle = acceptor.start(conn);

// Graceful shutdown - terminal; build a new Acceptor to serve again.
acceptor.shutdown(ShutdownMode::GracefulLogout {
    session_status: None,
    text: None,
}).await;
```

### Initiator (client)

```rust
use easyfix_session::{InMemoryStorage, Initiator};

let initiator = Initiator::new(
    session_id,
    session_settings,
    make_handler,
    |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
)?;

let handle = initiator.connect("exchange.example.com:9876");
// `on_session_ready` provides a Sender before the Logon response arrives.
// Staged application messages wait until the session is established.

// When ready to stop, request logout and wait for the session task to finish.
initiator.logout(None, None, false).await?;
handle.await??;
```

`connect` returns immediately; the task establishes TCP and initializes the
handler. The task returns `Result<(), InitiatorError>`, including connection
and initialization errors. `session_task` returns a future with the same
output when using a caller-provided executor. `close` also cancels a pending
connection attempt.

### Rejected connections

Use an optional `ConnectionObserver` to observe connections dropped before
session handling starts, such as unknown identities, duplicate connections,
timeouts, or handler initialization failures. Once session handling starts,
its connection's end is reported through `SessionHandler::on_session_end`.

```rust
use std::{net::SocketAddr, rc::Rc};

use easyfix_session::{ConnectionEnd, ConnectionObserver};

struct MyObserver;

impl ConnectionObserver for MyObserver {
    fn on_connection_dropped(&self, peer_addr: SocketAddr, end: ConnectionEnd<'_>) {
        tracing::warn!(%peer_addr, ?end, "connection dropped before session start");
    }
}

acceptor.set_connection_observer(Rc::new(MyObserver));
```

The observer is informational; address filtering belongs in a custom
`Connection`. Connections closed by `Acceptor::shutdown` are not reported.
Application authentication can refuse a Logon through `InputAction::Logout`
from `on_admin_msg_in`.

### Custom transport

Both `Acceptor::run_session` and `Initiator::run_session` accept any
`impl AsyncRead + Unpin` / `impl AsyncWrite + Unpin` pair, so you can use
TCP, Unix sockets, `tokio::io::duplex` (for tests), or any other transport.

Both expose the peer address through `SessionContext::peer_addr`. The acceptor
requires a `SocketAddr`; the initiator accepts `Option<SocketAddr>` for
transports without a socket address.

### Message storage

`MessagesStorage` holds sequence counters and outgoing message history.
The supplied `InMemoryStorage` retains them across reconnects using the same
object, but not across process restarts. Implement a custom backend when
durable storage is needed; see the [storage contract](src/messages_storage.rs).

`SessionSettings::persist_messages = false` disables message history while
retaining sequence counters. Recovery requests are answered with GapFill
instead of replaying messages. See [session settings](src/settings.rs) for
retention and recovery options.

## License

MIT
