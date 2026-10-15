//! Compile-time macros for easyfix.
//!
//! Literal validation used by `easyfix_core::fix_str!`.

#![feature(proc_macro_diagnostic)]

use proc_macro::TokenStream;
use quote::quote;
use syn::{LitStr, parse_macro_input};

const fn is_non_control_ascii_char(byte: u8) -> bool {
    byte > 0x1f && byte < 0x7f
}

/// Validates a printable ASCII string literal and returns that literal.
#[doc(hidden)]
#[proc_macro]
pub fn validate_fix_str(ts: TokenStream) -> TokenStream {
    let input = parse_macro_input!(ts as LitStr);

    for (i, c) in input.value().bytes().enumerate() {
        if !is_non_control_ascii_char(c) {
            input
                .span()
                .unwrap()
                .error(format!("wrong byte found at position {i}"))
                .emit();
            // Keep the expansion a valid expression after emitting the error,
            // so the wrapper does not add an unrelated syntax error.
            return quote!("").into();
        }
    }

    quote!(#input).into()
}
