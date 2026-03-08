//! Core types for session-message decoupling in easyfix.
//!
//! This crate defines the contract between the session layer and generated
//! message code: basic types, base messages, the `Message` trait, and
//! deserialization error types.
//!
//! # Basic types
//!
//! Foundational FIX types (`FixStr`, `FixString`, `SeqNum`, `UtcTimestamp`,
//! etc.) and user-facing field newtypes (`MsgTypeField`, `SessionStatusField`,
//! `SessionRejectReasonField`) with their corresponding value traits.
//!
//! # Base messages
//!
//! Minimal typed structures (`HeaderBase`, `AdminBase`, etc.) containing only
//! the fields the session needs, plus base enumerations (`MsgTypeBase`,
//! `SessionStatusBase`, etc.) — typed constants for session-relevant FIX
//! enumeration values. Used by `Message` trait implementations.
//!
//! # Message trait
//!
//! `Session<M: Message>` is generic over the message type. The `Message` and
//! `HeaderAccess` traits are implemented by generated code, bridging session
//! logic to concrete message definitions.

// Make `::easyfix_core` resolve within this crate (including examples and
// tests). Required by the `fix_str!` proc macro which emits
// `::easyfix_core::basic_types::FixStr::from_ascii_unchecked(...)`.
extern crate self as easyfix_core;

pub use easyfix_macros::fix_str;

pub mod base_messages;
pub mod basic_types;
pub mod country;
pub mod currency;
pub mod deserializer;
pub mod message;
pub mod serializer;
