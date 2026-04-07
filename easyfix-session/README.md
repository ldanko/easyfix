# easyfix-session

FIX session layer for the [easyfix](https://github.com/ldanko/easyfix) toolkit.

Implements the FIX Session Protocol (FIXT 1.1) with support for both
acceptor (server) and initiator (client) roles.

## Architecture

The crate is organized into three layers:

- **SessionEngine** -- pure sync protocol logic (`&mut self`, no async, no
  `Rc`). Owns session state directly. Produces output into storage-provided
  buffers or its own buffer when history is disabled.
- **Session IO loop** -- single async `select!` loop per session. Drives the
  engine, flushes output to TCP, manages timers.
- **Acceptor / Initiator** -- connection lifecycle, session registry, public
  API. Spawns one `spawn_local` task per session.

Key properties:

- One task per session -- sessions cannot block each other.
- Per-session `Application` trait -- direct function call, no channel hop on
  the hot path.
- Non-blocking staging sender -- `Sender::send` never blocks and never
  awaits. `MsgSeqNum` and `SendingTime` are stamped when the message is
  transmitted, not when it is staged. The queue is unbounded; the optional
  per-session caps (`max_outbound_queued_messages`, `max_outbound_lag`) end
  the session as a slow consumer instead of blocking the producer, and a
  producer must yield between bursts for them to fire at all.
- Async `fetch()` on the message storage -- supports both in-memory and
  database-backed storage.

## Usage

The snippets below show the shape of the API. A complete, runnable pair of
programs -- an `acceptor` and an `initiator` that talk to each other -- lives
in `examples/session` of the workspace and is built by CI; start there when
copying code.

### Application trait

Implement `Application<M>` to handle session events. Each session gets its
own instance, created by your `ApplicationFactory`.

```rust
use easyfix_session::{
    Application, ApplicationFactory, InputAction,
    DisconnectReason, Sender, SerializeError, SessionContext, SessionId,
};

struct MyApp {
    sender: Option<Sender<Message>>,
}

impl Application<Message> for MyApp {
    async fn on_session_ready(&mut self, _id: &SessionId, sender: Sender<Message>) {
        self.sender = Some(sender);
    }

    async fn on_session_end(&mut self, _id: &SessionId, _reason: DisconnectReason) {
        self.sender = None;
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        // process incoming application message
        InputAction::Accept
    }

    fn on_serialize_error(&mut self, msg: Box<Message>, error: &SerializeError) {
        // the message was not sent and will not be retried - decide here
        tracing::error!(%error, "dropping message that failed to serialize");
    }
}

struct MyFactory;

impl ApplicationFactory<Message> for MyFactory {
    type App = MyApp;

    fn create(&self, _ctx: &SessionContext<'_>) -> MyApp {
        MyApp { sender: None }
    }
}
```

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
use easyfix_session::{Acceptor, InMemoryStorage, ShutdownMode, TcpConnection};

// `Acceptor::new(MyFactory)` uses `AcceptorSettings::default()`; use
// `Acceptor::with_settings(..)` to customize listener-level values.
let acceptor = Acceptor::<Message, InMemoryStorage, MyFactory>::new(MyFactory);

// Register sessions before starting. Each session carries its identity and
// its full configuration (timeouts, buffer sizing, batch tuning).
acceptor.register_session(session_id, session_settings, |_id, max_message_size| {
    Ok(InMemoryStorage::new(max_message_size))
})?;

// Listen for connections. `shutdown` ends the listener and waits for it,
// so this handle needs neither abort nor await.
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
    MyFactory,
    |_id, max_message_size| Ok(InMemoryStorage::new(max_message_size)),
)?;

let handle = initiator.connect("exchange.example.com:9876").await?;
// The session task is running. The application receives its `Sender` in
// `on_session_ready`; for an initiator that is before the Logon response
// arrives, so anything staged there leaves the queue once the session is
// established.

// Graceful logout
initiator.logout(None, None).await?;
```

### Rejected connections

An inbound connection can die before it ever becomes a session -- an
unregistered `SenderCompID`/`TargetCompID`, a duplicate connection for an
already-active session, an undecodable first message, a peer that connects and
says nothing. No `Application` exists at that point, so these are reported to
an optional `ConnectionObserver` registered on the acceptor:

```rust
use easyfix_session::{ConnectionDropReason, ConnectionObserver};

impl ConnectionObserver for MyPolicy {
    fn on_connection_dropped(&self, peer_addr: SocketAddr, reason: ConnectionDropReason<'_>) {
        if let ConnectionDropReason::UnknownSession(id) = reason {
            // Many *distinct* unknown ids from one address is CompID
            // enumeration; one id repeating is usually a misconfigured peer.
            self.record_unknown_identity(peer_addr.ip(), id);
        }
    }
}

acceptor.set_connection_observer(Rc::new(my_policy));
```

The library reports; it does not classify or block. Enforcement belongs in your
`Connection` implementation, which can drop a blocked address before any bytes
are read -- and must, because the FIX Session Layer (4.6.4) requires an
unrecognized identity to be dropped without a reply, so as not to reveal which
identities are valid. Connections torn down by `Acceptor::shutdown` are not
reported: you initiated those.

An account that *is* recognized but fails authentication is a different case --
there the spec (4.3.10) calls for `Logout<5>` carrying a `SessionStatus(1409)`
such as `AccountLocked`, which the application returns as
`InputAction::Logout` from `on_admin_msg_in`.

### Custom transport

Both `Acceptor::run_session` and `Initiator::run_session` accept any
`impl AsyncRead + Unpin` / `impl AsyncWrite + Unpin` pair, so you can use
TCP, Unix sockets, `tokio::io::duplex` (for tests), or any other transport.

Each also takes the connection's peer address, surfaced to the application
through `SessionContext::peer_addr`. The acceptor requires one (its
`Connection` source always has it); the initiator takes an
`Option<SocketAddr>`, since a caller-supplied transport need not be a socket
-- pass the address of the underlying `TcpStream` when there is one (e.g.
under TLS), otherwise `None`.

### Message storage

The `MessagesStorage` trait supports pluggable backends:

- **`InMemoryStorage`** -- `BTreeMap`-based; every committed message stays
  retrievable, across reconnects included, so resends replay the real bytes.
  Good for development and testing.
- **Custom** -- implement the trait for shared-memory, database-backed, or
  other storage strategies.

`store` commits the prefix written by its serialization closure. Committed
records remain unchanged and available until reset; writing an occupied
sequence number fails. `fetch` returns the original bytes or an error,
including when a record is missing. `Application::should_gap_fill` can skip a
retransmission without deleting its record.

Set `SessionSettings::persist_messages` to `false` to disable message history.
The session then skips `store` and `fetch`, uses its own buffer for first
transmission, and answers recovery requests with GapFill. Counters still live
in storage and survive reconnects with that object. The remaining unsent
queue at finalization is discarded without numbering or output callbacks.
`InMemoryStorage` retains neither counters nor messages across process restart.

Backend errors end the current connection and are reported through
`on_session_end` with `StorageError`, unless an earlier disconnect reason or
an unconfirmed local reset takes precedence. Invalid historical messages and
replay serialization errors also end the connection. Ordinary serialization
errors for new messages still use `on_serialize_error`. Factories and offline
resets return `InitiatorError::Storage` or `AcceptorError::Storage`, preserving
the original backend error as a source; error types need no `Send` or `Sync`.

A failed mutation may have partially completed. Returning storage to its owner
does not certify consistency: the backend/application must recover before
reuse. There is no automatic repair, retry, atomic transaction spanning
counters and records, or autonomous notification of delayed background errors.
Durability after a process or power failure depends on the backend.

When assigning `MsgSeqNum` yourself, keep it consistent with the session's
outgoing counter. Pre-numbered messages do not advance that counter, and replay
ranges end at `next_sender_msg_seq_num - 1`. Duplicate record detection does
not synchronize numbering. With `manages_admin_output`, the application must
make externally delivered admin history available when recovery needs it, or
select `persist_messages=false`. Re-enabling history after running without it
does not fill past gaps: a missing required record remains an error.

## Runtime requirements

Requires a single-threaded tokio runtime with `LocalSet`:

```rust
let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .unwrap();
let local_set = tokio::task::LocalSet::new();
local_set.block_on(&runtime, async { /* ... */ });
```

## License

MIT
