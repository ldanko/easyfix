use easyfix_dictionary::BasicType;
use proc_macro2::{Ident, Literal, TokenStream};
use quote::quote;

use crate::gen::member::Type;

pub struct EnumDesc {
    name: Ident,
    type_: BasicType,
    // (VarianName, VariantValue, VariantValueAsBytes)
    values: Vec<(Ident, Literal, Literal)>,
}

impl EnumDesc {
    pub fn new(name: Ident, type_: BasicType, values: Vec<(Ident, Literal, Literal)>) -> EnumDesc {
        EnumDesc {
            name,
            type_,
            values,
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
        let mut variant_name = Vec::with_capacity(self.values.len());
        let mut variant_value = Vec::with_capacity(self.values.len());
        let mut variant_value_as_bytes = Vec::with_capacity(self.values.len());
        for (v_name, v_value, v_value_as_bytes) in &self.values {
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
                #(#variant_name,)*
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
                type Error = SessionRejectReason;

                fn try_from(input: #type_) -> Result<#name, SessionRejectReason> {
                    #try_from_match_input {
                        #(#variant_value => Ok(#name::#variant_name),)*
                        _ => Err(SessionRejectReason::ValueIsIncorrect),
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
