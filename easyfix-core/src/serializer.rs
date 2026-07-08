//! Writing FIX tag-value messages.
//!
//! [`Serializer`] appends fields into a caller-owned buffer; the generated
//! message types drive it one field at a time. Nothing is allocated, so the
//! buffer's length is the message size limit.

use std::fmt::{self, Write};

use chrono::Datelike;

use crate::basic_types::{
    Amt, Boolean, Char, Country, Currency, Data, DayOfMonth, Exchange, FixStr, Float, Int,
    Language, Length, LocalMktDate, LocalMktTime, MonthYear, MultipleCharValue,
    MultipleStringValue, NumInGroup, Percentage, Price, PriceOffset, Qty, SeqNum, TagNum, Tenor,
    TzTimeOnly, TzTimestamp, UtcDateOnly, UtcTimeOnly, UtcTimestamp, XmlData,
    offset_is_wire_representable, second_is_wire_representable, year_is_wire_representable,
};

#[cfg(test)]
mod tests;

/// Number of digits reserved for the BodyLength(9) placeholder, derived
/// from the output buffer size. Capped at 8 digits: with a buffer of
/// 100 MB or more the encodable body length tops out at 99 999 999 bytes,
/// and a body that does not fit the placeholder fails serialization with
/// `MaxMessageSizeExceeded` when the value is patched in.
const fn max_body_len_digits(max_msg_size: usize) -> usize {
    if max_msg_size < 10 {
        1
    } else if max_msg_size < 100 {
        2
    } else if max_msg_size < 1000 {
        3
    } else if max_msg_size < 10_000 {
        4
    } else if max_msg_size < 100_000 {
        5
    } else if max_msg_size < 1_000_000 {
        6
    } else if max_msg_size < 10_000_000 {
        7
    } else {
        8
    }
}

/// Why a value could not be written to the wire.
#[derive(Debug, thiserror::Error)]
pub enum SerializeError {
    /// The write would run past the end of the output buffer, or the patched
    /// BodyLength(9) does not fit the placeholder reserved for it.
    #[error("max message size exceeded")]
    MaxMessageSizeExceeded,
    /// A field that must carry at least one byte was empty: `FixStr`, `Data`,
    /// `XmlData`, a `MultipleCharValue` / `MultipleStringValue` collection or
    /// one of its elements, or an enum collection.
    #[error("empty value")]
    EmptyValue,
    /// A value outside the range its FIX datatype allows: a zero `TagNum`,
    /// `SeqNum`, `NumInGroup` or `Length`, a `Char` outside printable ASCII
    /// (`0x20..=0x7e`), or a timestamp the wire grammar cannot render (year
    /// outside `0000-9999`, sub-minute UTC offset, leap second in a zoned
    /// type).
    #[error("invalid value")]
    InvalidValue,
}

fn validate_char(c: Char) -> Result<(), SerializeError> {
    if matches!(c, 0x20..=0x7e) {
        Ok(())
    } else {
        Err(SerializeError::InvalidValue)
    }
}

/// Writes FIX tag-value fields into a caller-provided byte buffer.
///
/// The buffer is the message size limit: nothing is ever allocated, and a
/// message that does not fit fails with
/// [`SerializeError::MaxMessageSizeExceeded`] rather than growing. Size the
/// buffer for the largest message the session may emit.
///
/// A field is three calls - tag prefix, typed value, delimiter:
///
/// ```
/// # use easyfix_core::{basic_types::FixStr, fix_str, serializer::Serializer};
/// # fn main() -> Result<(), easyfix_core::serializer::SerializeError> {
/// let mut buf = [0u8; 64];
/// let mut serializer = Serializer::new(&mut buf);
/// serializer.put_slice(b"49=")?;
/// serializer.serialize_string(fix_str!("SENDER"))?;
/// serializer.put_soh()?;
/// assert_eq!(serializer.written(), b"49=SENDER\x01");
/// # Ok(())
/// # }
/// ```
///
/// A whole message additionally brackets the body with
/// [`serialize_body_len`](Self::serialize_body_len) (which reserves the
/// BodyLength(9) placeholder) and [`serialize_checksum`](Self::serialize_checksum)
/// (which patches that placeholder and appends CheckSum(10)).
pub struct Serializer<'a> {
    output: &'a mut [u8],
    pos: usize,
    body_start_idx: usize,
    /// Width of the BodyLength(9) placeholder written by
    /// `serialize_body_len`; `0` until then.
    body_len_digits: usize,
}

impl<'a> Serializer<'a> {
    /// Wraps `output`, whose length becomes this message's size limit.
    pub fn new(output: &'a mut [u8]) -> Serializer<'a> {
        Serializer {
            output,
            pos: 0,
            body_start_idx: 0,
            body_len_digits: 0,
        }
    }

    /// Number of bytes written so far.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The bytes written so far.
    pub fn written(&self) -> &[u8] {
        &self.output[..self.pos]
    }

    /// Append raw bytes.
    pub fn put_slice(&mut self, bytes: &[u8]) -> Result<(), SerializeError> {
        if bytes.len() > self.output.len() - self.pos {
            return Err(SerializeError::MaxMessageSizeExceeded);
        }
        let end = self.pos + bytes.len();
        self.output[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }

    /// Append a single byte.
    pub fn put_u8(&mut self, byte: u8) -> Result<(), SerializeError> {
        if self.pos >= self.output.len() {
            return Err(SerializeError::MaxMessageSizeExceeded);
        }
        self.output[self.pos] = byte;
        self.pos += 1;
        Ok(())
    }

    /// Write the FIX field delimiter (SOH, 0x01).
    pub fn put_soh(&mut self) -> Result<(), SerializeError> {
        self.put_u8(b'\x01')
    }

    /// Write the BodyLength(9) field as an all-zero placeholder and mark the
    /// body start.
    ///
    /// The placeholder width comes from the buffer size, not from the message,
    /// so the real length is patched in later by
    /// [`serialize_checksum`](Self::serialize_checksum). Call this once,
    /// directly after BeginString(8).
    pub fn serialize_body_len(&mut self) -> Result<(), SerializeError> {
        const PLACEHOLDERS: [&[u8]; 8] = [
            b"9=0\x01",
            b"9=00\x01",
            b"9=000\x01",
            b"9=0000\x01",
            b"9=00000\x01",
            b"9=000000\x01",
            b"9=0000000\x01",
            b"9=00000000\x01",
        ];
        let digits = max_body_len_digits(self.output.len());
        self.put_slice(PLACEHOLDERS[digits - 1])?;
        self.body_start_idx = self.pos;
        self.body_len_digits = digits;
        Ok(())
    }

    /// Close the message: patch the real body length into the placeholder,
    /// then append CheckSum(10) over every byte written before it.
    ///
    /// Returns [`SerializeError::MaxMessageSizeExceeded`] when the body length
    /// needs more digits than the placeholder reserved - including the case
    /// where [`serialize_body_len`](Self::serialize_body_len) was never
    /// called, which leaves no placeholder to patch.
    pub fn serialize_checksum(&mut self) -> Result<(), SerializeError> {
        let mut buffer = itoa::Buffer::new();

        let body_len = self.pos - self.body_start_idx;
        let body_len_slice = buffer.format(body_len).as_bytes();

        // Also rejects calling `serialize_checksum` without a preceding
        // `serialize_body_len` (`body_len_digits` is 0 then).
        if body_len_slice.len() > self.body_len_digits {
            return Err(SerializeError::MaxMessageSizeExceeded);
        }

        self.output[self.body_start_idx - body_len_slice.len() - 1..self.body_start_idx - 1]
            .copy_from_slice(body_len_slice);

        let checksum = self.output[..self.pos]
            .iter()
            .fold(0u8, |acc, &byte| u8::wrapping_add(acc, byte));

        self.put_slice(b"10=")?;
        if checksum < 10 {
            self.put_slice(b"00")?;
        } else if checksum < 100 {
            self.put_u8(b'0')?;
        }
        self.put_slice(buffer.format(checksum).as_bytes())?;
        self.put_u8(b'\x01')?;
        Ok(())
    }

    /// Serialize sequence of character digits without commas or decimals.
    /// Value must be positive and may not contain leading zeros.
    pub fn serialize_tag_num(&mut self, tag_num: &TagNum) -> Result<(), SerializeError> {
        if *tag_num == 0 {
            return Err(SerializeError::InvalidValue);
        }
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(*tag_num).as_bytes())
    }

    /// Serialize sequence of character digits without commas or decimals
    /// and optional sign character (characters `-` and `0` - `9`).
    /// The sign character utilizes one octet (i.e., positive int is `99999`
    /// while negative int is `-99999`).
    pub fn serialize_int(&mut self, int: &Int) -> Result<(), SerializeError> {
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(*int).as_bytes())
    }

    /// Serialize sequence of character digits without commas or decimals.
    /// Value must be positive.
    pub fn serialize_seq_num(&mut self, seq_num: &SeqNum) -> Result<(), SerializeError> {
        if *seq_num == 0 {
            return Err(SerializeError::InvalidValue);
        }
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(*seq_num).as_bytes())
    }

    /// Serialize sequence of character digits without commas or decimals.
    /// Value must be positive.
    pub fn serialize_num_in_group(
        &mut self,
        num_in_group: &NumInGroup,
    ) -> Result<(), SerializeError> {
        if *num_in_group == 0 {
            return Err(SerializeError::InvalidValue);
        }
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(*num_in_group).as_bytes())
    }

    /// Serialize sequence of character digits without commas or decimals
    /// (values 1 to 31).
    pub fn serialize_day_of_month(
        &mut self,
        day_of_month: DayOfMonth,
    ) -> Result<(), SerializeError> {
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(day_of_month).as_bytes())
    }

    /// Serialize sequence of character digits with optional decimal point
    /// and sign character (characters `-`, `0` - `9` and `.`);
    /// the absence of the decimal point within the string will be interpreted
    /// as the float representation of an integer value. Note that float values
    /// may contain leading zeros (e.g. `00023.23` = `23.23`) and may contain
    /// or omit trailing zeros after the decimal point
    /// (e.g. `23.0` = `23.0000` = `23` = `23.`).
    ///
    /// All float fields must accommodate up to fifteen significant digits.
    /// The number of decimal places used should be a factor of business/market
    /// needs and mutual agreement between counterparties.
    pub fn serialize_float(&mut self, float: &Float) -> Result<(), SerializeError> {
        self.put_slice(float.to_string().as_bytes())
    }

    /// Serialize a `Qty` - the `Float` grammar under a different name.
    pub fn serialize_qty(&mut self, qty: &Qty) -> Result<(), SerializeError> {
        self.serialize_float(qty)
    }

    /// Serialize a `Price` - the `Float` grammar under a different name.
    pub fn serialize_price(&mut self, price: &Price) -> Result<(), SerializeError> {
        self.serialize_float(price)
    }

    /// Serialize a `PriceOffset` - the `Float` grammar under a different name.
    pub fn serialize_price_offset(
        &mut self,
        price_offset: &PriceOffset,
    ) -> Result<(), SerializeError> {
        self.serialize_float(price_offset)
    }

    /// Serialize an `Amt` - the `Float` grammar under a different name.
    pub fn serialize_amt(&mut self, amt: &Amt) -> Result<(), SerializeError> {
        self.serialize_float(amt)
    }

    /// Serialize a `Percentage` - the `Float` grammar under a different name.
    pub fn serialize_percentage(&mut self, percentage: &Percentage) -> Result<(), SerializeError> {
        self.serialize_float(percentage)
    }

    /// Serialize a single-character boolean: `Y` for true, `N` for false.
    pub fn serialize_boolean(&mut self, boolean: &Boolean) -> Result<(), SerializeError> {
        if *boolean {
            self.put_u8(b'Y')
        } else {
            self.put_u8(b'N')
        }
    }

    /// Use any ASCII character except control characters.
    pub fn serialize_char(&mut self, c: &Char) -> Result<(), SerializeError> {
        validate_char(*c)?;
        self.put_u8(*c)
    }

    /// Serialize string containing one or more space-delimited single
    /// character values, e.g. `2 A F`.
    pub fn serialize_multiple_char_value(
        &mut self,
        mcv: &MultipleCharValue,
    ) -> Result<(), SerializeError> {
        if mcv.is_empty() {
            return Err(SerializeError::EmptyValue);
        }
        for c in mcv {
            validate_char(*c)?;
            self.put_u8(*c)?;
            self.put_u8(b' ')?;
        }
        // Drop trailing space (mirrors Vec::pop's no-op-on-empty behavior).
        self.pos = self.pos.saturating_sub(1);
        Ok(())
    }

    /// Serialize alphanumeric free-format strings can include any character
    /// except control characters.
    pub fn serialize_string(&mut self, input: &FixStr) -> Result<(), SerializeError> {
        if input.is_empty() {
            return Err(SerializeError::EmptyValue);
        }
        self.put_slice(input.as_bytes())
    }

    /// Serialize string containing one or more space-delimited multiple
    /// character values, e.g. `AV AN A`.
    pub fn serialize_multiple_string_value(
        &mut self,
        input: &MultipleStringValue,
    ) -> Result<(), SerializeError> {
        if input.is_empty() {
            return Err(SerializeError::EmptyValue);
        }
        for s in input {
            if s.is_empty() {
                return Err(SerializeError::EmptyValue);
            }
            self.put_slice(s.as_bytes())?;
            self.put_u8(b' ')?;
        }
        // Drop trailing space (mirrors Vec::pop's no-op-on-empty behavior).
        self.pos = self.pos.saturating_sub(1);
        Ok(())
    }

    /// Serialize ISO 3166-1:2013 Codes for the representation of names of
    /// countries and their subdivision (2-character code).
    pub fn serialize_country(&mut self, country: &Country) -> Result<(), SerializeError> {
        self.put_slice(country.to_bytes())
    }

    /// Serialize ISO 4217:2015 Codes for the representation of currencies
    /// and funds (3-character code).
    pub fn serialize_currency(&mut self, currency: &Currency) -> Result<(), SerializeError> {
        self.put_slice(currency.to_bytes())
    }

    /// Serialize ISO 10383:2012 Securities and related financial
    /// instruments - Codes for exchanges and market identification (MIC)
    /// (4-character code).
    pub fn serialize_exchange(&mut self, exchange: &Exchange) -> Result<(), SerializeError> {
        for &byte in exchange {
            validate_char(byte)?;
        }
        self.put_slice(exchange)
    }

    /// Serialize string representing month of a year.
    /// An optional day of the month can be appended or an optional week code.
    ///
    /// # Valid formats:
    /// YYYYMM
    /// YYYYMMDD
    /// YYYYMMWW
    ///
    /// # Valid values:
    /// YYYY = 0000-9999; MM = 01-12; DD = 01-31;
    /// WW = w1, w2, w3, w4, w5.
    pub fn serialize_month_year(&mut self, input: &MonthYear) -> Result<(), SerializeError> {
        self.serialize_string(input)
    }

    /// Serialize ISO 639-1:2002 Codes for the representation of names
    /// of languages (2-character code).
    pub fn serialize_language(&mut self, input: &Language) -> Result<(), SerializeError> {
        for &byte in input {
            validate_char(byte)?;
        }
        self.put_slice(input)
    }

    /// Serialize string representing time/date combination represented
    /// in UTC (Universal Time Coordinated) in either YYYYMMDD-HH:MM:SS
    /// (whole seconds) or YYYYMMDD-HH:MM:SS.sss* format, colons, dash,
    /// and period required.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31,
    /// - HH = 00-23,
    /// - MM = 0059,
    /// - SS = 00-60 (60 only if UTC leap second),
    /// - sss* fractions of seconds. The fractions of seconds may be empty when
    ///   no fractions of seconds are conveyed (in such a case the period
    ///   is not conveyed), it may include 3 digits to convey
    ///   milliseconds, 6 digits to convey microseconds, 9 digits
    ///   to convey nanoseconds, 12 digits to convey picoseconds;
    pub fn serialize_utc_timestamp(&mut self, input: &UtcTimestamp) -> Result<(), SerializeError> {
        if !year_is_wire_representable(input.timestamp().year()) {
            return Err(SerializeError::InvalidValue);
        }
        write!(self, "{}", input.format_precisely())
            .map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize string representing time-only represented in UTC
    /// (Universal Time Coordinated) in either HH:MM:SS (whole seconds)
    /// or HH:MM:SS.sss* (milliseconds) format, colons, and period required.
    ///
    /// This special-purpose field is paired with UTCDateOnly to form a proper
    /// UTCTimestamp for bandwidth-sensitive messages.
    ///
    /// # Valid values:
    /// - HH = 00-23,
    /// - MM = 00-59,
    /// - SS = 00-60 (60 only if UTC leap second),
    /// - sss* fractions of seconds. The fractions of seconds may be empty when
    ///   no fractions of seconds are conveyed (in such a case the period
    ///   is not conveyed), it may include 3 digits to convey
    ///   milliseconds, 6 digits to convey microseconds, 9 digits
    ///   to convey nanoseconds, 12 digits to convey picoseconds;
    pub fn serialize_utc_time_only(&mut self, input: &UtcTimeOnly) -> Result<(), SerializeError> {
        write!(self, "{input}").map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize date represented in UTC (Universal Time Coordinated)
    /// in YYYYMMDD format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn serialize_utc_date_only(&mut self, input: &UtcDateOnly) -> Result<(), SerializeError> {
        if !year_is_wire_representable(input.year()) {
            return Err(SerializeError::InvalidValue);
        }
        write!(self, "{}", input.format("%Y%m%d"))
            .map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize time local to a market center. Used where offset to UTC
    /// varies throughout the year and the defining market center is identified
    /// in a corresponding field.
    ///
    /// Format is HH:MM:SS where:
    /// - HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds.
    ///
    /// In general only the hour token is non-zero.
    pub fn serialize_local_mkt_time(&mut self, input: &LocalMktTime) -> Result<(), SerializeError> {
        if !second_is_wire_representable(input) {
            return Err(SerializeError::InvalidValue);
        }
        write!(self, "{}", input.format("%H:%M:%S"))
            .map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize date of local market (as opposed to UTC) in YYYYMMDD
    /// format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn serialize_local_mkt_date(&mut self, input: &LocalMktDate) -> Result<(), SerializeError> {
        if !year_is_wire_representable(input.year()) {
            return Err(SerializeError::InvalidValue);
        }
        write!(self, "{}", input.format("%Y%m%d"))
            .map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize string representing a time/date combination representing
    /// local time with an offset to UTC to allow identification of local time
    /// and time zone offset of that time.
    ///
    /// The representation is based on ISO 8601.
    ///
    /// Format is `YYYYMMDD-HH:MM:SS.sss*[Z | [ + | - hh[:mm]]]` where:
    /// - YYYY = 0000 to 9999,
    /// - MM = 01-12,
    /// - DD = 01-31 HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds,
    /// - hh = 01-12 offset hours,
    /// - mm = 00-59 offset minutes,
    /// - sss* fractions of seconds. The fractions of seconds may be empty when
    ///   no fractions of seconds are conveyed (in such a case the period
    ///   is not conveyed), it may include 3 digits to convey
    ///   milliseconds, 6 digits to convey microseconds, 9 digits
    ///   to convey nanoseconds, 12 digits to convey picoseconds;
    pub fn serialize_tz_timestamp(&mut self, input: &TzTimestamp) -> Result<(), SerializeError> {
        let timestamp = input.timestamp();
        if !year_is_wire_representable(timestamp.year())
            || !offset_is_wire_representable(*timestamp.offset())
            || !second_is_wire_representable(&timestamp)
        {
            return Err(SerializeError::InvalidValue);
        }
        write!(self, "{input}").map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize time of day with timezone. Time represented based on
    /// ISO 8601. This is the time with a UTC offset to allow identification of
    /// local time and time zone of that time.
    ///
    /// Format is `HH:MM[:SS][Z | [ + | - hh[:mm]]]` where:
    /// - HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds,
    /// - hh = 01-12 offset hours,
    /// - mm = 00-59 offset minutes.
    pub fn serialize_tz_timeonly(&mut self, input: &TzTimeOnly) -> Result<(), SerializeError> {
        if !offset_is_wire_representable(input.offset())
            || !second_is_wire_representable(&input.timestamp())
        {
            return Err(SerializeError::InvalidValue);
        }
        write!(self, "{input}").map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize sequence of character digits without commas or decimals.
    pub fn serialize_length(&mut self, length: &Length) -> Result<(), SerializeError> {
        if *length == 0 {
            return Err(SerializeError::InvalidValue);
        }
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(*length).as_bytes())
    }

    /// Serialize raw data with no format or content restrictions,
    /// or a character string encoded as specified by MessageEncoding(347).
    pub fn serialize_data(&mut self, data: &Data) -> Result<(), SerializeError> {
        if data.is_empty() {
            return Err(SerializeError::EmptyValue);
        }
        self.put_slice(data)
    }

    /// Serialize XML document.
    pub fn serialize_xml(&mut self, xml_data: &XmlData) -> Result<(), SerializeError> {
        if xml_data.is_empty() {
            return Err(SerializeError::EmptyValue);
        }
        self.put_slice(xml_data)
    }

    /// Serialize a tenor: the unit code (`D`, `M`, `W` or `Y`) followed by its
    /// value, e.g. `M3` for three months.
    pub fn serialize_tenor(&mut self, input: &Tenor) -> Result<(), SerializeError> {
        self.put_u8(input.unit.as_byte())?;
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(input.value.get()).as_bytes())
    }

    /// Serialize an enum as its FIX wire value.
    ///
    /// The value is a `&'static [u8]` held in read-only memory, so this is a
    /// single copy with nothing allocated.
    pub fn serialize_enum<T>(&mut self, value: &T) -> Result<(), SerializeError>
    where
        T: Copy + Into<&'static [u8]>,
    {
        self.put_slice((*value).into())
    }

    /// Serialize enums as a space-delimited list of their FIX wire values.
    ///
    /// An empty slice is [`SerializeError::EmptyValue`] - a FIX field must
    /// carry at least one byte.
    pub fn serialize_enum_collection<T>(&mut self, values: &[T]) -> Result<(), SerializeError>
    where
        T: Copy + Into<&'static [u8]>,
    {
        if values.is_empty() {
            return Err(SerializeError::EmptyValue);
        }
        for value in values {
            self.put_slice((*value).into())?;
            self.put_u8(b' ')?;
        }
        // Drop last space (mirrors Vec::pop's no-op-on-empty behavior).
        self.pos = self.pos.saturating_sub(1);
        Ok(())
    }
}

impl<'a> Write for Serializer<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let end = self.pos + bytes.len();
        if end > self.output.len() {
            return Err(fmt::Error);
        }
        self.output[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }
}
