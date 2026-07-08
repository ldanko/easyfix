use std::{borrow, fmt, mem, num::NonZero, ops};

#[cfg(feature = "serde-serialize")]
use chrono::Datelike;
use chrono::Timelike;
pub use chrono::{
    DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    format::{DelayedFormat, StrftimeItems},
};
pub use rust_decimal::Decimal;

use crate::{base_messages::SessionRejectReasonBase, fix_str, version::Version};
pub use crate::{country::Country, currency::Currency};

pub type Int = i64;
pub type NonZeroInt = NonZero<Int>;
pub type TagNum = u16;
pub type SeqNum = u32;
pub type NonZeroSeqNum = NonZero<SeqNum>;
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

/// Returns an empty `FixString`, equivalent to [`FixString::new`] - a
/// "not yet set" placeholder, not a valid field value.
impl Default for FixString {
    fn default() -> FixString {
        FixString::new()
    }
}

#[derive(Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct FixStr([u8]);

pub type MultipleStringValue = Vec<FixString>;

pub type Exchange = [u8; 4];
/// Month of a year, optionally narrowed to a day of the month (`YYYYMMDD`)
/// or a week within the month (`YYYYMMWW`, `WW` = `w1`..`w5`).
///
/// The format is not validated - the value is carried as a plain string,
/// and format conformance is left to the application. The `FixString`
/// invariant (printable ASCII) guarantees the value cannot corrupt
/// message framing.
pub type MonthYear = FixString;
pub type Language = [u8; 2];

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde-serialize", derive(serde::Serialize))]
#[cfg_attr(feature = "serde-deserialize", derive(serde::Deserialize))]
pub enum TimePrecision {
    Secs = 0,
    Millis = 3,
    Micros = 6,
    #[default]
    Nanos = 9,
}

#[derive(Clone, Copy, Debug)]
pub struct UtcTimestamp {
    timestamp: DateTime<Utc>,
    precision: TimePrecision,
}

/// Nanoseconds to keep when reducing `time` to whole-second precision.
///
/// A UTC leap second is not a fractional part: chrono represents `:60` as
/// `:59` carrying a nanosecond value of at least a whole second (see
/// [`Timelike::nanosecond`]). Clearing that field outright would silently
/// move `23:59:60` to `23:59:59` - a different instant, and a different
/// value on the wire, where `SS = 00-60` is valid (TagValue Encoding
/// section 6.2.2). So the leap offset survives the reduction; only the
/// sub-second fraction is dropped.
fn whole_second_nanos(time: &impl Timelike) -> u32 {
    const LEAP: u32 = 1_000_000_000;
    if time.nanosecond() >= LEAP { LEAP } else { 0 }
}

#[derive(Clone, Copy, Debug)]
pub struct UtcTimeOnly {
    timestamp: NaiveTime,
    precision: TimePrecision,
}
pub type UtcDateOnly = NaiveDate;

pub type LocalMktTime = NaiveTime;
pub type LocalMktDate = NaiveDate;

/// Date and time carrying a timezone offset, e.g. `20060901-07:39:00+05:30`.
///
/// The offset is required, in this type and on the wire. FIX defines the
/// datatype as "local time with an offset to UTC to allow identification of
/// local time and time zone offset of that time" (TagValue Encoding section
/// 6.2.2), so the offset is what the type exists to carry - and what makes
/// a value a point in time rather than a wall clock reading that cannot be
/// placed on a timeline. The grammar in that section brackets the offset,
/// but every example it gives carries one.
///
/// A local time whose zone does *not* follow from the value belongs in
/// [`LocalMktDate`] and [`LocalMktTime`] instead. Those name a time local to
/// a market center, with the market center identified by a separate field -
/// which is exactly the information an omitted offset would leave unstated.
///
/// The seconds are optional on input (the spec's own examples omit them) and
/// always present on output.
#[derive(Clone, Copy, Debug)]
pub struct TzTimestamp {
    timestamp: DateTime<FixedOffset>,
    precision: TimePrecision,
}

/// Time of day carrying a timezone offset, e.g. `07:39:00+05:30`.
///
/// The offset is required for the same reason as in [`TzTimestamp`]: without
/// it the value is a wall clock reading with no way to place it in time, and
/// FIX already covers that case with [`LocalMktTime`], where the market
/// center supplying the zone is named in a separate field.
#[derive(Clone, Copy, Debug)]
pub struct TzTimeOnly {
    timestamp: NaiveTime,
    offset: FixedOffset,
    precision: TimePrecision,
}

/// Wire type of every FIX `Length` field - including `BodyLength<9>`.
///
/// The `u16` width is a deliberate ceiling, not an incidental choice: it
/// caps a message body at 65535 octets, so a whole TagValue message tops
/// out around 65.5 KB once `8=`, `9=` and the `10=` trailer are counted.
/// Everything reading from a socket depends on it - see [`raw_message`],
/// which turns an out-of-range `BodyLength<9>` into
/// [`RawMessageError::Garbled`] before any body is buffered. Widening this
/// type would surrender that bound.
///
/// [`raw_message`]: crate::deserializer::raw_message
/// [`RawMessageError::Garbled`]: crate::deserializer::RawMessageError::Garbled
pub type Length = u16;
pub type NonZeroLength = NonZero<Length>;
pub type Data = Vec<u8>;
pub type XmlData = Data;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TenorUnit {
    Days,
    Months,
    Weeks,
    Years,
}

impl TenorUnit {
    /// The FIX wire code of this unit.
    pub const fn as_byte(self) -> u8 {
        match self {
            TenorUnit::Days => b'D',
            TenorUnit::Months => b'M',
            TenorUnit::Weeks => b'W',
            TenorUnit::Years => b'Y',
        }
    }

    /// The unit denoted by a FIX wire code, or `None` when the code is not
    /// one of the four defined units.
    pub const fn from_byte(byte: u8) -> Option<TenorUnit> {
        match byte {
            b'D' => Some(TenorUnit::Days),
            b'M' => Some(TenorUnit::Months),
            b'W' => Some(TenorUnit::Weeks),
            b'Y' => Some(TenorUnit::Years),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tenor {
    pub unit: TenorUnit,
    pub value: Length,
}

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
    byte > 0x1f && byte < 0x7f
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
        unsafe { mem::transmute(buf) }
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
        // SAFETY: `self` is a valid `FixStr`, so its bytes are already
        // validated as non-control ASCII.
        unsafe { FixString::from_ascii_unchecked(self.as_bytes().to_owned()) }
    }

    fn clone_into(&self, target: &mut FixString) {
        let mut buf = mem::take(target).into_bytes();
        self.as_bytes().clone_into(&mut buf);
        // SAFETY: `buf` holds bytes cloned from `self`, a valid `FixStr`.
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
        $crate::basic_types::FixString::from_ascii_lossy(std::format!($($arg)*).into_bytes())
    }}
}

impl FixString {
    /// Creates an empty `FixString` - a "not yet set" placeholder, not a
    /// valid field value.
    ///
    /// Every FIX field must carry at least one byte, so an empty value
    /// never reaches the wire: serializing it fails with
    /// [`SerializeError::EmptyValue`]. It exists so structs with
    /// `FixString` fields can implement `Default` (`..Default::default()`
    /// construction, session headers filled in at transmit time) and to
    /// give generic code a `const` starting value. To express a genuinely
    /// absent value, use `Option<FixString>` instead.
    ///
    /// [`SerializeError::EmptyValue`]: crate::serializer::SerializeError::EmptyValue
    pub const fn new() -> FixString {
        FixString(Vec::new())
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
        // SAFETY: `FixString` holds bytes validated at construction, the
        // same invariant `FixStr` requires.
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
        // Validate in place, so invalid input costs no allocation.
        FixStr::from_ascii(input).map(|fix_str| fix_str.to_owned())
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
        // Validate in place, so invalid input costs no allocation.
        FixStr::from_ascii(buf.as_bytes()).map(|fix_str| fix_str.to_owned())
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
        // Validate in place, so invalid input costs no allocation.
        FixStr::from_ascii(&buf).map(|fix_str| fix_str.to_owned())
    }
}

impl<const N: usize> TryFrom<&[u8; N]> for FixString {
    type Error = FixStringError;

    fn try_from(input: &[u8; N]) -> Result<FixString, Self::Error> {
        // Validate in place, so invalid input costs no allocation.
        FixStr::from_ascii(input).map(|fix_str| fix_str.to_owned())
    }
}

#[cfg(feature = "serde-deserialize")]
mod fix_string_serde_de {
    use serde::{
        Deserializer,
        de::{self, Visitor},
    };

    use super::*;

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

    impl<'de> serde::Deserialize<'de> for FixString {
        fn deserialize<D>(deserializer: D) -> Result<FixString, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(FixStringVisitor)
        }
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for FixString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
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

#[cfg(feature = "serde-deserialize")]
mod utc_timestamp_serde_de {
    use serde::{
        Deserializer,
        de::{self, Visitor},
    };

    use super::*;
    use crate::deserializer::parse_utc_timestamp;

    struct UtcTimestampVisitor;

    impl Visitor<'_> for UtcTimestampVisitor {
        type Value = UtcTimestamp;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string")
        }

        /// Deserialize a UTC timestamp in the FIX wire format. The grammar
        /// is defined by the shared parser also used by the tag-value
        /// deserializer; unlike the tag-value form the value here is
        /// length-delimited, so the whole input must be consumed.
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            match parse_utc_timestamp(value.as_bytes()) {
                // The whole input must be consumed - a length-delimited
                // value has no terminator after the timestamp.
                Ok((timestamp, [])) => Ok(timestamp),
                _ => Err(de::Error::custom("incorrect data format for UtcTimestamp")),
            }
        }
    }

    impl<'de> serde::Deserialize<'de> for UtcTimestamp {
        fn deserialize<D>(deserializer: D) -> Result<UtcTimestamp, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(UtcTimestampVisitor)
        }
    }
}

#[cfg(feature = "serde-deserialize")]
mod tenor_serde_de {
    use serde::{
        Deserializer,
        de::{self, Visitor},
    };

    use super::*;
    use crate::deserializer::parse_tenor;

    struct TenorVisitor;

    impl Visitor<'_> for TenorVisitor {
        type Value = Tenor;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string")
        }

        /// Deserialize a tenor in the FIX wire format. The grammar is defined
        /// by the shared parser also used by the tag-value deserializer;
        /// unlike the tag-value form the value here is length-delimited, so
        /// the whole input must be consumed.
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            match parse_tenor(value.as_bytes()) {
                // The whole input must be consumed - a length-delimited
                // value has no terminator after the digits.
                Ok((tenor, [])) => Ok(tenor),
                _ => Err(de::Error::custom("incorrect data format for Tenor")),
            }
        }
    }

    impl<'de> serde::Deserialize<'de> for Tenor {
        fn deserialize<D>(deserializer: D) -> Result<Tenor, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(TenorVisitor)
        }
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for Tenor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&format_args!(
            "{}{}",
            char::from(self.unit.as_byte()),
            self.value
        ))
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for UtcTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error;

        // The FIX grammar has a fixed 4-digit year; chrono formats years
        // outside that range with a sign and more digits, producing a string
        // that can never be deserialized back - fail fast instead.
        let year = self.timestamp.year();
        if !(0..=9999).contains(&year) {
            return Err(S::Error::custom(format!(
                "year {year} not representable in the 4-digit FIX timestamp format"
            )));
        }
        let formatted_timestamp = self.format_precisely().to_string();
        serializer.serialize_str(&formatted_timestamp)
    }
}

#[cfg(feature = "serde-deserialize")]
mod utc_time_only_serde_de {
    use serde::{
        Deserializer,
        de::{self, Visitor},
    };

    use super::*;
    use crate::deserializer::parse_utc_time_only;

    struct UtcTimeOnlyVisitor;

    impl Visitor<'_> for UtcTimeOnlyVisitor {
        type Value = UtcTimeOnly;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string")
        }

        /// Deserialize a UTC time-only value in the FIX wire format. The
        /// grammar is defined by the shared parser also used by the tag-value
        /// deserializer; unlike the tag-value form the value here is
        /// length-delimited, so the whole input must be consumed.
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            match parse_utc_time_only(value.as_bytes()) {
                Ok((time, [])) => Ok(time),
                _ => Err(de::Error::custom("incorrect data format for UtcTimeOnly")),
            }
        }
    }

    impl<'de> serde::Deserialize<'de> for UtcTimeOnly {
        fn deserialize<D>(deserializer: D) -> Result<UtcTimeOnly, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(UtcTimeOnlyVisitor)
        }
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for UtcTimeOnly {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde-deserialize")]
mod tz_timestamp_serde_de {
    use serde::{
        Deserializer,
        de::{self, Visitor},
    };

    use super::*;
    use crate::deserializer::parse_tz_timestamp;

    struct TzTimestampVisitor;

    impl Visitor<'_> for TzTimestampVisitor {
        type Value = TzTimestamp;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string")
        }

        /// Deserialize a timestamp with a timezone offset in the FIX wire
        /// format. The grammar is defined by the shared parser also used by
        /// the tag-value deserializer; unlike the tag-value form the value
        /// here is length-delimited, so the whole input must be consumed.
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            match parse_tz_timestamp(value.as_bytes()) {
                Ok((timestamp, [])) => Ok(timestamp),
                _ => Err(de::Error::custom("incorrect data format for TzTimestamp")),
            }
        }
    }

    impl<'de> serde::Deserialize<'de> for TzTimestamp {
        fn deserialize<D>(deserializer: D) -> Result<TzTimestamp, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(TzTimestampVisitor)
        }
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for TzTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error;

        let year = self.timestamp.year();
        if !year_is_wire_representable(year) {
            return Err(S::Error::custom(format!(
                "year {year} not representable in the 4-digit FIX timestamp format"
            )));
        }
        if !offset_is_wire_representable(*self.timestamp.offset()) {
            return Err(S::Error::custom(
                "timezone offset with a sub-minute part is not representable in the FIX format",
            ));
        }
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde-deserialize")]
mod tz_time_only_serde_de {
    use serde::{
        Deserializer,
        de::{self, Visitor},
    };

    use super::*;
    use crate::deserializer::parse_tz_time_only;

    struct TzTimeOnlyVisitor;

    impl Visitor<'_> for TzTimeOnlyVisitor {
        type Value = TzTimeOnly;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string")
        }

        /// Deserialize a time of day with a timezone offset in the FIX wire
        /// format. The grammar is defined by the shared parser also used by
        /// the tag-value deserializer; unlike the tag-value form the value
        /// here is length-delimited, so the whole input must be consumed.
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            match parse_tz_time_only(value.as_bytes()) {
                Ok((time, [])) => Ok(time),
                _ => Err(de::Error::custom("incorrect data format for TzTimeOnly")),
            }
        }
    }

    impl<'de> serde::Deserialize<'de> for TzTimeOnly {
        fn deserialize<D>(deserializer: D) -> Result<TzTimeOnly, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(TzTimeOnlyVisitor)
        }
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for TzTimeOnly {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error;

        if !offset_is_wire_representable(self.offset) {
            return Err(S::Error::custom(
                "timezone offset with a sub-minute part is not representable in the FIX format",
            ));
        }
        serializer.collect_str(self)
    }
}

impl PartialEq for UtcTimestamp {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl Eq for UtcTimestamp {}

#[expect(clippy::non_canonical_partial_ord_impl)]
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
}

impl Default for UtcTimestamp {
    fn default() -> Self {
        UtcTimestamp::MIN_UTC
    }
}

impl UtcTimestamp {
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
            timestamp: Self::timestamp_from_secs_and_nsecs(secs, whole_second_nanos(&date_time)),
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
    pub fn format_precisely(&self) -> DelayedFormat<StrftimeItems<'_>> {
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
    /// Creates UtcTimeOnly with given time precision
    /// input's precision is adjusted to requested one
    pub fn with_precision(time: NaiveTime, precision: TimePrecision) -> UtcTimeOnly {
        match precision {
            TimePrecision::Secs => UtcTimeOnly::with_secs(time),
            TimePrecision::Millis => UtcTimeOnly::with_millis(time),
            TimePrecision::Micros => UtcTimeOnly::with_micros(time),
            TimePrecision::Nanos => UtcTimeOnly::with_nanos(time),
        }
    }

    /// Creates UtcTimeOnly with time precision set to full seconds
    /// input's precision is adjusted to requested one
    pub fn with_secs(time: NaiveTime) -> UtcTimeOnly {
        UtcTimeOnly {
            timestamp: time.with_nanosecond(whole_second_nanos(&time)).unwrap(),
            precision: TimePrecision::Secs,
        }
    }

    /// Creates UtcTimeOnly with time precision set to full milliseconds
    /// input's precision is adjusted to requested one
    pub fn with_millis(time: NaiveTime) -> UtcTimeOnly {
        UtcTimeOnly {
            // Strip sub-millisecond precision from nanoseconds
            timestamp: time
                .with_nanosecond(time.nanosecond() - time.nanosecond() % 1_000_000)
                .unwrap(),
            precision: TimePrecision::Millis,
        }
    }

    /// Creates UtcTimeOnly with time precision set to full microseconds
    /// input's precision is adjusted to requested one
    pub fn with_micros(time: NaiveTime) -> UtcTimeOnly {
        UtcTimeOnly {
            // Strip sub-microsecond precision from nanoseconds
            timestamp: time
                .with_nanosecond(time.nanosecond() - time.nanosecond() % 1_000)
                .unwrap(),
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

    /// Formats the time with the precision set inside the struct.
    pub fn format_precisely(&self) -> DelayedFormat<StrftimeItems<'_>> {
        match self.precision {
            TimePrecision::Secs => self.format("%H:%M:%S"),
            TimePrecision::Millis => self.format("%H:%M:%S%.3f"),
            TimePrecision::Micros => self.format("%H:%M:%S%.6f"),
            TimePrecision::Nanos => self.format("%H:%M:%S%.9f"),
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

/// Renders the FIX wire form, honouring the precision carried by the value.
impl fmt::Display for UtcTimeOnly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_precisely())
    }
}

/// Whether a year can be rendered in the FIX wire form, which is exactly
/// four digits (TagValue Encoding section 6.2.2, `YYYY = 0000-9999`).
///
/// chrono renders anything outside that range with a sign and more digits,
/// producing a value no parser accepts - and one that still travels inside a
/// well-formed message, since BodyLength and CheckSum are computed over
/// whatever was written. Both the serializer and the serde impls reject such
/// a value instead.
pub(crate) fn year_is_wire_representable(year: i32) -> bool {
    (0..=9999).contains(&year)
}

/// Whether a UTC offset can be rendered in the FIX wire form, which carries
/// whole minutes only (`hh[:mm]`). A `FixedOffset` can hold seconds; those
/// would be silently dropped by the renderer.
pub(crate) fn offset_is_wire_representable(offset: FixedOffset) -> bool {
    offset.local_minus_utc() % 60 == 0
}

/// Write a UTC offset in the FIX wire form: `Z` for UTC, otherwise a signed
/// two-digit hour with `:mm` appended only when the offset has a non-zero
/// minute part.
fn write_tz_offset(f: &mut fmt::Formatter<'_>, offset: FixedOffset) -> fmt::Result {
    let total_secs = offset.local_minus_utc();
    if total_secs == 0 {
        return f.write_str("Z");
    }
    let sign = if total_secs < 0 { '-' } else { '+' };
    let abs_secs = total_secs.unsigned_abs();
    let hours = abs_secs / 3600;
    let minutes = (abs_secs % 3600) / 60;
    write!(f, "{sign}{hours:02}")?;
    if minutes != 0 {
        write!(f, ":{minutes:02}")?;
    }
    Ok(())
}

impl TzTimestamp {
    pub fn with_precision(
        timestamp: DateTime<FixedOffset>,
        precision: TimePrecision,
    ) -> TzTimestamp {
        TzTimestamp {
            timestamp,
            precision,
        }
    }

    pub fn with_secs(timestamp: DateTime<FixedOffset>) -> TzTimestamp {
        TzTimestamp {
            timestamp,
            precision: TimePrecision::Secs,
        }
    }

    pub fn with_millis(timestamp: DateTime<FixedOffset>) -> TzTimestamp {
        TzTimestamp {
            timestamp,
            precision: TimePrecision::Millis,
        }
    }

    pub fn with_micros(timestamp: DateTime<FixedOffset>) -> TzTimestamp {
        TzTimestamp {
            timestamp,
            precision: TimePrecision::Micros,
        }
    }

    pub fn with_nanos(timestamp: DateTime<FixedOffset>) -> TzTimestamp {
        TzTimestamp {
            timestamp,
            precision: TimePrecision::Nanos,
        }
    }

    /// Formats the timestamp with the precision set inside the struct,
    /// without the timezone offset.
    pub fn format_precisely(&self) -> DelayedFormat<StrftimeItems<'_>> {
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

    pub fn timestamp(&self) -> DateTime<FixedOffset> {
        self.timestamp
    }

    pub fn precision(&self) -> TimePrecision {
        self.precision
    }
}

/// Renders the FIX wire form, honouring the precision carried by the value.
impl fmt::Display for TzTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_precisely())?;
        write_tz_offset(f, *self.timestamp.offset())
    }
}

impl TzTimeOnly {
    pub fn new(timestamp: NaiveTime, offset: FixedOffset, precision: TimePrecision) -> TzTimeOnly {
        TzTimeOnly {
            timestamp,
            offset,
            precision,
        }
    }

    pub fn with_secs(timestamp: NaiveTime, offset: FixedOffset) -> TzTimeOnly {
        TzTimeOnly {
            timestamp: timestamp.with_nanosecond(0).unwrap(),
            offset,
            precision: TimePrecision::Secs,
        }
    }

    pub fn with_millis(timestamp: NaiveTime, offset: FixedOffset) -> TzTimeOnly {
        TzTimeOnly {
            timestamp: timestamp
                .with_nanosecond(timestamp.nanosecond() / 1_000_000 * 1_000_000)
                .unwrap(),
            offset,
            precision: TimePrecision::Millis,
        }
    }

    pub fn with_micros(timestamp: NaiveTime, offset: FixedOffset) -> TzTimeOnly {
        TzTimeOnly {
            timestamp: timestamp
                .with_nanosecond(timestamp.nanosecond() / 1_000 * 1_000)
                .unwrap(),
            offset,
            precision: TimePrecision::Micros,
        }
    }

    pub fn with_nanos(timestamp: NaiveTime, offset: FixedOffset) -> TzTimeOnly {
        TzTimeOnly {
            timestamp,
            offset,
            precision: TimePrecision::Nanos,
        }
    }

    /// Formats the time with the precision set inside the struct, without
    /// the timezone offset.
    pub fn format_precisely(&self) -> DelayedFormat<StrftimeItems<'_>> {
        match self.precision {
            TimePrecision::Secs => self.format("%H:%M:%S"),
            TimePrecision::Millis => self.format("%H:%M:%S%.3f"),
            TimePrecision::Micros => self.format("%H:%M:%S%.6f"),
            TimePrecision::Nanos => self.format("%H:%M:%S%.9f"),
        }
    }

    pub fn format<'a>(&self, fmt: &'a str) -> DelayedFormat<StrftimeItems<'a>> {
        self.timestamp.format(fmt)
    }

    pub fn timestamp(&self) -> NaiveTime {
        self.timestamp
    }

    pub fn offset(&self) -> FixedOffset {
        self.offset
    }

    pub fn precision(&self) -> TimePrecision {
        self.precision
    }
}

/// Renders the FIX wire form, honouring the precision carried by the value.
impl fmt::Display for TzTimeOnly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_precisely())?;
        write_tz_offset(f, self.offset)
    }
}

// ---------------------------------------------------------------------------
// MsgType (tag 35) — compact 1-2 byte representation
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum MsgTypeError {
    #[error("Empty message type")]
    Empty,
    #[error("Invalid character in message type: {0}")]
    InvalidChar(u8),
    #[error("Message type too long: expected 1-2 bytes, got {0}")]
    TooLong(usize),
}

const fn is_valid_msg_type_char(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z')
}

/// Trait for types whose value can be used as a MsgType field value.
/// Implemented by `MsgTypeBase` (in core) and the generated
/// `MsgType` enum (in easyfix-messages).
pub trait MsgTypeValue {
    fn raw_value(&self) -> MsgTypeField;
}

/// Compact, `Copy` newtype wrapping a validated MsgType raw value.
///
/// Stores 1-2 ASCII alphanumeric bytes inline. Single-byte values use
/// `0` as sentinel in the second position.
///
/// `Borrow<[u8]>` returns only the live bytes (enables `HashMap<MsgTypeField, _>`
/// lookup by `&[u8]`). `Hash` is implemented manually to hash only the live
/// bytes, satisfying the `Borrow` contract.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MsgTypeField {
    buf: [u8; 2],
}

impl std::hash::Hash for MsgTypeField {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl<T: MsgTypeValue> From<T> for MsgTypeField {
    fn from(v: T) -> Self {
        v.raw_value()
    }
}

impl MsgTypeField {
    /// Construct from a 1-2 byte raw MsgType value. Used internally by
    /// `MsgTypeBase::raw_value()`.
    pub(crate) const fn from_raw(buf: [u8; 2]) -> Self {
        MsgTypeField { buf }
    }

    pub const fn from_bytes(bytes: &[u8]) -> Result<MsgTypeField, MsgTypeError> {
        match bytes {
            [] => Err(MsgTypeError::Empty),
            [b0] => {
                if is_valid_msg_type_char(*b0) {
                    Ok(MsgTypeField { buf: [*b0, 0] })
                } else {
                    Err(MsgTypeError::InvalidChar(*b0))
                }
            }
            [b0, b1] => {
                if !is_valid_msg_type_char(*b0) {
                    Err(MsgTypeError::InvalidChar(*b0))
                } else if !is_valid_msg_type_char(*b1) {
                    Err(MsgTypeError::InvalidChar(*b1))
                } else {
                    Ok(MsgTypeField { buf: [*b0, *b1] })
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
        unsafe { std::str::from_utf8_unchecked(self.as_bytes()) }
    }

    pub fn as_fix_str(&self) -> &FixStr {
        // SAFETY: MsgType bytes are ASCII alphanumeric (0x30-0x39, 0x41-0x5A,
        //         0x61-0x7A), all within the valid FixStr range (0x20-0x7E)
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}

impl fmt::Debug for MsgTypeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MsgTypeField(\"{}\")", self.as_str())
    }
}

impl fmt::Display for MsgTypeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl borrow::Borrow<[u8]> for MsgTypeField {
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl std::str::FromStr for MsgTypeField {
    type Err = MsgTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MsgTypeField::from_bytes(s.as_bytes())
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for MsgTypeField {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde-deserialize")]
impl<'de> serde::Deserialize<'de> for MsgTypeField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct MsgTypeFieldVisitor;

        impl<'de> Visitor<'de> for MsgTypeFieldVisitor {
            type Value = MsgTypeField;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string with 1-2 alphanumeric characters")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                MsgTypeField::from_bytes(value.as_bytes())
                    .map_err(|e| de::Error::custom(e.to_string()))
            }
        }

        deserializer.deserialize_str(MsgTypeFieldVisitor)
    }
}

// ---------------------------------------------------------------------------
// SessionStatus (tag 1409)
// ---------------------------------------------------------------------------

/// Trait for types whose value can be used as a SessionStatus field value.
/// Implemented by `SessionStatusBase` (in core) and the generated
/// `SessionStatus` enum (in easyfix-messages).
pub trait SessionStatusValue {
    fn raw_value(&self) -> Int;
}

/// Newtype wrapping a validated SessionStatus raw value.
/// Can only be constructed from types implementing `SessionStatusValue`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionStatusField(Int);

impl<T: SessionStatusValue> From<T> for SessionStatusField {
    fn from(v: T) -> Self {
        Self(v.raw_value())
    }
}

impl SessionStatusField {
    pub fn into_inner(self) -> Int {
        self.0
    }
}

// ---------------------------------------------------------------------------
// SessionRejectReason (tag 373)
// ---------------------------------------------------------------------------

/// Trait for types whose value can be used as a SessionRejectReason field value.
/// Implemented by `SessionRejectReasonBase` (in core) and the generated
/// `SessionRejectReason` enum (in easyfix-messages).
pub trait SessionRejectReasonValue {
    fn raw_value(&self) -> Int;
}

/// Newtype wrapping a validated SessionRejectReason raw value.
/// Can only be constructed from types implementing `SessionRejectReasonValue`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionRejectReasonField(Int);

impl<T: SessionRejectReasonValue> From<T> for SessionRejectReasonField {
    fn from(v: T) -> Self {
        Self(v.raw_value())
    }
}

impl SessionRejectReasonField {
    pub fn into_inner(self) -> Int {
        self.0
    }
}

// ---------------------------------------------------------------------------
// ApplVerId (tags 1128 / 1137)
// ---------------------------------------------------------------------------

/// Invalid ApplVerID / DefaultApplVerID value (outside ApplVerIDCodeSet).
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid ApplVerID value: {0}")]
pub struct InvalidApplVerId(pub FixString);

/// Application version identifier - the ApplVerIDCodeSet shared by
/// `ApplVerID(1128)` and `DefaultApplVerID(1137)`.
///
/// The codeset is closed by the standard (FIX Session Layer §11.2 - values
/// are assigned only at service-pack release; custom application versions
/// live in `CstmApplVerID(1129)` / `DefaultCstmApplVerID(1408)`, never
/// here). Deliberately no `Default` impl - an implicit application version
/// is how a silent `1137=0` (FIX 2.7) ends up on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ApplVerId {
    Fix27,
    Fix30,
    Fix40,
    Fix41,
    Fix42,
    Fix43,
    Fix44,
    Fix50,
    Fix50Sp1,
    Fix50Sp2,
    FixLatest,
}

impl ApplVerId {
    /// Spec-defined meaning of an *absent* `DefaultApplVerID(1137)`:
    /// "If DefaultApplVerID(1137) is not present, the default application
    /// level is assumed to be FIXLatest" (FIX Session Layer §10, row 1137;
    /// §5.2.2). Applies to that absent-1137 rule only - not a blanket
    /// default for non-FIXT profiles.
    pub const DEFAULT_IF_ABSENT: ApplVerId = ApplVerId::FixLatest;

    pub const fn as_fix_str(self) -> &'static FixStr {
        match self {
            ApplVerId::Fix27 => fix_str!("0"),
            ApplVerId::Fix30 => fix_str!("1"),
            ApplVerId::Fix40 => fix_str!("2"),
            ApplVerId::Fix41 => fix_str!("3"),
            ApplVerId::Fix42 => fix_str!("4"),
            ApplVerId::Fix43 => fix_str!("5"),
            ApplVerId::Fix44 => fix_str!("6"),
            ApplVerId::Fix50 => fix_str!("7"),
            ApplVerId::Fix50Sp1 => fix_str!("8"),
            ApplVerId::Fix50Sp2 => fix_str!("9"),
            ApplVerId::FixLatest => fix_str!("10"),
        }
    }

    pub const fn as_bytes(self) -> &'static [u8] {
        self.as_fix_str().as_bytes()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<ApplVerId> {
        match bytes {
            b"0" => Some(ApplVerId::Fix27),
            b"1" => Some(ApplVerId::Fix30),
            b"2" => Some(ApplVerId::Fix40),
            b"3" => Some(ApplVerId::Fix41),
            b"4" => Some(ApplVerId::Fix42),
            b"5" => Some(ApplVerId::Fix43),
            b"6" => Some(ApplVerId::Fix44),
            b"7" => Some(ApplVerId::Fix50),
            b"8" => Some(ApplVerId::Fix50Sp1),
            b"9" => Some(ApplVerId::Fix50Sp2),
            b"10" => Some(ApplVerId::FixLatest),
            _ => None,
        }
    }

    pub fn from_fix_str(value: &FixStr) -> Result<ApplVerId, InvalidApplVerId> {
        ApplVerId::from_bytes(value.as_bytes()).ok_or_else(|| InvalidApplVerId(value.to_owned()))
    }

    /// Base-version projection onto [`Version`]. The extension-pack axis
    /// is disregarded: `FixLatest` projects onto its frozen base,
    /// [`Version::FIX_LATEST`] - the function does not claim FIX Latest
    /// *is* that frozen version, only that it is its version-axis base.
    pub fn to_version(self) -> Version {
        match self {
            ApplVerId::Fix27 => Version::FIX27,
            ApplVerId::Fix30 => Version::FIX30,
            ApplVerId::Fix40 => Version::FIX40,
            ApplVerId::Fix41 => Version::FIX41,
            ApplVerId::Fix42 => Version::FIX42,
            ApplVerId::Fix43 => Version::FIX43,
            ApplVerId::Fix44 => Version::FIX44,
            ApplVerId::Fix50 => Version::FIX50,
            ApplVerId::Fix50Sp1 => Version::FIX50SP1,
            ApplVerId::Fix50Sp2 => Version::FIX50SP2,
            ApplVerId::FixLatest => Version::FIX_LATEST,
        }
    }
}

impl From<ApplVerId> for &'static [u8] {
    fn from(value: ApplVerId) -> &'static [u8] {
        value.as_bytes()
    }
}

impl TryFrom<&FixStr> for ApplVerId {
    type Error = SessionRejectReasonBase;

    fn try_from(value: &FixStr) -> Result<ApplVerId, SessionRejectReasonBase> {
        ApplVerId::from_bytes(value.as_bytes()).ok_or(SessionRejectReasonBase::ValueIsIncorrect)
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for ApplVerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_fix_str().as_utf8())
    }
}

#[cfg(feature = "serde-deserialize")]
impl<'de> serde::Deserialize<'de> for ApplVerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct ApplVerIdVisitor;

        impl<'de> Visitor<'de> for ApplVerIdVisitor {
            type Value = ApplVerId;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an ApplVerIDCodeSet value (\"0\"..\"10\")")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                ApplVerId::from_bytes(value.as_bytes())
                    .ok_or_else(|| de::Error::custom(format!("invalid ApplVerID value: {value}")))
            }
        }

        deserializer.deserialize_str(ApplVerIdVisitor)
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
    fn fix_string_fail_on_del_character() {
        // 0x7F (DEL) is a control character despite being above the 0x00-0x1F range
        let buf = b"Hello\x7fworld!".to_vec();
        assert!(FixString::from_ascii(buf).is_err());
    }

    #[test]
    fn fix_string_accept_tilde() {
        // 0x7E (~) is the highest valid printable ASCII character, just below DEL
        let buf = b"Hello~world!".to_vec();
        assert!(FixString::from_ascii(buf).is_ok());
    }

    /// Every safe conversion into `FixString` must enforce the printable-ASCII
    /// invariant. `as_utf8` relies on it via `from_utf8_unchecked`, so a
    /// conversion that lets arbitrary bytes through would make even printing
    /// a `FixString` undefined behavior.
    #[test]
    fn fix_string_conversions_reject_non_ascii() {
        assert!(FixString::try_from(&b"Hello\xffworld!"[..]).is_err());
        assert!(FixString::try_from(b"Hello\xffworld!".to_vec()).is_err());
        assert!(FixString::try_from(*b"Hello\xffworld!").is_err());
        assert!(FixString::try_from(b"Hello\xffworld!").is_err());
    }

    #[test]
    fn fix_string_conversions_reject_control_characters() {
        assert!(FixString::try_from(&b"Hello\x01world!"[..]).is_err());
        assert!(FixString::try_from(b"Hello\x01world!".to_vec()).is_err());
        assert!(FixString::try_from(*b"Hello\x01world!").is_err());
        assert!(FixString::try_from(b"Hello\x01world!").is_err());
    }

    #[test]
    fn fix_string_conversions_accept_printable_ascii() {
        let expected = b"Hello world!";
        assert_eq!(FixString::try_from(&expected[..]).unwrap(), expected);
        assert_eq!(FixString::try_from(expected.to_vec()).unwrap(), expected);
        assert_eq!(FixString::try_from(*expected).unwrap(), expected);
        assert_eq!(FixString::try_from(expected).unwrap(), expected);
        assert_eq!(FixString::try_from("Hello world!").unwrap(), expected);
        assert_eq!(
            FixString::try_from(String::from("Hello world!")).unwrap(),
            expected
        );
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

    #[test]
    fn appl_ver_id_wire_round_trip() {
        for (code, id) in [
            (&b"0"[..], ApplVerId::Fix27),
            (b"1", ApplVerId::Fix30),
            (b"2", ApplVerId::Fix40),
            (b"3", ApplVerId::Fix41),
            (b"4", ApplVerId::Fix42),
            (b"5", ApplVerId::Fix43),
            (b"6", ApplVerId::Fix44),
            (b"7", ApplVerId::Fix50),
            (b"8", ApplVerId::Fix50Sp1),
            (b"9", ApplVerId::Fix50Sp2),
            (b"10", ApplVerId::FixLatest),
        ] {
            assert_eq!(id.as_bytes(), code);
            assert_eq!(ApplVerId::from_bytes(code), Some(id));
            assert_eq!(ApplVerId::from_fix_str(id.as_fix_str()).unwrap(), id);
        }
    }

    #[test]
    fn appl_ver_id_rejects_out_of_codeset() {
        assert!(ApplVerId::from_bytes(b"11").is_none());
        assert!(ApplVerId::from_bytes(b"X").is_none());
        assert!(ApplVerId::from_bytes(b"").is_none());
        let err = ApplVerId::from_fix_str(fix_str!("11")).unwrap_err();
        assert_eq!(err.0.as_utf8(), "11");
        assert_eq!(
            ApplVerId::try_from(fix_str!("42")),
            Err(SessionRejectReasonBase::ValueIsIncorrect),
        );
    }

    #[test]
    fn appl_ver_id_version_bridge() {
        assert_eq!(ApplVerId::Fix50Sp2.to_version(), Version::FIX50SP2);
        assert_eq!(ApplVerId::Fix27.to_version(), Version::FIX27);
        // Base-version projection: Latest lands on its frozen base.
        assert_eq!(ApplVerId::FixLatest.to_version(), Version::FIX_LATEST);
        assert_eq!(ApplVerId::FixLatest.to_version(), Version::FIX50SP2);
    }
}

#[cfg(all(test, feature = "serde-deserialize"))]
mod utc_timestamp_serde_tests {
    use serde::{
        Deserialize,
        de::value::{Error as DeError, StrDeserializer},
    };

    use super::*;

    fn de(input: &str) -> Result<UtcTimestamp, DeError> {
        UtcTimestamp::deserialize(StrDeserializer::<DeError>::new(input))
    }

    #[test]
    fn whole_second_timestamp_round_trips() {
        // A whole-second timestamp serializes without a fraction; reading
        // that form back must succeed and preserve the precision.
        let original = UtcTimestamp::with_secs(Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap());
        let formatted = original.format_precisely().to_string();
        assert_eq!(formatted, "20240102-03:04:05");
        let parsed = de(&formatted).expect("whole-second timestamp rejected");
        assert_eq!(parsed, original);
        assert_eq!(parsed.precision(), TimePrecision::Secs);
    }

    #[test]
    fn fractional_timestamps_round_trip() {
        for (input, precision) in [
            ("20240102-03:04:05.123", TimePrecision::Millis),
            ("20240102-03:04:05.123456", TimePrecision::Micros),
            ("20240102-03:04:05.123456789", TimePrecision::Nanos),
        ] {
            let parsed = de(input).expect("valid timestamp rejected");
            assert_eq!(parsed.precision(), precision);
            assert_eq!(parsed.format_precisely().to_string(), input);
        }
    }

    #[test]
    fn leap_second_survives_whole_second_precision() {
        // chrono represents a leap second as sec=59 with nanos >= 1_000_000_000.
        // Reducing to whole seconds keeps that offset, so :60 stays :60 - it is
        // a valid wire value (TagValue Encoding section 6.2.2, SS = 00-60) and
        // collapsing it to :59 would name a different instant.
        let parsed = de("20231231-23:59:60").expect("leap second rejected");
        assert_eq!(parsed.precision(), TimePrecision::Secs);
        assert_eq!(parsed.format_precisely().to_string(), "20231231-23:59:60");
        assert_ne!(
            parsed,
            UtcTimestamp::with_secs(Utc.with_ymd_and_hms(2023, 12, 31, 23, 59, 59).unwrap())
        );
    }

    #[test]
    fn leap_second_with_millis_is_accepted() {
        let expected = Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(2023, 12, 31)
                .unwrap()
                .and_hms_nano_opt(23, 59, 59, 1_123_000_000)
                .unwrap(),
        );
        let parsed = de("20231231-23:59:60.123").expect("leap second with millis rejected");
        assert_eq!(parsed.timestamp(), expected);
        assert_eq!(parsed.precision(), TimePrecision::Millis);
    }

    #[test]
    fn trailing_garbage_is_rejected() {
        assert!(de("20240102-03:04:05x").is_err());
        assert!(de("20240102-03:04:05.123x").is_err());
        assert!(de("20240102-03:04:05.123\x01").is_err());
    }

    #[test]
    fn overlong_fraction_is_rejected() {
        // A digit count congruent to a valid count modulo 256 must not pass.
        let mut input = String::from("20240102-03:04:05.");
        input.push_str(&"0".repeat(256));
        input.push_str("123");
        assert!(de(&input).is_err());
    }

    #[test]
    fn invalid_values_are_rejected() {
        // Wrong fraction digit count (only 3, 6, 9 and 12 are valid).
        assert!(de("20240102-03:04:05.12").is_err());
        // Empty fraction after the period.
        assert!(de("20240102-03:04:05.").is_err());
        // Nonexistent calendar date.
        assert!(de("20240232-03:04:05").is_err());
        // Second above the leap-second value.
        assert!(de("20240102-03:04:61").is_err());
        // Truncated value.
        assert!(de("20240102-03:04").is_err());
        assert!(de("").is_err());
    }
}

#[cfg(all(test, feature = "serde-serialize"))]
mod utc_timestamp_serde_ser_tests {
    use super::*;

    #[test]
    fn out_of_range_year_fails_to_serialize() {
        // The FIX grammar has a fixed 4-digit year; chrono formats years
        // outside 0000-9999 with a sign and more digits, producing a string
        // the deserializer can never accept. Serialization must fail fast
        // instead of emitting unreadable data. UtcTimestamp::default() is
        // MIN_UTC, so an unfilled timestamp field hits exactly this case.
        assert!(serde_json::to_string(&UtcTimestamp::default()).is_err());
        assert!(serde_json::to_string(&UtcTimestamp::MAX_UTC).is_err());
    }

    #[test]
    fn in_range_timestamp_serializes_to_plain_string() {
        let ts = UtcTimestamp::with_secs(Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap());
        assert_eq!(serde_json::to_string(&ts).unwrap(), "\"20240102-03:04:05\"");
    }
}

#[cfg(all(test, feature = "serde-serialize", feature = "serde-deserialize"))]
mod time_serde_tests {
    use serde::{
        Deserialize,
        de::value::{Error as DeError, StrDeserializer},
    };

    use super::*;

    fn de<'de, T: Deserialize<'de>>(input: &'de str) -> Result<T, DeError> {
        T::deserialize(StrDeserializer::<DeError>::new(input))
    }

    /// Serialize, deserialize, serialize again, and check the string never
    /// changed. The wire form encodes every part of these values - offset
    /// and precision included - so a round-trip that silently widens `Secs`
    /// to `Nanos` or drops an offset shows up here as a different string.
    fn assert_round_trip<T>(value: T, expected: &str)
    where
        T: serde::Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, format!("\"{expected}\""));
        let parsed: T = de(expected).expect("valid value rejected");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
    }

    #[test]
    fn utc_time_only_round_trips_at_every_precision() {
        let time = NaiveTime::from_hms_nano_opt(3, 4, 5, 123_456_789).unwrap();
        assert_round_trip(UtcTimeOnly::with_secs(time), "03:04:05");
        assert_round_trip(UtcTimeOnly::with_millis(time), "03:04:05.123");
        assert_round_trip(UtcTimeOnly::with_micros(time), "03:04:05.123456");
        assert_round_trip(UtcTimeOnly::with_nanos(time), "03:04:05.123456789");
    }

    #[test]
    fn tz_timestamp_round_trips_every_offset_form() {
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
            .unwrap()
            .and_hms_opt(7, 39, 0)
            .unwrap();
        for (offset_secs, expected) in [
            (0, "20060901-07:39:00Z"),
            (3600, "20060901-07:39:00+01"),
            (-3600, "20060901-07:39:00-01"),
            (5400, "20060901-07:39:00+01:30"),
            (-19_800, "20060901-07:39:00-05:30"),
        ] {
            let offset = FixedOffset::east_opt(offset_secs).unwrap();
            let value = TzTimestamp::with_secs(offset.from_local_datetime(&naive).unwrap());
            assert_round_trip(value, expected);
        }
    }

    #[test]
    fn tz_timestamp_round_trips_at_every_precision() {
        let offset = FixedOffset::east_opt(3600).unwrap();
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
            .unwrap()
            .and_hms_nano_opt(7, 39, 0, 123_456_789)
            .unwrap();
        let timestamp = offset.from_local_datetime(&naive).unwrap();
        // Precision selects how much of the fraction reaches the wire; the
        // Secs form drops it entirely.
        assert_round_trip(TzTimestamp::with_secs(timestamp), "20060901-07:39:00+01");
        assert_round_trip(
            TzTimestamp::with_millis(timestamp),
            "20060901-07:39:00.123+01",
        );
        assert_round_trip(
            TzTimestamp::with_micros(timestamp),
            "20060901-07:39:00.123456+01",
        );
        assert_round_trip(
            TzTimestamp::with_nanos(timestamp),
            "20060901-07:39:00.123456789+01",
        );
    }

    #[test]
    fn tz_time_only_round_trips_at_every_precision() {
        let time = NaiveTime::from_hms_nano_opt(7, 39, 0, 123_456_789).unwrap();
        let offset = FixedOffset::east_opt(-18_000).unwrap();
        assert_round_trip(TzTimeOnly::with_secs(time, offset), "07:39:00-05");
        assert_round_trip(TzTimeOnly::with_millis(time, offset), "07:39:00.123-05");
        assert_round_trip(TzTimeOnly::with_micros(time, offset), "07:39:00.123456-05");
        assert_round_trip(
            TzTimeOnly::with_nanos(time, offset),
            "07:39:00.123456789-05",
        );
    }

    #[test]
    fn tz_time_only_accepts_the_wire_form_without_seconds() {
        // The grammar makes :SS optional on input; the value defaults to
        // whole seconds, which is what the output form then carries.
        let parsed: TzTimeOnly = de("07:39Z").expect("valid value rejected");
        assert_eq!(
            parsed.timestamp(),
            NaiveTime::from_hms_opt(7, 39, 0).unwrap()
        );
        assert_eq!(parsed.precision(), TimePrecision::Secs);
        assert_eq!(parsed.offset(), FixedOffset::east_opt(0).unwrap());
    }

    #[test]
    fn tz_timestamp_accepts_the_wire_form_without_seconds() {
        // The grammar in TagValue Encoding section 6.2.2 shows SS, but every
        // example in that table omits it - including the spec's own
        // "20060901-07:39Z". Output always carries seconds.
        let parsed: TzTimestamp = de("20060901-07:39Z").expect("valid value rejected");
        assert_eq!(parsed.precision(), TimePrecision::Secs);
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            "\"20060901-07:39:00Z\""
        );
        let with_fraction: TzTimestamp =
            de("20060901-13:09.123+05:30").expect("valid value rejected");
        assert_eq!(
            serde_json::to_string(&with_fraction).unwrap(),
            "\"20060901-13:09:00.123+05:30\""
        );
    }

    #[test]
    fn sub_minute_offset_fails_to_serialize() {
        // The wire form of an offset carries whole minutes only, so rendering
        // such a value would silently drop the seconds part.
        let offset = FixedOffset::east_opt(45).unwrap();
        let time = NaiveTime::from_hms_opt(7, 39, 0).unwrap();
        assert!(serde_json::to_string(&TzTimeOnly::with_secs(time, offset)).is_err());
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
            .unwrap()
            .and_hms_opt(7, 39, 0)
            .unwrap();
        let value = TzTimestamp::with_secs(offset.from_local_datetime(&naive).unwrap());
        assert!(serde_json::to_string(&value).is_err());
    }

    #[test]
    fn out_of_range_year_fails_to_serialize() {
        // Same guard as UtcTimestamp: chrono renders such years with a sign
        // and extra digits, producing a string no deserializer accepts.
        let naive = NaiveDate::from_ymd_opt(-1, 9, 1)
            .unwrap()
            .and_hms_opt(7, 39, 0)
            .unwrap();
        let offset = FixedOffset::east_opt(0).unwrap();
        let value = TzTimestamp::with_secs(offset.from_local_datetime(&naive).unwrap());
        assert!(serde_json::to_string(&value).is_err());
    }

    #[test]
    fn malformed_values_are_rejected() {
        // Missing offset.
        assert!(de::<TzTimestamp>("20060901-07:39:00").is_err());
        assert!(de::<TzTimeOnly>("07:39:00").is_err());
        // Leap second - TZ values carry none.
        assert!(de::<TzTimestamp>("20060901-07:39:60Z").is_err());
        // Offset out of range.
        assert!(de::<TzTimestamp>("20060901-07:39:00+99").is_err());
        // Trailing garbage - the whole input must be consumed.
        assert!(de::<TzTimestamp>("20060901-07:39:00Zx").is_err());
        assert!(de::<TzTimeOnly>("07:39:00Z\x01").is_err());
        assert!(de::<UtcTimeOnly>("03:04:05x").is_err());
        // Truncated.
        assert!(de::<TzTimestamp>("20060901-07:39").is_err());
        assert!(de::<TzTimeOnly>("07:39:00+0").is_err());
        assert!(de::<UtcTimeOnly>("03:04").is_err());
        assert!(de::<TzTimestamp>("").is_err());
        assert!(de::<TzTimeOnly>("").is_err());
        assert!(de::<UtcTimeOnly>("").is_err());
    }
}

#[cfg(all(test, feature = "serde-deserialize"))]
mod tenor_serde_de_tests {
    use serde::{
        Deserialize,
        de::value::{Error as DeError, StrDeserializer},
    };

    use super::*;

    fn de(input: &str) -> Result<Tenor, DeError> {
        Tenor::deserialize(StrDeserializer::<DeError>::new(input))
    }

    #[test]
    fn every_unit_round_trips() {
        for (input, unit, value) in [
            ("D5", TenorUnit::Days, 5),
            ("M3", TenorUnit::Months, 3),
            ("W13", TenorUnit::Weeks, 13),
            ("Y1", TenorUnit::Years, 1),
        ] {
            let parsed = de(input).expect("valid tenor rejected");
            assert_eq!(parsed, Tenor { unit, value });
        }
    }

    #[test]
    fn malformed_values_are_rejected() {
        // Unknown unit code.
        assert!(de("X5").is_err());
        // Unit without a value.
        assert!(de("D").is_err());
        // Zero value - rejected on the wire as well.
        assert!(de("D0").is_err());
        // Value before the unit.
        assert!(de("5D").is_err());
        // Trailing garbage - the whole input must be consumed.
        assert!(de("D5x").is_err());
        assert!(de("D5\x01").is_err());
        // Value above the u16 range of Length.
        assert!(de("D65536").is_err());
        assert!(de("").is_err());
    }
}

#[cfg(all(test, feature = "serde-serialize"))]
mod tenor_serde_ser_tests {
    use super::*;

    #[test]
    fn tenor_serializes_to_wire_string() {
        for (unit, value, expected) in [
            (TenorUnit::Days, 5, "\"D5\""),
            (TenorUnit::Months, 3, "\"M3\""),
            (TenorUnit::Weeks, 13, "\"W13\""),
            (TenorUnit::Years, 1, "\"Y1\""),
        ] {
            let tenor = Tenor { unit, value };
            assert_eq!(serde_json::to_string(&tenor).unwrap(), expected);
        }
    }
}

#[cfg(all(test, feature = "serde-serialize", feature = "serde-deserialize"))]
mod decimal_serde_tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn decimal_is_a_string_that_keeps_its_scale() {
        // The `rust_decimal/serde` representation must stay string-based:
        // switching it to a float would drop trailing zeros and round values
        // that FIX carries exactly.
        let price = Decimal::from_str("97.0340").unwrap();
        let json = serde_json::to_string(&price).unwrap();
        assert_eq!(json, "\"97.0340\"");
        let parsed: Decimal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, price);
        assert_eq!(parsed.scale(), price.scale());
    }

    #[test]
    fn decimal_accepts_a_json_number() {
        let parsed: Decimal = serde_json::from_str("97.0347").unwrap();
        assert_eq!(parsed, Decimal::from_str("97.0347").unwrap());
    }
}
