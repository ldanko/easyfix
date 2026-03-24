use std::str::FromStr;

use assert_matches::assert_matches;
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};

use super::{Deserializer, RawMessage, deserialize_tag, raw_message};
use crate::{
    basic_types::{FixStr, LocalMktDate, Price, Tenor, TenorUnit, TimePrecision},
    deserializer::{DeserializeError, RawMessageError, deserialize_checksum},
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
    assert_eq!(deserializer.buf, &[b'\x00']);
}

#[test]
fn deserialize_string_ok() {
    let input = b"lorem ipsum\x01\x00";
    let mut deserializer = deserializer(input);
    let buf = deserializer
        .deserialize_string()
        .expect("failed to deserialize utc timestamp");
    assert_eq!(buf, "lorem ipsum");
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
}

/// FIXME: it seems that timeonly deserialization has not been working for some time now
#[ignore]
#[test]
fn deserialize_utc_timeonly_ok() {
    let input = b"11:51:27\x01\x00";
    let mut deserializer = deserializer(input);
    let utc_timeonly = deserializer
        .deserialize_utc_time_only()
        .expect("failed to deserialize utc timeonly");
    let time: NaiveTime = NaiveTime::from_hms_opt(11, 51, 27).unwrap();
    assert_eq!(utc_timeonly.timestamp(), time);
    assert_eq!(utc_timeonly.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
        assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
    assert_eq!(deserializer.buf, &[b'\x00']);
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
