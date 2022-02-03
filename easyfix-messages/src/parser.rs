use nom::{
    bytes::streaming::tag,
    bytes::streaming::{take_until, take_while},
    character::{
        is_alphanumeric,
        streaming::{u16, u8},
    },
    combinator::map,
    error::{context, ContextError, ParseError},
    multi::length_data,
    sequence::{delimited, separated_pair, terminated, tuple},
    IResult,
};

pub use nom::Err;

pub fn generic_field<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
    i: &'a [u8],
) -> IResult<&'a [u8], (u16, &[u8]), E> {
    terminated(
        separated_pair(u16, tag("="), take_until("\x01")),
        tag("\x01"),
    )(i)
}

fn begin_string<'a, E: ParseError<&'a [u8]> + ContextError<&'a [u8]>>(
    i: &'a [u8],
) -> IResult<&'a [u8], &[u8], E> {
    context(
        "begin_string",
        delimited(tag("8="), take_until("\x01"), tag("\x01")),
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
    pub begin_string: &'a [u8],
    pub body: &'a [u8],
    pub checksum: u8,
}

pub fn raw_message<'a>(i: &'a [u8]) -> IResult<&'a [u8], RawMessage<'a>> {
    map(
        tuple((begin_string, length_data(body_length), checksum)),
        |(begin_string, body, checksum)| RawMessage {
            begin_string,
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
