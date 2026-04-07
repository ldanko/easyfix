use std::{fmt, mem};

use easyfix_core::{
    basic_types::{FixStr, FixString},
    message::HeaderAccess,
    version::Version,
};

/// Identity of a FIX session: the protocol version plus the CompID pair,
/// stated from **this** side's point of view (FIX Session Layer §4.1: a
/// session is identified by BeginString(8) and the two CompIDs).
///
/// `sender_comp_id` is always us and `target_comp_id` always the
/// counterparty, whichever direction a given message travels - so both peers
/// of one link hold ids that are each other's reverse. The acceptor keys its
/// session registry on this type, which is why the CompIDs of an inbound
/// message get swapped on the way in ([`from_inbound`](Self::from_inbound)).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SessionId {
    version: Version,
    sender_comp_id: FixString,
    target_comp_id: FixString,
}

impl SessionId {
    pub fn new(
        version: Version,
        sender_comp_id: FixString,
        target_comp_id: FixString,
    ) -> SessionId {
        SessionId {
            version,
            sender_comp_id,
            target_comp_id,
        }
    }

    /// Build a `SessionId` from an inbound message.
    ///
    /// The remote's SenderCompID becomes our TargetCompID and vice versa.
    pub fn from_inbound(msg: &impl HeaderAccess) -> SessionId {
        SessionId::new(
            msg.version(),
            msg.target_comp_id().to_owned(),
            msg.sender_comp_id().to_owned(),
        )
    }

    /// Build a `SessionId` from an outbound message.
    ///
    /// SenderCompID and TargetCompID are taken as-is (no swap).
    pub fn from_outbound(msg: &impl HeaderAccess) -> SessionId {
        SessionId::new(
            msg.version(),
            msg.sender_comp_id().to_owned(),
            msg.target_comp_id().to_owned(),
        )
    }

    /// Swap the CompIDs, yielding the id the counterparty holds for this same
    /// session. The version is untouched.
    pub fn reverse_route(mut self) -> SessionId {
        mem::swap(&mut self.sender_comp_id, &mut self.target_comp_id);
        self
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn sender_comp_id(&self) -> &FixStr {
        &self.sender_comp_id
    }

    pub fn target_comp_id(&self) -> &FixStr {
        &self.target_comp_id
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} -> {}",
            self.version, self.sender_comp_id, self.target_comp_id
        )
    }
}
