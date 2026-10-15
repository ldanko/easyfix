use easyfix_core::{basic_types::FixStr, fix_str};

const VALUE: &FixStr = fix_str!("123");
static RAW: &FixStr = fix_str!(r#"raw "text""#);

fn main() {
    assert_eq!(VALUE.as_bytes(), b"123");
    assert_eq!(RAW.as_bytes(), b"raw \"text\"");
    assert_eq!(fix_str!("").as_bytes(), b"");
    assert_eq!(fix_str!(" ~").as_bytes(), b" ~");
    assert_eq!(fix_str!("\x41").as_bytes(), b"A");
}
