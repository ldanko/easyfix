use crate::types::basic_types::*;

pub struct Serializer {
    output: Vec<u8>,
}

impl Serializer {
    pub fn new() -> Serializer {
        Serializer { output: Vec::new() }
    }

    pub fn output_mut(&mut self) -> &mut Vec<u8> {
        &mut self.output
    }

    pub fn take(self) -> Vec<u8> {
        self.output
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
    pub fn serialize_string(&mut self, input: &Str) {
        self.output.extend_from_slice(&input);
    }

    /// Serialize string containing one or more space-delimited multiple
    /// character values, e.g. “AV AN A”.
    pub fn serialize_multiple_string_value(&mut self, input: &MultipleStringValue) {
        for s in input {
            self.output.extend_from_slice(s);
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
        self.output.extend_from_slice(input)
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
        self.output.extend_from_slice(input)
    }

    /// Serialize date represented in UTC (Universal Time Coordinated)
    /// in YYYYMMDD format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn serialize_utc_date_only(&mut self, input: &UtcDateOnly) {
        let (y, m, d) = *input;

        let mut buffer = itoa::Buffer::new();
        self.output.extend_from_slice(buffer.format(y).as_bytes());
        let mut buffer = itoa::Buffer::new();
        self.output.extend_from_slice(buffer.format(m).as_bytes());
        let mut buffer = itoa::Buffer::new();
        self.output.extend_from_slice(buffer.format(d).as_bytes());
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
        let mut buffer = itoa::Buffer::new();
        self.output
            .extend_from_slice(buffer.format(*input).as_bytes());
    }

    /// Serialize date of local market (as opposed to UTC) in YYYYMMDD
    /// format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn serialize_local_mkt_date(&mut self, input: &LocalMktDate) {
        let (y, m, d) = *input;

        let mut buffer = itoa::Buffer::new();
        self.output.extend_from_slice(buffer.format(y).as_bytes());
        let mut buffer = itoa::Buffer::new();
        self.output.extend_from_slice(buffer.format(m).as_bytes());
        let mut buffer = itoa::Buffer::new();
        self.output.extend_from_slice(buffer.format(d).as_bytes());
    }

    /// Serialize string representing a time/date combination representing
    /// local time with an offset to UTC to allow identification of local time
    /// and time zone offset of that time.
    ///
    /// The representation is based on ISO 8601.
    ///
    /// Format is YYYYMMDD-HH:MM:SS.sss*[Z | [ + | – hh[:mm]]] where:
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
    /// Format is HH:MM[:SS][Z | [ + | – hh[:mm]]] where:
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
        self.output.extend_from_slice(&data);
    }

    /// Serialize XML document.
    pub fn serialize_xml(&mut self, xml_data: &XmlData) {
        self.output.extend_from_slice(&xml_data);
    }

    // fn serialize_tenor(input: &[u8]) -> Result<Tenor, RejectReason>;

    pub fn serialize_enum<T>(&mut self, value: &T)
    where
        T: Copy + Into<&'static [u8]>
    {
        self.output.extend_from_slice(value.clone().into());
    }

    pub fn serialize_enum_collection<T>(&mut self, values: &[T])
    where
        T: Copy + Into<&'static [u8]>
    {
        for value in values {
            self.output.extend_from_slice(value.clone().into());
            self.output.push(b' ');
        }
        // Drop last space
        self.output.pop();
    }
}

trait Serialize: Sized {
    fn serialize(serializer: &mut Serializer) -> Result<Self, Box<dyn std::error::Error>>;
}
