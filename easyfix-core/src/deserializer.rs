use std::{error::Error, fmt};

use crate::{
    base_messages::SessionRejectReasonBase,
    basic_types::{
        Amt, Boolean, Char, Country, Currency, Data, DayOfMonth, Decimal, Exchange, FixStr,
        FixString, FixedOffset, Float, Int, Language, Length, LocalMktDate, LocalMktTime,
        MonthYear, MultipleCharValue, MultipleStringValue, NaiveDate, NaiveTime, NumInGroup,
        Percentage, Price, PriceOffset, Qty, SeqNum, SessionRejectReasonField, TagNum, Tenor,
        TenorUnit, TimeZone, TzTimeOnly, TzTimestamp, Utc, UtcDateOnly, UtcTimeOnly, UtcTimestamp,
        XmlData,
    },
};

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub enum DeserializeError {
    // TODO: enum maybe?
    GarbledMessage(String),
    Logout,
    Reject {
        msg_type: Option<FixString>,
        seq_num: SeqNum,
        tag: Option<TagNum>,
        reason: SessionRejectReasonField,
    },
}

impl fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeserializeError::GarbledMessage(reason) => write!(f, "garbled message: {}", reason),
            DeserializeError::Logout => write!(f, "MsgSeqNum missing"),
            DeserializeError::Reject {
                tag: Some(tag),
                reason,
                ..
            } => write!(f, "{reason:?} (tag={tag})"),
            DeserializeError::Reject {
                tag: None, reason, ..
            } => write!(f, "{reason:?}"),
        }
    }
}

impl Error for DeserializeError {}

impl From<RawMessageError> for DeserializeError {
    fn from(error: RawMessageError) -> Self {
        match error {
            RawMessageError::Incomplete => {
                DeserializeError::GarbledMessage("Incomplete message data".to_owned())
            }
            RawMessageError::Garbled => {
                DeserializeError::GarbledMessage("Message not well formed".to_owned())
            }
            RawMessageError::InvalidChecksum => {
                DeserializeError::GarbledMessage("Invalid checksum".to_owned())
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum DeserializeErrorInternal {
    #[error("Incomplete")]
    Incomplete,
    #[error("{0:?}")]
    Error(SessionRejectReasonBase),
}

fn deserialize_tag<'a>(bytes: &'a [u8], tag: &'a [u8]) -> Result<&'a [u8], RawMessageError> {
    if bytes.len() < tag.len() {
        Err(RawMessageError::Incomplete)
    } else if bytes.starts_with(tag) {
        Ok(&bytes[tag.len()..])
    } else {
        Err(RawMessageError::Garbled)
    }
}

fn deserialize_checksum(bytes: &[u8]) -> Result<(&[u8], u8), RawMessageError> {
    if bytes.len() < 4 {
        return Err(RawMessageError::Incomplete);
    }

    let mut value: u8 = 0;
    for b in &bytes[0..3] {
        match b {
            n @ b'0'..=b'9' => {
                value = value
                    .checked_mul(10)
                    .and_then(|v| v.checked_add(n - b'0'))
                    .ok_or(RawMessageError::InvalidChecksum)?;
            }
            _ => return Err(RawMessageError::InvalidChecksum),
        }
    }

    if bytes[3] != b'\x01' {
        return Err(RawMessageError::InvalidChecksum);
    }

    Ok((&bytes[4..], value))
}

/// Convert raw fractional-second digits to nanoseconds.
/// `digits` is the number of digits parsed (3=ms, 6=µs, 9=ns, 12=ps).
fn fraction_to_nanos(fraction: u64, digits: u8) -> Result<u32, SessionRejectReasonBase> {
    let (multiplier, divider) = match digits {
        3 => (1_000_000u64, 1u64),
        6 => (1_000, 1),
        9 => (1, 1),
        // chrono can't hold picoseconds — truncate to nanoseconds
        12 => (1, 1_000),
        _ => return Err(SessionRejectReasonBase::IncorrectDataFormatForValue),
    };
    (fraction * multiplier / divider)
        .try_into()
        .map_err(|_| SessionRejectReasonBase::ValueIsIncorrect)
}

fn deserialize_str(bytes: &[u8]) -> Result<(&[u8], &FixStr), DeserializeErrorInternal> {
    for (i, b) in bytes.iter().enumerate() {
        match b {
            // No control character is allowed
            0x00 | 0x02..=0x1f | 0x7f..=0xff => {
                return Err(DeserializeErrorInternal::Error(
                    SessionRejectReasonBase::ValueIsIncorrect,
                ));
            }
            // Except SOH which marks end of tag
            b'\x01' => {
                if i == 0 {
                    return Err(DeserializeErrorInternal::Error(
                        SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                    ));
                } else {
                    // SAFETY: Check for valid ASCII values just above
                    return Ok((&bytes[i + 1..], unsafe {
                        FixStr::from_ascii_unchecked(&bytes[..i])
                    }));
                }
            }
            _ => {}
        }
    }

    Err(DeserializeErrorInternal::Incomplete)
}

fn deserialize_length(bytes: &[u8]) -> Result<(&[u8], Length), DeserializeErrorInternal> {
    let mut value: Length = 0;
    for (i, b) in bytes.iter().enumerate() {
        match b {
            n @ b'0'..=b'9' => {
                value = value
                    .checked_mul(10)
                    .and_then(|v| v.checked_add(Length::from(n - b'0')))
                    .ok_or(DeserializeErrorInternal::Error(
                        SessionRejectReasonBase::ValueIsIncorrect,
                    ))?;
            }
            b'\x01' => {
                if i == 0 {
                    return Err(DeserializeErrorInternal::Error(
                        SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                    ));
                } else if value == 0 {
                    return Err(DeserializeErrorInternal::Error(
                        SessionRejectReasonBase::ValueIsIncorrect,
                    ));
                } else {
                    return Ok((&bytes[i + 1..], value));
                }
            }
            _ => {
                return Err(DeserializeErrorInternal::Error(
                    SessionRejectReasonBase::IncorrectDataFormatForValue,
                ));
            }
        }
    }

    Err(DeserializeErrorInternal::Incomplete)
}

#[derive(Debug)]
pub struct RawMessage<'a> {
    pub begin_string: &'a FixStr,
    pub body: &'a [u8],
    pub checksum: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum RawMessageError {
    #[error("Incomplete")]
    Incomplete,
    #[error("Garbled")]
    Garbled,
    #[error("Invalid checksum")]
    InvalidChecksum,
}

impl From<DeserializeErrorInternal> for RawMessageError {
    fn from(d: DeserializeErrorInternal) -> RawMessageError {
        match d {
            DeserializeErrorInternal::Incomplete => RawMessageError::Incomplete,
            DeserializeErrorInternal::Error(_) => RawMessageError::Garbled,
        }
    }
}

pub fn raw_message(bytes: &[u8]) -> Result<(&[u8], RawMessage<'_>), RawMessageError> {
    let orig_bytes = bytes;

    let bytes = deserialize_tag(bytes, b"8=")?;
    let (bytes, begin_string) = deserialize_str(bytes)?;

    let bytes = deserialize_tag(bytes, b"9=")?;
    let (bytes, body_length) = deserialize_length(bytes)?;
    let body_length = usize::from(body_length);

    const CHECKSUM_LEN: usize = 4;
    if bytes.len() < body_length + CHECKSUM_LEN {
        return Err(RawMessageError::Incomplete);
    }

    let body = &bytes[..body_length];
    let bytes = &bytes[body_length..];

    let calculated_checksum = orig_bytes[0..orig_bytes.len() - bytes.len()]
        .iter()
        .fold(0, |acc: u8, x| acc.wrapping_add(*x));

    let bytes = deserialize_tag(bytes, b"10=")?;
    let (bytes, checksum) = deserialize_checksum(bytes)?;
    if calculated_checksum != checksum {
        return Err(RawMessageError::InvalidChecksum);
    }
    Ok((
        bytes,
        RawMessage {
            begin_string,
            body,
            checksum,
        },
    ))
}

// TODO:
// enum GarbledReason

#[derive(Debug)]
pub struct Deserializer<'de> {
    raw_message: RawMessage<'de>,
    buf: &'de [u8],
    msg_type: Option<std::ops::Range<usize>>,
    seq_num: Option<SeqNum>,
    current_tag: Option<TagNum>,
    // Used to put tag back to deserializer, when switching to deserialization
    // another message section.
    tmp_tag: Option<TagNum>,
}

impl Deserializer<'_> {
    pub fn from_raw_message(raw_message: RawMessage) -> Deserializer {
        let buf = raw_message.body;
        Deserializer {
            raw_message,
            buf,
            msg_type: None,
            seq_num: None,
            current_tag: None,
            tmp_tag: None,
        }
    }

    pub fn begin_string(&self) -> FixString {
        self.raw_message.begin_string.to_owned()
    }

    pub fn body_length(&self) -> Length {
        self.raw_message.body.len() as Length
    }

    pub fn check_sum(&self) -> FixString {
        FixString::from_ascii_lossy(format!("{:03}", self.raw_message.checksum).into_bytes())
    }

    pub fn set_seq_num(&mut self, seq_num: SeqNum) {
        debug_assert!(self.seq_num.is_none());

        self.seq_num = Some(seq_num);
    }

    // This may fail when RawData or XmlData fields (or other binary fields)
    // are located before MsgSeqNum and has value `34=` inside
    fn try_find_msg_seq_num(&mut self) -> Result<SeqNum, DeserializeError> {
        let seq_num_tag = b"34=";

        let start_index = self
            .buf
            .windows(seq_num_tag.len())
            .position(|window| window == seq_num_tag)
            .ok_or(DeserializeError::Logout)?;
        self.buf = &self.buf[start_index + seq_num_tag.len()..];

        self.deserialize_seq_num()
    }

    pub fn reject(
        &mut self,
        tag: Option<TagNum>,
        reason: SessionRejectReasonBase,
    ) -> DeserializeError {
        let seq_num = if let Some(seq_num) = self.seq_num {
            seq_num
        } else {
            match self.try_find_msg_seq_num() {
                Ok(seq_num) => seq_num,
                Err(err) => return err,
            }
        };

        DeserializeError::Reject {
            msg_type: self.msg_type.clone().map(|msg_type| {
                FixString::from_ascii_lossy(self.raw_message.body[msg_type].to_vec())
            }),
            seq_num,
            tag,
            reason: reason.into(),
        }
    }

    pub fn repeating_group_fields_out_of_order(
        &mut self,
        expected_tags: &[u16],
        processed_tags: &[u16],
        current_tag: u16,
    ) -> DeserializeError {
        let mut current_tag_found = false;
        'outer: for processed_tag in processed_tags {
            for expected_tag in expected_tags {
                if expected_tag == processed_tag {
                    if current_tag_found {
                        return self.reject(
                            Some(*processed_tag),
                            SessionRejectReasonBase::RepeatingGroupFieldsOutOfOrder,
                        );
                    } else {
                        continue 'outer;
                    }
                } else if *expected_tag == current_tag {
                    current_tag_found = true;
                }
            }
        }
        // This should never happen
        debug_assert!(false);
        self.reject(
            None,
            SessionRejectReasonBase::RepeatingGroupFieldsOutOfOrder,
        )
    }

    pub fn put_tag(&mut self, tag: TagNum) {
        self.tmp_tag = Some(tag);
    }

    pub fn range_to_fixstr(&self, range: std::ops::Range<usize>) -> &FixStr {
        unsafe { FixStr::from_ascii_unchecked(&self.raw_message.body[range]) }
    }

    /// Deserialize MsgType
    pub fn deserialize_msg_type(&mut self) -> Result<std::ops::Range<usize>, DeserializeError> {
        let raw_message_pointer = self.raw_message.body.as_ptr();

        let msg_type_range = {
            let Ok(deser_str) = self.deserialize_str() else {
                return Err(self.reject(Some(35), SessionRejectReasonBase::InvalidMsgType));
            };
            let msg_type_pointer = deser_str.as_bytes().as_ptr();
            let msg_type_start_index =
                unsafe { msg_type_pointer.offset_from(raw_message_pointer) } as usize;
            let msg_type_len = deser_str.len();
            msg_type_start_index..(msg_type_start_index + msg_type_len)
        };

        self.msg_type = Some(msg_type_range.clone());
        Ok(msg_type_range)
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
            [b'0' | b'=', ..] => {
                return Err(self.reject(None, SessionRejectReasonBase::InvalidTagNumber));
            }
            _ => {}
        }

        let mut value: TagNum = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and self.buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    value = value
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as TagNum))
                        // Integer overflow
                        .ok_or_else(|| {
                            self.reject(None, SessionRejectReasonBase::InvalidTagNumber)
                        })?;
                }
                b'=' => {
                    if value == 0 {
                        return Err(self
                            .reject(self.current_tag, SessionRejectReasonBase::InvalidTagNumber));
                    } else {
                        self.current_tag = Some(value);
                        self.buf = &self.buf[i + 1..];
                        return Ok(Some(value));
                    }
                }
                // Unexpected value
                _ => return Err(self.reject(None, SessionRejectReasonBase::InvalidTagNumber)),
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
                )));
            }
            [b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                ));
            }
            [b'-', b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::IncorrectDataFormatForValue,
                ));
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
                            self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                        })?;
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    return Ok(if negative { -value } else { value });
                }
                _ => {
                    return Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    ));
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
                )));
            }
            [b'\x01', ..] => return Err(DeserializeError::Logout),
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
                        .ok_or(DeserializeError::Logout)?;
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    // XXX: Accept `0` as EndSeqNum<16> uses `0` as infinite
                    return Ok(value);
                }
                _ => return Err(DeserializeError::Logout),
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
                )));
            }
            [b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                ));
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
                        .ok_or_else(|| {
                            self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                        })?;
                }
                b'\x01' => {
                    if value == 0 {
                        return Err(self
                            .reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect));
                    } else {
                        self.buf = &self.buf[i + 1..];
                        return Ok(value);
                    }
                }
                _ => {
                    return Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    ));
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
                )));
            }
            [b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                ));
            }
            [b'0', ..] => {
                return Err(
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                );
            }
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
                        .ok_or_else(|| {
                            self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                        })?;
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    break;
                }
                _ => {
                    return Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    ));
                }
            }
        }

        match value {
            1..=31 => Ok(value),
            _ => Err(self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)),
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
                )));
            }
            [b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                ));
            }
            [b'-', b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::IncorrectDataFormatForValue,
                ));
            }
            [b'-', buf @ ..] => (true, buf),
            _ => (false, self.buf),
        };

        let mut num: i64 = 0;
        let mut scale = None;
        for i in 0..buf.len() {
            // SAFETY: i is between 0 and buf.len()
            match unsafe { buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    num = num
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as i64))
                        .ok_or_else(|| {
                            self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                        })?;
                    if let Some(scale) = scale.as_mut() {
                        *scale += 1;
                    }
                }
                b'.' => {
                    if scale.is_some() {
                        return Err(self.reject(
                            self.current_tag,
                            SessionRejectReasonBase::IncorrectDataFormatForValue,
                        ));
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
                    return Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    ));
                }
            }
        }

        Err(DeserializeError::GarbledMessage(format!(
            "no more data to parse tag {:?}",
            self.current_tag
        )))
    }

    #[inline(always)]
    pub fn deserialize_qty(&mut self) -> Result<Qty, DeserializeError> {
        self.deserialize_float()
    }

    #[inline(always)]
    pub fn deserialize_price(&mut self) -> Result<Price, DeserializeError> {
        self.deserialize_float()
    }

    #[inline(always)]
    pub fn deserialize_price_offset(&mut self) -> Result<PriceOffset, DeserializeError> {
        self.deserialize_float()
    }

    #[inline(always)]
    pub fn deserialize_amt(&mut self) -> Result<Amt, DeserializeError> {
        self.deserialize_float()
    }

    #[inline(always)]
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
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            [b'Y', b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok(true)
            }
            [b'N', b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok(false)
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize any ASCII character except control characters.
    // TODO: [Feature]: Deserialize any ISO/IEC 8859-1 (Latin-1) character except control characters.
    pub fn deserialize_char(&mut self) -> Result<Char, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            // ASCII controll characters range + unused range
            [0x00..=0x1f | 0x80..=0xff, ..] => {
                Err(self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect))
            }
            [n, b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok(*n)
            }
            // Missing separator at the end
            [_, byte] if *byte != b'\x01' => Err(DeserializeError::GarbledMessage(
                "missing tag spearator".into(),
            )),
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
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
                )));
            }
            [b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                ));
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
                                SessionRejectReasonBase::IncorrectDataFormatForValue,
                            ));
                        }
                        // Latin-1 controll characters ranges
                        // [0x00..=0x1f] | [0x80..=0x9f] | [0x00..=0x1f, _] | [0x80..=0x9f, _] => {

                        // ASCII controll character range + unused range
                        [0x00..=0x1f] | [0x7f..=0xff] => {
                            return Err(self.reject(
                                self.current_tag,
                                SessionRejectReasonBase::ValueIsIncorrect,
                            ));
                        }
                        [n] | [n, b' '] => result.push(*n),
                        _ => {
                            return Err(self.reject(
                                self.current_tag,
                                SessionRejectReasonBase::IncorrectDataFormatForValue,
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
    pub fn deserialize_str(&mut self) -> Result<&FixStr, DeserializeError> {
        match deserialize_str(self.buf) {
            Ok((leftover, fix_str)) => {
                self.buf = leftover;
                Ok(fix_str)
            }
            Err(DeserializeErrorInternal::Incomplete) => Err(DeserializeError::GarbledMessage(
                format!("no more data to parse tag {:?}", self.current_tag),
            )),
            Err(DeserializeErrorInternal::Error(reason)) => {
                Err(self.reject(self.current_tag, reason))
            }
        }
    }

    /// Deserialize alphanumeric free-format strings can include any character
    /// except control characters.
    #[inline(always)]
    pub fn deserialize_string(&mut self) -> Result<FixString, DeserializeError> {
        self.deserialize_str().map(FixString::from)
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
                )));
            }
            [b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                ));
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
                    let mut sub_result = Vec::with_capacity(part.len());
                    for byte in part {
                        match byte {
                            // ASCII controll characters range
                            0x00..=0x1f | 0x80..=0xff => {
                                return Err(self.reject(
                                    self.current_tag,
                                    SessionRejectReasonBase::ValueIsIncorrect,
                                ));
                            }
                            n => sub_result.push(*n),
                        }
                    }
                    // SAFETY: string validity checked above
                    result.push(unsafe { FixString::from_ascii_unchecked(sub_result) });
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
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            [_, b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
            bytes @ [_, _, b'\x01', buf @ ..] => {
                self.buf = buf;
                Country::from_bytes(&bytes[0..2]).ok_or_else(|| {
                    self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    )
                })
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
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize ISO 4217:2015 Codes for the representation of currencies
    /// and funds (3-character code).
    pub fn deserialize_currency(&mut self) -> Result<Currency, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            bytes @ [_, _, _, b'\x01', buf @ ..] => {
                self.buf = buf;
                Currency::from_bytes(&bytes[0..3]).ok_or_else(|| {
                    self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    )
                })
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize ISO 10383:2012 Securities and related financial instruments
    /// – Codes for exchanges and market identification (MIC)
    /// (4-character code).
    pub fn deserialize_exchange(&mut self) -> Result<Exchange, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            [a, b, c, d, b'\x01', buf @ ..] => {
                self.buf = buf;
                // TODO
                Ok([*a, *b, *c, *d])
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize string representing month of a year.
    /// An optional day of the month can be appended or an optional week code.
    ///
    /// # Valid formats:
    /// - `YYYYMM
    /// - `YYYYMMDD
    /// - `YYYYMMWW
    ///
    /// # Valid values:
    /// - YYYY = 0000-9999
    /// - MM = 01-12
    /// - DD = 01-31
    /// - WW = w1, w2, w3, w4, w5
    pub fn deserialize_month_year(&mut self) -> Result<MonthYear, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            [a, b, c, d, e, f, g, h, b'\x01', buf @ ..] => {
                self.buf = buf;
                // TODO
                Ok([*a, *b, *c, *d, *e, *f, *g, *h].into())
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize ISO 639-1:2002 Codes for the representation of names
    /// of languages (2-character code).
    pub fn deserialize_language(&mut self) -> Result<Language, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            [a, b, b'\x01', buf @ ..] => {
                self.buf = buf;
                Ok([*a, *b])
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    // Helper for UTC timestamp deserialization.
    fn deserialize_fraction_of_second(&mut self) -> Result<(u32, u8), DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )));
            }
            [b'\x01', rest @ ..] => {
                self.buf = rest;
                return Ok((0, 0));
            }
            // Do nothing here, fraction of second will be deserialized below
            [b'.', rest @ ..] => self.buf = rest,
            _ => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::IncorrectDataFormatForValue,
                ));
            }
        }

        let mut fraction_of_second: u64 = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    fraction_of_second = fraction_of_second
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as u64))
                        .ok_or_else(|| {
                            self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                        })?;
                }
                b'\x01' => {
                    self.buf = &self.buf[i + 1..];
                    return fraction_to_nanos(fraction_of_second, i as u8)
                        .map(|ns| (ns, i as u8))
                        .map_err(|reason| self.reject(self.current_tag, reason));
                }
                _ => {
                    return Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    ));
                }
            }
        }

        Err(self.reject(
            self.current_tag,
            SessionRejectReasonBase::IncorrectDataFormatForValue,
        ))
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
    /// - MM = 00-59,
    /// - SS = 00-60 (60 only if UTC leap second),
    /// - sss* fractions of seconds. The fractions of seconds may be empty when
    ///   no fractions of seconds are conveyed (in such a case the period
    ///   is not conveyed), it may include 3 digits to convey
    ///   milliseconds, 6 digits to convey microseconds, 9 digits
    ///   to convey nanoseconds, 12 digits to convey picoseconds;
    pub fn deserialize_utc_timestamp(&mut self) -> Result<UtcTimestamp, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [
                // Year
                y3 @ b'0'..=b'9',
                y2 @ b'0'..=b'9',
                y1 @ b'0'..=b'9',
                y0 @ b'0'..=b'9',
                // Month
                m1 @ b'0'..=b'1',
                m0 @ b'0'..=b'9',
                // Day
                d1 @ b'0'..=b'3',
                d0 @ b'0'..=b'9',
                b'-',
                // Hour
                h1 @ b'0'..=b'2',
                h0 @ b'0'..=b'9',
                b':',
                // Minute
                mm1 @ b'0'..=b'5',
                mm0 @ b'0'..=b'9',
                b':',
                rest @ ..,
            ] => {
                let year = (y3 - b'0') as i32 * 1000
                    + (y2 - b'0') as i32 * 100
                    + (y1 - b'0') as i32 * 10
                    + (y0 - b'0') as i32;
                let month = (m1 - b'0') as u32 * 10 + (m0 - b'0') as u32;
                let day = (d1 - b'0') as u32 * 10 + (d0 - b'0') as u32;
                let naive_date = NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| {
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                })?;
                let hour = (h1 - b'0') as u32 * 10 + (h0 - b'0') as u32;
                let min = (mm1 - b'0') as u32 * 10 + (mm0 - b'0') as u32;

                // Parse seconds: normal (00-59) or leap second (60)
                let (sec, leap_offset) = match rest {
                    [s1 @ b'0'..=b'5', s0 @ b'0'..=b'9', rest @ ..] => {
                        let sec = (s1 - b'0') as u32 * 10 + (s0 - b'0') as u32;
                        self.buf = rest;
                        (sec, 0)
                    }
                    [b'6', b'0', rest @ ..] => {
                        // chrono represents leap seconds as sec=59 with nanosecond >= 1_000_000_000
                        self.buf = rest;
                        (59, 1_000_000_000)
                    }
                    _ => {
                        return Err(self.reject(
                            self.current_tag,
                            SessionRejectReasonBase::IncorrectDataFormatForValue,
                        ));
                    }
                };

                let (fraction_of_second, precision) = self.deserialize_fraction_of_second()?;
                let naive_date_time = naive_date
                    .and_hms_nano_opt(hour, min, sec, leap_offset + fraction_of_second)
                    .ok_or_else(|| {
                        self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                    })?;
                let timestamp = Utc.from_utc_datetime(&naive_date_time);

                match precision {
                    0 => Ok(UtcTimestamp::with_secs(timestamp)),
                    3 => Ok(UtcTimestamp::with_millis(timestamp)),
                    6 => Ok(UtcTimestamp::with_micros(timestamp)),
                    9 => Ok(UtcTimestamp::with_nanos(timestamp)),
                    // XXX: Types from `chrono` crate can't hold
                    //      time at picosecond resolution
                    12 => Ok(UtcTimestamp::with_nanos(timestamp)),
                    _ => Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    )),
                }
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
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
    ///   no fractions of seconds are conveyed (in such a case the period
    ///   is not conveyed), it may include 3 digits to convey
    ///   milliseconds, 6 digits to convey microseconds, 9 digits
    ///   to convey nanoseconds, 12 digits to convey picoseconds;
    ///   // TODO: set precision!
    pub fn deserialize_utc_time_only(&mut self) -> Result<UtcTimeOnly, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            [
                // hours
                h1 @ b'0'..=b'2',
                h0 @ b'0'..=b'9',
                b':',
                // minutes
                m1 @ b'0'..=b'5',
                m0 @ b'0'..=b'9',
                b':',
                // seconds
                s1 @ b'0'..=b'5',
                s0 @ b'0'..=b'9',
                rest @ ..,
            ] => {
                let h = (h1 - b'0') * 10 + (h0 - b'0');
                let m = (m1 - b'0') * 10 + (m0 - b'0');
                let s = (s1 - b'0') * 10 + (s0 - b'0');
                self.buf = rest;
                let (ns, precision) = self.deserialize_fraction_of_second()?;
                let timestamp = NaiveTime::from_hms_nano_opt(h.into(), m.into(), s.into(), ns)
                    .ok_or_else(|| {
                        self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                    });
                match timestamp {
                    Ok(timestamp) => {
                        match precision {
                            0 => Ok(UtcTimeOnly::with_secs(timestamp)),
                            3 => Ok(UtcTimeOnly::with_millis(timestamp)),
                            6 => Ok(UtcTimeOnly::with_micros(timestamp)),
                            9 => Ok(UtcTimeOnly::with_nanos(timestamp)),
                            // XXX: Types from `chrono` crate can't hold
                            //      time at picosecond resolution
                            12 => Ok(UtcTimeOnly::with_nanos(timestamp)),
                            _ => Err(self.reject(
                                self.current_tag,
                                SessionRejectReasonBase::IncorrectDataFormatForValue,
                            )),
                        }
                    }
                    Err(err) => Err(err),
                }
            }
            // Leap second case
            [
                h1 @ b'0'..=b'2',
                h0 @ b'0'..=b'9',
                b':',
                m1 @ b'0'..=b'5',
                m0 @ b'0'..=b'9',
                b':',
                b'6',
                b'0',
                rest @ ..,
            ] => {
                let h = (h1 - b'0') * 10 + (h0 - b'0');
                let m = (m1 - b'0') * 10 + (m0 - b'0');
                self.buf = rest;
                let (ns, precision) = self.deserialize_fraction_of_second()?;
                // chrono represents leap seconds as sec=59 with nanosecond >= 1_000_000_000
                let timestamp =
                    NaiveTime::from_hms_nano_opt(h.into(), m.into(), 59, 1_000_000_000 + ns)
                        .ok_or_else(|| {
                            self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                        });
                match timestamp {
                    Ok(timestamp) => {
                        match precision {
                            0 => Ok(UtcTimeOnly::with_secs(timestamp)),
                            3 => Ok(UtcTimeOnly::with_millis(timestamp)),
                            6 => Ok(UtcTimeOnly::with_micros(timestamp)),
                            9 => Ok(UtcTimeOnly::with_nanos(timestamp)),
                            // XXX: Types from `chrono` crate can't hold
                            //      time at picosecond resolution
                            12 => Ok(UtcTimeOnly::with_nanos(timestamp)),
                            _ => Err(self.reject(
                                self.current_tag,
                                SessionRejectReasonBase::IncorrectDataFormatForValue,
                            )),
                        }
                    }
                    Err(err) => Err(err),
                }
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
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
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [
                // Year
                y3 @ b'0'..=b'9',
                y2 @ b'0'..=b'9',
                y1 @ b'0'..=b'9',
                y0 @ b'0'..=b'9',
                // Month
                m1 @ b'0'..=b'1',
                m0 @ b'0'..=b'9',
                // Day
                d1 @ b'0'..=b'3',
                d0 @ b'0'..=b'9',
                // Separator
                b'\x01',
                rest @ ..,
            ] => {
                let year = (y3 - b'0') as u16 * 1000
                    + (y2 - b'0') as u16 * 100
                    + (y1 - b'0') as u16 * 10
                    + (y0 - b'0') as u16;
                let month = (m1 - b'0') * 10 + (m0 - b'0');
                let day = (d1 - b'0') * 10 + (d0 - b'0');
                self.buf = rest;
                UtcDateOnly::from_ymd_opt(year.into(), month.into(), day.into()).ok_or_else(|| {
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                })
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
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
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),

            [
                // Hour
                h1 @ b'0'..=b'2',
                h0 @ b'0'..=b'9',
                b':',
                // Minute
                m1 @ b'0'..=b'5',
                m0 @ b'0'..=b'0',
                b':',
                // Second
                s1 @ b'0'..=b'5',
                s0 @ b'0'..=b'9',
                b'\x01',
                rest @ ..,
            ] => {
                let hour = (h1 - b'0') as u32 * 10 + (h0 - b'0') as u32;
                let minute = (m1 - b'0') as u32 * 10 + (m0 - b'0') as u32;
                let second = (s1 - b'0') as u32 * 10 + (s0 - b'0') as u32;
                self.buf = rest;
                LocalMktTime::from_hms_opt(hour, minute, second).ok_or_else(|| {
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                })
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
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
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [
                // Year
                y3 @ b'0'..=b'9',
                y2 @ b'0'..=b'9',
                y1 @ b'0'..=b'9',
                y0 @ b'0'..=b'9',
                // Month
                m1 @ b'0'..=b'1',
                m0 @ b'0'..=b'9',
                // Day
                d1 @ b'0'..=b'3',
                d0 @ b'0'..=b'9',
                // Separator
                b'\x01',
                rest @ ..,
            ] => {
                let year = (y3 - b'0') as u16 * 1000
                    + (y2 - b'0') as u16 * 100
                    + (y1 - b'0') as u16 * 10
                    + (y0 - b'0') as u16;
                let month = (m1 - b'0') * 10 + (m0 - b'0');
                let day = (d1 - b'0') * 10 + (d0 - b'0');
                self.buf = rest;
                LocalMktDate::from_ymd_opt(year.into(), month.into(), day.into()).ok_or_else(|| {
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                })
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Parse optional fractional seconds for TZ types.
    /// Like `deserialize_fraction_of_second`, but stops at Z/+/-/SOH
    /// instead of consuming SOH. Does not advance buf past the terminator.
    fn deserialize_tz_fraction_of_second(&mut self) -> Result<(u32, u8), DeserializeError> {
        match self.buf {
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )));
            }
            // No fraction — offset or SOH follows directly
            [b'Z' | b'+' | b'-' | b'\x01', ..] => {
                return Ok((0, 0));
            }
            // Fraction follows
            [b'.', rest @ ..] => self.buf = rest,
            _ => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::IncorrectDataFormatForValue,
                ));
            }
        }

        let mut fraction_of_second: u64 = 0;
        for i in 0..self.buf.len() {
            // SAFETY: i is between 0 and buf.len()
            match unsafe { self.buf.get_unchecked(i) } {
                n @ b'0'..=b'9' => {
                    fraction_of_second = fraction_of_second
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((n - b'0') as u64))
                        .ok_or_else(|| {
                            self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                        })?;
                }
                // Stop at offset or SOH — don't consume terminator
                b'Z' | b'+' | b'-' | b'\x01' => {
                    self.buf = &self.buf[i..];
                    return fraction_to_nanos(fraction_of_second, i as u8)
                        .map(|ns| (ns, i as u8))
                        .map_err(|reason| self.reject(self.current_tag, reason));
                }
                _ => {
                    return Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    ));
                }
            }
        }

        Err(self.reject(
            self.current_tag,
            SessionRejectReasonBase::IncorrectDataFormatForValue,
        ))
    }

    /// Parse timezone offset: Z, +hh, +hh:mm, -hh, -hh:mm.
    /// Consumes the offset and trailing SOH delimiter.
    fn deserialize_tz_offset(&mut self) -> Result<FixedOffset, DeserializeError> {
        match self.buf {
            [b'Z', b'\x01', rest @ ..] => {
                self.buf = rest;
                Ok(FixedOffset::east_opt(0).unwrap())
            }
            [
                sign @ (b'+' | b'-'),
                h1 @ b'0'..=b'9',
                h0 @ b'0'..=b'9',
                b':',
                m1 @ b'0'..=b'9',
                m0 @ b'0'..=b'9',
                b'\x01',
                rest @ ..,
            ] => {
                let hours = (*h1 - b'0') as i32 * 10 + (*h0 - b'0') as i32;
                let minutes = (*m1 - b'0') as i32 * 10 + (*m0 - b'0') as i32;
                let total_secs = hours * 3600 + minutes * 60;
                let total_secs = if *sign == b'-' {
                    -total_secs
                } else {
                    total_secs
                };
                self.buf = rest;
                FixedOffset::east_opt(total_secs).ok_or_else(|| {
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                })
            }
            [
                sign @ (b'+' | b'-'),
                h1 @ b'0'..=b'9',
                h0 @ b'0'..=b'9',
                b'\x01',
                rest @ ..,
            ] => {
                let hours = (*h1 - b'0') as i32 * 10 + (*h0 - b'0') as i32;
                let total_secs = hours * 3600;
                let total_secs = if *sign == b'-' {
                    -total_secs
                } else {
                    total_secs
                };
                self.buf = rest;
                FixedOffset::east_opt(total_secs).ok_or_else(|| {
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                })
            }
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize string representing a time/date combination representing
    /// local time with an offset to UTC to allow identification of local time
    /// and time zone offset of that time.
    ///
    /// The representation is based on ISO 8601.
    ///
    /// Format is `YYYYMMDD-HH:MM:SS[.sss*][Z | [ + | – hh[:mm]]]` where:
    /// - YYYY = 0000 to 9999,
    /// - MM = 01-12,
    /// - DD = 01-31,
    /// - HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds,
    /// - hh = 01-12 offset hours,
    /// - mm = 00-59 offset minutes,
    /// - sss* fractions of seconds. The fractions of seconds may be empty when
    ///   no fractions of seconds are conveyed (in such a case the period
    ///   is not conveyed), it may include 3 digits to convey
    ///   milliseconds, 6 digits to convey microseconds, 9 digits
    ///   to convey nanoseconds, 12 digits to convey picoseconds;
    pub fn deserialize_tz_timestamp(&mut self) -> Result<TzTimestamp, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [
                // Year
                y3 @ b'0'..=b'9',
                y2 @ b'0'..=b'9',
                y1 @ b'0'..=b'9',
                y0 @ b'0'..=b'9',
                // Month
                m1 @ b'0'..=b'1',
                m0 @ b'0'..=b'9',
                // Day
                d1 @ b'0'..=b'3',
                d0 @ b'0'..=b'9',
                b'-',
                // Hour
                h1 @ b'0'..=b'2',
                h0 @ b'0'..=b'9',
                b':',
                // Minute
                mm1 @ b'0'..=b'5',
                mm0 @ b'0'..=b'9',
                b':',
                // Second
                s1 @ b'0'..=b'5',
                s0 @ b'0'..=b'9',
                rest @ ..,
            ] => {
                self.buf = rest;
                let year = (y3 - b'0') as i32 * 1000
                    + (y2 - b'0') as i32 * 100
                    + (y1 - b'0') as i32 * 10
                    + (y0 - b'0') as i32;
                let month = (m1 - b'0') as u32 * 10 + (m0 - b'0') as u32;
                let day = (d1 - b'0') as u32 * 10 + (d0 - b'0') as u32;
                let hour = (h1 - b'0') as u32 * 10 + (h0 - b'0') as u32;
                let min = (mm1 - b'0') as u32 * 10 + (mm0 - b'0') as u32;
                let sec = (s1 - b'0') as u32 * 10 + (s0 - b'0') as u32;
                let (fraction_of_second, precision) = self.deserialize_tz_fraction_of_second()?;
                let offset = self.deserialize_tz_offset()?;

                let naive_date = NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| {
                    self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                })?;
                let naive_date_time = naive_date
                    .and_hms_nano_opt(hour, min, sec, fraction_of_second)
                    .ok_or_else(|| {
                        self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                    })?;
                let timestamp = offset
                    .from_local_datetime(&naive_date_time)
                    .single()
                    .ok_or_else(|| {
                        self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                    })?;

                match precision {
                    0 => Ok(TzTimestamp::with_secs(timestamp)),
                    3 => Ok(TzTimestamp::with_millis(timestamp)),
                    6 => Ok(TzTimestamp::with_micros(timestamp)),
                    9 => Ok(TzTimestamp::with_nanos(timestamp)),
                    // XXX: Types from `chrono` crate can't hold
                    //      time at picosecond resolution
                    12 => Ok(TzTimestamp::with_nanos(timestamp)),
                    _ => Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    )),
                }
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize time of day with timezone. Time represented based on
    /// ISO 8601. This is the time with a UTC offset to allow identification of
    /// local time and time zone of that time.
    ///
    /// Format is `HH:MM[:SS][Z | [ + | – hh[:mm]]]` where:
    /// - HH = 00-23 hours,
    /// - MM = 00-59 minutes,
    /// - SS = 00-59 seconds,
    /// - hh = 01-12 offset hours,
    /// - mm = 00-59 offset minutes.
    pub fn deserialize_tz_timeonly(&mut self) -> Result<TzTimeOnly, DeserializeError> {
        match self.buf {
            [] => Err(DeserializeError::GarbledMessage(format!(
                "no more data to parse tag {:?}",
                self.current_tag
            ))),
            [b'\x01', ..] => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::TagSpecifiedWithoutAValue,
            )),
            // Missing separator at the end
            [_] => Err(DeserializeError::GarbledMessage(format!(
                "missing tag ({:?}) separator",
                self.current_tag
            ))),
            [
                // Hour
                h1 @ b'0'..=b'2',
                h0 @ b'0'..=b'9',
                b':',
                // Minute
                mm1 @ b'0'..=b'5',
                mm0 @ b'0'..=b'9',
                rest @ ..,
            ] => {
                let hour = (h1 - b'0') as u32 * 10 + (h0 - b'0') as u32;
                let min = (mm1 - b'0') as u32 * 10 + (mm0 - b'0') as u32;
                self.buf = rest;

                // Optional :SS
                let sec = match self.buf {
                    [b':', s1 @ b'0'..=b'5', s0 @ b'0'..=b'9', rest @ ..] => {
                        let sec = (s1 - b'0') as u32 * 10 + (s0 - b'0') as u32;
                        self.buf = rest;
                        sec
                    }
                    _ => 0,
                };

                let (fraction_of_second, precision) = self.deserialize_tz_fraction_of_second()?;
                let offset = self.deserialize_tz_offset()?;

                let time = NaiveTime::from_hms_nano_opt(hour, min, sec, fraction_of_second)
                    .ok_or_else(|| {
                        self.reject(self.current_tag, SessionRejectReasonBase::ValueIsIncorrect)
                    })?;

                match precision {
                    0 => Ok(TzTimeOnly::with_secs(time, offset)),
                    3 => Ok(TzTimeOnly::with_millis(time, offset)),
                    6 => Ok(TzTimeOnly::with_micros(time, offset)),
                    9 => Ok(TzTimeOnly::with_nanos(time, offset)),
                    // XXX: Types from `chrono` crate can't hold
                    //      time at picosecond resolution
                    12 => Ok(TzTimeOnly::with_nanos(time, offset)),
                    _ => Err(self.reject(
                        self.current_tag,
                        SessionRejectReasonBase::IncorrectDataFormatForValue,
                    )),
                }
            }
            _ => Err(self.reject(
                self.current_tag,
                SessionRejectReasonBase::IncorrectDataFormatForValue,
            )),
        }
    }

    /// Deserialize sequence of character digits without commas or decimals.
    /// Value must be positive. Fields of datatype Length are referred to as
    /// Length fields.
    ///
    /// The Length field must be associated with a field of datatype data.
    ///
    /// The Length field must specify the number of octets of the value
    /// contained in the associated data field up to but not including
    /// the terminating `<SOH>`.
    pub fn deserialize_length(&mut self) -> Result<Length, DeserializeError> {
        match deserialize_length(self.buf) {
            Ok((leftover, len)) => {
                self.buf = leftover;
                Ok(len)
            }
            Err(DeserializeErrorInternal::Incomplete) => Err(DeserializeError::GarbledMessage(
                format!("no more data to parse tag {:?}", self.current_tag),
            )),
            Err(DeserializeErrorInternal::Error(reject)) => {
                Err(self.reject(self.current_tag, reject))
            }
        }
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
                )));
            }
            [b'\x01', ..] => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::TagSpecifiedWithoutAValue,
                ));
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

        // TODO: XML validation, SessionRejectReasonBase::XmlValidationError when invalid
        let xml = &self.buf[0..len];
        // Skip XML and separator
        self.buf = &self.buf[len + 1..];
        Ok(xml.into())
    }

    pub fn deserialize_tenor(&mut self) -> Result<Tenor, DeserializeError> {
        let (unit, rest) = match self.buf {
            [b'D', rest @ ..] => (TenorUnit::Days, rest),
            [b'M', rest @ ..] => (TenorUnit::Months, rest),
            [b'W', rest @ ..] => (TenorUnit::Weeks, rest),
            [b'Y', rest @ ..] => (TenorUnit::Years, rest),
            [] => {
                return Err(DeserializeError::GarbledMessage(format!(
                    "no more data to parse tag {:?}",
                    self.current_tag
                )));
            }
            _ => {
                return Err(self.reject(
                    self.current_tag,
                    SessionRejectReasonBase::IncorrectDataFormatForValue,
                ));
            }
        };
        match deserialize_length(rest) {
            Ok((leftover, value)) => {
                self.buf = leftover;
                Ok(Tenor { unit, value })
            }
            Err(DeserializeErrorInternal::Incomplete) => Err(DeserializeError::GarbledMessage(
                format!("no more data to parse tag {:?}", self.current_tag),
            )),
            Err(DeserializeErrorInternal::Error(reason)) => {
                Err(self.reject(self.current_tag, reason))
            }
        }
    }

    pub fn deserialize_int_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        T: TryFrom<Int, Error = SessionRejectReasonBase>,
    {
        T::try_from(self.deserialize_int()?).map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_num_in_group_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        T: TryFrom<NumInGroup, Error = SessionRejectReasonBase>,
    {
        T::try_from(self.deserialize_num_in_group()?)
            .map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_char_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        T: TryFrom<Char, Error = SessionRejectReasonBase>,
    {
        let value = self.deserialize_char()?;
        T::try_from(value).map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_string_enum<T>(&mut self) -> Result<T, DeserializeError>
    where
        for<'a> T: TryFrom<&'a FixStr, Error = SessionRejectReasonBase>,
    {
        let value = self.deserialize_str()?;
        T::try_from(value).map_err(|reason| self.reject(self.current_tag, reason))
    }

    pub fn deserialize_multiple_char_value_enum<T>(&mut self) -> Result<Vec<T>, DeserializeError>
    where
        T: TryFrom<Char, Error = SessionRejectReasonBase>,
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
        for<'a> T: TryFrom<&'a FixStr, Error = SessionRejectReasonBase>,
    {
        let values = self.deserialize_multiple_string_value()?;
        let mut result = Vec::with_capacity(values.len());
        for value in values {
            result
                .push(T::try_from(&value).map_err(|reason| self.reject(self.current_tag, reason))?);
        }
        Ok(result)
    }
}
