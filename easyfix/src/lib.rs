//! Facade over the easyfix crates, so a consumer names one dependency
//! instead of four.
//!
//! `easyfix_core` and the [`fix_str!`] macro are always re-exported; the rest
//! sit behind features, none of them on by default:
//!
//! | feature | re-export | what it is for |
//! |---|---|---|
//! | `codegen` | `Generator` | build-time generation of message types from a FIX XML dictionary |
//! | `dictionary` | `dictionary` | parsing and inspecting those XML dictionaries at runtime |
//! | `session` | `session` | the FIX session layer (initiator / acceptor) |
//! | `full` | - | all three of the above |
//!
//! `serde-serialize` / `serde-deserialize` forward to the same features on
//! `easyfix-core`.
//!
//! Every crate behind this facade is also publishable on its own; depend on
//! them directly when a single layer is all that is needed - a build script
//! that only runs the generator, for instance, wants `easyfix-messages` and
//! nothing else.

pub use easyfix_core as core;
pub use easyfix_core::{basic_types, deserializer, fix_format, serializer};
#[cfg(feature = "dictionary")]
pub use easyfix_dictionary as dictionary;
pub use easyfix_macros::fix_str;
#[cfg(feature = "codegen")]
pub use easyfix_messages::Generator;
#[cfg(feature = "session")]
pub use easyfix_session as session;
