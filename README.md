# easyfix

EasyFix is a Rust toolkit for building applications that exchange FIX
(Financial Information Exchange) messages. It combines message types generated
from XML dictionaries with encoding, decoding, and an asynchronous session
layer for acceptors and initiators.

Use the message types independently, or connect them to the session layer to
handle connections and session events in your application.

**The project is under active development, and its API may change without
compatibility guarantees.**

## Crates

Each crate can be used directly. The `easyfix` facade provides a common entry
point with optional features for the parts your application needs.

| Crate | Purpose |
| --- | --- |
| [easyfix](easyfix/README.md) | Facade and feature selection |
| [easyfix-core](easyfix-core/README.md) | Shared types, message traits, and encoding/decoding primitives |
| [easyfix-dictionary](easyfix-dictionary/README.md) | Parsing and inspecting FIX XML dictionaries |
| [easyfix-macros](easyfix-macros/README.md) | Compile-time FIX string literals |
| [easyfix-messages](easyfix-messages/README.md) | Rust message code generation from XML dictionaries |
| [easyfix-session](easyfix-session/README.md) | Acceptor and initiator sessions, application callbacks, and message storage |

## Try the example

The [session example](examples/session) contains a working acceptor and
initiator using generated messages. From the repository root, start the
acceptor in one terminal:

```bash
RUST_LOG=info cargo run -p session-example --bin acceptor
```

Then start the initiator in another:

```bash
RUST_LOG=info cargo run -p session-example --bin initiator
```

The initiator connects locally, sends a `NewOrderSingle`, and logs the
acceptor's `ExecutionReport` reply. Press Ctrl-C to shut down each program.

For your own application, see the [message generation guide](easyfix-messages/README.md)
and [session setup examples](easyfix-session/README.md).

## Building and testing

Nightly Rust is required. Run these commands from the repository root:

```bash
cargo build --workspace --locked
cargo test --workspace --all-features --locked
```

## License

[MIT](LICENSE)
