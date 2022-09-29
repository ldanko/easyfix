pub use nom::Err;
use nom::{
    bytes::streaming::{tag, take_until, take_while},
    character::{
        is_alphanumeric,
        streaming::{u16, u8},
    },
    combinator::{map, verify},
    error::{context, ContextError, ParseError},
    multi::length_data,
    sequence::{delimited, separated_pair, terminated, tuple},
    IResult,
};

use crate::fields::FixStr;

// TODO: Don't use Nom, no need to use additional dependency for RawMessage processing.

pub fn generic_field<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
    i: &'a [u8],
) -> IResult<&'a [u8], (u16, &[u8]), E> {
    terminated(
        separated_pair(u16, tag("="), take_until("\x01")),
        tag("\x01"),
    )(i)
}

fn verify_fix_str(bytes: &[u8]) -> bool {
    for b in bytes {
        // No control character is allowed
        if let 0x00..=0x1f | 0x80..=0xff = b {
            return false;
        }
    }
    true
}

fn begin_string<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
    i: &'a [u8],
) -> IResult<&'a [u8], &[u8], E> {
    context(
        "begin_string",
        delimited(
            tag("8="),
            verify(take_until("\x01"), verify_fix_str),
            tag("\x01"),
        ),
    )(i)
}

fn checksum<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
    i: &'a [u8],
) -> IResult<&[u8], u8, E> {
    context("checksum", delimited(tag("10="), u8, tag("\x01")))(i)
}

fn body_length<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
    i: &'a [u8],
) -> IResult<&[u8], u16, E> {
    delimited(tag("9="), u16, tag("\x01"))(i)
}

fn _message_type<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
    i: &'a [u8],
) -> IResult<&[u8], &[u8], E> {
    context(
        "message_type",
        delimited(tag("35="), take_while(is_alphanumeric), tag("\x01")),
    )(i)
}

#[derive(Debug)]
pub struct RawMessage<'a> {
    pub begin_string: &'a FixStr,
    pub body: &'a [u8],
    pub checksum: u8,
}

pub fn raw_message<'a>(i: &'a [u8]) -> IResult<&'a [u8], RawMessage<'a>> {
    map(
        tuple((begin_string, length_data(body_length), checksum)),
        |(begin_string, body, checksum)| RawMessage {
            // SAFETY: it's already checked
            begin_string: unsafe { FixStr::from_ascii_unchecked(begin_string) },
            body,
            checksum,
        },
    )(i)
}

#[cfg(test)]
mod tests {
    use nom::Err::Incomplete;

    use super::raw_message;

    #[test]
    fn parse_complete_ok() {
        let input = b"8=MSG_BODY\x019=19\x01<lots of tags here>10=015\x01";
        assert!(raw_message(input).is_ok());
    }

    #[test]
    fn parse_from_chunks_ok() {
        let input = &[
            b"8=MSG_BOD".as_slice(),
            b"Y\x019=19\x01<lots".as_slice(),
            b" of tags here>10=015\x01".as_slice(),
            b"leftover".as_slice(),
        ];
        let mut buf = Vec::new();
        let mut i = input.iter();
        {
            buf.extend_from_slice(i.next().unwrap());
            assert!(matches!(raw_message(&buf), Err(Incomplete(_))));
        }
        {
            buf.extend_from_slice(i.next().unwrap());
            assert!(matches!(raw_message(&buf), Err(Incomplete(_))));
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
}
