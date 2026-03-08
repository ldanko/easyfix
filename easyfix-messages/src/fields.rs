// Re-export basic_types module from easyfix-core.
pub mod basic_types {
    pub use easyfix_core::basic_types::*;
}
pub use basic_types::*;

include!(concat!(env!("OUT_DIR"), "/generated_fields.rs"));
