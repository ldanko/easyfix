use std::{error::Error, fmt};

pub mod basic_types {
    pub use chrono::{Date, DateTime, NaiveDate, Utc};
    pub use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::{
        borrow::{self, Cow},
        fmt, ops,
    };

    pub type Int = i64;
    pub type TagNum = u16;
    pub type SeqNum = u32;
    pub type NumInGroup = u8;
    pub type DayOfMonth = u8;

    pub type Float = Decimal;
    pub type Qty = Float;
    pub type Price = Float;
    pub type PriceOffset = Float;
    pub type Amt = Float;
    pub type Percentage = Float;

    pub type Boolean = bool;

    pub type Char = u8;
    pub type MultipleCharValue = Vec<Char>;

    #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct FixString(Vec<u8>);
    pub type MultipleStringValue = Vec<FixString>;

    pub use crate::country::Country;
    pub use crate::currency::Currency;
    pub type Exchange = [u8; 4];
    pub type MonthYear = Vec<u8>;
    pub type Language = [u8; 2];

    pub type UtcTimestamp = DateTime<Utc>;
    pub type UtcTimeOnly = Vec<u8>;
    pub type UtcDateOnly = Date<Utc>;

    pub type LocalMktTime = u64;
    pub type LocalMktDate = NaiveDate;

    pub type TzTimestamp = Vec<u8>;
    pub type TzTimeOnly = Vec<u8>;

    pub type Length = u16;
    pub type Data = Vec<u8>;
    pub type XmlData = Data;

    pub type Tenor = Vec<u8>;

    impl FixString {
        pub fn new() -> FixString {
            FixString(Vec::new())
        }

        pub fn from_vec(buf: Vec<u8>) -> FixString {
            FixString(buf)
        }

        pub fn to_utf8_lossy(&self) -> Cow<'_, str> {
            String::from_utf8_lossy(&self.0)
        }

        pub fn into_bytes(self) -> Vec<u8> {
            self.0
        }
    }

    impl fmt::Display for FixString {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
            self.to_utf8_lossy().fmt(f)
        }
    }

    impl fmt::Debug for FixString {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "FixString({})", self)
        }
    }

    impl ops::Deref for FixString {
        type Target = [u8];

        fn deref(&self) -> &[u8] {
            self.0.deref()
        }
    }

    impl AsRef<[u8]> for FixString {
        fn as_ref(&self) -> &[u8] {
            self.0.as_ref()
        }
    }

    impl borrow::Borrow<[u8]> for FixString {
        fn borrow(&self) -> &[u8] {
            self.0.borrow()
        }
    }

    impl From<&[u8]> for FixString {
        fn from(input: &[u8]) -> FixString {
            FixString(input.into())
        }
    }

    impl From<Vec<u8>> for FixString {
        fn from(input: Vec<u8>) -> FixString {
            FixString(input)
        }
    }

    impl From<&str> for FixString {
        fn from(input: &str) -> FixString {
            // TODO: covert encoding from UTF-8 to 8859-1 (Latin-1)
            FixString(input.into())
        }
    }

    impl From<String> for FixString {
        fn from(input: String) -> FixString {
            // TODO: covert encoding from UTF-8 to 8859-1 (Latin-1)
            FixString(input.into_bytes())
        }
    }

    impl<const N: usize> From<[u8; N]> for FixString {
        fn from(input: [u8; N]) -> FixString {
            FixString(input.into())
        }
    }

    impl<const N: usize> From<&[u8; N]> for FixString {
        fn from(input: &[u8; N]) -> FixString {
            FixString(input.as_slice().into())
        }
    }

    impl PartialEq<[u8]> for FixString {
        fn eq(&self, other: &[u8]) -> bool {
            self.0.eq(other)
        }
    }

    impl PartialEq<&[u8]> for FixString {
        fn eq(&self, other: &&[u8]) -> bool {
            self.0.eq(other)
        }
    }

    impl<const N: usize> PartialEq<[u8; N]> for FixString {
        fn eq(&self, other: &[u8; N]) -> bool {
            self.0.eq(other)
        }
    }

    impl<const N: usize> PartialEq<&'_ [u8; N]> for FixString {
        fn eq(&self, other: &&[u8; N]) -> bool {
            self.0.eq(other)
        }
    }

    impl PartialEq<Vec<u8>> for FixString {
        fn eq(&self, other: &Vec<u8>) -> bool {
            self.0.eq(other)
        }
    }

    use serde::de::{self, Visitor};

    struct FixStringVisitor;

    impl<'de> Visitor<'de> for FixStringVisitor {
        type Value = FixString;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            // TODO: covert encoding from UTF-8 to 8859-1 (Latin-1)
            Ok(FixString::from(value))
        }
    }

    impl<'de> Deserialize<'de> for FixString {
        fn deserialize<D>(deserializer: D) -> Result<FixString, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(FixStringVisitor)
        }
    }

    impl Serialize for FixString {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            // TODO: covert encoding from 8859-1 (Latin-1) to UTF-8
            serializer.serialize_str(&self.to_utf8_lossy())
        }
    }
}

pub use basic_types::*;

#[derive(Debug)]
pub enum DeserializeError {
    // TODO: enum maybe?
    GarbledMessage(String),
    Logout,
    Reject {
        msg_type: Vec<u8>,
        seq_num: SeqNum,
        tag: Option<TagNum>,
        reason: RejectReason,
    },
}

impl fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeserializeError::GarbledMessage(reason) => write!(f, "garbled message: {}", reason),
            DeserializeError::Logout => write!(f, "MsgSeqNum missing, force logout"),
            DeserializeError::Reject {
                msg_type,
                seq_num,
                tag,
                reason,
            } => write!(
                f,
                "message {:?}/{} rejected: {:?} (tag={:?})",
                msg_type, seq_num, reason, tag
            ),
        }
    }
}

impl Error for DeserializeError {}

// TODO:
// enum GarbledReason

#[derive(Debug)]
pub enum RejectReason {
    InvalidTagNumber,
    RequiredTagMissing,
    TagNotDefinedForThisMessageType,
    UndefinedTag,
    TagSpecifiedWithoutAValue,
    ValueIsIncorrect,
    IncorrectDataFormatForValue,
    DecryptionProblem,
    SignatureProblem,
    CompIDProblem,
    SendingTimeAccuracyProblem,
    InvalidMsgType,
    XMLValidationError,
    TagAppearsMoreThanOnce,
    TagSpecifiedOutOfRequiredOrder,
    RepeatingGroupFieldsOutOfOrder,
    IncorrectNumInGroupCountForRepeatingGroup,
    Non,
    Invalid,
    Other,
    //0	= Invalid Tag Number[InvalidTagNumber]
    //1	= Required Tag Missing[RequiredTagMissing]
    //2	= Tag not defined for this message type[TagNotDefinedForThisMessageType]
    //3	= Undefined tag[UndefinedTag]
    //4	= Tag specified without a value[TagSpecifiedWithoutAValue]
    //5	= Value is incorrect (out of range) for this tag[ValueIsIncorrect]
    //6	= Incorrect data format for value[IncorrectDataFormatForValue]
    //7	= Decryption problem[DecryptionProblem]
    //8	= Signature problem[SignatureProblem]
    //9	= CompID problem[CompIDProblem]
    //10	= SendingTime Accuracy Problem[SendingTimeAccuracyProblem]
    //11	= Invalid MsgType[InvalidMsgType]
    //12	= XML Validation Error[XMLValidationError]
    //13	= Tag appears more than once[TagAppearsMoreThanOnce]
    //14	= Tag specified out of required order[TagSpecifiedOutOfRequiredOrder]
    //15	= Repeating group fields out of order[RepeatingGroupFieldsOutOfOrder]
    //16	= Incorrect NumInGroup count for repeating group[IncorrectNumInGroupCountForRepeatingGroup]
    //17	= Non Data value includes field delimiter (<SOH> character)[Non]
    //18	= Invalid/Unsupported Application Version[Invalid]
    //99	= Other[Other]
}

// ----------------------------------------------------------------------------

pub fn parse_u16(input: &[u8]) -> Result<u16, &[u8]> {
    let i = input;

    if i.len() == 0 {
        return Err(input);
    }

    let mut value: u16 = 0;
    for i in input {
        match i {
            b'0'..=b'9' => {
                if let Some(v) = value.checked_mul(10).and_then(|v| v.checked_add(*i as u16)) {
                    value = v;
                } else {
                    return Err(input);
                }
            }
            _ => return Err(input),
        }
    }
    Ok(value)
}

/*






fn serialize_int(&self);
fn serialize_tag_num(&self);
fn serialize_seq_num(&self);
fn serialize_num_in_group(&self);
fn serialize_dat_of_month(&self);

fn serialize_float(&self);
fn serialize_qty(&self);
fn serialize_price(&self);
fn serialize_price_offset(&self);
fn serialize_percentage(&self);

fn serialize_boolean(&self);

fn serialize_char(&self);
fn serialize_multiple_char_value(&self);

fn serialize_string(&self);
fn serialize_multiple_string_value(&self);

fn serialize_country(&self);
fn serialize_currency(&self);
fn serialize_exchange(&self);
fn serialize_month_year(&self);
fn serialize_language(&self);

fn serialize_utc_timestamp(&self);
fn serialize_utc_time_only(&self);
fn serialize_utc_data_only(&self);

fn serialize_local_mkt_time(&self);
fn serialize_local_mkt_data(&self);

fn serialize_tz_timestamp(&self);
fn serialize_tz_timeonly(&self);

fn serialize_length(&self);
fn serialize_data(&self);
fn serialize_xml(&self);

fn serialize_tenor(&self);
*/
