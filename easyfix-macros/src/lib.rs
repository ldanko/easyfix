#![feature(proc_macro_diagnostic)]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::quote;
use syn::{Ident, LitStr, parse_macro_input};

const fn is_non_control_ascii_char(byte: u8) -> bool {
    byte > 0x1f && byte < 0x80
}

fn find_easyfix_core_path() -> proc_macro2::TokenStream {
    if let Ok(found) = crate_name("easyfix-core") {
        match found {
            // Use `::easyfix_core` even for the "Itself" case. This requires
            // `extern crate self as easyfix_core;` in easyfix-core's lib.rs,
            // but makes the macro work in examples/tests (which are separate
            // binary crate roots where `crate` doesn't point to easyfix_core).
            FoundCrate::Itself => quote!(::easyfix_core),
            FoundCrate::Name(name) => {
                let ident = Ident::new(&name, Span::call_site());
                quote!(::#ident)
            }
        }
    } else if let Ok(found) = crate_name("easyfix") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = Ident::new(&name, Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        panic!(
            "Could not find `easyfix-core` or `easyfix` in Cargo.toml. Add one of them as a dependency."
        );
    }
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

    let easyfix_path = find_easyfix_core_path();

    quote! {
      unsafe { #easyfix_path::basic_types::FixStr::from_ascii_unchecked(#input.as_bytes()) }
    }
    .into()
}
