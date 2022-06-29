pub(crate) mod basic_types;
pub use basic_types::*;

include!(concat!(env!("OUT_DIR"), "/generated_fields.rs"));
