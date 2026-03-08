//! Base messages and base enumerations — the contract between the session
//! layer and `Message` trait implementations.
//!
//! Types in this module are used by code that implements the [`Message`] trait
//! (typically the code generator, but also custom implementations). The session
//! layer produces and consumes these types without knowing the concrete message
//! representation.
//!
//! - **Base messages** (`HeaderBase`, `AdminBase`, `LogonBase`, etc.) — minimal
//!   typed structures with only the fields the session needs. String fields use
//!   `Cow` for zero-copy borrowing on incoming and owned construction on outgoing.
//!
//! - **Base enums** (`MsgTypeBase`, `SessionStatusBase`, `SessionRejectReasonBase`,
//!   `EncryptMethodBase`) — typed constants for session-relevant FIX enumeration
//!   values. `Message` implementations convert between these and the concrete
//!   generated enums via `From` impls.
//!
//! [`Message`]: crate::message::Message

use std::borrow::Cow;

use crate::basic_types::{
    Boolean, FixStr, Int, MsgTypeField, MsgTypeValue, SeqNum, SessionRejectReasonField,
    SessionRejectReasonValue, SessionStatusField, SessionStatusValue, UtcTimestamp,
};

// ---------------------------------------------------------------------------
// MsgTypeBase (tag 35)
// ---------------------------------------------------------------------------

/// MsgType (tag 35) base enum — typed constants for session-relevant
/// admin message types. Session code dispatches on these instead of
/// raw byte comparisons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsgTypeBase {
    Heartbeat,     // "0"
    TestRequest,   // "1"
    ResendRequest, // "2"
    Reject,        // "3"
    SequenceReset, // "4"
    Logout,        // "5"
    Logon,         // "A"
}

impl MsgTypeValue for MsgTypeBase {
    fn raw_value(&self) -> MsgTypeField {
        match self {
            MsgTypeBase::Heartbeat => MsgTypeField::from_raw([b'0', 0]),
            MsgTypeBase::TestRequest => MsgTypeField::from_raw([b'1', 0]),
            MsgTypeBase::ResendRequest => MsgTypeField::from_raw([b'2', 0]),
            MsgTypeBase::Reject => MsgTypeField::from_raw([b'3', 0]),
            MsgTypeBase::SequenceReset => MsgTypeField::from_raw([b'4', 0]),
            MsgTypeBase::Logout => MsgTypeField::from_raw([b'5', 0]),
            MsgTypeBase::Logon => MsgTypeField::from_raw([b'A', 0]),
        }
    }
}

impl PartialEq<MsgTypeBase> for MsgTypeField {
    fn eq(&self, other: &MsgTypeBase) -> bool {
        *self == other.raw_value()
    }
}

impl PartialEq<MsgTypeField> for MsgTypeBase {
    fn eq(&self, other: &MsgTypeField) -> bool {
        self.raw_value() == *other
    }
}

// ---------------------------------------------------------------------------
// SessionStatus (tag 1409)
// ---------------------------------------------------------------------------

/// SessionStatus (tag 1409). FIXT Logon/Logout — session reads on incoming
/// and sets on outgoing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatusBase {
    SessionActive = 0,
    SessionLogoutComplete = 4,
    ReceivedMsgSeqNumTooLow = 9,
    ReceivedNextExpectedMsgSeqNumTooHigh = 10,
}

impl SessionStatusValue for SessionStatusBase {
    fn raw_value(&self) -> Int {
        *self as Int
    }
}

// ---------------------------------------------------------------------------
// SessionRejectReason (tag 373)
// ---------------------------------------------------------------------------

/// SessionRejectReason (tag 373). Session uses this to build outgoing Reject messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionRejectReasonBase {
    InvalidTagNumber = 0,
    RequiredTagMissing = 1,
    TagNotDefinedForThisMessageType = 2,
    UndefinedTag = 3,
    TagSpecifiedWithoutAValue = 4,
    ValueIsIncorrect = 5,
    IncorrectDataFormatForValue = 6,
    CompIdProblem = 9,
    SendingTimeAccuracyProblem = 10,
    InvalidMsgType = 11,
    TagAppearsMoreThanOnce = 13,
    TagSpecifiedOutOfRequiredOrder = 14,
    RepeatingGroupFieldsOutOfOrder = 15,
    IncorrectNumInGroupCountForRepeatingGroup = 16,
}

impl SessionRejectReasonValue for SessionRejectReasonBase {
    fn raw_value(&self) -> Int {
        *self as Int
    }
}

impl PartialEq<SessionRejectReasonBase> for SessionRejectReasonField {
    fn eq(&self, other: &SessionRejectReasonBase) -> bool {
        self.into_inner() == other.raw_value()
    }
}

// ---------------------------------------------------------------------------
// EncryptMethod (tag 98)
// ---------------------------------------------------------------------------

/// EncryptMethod (tag 98). Session sets `None` (= 0) on outgoing Logon.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncryptMethodBase {
    #[default]
    None = 0,
}

/// Base header containing only the fields the session layer reads/writes.
///
/// - **Incoming**: returned by `Message::header()` with `Cow::Borrowed` — zero allocations.
/// - **Outgoing**: built by session with `Cow::Owned`, consumed via `From` conversion.
#[derive(Clone, Debug, Default)]
pub struct HeaderBase<'a> {
    pub begin_string: Cow<'a, FixStr>,
    pub sender_comp_id: Cow<'a, FixStr>,
    pub target_comp_id: Cow<'a, FixStr>,
    pub msg_seq_num: SeqNum,
    pub sending_time: UtcTimestamp,
    pub poss_dup_flag: Option<Boolean>,
    pub orig_sending_time: Option<UtcTimestamp>,
    /// FIXT only. `None` for pre-FIXT versions.
    pub appl_ver_id: Option<Cow<'a, FixStr>>,
}

/// Admin message base — the session dispatches on this after checking `msg.try_as_admin()`.
#[derive(Clone, Debug)]
pub enum AdminBase<'a> {
    Logon(LogonBase<'a>),
    Logout(LogoutBase<'a>),
    Heartbeat(HeartbeatBase<'a>),
    TestRequest(TestRequestBase<'a>),
    ResendRequest(ResendRequestBase),
    SequenceReset(SequenceResetBase),
    Reject(RejectBase<'a>),
}

#[derive(Clone, Debug)]
pub struct LogonBase<'a> {
    /// Typed value for outgoing.
    pub encrypt_method: EncryptMethodBase,
    /// Raw value for incoming comparison/logging.
    pub encrypt_method_raw: Int,
    pub heart_bt_int: Int,
    pub reset_seq_num_flag: Option<Boolean>,
    /// `None` for FIX < 4.4.
    pub next_expected_msg_seq_num: Option<SeqNum>,
    /// `None` for pre-FIXT.
    pub default_appl_ver_id: Option<Cow<'a, FixStr>>,
    /// FIXT only.
    pub session_status: Option<SessionStatusField>,
}

#[derive(Clone, Debug)]
pub struct LogoutBase<'a> {
    /// FIXT only.
    pub session_status: Option<SessionStatusField>,
    pub text: Option<Cow<'a, FixStr>>,
}

#[derive(Clone, Debug)]
pub struct HeartbeatBase<'a> {
    pub test_req_id: Option<Cow<'a, FixStr>>,
}

#[derive(Clone, Debug)]
pub struct TestRequestBase<'a> {
    pub test_req_id: Cow<'a, FixStr>,
}

#[derive(Clone, Copy, Debug)]
pub struct ResendRequestBase {
    pub begin_seq_no: SeqNum,
    pub end_seq_no: SeqNum,
}

#[derive(Clone, Copy, Debug)]
pub struct SequenceResetBase {
    pub gap_fill_flag: Option<Boolean>,
    pub new_seq_no: SeqNum,
}

#[derive(Clone, Debug)]
pub struct RejectBase<'a> {
    pub ref_seq_num: SeqNum,
    pub ref_tag_id: Option<Int>,
    pub ref_msg_type: Option<Cow<'a, FixStr>>,
    pub session_reject_reason: Option<SessionRejectReasonField>,
    pub text: Option<Cow<'a, FixStr>>,
}
