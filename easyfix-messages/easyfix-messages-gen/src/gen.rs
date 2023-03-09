mod enumeration;
mod member;
mod structure;

use std::{collections::HashMap, rc::Rc};

use convert_case::{Case, Casing};
use easyfix_dictionary::{BasicType, Dictionary, Member, MemberKind};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use self::structure::MessageProperties;
use crate::gen::{
    enumeration::EnumDesc,
    member::{MemberDesc, SimpleMember},
    structure::Struct,
};

pub struct Generator {
    begin_string: Vec<u8>,
    structs: Vec<Struct>,
    enums: Vec<EnumDesc>,
    fields_names: Vec<Ident>,
    fields_numbers: Vec<u16>,
}

fn process_members(
    members: &[Member],
    dictionary: &Dictionary,
    members_descs: &mut Vec<MemberDesc>,
    groups: &mut HashMap<String, Struct>,
) {
    let mut members = members.iter().peekable();
    while let Some(member) = members.next() {
        match member.kind() {
            MemberKind::Component => {
                let component = dictionary
                    .component(member.name())
                    .expect("unknown component");
                if let Some(number_of_elements) = component.number_of_elements() {
                    let number_of_elements_field = dictionary
                        .fields_by_name()
                        .get(number_of_elements.name())
                        .expect("unknown field");
                    let mut group_members = Vec::new();
                    process_members(component.members(), dictionary, &mut group_members, groups);
                    assert_eq!(component.name(), member.name(), "Componen t name mismatch");

                    members_descs.push(MemberDesc::group(
                        SimpleMember::num_in_group(
                            number_of_elements.name(),
                            number_of_elements_field.number(),
                            // When component holding group is required, group is also required, so `num in group` field is also required
                            member.required(),
                            // number_of_elements.required(),
                        ),
                        SimpleMember::group(
                            member.name(),
                            number_of_elements_field.number(),
                            member.required(),
                            // number_of_elements.required(),
                        ),
                        group_members
                            .iter()
                            .filter(|member| matches!(member, MemberDesc::Simple(_)))
                            //.map(|member| (member.tag_num(), member.required()))
                            .map(|member| member.tag_num())
                            .collect(),
                    ));
                    members_descs.push(MemberDesc::Simple(SimpleMember::group(
                        member.name(),
                        number_of_elements_field.number(),
                        member.required(),
                        // number_of_elements.required(),
                    )));

                    groups
                        .entry(component.name().to_owned())
                        .or_insert_with(|| Struct::new(component.name(), group_members, None));
                } else {
                    process_members(component.members(), dictionary, members_descs, groups);
                }
            }
            MemberKind::Field => {
                let field = dictionary
                    .fields_by_name()
                    .get(member.name())
                    .ok_or_else(|| format!("unknown field `{}`", member.name()))
                    .unwrap();

                match field.type_() {
                    BasicType::Length => {
                        // Do not skip peeked value, it must be procesed separately
                        // to generate code for TagSpecifiedOutOfRequiredOrdern rejects.
                        if let Some(next_member) = members.peek() {
                            let next_field = dictionary
                                .fields_by_name()
                                .get(next_member.name())
                                .expect("unknown field");
                            if let BasicType::Data | BasicType::XmlData = next_field.type_() {
                                members_descs.push(MemberDesc::custom_length(
                                    SimpleMember::length(
                                        member.name(),
                                        field.number(),
                                        member.required(),
                                    ),
                                    SimpleMember::field(
                                        next_member.name(),
                                        next_field.number(),
                                        next_member.required(),
                                        next_field.type_(),
                                    ),
                                ));
                            } else {
                                members_descs.push(MemberDesc::simple(
                                    member.name(),
                                    field.number(),
                                    member.required(),
                                    field.type_(),
                                ))
                            }
                        }
                    }
                    // Special case, to no create enumerations for boolean values
                    BasicType::Boolean => members_descs.push(MemberDesc::simple(
                        member.name(),
                        field.number(),
                        member.required(),
                        BasicType::Boolean,
                    )),
                    type_ => {
                        if let Some(_values) = field.values() {
                            members_descs.push(MemberDesc::enumeration(
                                member.name(),
                                field.number(),
                                member.required(),
                                type_,
                            ))
                        } else {
                            members_descs.push(MemberDesc::simple(
                                member.name(),
                                field.number(),
                                member.required(),
                                type_,
                            ))
                        }
                    }
                }
            }
        }
    }
}

impl Generator {
    pub fn new(dictionary: &Dictionary) -> Generator {
        let (protocol, version) = if let Some(fixt_version) = dictionary.fixt_version() {
            ("FIXT", fixt_version)
        } else if let Some(fix_version) = dictionary.fix_version() {
            ("FIX", fix_version)
        } else {
            panic!("Neither FIX nor FIXT version defined");
        };
        let begin_string = if version.service_pack() == 0 {
            format!("{}.{}.{}", protocol, version.major(), version.minor())
        } else {
            format!(
                "{}.{}.{}SP{}",
                protocol,
                version.major(),
                version.minor(),
                version.service_pack()
            )
        }
        .into_bytes();

        let mut structs = Vec::new();
        let mut groups = HashMap::new();

        let header = dictionary.header().expect("Missing FIX header definition");
        let header_members = {
            let mut header_members = Vec::new();
            process_members(
                header.members(),
                dictionary,
                &mut header_members,
                &mut groups,
            );
            structs.push(Struct::new(header.name(), header_members.clone(), None));
            Rc::new(header_members)
        };

        let trailer = dictionary
            .trailer()
            .expect("Missing FIX trailer definition");
        let trailer_members = {
            let mut trailer_members = Vec::new();
            process_members(
                trailer.members(),
                dictionary,
                &mut trailer_members,
                &mut groups,
            );
            structs.push(Struct::new(trailer.name(), trailer_members.clone(), None));
            Rc::new(trailer_members)
        };

        for msg in dictionary.messages().values() {
            let mut members_descs = Vec::with_capacity(1 + msg.members().len() + 1);
            {
                //members_descs.push(MemberDesc::header());
                process_members(msg.members(), dictionary, &mut members_descs, &mut groups);
                //members_descs.push(MemberDesc::trailer());
            }

            structs.push(Struct::new(
                msg.name(),
                members_descs,
                Some(MessageProperties {
                    msg_cat: msg.msg_cat(),
                    msg_type: msg.msg_type(),
                    header_members: header_members.clone(),
                    trailer_members: trailer_members.clone(),
                }),
            ));
        }

        structs.extend(groups.into_values());

        let mut enums = Vec::new();
        for field in dictionary.fields().values() {
            // Don't map booleans into YES/NO enumeration
            if let BasicType::Boolean = field.type_() {
                continue;
            }
            if let Some(values) = field.values() {
                let name = Ident::new(&field.name().to_case(Case::UpperCamel), Span::call_site());
                let literal_ctr = |value: &str| match field.type_() {
                    BasicType::String | BasicType::MultipleStringValue => {
                        Literal::byte_string(value.as_bytes())
                    }
                    BasicType::Char | BasicType::MultipleCharValue => {
                        Literal::u8_suffixed(value.as_bytes()[0])
                    }
                    BasicType::Int => {
                        Literal::i64_suffixed(value.parse().expect("Wrong enum value"))
                    }
                    BasicType::NumInGroup => {
                        Literal::u8_suffixed(value.parse().expect("Wrong enum value"))
                    }
                    type_ => panic!("type {:?} can not be represented as enum", type_),
                };
                enums.push(EnumDesc::new(
                    name,
                    field.type_(),
                    values
                        .iter()
                        .map(|value| {
                            (
                                Ident::new(
                                    &{
                                        let mut variant_name =
                                            value.description().to_case(Case::UpperCamel);
                                        if variant_name.as_bytes()[0].is_ascii_digit() {
                                            variant_name.insert(0, '_');
                                        }
                                        variant_name
                                    },
                                    Span::call_site(),
                                ),
                                literal_ctr(value.value()),
                                Literal::byte_string(value.value().as_bytes()),
                            )
                        })
                        .collect(),
                ));
            }
        }

        let mut fields = dictionary.fields().values().collect::<Vec<_>>();
        fields.sort_by_key(|f| f.number());
        let (fields_names, fields_numbers) = fields
            .iter()
            .map(|f| {
                (
                    Ident::new(&f.name().to_case(Case::UpperCamel), Span::call_site()),
                    f.number(),
                )
            })
            .unzip();

        Generator {
            begin_string,
            structs,
            enums,
            fields_names,
            fields_numbers,
        }
    }

    pub fn generate_fields(&self) -> TokenStream {
        let mut enums = Vec::new();
        for enum_ in &self.enums {
            enums.push(enum_.generate());
        }

        quote! {
            #(#enums)*
        }
    }

    pub fn generate_groups(&self) -> TokenStream {
        let mut groups_defs = Vec::new();

        for struct_ in &self.structs {
            if struct_.is_group() {
                groups_defs.push(struct_.generate());
            }
        }

        quote! {
            use crate::{
                deserializer::{DeserializeError, Deserializer},
                fields::{self, basic_types::*, SessionRejectReason},
                serializer::Serializer,
            };

            #(#groups_defs)*
        }
    }

    pub fn generate_messages(&self) -> TokenStream {
        let mut structs_defs = Vec::new();
        let mut name = Vec::new();
        let mut impl_from_msg = Vec::new();
        for struct_ in &self.structs {
            let struct_name = struct_.name();

            if !struct_.is_group() {
                structs_defs.push(struct_.generate());
            }

            if struct_.msg_props().is_some() {
                impl_from_msg.push(quote! {
                    impl From<#struct_name> for Message {
                        fn from(msg: #struct_name) -> Message {
                            Message::#struct_name(msg)
                        }
                    }
                });

                name.push(struct_name);
            }
        }

        let begin_string = Literal::byte_string(&self.begin_string);
        let fields_names = &self.fields_names;
        let fields_names_as_str: Vec<_> = self.fields_names.iter().map(|f| f.to_string()).collect();
        let fields_numbers = &self.fields_numbers;
        let fields_numbers_literals = self
            .fields_numbers
            .iter()
            .map(|num| Literal::u16_suffixed(*num))
            .collect::<Vec<_>>();

        quote! {
            use crate::{
                deserializer::{raw_message, DeserializeError, Deserializer, RawMessage},
                fields::{self, basic_types::*, SessionRejectReason},
                groups::*,
                serializer::Serializer,
            };
            use std::fmt;

            pub const BEGIN_STRING: &FixStr = unsafe { FixStr::from_ascii_unchecked(#begin_string) };

            #[derive(Clone, Copy, Debug, Eq, PartialEq)]
            #[repr(u16)]
            pub enum FieldTag {
                #(#fields_names = #fields_numbers,)*
            }

            impl fmt::Display for FieldTag {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    match self {
                        #(FieldTag::#fields_names => write!(f, #fields_names_as_str),)*
                    }
                }
            }

            impl FieldTag {
                fn from_tag_num(tag_num: TagNum) -> Option<FieldTag> {
                    match tag_num {
                        #(#fields_numbers_literals => Some(FieldTag::#fields_names),)*
                        _ => None,
                    }
                }
            }

            use fields::MsgType;

            #(#structs_defs)*

            #[derive(Clone, Debug)]
            pub enum Message {
                #(#name(#name),)*
            }

            impl Message {
                fn serialize(&self, serializer: &mut Serializer) {
                    match self {
                        #(Message::#name(msg) => msg.serialize(serializer),)*
                    }
                }

                fn deserialize(
                    deserializer: &mut Deserializer,
                    begin_string: FixString,
                    body_length: Length,
                    msg_type: MsgType
                ) -> Result<Box<FixtMessage>, DeserializeError> {
                    match msg_type {
                        #(
                            MsgType::#name => Ok(#name::deserialize(deserializer, begin_string, body_length, msg_type)?),
                        )*
                    }
                }

                pub const fn msg_type(&self) -> MsgType {
                    match self {
                        #(Message::#name(_) => MsgType::#name,)*
                    }
                }

                pub const fn msg_cat(&self) -> MsgCat {
                    match self {
                        #(Message::#name(msg) => msg.msg_cat(),)*
                    }
                }
            }

            #(#impl_from_msg)*

            #[derive(Clone, Debug)]
            pub struct FixtMessage {
                pub header: Header,
                pub body: Message,
                pub trailer: Trailer,
            }

            impl FixtMessage {
                pub fn serialize(&self) -> Vec<u8> {
                    let mut serializer = Serializer::new();
                    self.header.serialize(&mut serializer);
                    self.body.serialize(&mut serializer);
                    self.trailer.serialize(&mut serializer);
                    serializer.take()
                }

                pub fn deserialize(mut deserializer: Deserializer) -> Result<Box<FixtMessage>, DeserializeError> {
                    let begin_string = deserializer.begin_string();
                    if begin_string != BEGIN_STRING {
                        return Err(DeserializeError::GarbledMessage("begin string mismatch".into()));
                    }

                    let body_length = deserializer.body_length();

                    // Check if MsgType(35) is the third tag in a message.
                    let msg_type = if let Some(35) = deserializer
                        .deserialize_tag_num()
                        .map_err(|e| DeserializeError::GarbledMessage(format!("failed to parse MsgType<35>: {}", e)))?
                    {
                        deserializer.deserialize_msg_type()?
                    } else {
                        return Err(DeserializeError::GarbledMessage("MsgType<35> not third tag".into()));
                    };
                    deserializer.set_msg_type(msg_type);

                    Message::deserialize(&mut deserializer, begin_string, body_length, msg_type)
                }

                pub fn from_raw_message(raw_message: RawMessage) -> Result<Box<FixtMessage>, DeserializeError> {
                    let deserializer = Deserializer::from_raw_message(raw_message);
                    FixtMessage::deserialize(deserializer)
                }

                pub fn from_bytes(input: &[u8]) -> Result<Box<FixtMessage>, DeserializeError> {
                    let (_, raw_msg) = raw_message(input)
                        .map_err(|_| DeserializeError::GarbledMessage("Message not well formed".into()))?;
                    let deserializer = Deserializer::from_raw_message(raw_msg);
                    FixtMessage::deserialize(deserializer)
                }

                // TODO: Like chrono::Format::DelayedFormat
                pub fn dbg_fix_str(&self) -> impl fmt::Display {
                    let mut output = self.serialize();
                    for byte in output.iter_mut() {
                        if *byte == b'\x01' {
                            *byte = b'|';
                        }
                    }
                    String::from_utf8_lossy(&output).into_owned()
                }

                pub const fn msg_type(&self) -> MsgType {
                    self.body.msg_type()
                }

                pub const fn msg_cat(&self) -> MsgCat {
                    self.body.msg_cat()
                }
            }
        }
    }
}

pub fn _formatted(tokens_stream: &TokenStream) {
    use std::{
        io::prelude::*,
        process::{Command, Stdio},
    };
    let mut rustfmt = Command::new("rustfmt")
        .stdin(Stdio::piped())
        .spawn()
        .expect("failed to run rustfmt");
    rustfmt
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{}", tokens_stream).as_bytes())
        .unwrap();
    let output = rustfmt.wait_with_output().unwrap();
    std::io::stdout().write_all(&output.stdout).unwrap();
    std::io::stderr().write_all(&output.stderr).unwrap();
}
