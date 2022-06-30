pub use chrono::{Date, DateTime, NaiveDate, NaiveTime, Utc};
pub use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{borrow, fmt, ops, mem};

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

#[derive(Clone, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FixString(Vec<u8>);
#[derive(Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct FixStr([u8]);
pub type MultipleStringValue = Vec<FixString>;

pub use crate::country::Country;
pub use crate::currency::Currency;
pub type Exchange = [u8; 4];
pub type MonthYear = Vec<u8>;
pub type Language = [u8; 2];

pub type UtcTimestamp = DateTime<Utc>;
pub type UtcTimeOnly = Vec<u8>;
pub type UtcDateOnly = Date<Utc>;

pub type LocalMktTime = NaiveTime;
pub type LocalMktDate = NaiveDate;

pub type TzTimestamp = Vec<u8>;
pub type TzTimeOnly = Vec<u8>;

pub type Length = u16;
pub type Data = Vec<u8>;
pub type XmlData = Data;

pub type Tenor = Vec<u8>;

#[derive(Debug)]
pub struct FixStringError {
    idx: usize,
    value: u8,
}

impl fmt::Display for FixStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Unexpected character '{:#04x}' at idx {}",
            self.value, self.idx
        )
    }
}

impl std::error::Error for FixStringError {}

impl FixStr {
    pub fn from_ascii(buf: &[u8]) -> Result<&FixStr, FixStringError> {
        for i in 0..buf.len() {
            // SAFETY: `i` never exceeds buf.len()
            let c = unsafe { *buf.get_unchecked(i) };
            if c < 0x20 || c > 0x7f {
                return Err(FixStringError { idx: i, value: c });
            }
        }
        unsafe {
            Ok(FixStr::from_ascii_unchecked(buf))
        }
    }

    pub unsafe fn from_ascii_unchecked(buf: &[u8]) -> &FixStr {
        // SAFETY: the caller must guarantee that the bytes `v` are valid UTF-8.
        // Also relies on `&FixStr` and `&[u8]` having the same layout.
        mem::transmute(buf)
    }

    pub fn as_utf8(&self) -> &str {
        // SAFETY: ASCII is always valid UTF-8
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for FixStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        self.as_utf8().fmt(f)
    }
}

impl fmt::Debug for FixStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FixStr(\"{}\")", self)
    }
}

impl AsRef<FixStr> for FixStr {
    fn as_ref(&self) -> &FixStr {
        self
    }
}

impl AsRef<[u8]> for FixStr {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl AsRef<str> for FixStr {
    fn as_ref(&self) -> &str {
        self.as_utf8()
    }
}

impl ToOwned for FixStr {
    type Owned = FixString;

    #[inline]
    fn to_owned(&self) -> FixString {
        unsafe { FixString::from_ascii_unchecked(self.as_bytes().to_owned()) }
    }

    fn clone_into(&self, target: &mut FixString) {
        let mut buf = mem::take(target).into_bytes();
        self.as_bytes().clone_into(&mut buf);
        *target = unsafe { FixString::from_ascii_unchecked(buf) }
    }
}

impl PartialEq<[u8]> for FixStr {
    fn eq(&self, other: &[u8]) -> bool {
        self.0.eq(other)
    }
}

impl PartialEq<&[u8]> for FixStr {
    fn eq(&self, other: &&[u8]) -> bool {
        self.0.eq(*other)
    }
}

impl<const N: usize> PartialEq<[u8; N]> for FixStr {
    fn eq(&self, other: &[u8; N]) -> bool {
        self.0.eq(other)
    }
}

impl<const N: usize> PartialEq<&'_ [u8; N]> for FixStr {
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.0.eq(*other)
    }
}

impl PartialEq<Vec<u8>> for FixStr {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.0.eq(other)
    }
}

impl PartialEq<&str> for FixStr {
    fn eq(&self, other: &&str) -> bool {
        self.0.eq(other.as_bytes())
    }
}

impl PartialEq<str> for FixStr {
    fn eq(&self, other: &str) -> bool {
        self.0.eq(other.as_bytes())
    }
}

impl PartialEq<String> for FixStr {
    fn eq(&self, other: &String) -> bool {
        self.0.eq(other.as_bytes())
    }
}

// TODO: Optional feature for ISO 8859-1 encoded strings
impl FixString {
    pub fn new() -> FixString {
        FixString(Vec::new())
    }

    pub fn from_ascii(buf: Vec<u8>) -> Result<FixString, FixStringError> {
        for i in 0..buf.len() {
            // SAFETY: `i` never exceeds buf.len()
            let c = unsafe { *buf.get_unchecked(i) };
            if c < 0x20 || c > 0x7f {
                return Err(FixStringError { idx: i, value: c });
            }
        }
        Ok(FixString(buf))
    }

    pub unsafe fn from_ascii_unchecked(buf: Vec<u8>) -> FixString {
        FixString(buf)
    }

    pub fn from_ascii_lossy(mut buf: Vec<u8>) -> FixString {
        for i in 0..buf.len() {
            // SAFETY: `i` never exceeds buf.len()
            let c = unsafe { buf.get_unchecked_mut(i) };
            if *c < 0x20 || *c > 0x7f {
                *c = b'?';
            }
        }
        FixString(buf)
    }

    pub fn as_utf8(&self) -> &str {
        // SAFETY: ASCII is always valid UTF-8
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    pub fn into_utf8(self) -> String {
        // SAFETY: ASCII is always valid UTF-8
        unsafe { String::from_utf8_unchecked(self.0) }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for FixString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        self.as_utf8().fmt(f)
    }
}

impl fmt::Debug for FixString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FixString(\"{}\")", self)
    }
}

impl ops::Deref for FixString {
    type Target = FixStr;

    fn deref(&self) -> &FixStr {
        unsafe { FixStr::from_ascii_unchecked(&self.0) }
    }
}

impl AsRef<FixStr> for FixString {
    fn as_ref(&self) -> &FixStr {
        self
    }
}

impl AsRef<[u8]> for FixString {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl AsRef<str> for FixString {
    fn as_ref(&self) -> &str {
        self.as_utf8()
    }
}

impl borrow::Borrow<FixStr> for FixString {
    fn borrow(&self) -> &FixStr {
        self
    }
}

impl From<&FixStr> for FixString {
    fn from(input: &FixStr) -> FixString {
        input.to_owned()
    }
}

impl From<&[u8]> for FixString {
    fn from(input: &[u8]) -> FixString {
        FixString(input.into())
    }
}

impl TryFrom<Vec<u8>> for FixString {
    type Error = FixStringError;

    fn try_from(buf: Vec<u8>) -> Result<FixString, Self::Error> {
        FixString::from_ascii(buf)
    }
}

impl TryFrom<&str> for FixString {
    type Error = FixStringError;

    fn try_from(buf: &str) -> Result<FixString, Self::Error> {
        FixString::from_ascii(buf.as_bytes().to_owned())
    }
}

impl TryFrom<String> for FixString {
    type Error = FixStringError;

    fn try_from(buf: String) -> Result<FixString, Self::Error> {
        FixString::from_ascii(buf.into_bytes())
    }
}

impl<const N: usize> TryFrom<[u8; N]> for FixString {
    type Error = FixStringError;

    fn try_from(buf: [u8; N]) -> Result<FixString, Self::Error> {
        FixString::from_ascii(buf.to_vec())
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

impl PartialEq<&str> for FixString {
    fn eq(&self, other: &&str) -> bool {
        self.0.eq(other.as_bytes())
    }
}

impl PartialEq<str> for FixString {
    fn eq(&self, other: &str) -> bool {
        self.0.eq(other.as_bytes())
    }
}

impl PartialEq<String> for FixString {
    fn eq(&self, other: &String) -> bool {
        self.0.eq(other.as_bytes())
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
        value.try_into().map_err(de::Error::custom)
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
        serializer.serialize_str(&self.as_utf8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_string_fail_on_ctrl_character() {
        let buf = b"Hello\x01world!".to_vec();
        assert!(FixString::from_ascii(buf).is_err());
    }

    #[test]
    fn fix_string_fail_on_out_of_range_character() {
        let buf = b"Hello\x85world!".to_vec();
        assert!(FixString::from_ascii(buf).is_err());
    }

    #[test]
    fn fix_string_replacemen_character_on_ctrl() {
        let buf = b"Hello\x01world!".to_vec();
        assert_eq!(FixString::from_ascii_lossy(buf), "Hello?world!");
    }

    #[test]
    fn fix_string_replacemen_character_on_out_of_range() {
        let buf = b"Hello\x85world!".to_vec();
        assert_eq!(FixString::from_ascii_lossy(buf), "Hello?world!");
    }
}
