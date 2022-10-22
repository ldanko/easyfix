#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]

/// Message category
pub enum MsgCat {
    /// Administrative message
    Admin,
    /// Application message
    App,
}

include!(concat!(env!("OUT_DIR"), "/generated_messages.rs"));
