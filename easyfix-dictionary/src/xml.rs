//! XML parsing and representation of FIX dictionary elements.
//!
//! This module handles the deserialization of FIX XML dictionaries into
//! Rust structures using serde and quick-xml. It defines the basic types
//! and structures that represent the components of a FIX dictionary:
//! - Data types (FixType, BasicType, etc.)
//! - XML-based structures (Field, Component, Group, etc.)
//! - Serialization/deserialization helpers for FIX-specific formats

use std::{
    borrow::Borrow,
    fmt,
    hash::Hash,
    ops::Deref,
    str::{self, FromStr},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};

#[cfg(test)]
mod tests;

// Module for custom serialization of boolean "required" field
mod required_flag {
    use serde::{Deserialize, Deserializer, Serializer, de};

    // Deserialize function for required flag
    pub fn deserialize<'de, D>(deserializer: D) -> Result<bool, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "Y" | "YES" | "y" | "yes" => Ok(true),
            "N" | "NO" | "n" | "no" => Ok(false),
            _ => Err(de::Error::custom(format!(
                "invalid `required` flag value: {s}",
            ))),
        }
    }

    // Serialize function for required flag
    pub fn serialize<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = if *value { 'Y' } else { 'N' };
        serializer.serialize_char(s)
    }
}

/// A member of a message, component, or group in the FIX dictionary.
///
/// This enum represents the three possible member types in the FIX protocol:
/// - Field: A simple data element
/// - Component: A reusable collection of fields/components/groups
/// - Group: A repeating section
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Member {
    /// A field member with name and required flag
    #[serde(rename = "field")]
    Field(MemberRef),

    /// A component member with name and required flag
    #[serde(rename = "component")]
    Component(MemberRef),

    /// A group member (repeating section)
    #[serde(rename = "group")]
    Group(Group),
}

impl Member {
    pub fn name(&self) -> &str {
        match self {
            Member::Field(member_ref) => &member_ref.name,
            Member::Component(member_ref) => &member_ref.name,
            Member::Group(group) => &group.name,
        }
    }

    pub fn required(&self) -> bool {
        match self {
            Member::Field(member_ref) => member_ref.required,
            Member::Component(member_ref) => member_ref.required,
            Member::Group(group) => group.required,
        }
    }
}

/// A reference to a field or component member.
///
/// This structure represents a reference to a field or component,
/// including its name and whether it's required in its parent container.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberRef {
    /// The name of the referenced field or component
    #[serde(rename = "@name")]
    pub name: String,

    /// Whether this member is required in its parent
    #[serde(rename = "@required")]
    #[serde(with = "required_flag")]
    pub required: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Header {
    #[serde(rename = "$value")]
    #[serde(default)]
    pub members: Vec<Member>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Trailer {
    #[serde(rename = "$value")]
    #[serde(default)]
    pub members: Vec<Member>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Component {
    #[serde(rename = "@name")]
    pub name: String,
    //pub group: Option<Group>,
    #[serde(rename = "$value")]
    // enable `default`, empty members list is handled on higher layer
    #[serde(default)]
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@required")]
    #[serde(with = "required_flag")]
    pub required: bool,
    #[serde(rename = "$value")]
    pub members: Vec<Member>,
}

/// Basic data types defined in the FIX protocol.
///
/// These types define the format and validation rules for FIX field values.
/// Each field in a FIX message is associated with one of these types.
#[derive(Clone, Copy, Debug, Deserialize, Hash, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BasicType {
    /// Amount (decimal number with specific precision)
    Amt,
    /// Boolean value (Y/N)
    Boolean,
    /// Single character
    Char,
    /// Country code (ISO 3166)
    Country,
    /// Currency code (ISO 4217)
    Currency,
    /// Raw binary data
    Data,
    /// Exchange identifier
    Exchange,
    /// Floating point number
    Float,
    /// Integer number (renamed from LONG in some dictionaries)
    #[serde(alias = "LONG")]
    Int,
    /// Language identifier (ISO 639-1)
    Language,
    /// Binary data length
    Length,
    /// Local market date (YYYYMMDD)
    LocalMktDate,
    /// Month and year (YYYYMM or YYYYMMDD or YYYYMMWW)
    MonthYear,
    /// Multiple character value (space-delimited)
    MultipleCharValue,
    /// Multiple string value (space-delimited)
    MultipleStringValue,
    /// Number of entries in a repeating group
    NumInGroup,
    /// Percentage value
    Percentage,
    /// Price value (decimal number with specific precision)
    Price,
    /// Price offset value
    PriceOffset,
    /// Quantity value (decimal number with specific precision)
    Qty,
    /// Sequence number
    SeqNum,
    /// Character string (non-binary)
    String,
    /// Time with timezone
    TzTimeOnly,
    /// Timestamp with timezone
    TzTimestamp,
    /// UTC date (YYYYMMDD)
    UtcDateOnly,
    /// UTC time (HH:MM:SS.sss)
    UtcTimeOnly,
    /// UTC timestamp (YYYYMMDD-HH:MM:SS.sss)
    UtcTimestamp,
    /// XML data
    XmlData,
}

/// A field definition in the FIX dictionary.
///
/// Fields are the basic elements of FIX messages, representing individual
/// data points with specific types and possible enumerated values.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    /// The tag number that identifies this field
    #[serde(rename = "@number")]
    pub number: u16,

    /// The human-readable name of this field
    #[serde(rename = "@name")]
    pub name: String,

    /// The data type of this field
    #[serde(rename = "@type")]
    pub data_type: BasicType,

    /// Optional enumerated values for this field
    #[serde(rename = "$value")]
    pub values: Option<Vec<Value>>,
}

/// An enumerated value for a field.
///
/// Some FIX fields have a predefined set of valid values, each with a
/// specific meaning. This struct represents such a value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Value {
    /// The actual value (as it appears on the wire)
    #[serde(rename = "@enum")]
    pub value_enum: String,

    /// Human-readable description of what this value means
    #[serde(rename = "@description")]
    pub description: String,
}

/// Message category in the FIX protocol.
///
/// FIX messages are divided into two categories: administrative messages
/// for session management, and application messages for business functionality.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MsgCat {
    /// Administrative messages (session management)
    #[serde(rename = "admin")]
    Admin,

    /// Application messages (business functionality)
    #[serde(rename = "app")]
    App,
}

#[derive(Debug, thiserror::Error)]
pub enum MsgTypeError {
    #[error("Empty message type")]
    Empty,
    #[error("Invalid character in message type: {0}")]
    InvalidChar(u8),
    #[error("Message type too long: expected 1-2 bytes, got {0}")]
    TooLong(usize),
}

// Helper function to check MsgType validity
fn is_valid_char(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z')
}

#[derive(Clone, Copy, Eq)]
pub struct MsgType {
    buf: [u8; 2],
}

impl MsgType {
    pub fn from_bytes(bytes: &[u8]) -> Result<MsgType, MsgTypeError> {
        match bytes {
            [] => Err(MsgTypeError::Empty),
            [b0] => {
                if is_valid_char(*b0) {
                    Ok(MsgType { buf: [*b0, 0] })
                } else {
                    Err(MsgTypeError::InvalidChar(*b0))
                }
            }
            [b0, b1] => {
                if !is_valid_char(*b0) {
                    Err(MsgTypeError::InvalidChar(*b0))
                } else if !is_valid_char(*b1) {
                    Err(MsgTypeError::InvalidChar(*b1))
                } else {
                    Ok(MsgType { buf: [*b0, *b1] })
                }
            }
            bytes => Err(MsgTypeError::TooLong(bytes.len())),
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self.buf {
            [_, 0] => &self.buf[..1],
            [_, _] => &self.buf,
        }
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: We validate during construction that all bytes are ASCII
        //         alphanumeric (0-9, a-z, A-Z), which are all valid UTF-8
        //         characters
        unsafe { str::from_utf8_unchecked(self.as_bytes()) }
    }
}

impl fmt::Debug for MsgType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("MsgType(\"{}\")", self.as_str()))
    }
}

impl fmt::Display for MsgType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Deref for MsgType {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

impl Borrow<[u8]> for MsgType {
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl FromStr for MsgType {
    type Err = MsgTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MsgType::from_bytes(s.as_bytes())
    }
}

impl PartialEq for MsgType {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Hash for MsgType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl Serialize for MsgType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Convert MsgType to string and serialize
        let bytes = self.as_bytes();
        let s = str::from_utf8(bytes).map_err(ser::Error::custom)?;
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for MsgType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Define a visitor for deserializing into MsgType
        struct MsgTypeVisitor;

        impl<'de> de::Visitor<'de> for MsgTypeVisitor {
            type Value = MsgType;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string with 1-2 alphanumeric characters")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                MsgType::from_bytes(value.as_bytes()).map_err(|e| de::Error::custom(e.to_string()))
            }
        }

        deserializer.deserialize_str(MsgTypeVisitor)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@msgtype")]
    pub msg_type: MsgType,
    #[serde(rename = "@msgcat")]
    pub msg_cat: MsgCat,
    #[serde(rename = "$value")]
    // enable `default`, empty members list is handled on higher layer
    #[serde(default)]
    pub members: Vec<Member>,
}

fn unwrap_messages<'de, D>(deserializer: D) -> Result<Vec<Message>, D::Error>
where
    D: Deserializer<'de>,
{
    /// Represents <list>...</list>
    #[derive(Deserialize)]
    struct List {
        // default allows empty list
        //#[serde(default)]
        message: Vec<Message>,
    }
    Ok(List::deserialize(deserializer)?.message)
}

fn unwrap_components<'de, D>(deserializer: D) -> Result<Vec<Component>, D::Error>
where
    D: Deserializer<'de>,
{
    /// Represents <list>...</list>
    #[derive(Deserialize)]
    struct List {
        // default allows empty list
        #[serde(default)]
        component: Vec<Component>,
    }
    Ok(List::deserialize(deserializer)?.component)
}

fn unwrap_fields<'de, D>(deserializer: D) -> Result<Vec<Field>, D::Error>
where
    D: Deserializer<'de>,
{
    /// Represents <list>...</list>
    #[derive(Deserialize)]
    struct List {
        // default allows empty list
        //#[serde(default)]
        field: Vec<Field>,
    }
    Ok(List::deserialize(deserializer)?.field)
}

/// Type of FIX protocol.
///
/// Distinguishes between traditional FIX protocol versions and
/// the FIXT transport layer protocol introduced with FIX 5.0.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FixType {
    /// Traditional FIX protocol (FIX 4.0 - FIX 5.0)
    Fix,

    /// FIXT transport layer protocol (FIXT 1.1)
    Fixt,
}

impl fmt::Display for FixType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixType::Fix => f.write_str("FIX"),
            FixType::Fixt => f.write_str("FIXT"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Dictionary {
    #[serde(rename = "@type")]
    pub fix_type: FixType,
    #[serde(rename = "@major")]
    pub major: u8,
    #[serde(rename = "@minor")]
    pub minor: u8,
    #[serde(rename = "@servicepack")]
    pub servicepack: u8,
    pub header: Header,
    pub trailer: Trailer,
    #[serde(deserialize_with = "unwrap_messages")]
    pub messages: Vec<Message>,
    #[serde(deserialize_with = "unwrap_components")]
    pub components: Vec<Component>,
    #[serde(deserialize_with = "unwrap_fields")]
    pub fields: Vec<Field>,
}
