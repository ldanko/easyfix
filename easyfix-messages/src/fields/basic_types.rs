use std::{borrow, fmt, mem, ops};

use chrono::Timelike;
pub use chrono::{
    format::{DelayedFormat, StrftimeItems},
    DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
};
pub use rust_decimal::Decimal;
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
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

#[derive(Clone, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FixString(Vec<u8>);

#[derive(Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct FixStr([u8]);

pub type MultipleStringValue = Vec<FixString>;

pub use crate::{country::Country, currency::Currency};
pub type Exchange = [u8; 4];
// TODO: don't use Vec here
pub type MonthYear = Vec<u8>;
pub type Language = [u8; 2];

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TimePrecision {
    Secs = 0,
    Millis = 3,
    Micros = 6,
    #[default]
    Nanos = 9,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UtcTimestamp {
    timestamp: DateTime<Utc>,
    precision: TimePrecision,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct UtcTimeOnly {
    timestamp: NaiveTime,
    precision: TimePrecision,
}
pub type UtcDateOnly = NaiveDate;

pub type LocalMktTime = NaiveTime;
pub type LocalMktDate = NaiveDate;

// TODO: don't use Vec here
pub type TzTimestamp = Vec<u8>;
pub type TzTimeOnly = Vec<u8>;

pub type Length = u16;
pub type Data = Vec<u8>;
pub type XmlData = Data;

// TODO: don't use Vec here
pub type Tenor = Vec<u8>;

#[derive(Debug)]
pub struct FixStringError {
    idx: usize,
    value: u8,
}

impl FixStringError {
    /// Returns the index of unexpected character.
    pub fn idx(&self) -> usize {
        self.idx
    }

    /// Returns the value of unexpected character.
    pub fn value(&self) -> u8 {
        self.value
    }
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

const fn is_non_control_ascii_char(byte: u8) -> bool {
    byte > 0x1f && byte < 0x80
}

impl FixStr {
    /// Converts a slice of bytes to a string slice.
    ///
    /// A FIX string slice ([`&FixStr`]) is made of bytes ([`u8`]), and a byte
    /// slice ([`&[u8]`][slice]) is made of bytes, so this function
    /// converts between the two. Not all byte slices are valid string slices,
    /// however: [`&FixStr`] requires that it is valid ASCII without controll
    /// characters.
    /// `from_ascii()` checks to ensure that the bytes are valid, and then does
    /// the conversion.
    ///
    /// [`&FixStr`]: FixStr
    ///
    /// If you are sure that the byte slice is valid ASCII without controll
    /// characters, and you don't want to incur the overhead of the validity
    /// check, there is an unsafe version of this function,
    /// [`from_ascii_unchecked`], which has the same behavior but skips
    /// the check.
    ///
    /// [`from_ascii_unchecked`]: FixStr::from_ascii_unchecked
    ///
    /// If you need a `FixString` instead of a `&FixStr`, consider
    /// [`FixString::from_ascii`].
    ///
    /// Because you can stack-allocate a `[u8; N]`, and you can take a
    /// [`&[u8]`][slice] of it, this function is one way to have a
    /// stack-allocated string.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the slice is not ASCII.
    pub const fn from_ascii(buf: &[u8]) -> Result<&FixStr, FixStringError> {
        let mut i = 0;
        while i < buf.len() {
            let c = buf[i];
            if !is_non_control_ascii_char(c) {
                return Err(FixStringError { idx: i, value: c });
            }
            i += 1;
        }
        // SAFETY: `buf` validity checked just above.
        unsafe { Ok(FixStr::from_ascii_unchecked(buf)) }
    }

    /// Converts a slice of bytes to a FIX string slice without checking
    /// that it contains only ASCII characters.
    ///
    /// See the safe version, [`from_ascii`], for more information.
    ///
    /// [`from_ascii`]: FixStr::from_ascii
    ///
    /// # Safety
    ///
    /// The bytes passed in must consists from ASCII characters only.
    pub const unsafe fn from_ascii_unchecked(buf: &[u8]) -> &FixStr {
        // SAFETY: the caller must guarantee that the bytes `buf` are valid ASCII.
        // Also relies on `&FixStr` and `&[u8]` having the same layout.
        mem::transmute(buf)
    }

    pub const fn as_utf8(&self) -> &str {
        // SAFETY: ASCII is always valid UTF-8
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub const fn len(&self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
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

impl From<&FixStr> for String {
    fn from(input: &FixStr) -> String {
        input.to_owned().into()
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

macro_rules! impl_eq {
    ($lhs:ty, $lhs_bytes: ident, $rhs: ty, $rhs_bytes: ident) => {
        impl PartialEq<$rhs> for $lhs {
            #[inline]
            fn eq(&self, other: &$rhs) -> bool {
                PartialEq::eq(self.$lhs_bytes(), other.$rhs_bytes())
            }
        }

        impl PartialEq<$lhs> for $rhs {
            #[inline]
            fn eq(&self, other: &$lhs) -> bool {
                PartialEq::eq(self.$rhs_bytes(), other.$lhs_bytes())
            }
        }
    };
}

impl_eq!([u8], as_ref, FixStr, as_bytes);
impl_eq!([u8], as_ref, &FixStr, as_bytes);
impl_eq!(&[u8], as_ref, FixStr, as_bytes);
impl_eq!(Vec<u8>, as_slice, FixStr, as_bytes);
impl_eq!(Vec<u8>, as_slice, &FixStr, as_bytes);
impl_eq!(str, as_bytes, FixStr, as_bytes);
impl_eq!(&str, as_bytes, FixStr, as_bytes);
impl_eq!(str, as_bytes, &FixStr, as_bytes);
impl_eq!(String, as_bytes, FixStr, as_bytes);
impl_eq!(String, as_bytes, &FixStr, as_bytes);

impl_eq!([u8], as_ref, FixString, as_bytes);
impl_eq!(&[u8], as_ref, FixString, as_bytes);
impl_eq!(Vec<u8>, as_slice, FixString, as_bytes);
impl_eq!(str, as_bytes, FixString, as_bytes);
impl_eq!(&str, as_bytes, FixString, as_bytes);
impl_eq!(String, as_bytes, FixString, as_bytes);

impl_eq!(FixString, as_bytes, FixStr, as_bytes);
impl_eq!(FixString, as_bytes, &FixStr, as_bytes);

impl<const N: usize> PartialEq<[u8; N]> for FixStr {
    fn eq(&self, other: &[u8; N]) -> bool {
        self.0.eq(&other[..])
    }
}

impl<const N: usize> PartialEq<&'_ [u8; N]> for FixStr {
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.0.eq(*other)
    }
}

impl<const N: usize> PartialEq<[u8; N]> for &FixStr {
    fn eq(&self, other: &[u8; N]) -> bool {
        self.0.eq(&other[..])
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

/// Creates a `FixString` using interpolation of runtime expressions, replacing
/// invalid characters by `?`.
///
/// See [the formatting syntax documentation in `std::fmt`] for details.
#[macro_export]
macro_rules! fix_format {
    ($($arg:tt)*) => {{
        FixString::from_ascii_lossy(std::format!($($arg)*).into_bytes())
    }}
}

// TODO: Optional feature for ISO 8859-1 encoded strings
impl FixString {
    pub const fn new() -> FixString {
        FixString(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> FixString {
        FixString(Vec::with_capacity(capacity))
    }

    /// Converts a vector of bytes to a `FixString`.
    ///
    /// A FIX string ([`FixString`]) is made of bytes ([`u8`]),
    /// and a vector of bytes ([`Vec<u8>`]) is made of bytes, so this function
    /// converts between the two. Not all byte slices are valid `FixString`s,
    /// however: `FixString` requires that it is valid ASCII.
    /// `from_ascii()` checks to ensure that the bytes are valid ASCII,
    /// and then does the conversion.
    ///
    /// If you are sure that the byte slice is valid ASCII, and you don't want
    /// to incur the overhead of the validity check, there is an unsafe version
    /// of this function, [`from_ascii_unchecked`], which has the same behavior
    /// but skips the check.
    ///
    /// This method will take care to not copy the vector, for efficiency's
    /// sake.
    ///
    /// If you need a [`&FixStr`] instead of a `FixString`, consider
    /// [`FixStr::from_ascii`].
    ///
    /// The inverse of this method is [`into_bytes`].
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if the slice is not ASCII with a description as to why
    /// the provided bytes are not ASCII.
    ///
    /// [`from_ascii_unchecked`]: FixString::from_ascii_unchecked
    /// [`Vec<u8>`]: std::vec::Vec "Vec"
    /// [`&FixStr`]: FixStr
    /// [`into_bytes`]: FixString::into_bytes
    pub fn from_ascii(buf: Vec<u8>) -> Result<FixString, FixStringError> {
        for i in 0..buf.len() {
            // SAFETY: `i` never exceeds buf.len()
            let c = unsafe { *buf.get_unchecked(i) };
            if !is_non_control_ascii_char(c) {
                return Err(FixStringError { idx: i, value: c });
            }
        }
        Ok(FixString(buf))
    }

    /// Converts a vector of bytes to a `FixString` without checking that the
    /// it contains only ASCII characters.
    ///
    /// See the safe version, [`from_ascii`], for more details.
    ///
    /// [`from_ascii`]: FixString::from_ascii
    ///
    /// # Safety
    ///
    /// This function is unsafe because it does not check that the bytes passed
    /// to it are valid ASCII. If this constraint is violated, it may cause
    /// memory unsafety issues with future users of the `FixString`,
    /// as the rest of the library assumes that `FixString`s are valid ASCII.
    pub unsafe fn from_ascii_unchecked(buf: Vec<u8>) -> FixString {
        FixString(buf)
    }

    /// Converts a slice of bytes to a `FixString`, replacing invalid
    /// characters by `?`.
    pub fn from_ascii_lossy(mut buf: Vec<u8>) -> FixString {
        for i in 0..buf.len() {
            // SAFETY: `i` never exceeds buf.len()
            let c = unsafe { buf.get_unchecked_mut(i) };
            if !is_non_control_ascii_char(*c) {
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

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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

impl From<FixString> for String {
    fn from(input: FixString) -> String {
        // SAFETY: FixString consists of ASCII characters only thus it's valid UTF-8
        unsafe { String::from_utf8_unchecked(input.0) }
    }
}

impl TryFrom<&[u8]> for FixString {
    type Error = FixStringError;

    fn try_from(input: &[u8]) -> Result<FixString, Self::Error> {
        // TODO: check vefore allocation
        FixString::from_ascii(input.to_vec())
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

struct FixStringVisitor;

impl Visitor<'_> for FixStringVisitor {
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
        serializer.serialize_str(self.as_utf8())
    }
}

pub trait ToFixString {
    fn to_fix_string(&self) -> FixString;
}

impl ToFixString for FixStr {
    fn to_fix_string(&self) -> FixString {
        // SAFETY: FixStr is already checked against invalid characters
        unsafe { FixString::from_ascii_unchecked(self.as_bytes().to_owned()) }
    }
}

macro_rules! impl_to_fix_string_for_integer {
    ($t:ty) => {
        impl ToFixString for $t {
            fn to_fix_string(&self) -> FixString {
                // SAFETY: integers are always formatted using ASCII characters
                unsafe {
                    FixString::from_ascii_unchecked(
                        itoa::Buffer::new().format(*self).as_bytes().to_vec(),
                    )
                }
            }
        }
    };
}

impl_to_fix_string_for_integer!(i8);
impl_to_fix_string_for_integer!(i16);
impl_to_fix_string_for_integer!(i32);
impl_to_fix_string_for_integer!(i64);
impl_to_fix_string_for_integer!(isize);
impl_to_fix_string_for_integer!(u8);
impl_to_fix_string_for_integer!(u16);
impl_to_fix_string_for_integer!(u32);
impl_to_fix_string_for_integer!(u64);
impl_to_fix_string_for_integer!(usize);

fn deserialize_fraction_of_second<E>(buf: &[u8]) -> Result<(u32, u8), E>
where
    E: de::Error,
{
    // match buf {
    //     // Do nothing here, fraction of second will be deserializede below
    //     [b'.', ..] => buf = &buf[1..],
    //     _ => {
    //         return Err(de::Error::custom("incorrecct data format for UtcTimestamp"));
    //     }
    // }

    let [b'.', buf @ ..] = buf else {
        return Err(de::Error::custom("incorrecct data format for UtcTimestamp"));
    };

    let mut fraction_of_second: u64 = 0;
    for i in 0..buf.len() {
        // SAFETY: i is between 0 and buf.len()
        match unsafe { buf.get_unchecked(i) } {
            n @ b'0'..=b'9' => {
                fraction_of_second = fraction_of_second
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((n - b'0') as u64))
                    .ok_or_else(|| de::Error::custom("incorrect fraction of second (overflow)"))?;
            }
            _ => {
                return Err(de::Error::custom(
                    "incorrecct data format for fraction of second",
                ));
            }
        }
    }
    let (multiplier, divider) = match buf.len() {
        3 => (1_000_000, 1),
        6 => (1_000, 1),
        9 => (1, 1),
        // XXX: Types from `chrono` crate can't hold
        //      time at picosecond resolution
        12 => (1, 1_000),
        _ => {
            return Err(de::Error::custom(
                "incorrect fraction of second (wrong precision)",
            ));
        }
    };
    (fraction_of_second * multiplier / divider)
        .try_into()
        .map(|adjusted_fraction_of_second| (adjusted_fraction_of_second, buf.len() as u8))
        .map_err(|_| de::Error::custom("incorrecct data format for UtcTimestamp"))
}

struct UtcTimestampVisitor;

impl Visitor<'_> for UtcTimestampVisitor {
    type Value = UtcTimestamp;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("string")
    }

    /// TODO: Same as in Deserializer
    /// Deserialize string representing time/date combination represented
    /// in UTC (Universal Time Coordinated) in either YYYYMMDD-HH:MM:SS
    /// (whole seconds) or YYYYMMDD-HH:MM:SS.sss* format, colons, dash,
    /// and period required.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31,
    /// - HH = 00-23,
    /// - MM = 00-59,
    /// - SS = 00-60 (60 only if UTC leap second),
    /// - sss* fractions of seconds. The fractions of seconds may be empty when
    ///   no fractions of seconds are conveyed (in such a case the period
    ///   is not conveyed), it may include 3 digits to convey
    ///   milliseconds, 6 digits to convey microseconds, 9 digits
    ///   to convey nanoseconds, 12 digits to convey picoseconds;
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value.as_bytes() {
            [
                // Year
                y3 @ b'0'..=b'9', y2 @ b'0'..=b'9', y1 @ b'0'..=b'9', y0 @ b'0'..=b'9',
                // Month
                m1 @ b'0'..=b'1', m0 @ b'0'..=b'9',
                // Day
                d1 @ b'0'..=b'3', d0 @ b'0'..=b'9',
                b'-',
                // Hour
                h1 @ b'0'..=b'2', h0 @ b'0'..=b'9',
                b':',
                // Minute
                mm1 @ b'0'..=b'5', mm0 @ b'0'..=b'9',
                b':',
                // TODO: leap second!
                // Second
                s1 @ b'0'..=b'5', s0 @ b'0'..=b'9',
                ..
            ] => {
                let value = &value[17..];
                let year = (y3 - b'0') as i32 * 1000
                    + (y2 - b'0') as i32 * 100
                    + (y1 - b'0') as i32 * 10
                    + (y0 - b'0') as i32;
                let month = (m1 - b'0') as u32 * 10 + (m0 - b'0') as u32;
                let day = (d1 - b'0') as u32 * 10 + (d0 - b'0') as u32;
                let naive_date = NaiveDate::from_ymd_opt(year, month, day)
                    .ok_or_else(|| de::Error::custom("incorrecct data format for UtcTimestamp"))?;
                let hour = (h1 - b'0') as u32 * 10 + (h0 - b'0') as u32;
                let min = (mm1 - b'0') as u32 * 10 + (mm0 - b'0') as u32;
                let sec = (s1 - b'0') as u32 * 10 + (s0 - b'0') as u32;
                let (fraction_of_second, precision) = deserialize_fraction_of_second(value.as_bytes())?;
                let naive_date_time = naive_date
                    .and_hms_nano_opt(hour, min, sec, fraction_of_second)
                    .ok_or_else(|| de::Error::custom("incorrecct data format for UtcTimestamp"))?;
                let timestamp = Utc.from_utc_datetime(&naive_date_time);

                match precision {
                    0 => Ok(UtcTimestamp::with_secs(timestamp)),
                    3 => Ok(UtcTimestamp::with_millis(timestamp)),
                    6 => Ok(UtcTimestamp::with_micros(timestamp)),
                    9 => Ok(UtcTimestamp::with_nanos(timestamp)),
                    // XXX: Types from `chrono` crate can't hold
                    //      time at picosecond resolution
                    12 => Ok(UtcTimestamp::with_nanos(timestamp)),
                    _ => Err(de::Error::custom("incorrecct data format for UtcTimestamp")),
                }
            }
            _ => Err(de::Error::custom("incorrecct data format for UtcTimestamp")),
        }
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<UtcTimestamp, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(UtcTimestampVisitor)
    }
}

impl Serialize for UtcTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let formatted_timestamp = self.format_precisely().to_string();
        serializer.serialize_str(&formatted_timestamp)
    }
}

impl PartialEq for UtcTimestamp {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl Eq for UtcTimestamp {}

impl PartialOrd for UtcTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.timestamp.cmp(&other.timestamp))
    }
}

impl Ord for UtcTimestamp {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timestamp().cmp(&other.timestamp())
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let result = self.format_precisely().to_string();
        write!(f, "{}", result)
    }
}

impl UtcTimestamp {
    pub const MAX_UTC: UtcTimestamp = UtcTimestamp {
        timestamp: DateTime::<Utc>::MAX_UTC,
        precision: TimePrecision::Nanos,
    };
    pub const MIN_UTC: UtcTimestamp = UtcTimestamp {
        timestamp: DateTime::<Utc>::MIN_UTC,
        precision: TimePrecision::Nanos,
    };

    /// Creates UtcTimestamp that represents current date and time with default precision
    pub fn now() -> UtcTimestamp {
        UtcTimestamp::with_precision(Utc::now(), TimePrecision::default())
    }

    /// Creates UtcTimestamp with given time precision
    /// input's precision is adjusted to requested one
    pub fn with_precision(date_time: DateTime<Utc>, precision: TimePrecision) -> UtcTimestamp {
        match precision {
            TimePrecision::Secs => UtcTimestamp::with_secs(date_time),
            TimePrecision::Millis => UtcTimestamp::with_millis(date_time),
            TimePrecision::Micros => UtcTimestamp::with_micros(date_time),
            TimePrecision::Nanos => UtcTimestamp::with_nanos(date_time),
        }
    }

    fn timestamp_from_secs_and_nsecs(secs: i64, nsecs: u32) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, nsecs).unwrap()
    }

    /// Creates UtcTimestamp with time precision set to full seconds
    /// input's precision is adjusted to requested one
    pub fn with_secs(date_time: DateTime<Utc>) -> UtcTimestamp {
        let secs = date_time.timestamp();
        UtcTimestamp {
            timestamp: Self::timestamp_from_secs_and_nsecs(secs, 0),
            precision: TimePrecision::Secs,
        }
    }

    pub fn now_with_secs() -> UtcTimestamp {
        UtcTimestamp::with_secs(Utc::now())
    }

    /// Creates UtcTimestamp with time precision set to milliseconds
    /// input's precision is adjusted to requested one
    pub fn with_millis(date_time: DateTime<Utc>) -> UtcTimestamp {
        let secs = date_time.timestamp();
        let nsecs = date_time.timestamp_subsec_millis() * 1_000_000;
        UtcTimestamp {
            timestamp: Self::timestamp_from_secs_and_nsecs(secs, nsecs),
            precision: TimePrecision::Millis,
        }
    }

    /// Creates UtcTimestamp with time precision set to microseconds
    /// input's precision is adjusted to requested one
    pub fn with_micros(date_time: DateTime<Utc>) -> UtcTimestamp {
        let secs = date_time.timestamp();
        let nsecs = date_time.timestamp_subsec_micros() * 1_000;
        UtcTimestamp {
            timestamp: Self::timestamp_from_secs_and_nsecs(secs, nsecs),
            precision: TimePrecision::Micros,
        }
    }

    /// Creates UtcTimestamp with time precision set to nanoseconds
    /// input's precision is adjusted to requested one
    pub fn with_nanos(date_time: DateTime<Utc>) -> UtcTimestamp {
        let secs = date_time.timestamp();
        let nsecs = date_time.timestamp_subsec_nanos();
        UtcTimestamp {
            timestamp: Self::timestamp_from_secs_and_nsecs(secs, nsecs),
            precision: TimePrecision::Nanos,
        }
    }

    /// Formats timestamp with precision set inside the struct
    pub fn format_precisely(&self) -> DelayedFormat<StrftimeItems> {
        match self.precision {
            TimePrecision::Secs => self.format("%Y%m%d-%H:%M:%S"),
            TimePrecision::Millis => self.format("%Y%m%d-%H:%M:%S%.3f"),
            TimePrecision::Micros => self.format("%Y%m%d-%H:%M:%S%.6f"),
            TimePrecision::Nanos => self.format("%Y%m%d-%H:%M:%S%.9f"),
        }
    }

    pub fn format<'a>(&self, fmt: &'a str) -> DelayedFormat<StrftimeItems<'a>> {
        self.timestamp.format(fmt)
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    pub fn precision(&self) -> TimePrecision {
        self.precision
    }
}

impl UtcTimeOnly {
    /// Creates UtcTimeOnly with time precision set to full seconds
    /// input's precision is adjusted to requested one
    pub fn with_secs(time: NaiveTime) -> UtcTimeOnly {
        UtcTimeOnly {
            timestamp: time.with_nanosecond(0).unwrap(),
            precision: TimePrecision::Secs,
        }
    }

    /// Creates UtcTimeOnly with time precision set to full milliseconds
    /// input's precision is adjusted to requested one
    pub fn with_millis(time: NaiveTime) -> UtcTimeOnly {
        UtcTimeOnly {
            timestamp: time.with_nanosecond(time.nanosecond() / 1_000_000).unwrap(),
            precision: TimePrecision::Millis,
        }
    }

    /// Creates UtcTimeOnly with time precision set to full microseconds
    /// input's precision is adjusted to requested one
    pub fn with_micros(time: NaiveTime) -> UtcTimeOnly {
        UtcTimeOnly {
            timestamp: time.with_nanosecond(time.nanosecond() / 1_000).unwrap(),
            precision: TimePrecision::Micros,
        }
    }

    /// Creates UtcTimeOnly with time precision set to full nanoseconds
    /// input's precision is adjusted to requested one
    pub fn with_nanos(time: NaiveTime) -> UtcTimeOnly {
        UtcTimeOnly {
            timestamp: time,
            precision: TimePrecision::Nanos,
        }
    }

    pub fn format<'a>(&self, fmt: &'a str) -> DelayedFormat<StrftimeItems<'a>> {
        self.timestamp.format(fmt)
    }

    pub fn timestamp(&self) -> NaiveTime {
        self.timestamp
    }

    pub fn precision(&self) -> TimePrecision {
        self.precision
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

    #[test]
    fn utc_timestamp_default_precision_nanos() {
        let now = UtcTimestamp::now();
        assert_eq!(now.precision(), TimePrecision::Nanos);
    }
}
