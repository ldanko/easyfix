use crate::gen::member::MemberDesc;
use convert_case::{Case, Casing};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

pub struct Struct {
    name: Ident,
    members: Vec<MemberDesc>,
    msg_type: Option<Vec<u8>>,
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
    msg_type: Vec<u8>,
}
*/

impl Struct {
    pub fn new(name: &str, members: Vec<MemberDesc>, msg_type: Option<Vec<u8>>) -> Struct {
        Struct {
            name: Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            members,
            msg_type,
        }
    }

    pub fn name(&self) -> &Ident {
        &self.name
    }

    pub fn msg_type(&self) -> Option<&[u8]> {
        self.msg_type.as_deref()
    }

    pub fn is_group(&self) -> bool {
        self.msg_type().is_none() && self.name != "Header" && self.name != "Trailer"
    }

    fn generate_serialize(&self) -> Vec<TokenStream> {
        self.members
            .iter()
            .filter_map(|member| member.gen_serialize())
            .collect()
    }

    fn generate_de_header(&self) -> TokenStream {
        let name = &self.name;
        let mut variables_definitions = Vec::with_capacity(self.members.len());
        let mut de_struct_entries = Vec::with_capacity(self.members.len());
        let mut de_match_entries = Vec::with_capacity(self.members.len()); //self.generate_de_match_entries();
        for member in &self.members {
            variables_definitions.push(member.gen_opt_variables());
            if let Some(de_match_entry) = member.gen_deserialize_match_entries() {
                de_match_entries.push(de_match_entry);
            }
            if let Some(de_struct_entry) = member.gen_deserialize_struct_entries() {
                de_struct_entries.push(de_struct_entry);
            }
        }
        quote! {
            #(#variables_definitions)*
            while let Some(tag) = deserializer.deserialize_tag_num()? {
                match tag {
                    #(#de_match_entries,)*
                    tag => {
                        deserializer.put_tag(tag);
                        break;
                    },
                }
            }
            Ok(#name {
                #(#de_struct_entries,)*
            })
        }
    }

    fn generate_de_trailer(&self) -> TokenStream {
        let name = &self.name;
        let mut variables_definitions = Vec::with_capacity(self.members.len());
        let mut de_struct_entries = Vec::with_capacity(self.members.len());
        let mut de_match_entries = Vec::with_capacity(self.members.len()); //self.generate_de_match_entries();
        for member in &self.members {
            variables_definitions.push(member.gen_opt_variables());
            if let Some(de_match_entry) = member.gen_deserialize_match_entries() {
                de_match_entries.push(de_match_entry);
            }
            if let Some(de_struct_entry) = member.gen_deserialize_struct_entries() {
                de_struct_entries.push(de_struct_entry);
            }
        }
        quote! {
            #(#variables_definitions)*
            while let Some(tag) = deserializer.deserialize_tag_num()? {
                match tag {
                    #(#de_match_entries,)*
                    // TODO: This may also be UndefinedTag or TagAppearsMoreThanOnce (in header or
                    // in body) and maybe TagSpecifiedOutOfRequiredOrder (also from header or body)
                    tag => return Err(deserializer.reject(Some(tag), RejectReason::TagNotDefinedForThisMessageType)),
                }
            }
            Ok(#name {
                #(#de_struct_entries,)*
            })
        }
    }

    fn generate_de_message(&self) -> TokenStream {
        let name = &self.name;
        let mut variables_definitions = Vec::with_capacity(self.members.len());
        let mut de_struct_entries = Vec::with_capacity(self.members.len());
        let mut de_match_entries = Vec::with_capacity(self.members.len()); //self.generate_de_match_entries();
        for member in &self.members {
            variables_definitions.push(member.gen_opt_variables());
            if let Some(de_match_entry) = member.gen_deserialize_match_entries() {
                de_match_entries.push(de_match_entry);
            }
            if let Some(de_struct_entry) = member.gen_deserialize_struct_entries() {
                de_struct_entries.push(de_struct_entry);
            }
        }
        let (expected_tags_iterator, tag_num_loop_label, group_handling_loop) = if self.is_group() {
            (
                Some(quote! { let mut expected_tags = expected_tags.iter_mut(); }),
                Some(quote! { 'tag_num_loop: }),
                Some(quote! {
                    loop {
                        if let Some(exp_tag) = expected_tags.next() {
                            if first_run {
                                let (expected_tag, required) = exp_tag
                                    .expect("internal error - on group member, expected tag can not be `None`");
                                if expected_tag != tag {
                                    if required {
                                        // Return early, no need to wait for object construction
                                        return Err(deserializer.reject(Some(expected_tag), RejectReason::RequiredTagMissing));
                                    } else {
                                        *exp_tag = None;
                                        continue;
                                    }
                                } else {
                                    break;
                                }
                            } else {
                                if let Some((expected_tag, _)) = exp_tag {
                                    if *expected_tag != tag {
                                        return Err(deserializer.reject(Some(*expected_tag), RejectReason::RequiredTagMissing));
                                    } else {
                                        break;
                                    }
                                } else {
                                    continue;
                                }
                            }
                        } else {
                            deserializer.put_tag(tag);
                            break 'tag_num_loop;
                        }
                    }
                }),
            )
        } else {
            (None, None, None)
        };
        quote! {
            #(#variables_definitions)*
            #expected_tags_iterator
            #tag_num_loop_label while let Some(tag) = deserializer.deserialize_tag_num()? {
                #group_handling_loop
                match tag {
                    #(#de_match_entries,)*
                    tag => {
                        deserializer.put_tag(tag);
                        break;
                    },
                }
            }
            Ok(#name {
                #(#de_struct_entries,)*
            })
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

        let fn_deserialize_definition = if self.is_group() {
            quote! {
                fn deserialize(
                    deserializer: &mut Deserializer,
                    first_run: bool,
                    expected_tags: &mut [Option<(TagNum, bool)>],
                ) -> Result<#name, DeserializeError>
            }
        } else {
            quote! { fn deserialize(deserializer: &mut Deserializer) -> Result<#name, DeserializeError> }
        };

        let deserialize_body = if self.name == "Header" {
            self.generate_de_header()
        } else if self.name == "Trailer" {
            self.generate_de_trailer()
        } else {
            self.generate_de_message()
        };

        let serialize = self.generate_serialize();

        quote! {
            #[derive(Clone, Debug)]
            pub struct #name {
                #(pub #members_definitions,)*
            }

            impl #name {
                fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                #fn_deserialize_definition {
                    #deserialize_body
                }
            }
        }
    }
}
