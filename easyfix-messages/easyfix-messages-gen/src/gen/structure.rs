use std::rc::Rc;

use convert_case::{Case, Casing};
use easyfix_dictionary::{MsgCat, MsgType};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::gen::member::MemberDesc;

pub struct MessageProperties {
    pub msg_cat: MsgCat,
    pub _msg_type: MsgType,
    pub header_members: Rc<Vec<MemberDesc>>,
    pub trailer_members: Rc<Vec<MemberDesc>>,
}

pub struct Struct {
    name: Ident,
    members: Vec<MemberDesc>,
    msg_props: Option<MessageProperties>,
}

/*
struct HeaderStruct {
    body: Struct,
}

struct TrailerStruct {
    body: Struct,
}

struct GroupStruct {
    body: Struct,
}

struct MessageStruct {
    header: HeaderStruct,
    body: Struct,
    trailer: HeaderStruct,
    msg_type: MsgType,
    msg_cat: MsgCat,
}
*/

impl Struct {
    pub fn new(
        name: &str,
        members: Vec<MemberDesc>,
        msg_props: Option<MessageProperties>,
    ) -> Struct {
        Struct {
            name: Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            members,
            msg_props,
        }
    }

    pub fn name(&self) -> &Ident {
        &self.name
    }

    pub fn msg_props(&self) -> Option<&MessageProperties> {
        self.msg_props.as_ref()
    }

    pub fn is_group(&self) -> bool {
        self.msg_props.is_none() && self.name != "Header" && self.name != "Trailer"
    }

    fn generate_serialize(&self) -> Vec<TokenStream> {
        self.members
            .iter()
            .filter_map(|member| member.gen_serialize())
            .collect()
    }

    fn generate_de_group(&self) -> TokenStream {
        let name = &self.name;
        let mut variables_definitions = Vec::with_capacity(self.members.len());
        let mut de_struct_entries = Vec::with_capacity(self.members.len());
        let mut de_match_entries = Vec::with_capacity(self.members.len());
        let Some((first_member, members)) = self.members.split_first() else {
            panic!("Empty group {name}");
        };
        for member in members {
            variables_definitions.push(member.gen_opt_variables());
            if let Some(de_match_entry) = member.gen_deserialize_match_entries() {
                de_match_entries.push(de_match_entry);
            }
            if let Some(de_struct_entry) = member.gen_deserialize_struct_entries() {
                de_struct_entries.push(de_struct_entry);
            }
        }

        //let mut first_member = first_member.clone();
        //first_member.set_required(true);
        let first_member_tag = first_member.tag_num();
        let first_member_deserialize_value = first_member.gen_deserialize_value();
        let first_member_def = first_member.gen_opt_variables();
        let first_member_struct_entry = first_member
            .gen_deserialize_struct_entries()
            .map(|entry| quote! { #entry, });

        let deserialize_loop = if members.is_empty() {
            // No members - no loop
            quote! {
                if let Some(tag) = deserializer.deserialize_tag_num()? {
                    if tag == #first_member_tag && last_run || tag != #first_member_tag && !last_run {
                        return Err(deserializer.reject(
                            Some(num_in_group_tag),
                            SessionRejectReason::IncorrectNumingroupCountForRepeatingGroup,
                        ));
                    }
                    deserializer.put_tag(tag);
                }
            }
        } else {
            quote! {
                while let Some(tag) = deserializer.deserialize_tag_num()? {
                    match tag {
                        #(#de_match_entries,)*
                        tag => {
                            if tag == #first_member_tag && last_run || tag != #first_member_tag && !last_run {
                                return Err(deserializer.reject(
                                    Some(num_in_group_tag),
                                    SessionRejectReason::IncorrectNumingroupCountForRepeatingGroup,
                                ));
                            } else {
                                deserializer.put_tag(tag);
                                break;
                            }
                        },
                    }
                    processed_tags.push(tag);
                    let mut tag_in_order = false;
                    for expected_tag in iter.by_ref() {
                        if *expected_tag == tag {
                            tag_in_order = true;
                            break;
                        }
                    }
                    if !tag_in_order {
                        return Err(deserializer.repeating_group_fields_out_of_order(
                            expected_tags,
                            &processed_tags,
                            tag
                        ));
                    }
                }
            }
        };

        quote! {
            pub(crate) fn deserialize(
                deserializer: &mut Deserializer,
                num_in_group_tag: u16,
                expected_tags: &[u16],
                last_run: bool,
            ) -> Result<#name, DeserializeError> {
                // Check if tag of first group member is present
                #first_member_def
                #(#variables_definitions)*
                if let Some(#first_member_tag) = deserializer.deserialize_tag_num()? {
                    #first_member_deserialize_value
                } else {
                    // if not, return error as first group member is always required (even when
                    // defined as optional)
                    return Err(deserializer.reject(
                        Some(#first_member_tag),
                        SessionRejectReason::RequiredTagMissing)
                    )
                };

                let mut processed_tags = Vec::with_capacity(expected_tags.len());

                let mut iter = expected_tags.iter();
                // Advance iterator, as first expected tag is alrady processed
                iter.next();
                processed_tags.push(#first_member_tag);

                #deserialize_loop

                Ok(#name {
                    #first_member_struct_entry
                    #(#de_struct_entries,)*
                })
            }
        }
    }

    fn generate_de_message(&self) -> TokenStream {
        let name = &self.name;
        let mut variables_definitions = Vec::with_capacity(self.members.len());
        let mut de_header_entries = Vec::with_capacity(self.members.len());
        let mut de_struct_entries = Vec::with_capacity(self.members.len());
        let mut de_trailer_entries = Vec::with_capacity(self.members.len());
        let mut de_match_entries = Vec::with_capacity(self.members.len()); //self.generate_de_match_entries();
        for member in self.msg_props().unwrap().header_members.iter() {
            variables_definitions.push(member.gen_opt_variables());
            if let Some(de_match_entry) = member.gen_deserialize_match_entries() {
                de_match_entries.push(de_match_entry);
            }
            if let Some(de_struct_entry) = member.gen_deserialize_struct_entries() {
                de_header_entries.push(de_struct_entry);
            }
        }
        for member in &self.members {
            variables_definitions.push(member.gen_opt_variables());
            if let Some(de_match_entry) = member.gen_deserialize_match_entries() {
                de_match_entries.push(de_match_entry);
            }
            if let Some(de_struct_entry) = member.gen_deserialize_struct_entries() {
                de_struct_entries.push(de_struct_entry);
            }
        }
        for member in self.msg_props().unwrap().trailer_members.iter() {
            variables_definitions.push(member.gen_opt_variables());
            if let Some(de_match_entry) = member.gen_deserialize_match_entries() {
                de_match_entries.push(de_match_entry);
            }
            if let Some(de_struct_entry) = member.gen_deserialize_struct_entries() {
                de_trailer_entries.push(de_struct_entry);
            }
        }
        quote! {
            fn deserialize(
                deserializer: &mut Deserializer,
                begin_string: FixString,
                body_length: Length,
                msg_type: MsgType
            ) -> Result<Box<FixtMessage>, DeserializeError> {
                #(#variables_definitions)*
                while let Some(tag) = deserializer.deserialize_tag_num()? {
                    match tag {
                        #(#de_match_entries,)*
                        tag => {
                            if FieldTag::from_tag_num(tag).is_some() {
                                return Err(deserializer.reject(Some(tag), SessionRejectReason::TagNotDefinedForThisMessageType));
                            } else {
                                return Err(deserializer.reject(Some(tag), SessionRejectReason::UndefinedTag));
                            }
                        },
                    }
                }
                Ok(Box::new(FixtMessage {
                    header: Box::new(Header {
                        #(#de_header_entries,)*
                    }),
                    body: Box::new(Message::#name(#name {
                        #(#de_struct_entries,)*
                    })),
                    trailer: Box::new(Trailer {
                        #(#de_trailer_entries,)*
                    })
                }))
            }
        }
    }

    pub fn generate(&self) -> TokenStream {
        let name = &self.name;

        let mut members_definitions = Vec::with_capacity(self.members.len());
        for member in &self.members {
            if let Some(member_def) = member.gen_definition() {
                members_definitions.push(member_def);
            }
        }

        let fn_deserialize = if self.name == "Header" || self.name == "Trailer" {
            None
        } else if self.is_group() {
            Some(self.generate_de_group())
        } else {
            Some(self.generate_de_message())
        };

        let serialize = self.generate_serialize();

        let fn_msg_type_msg_cat = if let Some(props) = self.msg_props() {
            let msg_cat = Ident::new(&format!("{:?}", props.msg_cat), Span::call_site());
            Some(quote! {
                pub const fn msg_type(&self) -> MsgType {
                    MsgType::#name
                }

                pub const fn msg_cat(&self) -> MsgCat {
                    MsgCat::#msg_cat
                }
            })
        } else {
            None
        };

        quote! {
            #[derive(Clone, Debug, Default)]
            #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
            #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
            pub struct #name {
                #(pub #members_definitions,)*
            }

            impl #name {
                pub(crate) fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                #fn_deserialize

                #fn_msg_type_msg_cat
            }
        }
    }
}
