use convert_case::{Case, Casing};
use easyfix_dictionary::Variant;
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use super::member::EnumerableType;

fn variant_ident(name: &str) -> Ident {
    let mut variant_name = name.to_case(Case::UpperCamel);
    if variant_name.as_bytes()[0].is_ascii_digit() {
        variant_name.insert(0, '_');
    }
    Ident::new(&variant_name, Span::call_site())
}

pub struct EnumCodeGen {
    name: Ident,
    tag: u16,
    enumerable_type: EnumerableType,
    variants: Vec<Variant>,
}
impl EnumCodeGen {
    pub fn new(
        name: &str,
        tag: u16,
        enumerable_type: EnumerableType,
        variants: Vec<Variant>,
    ) -> EnumCodeGen {
        EnumCodeGen {
            name: Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            tag,
            enumerable_type,
            variants,
        }
    }

    pub fn tag(&self) -> u16 {
        self.tag
    }

    pub fn generate(&self) -> TokenStream {
        let name = &self.name;
        let try_from_type = match self.enumerable_type {
            EnumerableType::Int => quote! { Int },
            EnumerableType::NumInGroup => quote! { NumInGroup },
            EnumerableType::Char => quote! { Char },
            EnumerableType::String | EnumerableType::MultipleStringValue => quote! { &FixStr },
            EnumerableType::MultipleCharValue => quote! { Char },
        };
        let mut variant_def = Vec::with_capacity(self.variants.len());
        let mut variant_name = Vec::with_capacity(self.variants.len());
        let mut variant_value_as_bytes = Vec::with_capacity(self.variants.len());
        for variant in &self.variants {
            let v_name = variant_ident(variant.name());
            let v_value_as_bytes = Literal::byte_string(variant.value().as_bytes());
            // TODO: check in easyfix-dictionary if variant value can be expressed as FixString
            // (no utf-8, no control characteres, etc), if not check here before using it in
            // FixStr::from_ascii_unchecked

            let variant_doc_comment = format!("Value \"{}\"", variant.value());
            variant_def.push(quote! {
                #[doc = #variant_doc_comment]
                #v_name
            });
            variant_name.push(v_name);
            variant_value_as_bytes.push(v_value_as_bytes);
        }
        let variant_value = self
            .variants
            .iter()
            .map(|variant| self.enumerable_type.literal(variant.value()));
        let try_from_match_input = if matches!(
            self.enumerable_type,
            EnumerableType::String | EnumerableType::MultipleStringValue
        ) {
            quote! { match input.as_bytes() }
        } else {
            quote! { match input }
        };
        let derives = if name == "MsgType" {
            quote! {
                #[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
                #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
                #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
            }
        } else {
            quote! {
                #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
                #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
                #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
            }
        };
        quote! {
            #derives
            pub enum #name {
                #[default]
                #(#variant_def,)*
            }

            impl #name {
                // TODO: try_*
                pub const fn from_bytes(input: &[u8]) -> Option<#name> {
                    match input {
                        #(#variant_value_as_bytes => Some(#name::#variant_name),)*
                        _ => None,
                    }
                }

                // TODO: try_*
                pub const fn from_fix_str(input: &FixStr) -> Option<#name> {
                    #name::from_bytes(input.as_bytes())
                }

                pub const fn as_bytes(&self) -> &'static [u8] {
                    match self {
                        #(#name::#variant_name => #variant_value_as_bytes,)*
                    }
                }

                pub const fn as_fix_str(&self) -> &'static FixStr {
                    // Safety: value was checked when it was generated
                    unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
                }
            }

            impl ToFixString for #name {
                fn to_fix_string(&self) -> FixString {
                    self.as_fix_str().to_owned()
                }
            }

            impl TryFrom<#try_from_type> for #name {
                type Error = ParseRejectReason;

                fn try_from(input: #try_from_type) -> Result<#name, ParseRejectReason> {
                    #try_from_match_input {
                        #(#variant_value => Ok(#name::#variant_name),)*
                        _ => Err(ParseRejectReason::ValueIsIncorrect),
                    }
                }
            }

            impl From<#name> for &'static [u8] {
                fn from(input: #name) -> &'static [u8] {
                    input.as_bytes()
                }
            }
        }
    }
}
