use easyfix_dictionary::{BasicType};
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
            t @ (BasicType::Int | BasicType::NumInGroup | BasicType::Char | BasicType::String) => {
                Type::basic_type(t).gen_type()
            }
            BasicType::MultipleCharValue => Type::basic_type(BasicType::Char).gen_type(),
            BasicType::MultipleStringValue => Type::basic_type(BasicType::String).gen_type(),
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
            quote! { match input.as_ref() }
        } else {
            quote! { match input }
        };
        let derives = if name == "MsgType" {
            quote! { #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)] }
        } else {
            quote! { #[derive(Clone, Copy, Debug, Eq, PartialEq)] }
        };
        quote! {
            #derives
            pub enum #name {
                #(#variant_name,)*
            }

            impl #name {
                pub fn from_bytes(input: &[u8]) -> Option<#name> {
                    match input {
                        #(#variant_value_as_bytes => Some(#name::#variant_name),)*
                        _ => None,
                    }
                }

                pub fn as_bytes(&self) -> &'static [u8] {
                    match self {
                        #(#name::#variant_name => #variant_value_as_bytes,)*
                    }
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

