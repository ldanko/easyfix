//! The `Message` trait and related types.
//!
//! `Session<M: Message>` is generic over the message type. Implementations
//! provide the bridge between session logic and concrete message definitions.

use std::fmt::Debug;

use crate::{
    base_messages::{AdminBase, HeaderBase},
    basic_types::{Boolean, FixStr, FixString, MsgTypeField, SeqNum, UtcTimestamp},
    deserializer::{DeserializeError, RawMessage},
};

/// Admin vs App message category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsgCat {
    /// Administrative message
    Admin,
    /// Application message
    App,
}

/// Core message trait that bridges session logic and concrete message types.
///
/// `Session<M: SessionMessage>` uses this trait to deserialize, inspect,
/// construct, and serialize messages without knowing the concrete type.
pub trait SessionMessage: Sized + Debug + HeaderAccess {
    /// Deserialize from a structurally validated `RawMessage`.
    fn from_raw_message(raw: RawMessage<'_>) -> Result<Self, DeserializeError>;

    /// Serialize to FIX tag-value wire format.
    fn serialize(&self) -> Vec<u8>;

    /// Extract header fields as a borrowed base. Zero-copy.
    fn header(&self) -> HeaderBase<'_>;

    /// If this is an admin message, return the borrowed base. Zero-copy.
    fn try_as_admin(&self) -> Option<AdminBase<'_>>;

    /// Compact message type identifier (e.g. `"A"` for Logon, `"D"` for
    /// NewOrderSingle).
    ///
    /// Returns [`MsgTypeField`] — a compact, copyable representation that
    /// can be compared against [`MsgTypeBase`] variants for admin message
    /// dispatch, or converted to a richer type via `From` when exhaustive
    /// matching is needed.
    ///
    /// [`MsgTypeBase`]: crate::base_messages::MsgTypeBase
    fn msg_type(&self) -> MsgTypeField;

    /// Whether this is an admin or application message.
    fn msg_cat(&self) -> MsgCat;

    /// Human-readable message name (e.g., `"Logon"`, `"NewOrderSingle"`).
    fn name(&self) -> &'static str;

    /// Build an outgoing admin message from owned base messages.
    fn from_admin(header: HeaderBase<'static>, admin: AdminBase<'static>) -> Self;
}

/// Direct get/set access to header fields on `M`.
///
/// Used by session for: filling headers on outgoing app messages, setting
/// `PossDupFlag` + `OrigSendingTime` on resend, incoming validation.
pub trait HeaderAccess {
    /// BeginString (tag 8) — FIX protocol version (e.g. `"FIX.4.4"`, `"FIXT.1.1"`).
    fn begin_string(&self) -> &FixStr;

    /// SenderCompID (tag 49) — identifier of the message sender.
    fn sender_comp_id(&self) -> &FixStr;

    /// TargetCompID (tag 56) — identifier of the message recipient.
    fn target_comp_id(&self) -> &FixStr;

    /// MsgSeqNum (tag 34) — message sequence number within the session.
    fn msg_seq_num(&self) -> SeqNum;

    /// SendingTime (tag 52) — time the message was sent (UTC).
    fn sending_time(&self) -> UtcTimestamp;

    /// PossDupFlag (tag 43) — `true` if this is a possible duplicate (resend).
    fn poss_dup_flag(&self) -> Option<Boolean>;

    /// OrigSendingTime (tag 122) — original sending time for resent messages.
    /// Returns `None` if the field is absent or the FIX version doesn't define it.
    fn orig_sending_time(&self) -> Option<UtcTimestamp>;

    /// ApplVerID (tag 1128) — application-level protocol version.
    /// Only relevant for FIXT (FIX 5.0+); return `None` for FIX 4.x.
    fn appl_ver_id(&self) -> Option<&FixStr>;

    /// Set BeginString (tag 8).
    fn set_begin_string(&mut self, value: FixString);

    /// Set SenderCompID (tag 49).
    fn set_sender_comp_id(&mut self, value: FixString);

    /// Set TargetCompID (tag 56).
    fn set_target_comp_id(&mut self, value: FixString);

    /// Set MsgSeqNum (tag 34).
    fn set_msg_seq_num(&mut self, value: SeqNum);

    /// Set SendingTime (tag 52).
    fn set_sending_time(&mut self, value: UtcTimestamp);

    /// Set PossDupFlag (tag 43). Pass `None` to clear.
    fn set_poss_dup_flag(&mut self, value: Option<Boolean>);

    /// Set OrigSendingTime (tag 122). Pass `None` to clear.
    fn set_orig_sending_time(&mut self, value: Option<UtcTimestamp>);

    /// Set ApplVerID (tag 1128). Pass `None` to clear.
    /// No-op for FIX 4.x implementations.
    fn set_appl_ver_id(&mut self, value: Option<FixString>);
}
