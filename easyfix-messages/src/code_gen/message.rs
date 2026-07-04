use easyfix_core::basic_types::FixStr;
use easyfix_dictionary::MsgCat;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use super::{ident::ToIdent, member::Member, serde_derives};

/// Message body definition (generated into messages.rs)
pub struct MessageCodeGen {
    name: Ident,
    body_members: Vec<Member>,
    msg_cat: MsgCat,
}

impl MessageCodeGen {
    pub fn new(name: &FixStr, body_members: Vec<Member>, msg_cat: MsgCat) -> MessageCodeGen {
        MessageCodeGen {
            name: name.to_pascal_ident(),
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
                            if Header::is_header_field(tag) || Trailer::is_trailer_field(tag) {
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder));
                            } else if FieldTag::from_tag_num(tag).is_some() {
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::TagNotDefinedForThisMessageType));
                            } else {
                                // A tag defined in no dictionary:
                                // InvalidTagNumber, not UndefinedTag).
                                // See Scenario 14a
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::InvalidTagNumber));
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

    pub fn generate(&self, serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
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
        let serde_derives = serde_derives(serde_serialize, serde_deserialize);

        // A required core `ApplVerId` field (FIXT Logon's 1137) has no
        // `Default`, so the struct emits `Default` manually with the
        // spec's absence semantics for that field.
        let (default_derive, manual_default) = if self
            .body_members
            .iter()
            .any(|member| member.needs_manual_default())
        {
            let default_entries = self
                .body_members
                .iter()
                .map(|member| member.gen_default_entry());
            (
                quote! {},
                quote! {
                    impl Default for #name {
                        fn default() -> #name {
                            #name {
                                #(#default_entries,)*
                            }
                        }
                    }
                },
            )
        } else {
            (quote! { , Default }, quote! {})
        };

        quote! {
            #[allow(dead_code)]
            #[derive(Clone, Debug #default_derive)]
            #serde_derives
            pub struct #name {
                #(#members_definitions,)*
            }

            #manual_default

            #[allow(dead_code)]
            impl #name {
                pub(crate) fn serialize(&self, serializer: &mut Serializer) -> Result<(), SerializeError> {
                    #(#serialize)*
                    Ok(())
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
