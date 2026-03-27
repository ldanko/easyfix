# easyfix-messages

Rust code generator for FIX protocol message types. Takes FIX XML definitions
and produces type-safe Rust structs, enums, and serialization/deserialization
code.

Part of the [easyfix](https://github.com/ldanko/easyfix) FIX engine.

## Overview

`easyfix-messages` parses FIX XML specifications and generates Rust source files
containing:

- Field enumerations with typed values
- Message structs (admin and application)
- Repeating group structs
- Header and trailer structs
- Message type discriminator enums
- Serialization and deserialization implementations

Generated code depends on [`easyfix-core`](../easyfix-core) for runtime types
and protocol primitives.

## Usage

The crate provides both a **library API** (for `build.rs` integration) and a
**CLI binary** (for standalone code generation).

### From `build.rs`

```toml
# Cargo.toml
[dependencies]
easyfix-core = "0.6"

[build-dependencies]
easyfix-messages = "0.6"
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
  --serde-serialize       Generate #[derive(serde::Serialize)] on types
  --serde-deserialize     Generate #[derive(serde::Deserialize)] on types
  -h, --help              Print help
```

## Serde support

To derive `serde::Serialize` or `serde::Deserialize` on generated types, pass
the corresponding option to the generator:

```rust
Generator::new()
    .fix_xml("xml/FIX50SP2.xml")
    .output(out_dir.join("messages.rs"))
    .serde_serialize(true)
    .serde_deserialize(true)
    .generate()?;
```

Or via CLI:

```bash
easyfix-messages \
    --fix-xml xml/FIX50SP2.xml \
    --output src/generated/messages.rs \
    --serde-serialize \
    --serde-deserialize
```

The consuming crate must also enable the matching features on `easyfix-core`:

```toml
[dependencies]
easyfix-core = { version = "0.6", features = ["serde-serialize", "serde-deserialize"] }
```

## FIX version support

- **FIX 5.0+**: Provide both `--fixt-xml` (transport layer) and `--fix-xml`
  (application layer).
- **Pre-FIX 5.0** (FIX 4.x): Provide only `--fix-xml` — the transport and
  application layers are defined in a single XML file.

## License

MIT
