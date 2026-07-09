use std::str::FromStr;

use assert_matches::assert_matches;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};

use super::{Deserializer, RawMessage, deserialize_tag, raw_message};
use crate::{
    base_messages::SessionRejectReasonBase,
    basic_types::{
        FixStr, LocalMktDate, Price, SessionRejectReasonField, Tenor, TenorUnit, TenorValue,
        TimePrecision,
    },
    deserializer::{
        DeserializeErrorKind, GarbledReason, LogoutReason, RawMessageError, deserialize_checksum,
    },
    fix_str,
    serializer::Serializer,
};

const BEGIN_STRING: &FixStr = fix_str!("FIXT.1.1");

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

/// A malformed `CheckSum(10)` field has no known end, so it is `Garbled`
/// (resynchronize), never `InvalidChecksum` (skip a known frame).
#[test]
fn deserialize_checksum_garbled() {
    assert_matches!(
        deserialize_checksum(b"A23\x01"),
        Err(RawMessageError::Garbled)
    );

    assert_matches!(deserialize_checksum(b"1234"), Err(RawMessageError::Garbled));
    assert_matches!(
        deserialize_checksum(b"1234\x01"),
        Err(RawMessageError::Garbled)
    );
}

/// A well-shaped value above 255 can never match; it is left to
/// `raw_message` to report as `InvalidChecksum`.
#[test]
fn deserialize_checksum_accepts_three_digits_above_u8() {
    assert_matches!(deserialize_checksum(b"999\x01"), Ok((b"", 999)));
}

/// A complete frame with a wrong `CheckSum(10)` reports its own length, so
/// a caller can skip exactly the rejected frame instead of scanning inside
/// it for the next message start.
#[test]
fn raw_message_invalid_checksum_reports_frame_len() {
    let frame = b"8=MSG_BODY\x019=19\x01<lots of tags here>10=144\x01";
    let mut input = frame.to_vec();
    input.extend_from_slice(b"leftover");
    assert_matches!(
        raw_message(&input),
        Err(RawMessageError::InvalidChecksum { frame_len }) => {
            assert_eq!(frame_len, frame.len());
        }
    );
    assert_matches!(
        raw_message(b"8=MSG_BODY\x019=19\x01<lots of tags here>10=999\x01"),
        Err(RawMessageError::InvalidChecksum { frame_len: 42 })
    );
}

#[test]
fn raw_message_ok() {
    let input = b"8=MSG_BODY\x019=19\x01<lots of tags here>10=143\x01";
    assert!(raw_message(input).is_ok());
}

/// A `BodyLength<9>` above `Length::MAX` must be **Garbled**, never
/// Incomplete. This is the whole inbound memory bound: `Garbled` tells the
/// caller to drop the bytes, `Incomplete` tells it to read more, so a
/// hostile `9=99999999` answered with `Incomplete` would be an unbounded
/// read. The verdict lands as soon as the length field itself is parsed -
/// no body of the declared size is ever buffered. Widening `Length` past
/// `u16` silently forfeits this; that is what this test guards.
#[test]
fn raw_message_body_length_above_length_max_is_garbled() {
    assert_matches!(
        raw_message(b"8=FIXT.1.1\x019=65536\x0135=A\x01"),
        Err(RawMessageError::Garbled)
    );
    assert_matches!(
        raw_message(b"8=FIXT.1.1\x019=99999999\x0135=A\x01"),
        Err(RawMessageError::Garbled)
    );
    // Just inside the range is only unsatisfied, not malformed - the
    // caller is told to keep reading.
    assert_matches!(
        raw_message(b"8=FIXT.1.1\x019=65535\x0135=A\x01"),
        Err(RawMessageError::Incomplete)
    );
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

/// Like [`deserializer`], but with MsgSeqNum(34) not yet parsed, so error
/// paths exercise the fallback seq num scan inside `reject`.
fn deserializer_without_seq_num(body: &[u8]) -> Deserializer<'_> {
    let raw_message = RawMessage {
        begin_string: BEGIN_STRING,
        body,
        checksum: 0,
    };

    Deserializer {
        raw_message,
        buf: body,
        msg_type: None,
        seq_num: None,
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
fn deserialize_utc_timestamp_rejects_overlong_fraction() {
    // A fraction digit count outside {3, 6, 9, 12} must be rejected even
    // when it is congruent to a valid count modulo 256.
    let mut input = b"20240102-03:04:05.".to_vec();
    input.extend_from_slice(&[b'0'; 256]);
    input.extend_from_slice(b"123\x01\x00");
    let mut deserializer = deserializer(&input);
    assert_matches!(
        deserializer.deserialize_utc_timestamp(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_utc_timestamp_overlong_fraction_does_not_overflow() {
    // A long digit string with a large value must be rejected without
    // overflowing the accumulator (an overflow panics in debug builds).
    let mut input = b"20240102-03:04:05.".to_vec();
    input.extend_from_slice(&[b'0'; 245]);
    input.extend_from_slice(&[b'9'; 14]);
    input.extend_from_slice(b"\x01\x00");
    let mut deserializer = deserializer(&input);
    assert_matches!(
        deserializer.deserialize_utc_timestamp(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_utc_timestamp_rejects_thirteen_digit_fraction() {
    let input = b"20240102-03:04:05.1234567890123\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_utc_timestamp(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_utc_timestamp_reject_scan_skips_own_value() {
    // The seconds digits "34" followed by '=' inside a malformed value must
    // not satisfy the fallback MsgSeqNum scan; the real tag 34 follows.
    let input = b"20240102-03:04:34=997\x0134=2\x01";
    let mut deserializer = deserializer_without_seq_num(input);
    assert_matches!(
        deserializer.deserialize_utc_timestamp(),
        Err(DeserializeErrorKind::Reject { seq_num: 2, .. })
    );
}

#[test]
fn deserialize_utc_timestamp_reject_without_seq_num_is_logout() {
    // No MsgSeqNum(34) after the malformed value: the reject must degrade
    // to Logout instead of fabricating a seq num from the value's own bytes.
    let input = b"20240102-03:04:34=997\x0158=text\x01";
    let mut deserializer = deserializer_without_seq_num(input);
    assert_matches!(
        deserializer.deserialize_utc_timestamp(),
        Err(DeserializeErrorKind::Logout(LogoutReason::MsgSeqNumMissing))
    );
}

#[test]
fn deserialize_utc_timestamp_truncated_value_is_garbled() {
    // Body ends before the value (or its SOH) is complete: incomplete
    // data, regardless of where the cut lands.
    for input in [
        &b"2024"[..],
        b"20240102-03:0",
        b"20240102-03:04:0",
        b"20240102-03:04:05",
        b"20240102-03:04:05.",
        b"20240102-03:04:05.12",
        b"20240102-03:04:05.123",
    ] {
        let mut deserializer = deserializer(input);
        assert_matches!(
            deserializer.deserialize_utc_timestamp(),
            Err(DeserializeErrorKind::Garbled(
                GarbledReason::IncompleteMessageData
            )),
            "input: {input:?}"
        );
    }
}

#[test]
fn deserialize_utc_timeonly_truncated_value_is_garbled() {
    for input in [&b"12:00:0"[..], b"12:00:00.1", b"12:00:00.123"] {
        let mut deserializer = deserializer(input);
        assert_matches!(
            deserializer.deserialize_utc_time_only(),
            Err(DeserializeErrorKind::Garbled(
                GarbledReason::IncompleteMessageData
            )),
            "input: {input:?}"
        );
    }
}

#[test]
fn deserialize_tz_timestamp_truncated_fraction_is_garbled() {
    let input = b"20060901-13:09:00.12";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timestamp(),
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::IncompleteMessageData
        ))
    );
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
            value: TenorValue::new(5).unwrap()
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
            value: TenorValue::new(3).unwrap()
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
            value: TenorValue::new(13).unwrap()
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
            value: TenorValue::new(1).unwrap()
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
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::IncorrectDataFormatForValue)
    );
}

#[test]
fn deserialize_tenor_unit_without_value() {
    // The value is present but malformed, which is a syntax error - not the
    // empty-value case (FIX Session Test Cases Scenario 14, rows d and f).
    let input = b"D\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::IncorrectDataFormatForValue)
    );
}

#[test]
fn deserialize_tenor_trailing_garbage() {
    let input = b"D5x\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::IncorrectDataFormatForValue)
    );
}

#[test]
fn deserialize_tenor_zero_value() {
    let input = b"D0\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_tenor_value_out_of_range() {
    let input = b"D65536\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_tenor_empty() {
    // Tag specified with no value at all (FIX Session Test Cases
    // Scenario 14, row d).
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tenor(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::TagSpecifiedWithoutAValue)
    );
}

#[test]
fn deserialize_tenor_truncated() {
    for input in [b"D5".as_slice(), b"D".as_slice(), b"".as_slice()] {
        let mut deserializer = deserializer(input);
        assert_matches!(
            deserializer.deserialize_tenor(),
            Err(DeserializeErrorKind::Garbled(
                GarbledReason::IncompleteMessageData
            ))
        );
    }
}

// --- LocalMktTime tests ---

/// Every minute value must parse, not just multiples of ten - the grammar is
/// `MM = 00-59` (TagValue Encoding section 6.2.2). A too-narrow digit range
/// here turns ordinary values like `09:31:00` into a session-level Reject.
#[test]
fn deserialize_local_mkt_time_accepts_every_minute_and_second() {
    for (input, expected) in [
        (&b"09:31:00\x01\x00"[..], (9, 31, 0)),
        (b"14:05:07\x01\x00", (14, 5, 7)),
        (b"00:00:00\x01\x00", (0, 0, 0)),
        (b"23:59:59\x01\x00", (23, 59, 59)),
    ] {
        let mut deserializer = deserializer(input);
        let (h, m, s) = expected;
        assert_eq!(
            deserializer
                .deserialize_local_mkt_time()
                .expect("valid LocalMktTime rejected"),
            NaiveTime::from_hms_opt(h, m, s).unwrap()
        );
    }
}

#[test]
fn deserialize_local_mkt_time_rejects_out_of_range_components() {
    for input in [
        &b"24:00:00\x01\x00"[..],
        b"09:60:00\x01\x00",
        // No leap second in this datatype - SS = 00-59.
        b"23:59:60\x01\x00",
    ] {
        let mut deserializer = deserializer(input);
        assert_matches!(
            deserializer.deserialize_local_mkt_time(),
            Err(DeserializeErrorKind::Reject { .. })
        );
    }
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
fn deserialize_tz_timestamp_without_seconds() {
    // The spec's own TZTimestamp example (TagValue Encoding section 6.2.2).
    let input = b"20060901-07:39Z\x01\x00";
    let mut deserializer = deserializer(input);
    let ts = deserializer
        .deserialize_tz_timestamp()
        .expect("failed to deserialize tz timestamp without seconds");
    let offset = FixedOffset::east_opt(0).unwrap();
    let expected = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_opt(7, 39, 0)
        .unwrap();
    assert_eq!(
        ts.timestamp(),
        offset.from_local_datetime(&expected).unwrap()
    );
    assert_eq!(ts.precision(), TimePrecision::Secs);
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_tz_timestamp_invalid_format() {
    let input = b"not-a-timestamp\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timestamp(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_tz_timestamp_empty() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timestamp(),
        Err(DeserializeErrorKind::Reject { .. })
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
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_tz_timeonly_empty() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timeonly(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_tz_timeonly_dangling_seconds_separator_is_rejected() {
    // The value is framed by its SOH, so it is malformed rather than
    // truncated, even when it is the last field in the buffer - a Reject,
    // not a garbled message (FIX Session Test Cases Scenario 14, row f).
    let input = b"07:39:\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timeonly(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::IncorrectDataFormatForValue)
    );
}

#[test]
fn deserialize_tz_timeonly_offset_minutes_out_of_range() {
    // mm = 00-59 (TagValue Encoding section 6.2.2); ":99" must not be
    // folded into the hour.
    let input = b"07:39:00+00:99\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timeonly(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::IncorrectDataFormatForValue)
    );
}

#[test]
fn deserialize_tz_timestamp_offset_minutes_out_of_range() {
    let input = b"20060901-07:39:00+00:99\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_tz_timestamp(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::IncorrectDataFormatForValue)
    );
}

/// Serialize a value with `ser` after parsing `input` with `de`, and return
/// what was written.
fn round_trip<T>(
    input: &[u8],
    de: impl FnOnce(&mut Deserializer) -> T,
    ser: impl FnOnce(&mut Serializer, &T),
) -> Vec<u8> {
    let mut body = input.to_vec();
    body.push(b'\x01');
    let mut deserializer = deserializer(&body);
    let value = de(&mut deserializer);
    let mut buf = [0u8; 128];
    let mut serializer = Serializer::new(&mut buf);
    ser(&mut serializer, &value);
    serializer.written().to_vec()
}

/// Assert that `input` - a value in the form this serializer emits - comes
/// back byte-identical after a parse and a re-serialize.
///
/// This is the invariant the resend path rides on. A retransmission is
/// produced by decoding a stored message, rewriting header fields and
/// serializing it again, while FIX Transport section 3.4 allows only
/// CheckSum, OrigSendingTime, SendingTime, BodyLength and PossDupFlag to
/// differ from the original. Every other field therefore has to be a fixed
/// point of parse -> serialize. Values in *other* valid wire forms (leading
/// zeros, `+01:00` for a whole-hour offset) are deliberately not covered:
/// they normalize on the way in, and they never reach storage because we
/// never emit them.
macro_rules! assert_round_trips {
    ($de:ident, $ser:ident, $($input:expr),+ $(,)?) => {
        $(
            let input: &[u8] = $input;
            let output = round_trip(
                input,
                |d| d.$de().expect("valid value rejected"),
                |s, v| s.$ser(v).expect("serialization failed"),
            );
            assert_eq!(
                output,
                input,
                "{} is not a fixed point of {} -> {}",
                String::from_utf8_lossy(input),
                stringify!($de),
                stringify!($ser),
            );
        )+
    };
}

#[test]
fn numeric_values_round_trip_unchanged() {
    assert_round_trips!(deserialize_int, serialize_int, b"23", b"-99999", b"0");
    assert_round_trips!(deserialize_length, serialize_length, b"23");
    assert_round_trips!(deserialize_seq_num, serialize_seq_num, b"7");
    assert_round_trips!(
        deserialize_float,
        serialize_float,
        b"23.23",
        b"23.0000",
        b"-97.0347",
        b"0",
    );
    assert_round_trips!(
        deserialize_tenor,
        serialize_tenor,
        b"D5",
        b"M3",
        b"W13",
        b"Y1"
    );
}

#[test]
fn time_values_round_trip_unchanged() {
    assert_round_trips!(
        deserialize_utc_timestamp,
        serialize_utc_timestamp,
        b"20240102-03:04:05",
        b"20240102-03:04:05.123",
        b"20240102-03:04:05.123456",
        b"20240102-03:04:05.123456789",
        // A leap second is a valid wire value at every precision, and it
        // must not degrade to :59 on the way through the type.
        b"19981231-23:59:60",
        b"19981231-23:59:60.123",
    );
    assert_round_trips!(
        deserialize_utc_time_only,
        serialize_utc_time_only,
        b"03:04:05",
        b"03:04:05.123456789",
        b"23:59:60",
        b"23:59:60.123",
    );
    assert_round_trips!(
        deserialize_utc_date_only,
        serialize_utc_date_only,
        b"20240102"
    );
    assert_round_trips!(
        deserialize_tz_timestamp,
        serialize_tz_timestamp,
        b"20060901-07:39:00Z",
        b"20060901-07:39:00+01",
        b"20060901-07:39:00-05",
        b"20060901-07:39:00+05:30",
        b"20060901-07:39:00.123+01",
    );
    assert_round_trips!(
        deserialize_tz_timeonly,
        serialize_tz_timeonly,
        b"07:39:00Z",
        b"07:39:00-05",
        b"07:39:00.123456789+05:30",
    );
}

#[test]
fn textual_values_round_trip_unchanged() {
    assert_round_trips!(deserialize_string, serialize_string, b"IBM");
    assert_round_trips!(deserialize_boolean, serialize_boolean, b"Y", b"N");
    assert_round_trips!(deserialize_char, serialize_char, b"F");
    assert_round_trips!(deserialize_country, serialize_country, b"PL");
    assert_round_trips!(deserialize_currency, serialize_currency, b"PLN");
    assert_round_trips!(deserialize_exchange, serialize_exchange, b"XNAS");
    assert_round_trips!(deserialize_language, serialize_language, b"pl");
    assert_round_trips!(deserialize_month_year, serialize_month_year, b"202401");
    assert_round_trips!(
        deserialize_multiple_char_value,
        serialize_multiple_char_value,
        b"2 A F"
    );
    assert_round_trips!(
        deserialize_multiple_string_value,
        serialize_multiple_string_value,
        b"AV AN A"
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
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::IncompleteMessageData
        ))
    );
}

#[test]
fn deserialize_multiple_char_value_no_value() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_leading_space() {
    let input = b" A\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_control_char() {
    let input = b"\x05\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_high_byte() {
    let input = b"\x80\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

// Values are read two bytes at a time, so a rejected byte anywhere but at the
// end arrives paired with its trailing space.
#[test]
fn deserialize_multiple_char_value_control_char_not_last() {
    let input = b"\x05 A\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_high_byte_not_last() {
    let input = b"\x80 A\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_two_chars_no_space() {
    let input = b"AB\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_char_value_no_soh() {
    let input = b"A B";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::IncompleteMessageData
        ))
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
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::IncompleteMessageData
        ))
    );
}

#[test]
fn deserialize_multiple_string_value_no_value() {
    let input = b"\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_string_value_control_char() {
    let input = b"AB\x05CD\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_string_value_high_byte() {
    let input = b"AB\x80\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeErrorKind::Reject { .. })
    );
}

#[test]
fn deserialize_multiple_string_value_no_soh() {
    let input = b"AV AN";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::IncompleteMessageData
        ))
    );
}

// ── Data / XmlData ──────────────────────────────────────────

#[test]
fn deserialize_data_ok() {
    // 5 bytes of data + SOH + marker
    let input = b"hello\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_data(5)
        .expect("failed to deserialize data");
    assert_eq!(&*result, b"hello");
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_data_with_soh_inside() {
    // Data can contain any bytes including SOH - length delimits, not SOH
    let input = b"ab\x01cd\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_data(5)
        .expect("failed to deserialize data with embedded SOH");
    assert_eq!(&*result, b"ab\x01cd");
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_data_missing_separator() {
    // Data without SOH separator after - should be detected as error
    let input = b"helloX\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_data(5),
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::MessageNotWellFormed
        ))
    );
}

#[test]
fn deserialize_data_empty_buf() {
    let input = b"";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_data(5),
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::IncompleteMessageData
        ))
    );
}

#[test]
fn deserialize_data_buf_too_short() {
    let input = b"hi\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_data(5),
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::IncompleteMessageData
        ))
    );
}

#[test]
fn deserialize_data_soh_after_separator() {
    // Byte after separator happens to be SOH (next field is empty-valued)
    // This is valid and should NOT cause a false "missing separator" error
    let input = b"hello\x01\x01";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_data(5)
        .expect("SOH after separator should not cause error");
    assert_eq!(&*result, b"hello");
    assert_eq!(deserializer.buf, b"\x01");
}

#[test]
fn deserialize_xml_ok() {
    let input = b"<x/>\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_xml(4)
        .expect("failed to deserialize xml");
    assert_eq!(&*result, b"<x/>");
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_xml_missing_separator() {
    let input = b"<x/>X\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_xml(4),
        Err(DeserializeErrorKind::Garbled(
            GarbledReason::MessageNotWellFormed
        ))
    );
}

#[test]
fn deserialize_exchange_ok() {
    let input = b"XNAS\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_exchange()
        .expect("failed to deserialize exchange");
    assert_eq!(result, *b"XNAS");
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_exchange_control_char() {
    let input = b"XN\x02S\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_exchange(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_exchange_high_byte() {
    let input = b"XN\x80S\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_exchange(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_exchange_wrong_length() {
    let input = b"XNA\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_exchange(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::IncorrectDataFormatForValue)
    );
}

#[test]
fn deserialize_language_ok() {
    let input = b"pl\x01\x00";
    let mut deserializer = deserializer(input);
    let result = deserializer
        .deserialize_language()
        .expect("failed to deserialize language");
    assert_eq!(result, *b"pl");
    assert_eq!(deserializer.buf, b"\x00");
}

#[test]
fn deserialize_language_control_char() {
    let input = b"p\x1f\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_language(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_char_del() {
    // 0x7f (DEL) is a control character despite being above the 0x00-0x1f range
    let input = b"\x7f\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_char(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_multiple_char_value_del() {
    let input = b"\x7f\x01\x00";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_char_value(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_multiple_string_value_del() {
    let input = b"ab\x7fcd\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_multiple_string_value(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_str_del() {
    let input = b"ab\x7fcd\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_str(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_exchange_del() {
    let input = b"XN\x7fS\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_exchange(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_language_del() {
    let input = b"p\x7f\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_language(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}

#[test]
fn deserialize_language_high_byte() {
    let input = b"p\x80\x01";
    let mut deserializer = deserializer(input);
    assert_matches!(
        deserializer.deserialize_language(),
        Err(DeserializeErrorKind::Reject { reason, .. })
            if reason == SessionRejectReasonField::from(SessionRejectReasonBase::ValueIsIncorrect)
    );
}
