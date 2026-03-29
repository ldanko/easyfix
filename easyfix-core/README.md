# easyfix-core

Core types and traits for the [easyfix](https://github.com/ldanko/easyfix) FIX
engine.

This crate defines the contract between the session layer (`easyfix-session`)
and message implementations — whether generated from XML by `easyfix-messages`
or hand-written for custom use cases.

## What's inside

### Basic types

Foundational FIX types used across the engine:

- String types: `FixStr` (borrowed), `FixString` (owned)
- Numeric types: `Int`, `Float` (`Decimal`), `SeqNum`, `TagNum`
- Temporal types: `UtcTimestamp`, `UtcTimeOnly`, `UtcDateOnly`, `TzTimestamp`,
  `TzTimeOnly` (re-exported from `chrono`)
- Other: `Boolean`, `Char`, `Country`, `Currency`
- Field newtypes: `MsgTypeField`, `SessionStatusField`,
  `SessionRejectReasonField` — opaque wrappers used in base messages and
  session APIs

### Base messages

Minimal typed structures containing only the fields the session layer needs:

- `HeaderBase` — routing and sequencing fields (BeginString, SenderCompID,
  TargetCompID, MsgSeqNum, SendingTime, etc.)
- `AdminBase` — enum over the 7 admin message types (Logon, Logout, Heartbeat,
  TestRequest, ResendRequest, SequenceReset, Reject)
- Base enumerations: `MsgTypeBase`, `SessionStatusBase`,
  `SessionRejectReasonBase`, `EncryptMethodBase`

Base messages use `Cow<FixStr>` for string fields — zero-copy borrowing on
incoming messages, owned construction on outgoing.

### Traits

- **`SessionMessage`** — core trait for message types. Provides
  deserialization, serialization, header/admin extraction, and admin message
  construction. The session is generic: `Session<M: SessionMessage>`.
- **`HeaderAccess`** — direct get/set access to header fields. Used by the
  session for filling outgoing headers, resend handling, and incoming
  validation.

### Serializer / Deserializer

FIX tag-value format encoding and decoding:

- `Deserializer` — parses raw FIX bytes into typed fields
- `Serializer` — writes typed fields to FIX tag-value format
- `RawMessage` — structurally validated message (BeginString, BodyLength,
  CheckSum checked) ready for content parsing
- `DeserializeError` — structured error with reject reason and metadata

## Implementing custom message types

For use cases where code generation from XML is not suitable, you can implement
`SessionMessage` and `HeaderAccess` directly. See the
[`dynamic_message`](examples/dynamic_message.rs) example for a complete
reference implementation that stores fields in a `HashMap`.

## Serde support

Optional features for JSON/other format serialization of core types:

```toml
[dependencies]
easyfix-core = { version = "0.1", features = ["serde-serialize", "serde-deserialize"] }
```

## License

MIT
