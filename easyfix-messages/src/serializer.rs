use std::io::Write;

use crate::fields::basic_types::*;

// TODO: This should be parametrizable and also used in parser to cut too big messages.
const MAX_MSG_SIZE: usize = 4096;
const MAX_BODY_LEN_DIGITS: usize = if MAX_MSG_SIZE < 10000 {
    4
} else if MAX_MSG_SIZE < 100000 {
    5
} else if MAX_MSG_SIZE < 1000000 {
    6
} else if MAX_MSG_SIZE < 10000000 {
    7
} else if MAX_MSG_SIZE < 100000000 {
    8
} else {
    panic!("MAX_MSG_SIZE too big");
};

// TODO: SerializeError: Empty Vec/Group, `0` on SeqNum,TagNum,NumInGroup,Length

impl Default for Serializer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Serializer {
    output: Vec<u8>,
    body_start_idx: usize,
}

impl Serializer {
    pub fn new() -> Serializer {
        Serializer {
            output: Vec::with_capacity(MAX_MSG_SIZE),
            body_start_idx: 0,
        }
    }

    pub fn output_mut(&mut self) -> &mut Vec<u8> {
        &mut self.output
    }

    pub fn take(self) -> Vec<u8> {
        self.output
    }

    pub fn serialize_body_len(&mut self) {
        const BODY_LEN_PLACEHOLDER: &[u8] = match MAX_BODY_LEN_DIGITS {
            4 => b"9=0000\x01",
            5 => b"9=00000\x01",
            _ => panic!("unexpected count of maximum body length digits"),
        };
        self.output.extend_from_slice(BODY_LEN_PLACEHOLDER);
        self.body_start_idx = self.output.len();
    }

    // TODO: add test cases for body len and checksum verification
    pub fn serialize_checksum(&mut self) {
        let mut buffer = itoa::Buffer::new();

        let body = &self.output[self.body_start_idx..];
        let body_len = body.len();
        let body_len_slice = buffer.format(body_len).as_bytes();

        self.output[self.body_start_idx - body_len_slice.len() - 1..self.body_start_idx - 1]
            .copy_from_slice(body_len_slice);

        let checksum = self
            .output
            .iter()
            .fold(0u8, |acc, &byte| u8::wrapping_add(acc, byte));

        self.output.extend_from_slice(b"10=");
        if checksum < 10 {
            self.output.extend_from_slice(b"00");
        } else if checksum < 100 {
            self.output.extend_from_slice(b"0");
        }
        self.output
            .extend_from_slice(buffer.format(checksum).as_bytes());
        self.output.push(b'\x01');
    }

    /// Serialize sequence of character digits without commas or decimals.
    /// Value must be positive and may not contain leading zeros.
    pub fn serialize_tag_num(&mut self, tag_num: &TagNum) {
        let mut buffer = itoa::Buffer::new();
        self.output
            .extend_from_slice(buffer.format(*tag_num).as_bytes());
    }

    /// Serialize sequence of character digits without commas or decimals
    /// and optional sign character (characters “-” and “0” – “9” ).
    /// The sign character utilizes one octet (i.e., positive int is “99999”
    /// while negative int is “-99999”).
    pub fn serialize_int(&mut self, int: &Int) {
        let mut buffer = itoa::Buffer::new();
        self.output
            .extend_from_slice(buffer.format(*int).as_bytes());
    }

    /// Serialize sequence of character digits without commas or decimals.
    /// Value must be positive.
    pub fn serialize_seq_num(&mut self, seq_num: &SeqNum) {
        let mut buffer = itoa::Buffer::new();
        self.output
            .extend_from_slice(buffer.format(*seq_num).as_bytes());
    }

    /// Serialize sequence of character digits without commas or decimals.
    /// Value must be positive.
    pub fn serialize_num_in_group(&mut self, num_in_group: &NumInGroup) {
        let mut buffer = itoa::Buffer::new();
        self.output
            .extend_from_slice(buffer.format(*num_in_group).as_bytes());
    }

    /// Serialize sequence of character digits without commas or decimals
    /// (values 1 to 31).
    pub fn serialize_day_of_month(&mut self, day_of_month: DayOfMonth) {
        let mut buffer = itoa::Buffer::new();
        self.output
            .extend_from_slice(buffer.format(day_of_month).as_bytes());
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
    pub fn serialize_float(&mut self, float: &Float) {
        self.output.extend_from_slice(float.to_string().as_bytes())
    }

    pub fn serialize_qty(&mut self, qty: &Qty) {
        self.serialize_float(qty)
    }

    pub fn serialize_price(&mut self, price: &Price) {
        self.serialize_float(price)
    }

    pub fn serialize_price_offset(&mut self, price_offset: &PriceOffset) {
        self.serialize_float(price_offset)
    }

    pub fn serialize_amt(&mut self, amt: &Amt) {
        self.serialize_float(amt)
    }

    pub fn serialize_percentage(&mut self, percentage: &Percentage) {
        self.serialize_float(percentage)
    }

    pub fn serialize_boolean(&mut self, boolean: &Boolean) {
        if *boolean {
            self.output.extend_from_slice(b"Y");
        } else {
            self.output.extend_from_slice(b"N");
        }
    }

    /// Dese any ISO/IEC 8859-1 (Latin-1) character except control characters.
    pub fn serialize_char(&mut self, c: &Char) {
        self.output.push(*c);
    }

    /// Serialize string containing one or more space-delimited single
    /// character values, e.g. “2 A F”.
    pub fn serialize_multiple_char_value(&mut self, mcv: &MultipleCharValue) {
        for c in mcv {
            self.output.push(*c);
            self.output.push(b' ');
        }
        self.output.pop();
    }

    /// Serialize alphanumeric free-format strings can include any character
    /// except control characters.
    pub fn serialize_string(&mut self, input: &FixStr) {
        self.output.extend_from_slice(input.as_bytes());
    }

    /// Serialize string containing one or more space-delimited multiple
    /// character values, e.g. “AV AN A”.
    pub fn serialize_multiple_string_value(&mut self, input: &MultipleStringValue) {
        for s in input {
            self.output.extend_from_slice(s.as_bytes());
            self.output.push(b' ');
        }
        self.output.pop();
    }

    /// Serialize ISO 3166-1:2013 Codes for the representation of names of
    /// countries and their subdivision (2-character code).
    pub fn serialize_country(&mut self, country: &Country) {
        self.output.extend_from_slice(country.to_bytes());
    }

    /// Serialize ISO 4217:2015 Codes for the representation of currencies
    /// and funds (3-character code).
    pub fn serialize_currency(&mut self, currency: &Currency) {
        self.output.extend_from_slice(currency.to_bytes());
    }

    /// Serialize ISO 10383:2012 Securities and related financial instruments
    /// – Codes for exchanges and market identification (MIC)
    /// (4-character code).
    pub fn serialize_exchange(&mut self, exchange: &Exchange) {
        self.output.extend_from_slice(exchange);
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
    pub fn serialize_month_year(&mut self, input: &MonthYear) {
        self.output.extend_from_slice(input);
    }

    /// Serialize ISO 639-1:2002 Codes for the representation of names
    /// of languages (2-character code).
    pub fn serialize_language(&mut self, input: &Language) {
        self.output.extend_from_slice(input);
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
    ///        no fractions of seconds are conveyed (in such a case the period
    ///        is not conveyed), it may include 3 digits to convey
    ///        milliseconds, 6 digits to convey microseconds, 9 digits
    ///        to convey nanoseconds, 12 digits to convey picoseconds;
    pub fn serialize_utc_timestamp(&mut self, input: &UtcTimestamp) {
        write!(self.output, "{}", input.format("%Y%m%d-%H:%M:%S.%f"))
            .expect("UtcTimestamp serialization failed")
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
    ///        no fractions of seconds are conveyed (in such a case the period
    ///        is not conveyed), it may include 3 digits to convey
    ///        milliseconds, 6 digits to convey microseconds, 9 digits
    ///        to convey nanoseconds, 12 digits to convey picoseconds;
    ///        // TODO: set precision!
    pub fn serialize_utc_time_only(&mut self, input: &UtcTimeOnly) {
        write!(self.output, "{}", input.format("%H:%M:%S.%f"))
            .expect("UtcTimeOnly serialization failed")
    }

    /// Serialize date represented in UTC (Universal Time Coordinated)
    /// in YYYYMMDD format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn serialize_utc_date_only(&mut self, input: &UtcDateOnly) {
        write!(self.output, "{}", input.format("%Y%m%d")).expect("UtcDateOnly serialization failed")
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
    pub fn serialize_local_mkt_time(&mut self, input: &LocalMktTime) {
        write!(self.output, "{}", input.format("%H:%M:%S"))
            .expect("LocalMktTime serialization failed")
    }

    /// Serialize date of local market (as opposed to UTC) in YYYYMMDD
    /// format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn serialize_local_mkt_date(&mut self, input: &LocalMktDate) {
        write!(self.output, "{}", input.format("%Y%m%d"))
            .expect("LocalMktDate serialization failed")
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
    ///        no fractions of seconds are conveyed (in such a case the period
    ///        is not conveyed), it may include 3 digits to convey
    ///        milliseconds, 6 digits to convey microseconds, 9 digits
    ///        to convey nanoseconds, 12 digits to convey picoseconds;
    pub fn serialize_tz_timestamp(&mut self, input: &TzTimestamp) {
        self.output.extend_from_slice(input)
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
    pub fn serialize_tz_timeonly(&mut self, input: &TzTimeOnly) {
        self.output.extend_from_slice(input)
    }

    /// Serialize sequence of character digits without commas or decimals.
    pub fn serialize_length(&mut self, length: &Length) {
        let mut buffer = itoa::Buffer::new();
        self.output
            .extend_from_slice(buffer.format(*length).as_bytes());
    }

    /// Serialize raw data with no format or content restrictions,
    /// or a character string encoded as specified by MessageEncoding(347).
    pub fn serialize_data(&mut self, data: &Data) {
        self.output.extend_from_slice(data);
    }

    /// Serialize XML document.
    pub fn serialize_xml(&mut self, xml_data: &XmlData) {
        self.output.extend_from_slice(xml_data);
    }

    // fn serialize_tenor(input: &[u8]) -> Result<Tenor, RejectReason>;

    pub fn serialize_enum<T>(&mut self, value: &T)
    where
        T: Copy + Into<&'static [u8]>,
    {
        self.output.extend_from_slice((*value).into());
    }

    pub fn serialize_enum_collection<T>(&mut self, values: &[T])
    where
        T: Copy + Into<&'static [u8]>,
    {
        for value in values {
            self.output.extend_from_slice((*value).into());
            self.output.push(b' ');
        }
        // Drop last space
        self.output.pop();
    }
}

trait Serialize: Sized {
    fn serialize(serializer: &mut Serializer) -> Result<Self, Box<dyn std::error::Error>>;
}
