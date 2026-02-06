# easyfix-dictionary

A Rust library for parsing and representing FIX (Financial Information Exchange) protocol dictionaries.

## Overview

The `easyfix-dictionary` crate provides functionality to parse XML-based FIX protocol specifications into Rust structures. It supports:

- Different FIX protocol versions
- Component and group membership
- Field definitions with data types
- Message specifications and categorization

## Features

- XML parsing of standard FIX dictionary formats
- Rich type representation of FIX protocol components
- Support for field types, message types, and component hierarchies

## Usage

### Basic Usage

```rust
use easyfix_dictionary::{DictionaryBuilder, Version};
use std::path::Path;

// Parse a standard FIX dictionary
let dictionary = DictionaryBuilder::new()
    .with_fix_xml("path/to/FIX50SP2.xml")
    .with_strict_check(true)
    .build()
    .expect("Failed to parse dictionary");

// Access field definitions
if let Some(field) = dictionary.field_by_name("BeginString") {
    println!("Field number: {}", field.number);
}

// Access message definitions
if let Some(message) = dictionary.message_by_name("Heartbeat") {
    println!("Message type: {}", message.msg_type());

    // Iterate through message members
    for member in message.members() {
        println!("Member: {}, Required: {}", member.definition().name(), member.required());
    }
}
```

### Working with Modern FIX (FIXT)

```rust
use easyfix_dictionary::{DictionaryBuilder, Version};

// Parse FIXT1.1 with application dictionaries
let dictionary = DictionaryBuilder::new()
    .with_fixt_xml("path/to/FIXT11.xml")
    .with_fix_xml("path/to/FIX50SP2.xml")
    .build()
    .expect("Failed to parse dictionary");

// Accessing the application-level subdictionary
if let Some(app_dict) = dictionary.subdictionary(Version::FIX50SP2) {
    // Use app_dict for application messages
}
```

## Dictionary Structure

The FIX dictionary consists of:

- **Fields**: Individual data elements with types and possible enum values
- **Components**: Reusable groups of fields
- **Groups**: Repeating sets of fields or components
- **Messages**: Specific message types composed of fields, components, and groups

Each element is linked through references, creating a comprehensive representation of the FIX protocol.
