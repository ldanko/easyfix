# easyfix-messages

Rust code generator for FIX protocol message types. Takes FIX XML definitions
and produces type-safe Rust structs, enums, and serialization/deserialization
code.

Part of the [easyfix](https://github.com/ldanko/easyfix) FIX engine.

## Overview

`easyfix-messages` parses FIX XML specifications and generates a single Rust
source file containing:

- Field enumerations with typed values
- Message structs (admin and application)
- Repeating group structs
- Header and trailer structs
- Message type discriminator enums
- Serialization and deserialization implementations
- A `Message` type implementing `SessionMessage` and `HeaderAccess` for use
  with the session layer

Generated code depends on [`easyfix-core`](../easyfix-core) for runtime types
and protocol primitives, either directly or through the `easyfix` facade.
The generated module re-exports the core types and traits used by its public
API.

## Usage

The crate provides both a **library API** (for `build.rs` integration) and a
**CLI binary** (for standalone code generation).

### From `build.rs`

```toml
# Cargo.toml
[dependencies]
easyfix-core = "0.20"

[build-dependencies]
easyfix-messages = "0.20"
```

```rust
// build.rs
use easyfix_messages::Generator;
use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=xml/");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    Generator::new()
        .fixt_xml("xml/FIXT11.xml")
        .fix_xml("xml/FIX50SP2.xml")
        .output(out_dir.join("messages.rs"))
        .generate()
        .expect("FIX message generation failed");
}
```

```rust
// src/lib.rs
mod messages {
    include!(concat!(env!("OUT_DIR"), "/messages.rs"));
}
```

### From the command line

The output directory must already exist; the generator does not create it.

```bash
easyfix-messages \
    --fixt-xml xml/FIXT11.xml \
    --fix-xml xml/FIX50SP2.xml \
    --output src/generated/messages.rs
```

Then include the generated file with `mod` or `include!()`.

#### CLI options

```
easyfix-messages [OPTIONS] --fix-xml <PATH> --output <FILE>

Options:
  --fixt-xml <PATH>       Path to FIXT transport XML (e.g., FIXT11.xml)
  --fix-xml <PATH>        Path to FIX application XML (e.g., FIX50SP2.xml)
  --output <FILE>         Output path for generated Rust source file
  --core-path <PATH>      Rust path to core API (default: easyfix_core)
  --serde-serialize       Generate #[derive(serde::Serialize)] on types
  --serde-deserialize     Generate #[derive(serde::Deserialize)] on types
  --serde-serialize-feature <NAME>    Gate Serialize on a consuming crate feature
  --serde-deserialize-feature <NAME>  Gate Deserialize on a consuming crate feature
  -h, --help              Print help
```

### Using the `easyfix` facade

If the consuming crate depends on `easyfix` instead of `easyfix-core`, set
`.core_path("easyfix::core")` on the generator. This works with both
`easyfix_messages::Generator` and the `easyfix::Generator` re-export (which
requires the `codegen` feature on the build dependency).

For the CLI, pass `--core-path easyfix::core`. The default is `easyfix_core`.
A renamed dependency can use its corresponding Rust path, such as
`my_fix::core`. The path must resolve in the module containing the generated
code.

## Serde support

To derive `serde::Serialize` or `serde::Deserialize` on generated types, pass
the corresponding option to the generator:

```rust
Generator::new()
    .fixt_xml("xml/FIXT11.xml")
    .fix_xml("xml/FIX50SP2.xml")
    .output(out_dir.join("messages.rs"))
    .serde_serialize(true)
    .serde_deserialize(true)
    .generate()?;
```

Or via CLI:

```bash
easyfix-messages \
    --fixt-xml xml/FIXT11.xml \
    --fix-xml xml/FIX50SP2.xml \
    --output src/generated/messages.rs \
    --serde-serialize \
    --serde-deserialize
```

The consuming crate must also depend on `serde` with its `derive` feature and
enable the matching features on `easyfix-core`:

```toml
[dependencies]
easyfix-core = { version = "0.20", features = ["serde-serialize", "serde-deserialize"] }
serde = { version = "1", features = ["derive"] }
```

For code generated once and committed, gate the derives on features of the
consuming crate instead:

```rust
Generator::new()
    .fixt_xml("xml/FIXT11.xml")
    .fix_xml("xml/FIX50SP2.xml")
    .output("src/generated/messages.rs")
    .core_path("easyfix::core")
    .serde_serialize_feature("serde-serialize")
    .serde_deserialize_feature("serde-deserialize")
    .generate()?;
```

This emits the following attributes on the generated types:

```rust
#[cfg_attr(feature = "serde-serialize", derive(serde::Serialize))]
#[cfg_attr(feature = "serde-deserialize", derive(serde::Deserialize))]
```

Declare those features in the consuming crate and forward them to the core
types through the facade:

```toml
[features]
serde-serialize = ["dep:serde", "easyfix/serde-serialize"]
serde-deserialize = ["dep:serde", "easyfix/serde-deserialize"]

[dependencies]
easyfix = "0.20"
serde = { version = "1", optional = true, features = ["derive"] }
```

If serde is already a non-optional dependency, omit `dep:serde`. Consumers
depending directly on `easyfix-core` use its feature names instead of the
facade's and leave `core_path` at its default.

Feature names are independent of easyfix's feature names. Both directions
may use the same feature, or either direction may be omitted. Switching
features when compiling the consuming crate does not require regeneration.
Each builder setter replaces the previous setting for its direction;
`serde_serialize(false)` or `serde_deserialize(false)` disables that derive
even after a feature was configured.

The equivalent CLI options are `--serde-serialize-feature serde-serialize` and
`--serde-deserialize-feature serde-deserialize`. Each conflicts with the
unconditional flag for the same direction.

## FIX version support

- **FIXT 1.1 + FIX 5.0 SP2**: Provide both `--fixt-xml` (transport layer) and
  `--fix-xml` (application layer). The generator currently selects only the
  FIX 5.0 SP2 application dictionary; FIX 5.0 and FIX 5.0 SP1 application
  dictionaries are not supported.
- **Pre-FIX 5.0** (FIX 4.x): Provide only `--fix-xml` — the transport and
  application layers are defined in a single XML file.

## License

MIT
