# easyfix-core

Core types and traits for the [easyfix](https://github.com/ldanko/easyfix) FIX
engine.

This crate defines the contract between the session layer (`easyfix-session`)
and message implementations — whether generated from XML by `easyfix-messages`
or hand-written for custom use cases.

## What's inside

- Basic types: `FixStr` / `FixString`, decimal numbers, sequence and tag
  numbers, dates and times. Timestamp and time wrappers carry `TimePrecision`.
  The `fix_str!` macro validates string literals at compile time.
- Version types: `Version` and `ApplVerId`; field wrappers such as
  `MsgTypeField`, `SessionStatusField` and `SessionRejectReasonField`.
- Base messages: `HeaderBase` for routing and sequencing fields, `AdminBase`
  for the seven admin message types, and their base enumerations. String
  fields use `Cow<FixStr>` for borrowed or owned values.
- Traits: `SessionMessage` connects message implementations to the session;
  `HeaderAccess` provides header access and the protocol version.
- Wire encoding: `RawMessage` framing, `Deserializer` for typed fields and
  `Serializer` writing into a caller-provided buffer, with structured
  deserialization and serialization errors.

## Implementing custom message types

For use cases where code generation from XML is not suitable, you can implement
`SessionMessage` and `HeaderAccess` directly. See the
[`dynamic_message`](examples/dynamic_message.rs) example for a simplified
implementation backed by a `HashMap`, covering admin messages and
`NewOrderSingle`.

## Serde support

Optional `serde-serialize` and `serde-deserialize` features enable Serde
support for core types.

## License

MIT
