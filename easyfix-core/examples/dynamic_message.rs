//! A `DynamicMessage` implementation that stores fields as `HashMap<TagNum, Value>`.
//!
//! This example shows how to implement the [`SessionMessage`] and [`HeaderAccess`] traits
//! for a custom message type. Unlike the generated `Message`, this implementation
//! does not use code generation — all fields are stored dynamically in a hash map.
//!
//! This approach trades type safety for flexibility: you can handle any FIX message
//! without generating code from XML dictionaries. The downside is that field access
//! requires runtime tag lookups and value type matching instead of compile-time
//! struct field access.
//!
//! # Assumptions
//!
//! Since a successfully parsed message is guaranteed to contain all required fields,
//! this example uses `expect()` on required field lookups. This is safe because
//! `from_raw_message()` validates presence of required fields during parsing.
//!
//! # Scope
//!
//! This example supports the 7 admin message types (required by the session layer)
//! plus `NewOrderSingle` (D) as a sample application message. Extending to other
//! message types follows the same pattern.

use std::{borrow::Cow, collections::HashMap, fmt};

use easyfix_core::{
    base_messages::*,
    basic_types::*,
    deserializer::{DeserializeError, Deserializer, RawMessage},
    fix_str,
    message::{HeaderAccess, MsgCat, SessionMessage},
    serializer::Serializer,
};

// ---------------------------------------------------------------------------
// Value type — a tagged union for FIX field values
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Value {
    Int(Int),
    SeqNum(SeqNum),
    Bool(Boolean),
    Str(FixString),
    Timestamp(UtcTimestamp),
}

impl Value {
    fn as_int(&self) -> Int {
        match self {
            Value::Int(v) => *v,
            other => panic!("expected Int, got {other:?}"),
        }
    }

    fn as_seq_num(&self) -> SeqNum {
        match self {
            Value::SeqNum(v) => *v,
            other => panic!("expected SeqNum, got {other:?}"),
        }
    }

    fn as_bool(&self) -> Boolean {
        match self {
            Value::Bool(v) => *v,
            other => panic!("expected Bool, got {other:?}"),
        }
    }

    fn as_str(&self) -> &FixStr {
        match self {
            Value::Str(v) => v,
            other => panic!("expected Str, got {other:?}"),
        }
    }

    fn as_timestamp(&self) -> UtcTimestamp {
        match self {
            Value::Timestamp(v) => *v,
            other => panic!("expected Timestamp, got {other:?}"),
        }
    }
}

/// Helper for constructing `SessionRejectReasonField` from a raw integer.
///
/// `SessionRejectReasonField` can only be constructed via `From<T: SessionRejectReasonValue>`.
/// This wrapper implements the trait to pass through any integer value.
struct RawRejectReason(Int);

impl SessionRejectReasonValue for RawRejectReason {
    fn raw_value(&self) -> Int {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Well-known FIX tag numbers
// ---------------------------------------------------------------------------

const TAG_BEGIN_STRING: TagNum = 8;
const TAG_MSG_TYPE: TagNum = 35;
const TAG_SENDER_COMP_ID: TagNum = 49;
const TAG_TARGET_COMP_ID: TagNum = 56;
const TAG_MSG_SEQ_NUM: TagNum = 34;
const TAG_SENDING_TIME: TagNum = 52;
const TAG_POSS_DUP_FLAG: TagNum = 43;
const TAG_ORIG_SENDING_TIME: TagNum = 122;

// Admin message field tags
const TAG_ENCRYPT_METHOD: TagNum = 98;
const TAG_HEART_BT_INT: TagNum = 108;
const TAG_RESET_SEQ_NUM_FLAG: TagNum = 141;
const TAG_NEXT_EXPECTED_MSG_SEQ_NUM: TagNum = 789;
const TAG_TEST_REQ_ID: TagNum = 112;
const TAG_BEGIN_SEQ_NO: TagNum = 7;
const TAG_END_SEQ_NO: TagNum = 16;
const TAG_GAP_FILL_FLAG: TagNum = 123;
const TAG_NEW_SEQ_NO: TagNum = 36;
const TAG_REF_SEQ_NUM: TagNum = 45;
const TAG_REF_TAG_ID: TagNum = 371;
const TAG_REF_MSG_TYPE: TagNum = 372;
const TAG_SESSION_REJECT_REASON: TagNum = 373;
const TAG_TEXT: TagNum = 58;

// NewOrderSingle field tags (sample application message)
const TAG_CL_ORD_ID: TagNum = 11;
const TAG_SYMBOL: TagNum = 55;
const TAG_SIDE: TagNum = 54;
const TAG_ORDER_QTY: TagNum = 38;
const TAG_ORD_TYPE: TagNum = 40;
const TAG_TRANSACT_TIME: TagNum = 60;

// ---------------------------------------------------------------------------
// DynamicMessage
// ---------------------------------------------------------------------------

/// A FIX message that stores all fields in a `HashMap<TagNum, Value>`.
///
/// The `msg_type` is stored separately because it determines message identity
/// and is needed before field access (for routing, serialization, etc.).
#[derive(Clone, Debug)]
pub struct DynamicMessage {
    msg_type: MsgTypeField,
    fields: HashMap<TagNum, Value>,
}

impl DynamicMessage {
    /// Create a new message with the given MsgType.
    fn new(msg_type: MsgTypeField) -> Self {
        DynamicMessage {
            msg_type,
            fields: HashMap::new(),
        }
    }

    fn set(&mut self, tag: TagNum, value: Value) {
        self.fields.insert(tag, value);
    }

    fn get(&self, tag: TagNum) -> Option<&Value> {
        self.fields.get(&tag)
    }

    /// Classify a MsgType as admin or application.
    fn classify(msg_type: MsgTypeField) -> MsgCat {
        match msg_type.as_bytes() {
            b"0" | b"1" | b"2" | b"3" | b"4" | b"5" | b"A" => MsgCat::Admin,
            _ => MsgCat::App,
        }
    }

    /// Map MsgType to a human-readable name.
    fn msg_type_name(msg_type: MsgTypeField) -> &'static str {
        match msg_type.as_bytes() {
            b"0" => "Heartbeat",
            b"1" => "TestRequest",
            b"2" => "ResendRequest",
            b"3" => "Reject",
            b"4" => "SequenceReset",
            b"5" => "Logout",
            b"A" => "Logon",
            b"D" => "NewOrderSingle",
            _ => "Unknown",
        }
    }

    // -- Admin base conversions (incoming: borrow from self) ----------------

    fn to_admin_base(&self) -> Option<AdminBase<'_>> {
        match self.msg_type.as_bytes() {
            b"A" => Some(AdminBase::Logon(LogonBase {
                encrypt_method: EncryptMethodBase::None,
                encrypt_method_raw: self.get(TAG_ENCRYPT_METHOD).map(Value::as_int).unwrap_or(0),
                heart_bt_int: self
                    .get(TAG_HEART_BT_INT)
                    .expect("Logon requires HeartBtInt(108)")
                    .as_int(),
                reset_seq_num_flag: self.get(TAG_RESET_SEQ_NUM_FLAG).map(Value::as_bool),
                next_expected_msg_seq_num: self
                    .get(TAG_NEXT_EXPECTED_MSG_SEQ_NUM)
                    .map(Value::as_seq_num),
                default_appl_ver_id: None,
                session_status: None,
            })),
            b"5" => Some(AdminBase::Logout(LogoutBase {
                session_status: None,
                text: self.get(TAG_TEXT).map(|v| Cow::Borrowed(v.as_str())),
            })),
            b"0" => Some(AdminBase::Heartbeat(HeartbeatBase {
                test_req_id: self.get(TAG_TEST_REQ_ID).map(|v| Cow::Borrowed(v.as_str())),
            })),
            b"1" => Some(AdminBase::TestRequest(TestRequestBase {
                test_req_id: Cow::Borrowed(
                    self.get(TAG_TEST_REQ_ID)
                        .expect("TestRequest requires TestReqID(112)")
                        .as_str(),
                ),
            })),
            b"2" => Some(AdminBase::ResendRequest(ResendRequestBase {
                begin_seq_no: self
                    .get(TAG_BEGIN_SEQ_NO)
                    .expect("ResendRequest requires BeginSeqNo(7)")
                    .as_seq_num(),
                end_seq_no: self
                    .get(TAG_END_SEQ_NO)
                    .expect("ResendRequest requires EndSeqNo(16)")
                    .as_seq_num(),
            })),
            b"4" => Some(AdminBase::SequenceReset(SequenceResetBase {
                gap_fill_flag: self.get(TAG_GAP_FILL_FLAG).map(Value::as_bool),
                new_seq_no: self
                    .get(TAG_NEW_SEQ_NO)
                    .expect("SequenceReset requires NewSeqNo(36)")
                    .as_seq_num(),
            })),
            b"3" => Some(AdminBase::Reject(RejectBase {
                ref_seq_num: self
                    .get(TAG_REF_SEQ_NUM)
                    .expect("Reject requires RefSeqNum(45)")
                    .as_seq_num(),
                ref_tag_id: self.get(TAG_REF_TAG_ID).map(|v| v.as_int()),
                ref_msg_type: self
                    .get(TAG_REF_MSG_TYPE)
                    .map(|v| Cow::Borrowed(v.as_str())),
                session_reject_reason: self
                    .get(TAG_SESSION_REJECT_REASON)
                    .map(|v| RawRejectReason(v.as_int()).into()),
                text: self.get(TAG_TEXT).map(|v| Cow::Borrowed(v.as_str())),
            })),
            _ => None,
        }
    }

    // -- Admin base conversions (outgoing: consume owned base) --------------

    fn from_admin_base(admin: AdminBase<'static>) -> (MsgTypeField, HashMap<TagNum, Value>) {
        let mut fields = HashMap::new();
        let msg_type = match admin {
            AdminBase::Logon(base) => {
                fields.insert(TAG_ENCRYPT_METHOD, Value::Int(base.encrypt_method_raw));
                fields.insert(TAG_HEART_BT_INT, Value::Int(base.heart_bt_int));
                if let Some(v) = base.reset_seq_num_flag {
                    fields.insert(TAG_RESET_SEQ_NUM_FLAG, Value::Bool(v));
                }
                if let Some(v) = base.next_expected_msg_seq_num {
                    fields.insert(TAG_NEXT_EXPECTED_MSG_SEQ_NUM, Value::SeqNum(v));
                }
                MsgTypeBase::Logon.raw_value()
            }
            AdminBase::Logout(base) => {
                if let Some(v) = base.text {
                    fields.insert(TAG_TEXT, Value::Str(v.into_owned()));
                }
                MsgTypeBase::Logout.raw_value()
            }
            AdminBase::Heartbeat(base) => {
                if let Some(v) = base.test_req_id {
                    fields.insert(TAG_TEST_REQ_ID, Value::Str(v.into_owned()));
                }
                MsgTypeBase::Heartbeat.raw_value()
            }
            AdminBase::TestRequest(base) => {
                fields.insert(TAG_TEST_REQ_ID, Value::Str(base.test_req_id.into_owned()));
                MsgTypeBase::TestRequest.raw_value()
            }
            AdminBase::ResendRequest(base) => {
                fields.insert(TAG_BEGIN_SEQ_NO, Value::SeqNum(base.begin_seq_no));
                fields.insert(TAG_END_SEQ_NO, Value::SeqNum(base.end_seq_no));
                MsgTypeBase::ResendRequest.raw_value()
            }
            AdminBase::SequenceReset(base) => {
                if let Some(v) = base.gap_fill_flag {
                    fields.insert(TAG_GAP_FILL_FLAG, Value::Bool(v));
                }
                fields.insert(TAG_NEW_SEQ_NO, Value::SeqNum(base.new_seq_no));
                MsgTypeBase::SequenceReset.raw_value()
            }
            AdminBase::Reject(base) => {
                fields.insert(TAG_REF_SEQ_NUM, Value::SeqNum(base.ref_seq_num));
                if let Some(v) = base.ref_tag_id {
                    fields.insert(TAG_REF_TAG_ID, Value::Int(v));
                }
                if let Some(v) = base.ref_msg_type {
                    fields.insert(TAG_REF_MSG_TYPE, Value::Str(v.into_owned()));
                }
                if let Some(v) = base.session_reject_reason {
                    fields.insert(TAG_SESSION_REJECT_REASON, Value::Int(v.into_inner()));
                }
                if let Some(v) = base.text {
                    fields.insert(TAG_TEXT, Value::Str(v.into_owned()));
                }
                MsgTypeBase::Reject.raw_value()
            }
        };
        (msg_type, fields)
    }
}

// ---------------------------------------------------------------------------
// HeaderAccess
// ---------------------------------------------------------------------------

impl HeaderAccess for DynamicMessage {
    fn begin_string(&self) -> &FixStr {
        self.get(TAG_BEGIN_STRING)
            .expect("BeginString(8) is always present")
            .as_str()
    }

    fn sender_comp_id(&self) -> &FixStr {
        self.get(TAG_SENDER_COMP_ID)
            .expect("SenderCompID(49) is always present")
            .as_str()
    }

    fn target_comp_id(&self) -> &FixStr {
        self.get(TAG_TARGET_COMP_ID)
            .expect("TargetCompID(56) is always present")
            .as_str()
    }

    fn msg_seq_num(&self) -> SeqNum {
        self.get(TAG_MSG_SEQ_NUM)
            .expect("MsgSeqNum(34) is always present")
            .as_seq_num()
    }

    fn sending_time(&self) -> UtcTimestamp {
        self.get(TAG_SENDING_TIME)
            .expect("SendingTime(52) is always present")
            .as_timestamp()
    }

    fn poss_dup_flag(&self) -> Option<Boolean> {
        self.get(TAG_POSS_DUP_FLAG).map(Value::as_bool)
    }

    fn orig_sending_time(&self) -> Option<UtcTimestamp> {
        self.get(TAG_ORIG_SENDING_TIME).map(Value::as_timestamp)
    }

    fn appl_ver_id(&self) -> Option<&FixStr> {
        None // FIX 4.x only
    }

    fn set_begin_string(&mut self, value: FixString) {
        self.set(TAG_BEGIN_STRING, Value::Str(value));
    }

    fn set_sender_comp_id(&mut self, value: FixString) {
        self.set(TAG_SENDER_COMP_ID, Value::Str(value));
    }

    fn set_target_comp_id(&mut self, value: FixString) {
        self.set(TAG_TARGET_COMP_ID, Value::Str(value));
    }

    fn set_msg_seq_num(&mut self, value: SeqNum) {
        self.set(TAG_MSG_SEQ_NUM, Value::SeqNum(value));
    }

    fn set_sending_time(&mut self, value: UtcTimestamp) {
        self.set(TAG_SENDING_TIME, Value::Timestamp(value));
    }

    fn set_poss_dup_flag(&mut self, value: Option<Boolean>) {
        match value {
            Some(v) => self.set(TAG_POSS_DUP_FLAG, Value::Bool(v)),
            None => {
                self.fields.remove(&TAG_POSS_DUP_FLAG);
            }
        }
    }

    fn set_orig_sending_time(&mut self, value: Option<UtcTimestamp>) {
        match value {
            Some(v) => self.set(TAG_ORIG_SENDING_TIME, Value::Timestamp(v)),
            None => {
                self.fields.remove(&TAG_ORIG_SENDING_TIME);
            }
        }
    }

    fn set_appl_ver_id(&mut self, _value: Option<FixString>) {
        // No-op for FIX 4.x
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Serialize a `DynamicMessage` to FIX TagValue wire format.
fn serialize_message(msg: &DynamicMessage) -> Vec<u8> {
    let mut s = Serializer::new();

    // Tag 8: BeginString
    s.output_mut().extend_from_slice(b"8=");
    s.serialize_string(msg.begin_string());
    s.output_mut().push(b'\x01');

    // Tag 9: BodyLength placeholder
    s.serialize_body_len();

    // Tag 35: MsgType
    s.output_mut().extend_from_slice(b"35=");
    s.output_mut().extend_from_slice(msg.msg_type.as_bytes());
    s.output_mut().push(b'\x01');

    // Header fields
    serialize_tag_str(&mut s, TAG_SENDER_COMP_ID, msg.sender_comp_id());
    serialize_tag_str(&mut s, TAG_TARGET_COMP_ID, msg.target_comp_id());
    serialize_tag_seq_num(&mut s, TAG_MSG_SEQ_NUM, msg.msg_seq_num());
    serialize_tag_timestamp(&mut s, TAG_SENDING_TIME, &msg.sending_time());

    if let Some(flag) = msg.poss_dup_flag() {
        serialize_tag_bool(&mut s, TAG_POSS_DUP_FLAG, flag);
    }
    if let Some(time) = msg.orig_sending_time() {
        serialize_tag_timestamp(&mut s, TAG_ORIG_SENDING_TIME, &time);
    }

    // Body fields — serialize all non-header tags in tag-number order
    // for deterministic output.
    let header_tags = [
        TAG_BEGIN_STRING,
        TAG_MSG_TYPE,
        TAG_SENDER_COMP_ID,
        TAG_TARGET_COMP_ID,
        TAG_MSG_SEQ_NUM,
        TAG_SENDING_TIME,
        TAG_POSS_DUP_FLAG,
        TAG_ORIG_SENDING_TIME,
    ];

    let mut body_tags: Vec<_> = msg
        .fields
        .keys()
        .filter(|t| !header_tags.contains(t))
        .copied()
        .collect();
    body_tags.sort();

    for tag in body_tags {
        let value = &msg.fields[&tag];
        serialize_tag_value(&mut s, tag, value);
    }

    // Checksum (also patches body length)
    s.serialize_checksum();

    s.take()
}

fn serialize_tag_value(s: &mut Serializer, tag: TagNum, value: &Value) {
    let tag_prefix = format!("{tag}=");
    s.output_mut().extend_from_slice(tag_prefix.as_bytes());
    match value {
        Value::Int(v) => s.serialize_int(v),
        Value::SeqNum(v) => s.serialize_seq_num(v),
        Value::Bool(v) => s.serialize_boolean(v),
        Value::Str(v) => s.serialize_string(v),
        Value::Timestamp(v) => s.serialize_utc_timestamp(v),
    }
    s.output_mut().push(b'\x01');
}

fn serialize_tag_str(s: &mut Serializer, tag: TagNum, value: &FixStr) {
    let tag_prefix = format!("{tag}=");
    s.output_mut().extend_from_slice(tag_prefix.as_bytes());
    s.serialize_string(value);
    s.output_mut().push(b'\x01');
}

fn serialize_tag_seq_num(s: &mut Serializer, tag: TagNum, value: SeqNum) {
    let tag_prefix = format!("{tag}=");
    s.output_mut().extend_from_slice(tag_prefix.as_bytes());
    s.serialize_seq_num(&value);
    s.output_mut().push(b'\x01');
}

fn serialize_tag_timestamp(s: &mut Serializer, tag: TagNum, value: &UtcTimestamp) {
    let tag_prefix = format!("{tag}=");
    s.output_mut().extend_from_slice(tag_prefix.as_bytes());
    s.serialize_utc_timestamp(value);
    s.output_mut().push(b'\x01');
}

fn serialize_tag_bool(s: &mut Serializer, tag: TagNum, value: Boolean) {
    let tag_prefix = format!("{tag}=");
    s.output_mut().extend_from_slice(tag_prefix.as_bytes());
    s.serialize_boolean(&value);
    s.output_mut().push(b'\x01');
}

// ---------------------------------------------------------------------------
// Deserialization
// ---------------------------------------------------------------------------

/// Known body tags for each supported MsgType.
///
/// Tags not listed here are silently ignored. A production implementation
/// would return `TagNotDefinedForThisMessageType` for truly unknown tags.
fn known_body_tags(msg_type: &[u8]) -> &'static [TagNum] {
    match msg_type {
        b"A" => &[
            TAG_ENCRYPT_METHOD,
            TAG_HEART_BT_INT,
            TAG_RESET_SEQ_NUM_FLAG,
            TAG_NEXT_EXPECTED_MSG_SEQ_NUM,
        ],
        b"5" => &[TAG_TEXT],
        b"0" => &[TAG_TEST_REQ_ID],
        b"1" => &[TAG_TEST_REQ_ID],
        b"2" => &[TAG_BEGIN_SEQ_NO, TAG_END_SEQ_NO],
        b"4" => &[TAG_GAP_FILL_FLAG, TAG_NEW_SEQ_NO],
        b"3" => &[
            TAG_REF_SEQ_NUM,
            TAG_REF_TAG_ID,
            TAG_REF_MSG_TYPE,
            TAG_SESSION_REJECT_REASON,
            TAG_TEXT,
        ],
        b"D" => &[
            TAG_CL_ORD_ID,
            TAG_SYMBOL,
            TAG_SIDE,
            TAG_ORDER_QTY,
            TAG_ORD_TYPE,
            TAG_TRANSACT_TIME,
        ],
        _ => &[],
    }
}

/// Tags that are required for each MsgType (subset of known body tags).
fn required_body_tags(msg_type: &[u8]) -> &'static [TagNum] {
    match msg_type {
        b"A" => &[TAG_ENCRYPT_METHOD, TAG_HEART_BT_INT],
        b"1" => &[TAG_TEST_REQ_ID],
        b"2" => &[TAG_BEGIN_SEQ_NO, TAG_END_SEQ_NO],
        b"4" => &[TAG_NEW_SEQ_NO],
        b"3" => &[TAG_REF_SEQ_NUM],
        b"D" => &[
            TAG_CL_ORD_ID,
            TAG_SYMBOL,
            TAG_SIDE,
            TAG_ORDER_QTY,
            TAG_ORD_TYPE,
            TAG_TRANSACT_TIME,
        ],
        _ => &[],
    }
}

fn deserialize_message(raw: RawMessage<'_>) -> Result<DynamicMessage, DeserializeError> {
    let mut des = Deserializer::from_raw_message(raw);
    let begin_string = des.begin_string();

    // Tag 35: MsgType — the first tag in RawMessage.body (tags 8 and 9
    // are already consumed by raw_message()). First consume the tag number
    // with deserialize_tag_num(), then read the value with deserialize_msg_type().
    // Copy msg_type bytes immediately to release the borrow on `des`.
    match des.deserialize_tag_num()? {
        Some(35) => {}
        _ => {
            return Err(DeserializeError::GarbledMessage(
                "expected MsgType(35) as the first tag after BodyLength".into(),
            ));
        }
    }
    let msg_type_range = des.deserialize_msg_type()?;
    let msg_type_bytes: Vec<u8> = des.range_to_fixstr(msg_type_range).as_bytes().to_vec();
    let msg_type = MsgTypeField::from_bytes(&msg_type_bytes)
        .map_err(|_| des.reject(None, SessionRejectReasonBase::InvalidMsgType))?;

    let known_tags = known_body_tags(&msg_type_bytes);

    let mut msg = DynamicMessage::new(msg_type);
    msg.set(TAG_BEGIN_STRING, Value::Str(begin_string));

    // Parse header and body tags in one loop. The FIX spec says header tags
    // come first, but many implementations are lenient about ordering.
    while let Some(tag) = des.deserialize_tag_num()? {
        match tag {
            TAG_SENDER_COMP_ID => {
                msg.set(tag, Value::Str(des.deserialize_string()?));
            }
            TAG_TARGET_COMP_ID => {
                msg.set(tag, Value::Str(des.deserialize_string()?));
            }
            TAG_MSG_SEQ_NUM => {
                let seq = des.deserialize_seq_num()?;
                des.set_seq_num(seq);
                msg.set(tag, Value::SeqNum(seq));
            }
            TAG_SENDING_TIME => {
                msg.set(tag, Value::Timestamp(des.deserialize_utc_timestamp()?));
            }
            TAG_POSS_DUP_FLAG => {
                msg.set(tag, Value::Bool(des.deserialize_boolean()?));
            }
            TAG_ORIG_SENDING_TIME => {
                msg.set(tag, Value::Timestamp(des.deserialize_utc_timestamp()?));
            }
            // Body tags — dispatch by type based on what we know about the tag
            t if known_tags.contains(&t) => {
                let value = deserialize_field_value(&mut des, t)?;
                msg.set(t, value);
            }
            // Unknown tags — skip by reading the value as a string
            _ => {
                let _ = des.deserialize_string()?;
            }
        }
    }

    // Validate required body tags
    for &tag in required_body_tags(&msg_type_bytes) {
        if msg.get(tag).is_none() {
            return Err(des.reject(Some(tag), SessionRejectReasonBase::RequiredTagMissing));
        }
    }

    Ok(msg)
}

/// Deserialize a body field value based on its tag number.
///
/// This mapping defines the expected type for each supported tag. In a full
/// implementation you would derive this from the FIX dictionary XML.
fn deserialize_field_value(
    des: &mut Deserializer<'_>,
    tag: TagNum,
) -> Result<Value, DeserializeError> {
    match tag {
        // Int fields
        TAG_ENCRYPT_METHOD | TAG_REF_TAG_ID | TAG_SESSION_REJECT_REASON => {
            Ok(Value::Int(des.deserialize_int()?))
        }
        // SeqNum fields
        TAG_HEART_BT_INT => Ok(Value::Int(des.deserialize_int()?)),
        TAG_BEGIN_SEQ_NO
        | TAG_END_SEQ_NO
        | TAG_NEW_SEQ_NO
        | TAG_NEXT_EXPECTED_MSG_SEQ_NUM
        | TAG_REF_SEQ_NUM => Ok(Value::SeqNum(des.deserialize_seq_num()?)),
        // Boolean fields
        TAG_RESET_SEQ_NUM_FLAG | TAG_GAP_FILL_FLAG => Ok(Value::Bool(des.deserialize_boolean()?)),
        // Timestamp fields
        TAG_TRANSACT_TIME => Ok(Value::Timestamp(des.deserialize_utc_timestamp()?)),
        // String fields (default for everything else)
        _ => Ok(Value::Str(des.deserialize_string()?)),
    }
}

// ---------------------------------------------------------------------------
// Message trait implementation
// ---------------------------------------------------------------------------

impl fmt::Display for DynamicMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}({})",
            Self::msg_type_name(self.msg_type),
            self.msg_type
        )
    }
}

impl SessionMessage for DynamicMessage {
    fn from_raw_message(raw: RawMessage<'_>) -> Result<Self, DeserializeError> {
        deserialize_message(raw)
    }

    fn serialize(&self) -> Vec<u8> {
        serialize_message(self)
    }

    fn header(&self) -> HeaderBase<'_> {
        HeaderBase {
            begin_string: Cow::Borrowed(self.begin_string()),
            sender_comp_id: Cow::Borrowed(self.sender_comp_id()),
            target_comp_id: Cow::Borrowed(self.target_comp_id()),
            msg_seq_num: self.msg_seq_num(),
            sending_time: self.sending_time(),
            poss_dup_flag: self.poss_dup_flag(),
            orig_sending_time: self.orig_sending_time(),
            appl_ver_id: None,
        }
    }

    fn try_as_admin(&self) -> Option<AdminBase<'_>> {
        self.to_admin_base()
    }

    fn msg_type(&self) -> MsgTypeField {
        self.msg_type
    }

    fn msg_cat(&self) -> MsgCat {
        Self::classify(self.msg_type)
    }

    fn name(&self) -> &'static str {
        Self::msg_type_name(self.msg_type)
    }

    fn from_admin(header: HeaderBase<'static>, admin: AdminBase<'static>) -> Self {
        let (msg_type, mut fields) = Self::from_admin_base(admin);
        // Populate header fields
        fields.insert(
            TAG_BEGIN_STRING,
            Value::Str(header.begin_string.into_owned()),
        );
        fields.insert(
            TAG_SENDER_COMP_ID,
            Value::Str(header.sender_comp_id.into_owned()),
        );
        fields.insert(
            TAG_TARGET_COMP_ID,
            Value::Str(header.target_comp_id.into_owned()),
        );
        fields.insert(TAG_MSG_SEQ_NUM, Value::SeqNum(header.msg_seq_num));
        fields.insert(TAG_SENDING_TIME, Value::Timestamp(header.sending_time));
        if let Some(v) = header.poss_dup_flag {
            fields.insert(TAG_POSS_DUP_FLAG, Value::Bool(v));
        }
        if let Some(v) = header.orig_sending_time {
            fields.insert(TAG_ORIG_SENDING_TIME, Value::Timestamp(v));
        }
        DynamicMessage { msg_type, fields }
    }
}

// ---------------------------------------------------------------------------
// Demo
// ---------------------------------------------------------------------------

fn main() {
    // Build a Logon message manually
    let mut logon = DynamicMessage::new(MsgTypeBase::Logon.raw_value());
    logon.set(TAG_BEGIN_STRING, Value::Str(fix_str!("FIX.4.4").to_owned()));
    logon.set(
        TAG_SENDER_COMP_ID,
        Value::Str(fix_str!("SENDER").to_owned()),
    );
    logon.set(
        TAG_TARGET_COMP_ID,
        Value::Str(fix_str!("TARGET").to_owned()),
    );
    logon.set(TAG_MSG_SEQ_NUM, Value::SeqNum(1));
    logon.set(TAG_SENDING_TIME, Value::Timestamp(UtcTimestamp::now()));
    logon.set(TAG_ENCRYPT_METHOD, Value::Int(0));
    logon.set(TAG_HEART_BT_INT, Value::Int(30));

    println!("Message: {logon}");
    println!("MsgCat:  {:?}", logon.msg_cat());
    println!("Name:    {}", logon.name());

    // Serialize
    let wire = logon.serialize();
    let wire_display = String::from_utf8_lossy(&wire).replace('\x01', "|");
    println!("Wire:    {wire_display}");

    // Round-trip: parse the serialized bytes back
    let (_, raw) = easyfix_core::deserializer::raw_message(&wire).expect("valid framing");
    let parsed = DynamicMessage::from_raw_message(raw).expect("valid message");

    println!("\nRound-trip OK:");
    println!("  MsgType:      {}", parsed.msg_type());
    println!("  SenderCompID: {}", parsed.sender_comp_id());
    println!("  TargetCompID: {}", parsed.target_comp_id());
    println!("  MsgSeqNum:    {}", parsed.msg_seq_num());
    println!(
        "  HeartBtInt:   {}",
        parsed.get(TAG_HEART_BT_INT).unwrap().as_int()
    );

    // Verify admin base conversion
    let admin = parsed.try_as_admin().expect("Logon is an admin message");
    match admin {
        AdminBase::Logon(logon_base) => {
            println!(
                "  AdminBase:    Logon(heart_bt_int={})",
                logon_base.heart_bt_int
            );
        }
        _ => panic!("expected Logon"),
    }

    println!("\nAll checks passed!");
}
