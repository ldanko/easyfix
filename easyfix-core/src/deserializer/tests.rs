use std::str::FromStr;

use assert_matches::assert_matches;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};

use super::{Deserializer, RawMessage, deserialize_tag, raw_message};
use crate::{
    basic_types::{FixStr, LocalMktDate, Price, Tenor, TenorUnit, TimePrecision},
    deserializer::{DeserializeError, RawMessageError, deserialize_checksum},
    fix_str,
};

const BEGIN_STRING: &FixStr = unsafe { FixStr::from_ascii_unchecked(b"FIXT.1.1") };

#[test]
fn deserialize_tag_ok() {
    assert_matches!(deserialize_tag(b"8=FIXT1.1\x01", b"8="), Ok(b"FIXT1.1\x01"));
}

#[test]
fn deserialize_tag_incomplete() {
    assert_matches!(
        deserialize_tag(b"", b"8="),
        Err(RawMessageError::Incomplete)
    );

    assert_matches!(
        deserialize_tag(b"8", b"8="),
        Err(RawMessageError::Incomplete)
    );
}

#[test]
fn deserialize_tag_garbled() {
    assert_matches!(
        deserialize_tag(b"89FIXT1.1\x01", b"8="),
        Err(RawMessageError::Garbled)
    );
}

#[test]
fn deserialize_checksum_ok() {
    assert_matches!(deserialize_checksum(b"123\x01"), Ok((b"", 123)));

    assert_matches!(
        deserialize_checksum(b"123\x01more data"),
        Ok((b"more data", 123))
    );
}

#[test]
fn deserialize_checksum_incomplete() {
    assert_matches!(deserialize_checksum(b"1"), Err(RawMessageError::Incomplete));

    assert_matches!(
        deserialize_checksum(b"12"),
        Err(RawMessageError::Incomplete)
    );

    assert_matches!(
        deserialize_checksum(b"123"),
        Err(RawMessageError::Incomplete)
    );
}

#[test]
fn deserialize_checksum_garbled() {
    assert_matches!(
        deserialize_checksum(b"A23\x01"),
        Err(RawMessageError::InvalidChecksum)
    );

    assert_matches!(
        deserialize_checksum(b"1234"),
        Err(RawMessageError::InvalidChecksum)
    );
    assert_matches!(
        deserialize_checksum(b"1234\x01"),
        Err(RawMessageError::InvalidChecksum)
    );
}

#[test]
fn raw_message_ok() {
    let input = b"8=MSG_BODY\x019=19\x01<lots of tags here>10=143\x01";
    assert!(raw_message(input).is_ok());
}

#[test]
fn raw_message_from_chunks_ok() {
    let input = &[
        b"8=MSG_BOD".as_slice(),
        b"Y\x019=19\x01<lots".as_slice(),
        b" of tags here>10=143\x01".as_slice(),
        b"leftover".as_slice(),
    ];
    let mut buf = Vec::new();
    let mut i = input.iter();
    {
        buf.extend_from_slice(i.next().unwrap());
        assert!(matches!(
            raw_message(&buf),
            Err(RawMessageError::Incomplete)
        ));
    }
    {
        buf.extend_from_slice(i.next().unwrap());
        assert!(matches!(
            raw_message(&buf),
            Err(RawMessageError::Incomplete)
        ));
    }
    {
        buf.extend_from_slice(i.next().unwrap());
        assert!(matches!(raw_message(&buf), Ok(([], _))));
    }
    {
        buf.extend_from_slice(i.next().unwrap());
        assert!(matches!(raw_message(&buf), Ok((b"leftover", _))));
    }
}

fn deserializer(body: &[u8]) -> Deserializer<'_> {
    let raw_message = RawMessage {
        begin_string: BEGIN_STRING,
        body,
        checksum: 0,
    };

    Deserializer {
        raw_message,
        buf: body,
        msg_type: None,
        seq_num: Some(1),
        current_tag: None,
        tmp_tag: None,
    }
}

#[test]
fn deserialize_str_ok() {
    let input = b"lorem ipsum\x01\x00";
    let mut deserializer = deserializer(input);
    let buf = deserializer
        .deserialize_str()
        .expect("failed to deserialize utc timestamp");
    assert_eq!(buf, "lorem ipsum");
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_string_ok() {
    let input = b"lorem ipsum\x01\x00";
    let mut deserializer = deserializer(input);
    let buf = deserializer
        .deserialize_string()
        .expect("failed to deserialize utc timestamp");
    assert_eq!(buf, "lorem ipsum");
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timestamp_ok() {
    let input = b"20190605-11:51:27\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timestamp = deserializer
        .deserialize_utc_timestamp()
        .expect("failed to deserialize utc timestamp");
    let date_time: DateTime<Utc> = Utc.from_utc_datetime(
        &NaiveDate::from_ymd_opt(2019, 6, 5)
            .unwrap()
            .and_hms_opt(11, 51, 27)
            .unwrap(),
    );
    assert_eq!(utc_timestamp.timestamp(), date_time);
    assert_eq!(utc_timestamp.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timestamp_with_millis_ok() {
    let input = b"20190605-11:51:27.848\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timestamp = deserializer
        .deserialize_utc_timestamp()
        .expect("failed to deserialize utc timestamp");
    let date_time = Utc.from_utc_datetime(
        &NaiveDate::from_ymd_opt(2019, 6, 5)
            .unwrap()
            .and_hms_milli_opt(11, 51, 27, 848)
            .unwrap(),
    );
    assert_eq!(utc_timestamp.timestamp(), date_time);
    assert_eq!(utc_timestamp.precision(), TimePrecision::Millis);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timestamp_with_micros_ok() {
    let input = b"20190605-11:51:27.848757\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timestamp = deserializer
        .deserialize_utc_timestamp()
        .expect("failed to deserialize utc timestamp");
    let date_time = Utc.from_utc_datetime(
        &NaiveDate::from_ymd_opt(2019, 6, 5)
            .unwrap()
            .and_hms_micro_opt(11, 51, 27, 848757)
            .unwrap(),
    );
    assert_eq!(utc_timestamp.timestamp(), date_time);
    assert_eq!(utc_timestamp.precision(), TimePrecision::Micros);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timestamp_with_nanos_ok() {
    let input = b"20190605-11:51:27.848757123\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timestamp = deserializer
        .deserialize_utc_timestamp()
        .expect("failed to deserialize utc timestamp");
    let date_time = Utc.from_utc_datetime(
        &NaiveDate::from_ymd_opt(2019, 6, 5)
            .unwrap()
            .and_hms_nano_opt(11, 51, 27, 848757123)
            .unwrap(),
    );
    assert_eq!(utc_timestamp.timestamp(), date_time);
    assert_eq!(utc_timestamp.precision(), TimePrecision::Nanos);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timestamp_with_picos_ok() {
    let input = b"20190605-11:51:27.848757123999\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timestamp = deserializer
        .deserialize_utc_timestamp()
        .expect("failed to deserialize utc timestamp");
    let date_time = Utc.from_utc_datetime(
        &NaiveDate::from_ymd_opt(2019, 6, 5)
            .unwrap()
            .and_hms_nano_opt(11, 51, 27, 848757123)
            .unwrap(),
    );
    assert_eq!(utc_timestamp.timestamp(), date_time);
    assert_eq!(utc_timestamp.precision(), TimePrecision::Nanos);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timeonly_secs() {
    let input = b"11:51:27\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timeonly = deserializer
        .deserialize_utc_time_only()
        .expect("failed to deserialize utc timeonly");
    let time = NaiveTime::from_hms_opt(11, 51, 27).unwrap();
    assert_eq!(utc_timeonly.timestamp(), time);
    assert_eq!(utc_timeonly.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timeonly_millis() {
    let input = b"11:51:27.123\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timeonly = deserializer
        .deserialize_utc_time_only()
        .expect("failed to deserialize utc timeonly");
    let time = NaiveTime::from_hms_milli_opt(11, 51, 27, 123).unwrap();
    assert_eq!(utc_timeonly.timestamp(), time);
    assert_eq!(utc_timeonly.precision(), TimePrecision::Millis);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timeonly_nanos() {
    let input = b"11:51:27.123456789\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timeonly = deserializer
        .deserialize_utc_time_only()
        .expect("failed to deserialize utc timeonly");
    let time = NaiveTime::from_hms_nano_opt(11, 51, 27, 123_456_789).unwrap();
    assert_eq!(utc_timeonly.timestamp(), time);
    assert_eq!(utc_timeonly.precision(), TimePrecision::Nanos);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timeonly_leap_second() {
    let input = b"23:59:60\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timeonly = deserializer
        .deserialize_utc_time_only()
        .expect("failed to deserialize utc timeonly leap second");
    assert_eq!(utc_timeonly.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_utc_timeonly_leap_second_with_millis() {
    let input = b"23:59:60.123\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timeonly = deserializer
        .deserialize_utc_time_only()
        .expect("failed to deserialize utc timeonly leap second with millis");
    assert_eq!(utc_timeonly.precision(), TimePrecision::Millis);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_local_mkt_date_ok() {
    let input = b"20220530\x01\x00";
    let mut deserializer = deserializer(input);
    let local_mkt_date = deserializer
        .deserialize_local_mkt_date()
        .expect("failed to deserialize utc timestamp");
    assert_eq!(
        local_mkt_date,
        LocalMktDate::from_ymd_opt(2022, 5, 30).unwrap()
    );
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_price_ok() {
    let values: &[(&[u8], Price)] = &[
        (b"97\x01\x00", Price::from_str("97").expect("Wrong decimal")),
        (
            b"97.\x01\x00",
            Price::from_str("97.").expect("Wrong decimal"),
        ),
        (
            b"97.0347\x01\x00",
            Price::from_str("97.0347").expect("Wrong decimal"),
        ),
    ];
    for (input, value) in values {
        let mut deserializer = deserializer(input);
        let price = deserializer
            .deserialize_price()
            .expect("failed to deserialize price");
        assert_eq!(price, *value);
        assert_eq!(deserializer.buf, b"\x00");
    }
}

#[test]
fn deserialize_tenor_days() {
    let input = b"D5\x01\x00";
    let mut deserializer = deserializer(input);
    let tenor = deserializer
        .deserialize_tenor()
        .expect("failed to deserialize tenor");
    assert_eq!(
        tenor,
        Tenor {
            unit: TenorUnit::Days,
            value: 5
        }
    );
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tenor_months() {
    let input = b"M3\x01\x00";
    let mut deserializer = deserializer(input);
    let tenor = deserializer
        .deserialize_tenor()
        .expect("failed to deserialize tenor");
    assert_eq!(
        tenor,
        Tenor {
            unit: TenorUnit::Months,
            value: 3
        }
    );
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tenor_weeks() {
    let input = b"W13\x01\x00";
    let mut deserializer = deserializer(input);
    let tenor = deserializer
        .deserialize_tenor()
        .expect("failed to deserialize tenor");
    assert_eq!(
        tenor,
        Tenor {
            unit: TenorUnit::Weeks,
            value: 13
        }
    );
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tenor_years() {
    let input = b"Y1\x01\x00";
    let mut deserializer = deserializer(input);
    let tenor = deserializer
        .deserialize_tenor()
        .expect("failed to deserialize tenor");
    assert_eq!(
        tenor,
        Tenor {
            unit: TenorUnit::Years,
            value: 1
        }
    );
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tenor_invalid_unit() {
    let input = b"X5\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_tenor_zero_value() {
    let input = b"D0\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_tenor_empty() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeError::Reject { .. })
    );
}

// --- TzTimestamp tests ---

#[test]
fn deserialize_tz_timestamp_utc() {
    // "20060901-07:39Z" from spec examples
    let input = b"20060901-07:39:00Z\x01\x00";
    let mut deserializer = deserializer(input);
    let ts = deserializer
        .deserialize_tz_timestamp()
        .expect("failed to deserialize tz timestamp");
    let offset = FixedOffset::east_opt(0).unwrap();
    let expected = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_opt(7, 39, 0)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    assert_eq!(ts.timestamp(), expected);
    assert_eq!(ts.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timestamp_negative_offset() {
    // "20060901-02:39-05" from spec examples
    let input = b"20060901-02:39:00-05\x01\x00";
    let mut deserializer = deserializer(input);
    let ts = deserializer
        .deserialize_tz_timestamp()
        .expect("failed to deserialize tz timestamp");
    let offset = FixedOffset::west_opt(5 * 3600).unwrap();
    let expected = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_opt(2, 39, 0)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    assert_eq!(ts.timestamp(), expected);
    assert_eq!(ts.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timestamp_positive_offset_with_minutes() {
    // "20060901-13:09+05:30" from spec examples (India time)
    let input = b"20060901-13:09:00+05:30\x01\x00";
    let mut deserializer = deserializer(input);
    let ts = deserializer
        .deserialize_tz_timestamp()
        .expect("failed to deserialize tz timestamp");
    let offset = FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
    let expected = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_opt(13, 9, 0)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    assert_eq!(ts.timestamp(), expected);
    assert_eq!(ts.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timestamp_with_millis() {
    let input = b"20060901-13:09:00.123+05:30\x01\x00";
    let mut deserializer = deserializer(input);
    let ts = deserializer
        .deserialize_tz_timestamp()
        .expect("failed to deserialize tz timestamp");
    let offset = FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
    let expected = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_milli_opt(13, 9, 0, 123)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    assert_eq!(ts.timestamp(), expected);
    assert_eq!(ts.precision(), TimePrecision::Millis);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timestamp_with_nanos_utc() {
    let input = b"20060901-13:09:00.123456789Z\x01\x00";
    let mut deserializer = deserializer(input);
    let ts = deserializer
        .deserialize_tz_timestamp()
        .expect("failed to deserialize tz timestamp");
    let offset = FixedOffset::east_opt(0).unwrap();
    let expected = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_nano_opt(13, 9, 0, 123_456_789)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    assert_eq!(ts.timestamp(), expected);
    assert_eq!(ts.precision(), TimePrecision::Nanos);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timestamp_invalid_format() {
    let input = b"not-a-timestamp\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timestamp(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_tz_timestamp_empty() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timestamp(),
        Err(DeserializeError::Reject { .. })
    );
}

// --- TzTimeOnly tests ---

#[test]
fn deserialize_tz_timeonly_utc_no_seconds() {
    let input = b"07:39Z\x01\x00";
    let mut deserializer = deserializer(input);
    let t = deserializer
        .deserialize_tz_timeonly()
        .expect("failed to deserialize tz timeonly");
    let offset = FixedOffset::east_opt(0).unwrap();
    assert_eq!(t.timestamp(), NaiveTime::from_hms_opt(7, 39, 0).unwrap());
    assert_eq!(t.offset(), offset);
    assert_eq!(t.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timeonly_with_seconds() {
    let input = b"07:39:45+08\x01\x00";
    let mut deserializer = deserializer(input);
    let t = deserializer
        .deserialize_tz_timeonly()
        .expect("failed to deserialize tz timeonly");
    let offset = FixedOffset::east_opt(8 * 3600).unwrap();
    assert_eq!(t.timestamp(), NaiveTime::from_hms_opt(7, 39, 45).unwrap());
    assert_eq!(t.offset(), offset);
    assert_eq!(t.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timeonly_negative_offset_with_minutes() {
    let input = b"13:09:30-05:30\x01\x00";
    let mut deserializer = deserializer(input);
    let t = deserializer
        .deserialize_tz_timeonly()
        .expect("failed to deserialize tz timeonly");
    let offset = FixedOffset::west_opt(5 * 3600 + 30 * 60).unwrap();
    assert_eq!(t.timestamp(), NaiveTime::from_hms_opt(13, 9, 30).unwrap());
    assert_eq!(t.offset(), offset);
    assert_eq!(t.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timeonly_with_millis() {
    let input = b"13:09:30.123+05:30\x01\x00";
    let mut deserializer = deserializer(input);
    let t = deserializer
        .deserialize_tz_timeonly()
        .expect("failed to deserialize tz timeonly");
    let offset = FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
    assert_eq!(
        t.timestamp(),
        NaiveTime::from_hms_milli_opt(13, 9, 30, 123).unwrap()
    );
    assert_eq!(t.offset(), offset);
    assert_eq!(t.precision(), TimePrecision::Millis);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timeonly_invalid_format() {
    let input = b"xx:yy\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timeonly(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_tz_timeonly_empty() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timeonly(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_negative_float() {
    let values: &[(&[u8], Price)] = &[
        (b"-3\x01\x00", Price::from_str("-3").expect("Wrong decimal")),
        (
            b"-3.14\x01\x00",
            Price::from_str("-3.14").expect("Wrong decimal"),
        ),
        (
            b"-97.0347\x01\x00",
            Price::from_str("-97.0347").expect("Wrong decimal"),
        ),
    ];
    for (input, value) in values {
        let mut deserializer = deserializer(input);
        let price = deserializer
            .deserialize_price()
            .expect("failed to deserialize negative price");
        assert_eq!(price, *value);
        assert_eq!(deserializer.buf, b"\x00");
    }
}

// ── MultipleCharValue ───────────────────────────────────────

#[test]
fn deserialize_multiple_char_value_single() {
    let input = b"A\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_multiple_char_value()
        .expect("failed to deserialize");
    assert_eq!(result, vec![b'A']);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_multiple_char_value_multiple() {
    let input = b"2 A F\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_multiple_char_value()
        .expect("failed to deserialize");
    assert_eq!(result, vec![b'2', b'A', b'F']);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_multiple_char_value_empty() {
    let input = b"";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeError::GarbledMessage(_))
    );
}

#[test]
fn deserialize_multiple_char_value_no_value() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_leading_space() {
    let input = b" A\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_control_char() {
    let input = b"\x05\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_high_byte() {
    let input = b"\x80\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_two_chars_no_space() {
    let input = b"AB\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_no_soh() {
    let input = b"A B";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeError::GarbledMessage(_))
    );
}

// ── MultipleStringValue ─────────────────────────────────────

#[test]
fn deserialize_multiple_string_value_single() {
    let input = b"ABC\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_multiple_string_value()
        .expect("failed to deserialize");
    assert_eq!(result, vec![fix_str!("ABC").to_owned()]);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_multiple_string_value_multiple() {
    let input = b"AV AN A\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_multiple_string_value()
        .expect("failed to deserialize");
    assert_eq!(
        result,
        vec![
            fix_str!("AV").to_owned(),
            fix_str!("AN").to_owned(),
            fix_str!("A").to_owned(),
        ]
    );
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_multiple_string_value_empty() {
    let input = b"";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeError::GarbledMessage(_))
    );
}

#[test]
fn deserialize_multiple_string_value_no_value() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_string_value_control_char() {
    let input = b"AB\x05CD\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_string_value_high_byte() {
    let input = b"AB\x80\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeError::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_string_value_no_soh() {
    let input = b"AV AN";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeError::GarbledMessage(_))
    );
}
