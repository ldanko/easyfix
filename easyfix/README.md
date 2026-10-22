# easyfix

The facade crate for the [EasyFix](https://github.com/ldanko/easyfix) toolkit.
It brings shared types, dictionary access, message generation, and session
support under one namespace. Individual crates can also be used directly.

## Available API

By default, `easyfix` exposes `easyfix-core` as `easyfix::core`, with
`basic_types`, `serializer`, `deserializer`, `fix_str!`, and `fix_format!` also
available at the crate root. No optional features are enabled by default.

| Feature | Enables |
| --- | --- |
| `codegen` | `easyfix::Generator` for generating message types from XML |
| `dictionary` | `easyfix::dictionary` for parsing and inspecting dictionaries |
| `session` | `easyfix::session` for acceptor and initiator sessions |
| `full` | `codegen`, `dictionary`, and `session` |
| `serde-serialize` | Serde serialization for core types |
| `serde-deserialize` | Serde deserialization for core types |

The Serde features are independent of `full`. Deriving Serde support for
generated messages is configured separately through the generator.

## Usage

Shared types and macros are available without optional features:

```rust
use easyfix::{basic_types::FixStr, fix_str};

const SENDER: &FixStr = fix_str!("MY_APP");
assert_eq!(SENDER.as_utf8(), "MY_APP");
```

For message generation, enable `codegen` on a build dependency or use
`easyfix-messages` directly. Set `.core_path("easyfix::core")` on the generator
or pass `--core-path easyfix::core` to its CLI so the generated source uses
the facade. See the [generator README](../easyfix-messages/README.md) for
build-script integration, CLI usage, and Serde configuration.

Enable `session` to access the session API through `easyfix::session`.
See the [session README](../easyfix-session/README.md) for runtime requirements
and application, acceptor, and initiator examples.

## License

[MIT](../LICENSE)
