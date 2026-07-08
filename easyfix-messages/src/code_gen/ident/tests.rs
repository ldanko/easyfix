use easyfix_core::fix_str;

use super::ToIdent;

#[test]
fn snake_ident() {
    assert_eq!(fix_str!("MsgType").to_snake_ident(), "msg_type");
    assert_eq!(fix_str!("SenderCompID").to_snake_ident(), "sender_comp_id");
}

#[test]
fn pascal_ident() {
    assert_eq!(
        fix_str!("NewOrderSingle").to_pascal_ident(),
        "NewOrderSingle"
    );
    assert_eq!(
        fix_str!("LOGONS_ARE_NOT_ALLOWED_AT_THIS_TIME").to_pascal_ident(),
        "LogonsAreNotAllowedAtThisTime"
    );
}

#[test]
fn keyword_escaped() {
    assert_eq!(fix_str!("Yield").to_snake_ident(), "yield_");
    assert_eq!(fix_str!("SELF").to_pascal_ident(), "Self_");
}

#[test]
fn leading_digit_escaped() {
    assert_eq!(
        fix_str!("1ST_FUTURE_ROLL").to_pascal_ident(),
        "_1StFutureRoll"
    );
    assert_eq!(fix_str!("1stField").to_snake_ident(), "_1_st_field");
}
