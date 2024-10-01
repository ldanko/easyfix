use convert_case::{Case, Casing};
use easyfix_dictionary::{BasicType, Value};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use crate::gen::member::Type;

pub struct EnumDesc {
    name: Ident,
    type_: BasicType,
    // (VarianName, VariantValue, VariantValueAsBytes)
    values: Vec<Value>,
}
impl EnumDesc {
    pub fn new(name: Ident, type_: BasicType, values: Vec<Value>) -> EnumDesc {
        EnumDesc {
            name,
            type_,
            values,
        }
    }

    fn literal_ctr(&self, value: &str) -> Literal {
        match self.type_ {
            BasicType::String | BasicType::MultipleStringValue => {
                Literal::byte_string(value.as_bytes())
            }
            BasicType::Char | BasicType::MultipleCharValue => {
                Literal::u8_suffixed(value.as_bytes()[0])
            }
            BasicType::Int => Literal::i64_suffixed(value.parse().expect("Wrong enum value")),
            BasicType::NumInGroup => Literal::u8_suffixed(value.parse().expect("Wrong enum value")),
            type_ => panic!("type {:?} can not be represented as enum", type_),
        }
    }

    pub fn generate(&self) -> TokenStream {
        let name = &self.name;
        let type_ = match self.type_ {
            t @ (BasicType::Int | BasicType::NumInGroup | BasicType::Char) => {
                Type::basic_type(t).gen_type()
            }
            BasicType::String | BasicType::MultipleStringValue => quote! { &FixStr },
            BasicType::MultipleCharValue => Type::basic_type(BasicType::Char).gen_type(),
            type_ => panic!("type {:?} can not be used as enumeration", type_),
        };
        let mut variant_def = Vec::with_capacity(self.values.len());
        let mut variant_name = Vec::with_capacity(self.values.len());
        let mut variant_value = Vec::with_capacity(self.values.len());
        let mut variant_value_as_bytes = Vec::with_capacity(self.values.len());
        for value in &self.values {
            let v_name = Ident::new(
                &{
                    let mut variant_name = value.description().to_case(Case::UpperCamel);
                    if variant_name.as_bytes()[0].is_ascii_digit() {
                        variant_name.insert(0, '_');
                    }
                    variant_name
                },
                Span::call_site(),
            );
            let v_value = self.literal_ctr(value.value());
            let v_value_as_bytes = Literal::byte_string(value.value().as_bytes());

            let variant_doc_comment = format!("Value \"{}\"", value.value());
            variant_def.push(quote! {
                #[doc = #variant_doc_comment]
                #v_name
            });
            variant_name.push(v_name.clone());
            variant_value.push(v_value.clone());
            variant_value_as_bytes.push(v_value_as_bytes.clone());
        }
        let try_from_match_input = if matches!(
            self.type_,
            BasicType::String | BasicType::MultipleStringValue
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
                pub const fn from_bytes(input: &[u8]) -> Option<#name> {
                    match input {
                        #(#variant_value_as_bytes => Some(#name::#variant_name),)*
                        _ => None,
                    }
                }

                pub const fn from_fix_str(input: &FixStr) -> Option<#name> {
                    #name::from_bytes(input.as_bytes())
                }

                pub const fn as_bytes(&self) -> &'static [u8] {
                    match self {
                        #(#name::#variant_name => #variant_value_as_bytes,)*
                    }
                }

                pub const fn as_fix_str(&self) -> &'static FixStr {
                    unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
                }
            }

            impl ToFixString for #name {
                fn to_fix_string(&self) -> FixString {
                    self.as_fix_str().to_owned()
                }
            }

            impl TryFrom<#type_> for #name {
                type Error = ParseRejectReason;

                fn try_from(input: #type_) -> Result<#name, ParseRejectReason> {
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
