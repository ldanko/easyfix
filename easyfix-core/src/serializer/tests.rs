use std::str;

use assert_matches::assert_matches;

use super::{SerializeError, Serializer, max_body_len_digits};
use crate::basic_types::{
    FixedOffset, NaiveDate, NaiveTime, TimeZone, TzTimeOnly, TzTimestamp, Utc, UtcTimeOnly,
    UtcTimestamp,
};

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
#[expect(
    clippy::cast_possible_truncation,
    reason = "reduced mod 256 on the line, so the value fits u8 by construction"
)]
fn expected_checksum(bytes: &[u8]) -> u8 {
    (bytes.iter().map(|&b| b as u32).sum::<u32>() % 256) as u8
}

/// Split a framed message into (everything before `10=`, the CheckSum
/// field). The CheckSum field is always exactly `10=NNN\x01`.
fn split_checksum(msg: &[u8]) -> (&[u8], &[u8]) {
    msg.split_at(msg.len() - 7)
}

/// Serialize a single value and return the bytes it wrote.
fn serialize_value(f: impl FnOnce(&mut Serializer) -> Result<(), SerializeError>) -> Vec<u8> {
    let mut buf = [0u8; 64];
    let mut serializer = Serializer::new(&mut buf);
    f(&mut serializer).unwrap();
    serializer.written().to_vec()
}

/// A zero `SeqNum` reaches the wire. `EndSeqNo(16)` uses it for "no upper
/// bound" - the form `SessionEngine::process_resend_request` normalizes on
/// receipt - and `LastMsgSeqNumProcessed(369)` for "nothing processed yet".
/// The deserializer has always accepted it, so refusing to write it left the
/// codec able to read messages it could not produce.
///
/// The relaxation is specific to `SeqNum`: the neighbouring digit types keep
/// their guards, since a zero `TagNum` or a zero-entry repeating group has no
/// valid wire form at all.
#[test]
fn seq_num_zero_is_written_while_the_other_digit_types_still_reject_it() {
    assert_eq!(serialize_value(|s| s.serialize_seq_num(&0)), b"0");
    assert_eq!(serialize_value(|s| s.serialize_seq_num(&1)), b"1");

    let mut buf = [0u8; 64];
    let mut serializer = Serializer::new(&mut buf);
    assert_matches!(
        serializer.serialize_tag_num(&0),
        Err(SerializeError::InvalidValue)
    );
    assert_matches!(
        serializer.serialize_num_in_group(&0),
        Err(SerializeError::InvalidValue)
    );
}

#[test]
fn utc_time_only_is_written_with_the_precision_it_carries() {
    let time = NaiveTime::from_hms_nano_opt(3, 4, 5, 123_456_789).unwrap();
    for (value, expected) in [
        (UtcTimeOnly::with_secs(time), "03:04:05"),
        (UtcTimeOnly::with_millis(time), "03:04:05.123"),
        (UtcTimeOnly::with_micros(time), "03:04:05.123456"),
        (UtcTimeOnly::with_nanos(time), "03:04:05.123456789"),
    ] {
        let written = serialize_value(|s| s.serialize_utc_time_only(&value));
        assert_eq!(written, expected.as_bytes());
    }
}

#[test]
fn tz_timestamp_offsets_use_the_wire_form() {
    let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_opt(7, 39, 0)
        .unwrap();
    for (offset_secs, expected) in [
        (0, "20060901-07:39:00Z"),
        (3600, "20060901-07:39:00+01"),
        (-3600, "20060901-07:39:00-01"),
        (5400, "20060901-07:39:00+01:30"),
        (-5400, "20060901-07:39:00-01:30"),
    ] {
        let offset = FixedOffset::east_opt(offset_secs).unwrap();
        let value = TzTimestamp::with_secs(offset.from_local_datetime(&naive).unwrap());
        let written = serialize_value(|s| s.serialize_tz_timestamp(&value));
        assert_eq!(written, expected.as_bytes());
    }
}

#[test]
fn values_outside_the_wire_grammar_fail_instead_of_corrupting_the_message() {
    // BodyLength and CheckSum are computed over whatever was written, so
    // an unrepresentable value would travel inside a well-formed message
    // and only surface as a reject at the counterparty.
    let out_of_range_year = NaiveDate::from_ymd_opt(10_000, 9, 1)
        .unwrap()
        .and_hms_opt(7, 39, 0)
        .unwrap();
    let utc = Utc.from_utc_datetime(&out_of_range_year);
    let mut buf = [0u8; 64];
    let mut serializer = Serializer::new(&mut buf);
    assert_matches!(
        serializer.serialize_utc_timestamp(&UtcTimestamp::with_secs(utc)),
        Err(SerializeError::InvalidValue)
    );
    assert_matches!(
        serializer.serialize_utc_date_only(&out_of_range_year.date()),
        Err(SerializeError::InvalidValue)
    );
    assert_matches!(
        serializer.serialize_local_mkt_date(&out_of_range_year.date()),
        Err(SerializeError::InvalidValue)
    );

    let utc_offset = FixedOffset::east_opt(0).unwrap();
    assert_matches!(
        serializer.serialize_tz_timestamp(&TzTimestamp::with_secs(
            utc_offset.from_local_datetime(&out_of_range_year).unwrap()
        )),
        Err(SerializeError::InvalidValue)
    );

    // The wire form of an offset carries whole minutes only.
    let sub_minute = FixedOffset::east_opt(45).unwrap();
    let time = NaiveTime::from_hms_opt(7, 39, 0).unwrap();
    assert_matches!(
        serializer.serialize_tz_timeonly(&TzTimeOnly::with_secs(time, sub_minute)),
        Err(SerializeError::InvalidValue)
    );
    let naive = NaiveDate::from_ymd_opt(2006, 9, 1)
        .unwrap()
        .and_hms_opt(7, 39, 0)
        .unwrap();
    assert_matches!(
        serializer.serialize_tz_timestamp(&TzTimestamp::with_secs(
            sub_minute.from_local_datetime(&naive).unwrap()
        )),
        Err(SerializeError::InvalidValue)
    );

    // The zoned grammar caps seconds at 00-59, so a leap second - which
    // the UTC forms do carry - has no representation here. chrono renders
    // it as `:60` regardless, and `parse_tz_timestamp` rejects that.
    let leap_time = NaiveTime::from_hms_nano_opt(23, 59, 59, 1_000_000_000).unwrap();
    let leap_naive = NaiveDate::from_ymd_opt(2016, 12, 31)
        .unwrap()
        .and_time(leap_time);
    assert_matches!(
        serializer.serialize_tz_timestamp(&TzTimestamp::with_nanos(
            utc_offset.from_local_datetime(&leap_naive).unwrap()
        )),
        Err(SerializeError::InvalidValue)
    );
    assert_matches!(
        serializer.serialize_tz_timeonly(&TzTimeOnly::with_nanos(leap_time, utc_offset)),
        Err(SerializeError::InvalidValue)
    );
    // The UTC counterpart is representable and must still go through.
    assert_eq!(
        serialize_value(|s| s.serialize_utc_time_only(&UtcTimeOnly::with_secs(leap_time))),
        b"23:59:60"
    );
}

#[test]
fn tz_time_only_is_written_with_the_precision_it_carries() {
    let time = NaiveTime::from_hms_nano_opt(7, 39, 0, 123_456_789).unwrap();
    let offset = FixedOffset::east_opt(-18_000).unwrap();
    for (value, expected) in [
        (TzTimeOnly::with_secs(time, offset), "07:39:00-05"),
        (TzTimeOnly::with_millis(time, offset), "07:39:00.123-05"),
        (TzTimeOnly::with_micros(time, offset), "07:39:00.123456-05"),
        (
            TzTimeOnly::with_nanos(time, offset),
            "07:39:00.123456789-05",
        ),
    ] {
        let written = serialize_value(|s| s.serialize_tz_timeonly(&value));
        assert_eq!(written, expected.as_bytes());
    }
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
        str::from_utf8(head).unwrap(),
        expected_checksum(head)
    );
    assert_eq!(&buf[..len], expected.as_bytes());
}
