//! Core types for session-message decoupling in easyfix.
//!
//! This crate defines the contract between the session layer and generated
//! message code: basic types, base messages, the [`SessionMessage`] trait, and
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
//! `SessionStatusBase`, etc.) - typed constants for session-relevant FIX
//! enumeration values. Used by [`SessionMessage`] implementations.
//!
//! # SessionMessage trait
//!
//! The session layer is generic over `M: SessionMessage`, so it never names a
//! concrete message type. [`SessionMessage`] and [`HeaderAccess`] are
//! implemented by generated code, bridging session logic to concrete message
//! definitions.
//!
//! [`SessionMessage`]: crate::message::SessionMessage
//! [`HeaderAccess`]: crate::message::HeaderAccess

// `variant_count` backs the compile-time completeness guards on the base
// enums' `ALL` slices in `base_messages`.
#![feature(variant_count)]

#[doc(hidden)]
pub use easyfix_macros::validate_fix_str as __validate_fix_str;

pub mod base_messages;
pub mod basic_types;
pub mod country;
pub mod currency;
pub mod deserializer;
pub mod language;
pub mod message;
pub mod serializer;
pub mod version;

/// Builds a `&'static FixStr` from a string literal.
///
/// Every byte is checked at expansion time to be printable ASCII
/// (`0x20`-`0x7e`); a literal containing anything else is a compile error
/// naming the offending position. Usable in `const` context and in ordinary
/// expressions alike, so it is the way to write a `FixStr` literal - reach
/// for `FixStr::from_ascii_unchecked` only for runtime bytes, which this
/// macro cannot see.
///
/// ```
/// # use easyfix_core::{basic_types::FixStr, fix_str};
/// const BEGIN_STRING: &FixStr = fix_str!("FIXT.1.1");
/// assert_eq!(BEGIN_STRING.as_utf8(), "FIXT.1.1");
/// ```
///
/// ```compile_fail
/// # use easyfix_core::fix_str;
/// // SOH is a control character - rejected at compile time.
/// let bad = fix_str!("FIXT\x01");
/// ```
#[macro_export]
macro_rules! fix_str {
    ($($input:tt)*) => {{
        let literal = $crate::__validate_fix_str!($($input)*);
        // SAFETY: the validator only emits printable ASCII string literals.
        unsafe {
            $crate::basic_types::FixStr::from_ascii_unchecked(literal.as_bytes())
        }
    }};
}
