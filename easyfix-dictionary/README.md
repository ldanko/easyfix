# easyfix-dictionary

Parses FIX XML specifications into Rust data structures for inspection and code
generation.

Part of the [easyfix](https://github.com/ldanko/easyfix) FIX engine.

## Overview

The dictionary represents the FIX protocol structure:

- **Fields** — data elements with types and possible enumerated values
- **Components** — reusable groups of fields
- **Groups** — repeating sets of fields or components
- **Messages** — message types composed of fields, components, and groups

## Usage

```rust
use easyfix_dictionary::DictionaryBuilder;

let dictionary = DictionaryBuilder::new()
    .with_fix_xml("path/to/FIX50SP2.xml")
    .with_strict_check(true)
    .build()
    .expect("Failed to parse dictionary");

if let Some(field) = dictionary.field_by_name("BeginString") {
    println!("Field number: {}", field.number());
}

if let Some(message) = dictionary.message_by_name("Heartbeat") {
    println!("Message type: {}", message.msg_type());

    for member in message.members() {
        println!("  {}, required: {}", member.name(), member.required());
    }
}
```

### FIXT (FIX 5.0+)

FIX 5.0+ splits the protocol into transport (FIXT) and application layers.
Provide both XML files and use `subdictionary()` to access the application
layer:

```rust
use easyfix_dictionary::{DictionaryBuilder, Version};

let dictionary = DictionaryBuilder::new()
    .with_fixt_xml("path/to/FIXT11.xml")
    .with_fix_xml("path/to/FIX50SP2.xml")
    .build()
    .expect("Failed to parse dictionary");

if let Some(app) = dictionary.subdictionary(Version::FIX50SP2) {
    // application-level messages and fields
}
```

## License

MIT
