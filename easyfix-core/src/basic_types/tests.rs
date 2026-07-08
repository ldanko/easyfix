use super::*;

#[test]
fn fix_string_fail_on_ctrl_character() {
    let buf = b"Hello\x01world!".to_vec();
    assert!(FixString::from_ascii(buf).is_err());
}

#[test]
fn fix_string_fail_on_out_of_range_character() {
    let buf = b"Hello\x85world!".to_vec();
    assert!(FixString::from_ascii(buf).is_err());
}

#[test]
fn fix_string_fail_on_del_character() {
    // 0x7F (DEL) is a control character despite being above the 0x00-0x1F range
    let buf = b"Hello\x7fworld!".to_vec();
    assert!(FixString::from_ascii(buf).is_err());
}

#[test]
fn fix_string_accept_tilde() {
    // 0x7E (~) is the highest valid printable ASCII character, just below DEL
    let buf = b"Hello~world!".to_vec();
    assert!(FixString::from_ascii(buf).is_ok());
}

/// Every safe conversion into `FixString` must enforce the printable-ASCII
/// invariant. `as_utf8` relies on it via `from_utf8_unchecked`, so a
/// conversion that lets arbitrary bytes through would make even printing
/// a `FixString` undefined behavior.
#[test]
fn fix_string_conversions_reject_non_ascii() {
    assert!(FixString::try_from(&b"Hello\xffworld!"[..]).is_err());
    assert!(FixString::try_from(b"Hello\xffworld!".to_vec()).is_err());
    assert!(FixString::try_from(*b"Hello\xffworld!").is_err());
    assert!(FixString::try_from(b"Hello\xffworld!").is_err());
}

#[test]
fn fix_string_conversions_reject_control_characters() {
    assert!(FixString::try_from(&b"Hello\x01world!"[..]).is_err());
    assert!(FixString::try_from(b"Hello\x01world!".to_vec()).is_err());
    assert!(FixString::try_from(*b"Hello\x01world!").is_err());
    assert!(FixString::try_from(b"Hello\x01world!").is_err());
}

#[test]
fn fix_string_conversions_accept_printable_ascii() {
    let expected = b"Hello world!";
    assert_eq!(FixString::try_from(&expected[..]).unwrap(), expected);
    assert_eq!(FixString::try_from(expected.to_vec()).unwrap(), expected);
    assert_eq!(FixString::try_from(*expected).unwrap(), expected);
    assert_eq!(FixString::try_from(expected).unwrap(), expected);
    assert_eq!(FixString::try_from("Hello world!").unwrap(), expected);
    assert_eq!(
        FixString::try_from(String::from("Hello world!")).unwrap(),
        expected
    );
}

#[test]
fn fix_string_replacemen_character_on_ctrl() {
    let buf = b"Hello\x01world!".to_vec();
    assert_eq!(FixString::from_ascii_lossy(buf), "Hello?world!");
}

#[test]
fn fix_string_replacemen_character_on_out_of_range() {
    let buf = b"Hello\x85world!".to_vec();
    assert_eq!(FixString::from_ascii_lossy(buf), "Hello?world!");
}

/// Unlike the clock bounds, the epoch is transmittable - and it renders at
/// the width the caller asked for, since that width is what reaches the
/// wire.
#[test]
fn unix_epoch_is_transmittable_at_the_requested_width() {
    for (precision, expected) in [
        (TimePrecision::Secs, "19700101-00:00:00"),
        (TimePrecision::Millis, "19700101-00:00:00.000"),
        (TimePrecision::Nanos, "19700101-00:00:00.000000000"),
    ] {
        let epoch = UtcTimestamp::unix_epoch(precision);
        assert_eq!(epoch.precision(), precision);
        assert_eq!(epoch.format_precisely().to_string(), expected);
    }

    // Equality ignores precision, so every width is the same instant.
    assert_eq!(
        UtcTimestamp::unix_epoch(TimePrecision::Secs),
        UtcTimestamp::unix_epoch(TimePrecision::Nanos)
    );
    // ... and none of them is the "not set" sentinel.
    assert_ne!(
        UtcTimestamp::unix_epoch(TimePrecision::Secs),
        UtcTimestamp::default()
    );
}

#[test]
fn utc_timestamp_now_keeps_requested_precision() {
    for precision in [
        TimePrecision::Secs,
        TimePrecision::Millis,
        TimePrecision::Micros,
        TimePrecision::Nanos,
    ] {
        assert_eq!(UtcTimestamp::now(precision).precision(), precision);
    }
}

#[test]
fn appl_ver_id_wire_round_trip() {
    for (code, id) in [
        (&b"0"[..], ApplVerId::Fix27),
        (b"1", ApplVerId::Fix30),
        (b"2", ApplVerId::Fix40),
        (b"3", ApplVerId::Fix41),
        (b"4", ApplVerId::Fix42),
        (b"5", ApplVerId::Fix43),
        (b"6", ApplVerId::Fix44),
        (b"7", ApplVerId::Fix50),
        (b"8", ApplVerId::Fix50Sp1),
        (b"9", ApplVerId::Fix50Sp2),
        (b"10", ApplVerId::FixLatest),
    ] {
        assert_eq!(id.as_bytes(), code);
        assert_eq!(ApplVerId::from_bytes(code), Some(id));
        assert_eq!(ApplVerId::from_fix_str(id.as_fix_str()).unwrap(), id);
    }
}

#[test]
fn appl_ver_id_rejects_out_of_codeset() {
    assert!(ApplVerId::from_bytes(b"11").is_none());
    assert!(ApplVerId::from_bytes(b"X").is_none());
    assert!(ApplVerId::from_bytes(b"").is_none());
    let err = ApplVerId::from_fix_str(fix_str!("11")).unwrap_err();
    assert_eq!(err.0.as_utf8(), "11");
    assert_eq!(
        ApplVerId::try_from(fix_str!("42")),
        Err(SessionRejectReasonBase::ValueIsIncorrect),
    );
}

#[test]
fn appl_ver_id_version_bridge() {
    assert_eq!(ApplVerId::Fix50Sp2.to_version(), Version::FIX50SP2);
    assert_eq!(ApplVerId::Fix27.to_version(), Version::FIX27);
    // Base-version projection: Latest lands on its frozen base.
    assert_eq!(ApplVerId::FixLatest.to_version(), Version::FIX_LATEST);
    assert_eq!(ApplVerId::FixLatest.to_version(), Version::FIX50SP2);
}

#[cfg(feature = "serde-deserialize")]
mod utc_timestamp_serde_tests {
    use serde::{
        Deserialize,
        de::value::{Error as DeError, StrDeserializer},
    };

    use super::super::*;

    fn de(input: &str) -> Result<UtcTimestamp, DeError> {
        UtcTimestamp::deserialize(StrDeserializer::<DeError>::new(input))
    }

    #[test]
    fn whole_second_timestamp_round_trips() {
        // A whole-second timestamp serializes without a fraction; reading
        // that form back must succeed and preserve the precision.
        let original = UtcTimestamp::with_secs(Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap());
        let formatted = original.format_precisely().to_string();
        assert_eq!(formatted, "20240102-03:04:05");
        let parsed = de(&formatted).expect("whole-second timestamp rejected");
        assert_eq!(parsed, original);
        assert_eq!(parsed.precision(), TimePrecision::Secs);
    }

    #[test]
    fn fractional_timestamps_round_trip() {
        for (input, precision) in [
            ("20240102-03:04:05.123", TimePrecision::Millis),
            ("20240102-03:04:05.123456", TimePrecision::Micros),
            ("20240102-03:04:05.123456789", TimePrecision::Nanos),
        ] {
            let parsed = de(input).expect("valid timestamp rejected");
            assert_eq!(parsed.precision(), precision);
            assert_eq!(parsed.format_precisely().to_string(), input);
        }
    }

    #[test]
    fn leap_second_survives_whole_second_precision() {
        // chrono represents a leap second as sec=59 with nanos >= 1_000_000_000.
        // Reducing to whole seconds keeps that offset, so :60 stays :60 - it is
        // a valid wire value (TagValue Encoding section 6.2.2, SS = 00-60) and
        // collapsing it to :59 would name a different instant.
        let parsed = de("20231231-23:59:60").expect("leap second rejected");
        assert_eq!(parsed.precision(), TimePrecision::Secs);
        assert_eq!(parsed.format_precisely().to_string(), "20231231-23:59:60");
        assert_ne!(
            parsed,
            UtcTimestamp::with_secs(Utc.with_ymd_and_hms(2023, 12, 31, 23, 59, 59).unwrap())
        );
    }

    #[test]
    fn leap_second_with_millis_is_accepted() {
        let expected = Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(2023, 12, 31)
                .unwrap()
                .and_hms_nano_opt(23, 59, 59, 1_123_000_000)
                .unwrap(),
        );
        let parsed = de("20231231-23:59:60.123").expect("leap second with millis rejected");
        assert_eq!(parsed.timestamp(), expected);
        assert_eq!(parsed.precision(), TimePrecision::Millis);
    }

    #[test]
    fn trailing_garbage_is_rejected() {
        assert!(de("20240102-03:04:05x").is_err());
        assert!(de("20240102-03:04:05.123x").is_err());
        assert!(de("20240102-03:04:05.123\x01").is_err());
    }

    #[test]
    fn overlong_fraction_is_rejected() {
        // A digit count congruent to a valid count modulo 256 must not pass.
        let mut input = String::from("20240102-03:04:05.");
        input.push_str(&"0".repeat(256));
        input.push_str("123");
        assert!(de(&input).is_err());
    }

    #[test]
    fn invalid_values_are_rejected() {
        // Wrong fraction digit count (only 3, 6, 9 and 12 are valid).
        assert!(de("20240102-03:04:05.12").is_err());
        // Empty fraction after the period.
        assert!(de("20240102-03:04:05.").is_err());
        // Nonexistent calendar date.
        assert!(de("20240232-03:04:05").is_err());
        // Second above the leap-second value.
        assert!(de("20240102-03:04:61").is_err());
        // Truncated value.
        assert!(de("20240102-03:04").is_err());
        assert!(de("").is_err());
    }
}

#[cfg(feature = "serde-serialize")]
mod utc_timestamp_serde_ser_tests {
    use super::super::*;

    #[test]
    fn out_of_range_year_fails_to_serialize() {
        // The FIX grammar has a fixed 4-digit year; chrono formats years
        // outside 0000-9999 with a sign and more digits, producing a string
        // the deserializer can never accept. Serialization must fail fast
        // instead of emitting unreadable data. UtcTimestamp::default() is
        // MIN_UTC, so an unfilled timestamp field hits exactly this case.
        assert!(serde_json::to_string(&UtcTimestamp::default()).is_err());
        assert!(serde_json::to_string(&UtcTimestamp::MAX_UTC).is_err());
    }

    #[test]
    fn in_range_timestamp_serializes_to_plain_string() {
        let ts = UtcTimestamp::with_secs(Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap());
        assert_eq!(serde_json::to_string(&ts).unwrap(), "\"20240102-03:04:05\"");
    }
}

#[cfg(all(feature = "serde-serialize", feature = "serde-deserialize"))]
mod time_serde_tests {
    use serde::{
        Deserialize,
        de::value::{Error as DeError, StrDeserializer},
    };

    use super::super::*;

    fn de<'de, T: Deserialize<'de>>(input: &'de str) -> Result<T, DeError> {
        T::deserialize(StrDeserializer::<DeError>::new(input))
    }

    /// Serialize, deserialize, serialize again, and check the string never
    /// changed. The wire form encodes every part of these values - offset
    /// and precision included - so a round-trip that silently widens `Secs`
    /// to `Nanos` or drops an offset shows up here as a different string.
    fn assert_round_trip<T>(value: T, expected: &str)
    where
        T: serde::Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, format!("\"{expected}\""));
        let parsed: T = de(expected).expect("valid value rejected");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
    }

    #[test]
    fn utc_time_only_round_trips_at_every_precision() {
        let time = NaiveTime::from_hms_nano_opt(3, 4, 5, 123_456_789).unwrap();
        assert_round_trip(UtcTimeOnly::with_secs(time), "03:04:05");
        assert_round_trip(UtcTimeOnly::with_millis(time), "03:04:05.123");
        assert_round_trip(UtcTimeOnly::with_micros(time), "03:04:05.123456");
        assert_round_trip(UtcTimeOnly::with_nanos(time), "03:04:05.123456789");
    }

    #[test]
    fn tz_timestamp_round_trips_every_offset_form() {
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
            .unwrap()
            .and_hms_opt(7, 39, 0)
            .unwrap();
        for (offset_secs, expected) in [
            (0, "20060901-07:39:00Z"),
            (3600, "20060901-07:39:00+01"),
            (-3600, "20060901-07:39:00-01"),
            (5400, "20060901-07:39:00+01:30"),
            (-19_800, "20060901-07:39:00-05:30"),
        ] {
            let offset = FixedOffset::east_opt(offset_secs).unwrap();
            let value = TzTimestamp::with_secs(offset.from_local_datetime(&naive).unwrap());
            assert_round_trip(value, expected);
        }
    }

    #[test]
    fn tz_timestamp_round_trips_at_every_precision() {
        let offset = FixedOffset::east_opt(3600).unwrap();
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
            .unwrap()
            .and_hms_nano_opt(7, 39, 0, 123_456_789)
            .unwrap();
        let timestamp = offset.from_local_datetime(&naive).unwrap();
        // Precision selects how much of the fraction reaches the wire; the
        // Secs form drops it entirely.
        assert_round_trip(TzTimestamp::with_secs(timestamp), "20060901-07:39:00+01");
        assert_round_trip(
            TzTimestamp::with_millis(timestamp),
            "20060901-07:39:00.123+01",
        );
        assert_round_trip(
            TzTimestamp::with_micros(timestamp),
            "20060901-07:39:00.123456+01",
        );
        assert_round_trip(
            TzTimestamp::with_nanos(timestamp),
            "20060901-07:39:00.123456789+01",
        );
    }

    #[test]
    fn tz_time_only_round_trips_at_every_precision() {
        let time = NaiveTime::from_hms_nano_opt(7, 39, 0, 123_456_789).unwrap();
        let offset = FixedOffset::east_opt(-18_000).unwrap();
        assert_round_trip(TzTimeOnly::with_secs(time, offset), "07:39:00-05");
        assert_round_trip(TzTimeOnly::with_millis(time, offset), "07:39:00.123-05");
        assert_round_trip(TzTimeOnly::with_micros(time, offset), "07:39:00.123456-05");
        assert_round_trip(
            TzTimeOnly::with_nanos(time, offset),
            "07:39:00.123456789-05",
        );
    }

    /// The zoned grammar caps seconds at `SS = 00-59` (TagValue Encoding
    /// section 6.2.2), unlike the UTC forms which allow `60` for a leap
    /// second. chrono renders the leap as `:60` regardless of the type, so
    /// serialization must refuse rather than emit a field the grammar forbids
    /// and `parse_tz_timestamp` would reject.
    #[test]
    fn tz_types_refuse_to_serialize_a_leap_second() {
        let offset = FixedOffset::east_opt(3600).unwrap();
        let leap_time = NaiveTime::from_hms_nano_opt(23, 59, 59, 1_000_000_000).unwrap();
        let leap_naive = NaiveDate::from_ymd_opt(2016, 12, 31)
            .unwrap()
            .and_time(leap_time);
        let leap_ts = TzTimestamp::with_nanos(offset.from_local_datetime(&leap_naive).unwrap());

        assert!(
            serde_json::to_string(&leap_ts).is_err(),
            "leap second must not reach the wire as a TZTimestamp"
        );
        assert!(
            serde_json::to_string(&TzTimeOnly::with_nanos(leap_time, offset)).is_err(),
            "leap second must not reach the wire as a TZTimeOnly"
        );

        // The UTC forms do carry it - the same section allows SS = 60 there.
        let utc_leap = UtcTimeOnly::with_nanos(leap_time);
        assert_eq!(
            serde_json::to_string(&utc_leap).unwrap(),
            "\"23:59:60.000000000\""
        );
    }

    /// The leap offset survives the precision reduction instead of being
    /// cleared: dropping it would move the instant to `:59`, a different
    /// value. Serialization refuses the result; construction does not corrupt
    /// it.
    #[test]
    fn tz_reduction_to_whole_seconds_keeps_the_leap_offset() {
        let offset = FixedOffset::east_opt(3600).unwrap();
        let leap_time = NaiveTime::from_hms_nano_opt(23, 59, 59, 1_500_000_000).unwrap();

        let tz_time_only = TzTimeOnly::with_secs(leap_time, offset);
        assert_eq!(tz_time_only.timestamp().second(), 59);
        assert_eq!(tz_time_only.timestamp().nanosecond(), 1_000_000_000);

        let leap_naive = NaiveDate::from_ymd_opt(2016, 12, 31)
            .unwrap()
            .and_time(leap_time);
        let tz_timestamp = TzTimestamp::with_secs(offset.from_local_datetime(&leap_naive).unwrap());
        assert_eq!(tz_timestamp.timestamp().nanosecond(), 1_000_000_000);
    }

    /// `TzTimestamp::with_*` used to only tag the precision, leaving the
    /// sub-precision digits in the value - so a value built in-process was not
    /// equal to the same value parsed back from the bytes it produced.
    #[test]
    fn tz_timestamp_construction_truncates_to_the_stated_precision() {
        let offset = FixedOffset::east_opt(3600).unwrap();
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
            .unwrap()
            .and_hms_nano_opt(7, 39, 0, 123_456_789)
            .unwrap();
        let timestamp = offset.from_local_datetime(&naive).unwrap();

        assert_eq!(
            TzTimestamp::with_secs(timestamp).timestamp().nanosecond(),
            0
        );
        assert_eq!(
            TzTimestamp::with_millis(timestamp).timestamp().nanosecond(),
            123_000_000
        );
        assert_eq!(
            TzTimestamp::with_micros(timestamp).timestamp().nanosecond(),
            123_456_000
        );

        // What was constructed equals what the wire form parses back to.
        for value in [
            TzTimestamp::with_secs(timestamp),
            TzTimestamp::with_millis(timestamp),
            TzTimestamp::with_micros(timestamp),
            TzTimestamp::with_nanos(timestamp),
        ] {
            let encoded = serde_json::to_string(&value).unwrap();
            let parsed: TzTimestamp = serde_json::from_str(&encoded).expect("round trip");
            assert_eq!(parsed.timestamp(), value.timestamp());
            assert_eq!(parsed.precision(), value.precision());
        }
    }

    /// The runtime-precision constructors must truncate exactly like the
    /// fixed-precision ones they stand in for - they are what a caller reaches
    /// for when the width comes from configuration rather than a literal.
    #[test]
    fn tz_runtime_precision_constructors_match_the_fixed_ones() {
        let offset = FixedOffset::east_opt(3600).unwrap();
        let time = NaiveTime::from_hms_nano_opt(7, 39, 0, 123_456_789).unwrap();
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1).unwrap().and_time(time);
        let timestamp = offset.from_local_datetime(&naive).unwrap();

        for (precision, fixed_ts, fixed_time_only) in [
            (
                TimePrecision::Secs,
                TzTimestamp::with_secs(timestamp),
                TzTimeOnly::with_secs(time, offset),
            ),
            (
                TimePrecision::Millis,
                TzTimestamp::with_millis(timestamp),
                TzTimeOnly::with_millis(time, offset),
            ),
            (
                TimePrecision::Micros,
                TzTimestamp::with_micros(timestamp),
                TzTimeOnly::with_micros(time, offset),
            ),
            (
                TimePrecision::Nanos,
                TzTimestamp::with_nanos(timestamp),
                TzTimeOnly::with_nanos(time, offset),
            ),
        ] {
            let runtime_ts = TzTimestamp::with_precision(timestamp, precision);
            assert_eq!(runtime_ts.timestamp(), fixed_ts.timestamp());
            assert_eq!(runtime_ts.precision(), fixed_ts.precision());

            let runtime_time_only = TzTimeOnly::new(time, offset, precision);
            assert_eq!(runtime_time_only.timestamp(), fixed_time_only.timestamp());
            assert_eq!(runtime_time_only.precision(), fixed_time_only.precision());
        }
    }

    #[test]
    fn tz_time_only_accepts_the_wire_form_without_seconds() {
        // The grammar makes :SS optional on input; the value defaults to
        // whole seconds, which is what the output form then carries.
        let parsed: TzTimeOnly = de("07:39Z").expect("valid value rejected");
        assert_eq!(
            parsed.timestamp(),
            NaiveTime::from_hms_opt(7, 39, 0).unwrap()
        );
        assert_eq!(parsed.precision(), TimePrecision::Secs);
        assert_eq!(parsed.offset(), FixedOffset::east_opt(0).unwrap());
    }

    #[test]
    fn tz_timestamp_accepts_the_wire_form_without_seconds() {
        // The grammar in TagValue Encoding section 6.2.2 shows SS, but every
        // example in that table omits it - including the spec's own
        // "20060901-07:39Z". Output always carries seconds.
        let parsed: TzTimestamp = de("20060901-07:39Z").expect("valid value rejected");
        assert_eq!(parsed.precision(), TimePrecision::Secs);
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            "\"20060901-07:39:00Z\""
        );
        let with_fraction: TzTimestamp =
            de("20060901-13:09.123+05:30").expect("valid value rejected");
        assert_eq!(
            serde_json::to_string(&with_fraction).unwrap(),
            "\"20060901-13:09:00.123+05:30\""
        );
    }

    #[test]
    fn sub_minute_offset_fails_to_serialize() {
        // The wire form of an offset carries whole minutes only, so rendering
        // such a value would silently drop the seconds part.
        let offset = FixedOffset::east_opt(45).unwrap();
        let time = NaiveTime::from_hms_opt(7, 39, 0).unwrap();
        assert!(serde_json::to_string(&TzTimeOnly::with_secs(time, offset)).is_err());
        let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
            .unwrap()
            .and_hms_opt(7, 39, 0)
            .unwrap();
        let value = TzTimestamp::with_secs(offset.from_local_datetime(&naive).unwrap());
        assert!(serde_json::to_string(&value).is_err());
    }

    #[test]
    fn out_of_range_year_fails_to_serialize() {
        // Same guard as UtcTimestamp: chrono renders such years with a sign
        // and extra digits, producing a string no deserializer accepts.
        let naive = NaiveDate::from_ymd_opt(-1, 9, 1)
            .unwrap()
            .and_hms_opt(7, 39, 0)
            .unwrap();
        let offset = FixedOffset::east_opt(0).unwrap();
        let value = TzTimestamp::with_secs(offset.from_local_datetime(&naive).unwrap());
        assert!(serde_json::to_string(&value).is_err());
    }

    #[test]
    fn malformed_values_are_rejected() {
        // Missing offset.
        assert!(de::<TzTimestamp>("20060901-07:39:00").is_err());
        assert!(de::<TzTimeOnly>("07:39:00").is_err());
        // Leap second - TZ values carry none.
        assert!(de::<TzTimestamp>("20060901-07:39:60Z").is_err());
        // Offset out of range.
        assert!(de::<TzTimestamp>("20060901-07:39:00+99").is_err());
        // Trailing garbage - the whole input must be consumed.
        assert!(de::<TzTimestamp>("20060901-07:39:00Zx").is_err());
        assert!(de::<TzTimeOnly>("07:39:00Z\x01").is_err());
        assert!(de::<UtcTimeOnly>("03:04:05x").is_err());
        // Truncated.
        assert!(de::<TzTimestamp>("20060901-07:39").is_err());
        assert!(de::<TzTimeOnly>("07:39:00+0").is_err());
        assert!(de::<UtcTimeOnly>("03:04").is_err());
        assert!(de::<TzTimestamp>("").is_err());
        assert!(de::<TzTimeOnly>("").is_err());
        assert!(de::<UtcTimeOnly>("").is_err());
    }
}

#[cfg(feature = "serde-deserialize")]
mod tenor_serde_de_tests {
    use serde::{
        Deserialize,
        de::value::{Error as DeError, StrDeserializer},
    };

    use super::super::*;

    fn de(input: &str) -> Result<Tenor, DeError> {
        Tenor::deserialize(StrDeserializer::<DeError>::new(input))
    }

    #[test]
    fn every_unit_round_trips() {
        for (input, unit, value) in [
            ("D5", TenorUnit::Days, 5),
            ("M3", TenorUnit::Months, 3),
            ("W13", TenorUnit::Weeks, 13),
            ("Y1", TenorUnit::Years, 1),
        ] {
            let parsed = de(input).expect("valid tenor rejected");
            assert_eq!(
                parsed,
                Tenor {
                    unit,
                    value: TenorValue::new(value).unwrap()
                }
            );
        }
    }

    #[test]
    fn malformed_values_are_rejected() {
        // Unknown unit code.
        assert!(de("X5").is_err());
        // Unit without a value.
        assert!(de("D").is_err());
        // Zero value - rejected on the wire as well.
        assert!(de("D0").is_err());
        // Value before the unit.
        assert!(de("5D").is_err());
        // Trailing garbage - the whole input must be consumed.
        assert!(de("D5x").is_err());
        assert!(de("D5\x01").is_err());
        // Value above the u16 range of TenorValue.
        assert!(de("D65536").is_err());
        assert!(de("").is_err());
    }
}

#[cfg(feature = "serde-serialize")]
mod tenor_serde_ser_tests {
    use super::super::*;

    #[test]
    fn tenor_serializes_to_wire_string() {
        for (unit, value, expected) in [
            (TenorUnit::Days, 5, "\"D5\""),
            (TenorUnit::Months, 3, "\"M3\""),
            (TenorUnit::Weeks, 13, "\"W13\""),
            (TenorUnit::Years, 1, "\"Y1\""),
        ] {
            let tenor = Tenor {
                unit,
                value: TenorValue::new(value).unwrap(),
            };
            assert_eq!(serde_json::to_string(&tenor).unwrap(), expected);
        }
    }
}

#[cfg(all(feature = "serde-serialize", feature = "serde-deserialize"))]
mod decimal_serde_tests {
    use std::str::FromStr;

    use super::super::*;

    #[test]
    fn decimal_is_a_string_that_keeps_its_scale() {
        // The `rust_decimal/serde` representation must stay string-based:
        // switching it to a float would drop trailing zeros and round values
        // that FIX carries exactly.
        let price = Decimal::from_str("97.0340").unwrap();
        let json = serde_json::to_string(&price).unwrap();
        assert_eq!(json, "\"97.0340\"");
        let parsed: Decimal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, price);
        assert_eq!(parsed.scale(), price.scale());
    }

    #[test]
    fn decimal_accepts_a_json_number() {
        let parsed: Decimal = serde_json::from_str("97.0347").unwrap();
        assert_eq!(parsed, Decimal::from_str("97.0347").unwrap());
    }
}
