use convert_case::{Case, Casing};
use easyfix_dictionary::MsgCat;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use super::member::Member;

/// Message body definition (generated into messages.rs)
pub struct MessageCodeGen {
    name: Ident,
    body_members: Vec<Member>,
    msg_cat: MsgCat,
}

impl MessageCodeGen {
    pub fn new(name: &str, body_members: Vec<Member>, msg_cat: MsgCat) -> MessageCodeGen {
        MessageCodeGen {
            name: Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            body_members,
            msg_cat,
        }
    }

    pub fn name(&self) -> &Ident {
        &self.name
    }

    pub fn body_members(&self) -> &[Member] {
        &self.body_members
    }

    fn generate_de_message(&self) -> TokenStream {
        let name = &self.name;
        let mut variables_definitions = Vec::with_capacity(self.body_members.len());
        let mut de_struct_entries = Vec::with_capacity(self.body_members.len());
        let mut de_match_entries = Vec::with_capacity(self.body_members.len());
        for member in &self.body_members {
            variables_definitions.push(member.gen_opt_variables());
            de_match_entries.extend(member.gen_deserialize_match_entries());
            de_struct_entries.push(member.gen_deserialize_struct_entries());
        }
        quote! {
            fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
                #(#variables_definitions)*

                while let Some(tag) = deserializer.deserialize_tag_num()? {
                    match tag {
                        #(#de_match_entries,)*
                        tag => {
                            if FieldTag::from_tag_num(tag).is_some() {
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::TagNotDefinedForThisMessageType));
                            } else {
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag));
                            }
                        },
                    }
                }

                Ok(Box::new(Body::#name(#name {
                    #(#de_struct_entries,)*
                })))
            }
        }
    }

    pub fn generate(&self) -> TokenStream {
        let name = &self.name;
        let members_definitions = self
            .body_members
            .iter()
            .map(|member| member.gen_definition());
        let serialize = self
            .body_members
            .iter()
            .map(|member| member.gen_serialize());
        let fn_deserialize = self.generate_de_message();
        let msg_cat = Ident::new(&format!("{:?}", self.msg_cat), Span::call_site());

        quote! {
            #[derive(Clone, Debug, Default)]
            #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
            #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
            pub struct #name {
                #(#members_definitions,)*
            }

            impl #name {
                pub(crate) fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                #fn_deserialize

                pub const fn msg_type(&self) -> MsgType {
                    MsgType::#name
                }

                pub const fn msg_cat(&self) -> MsgCat {
                    MsgCat::#msg_cat
                }
            }
        }
    }
}
