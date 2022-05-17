use anyhow::Result;

use crate::{
    parser::RawMessage,
    types::{basic_types::*, DeserializeError, RejectReason},
};

#[derive(Debug)]
pub struct Deserializer<'de> {
    raw_message: RawMessage<'de>,
    buf: &'de [u8],
    msg_type: Vec<u8>,
    seq_num: Option<SeqNum>,
    current_tag: Option<TagNum>,
    // Used to put tag back to deserializer, when switching to deserialization
    // another message section.
    tmp_tag: Option<TagNum>,
}

impl<'de> Deserializer<'de> {
    pub fn from_raw_message(raw_message: RawMessage) -> Deserializer {
        let buf = raw_message.body;
        Deserializer {
            raw_message,
            buf,
            msg_type: Vec::new(),
            seq_num: None,
            current_tag: None,
            tmp_tag: None,
        }
    }

    pub fn begin_string(&self) -> Str {
        self.raw_message.begin_string.into()
    }

    pub fn body_length(&self) -> Length {
        self.raw_message.body.len() as Length
    }

    pub fn check_sum(&self) -> Str {
        format!("{:03}", self.raw_message.checksum).into_bytes()
    }

    pub fn set_seq_num(&mut self, seq_num: SeqNum) {
        self.seq_num = Some(seq_num);
    }

    pub fn reject(&mut self, tag: Option<TagNum>, reason: RejectReason) -> DeserializeError {
        if let Some(seq_num) = self.seq_num {
            DeserializeError::Reject {
                msg_type: self.msg_type.clone(),
                seq_num,
                tag,
                reason,
            }
        } else {
            DeserializeError::Logout
        }
    }

    pub fn put_tag(&mut self, tag: TagNum) {
        self.tmp_tag = Some(tag);
    }

    /// Deserialize sequence of character digits without commas or decimals.
    /// Value must be positive and may not contain leading zeros.
    pub fn deserialize_tag_num(&mut self) -> Result<Option<TagNum>, DeserializeError> {
        if self.tmp_tag.is_some() {
            return Ok(self.tmp_tag.take());
        }

        match self.buf {
            // End of stream
            [] => return Ok(None),
            // Leading zero
            [b'0' | b'=', ..] => return Err(self.reject(None, RejectReason::InvalidTagNumber)),
            _ => {}
        }

        let mut value: TagNum = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and self.bug.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    value = value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as TagNum))
                        // Integer overflow
                        .ok_or_else(|| self.reject(None, RejectReason::InvalidTagNumber))?;
                }
                b'=' => {
                    if value == 0 {
                        return Err(self.reject(self.current_tag, RejectReason::InvalidTagNumber));
                    } else {
                        self.current_tag = Some(value);
                        self.buf = &self.buf[i + 1..];
                        return Ok(Some(value));
                    }
                }
                // Unexpected value
                _ => return Err(self.reject(None, RejectReason::InvalidTagNumber)),
            }
        }

        // End of stream
        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize sequence of character digits without commas or decimals
    /// and optional sign character (characters “-” and “0” – “9” ).
    /// The sign character utilizes one octet (i.e., positive int is “99999”
    /// while negative int is “-99999”).
    ///
    /// Note that int values may contain leading zeros (e.g. “00023” = “23”).
    pub fn deserialize_int(&mut self) -> Result<Int, DeserializeError> {
        let negative = match self.buf {
            // MSG Garbled
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [b'-', b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue))
            }
            [b'-', buf @ ..] => {
                self.buf = buf;
                true
            }
            _ => false,
        };

        let mut value: Int = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and self.buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    value = value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as Int))
                        .ok_or_else(|| {
                            self.reject(self.current_tag, RejectReason::ValueIsIncorrect)
                        })?;
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    return Ok(if negative { -value } else { value });
                }
                _ => {
                    return Err(
                        self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue)
                    )
                }
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize sequence of character digits without commas or decimals.
    /// Value must be positive.
    pub fn deserialize_seq_num(&mut self) -> Result<SeqNum, DeserializeError> {
        match self.buf {
            // No more data, MSG Garbled
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            _ => {}
        }

        let mut value: SeqNum = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    value = value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as SeqNum))
                        .ok_or_else(|| {
                            self.reject(self.current_tag, RejectReason::ValueIsIncorrect)
                        })?;
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    if value == 0 {
                        return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                    } else {
                        return Ok(value);
                    }
                }
                _ => {
                    return Err(
                        self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)
                    )
                }
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize sequence of character digits without commas or decimals.
    /// Value must be positive.
    pub fn deserialize_num_in_group(&mut self) -> Result<NumInGroup, DeserializeError> {
        match self.buf {
            // MSG Garbled
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            _ => {}
        }

        let mut value: NumInGroup = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    value = value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as NumInGroup))
                        .ok_or(self.reject(self.current_tag, RejectReason::ValueIsIncorrect))?;
                }
                b'\x01' => {
                    if value == 0 {
                        return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                    } else {
                        self.buf = &self.buf[i + 1..];
                        return Ok(value);
                    }
                }
                _ => {
                    return Err(
                        self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)
                    )
                }
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize sequence of character digits without commas or decimals
    /// (values 1 to 31).
    pub fn deserialize_day_of_month(&mut self) -> Result<DayOfMonth, DeserializeError> {
        match self.buf {
            // MSG Garbled
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [b'0', ..] => return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect)),
            _ => {}
        };

        let mut value: NumInGroup = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    value = value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as NumInGroup))
                        .ok_or(self.reject(self.current_tag, RejectReason::ValueIsIncorrect))?;
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    break;
                }
                _ => {
                    return Err(
                        self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)
                    )
                }
            }
        }

        match value {
            1..=31 => Ok(value),
            _ => Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect)),
        }
    }

    /// Deserialize sequence of character digits with optional decimal point
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
    pub fn deserialize_float(&mut self) -> Result<Float, DeserializeError> {
        let (negative, buf) = match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [b'-', b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue))
            }
            [b'-', buf @ ..] => (true, buf),
            _ => (false, self.buf),
        };

        let mut num: i64 = 0;
        let mut scale = None;
        for i in 0..buf.len() {
            if let Some(scale) = scale.as_mut() {
                *scale += 1;
            }
            // SAFETY: i is between 0 and buf.len()
            match unsafe { buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    num = num
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as i64))
                        .ok_or(self.reject(self.current_tag, RejectReason::ValueIsIncorrect))?;
                }
                b'.' => {
                    if scale.is_some() {
                        return Err(self
                            .reject(self.current_tag, RejectReason::IncorrectDataFormatForValue));
                    }
                    scale = Some(0);
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    let scale = scale.unwrap_or(0);
                    // TODO: Limit scale (28 or more panics!)
                    return Ok(Decimal::new(if negative { -num } else { num }, scale));
                }
                _ => {
                    return Err(
                        self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)
                    )
                }
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    pub fn deserialize_qty(&mut self) -> Result<Qty, DeserializeError> {
        self.deserialize_float()
    }

    pub fn deserialize_price(&mut self) -> Result<Price, DeserializeError> {
        self.deserialize_float()
    }

    pub fn deserialize_price_offset(&mut self) -> Result<PriceOffset, DeserializeError> {
        self.deserialize_float()
    }

    pub fn deserialize_amt(&mut self) -> Result<Amt, DeserializeError> {
        self.deserialize_float()
    }

    pub fn deserialize_percentage(&mut self) -> Result<Percentage, DeserializeError> {
        self.deserialize_float()
    }

    pub fn deserialize_boolean(&mut self) -> Result<Boolean, DeserializeError> {
        match self.buf {
            // Empty or missing separator at the end
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'Y'] | [b'N'] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [b'Y', b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok(true)
            }
            [b'N', b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok(false)
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Dese any ISO/IEC 8859-1 (Latin-1) character except control characters.
    pub fn deserialize_char(&mut self) -> Result<Char, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            // ASCII controll characters range
            [0..=31, ..] => Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect)),
            [n, b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok(*n)
            }
            // Missing separator at the end
            [_, byte] if *byte != b'\x01' => Err(DeserializeError::GarbledMessage(
                "missing tag spearator".into(),
            )),
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize string containing one or more space-delimited single
    /// character values, e.g. “2 A F”.
    pub fn deserialize_multiple_char_value(
        &mut self,
    ) -> Result<MultipleCharValue, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue));
            }
            _ => {}
        }

        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and input.len()
            if let b'\x01' = unsafe { self.buf.get_unchecked(i) } {
                let data = &self.buf[0..i];
                // Skip data and separator
                self.buf = &self.buf[i + 1..];
                let mut result = MultipleCharValue::with_capacity(data.len() / 2 + 1);
                for chunk in data.chunks(2) {
                    match chunk {
                        [b' '] | [b' ', _] => {
                            return Err(self.reject(
                                self.current_tag,
                                RejectReason::IncorrectDataFormatForValue,
                            ))
                        }
                        // Latin-1 controll characters ranges
                        [0x00..=0x1f] | [0x80..=0x9f] | [0x00..=0x1f, _] | [0x80..=0x9f, _] => {
                            return Err(
                                self.reject(self.current_tag, RejectReason::ValueIsIncorrect)
                            );
                        }
                        [n] | [n, b' '] => result.push(*n),
                        _ => {
                            return Err(self.reject(
                                self.current_tag,
                                RejectReason::IncorrectDataFormatForValue,
                            ));
                        }
                    }
                }
                return Ok(result);
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize alphanumeric free-format strings can include any character
    /// except control characters.
    pub fn deserialize_string(&mut self) -> Result<Str, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue));
            }
            _ => {}
        }

        const DEFAULT_CAPACITY: usize = 16;
        let mut result = Str::with_capacity(DEFAULT_CAPACITY);
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and input.len()
            match unsafe { self.buf.get_unchecked(i) } {
                // No control character is allowed
                0x00 | 0x02..=0x1f | 0x80..=0x9f => {
                    println!("Wrong byte {} at pos {}", self.buf.get(i).unwrap(), i);
                    return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                }
                // Except SOH which marks end of tag
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    return Ok(result);
                }
                byte => result.push(*byte),
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize string containing one or more space-delimited multiple
    /// character values, e.g. “AV AN A”.
    pub fn deserialize_multiple_string_value(
        &mut self,
    ) -> Result<MultipleStringValue, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue));
            }
            _ => {}
        }

        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and input.len()
            if let b'\x01' = unsafe { self.buf.get_unchecked(i) } {
                let data = &self.buf[0..i];
                // Skip data and separator
                self.buf = &self.buf[i + 1..];
                const DEFAULT_CAPACITY: usize = 4;
                let mut result = MultipleStringValue::with_capacity(DEFAULT_CAPACITY);
                for part in data.split(|p| *p == b' ') {
                    let mut sub_result = Str::with_capacity(part.len());
                    for byte in part {
                        match byte {
                            // ASCII controll characters range
                            0..=31 => {
                                return Err(
                                    self.reject(self.current_tag, RejectReason::ValueIsIncorrect)
                                );
                            }
                            n => sub_result.push(*n),
                        }
                    }
                    result.push(sub_result);
                }
                return Ok(result);
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize ISO 3166-1:2013 Codes for the representation of names of
    /// countries and their subdivision (2-character code).
    pub fn deserialize_country(&mut self) -> Result<Country, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [_, b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue))
            }
            bytes @ [_, _, b'\x01', buf @ ..] => {
                self.buf = buf;
                Country::from_bytes(&bytes[0..2])
                    .ok_or(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue))
            }
            // TODO: add the same for [a, b] and [a] cases
            // TODO: and do it in every deserialize_* function without loop
            // TODO: or maybe better just check if len < expected message size
            &[a, b, c] if a != b'\x01' && b != b'\x01' && c != b'\x01' => {
                Err(DeserializeError::GarbledMessage(format!(
                    "missing tag ({:?}) separator",
                    self.current_tag
                )))
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize ISO 4217:2015 Codes for the representation of currencies
    /// and funds (3-character code).
    pub fn deserialize_currency(&mut self) -> Result<Currency, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            bytes @ [_, _, _, b'\x01', buf @ ..] => {
                self.buf = buf;
                Currency::from_bytes(&bytes[0..3])
                    .ok_or(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue))
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize ISO 10383:2012 Securities and related financial instruments
    /// – Codes for exchanges and market identification (MIC)
    /// (4-character code).
    pub fn deserialize_exchange(&mut self) -> Result<Exchange, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [a, b, c, d, b'\x01', buf @ ..] => {
                self.buf = buf;
                // TODO
                Ok([*a, *b, *c, *d])
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize string representing month of a year.
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
    pub fn deserialize_month_year(&mut self) -> Result<MonthYear, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [a, b, c, d, e, f, g, h, b'\x01', buf @ ..] => {
                self.buf = buf;
                // TODO
                Ok([*a, *b, *c, *d, *e, *f, *g, *h].into())
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize ISO 639-1:2002 Codes for the representation of names
    /// of languages (2-character code).
    pub fn deserialize_language(&mut self) -> Result<Language, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            [a, b, b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok([*a, *b])
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

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
    /// - MM = 0059,
    /// - SS = 00-60 (60 only if UTC leap second),
    /// - sss* fractions of seconds. The fractions of seconds may be empty when
    ///        no fractions of seconds are conveyed (in such a case the period
    ///        is not conveyed), it may include 3 digits to convey
    ///        milliseconds, 6 digits to convey microseconds, 9 digits
    ///        to convey nanoseconds, 12 digits to convey picoseconds;
    ///        // TODO: set precision!
    pub fn deserialize_utc_timestamp(&mut self) -> Result<UtcTimestamp, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01'] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            _ => {}
        }
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            if let b'\x01' = unsafe { self.buf.get_unchecked(i) } {
                // -1 to drop separator, separator on idx 0 is checked separately
                let data = &self.buf[0..i];
                self.buf = &self.buf[i + 1..];
                // TODO
                return Ok(data.into());
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize string representing time-only represented in UTC
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
    pub fn deserialize_utc_time_only(&mut self) -> Result<UtcTimeOnly, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            _ => {}
        }
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            if let b'\x01' = unsafe { self.buf.get_unchecked(i) } {
                // -1 to drop separator, separator on idx 0 is checked separately
                let data = &self.buf[0..i - 1];
                self.buf = &self.buf[i + 1..];
                // TODO
                return Ok(data.into());
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize date represented in UTC (Universal Time Coordinated)
    /// in YYYYMMDD format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn deserialize_utc_date_only(&mut self) -> Result<UtcDateOnly, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue)),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [
                // Year
                y3 @ b'0'..=b'9', y2 @ b'0'..=b'9', y1 @ b'0'..=b'9', y0 @ b'0'..=b'9',
                // Month
                m1 @ b'0'..=b'1', m0 @ b'0'..=b'9',
                // Day
                d1 @ b'0'..=b'3', d0 @ b'0'..=b'9',
                // Separator
                b'\x01',
            ] => {
                let year = (y3 - b'0') as u16 * 1000
                    + (y2 - b'0') as u16 * 100
                    + (y1 - b'0') as u16 * 10
                    + (y0 - b'0') as u16;
                let month = (m1 - b'0') * 10 + (m0 - b'0');
                if !(1..31).contains(&month) {
                    return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                }
                let day = (d1 - b'0') * 10 + (d0 - b'0');
                if !(1..12).contains(&day) {
                    return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                }
                self.buf = &self.buf[8..];
                Ok((year, month, day))
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize time local to a market center. Used where offset to UTC
    /// varies throughout the year and the defining market center is identified
    /// in a corresponding field.
    ///
    /// Format is HH:MM:SS where:
    /// - HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds.
    ///
    /// In general only the hour token is non-zero.
    pub fn deserialize_local_mkt_time(&mut self) -> Result<LocalMktTime, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [_h1, _h0, b':', _m1, _m0, b':', _s1, _s0] => {
                self.buf = &self.buf[8..];
                // TODO
                Ok(0)
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize date of local market (as opposed to UTC) in YYYYMMDD
    /// format.
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999,
    /// - MM = 01-12,
    /// - DD = 01-31.
    pub fn deserialize_local_mkt_date(&mut self) -> Result<LocalMktDate, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue)),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [
                // Year
                y3 @ b'0'..=b'9', y2 @ b'0'..=b'9', y1 @ b'0'..=b'9', y0 @ b'0'..=b'9',
                // Month
                m1 @ b'0'..=b'1', m0 @ b'0'..=b'9',
                // Day
                d1 @ b'0'..=b'3', d0 @ b'0'..=b'9',
                // Separator
                b'\x01',
            ] => {
                let year = (y3 - b'0') as u16 * 1000
                    + (y2 - b'0') as u16 * 100
                    + (y1 - b'0') as u16 * 10
                    + (y0 - b'0') as u16;
                let month = (m1 - b'0') * 10 + (m0 - b'0');
                if !(1..31).contains(&month) {
                    return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                }
                let day = (d1 - b'0') * 10 + (d0 - b'0');
                if !(1..12).contains(&day) {
                    return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                }
                self.buf = &self.buf[8..];
                Ok((year, month, day))
            }
            _ => Err(self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)),
        }
    }

    /// Deserialize string representing a time/date combination representing
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
    pub fn deserialize_tz_timestamp(&mut self) -> Result<TzTimestamp, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            // Missing separator at the end
            [_] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "missing tag ({:?}) separator",
                    self.current_tag
                )))
            }
            _ => {}
        }
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            if let b'\x01' = unsafe { self.buf.get_unchecked(i) } {
                // -1 to drop separator, separator on idx 0 is checked separately
                let data = &self.buf[0..i - 1];
                self.buf = &self.buf[i + 1..];
                // TODO
                return Ok(data.into());
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize time of day with timezone. Time represented based on
    /// ISO 8601. This is the time with a UTC offset to allow identification of
    /// local time and time zone of that time.
    ///
    /// Format is HH:MM[:SS][Z | [ + | – hh[:mm]]] where:
    /// - HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds,
    /// - hh = 01-12 offset hours,
    /// - mm = 00-59 offset minutes.
    pub fn deserialize_tz_timeonly(&mut self) -> Result<TzTimeOnly, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            &[b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            // Missing separator at the end
            &[_] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "missing tag ({:?}) separator",
                    self.current_tag
                )))
            }
            _ => {}
        }
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            if let b'\x01' = unsafe { self.buf.get_unchecked(i) } {
                // -1 to drop separator, separator on idx 0 is checked separately
                let data = &self.buf[0..i - 1];
                self.buf = &self.buf[i + 1..];
                return Ok(data.into());
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize sequence of character digits without commas or decimals.
    /// Value must be positive. Fields of datatype Length are referred to as
    /// Length fields.
    ///
    /// The Length field must be associated with a field of datatype data.
    ///
    /// The Length field must specify the number of octets of the value
    /// contained in the associated data field up to but not including
    /// the terminating <SOH>.
    pub fn deserialize_length(&mut self) -> Result<Length, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue));
            }
            // Leading zero
            [b'0', n, ..] if *n != b'\x01' => {
                return Err(
                    self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)
                );
            }
            _ => {}
        }

        let mut value: Length = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    value = value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as Length))
                        .ok_or_else(|| {
                            self.reject(self.current_tag, RejectReason::ValueIsIncorrect)
                        })?;
                }
                b'\x01' => {
                    if value == 0 {
                        return Err(self.reject(self.current_tag, RejectReason::ValueIsIncorrect));
                    } else {
                        self.buf = &self.buf[i + 1..];
                        return Ok(value);
                    }
                }
                _ => {
                    return Err(
                        self.reject(self.current_tag, RejectReason::IncorrectDataFormatForValue)
                    )
                }
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    /// Deserialize raw data with no format or content restrictions,
    /// or a character string encoded as specified by MessageEncoding(347).
    /// Fields of datatype data must have an associated field of type Length.
    /// Fields of datatype data must be immediately preceded by their
    /// associated Length field.
    pub fn deserialize_data(&mut self, len: usize) -> Result<Data, DeserializeError> {
        if self.buf.is_empty() {
            return Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            )));
        }

        // Data length + separator (SOH)
        if self.buf.len() < len + 1 {
            return Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            )));
        }

        // SAFETY: length checked above
        if let b'\x01' = unsafe { *self.buf.get_unchecked(len + 1) } {
            // Missing separator
            return Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            )));
        }

        let data = &self.buf[0..len];
        // Skip data and separator
        self.buf = &self.buf[len + 1..];
        Ok(data.into())
    }

    /// Deserialize XML document with characterstring repertoire specified
    /// as value of XML encoding declaration.
    ///
    /// # Requirements
    /// - A field of datatype XMLData must contain a well-formed document,
    ///   as defined by the W3C XML recommendation.
    /// - Fields of datatype XMLData must have an associated field of type
    ///   Length.
    /// - Fields of datatype XMLData must be immediately preceded by their
    ///   associated Length field.
    pub fn deserialize_xml(&mut self, len: usize) -> Result<XmlData, DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )))
            }
            [b'\x01', ..] => {
                return Err(self.reject(self.current_tag, RejectReason::TagSpecifiedWithoutAValue))
            }
            _ => {}
        }

        // XML length + separator (SOH)
        if self.buf.len() < len + 1 {
            return Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            )));
        }

        // SAFETY: length checked above
        if let b'\x01' = unsafe { *self.buf.get_unchecked(len + 1) } {
            // Missing separator
            return Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            )));
        }

        // TODO: XML validation, RejectReason::XMLValidationError when invalid
        let xml = &self.buf[0..len];
        // Skip XML and separator
        self.buf = &self.buf[len + 1..];
        Ok(xml.into())
    }

    // fn deserialize_tenor(input: &[u8]) -> Result<Tenor, RejectReason>;

    // TODO: it would be nice to have on generic function for all `*_enum`
    //       deserializations

    pub fn deserialize_int_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        T: TryFrom<Int, Error = RejectReason>,
    {
        T::try_from(self.deserialize_int()?).map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_num_in_group_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        T: TryFrom<NumInGroup, Error = RejectReason>,
    {
        T::try_from(self.deserialize_num_in_group()?)
            .map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_char_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        T: TryFrom<Char, Error = RejectReason>,
    {
        let value = self.deserialize_char()?;
        T::try_from(value).map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_string_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        T: TryFrom<Vec<u8>, Error = RejectReason>,
    {
        let value = self.deserialize_string()?;
        T::try_from(value).map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_multiple_char_value_enum<T>(&mut self) -> Result<Vec<T>, DeserializeError>
    where
        T: TryFrom<Char, Error = RejectReason>,
    {
        let values = self.deserialize_multiple_char_value()?;
        let mut result = Vec::with_capacity(values.len());
        for value in values {
            result
                .push(T::try_from(value).map_err(|reason| self.reject(self.current_tag, reason))?);
        }
        Ok(result)
    }

    pub fn deserialize_multiple_string_value_enum<T>(&mut self) -> Result<Vec<T>, DeserializeError>
    where
        T: TryFrom<Vec<u8>, Error = RejectReason>,
    {
        let values = self.deserialize_multiple_string_value()?;
        let mut result = Vec::with_capacity(values.len());
        for value in values {
            result
                .push(T::try_from(value).map_err(|reason| self.reject(self.current_tag, reason))?);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::Deserializer;
    use crate::parser::RawMessage;

    fn deserializer(body: &[u8]) -> Deserializer {
        let raw_message = RawMessage {
            begin_string: &[],
            body,
            checksum: 0,
        };

        Deserializer {
            raw_message,
            buf: body,
            msg_type: Vec::new(),
            seq_num: None,
            current_tag: None,
            tmp_tag: None,
        }
    }

    #[test]
    fn deserialize_utc_timestamp_ok() {
        let input = b"20190605-11:51:27.848\x01";
        let mut deserializer = deserializer(input);
        let utc_timestamp = deserializer
            .deserialize_utc_timestamp()
            .expect("failed to deserialize utc timestamp");
        println!("{deserializer:?}");
        assert_eq!(utc_timestamp, input[..input.len() - 1]);
        assert!(deserializer.buf.is_empty());
    }
}
