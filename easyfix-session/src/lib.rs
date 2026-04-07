//! FIX session layer: initiator and acceptor on single-threaded tokio.
//!
//! [`Initiator`] drives a single outbound (client) session; [`Acceptor`]
//! accepts inbound connections and matches each one to a registered
//! session, running one task per connection. Both are generic over an
//! `M` implementing [`SessionMessage`], so session logic is decoupled
//! from generated message types. Users implement [`Application`] (built
//! per session by an [`ApplicationFactory`]) for callbacks and choose a
//! [`MessagesStorage`] implementation, such as [`InMemoryStorage`], to retain
//! sequence numbers and resend data.
//! Tasks are spawned with `spawn_local`, so a tokio `LocalSet` (or other
//! local-task context) is required.
//!
//! FIX application-layer encryption and cryptographic signature verification
//! are unsupported. Session-generated Logons use `EncryptMethod(98)=0`.
//! A decoded Logon requesting another method is refused before application
//! input with `Logout<5>` and a diagnostic, then disconnected. Values outside
//! the dictionary enum follow normal decoding-error handling. Header and
//! session-state validation take precedence.
//! For transport encryption, supply a TLS stream through
//! [`Initiator::run_session`] or a custom [`Connection`] implementation.
//!
//! Third-party routing is the application's responsibility. A message
//! dictionary may expose fields such as `OnBehalfOfCompID(115)` and
//! `DeliverToCompID(128)` for the application to read and populate. The
//! application validates those addresses and forwards messages between
//! sessions. Session-generated replies do not inherit third-party routing
//! fields; the application must supply them when needed.

// Let `easyfix_session::...` paths resolve inside the crate's own tests. The
// fixtures in `tests/common/fixtures.rs` are compiled into both the
// integration binaries and these unit tests, and name the crate the way an
// external user does. Same device as `extern crate self as easyfix_core` in
// `easyfix-core`.
#[cfg(test)]
extern crate self as easyfix_session;

mod acceptor;
mod application;
mod engine;
mod initiator;
mod io;
mod messages_storage;
mod session_id;
mod settings;
#[cfg(doc)]
#[doc = include_str!("session_reset.md")]
pub mod session_reset {}
#[cfg(test)]
mod test_helpers;

pub use acceptor::{
    Acceptor, AcceptorError, Connection, ConnectionDropReason, ConnectionObserver, ShutdownMode,
    TcpConnection,
};
pub use application::{
    Application, ApplicationFactory, DisconnectReason, InputAction, SessionContext,
    invalid_heart_bt_int_range_text, invalid_heart_bt_int_text, max_message_size_exceeded_text,
};
pub use easyfix_core::{
    basic_types,
    deserializer::DeserializeErrorKind,
    fix_str,
    message::{DeserializeError, SessionMessage},
    serializer::SerializeError,
    version::Version,
};
pub use initiator::{Initiator, InitiatorError};
pub use io::sender::{SendError, Sender};
pub use messages_storage::{InMemoryStorage, InMemoryStorageError, MessagesStorage, StoreError};
pub use session_id::SessionId;
pub use settings::{AcceptorSettings, SessionSettings};
