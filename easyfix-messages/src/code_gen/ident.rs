//! Conversion of dictionary names into Rust identifiers for generated code.
//!
//! Dictionary names are `FixStr`, so the input is contractually printable
//! ASCII; `convert_case` strips separators and punctuation, leaving only
//! alphanumerics. The remaining ways a converted name can fail to be a
//! valid identifier - a leading digit or a keyword collision - are fixed
//! up here, uniformly for every identifier kind.

use std::{collections::HashSet, sync::LazyLock};

use convert_case::{Case, Casing};
use easyfix_core::basic_types::FixStr;
use proc_macro2::{Ident, Span};

/// Conversion of a dictionary name into a Rust identifier.
pub trait ToIdent {
    /// Snake-case identifier, for struct fields and modules.
    fn to_snake_ident(&self) -> Ident;

    /// UpperCamel identifier, for types and enum variants.
    fn to_pascal_ident(&self) -> Ident;
}

impl ToIdent for FixStr {
    fn to_snake_ident(&self) -> Ident {
        ident_with_fixups(self.as_utf8().to_case(Case::Snake))
    }

    fn to_pascal_ident(&self) -> Ident {
        ident_with_fixups(self.as_utf8().to_case(Case::UpperCamel))
    }
}

/// Rust keywords (all editions) plus reserved and weak keywords that cannot
/// be used as identifiers.
static RESERVED: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "Self",
        "_",
        "abstract",
        "as",
        "async",
        "await",
        "become",
        "box",
        "break",
        "const",
        "continue",
        "crate",
        "do",
        "dyn",
        "else",
        "enum",
        "extern",
        "false",
        "final",
        "fn",
        "for",
        "gen",
        "if",
        "impl",
        "in",
        "let",
        "loop",
        "macro",
        "macro_rules",
        "match",
        "mod",
        "move",
        "mut",
        "override",
        "priv",
        "pub",
        "raw",
        "ref",
        "return",
        "safe",
        "self",
        "static",
        "struct",
        "super",
        "trait",
        "true",
        "try",
        "type",
        "typeof",
        "union",
        "unsafe",
        "unsized",
        "use",
        "virtual",
        "where",
        "while",
        "yield",
    ])
});

fn ident_with_fixups(mut name: String) -> Ident {
    if name.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        name.insert(0, '_');
    }
    if RESERVED.contains(name.as_str()) {
        // TODO: or maybe `r#reserved`?
        name.push('_');
    }
    Ident::new(&name, Span::call_site())
}

#[cfg(test)]
mod tests {
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
}
