//! Compile-time macros for easyfix.
//!
//! Currently just [`fix_str!`], which turns a string literal into a
//! `&'static FixStr` after validating it at expansion time.

#![feature(proc_macro_diagnostic)]

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::Span;
use quote::quote;
use syn::{Ident, LitStr, parse_macro_input};

const fn is_non_control_ascii_char(byte: u8) -> bool {
    byte > 0x1f && byte < 0x7f
}

// Crates that expose `basic_types` at their root, so the expansion can name
// `FixStr` through any of them. Whichever the calling crate depends on hosts
// the expansion; with several present the first match wins and all resolve
// to the same type.
const HOST_CRATES: [&str; 3] = ["easyfix-core", "easyfix", "easyfix-session"];

fn find_easyfix_core_path() -> proc_macro2::TokenStream {
    for host in HOST_CRATES {
        let Ok(found) = crate_name(host) else {
            continue;
        };
        return match found {
            // Use `::easyfix_core` even for the "Itself" case. This requires
            // `extern crate self as easyfix_core;` in easyfix-core's lib.rs,
            // but makes the macro work in examples/tests (which are separate
            // binary crate roots where `crate` doesn't point to easyfix_core).
            FoundCrate::Itself if host == "easyfix-core" => quote!(::easyfix_core),
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = Ident::new(&name, Span::call_site());
                quote!(::#ident)
            }
        };
    }
    panic!(
        "Could not find `easyfix-core`, `easyfix` or `easyfix-session` in Cargo.toml. \
         Add one of them as a dependency."
    );
}

/// Builds a `&'static FixStr` from a string literal.
///
/// Every byte is checked at expansion time to be printable ASCII
/// (`0x20`-`0x7e`); a literal containing anything else is a compile error
/// naming the offending position. Usable in `const` context and in ordinary
/// expressions alike, so it is the way to write a `FixStr` literal - reach
/// for `FixStr::from_ascii_unchecked` only for runtime bytes, which this
/// macro cannot see.
///
/// The expansion names the `easyfix-core` types through whichever of
/// `easyfix-core`, `easyfix` or `easyfix-session` the calling crate depends
/// on; one of the three must be in its `Cargo.toml`.
///
/// ```
/// # use easyfix_core::{basic_types::FixStr, fix_str};
/// const BEGIN_STRING: &FixStr = fix_str!("FIXT.1.1");
/// assert_eq!(BEGIN_STRING.as_utf8(), "FIXT.1.1");
/// ```
///
/// ```compile_fail
/// # use easyfix_core::fix_str;
/// // SOH is a control character - rejected at compile time.
/// let bad = fix_str!("FIXT\x01");
/// ```
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

    // SAFETY (of the emitted `unsafe`): the literal's bytes were validated
    // as non-control ASCII by the loop above, at macro expansion time.
    quote! {
      unsafe { #easyfix_path::basic_types::FixStr::from_ascii_unchecked(#input.as_bytes()) }
    }
    .into()
}
