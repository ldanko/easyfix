use proc_macro2::TokenStream;
use quote::quote;

use super::member::Member;

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
        let de_match_entries = self
            .members
            .iter()
            .flat_map(|member| member.gen_deserialize_match_entries());
        let de_trailer_entries = self
            .members
            .iter()
            .map(|member| member.gen_deserialize_struct_entries());
        quote! {
            fn deserialize(deserializer: &mut Deserializer) -> Result<Trailer, DeserializeError> {
                #(#variables_definitions)*

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

                Ok(Trailer {
                    #(#de_trailer_entries,)*
                })
            }
        }
    }

    pub fn generate(&self) -> TokenStream {
        let members_definitions = self.members.iter().map(|member| member.gen_definition());
        let serialize = self.members.iter().map(|member| member.gen_serialize());
        let deserialize = self.generate_deserialize();

        quote! {
            #[derive(Clone, Debug, Default)]
            #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
            #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
            pub struct Trailer {
                #(#members_definitions,)*
            }

            impl Trailer {
                pub(crate) fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                #deserialize
            }
        }
    }
}
