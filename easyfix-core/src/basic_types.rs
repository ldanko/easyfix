//! The FIX datatypes every message is built from.
//!
//! Three groups live here:
//!
//! - **String types** - [`FixStr`] / [`FixString`], a `str`/`String` pair
//!   restricted to printable ASCII (`0x20`-`0x7e`), so a value can never
//!   carry an SOH and break message framing.
//! - **Wire-format types** - the timestamps ([`UtcTimestamp`],
//!   [`TzTimestamp`], ...), each carrying the [`TimePrecision`] it renders
//!   with, plus [`Tenor`], [`Country`], [`Currency`] and the numeric aliases.
//!   Money is [`Decimal`], never a float.
//! - **Field newtypes** - [`MsgTypeField`], [`SessionStatusField`],
//!   [`SessionRejectReasonField`] and [`ApplVerId`], which the session layer
//!   compares against the base enums without knowing the generated ones.

use std::{borrow, cmp, error::Error as StdError, fmt, hash, mem, num::NonZero, ops, str};

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

#[cfg(test)]
mod tests;

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

/// Number of fractional-second digits a time value carries on the wire.
///
/// The FIX baseline is the millisecond; a finer width is agreed with the
/// counterparty, per field (TagValue Encoding section 6.2.2). There is no
/// `Default` - every construction states the width it wants. Session-layer
/// timestamps take it from the session's configuration; application fields
/// state their own.
///
/// On a parsed value this reports the width that arrived, so re-serializing
/// reproduces it. Two received forms are exceptions, and matter to code that
/// echoes a counterparty's timestamp back:
///
/// - a zoned value written without seconds (`20060901-07:39Z`, which the
///   grammar permits) reads back as `Secs` and re-emits as
///   `20060901-07:39:00Z`;
/// - a 12-digit picosecond fraction reads back as `Nanos` and re-emits with
///   9 digits - chrono holds no finer resolution.
// No `Default`, deliberately: a type-level answer would fix the wire format
// globally for a choice that belongs to one counterparty and one field, and
// callers would inherit it without noticing. Do not add one.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde-serialize", derive(serde::Serialize))]
#[cfg_attr(feature = "serde-deserialize", derive(serde::Deserialize))]
pub enum TimePrecision {
    /// Whole seconds, no fractional part and no period.
    Secs = 0,
    /// Milliseconds - 3 digits. The FIX baseline.
    Millis = 3,
    /// Microseconds - 6 digits.
    Micros = 6,
    /// Nanoseconds - 9 digits. The finest width representable here; a
    /// 12-digit picosecond fraction read off the wire lands as this.
    Nanos = 9,
}

/// Date and time in UTC, e.g. `20060901-07:39:00.123`, carrying the
/// fractional-second width it is rendered with.
#[derive(Clone, Copy, Debug)]
pub struct UtcTimestamp {
    timestamp: DateTime<Utc>,
    precision: TimePrecision,
}

/// Nanoseconds to keep when reducing `time` to whole-second precision: `0`
/// for an ordinary time, a whole second for a UTC leap second, which chrono
/// represents as `:59` carrying a nanosecond value of at least a second (see
/// [`Timelike::nanosecond`]). Only the sub-second fraction is dropped; the
/// leap offset survives.
//
// Clearing the nanosecond field outright would silently move `23:59:60` to
// `23:59:59` - a different instant, and a different value on the wire, where
// `SS = 00-60` is valid (TagValue Encoding section 6.2.2).
fn whole_second_nanos(time: &impl Timelike) -> u32 {
    const LEAP: u32 = 1_000_000_000;
    if time.nanosecond() >= LEAP { LEAP } else { 0 }
}

/// Time of day in UTC, e.g. `07:39:00.123`, carrying the fractional-second
/// width it is rendered with. Paired with a [`UtcDateOnly`] where a full
/// timestamp would cost bandwidth.
#[derive(Clone, Copy, Debug)]
pub struct UtcTimeOnly {
    timestamp: NaiveTime,
    precision: TimePrecision,
}
/// Date in UTC, rendered `YYYYMMDD`.
pub type UtcDateOnly = NaiveDate;

/// Time local to a market center, rendered `HH:MM:SS`. The zone does not
/// follow from the value - a separate field names the market center.
pub type LocalMktTime = NaiveTime;
/// Date local to a market center, rendered `YYYYMMDD`.
pub type LocalMktDate = NaiveDate;

/// Date and time carrying a timezone offset, e.g. `20060901-07:39:00+05:30`.
///
/// The offset is required, in this type and on the wire. FIX defines the
/// datatype as "local time with an offset to UTC to allow identification of
/// local time and time zone offset of that time" (TagValue Encoding section
/// 6.2.2).
///
/// A local time whose zone does *not* follow from the value belongs in
/// [`LocalMktDate`] and [`LocalMktTime`] instead. Those name a time local to
/// a market center, with the market center identified by a separate field.
///
/// The seconds are optional on input (the spec's own examples omit them) and
/// always present on output.
//
// The grammar in TagValue Encoding section 6.2.2 brackets the offset, but
// every example it gives carries one, and without it the value is a wall
// clock reading that cannot be placed on a timeline. Do not make it optional.
#[derive(Clone, Copy, Debug)]
pub struct TzTimestamp {
    timestamp: DateTime<FixedOffset>,
    precision: TimePrecision,
}

/// Time of day carrying a timezone offset, e.g. `07:39:00+05:30`.
///
/// The offset is required, as in [`TzTimestamp`]. A time of day whose zone
/// does *not* follow from the value belongs in [`LocalMktTime`], where the
/// market center supplying the zone is named in a separate field.
#[derive(Clone, Copy, Debug)]
pub struct TzTimeOnly {
    timestamp: NaiveTime,
    offset: FixedOffset,
    precision: TimePrecision,
}

/// Wire type of every FIX `Length` field - including `BodyLength<9>`.
///
/// The `u16` width caps a message body at 65535 octets, so a whole TagValue
/// message tops out around 65.5 KB once `8=`, `9=` and the `10=` trailer are
/// counted. [`raw_message`] turns an out-of-range `BodyLength<9>` into
/// [`RawMessageError::Garbled`] before any body is buffered.
///
/// [`raw_message`]: crate::deserializer::raw_message
/// [`RawMessageError::Garbled`]: crate::deserializer::RawMessageError::Garbled
//
// The width is a deliberate ceiling, not an incidental choice: everything
// reading from a socket depends on it to bound its buffer. Widening this type
// would surrender that bound.
pub type Length = u16;
pub type NonZeroLength = NonZero<Length>;
/// Raw bytes with no format restrictions. Delimited by a preceding `Length`
/// field, not by SOH, so the content may contain any byte.
pub type Data = Vec<u8>;
/// An XML document, delimited the same way as [`Data`].
pub type XmlData = Data;

/// Time unit of a [`Tenor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TenorUnit {
    /// Wire code `D`.
    Days,
    /// Wire code `M`.
    Months,
    /// Wire code `W`.
    Weeks,
    /// Wire code `Y`.
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

/// How many [`TenorUnit`]s a [`Tenor`] counts.
///
/// A tenor of zero units is a reject with `ValueIsIncorrect` on the wire, so
/// it is unrepresentable here. The `u16` width caps a tenor at 65535 units,
/// which no maturity reaches in any unit the type offers.
pub type TenorValue = NonZero<u16>;

/// A time-to-maturity expressed as a unit and a count, e.g. `M3` for three
/// months. Rendered as the unit code followed by the value, with no
/// separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tenor {
    /// The unit the value counts.
    pub unit: TenorUnit,
    /// How many units.
    pub value: TenorValue,
}

/// A byte that cannot appear in a [`FixStr`], with where it was found.
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

impl StdError for FixStringError {}

const fn is_non_control_ascii_char(byte: u8) -> bool {
    byte > 0x1f && byte < 0x7f
}

impl FixStr {
    /// Converts a slice of bytes to a string slice.
    ///
    /// A [`&FixStr`] requires printable ASCII (`0x20`-`0x7e`); `from_ascii`
    /// checks every byte before converting. To skip the check see
    /// [`from_ascii_unchecked`]; for an owned result see
    /// [`FixString::from_ascii`].
    ///
    /// [`&FixStr`]: FixStr
    /// [`from_ascii_unchecked`]: FixStr::from_ascii_unchecked
    ///
    /// # Errors
    ///
    /// Returns `Err` if any byte is outside printable ASCII, reporting its
    /// index and value.
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

    /// Converts a slice of bytes to a FIX string slice without checking it.
    ///
    /// See the safe version, [`from_ascii`], for more information.
    ///
    /// [`from_ascii`]: FixStr::from_ascii
    ///
    /// # Safety
    ///
    /// Every byte passed in must be printable ASCII (`0x20`-`0x7e`).
    pub const unsafe fn from_ascii_unchecked(buf: &[u8]) -> &FixStr {
        // SAFETY: the caller must guarantee that the bytes `buf` are valid ASCII.
        // Also relies on `&FixStr` and `&[u8]` having the same layout.
        unsafe { mem::transmute(buf) }
    }

    pub const fn as_utf8(&self) -> &str {
        // SAFETY: ASCII is always valid UTF-8
        unsafe { str::from_utf8_unchecked(&self.0) }
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
    /// [`SerializeError::EmptyValue`]. Use it where a `Default` or a `const`
    /// starting value is needed; to express a genuinely absent value, use
    /// `Option<FixString>` instead.
    ///
    /// [`SerializeError::EmptyValue`]: crate::serializer::SerializeError::EmptyValue
    pub const fn new() -> FixString {
        FixString(Vec::new())
    }

    /// Converts a vector of bytes to a `FixString`.
    ///
    /// A `FixString` requires printable ASCII (`0x20`-`0x7e`); `from_ascii`
    /// checks every byte and then takes ownership of the vector without
    /// copying it. To skip the check see [`from_ascii_unchecked`]; for a
    /// borrowed result see [`FixStr::from_ascii`]. The inverse is
    /// [`into_bytes`].
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if any byte is outside printable ASCII, reporting its
    /// index and value.
    ///
    /// [`from_ascii_unchecked`]: FixString::from_ascii_unchecked
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

    /// Converts a vector of bytes to a `FixString` without checking it.
    ///
    /// See the safe version, [`from_ascii`], for more details.
    ///
    /// [`from_ascii`]: FixString::from_ascii
    ///
    /// # Safety
    ///
    /// Every byte passed in must be printable ASCII (`0x20`-`0x7e`).
    /// Violating this is undefined behavior: the rest of the library relies
    /// on the invariant, including `as_utf8`, which skips UTF-8 validation.
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
        unsafe { str::from_utf8_unchecked(&self.0) }
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

        let year = self.timestamp.year();
        if !year_is_wire_representable(year) {
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
        if !second_is_wire_representable(&self.timestamp) {
            return Err(S::Error::custom(
                "leap second is not representable in the FIX TZTimestamp format (SS = 00-59)",
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
        if !second_is_wire_representable(&self.timestamp) {
            return Err(S::Error::custom(
                "leap second is not representable in the FIX TZTimeOnly format (SS = 00-59)",
            ));
        }
        serializer.collect_str(self)
    }
}

/// Compares the instant only - two values are equal whatever
/// [`TimePrecision`] they carry, even though they would render differently.
impl PartialEq for UtcTimestamp {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp
    }
}

impl Eq for UtcTimestamp {}

#[expect(
    clippy::non_canonical_partial_ord_impl,
    reason = "ordering is total and compares the timestamp only, exactly like `Ord` below and `PartialEq` above; spelling it out keeps the three impls readable side by side"
)]
impl PartialOrd for UtcTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.timestamp.cmp(&other.timestamp))
    }
}

impl Ord for UtcTimestamp {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
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
    /// Upper bound of the underlying clock. Like [`UtcTimestamp::MIN_UTC`] it
    /// is a sentinel, not a value - it cannot be put on the wire.
    pub const MAX_UTC: UtcTimestamp = UtcTimestamp {
        timestamp: DateTime::<Utc>::MAX_UTC,
        precision: TimePrecision::Nanos,
    };
    /// Lower bound of the underlying clock, and the "not set yet" sentinel
    /// returned by [`UtcTimestamp::default`].
    ///
    /// Neither bound can be transmitted: their years (`-262144` and `262143`)
    /// fall outside the `YYYY = 0000-9999` the grammar allows (TagValue
    /// Encoding section 6.2.2), so serializing one fails with
    /// [`SerializeError::InvalidValue`] rather than emitting anything. That is
    /// what makes the lower bound a usable sentinel: a timestamp left unfilled
    /// is caught here, not at the counterparty.
    ///
    /// The precision they carry is arbitrary - it reaches no renderer, and
    /// equality ignores it.
    ///
    /// [`SerializeError::InvalidValue`]: crate::serializer::SerializeError::InvalidValue
    pub const MIN_UTC: UtcTimestamp = UtcTimestamp {
        timestamp: DateTime::<Utc>::MIN_UTC,
        precision: TimePrecision::Nanos,
    };

    /// The Unix epoch, `1970-01-01 00:00:00 UTC`, rendered with `precision`
    /// fractional-second digits.
    ///
    /// Unlike [`MIN_UTC`](Self::MIN_UTC) this is an ordinary transmittable
    /// value, so it reaches the counterparty as a real timestamp. Reach for it
    /// as an explicit placeholder, or when converting from a source where zero
    /// means the epoch - not as a stand-in for "not set", which is what the
    /// sentinel is for.
    ///
    /// Truncation to `precision` is a no-op here - the epoch has no fraction
    /// to lose.
    pub const fn unix_epoch(precision: TimePrecision) -> UtcTimestamp {
        UtcTimestamp {
            timestamp: DateTime::<Utc>::UNIX_EPOCH,
            precision,
        }
    }
}

impl Default for UtcTimestamp {
    /// [`UtcTimestamp::MIN_UTC`] - the "not set yet" sentinel, **not** a
    /// usable timestamp.
    ///
    /// Leave a `SendingTime<52>` defaulted and the session stamps it at
    /// transmit time, at the precision configured for the session; set it to
    /// anything else and that stamping is skipped. Anywhere other than a
    /// header field the default is a bug - it cannot be put on the wire (see
    /// [`UtcTimestamp::MIN_UTC`]). Build application timestamps with
    /// [`UtcTimestamp::now`] or [`UtcTimestamp::with_precision`] instead.
    fn default() -> Self {
        UtcTimestamp::MIN_UTC
    }
}

impl UtcTimestamp {
    /// Current date and time, rendered with `precision` fractional-second
    /// digits. See [`TimePrecision`] for choosing one.
    pub fn now(precision: TimePrecision) -> UtcTimestamp {
        UtcTimestamp::with_precision(Utc::now(), precision)
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

    /// Current date and time, truncated to whole seconds.
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
//
// chrono renders anything outside that range with a sign and more digits,
// producing a value no parser accepts - and one that still travels inside a
// well-formed message, since BodyLength and CheckSum are computed over
// whatever was written. Both the serializer and the serde impls refuse such a
// value instead of emitting it.
pub(crate) fn year_is_wire_representable(year: i32) -> bool {
    (0..=9999).contains(&year)
}

/// Whether a UTC offset can be rendered in the FIX wire form, which carries
/// whole minutes only (`hh[:mm]`).
//
// A `FixedOffset` can hold seconds; the renderer would silently drop them.
pub(crate) fn offset_is_wire_representable(offset: FixedOffset) -> bool {
    offset.local_minus_utc() % 60 == 0
}

/// Whether a second can be rendered by the FIX datatypes whose grammar caps
/// it at `SS = 00-59`: `TZTimestamp`, `TZTimeOnly` and `LocalMktTime`
/// (TagValue Encoding section 6.2.2). Those are stricter than `UTCTimestamp` /
/// `UTCTimeOnly`, where the same section allows `SS = 00-60 (60 only if UTC
/// leap second)`.
//
// chrono carries a leap second as a nanosecond value of at least a whole
// second and renders it as `:60` whatever the type, so without this check a
// leap-second TzTimestamp emits a field the grammar forbids - and one our own
// parsers reject, since they accept `0-5` in the tens-of-seconds position. The
// leap offset stays in the value rather than being normalized away (see
// `whole_second_nanos`): clearing it would move the instant, so the
// unrepresentable value is refused here instead of silently rewritten.
pub(crate) fn second_is_wire_representable(time: &impl Timelike) -> bool {
    time.nanosecond() < 1_000_000_000
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
    /// Build a value rendered with `precision` fractional-second digits,
    /// truncating anything finer.
    pub fn with_precision(
        timestamp: DateTime<FixedOffset>,
        precision: TimePrecision,
    ) -> TzTimestamp {
        match precision {
            TimePrecision::Secs => TzTimestamp::with_secs(timestamp),
            TimePrecision::Millis => TzTimestamp::with_millis(timestamp),
            TimePrecision::Micros => TzTimestamp::with_micros(timestamp),
            TimePrecision::Nanos => TzTimestamp::with_nanos(timestamp),
        }
    }

    /// Build a value rendered with whole seconds, dropping any fraction.
    pub fn with_secs(timestamp: DateTime<FixedOffset>) -> TzTimestamp {
        TzTimestamp {
            timestamp: timestamp
                .with_nanosecond(whole_second_nanos(&timestamp))
                .unwrap(),
            precision: TimePrecision::Secs,
        }
    }

    /// Build a value rendered with 3 fractional digits, truncating finer.
    pub fn with_millis(timestamp: DateTime<FixedOffset>) -> TzTimestamp {
        TzTimestamp {
            timestamp: timestamp
                .with_nanosecond(timestamp.nanosecond() / 1_000_000 * 1_000_000)
                .unwrap(),
            precision: TimePrecision::Millis,
        }
    }

    /// Build a value rendered with 6 fractional digits, truncating finer.
    pub fn with_micros(timestamp: DateTime<FixedOffset>) -> TzTimestamp {
        TzTimestamp {
            timestamp: timestamp
                .with_nanosecond(timestamp.nanosecond() / 1_000 * 1_000)
                .unwrap(),
            precision: TimePrecision::Micros,
        }
    }

    /// Build a value rendered with 9 fractional digits - the full
    /// resolution chrono carries.
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
    /// Build a value rendered with `precision` fractional-second digits,
    /// truncating anything finer.
    pub fn new(timestamp: NaiveTime, offset: FixedOffset, precision: TimePrecision) -> TzTimeOnly {
        match precision {
            TimePrecision::Secs => TzTimeOnly::with_secs(timestamp, offset),
            TimePrecision::Millis => TzTimeOnly::with_millis(timestamp, offset),
            TimePrecision::Micros => TzTimeOnly::with_micros(timestamp, offset),
            TimePrecision::Nanos => TzTimeOnly::with_nanos(timestamp, offset),
        }
    }

    /// Build a value rendered with whole seconds, dropping any fraction.
    pub fn with_secs(timestamp: NaiveTime, offset: FixedOffset) -> TzTimeOnly {
        TzTimeOnly {
            timestamp: timestamp
                .with_nanosecond(whole_second_nanos(&timestamp))
                .unwrap(),
            offset,
            precision: TimePrecision::Secs,
        }
    }

    /// Build a value rendered with 3 fractional digits, truncating finer.
    pub fn with_millis(timestamp: NaiveTime, offset: FixedOffset) -> TzTimeOnly {
        TzTimeOnly {
            timestamp: timestamp
                .with_nanosecond(timestamp.nanosecond() / 1_000_000 * 1_000_000)
                .unwrap(),
            offset,
            precision: TimePrecision::Millis,
        }
    }

    /// Build a value rendered with 6 fractional digits, truncating finer.
    pub fn with_micros(timestamp: NaiveTime, offset: FixedOffset) -> TzTimeOnly {
        TzTimeOnly {
            timestamp: timestamp
                .with_nanosecond(timestamp.nanosecond() / 1_000 * 1_000)
                .unwrap(),
            offset,
            precision: TimePrecision::Micros,
        }
    }

    /// Build a value rendered with 9 fractional digits - the full
    /// resolution chrono carries.
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
// MsgType (tag 35) - compact 1-2 byte representation
// ---------------------------------------------------------------------------

/// Why bytes could not be read as a [`MsgTypeField`].
#[derive(Debug, thiserror::Error)]
pub enum MsgTypeError {
    /// No bytes at all.
    #[error("Empty message type")]
    Empty,
    /// A byte outside `0-9`, `a-z`, `A-Z`.
    #[error("Invalid character in message type: {0}")]
    InvalidChar(u8),
    /// More than two bytes.
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

/// Compact, `Copy` newtype wrapping a validated MsgType raw value: 1-2 ASCII
/// alphanumeric bytes, stored inline.
///
/// `Borrow<[u8]>` yields only the live bytes, so a `HashMap<MsgTypeField, _>`
/// can be looked up by a `&[u8]` slice without allocating.
//
// Single-byte values keep `0` as a sentinel in the second position, and `Hash`
// is implemented manually over the live bytes only - hashing the sentinel too
// would break the `Borrow` contract.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MsgTypeField {
    buf: [u8; 2],
}

impl hash::Hash for MsgTypeField {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl<T: MsgTypeValue> From<T> for MsgTypeField {
    fn from(v: T) -> Self {
        v.raw_value()
    }
}

impl MsgTypeField {
    /// Construct from a 1-2 byte raw MsgType value, unvalidated. A single-byte
    /// value must be padded with `0`.
    pub(crate) const fn from_raw(buf: [u8; 2]) -> Self {
        MsgTypeField { buf }
    }

    /// Validate 1-2 ASCII alphanumeric bytes as a MsgType value.
    ///
    /// # Errors
    ///
    /// [`MsgTypeError::Empty`] for no bytes, [`MsgTypeError::TooLong`] for
    /// more than two, [`MsgTypeError::InvalidChar`] for anything outside
    /// `0-9`, `a-z`, `A-Z`.
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

    /// The live bytes - one or two, never the padding.
    pub fn as_bytes(&self) -> &[u8] {
        match self.buf {
            [_, 0] => &self.buf[..1],
            [_, _] => &self.buf,
        }
    }

    /// The live bytes as `&str`. Infallible - the validated bytes are ASCII.
    pub fn as_str(&self) -> &str {
        // SAFETY: We validate during construction that all bytes are ASCII
        //         alphanumeric (0-9, a-z, A-Z), which are all valid UTF-8
        unsafe { str::from_utf8_unchecked(self.as_bytes()) }
    }

    /// The live bytes as `&FixStr`. Infallible - the validated bytes are
    /// within the printable-ASCII range `FixStr` requires.
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

impl str::FromStr for MsgTypeField {
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
    /// The raw tag 1409 value.
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
    /// The raw tag 373 value.
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
/// here). There is no `Default` - every construction names a version, and
/// [`ApplVerId::DEFAULT_IF_ABSENT`] covers the one case the spec defines.
//
// No `Default`, deliberately: an implicit application version is how a silent
// `1137=0` (FIX 2.7) ends up on the wire. Do not add one.
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

    /// The FIX wire value, e.g. `9` for FIX 5.0 SP2.
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

    /// The FIX wire value as bytes.
    pub const fn as_bytes(self) -> &'static [u8] {
        self.as_fix_str().as_bytes()
    }

    /// Parse a FIX wire value, or `None` when it is outside the
    /// ApplVerIDCodeSet.
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

    /// Parse a FIX wire value, reporting the offending value on failure.
    /// [`from_bytes`](Self::from_bytes) is the same check with an `Option`.
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
