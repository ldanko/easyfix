//! # easyfix-dictionary
//!
//! A crate for parsing and representing FIX (Financial Information Exchange) protocol dictionaries.
//!
//! This crate provides functionality to parse XML-based FIX protocol specifications into Rust structures.
//! It supports different FIX protocol versions (FIX 4.0-5.0, FIXT1.1), component and group membership,
//! field definitions with data types, and message specifications.
//!
//! ## Features
//!
//! - XML parsing of standard FIX dictionary formats
//! - Rich type representation of FIX protocol components
//! - Support for field types, message types, and component hierarchies
//! - Builder pattern for dictionary configuration
//! - Comprehensive error handling
//!
//! ## Basic Usage
//!
//! ```rust,no_run
//! use easyfix_dictionary::{DictionaryBuilder, Version};
//! use std::path::Path;
//!
//! // Parse a standard FIX dictionary
//! let dictionary = DictionaryBuilder::new()
//!     .with_fix_xml("path/to/FIX50SP2.xml")
//!     .with_strict_check(true)
//!     .build()
//!     .expect("Failed to parse dictionary");
//!
//! // Access field definitions
//! if let Some(field) = dictionary.field_by_name("BeginString") {
//!     println!("Field number: {}", field.number);
//! }
//!
//! // Access message definitions
//! if let Some(message) = dictionary.message_by_name("Heartbeat") {
//!     println!("Message type: {}", message.msg_type());
//! }
//! ```

mod dictionary;
mod xml;

// Re-export all public items from the dictionary module
pub use dictionary::{
    BasicType, Component, Dictionary, DictionaryBuilder, Error, Field, FixType, Group, Member,
    MemberDefinition, Message, MsgCat, MsgType, ParseRejectReason, Value, Version,
};
