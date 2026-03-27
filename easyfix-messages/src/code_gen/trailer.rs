use proc_macro2::TokenStream;
use quote::quote;

use super::{member::Member, serde_derives};

/// Trailer generator
pub struct Trailer {
    members: Vec<Member>,
}

impl Trailer {
    pub fn new(members: Vec<Member>) -> Trailer {
        Trailer { members }
    }

    fn generate_deserialize(&self) -> TokenStream {
        let variables_definitions = self.members.iter().map(|member| member.gen_opt_variables());
        let de_trailer_entries = self
            .members
            .iter()
            .map(|member| member.gen_deserialize_struct_entries());
        let deserialize_values = if self.members.len() > 1 {
            let de_match_entries = self
                .members
                .iter()
                .flat_map(|member| member.gen_deserialize_match_entries());
            quote! {
                while let Some(tag) = deserializer.deserialize_tag_num()? {
                    match tag {
                        #(#de_match_entries,)*
                        tag => {
                            if FieldTag::from_tag_num(tag).is_some() {
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder));
                            } else {
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag));
                            }
                        },
                    }
                }
            }
        } else {
            quote! {
                if let Some(tag) = deserializer.deserialize_tag_num()? {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder));
                    } else {
                        return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag));
                    }
                }
            }
        };

        quote! {
            fn deserialize(deserializer: &mut Deserializer) -> Result<Trailer, DeserializeError> {
                #(#variables_definitions)*

                #deserialize_values

                Ok(Trailer {
                    #(#de_trailer_entries,)*
                })
            }
        }
    }

    pub fn generate(&self, serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
        let members_definitions = self.members.iter().map(|member| member.gen_definition());
        let serialize = self.members.iter().map(|member| member.gen_serialize());
        let deserialize = self.generate_deserialize();
        let serde_derives = serde_derives(serde_serialize, serde_deserialize);

        quote! {
            #[allow(dead_code)]
            #[derive(Clone, Debug, Default)]
            #serde_derives
            pub struct Trailer {
                #(#members_definitions,)*
            }

            #[allow(dead_code)]
            impl Trailer {
                pub(crate) fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                #deserialize
            }
        }
    }
}
