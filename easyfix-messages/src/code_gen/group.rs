use convert_case::{Case, Casing};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use super::{member::Member, serde_derives};

/// Repeating group definition (generated into groups.rs)
pub struct GroupCodeGen {
    name: Ident,
    num_in_group_tag: u16,
    members: Vec<Member>,
}

impl GroupCodeGen {
    pub fn new(name: &str, num_in_group_tag: u16, members: Vec<Member>) -> GroupCodeGen {
        GroupCodeGen {
            name: Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            num_in_group_tag,
            members,
        }
    }

    pub fn num_in_group_tag(&self) -> u16 {
        self.num_in_group_tag
    }

    fn generate_de_group(&self) -> TokenStream {
        let name = &self.name;
        let mut de_struct_entries = Vec::with_capacity(self.members.len());
        let mut de_match_entries = Vec::with_capacity(self.members.len());
        let Some((first_member, members)) = self.members.split_first() else {
            panic!("Empty group {name}");
        };
        let variables_definitions = members.iter().map(|member| member.gen_opt_variables());
        for member in members {
            de_match_entries.extend(member.gen_deserialize_match_entries());
            de_struct_entries.push(member.gen_deserialize_struct_entries());
        }

        let first_member_tag = first_member.tag_num();
        let first_member_deserialize_value = first_member.gen_deserialize_value();
        let first_member_def = first_member.gen_opt_variables();
        let first_member_struct_entry = first_member.gen_deserialize_struct_entries();

        let deserialize_loop = if members.is_empty() {
            // No members - no loop
            quote! {
                if let Some(tag) = deserializer.deserialize_tag_num()? {
                    // tag == #first_member_tag && last_run || tag != #first_member_tag && !last_run
                    if (tag == #first_member_tag) == last_run {
                        return Err(deserializer.reject(
                            Some(num_in_group_tag),
                            SessionRejectReasonBase::IncorrectNumInGroupCountForRepeatingGroup,
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
                                    SessionRejectReasonBase::IncorrectNumInGroupCountForRepeatingGroup,
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
                        SessionRejectReasonBase::RequiredTagMissing)
                    )
                };

                let mut processed_tags = Vec::with_capacity(expected_tags.len());

                let mut iter = expected_tags.iter();
                // Advance iterator, as first expected tag is alrady processed
                iter.next();
                processed_tags.push(#first_member_tag);

                #deserialize_loop

                Ok(#name {
                    #first_member_struct_entry,
                    #(#de_struct_entries,)*
                })
            }
        }
    }

    pub fn generate(&self, serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
        let name = &self.name;
        let members_definitions = self.members.iter().map(|member| member.gen_definition());
        let serialize = self.members.iter().map(|member| member.gen_serialize());
        let fn_deserialize = self.generate_de_group();
        let serde_derives = serde_derives(serde_serialize, serde_deserialize);

        quote! {
            #[allow(dead_code)]
            #[derive(Clone, Debug, Default)]
            #serde_derives
            pub struct #name {
                #(#members_definitions,)*
            }

            #[allow(dead_code)]
            impl #name {
                pub(crate) fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                #fn_deserialize
            }
        }
    }
}
