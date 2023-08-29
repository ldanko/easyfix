#![feature(proc_macro_diagnostic)]

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, LitStr};

const fn is_non_control_ascii_char(byte: u8) -> bool {
    byte > 0x1f && byte < 0x80
}

#[proc_macro]
pub fn fix_str(ts: TokenStream) -> TokenStream {
    let input = parse_macro_input!(ts as LitStr);

    input.value();

    for (i, c) in input.value().bytes().enumerate() {
        if !is_non_control_ascii_char(c) {
            input
                .span()
                .unwrap()
                .error(format!("wrong byte found at position {i}"))
                .emit();
            return TokenStream::new();
        }
    }

    quote! {
      unsafe { easyfix_messages::fields::FixStr::from_ascii_unchecked(#input.as_bytes()) }
    }
    .into()
}
