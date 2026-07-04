use std::fmt::{self, Write};

use crate::basic_types::{
    Amt, Boolean, Char, Country, Currency, Data, DayOfMonth, Exchange, FixStr, FixedOffset, Float,
    Int, Language, Length, LocalMktDate, LocalMktTime, MonthYear, MultipleCharValue,
    MultipleStringValue, NumInGroup, Percentage, Price, PriceOffset, Qty, SeqNum, TagNum, Tenor,
    TenorUnit, TimePrecision, TzTimeOnly, TzTimestamp, UtcDateOnly, UtcTimeOnly, UtcTimestamp,
    XmlData,
};

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

#[derive(Debug, thiserror::Error)]
pub enum SerializeError {
    #[error("max message size exceeded")]
    MaxMessageSizeExceeded,
    #[error("empty value")]
    EmptyValue,
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

pub struct Serializer<'a> {
    output: &'a mut [u8],
    pos: usize,
    body_start_idx: usize,
    /// Width of the BodyLength(9) placeholder written by
    /// `serialize_body_len`; `0` until then.
    body_len_digits: usize,
}

impl<'a> Serializer<'a> {
    pub fn new(output: &'a mut [u8]) -> Serializer<'a> {
        Serializer {
            output,
            pos: 0,
            body_start_idx: 0,
            body_len_digits: 0,
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn written(&self) -> &[u8] {
        &self.output[..self.pos]
    }

    pub fn put_slice(&mut self, bytes: &[u8]) -> Result<(), SerializeError> {
        if bytes.len() > self.output.len() - self.pos {
            return Err(SerializeError::MaxMessageSizeExceeded);
        }
        let end = self.pos + bytes.len();
        self.output[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }

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
    /// and optional sign character (characters “-” and “0” – “9” ).
    /// The sign character utilizes one octet (i.e., positive int is “99999”
    /// while negative int is “-99999”).
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
    /// and sign character (characters “-”, “0” – “9” and “.”);
    /// the absence of the decimal point within the string will be interpreted
    /// as the float representation of an integer value. Note that float values
    /// may contain leading zeros (e.g. “00023.23” = “23.23”) and may contain
    /// or omit trailing zeros after the decimal point
    /// (e.g. “23.0” = “23.0000” = “23” = “23.”).
    ///
    /// All float fields must accommodate up to fifteen significant digits.
    /// The number of decimal places used should be a factor of business/market
    /// needs and mutual agreement between counterparties.
    pub fn serialize_float(&mut self, float: &Float) -> Result<(), SerializeError> {
        self.put_slice(float.to_string().as_bytes())
    }

    pub fn serialize_qty(&mut self, qty: &Qty) -> Result<(), SerializeError> {
        self.serialize_float(qty)
    }

    pub fn serialize_price(&mut self, price: &Price) -> Result<(), SerializeError> {
        self.serialize_float(price)
    }

    pub fn serialize_price_offset(
        &mut self,
        price_offset: &PriceOffset,
    ) -> Result<(), SerializeError> {
        self.serialize_float(price_offset)
    }

    pub fn serialize_amt(&mut self, amt: &Amt) -> Result<(), SerializeError> {
        self.serialize_float(amt)
    }

    pub fn serialize_percentage(&mut self, percentage: &Percentage) -> Result<(), SerializeError> {
        self.serialize_float(percentage)
    }

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
    /// character values, e.g. “2 A F”.
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
    /// character values, e.g. “AV AN A”.
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

    /// Serialize ISO 10383:2012 Securities and related financial instruments
    /// – Codes for exchanges and market identification (MIC)
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
    ///   // TODO: set precision!
    pub fn serialize_utc_time_only(&mut self, input: &UtcTimeOnly) -> Result<(), SerializeError> {
        write!(self, "{}", input.format("%H:%M:%S.%f"))
            .map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize date represented in UTC (Universal Time Coordinated)
    /// in YYYYMMDD format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn serialize_utc_date_only(&mut self, input: &UtcDateOnly) -> Result<(), SerializeError> {
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
        write!(self, "{}", input.format("%Y%m%d"))
            .map_err(|_| SerializeError::MaxMessageSizeExceeded)
    }

    /// Serialize string representing a time/date combination representing
    /// local time with an offset to UTC to allow identification of local time
    /// and time zone offset of that time.
    ///
    /// The representation is based on ISO 8601.
    ///
    /// Format is `YYYYMMDD-HH:MM:SS.sss*[Z | [ + | – hh[:mm]]]` where:
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
        let ts = input.timestamp();
        let fmt = match input.precision() {
            TimePrecision::Secs => "%Y%m%d-%H:%M:%S",
            TimePrecision::Millis => "%Y%m%d-%H:%M:%S%.3f",
            TimePrecision::Micros => "%Y%m%d-%H:%M:%S%.6f",
            TimePrecision::Nanos => "%Y%m%d-%H:%M:%S%.9f",
        };
        write!(self, "{}", ts.format(fmt)).map_err(|_| SerializeError::MaxMessageSizeExceeded)?;
        self.serialize_tz_offset(ts.offset())
    }

    /// Serialize time of day with timezone. Time represented based on
    /// ISO 8601. This is the time with a UTC offset to allow identification of
    /// local time and time zone of that time.
    ///
    /// Format is `HH:MM[:SS][Z | [ + | – hh[:mm]]]` where:
    /// - HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds,
    /// - hh = 01-12 offset hours,
    /// - mm = 00-59 offset minutes.
    pub fn serialize_tz_timeonly(&mut self, input: &TzTimeOnly) -> Result<(), SerializeError> {
        let fmt = match input.precision() {
            TimePrecision::Secs => "%H:%M:%S",
            TimePrecision::Millis => "%H:%M:%S%.3f",
            TimePrecision::Micros => "%H:%M:%S%.6f",
            TimePrecision::Nanos => "%H:%M:%S%.9f",
        };
        write!(self, "{}", input.timestamp().format(fmt))
            .map_err(|_| SerializeError::MaxMessageSizeExceeded)?;
        self.serialize_tz_offset(&input.offset())
    }

    fn serialize_tz_offset(&mut self, offset: &FixedOffset) -> Result<(), SerializeError> {
        let total_secs = offset.local_minus_utc();
        if total_secs == 0 {
            return self.put_u8(b'Z');
        }
        let sign = if total_secs < 0 { b'-' } else { b'+' };
        let abs_secs = total_secs.unsigned_abs();
        let hours = abs_secs / 3600;
        let minutes = (abs_secs % 3600) / 60;
        self.put_u8(sign)?;
        write!(self, "{hours:02}").map_err(|_| SerializeError::MaxMessageSizeExceeded)?;
        if minutes != 0 {
            write!(self, ":{minutes:02}").map_err(|_| SerializeError::MaxMessageSizeExceeded)?;
        }
        Ok(())
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

    pub fn serialize_tenor(&mut self, input: &Tenor) -> Result<(), SerializeError> {
        let unit_byte = match input.unit {
            TenorUnit::Days => b'D',
            TenorUnit::Months => b'M',
            TenorUnit::Weeks => b'W',
            TenorUnit::Years => b'Y',
        };
        self.put_u8(unit_byte)?;
        let mut buffer = itoa::Buffer::new();
        self.put_slice(buffer.format(input.value).as_bytes())
    }

    pub fn serialize_enum<T>(&mut self, value: &T) -> Result<(), SerializeError>
    where
        T: Copy + Into<&'static [u8]>,
    {
        self.put_slice((*value).into())
    }

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

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::{SerializeError, Serializer, max_body_len_digits};

    const BEGIN_STRING: &[u8] = b"8=FIXT.1.1\x01";

    /// Frame `body` as a complete FIX message and return the byte count.
    ///
    /// Mirrors what a generated `FixtMessage::serialize` does: BeginString,
    /// BodyLength placeholder, body, CheckSum.
    fn frame(buf: &mut [u8], body: &[u8]) -> usize {
        let mut serializer = Serializer::new(buf);
        serializer.put_slice(BEGIN_STRING).unwrap();
        serializer.serialize_body_len().unwrap();
        serializer.put_slice(body).unwrap();
        serializer.serialize_checksum().unwrap();
        serializer.pos()
    }

    /// Independent checksum oracle: plain u32 sum reduced mod 256, rather
    /// than the `u8::wrapping_add` fold the serializer uses.
    fn expected_checksum(bytes: &[u8]) -> u8 {
        (bytes.iter().map(|&b| b as u32).sum::<u32>() % 256) as u8
    }

    /// Split a framed message into (everything before `10=`, the CheckSum
    /// field). The CheckSum field is always exactly `10=NNN\x01`.
    fn split_checksum(msg: &[u8]) -> (&[u8], &[u8]) {
        msg.split_at(msg.len() - 7)
    }

    #[test]
    fn body_len_counts_bytes_between_body_length_and_checksum() {
        let body = b"35=0\x0134=1\x0149=SENDER\x0156=TARGET\x01";
        assert_eq!(body.len(), 30);

        let mut buf = [0u8; 4096];
        let len = frame(&mut buf, body);
        let msg = &buf[..len];

        // A 4096-byte buffer yields a 4-digit placeholder, so the BodyLength
        // field occupies 7 bytes ("9=0000\x01") right after BeginString.
        assert_eq!(max_body_len_digits(buf.len()), 4);
        assert_eq!(
            &msg[BEGIN_STRING.len()..BEGIN_STRING.len() + 7],
            b"9=0030\x01"
        );
    }

    #[test]
    fn body_len_is_patched_right_aligned_keeping_leading_zeros() {
        // Body length 5 into a 4-digit placeholder must become "0005",
        // never "5000" or "5\0\0\0".
        let body = b"35=0\x01";
        assert_eq!(body.len(), 5);

        let mut buf = [0u8; 4096];
        let len = frame(&mut buf, body);
        let msg = &buf[..len];

        assert_eq!(
            &msg[BEGIN_STRING.len()..BEGIN_STRING.len() + 7],
            b"9=0005\x01"
        );
    }

    #[test]
    fn body_len_placeholder_width_follows_buffer_size() {
        // The placeholder is sized from the buffer, not from the payload.
        for (buf_len, digits) in [(64usize, 2usize), (500, 3), (4096, 4), (65536, 5)] {
            let mut buf = vec![0u8; buf_len];
            let len = frame(&mut buf, b"35=0\x01");
            let msg = &buf[..len];

            assert_eq!(max_body_len_digits(buf_len), digits);
            let body_len_field = &msg[BEGIN_STRING.len()..BEGIN_STRING.len() + digits + 3];
            let expected = format!("9={:0width$}\x01", 5, width = digits);
            assert_eq!(
                body_len_field,
                expected.as_bytes(),
                "buffer of {buf_len} bytes"
            );
        }
    }

    #[test]
    fn body_len_digits_cap_at_8_for_huge_buffers() {
        // Buffers of 100 MB and beyond used to panic; now they clamp to the
        // widest placeholder.
        assert_eq!(max_body_len_digits(100_000_000), 8);
        assert_eq!(max_body_len_digits(usize::MAX), 8);
    }

    #[test]
    fn checksum_without_body_len_placeholder_errors_instead_of_panicking() {
        // `serialize_checksum` with no preceding `serialize_body_len` has
        // no placeholder to patch - it must fail, not index out of bounds.
        let mut buf = [0u8; 64];
        let mut serializer = Serializer::new(&mut buf);
        serializer.put_slice(b"8=FIXT.1.1\x0135=0\x01").unwrap();
        assert_matches!(
            serializer.serialize_checksum(),
            Err(SerializeError::MaxMessageSizeExceeded)
        );
    }

    #[test]
    fn checksum_covers_every_byte_before_the_checksum_field() {
        let body = b"35=0\x0134=1\x0149=SENDER\x0156=TARGET\x01";
        let mut buf = [0u8; 4096];
        let len = frame(&mut buf, body);
        let (covered, checksum_field) = split_checksum(&buf[..len]);

        let expected = format!("10={:03}\x01", expected_checksum(covered));
        assert_eq!(checksum_field, expected.as_bytes());
    }

    #[test]
    fn checksum_is_computed_after_body_len_is_patched() {
        // The placeholder digits differ from the patched ones, so a checksum
        // taken before patching would disagree with a recompute over the
        // final bytes.
        let body = b"35=0\x0134=1\x0149=SENDER\x0156=TARGET\x01";
        let mut buf = [0u8; 4096];
        let len = frame(&mut buf, body);
        let msg = &buf[..len];
        let (covered, _) = split_checksum(msg);

        // "9=0030" must already be in the covered range - not "9=0000".
        assert!(covered.windows(6).any(|w| w == b"9=0030"));
    }

    #[test]
    fn checksum_is_always_zero_padded_to_three_digits() {
        // Sweep filler bytes and lengths so all three padding branches
        // (< 10, < 100, >= 100) are exercised.
        let mut saw_single_digit = false;
        let mut saw_two_digits = false;
        let mut saw_three_digits = false;

        for filler in 0x20u8..=0x7e {
            for pad in 0..8usize {
                let mut body = Vec::from(&b"35=0\x0158="[..]);
                body.resize(body.len() + pad, filler);
                body.push(b'\x01');

                let mut buf = [0u8; 4096];
                let len = frame(&mut buf, &body);
                let (covered, checksum_field) = split_checksum(&buf[..len]);

                let checksum = expected_checksum(covered);
                match checksum {
                    0..=9 => saw_single_digit = true,
                    10..=99 => saw_two_digits = true,
                    _ => saw_three_digits = true,
                }

                let expected = format!("10={checksum:03}\x01");
                assert_eq!(
                    checksum_field,
                    expected.as_bytes(),
                    "filler {filler:#04x}, pad {pad}"
                );
            }
        }

        assert!(saw_single_digit, "no checksum below 10 was exercised");
        assert!(saw_two_digits, "no checksum in 10..100 was exercised");
        assert!(saw_three_digits, "no checksum >= 100 was exercised");
    }

    #[test]
    fn serialize_exchange_rejects_bytes_outside_printable_ascii() {
        let mut buf = [0u8; 16];
        let mut serializer = Serializer::new(&mut buf);
        assert_matches!(
            serializer.serialize_exchange(b"XN\x01S"),
            Err(SerializeError::InvalidValue)
        );
        assert_matches!(
            serializer.serialize_exchange(&[b'X', b'N', 0x80, b'S']),
            Err(SerializeError::InvalidValue)
        );
    }

    #[test]
    fn serialize_exchange_writes_printable_ascii() {
        let mut buf = [0u8; 16];
        let mut serializer = Serializer::new(&mut buf);
        serializer.serialize_exchange(b"XNAS").unwrap();
        assert_eq!(serializer.written(), b"XNAS");
    }

    #[test]
    fn serialize_language_rejects_bytes_outside_printable_ascii() {
        let mut buf = [0u8; 16];
        let mut serializer = Serializer::new(&mut buf);
        assert_matches!(
            serializer.serialize_language(b"p\x01"),
            Err(SerializeError::InvalidValue)
        );
        assert_matches!(
            serializer.serialize_language(&[b'p', 0xff]),
            Err(SerializeError::InvalidValue)
        );
    }

    #[test]
    fn serialize_language_writes_printable_ascii() {
        let mut buf = [0u8; 16];
        let mut serializer = Serializer::new(&mut buf);
        serializer.serialize_language(b"pl").unwrap();
        assert_eq!(serializer.written(), b"pl");
    }

    #[test]
    fn framed_message_matches_a_hand_computed_reference() {
        // Full regression anchor: exact bytes, including both patched fields.
        let body = b"35=0\x0134=1\x01";
        let mut buf = [0u8; 4096];
        let len = frame(&mut buf, body);

        let head = b"8=FIXT.1.1\x019=0010\x0135=0\x0134=1\x01";
        let expected = format!(
            "{}10={:03}\x01",
            std::str::from_utf8(head).unwrap(),
            expected_checksum(head)
        );
        assert_eq!(&buf[..len], expected.as_bytes());
    }
}
