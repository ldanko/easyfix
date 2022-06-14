mod enumeration;
mod member;
mod structure;

use crate::gen::{
    enumeration::EnumDesc, member::MemberDesc, member::SimpleMember, structure::Struct,
};
use convert_case::{Case, Casing};
use easyfix_dictionary::{BasicType, Dictionary, Member, MemberKind};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;
use std::collections::HashMap;

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

                    members_descs.push(MemberDesc::group(
                        SimpleMember::num_in_group(
                            number_of_elements.name(),
                            number_of_elements_field.number(),
                            number_of_elements.required(),
                        ),
                        SimpleMember::group(
                            member.name(),
                            number_of_elements.required(),
                            component.name(),
                        ),
                        group_members
                            .iter()
                            .filter(|member| matches!(member, MemberDesc::Simple(_)))
                            .map(|member| (member.tag_num(), member.required()))
                            .collect(),
                    ));
                    members_descs.push(MemberDesc::Simple(SimpleMember::group(
                        member.name(),
                        number_of_elements.required(),
                        component.name(),
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
                    .expect("unknown field");

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
        let begin_string = if let Some(fixt_version) = dictionary.fixt_version() {
            if fixt_version.service_pack() == 0 {
                format!("FIXT.{}.{}", fixt_version.major(), fixt_version.minor())
            } else {
                format!(
                    "FIXT.{}.{}SP{}",
                    fixt_version.major(),
                    fixt_version.minor(),
                    fixt_version.service_pack()
                )
            }
        } else if let Some(fix_version) = dictionary.fix_version() {
            if fix_version.service_pack() == 0 {
                format!("FIX.{}.{}", fix_version.major(), fix_version.minor())
            } else {
                format!(
                    "FIX.{}.{}SP{}",
                    fix_version.major(),
                    fix_version.minor(),
                    fix_version.service_pack()
                )
            }
        } else {
            panic!("Neither FIX nor FIXT version defined");
        }
        .into_bytes();

        let mut structs = Vec::new();
        let mut groups = HashMap::new();

        let header = dictionary.header().expect("Missing FIX header definition");
        {
            let mut header_members = Vec::new();
            process_members(
                header.members(),
                dictionary,
                &mut header_members,
                &mut groups,
            );
            structs.push(Struct::new(header.name(), header_members, None));
        }

        let trailer = dictionary
            .trailer()
            .expect("Missing FIX trailer definition");
        {
            let mut trailer_members = Vec::new();
            process_members(
                trailer.members(),
                dictionary,
                &mut trailer_members,
                &mut groups,
            );
            structs.push(Struct::new(trailer.name(), trailer_members, None));
        }

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
                Some(msg.msgtype().into()),
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

    pub fn generate(&self) -> TokenStream {
        let mut structs_defs = Vec::new();
        let mut name = Vec::new();
        for struct_ in &self.structs {
            structs_defs.push(struct_.generate());
            if let Some(_) = struct_.msg_type() {
                name.push(struct_.name().clone());
            }
        }

        let mut enums = Vec::new();
        for enum_ in &self.enums {
            enums.push(enum_.generate());
        }

        let begin_string = Literal::byte_string(&self.begin_string);
        let fields_names = &self.fields_names;
        let fields_numbers = &self.fields_numbers;

        quote! {
            use crate::{
                deserializer::Deserializer,
                parser::{raw_message, RawMessage},
                serializer::Serializer,
                types::*,
            };
            use std::fmt::Display;

            pub const BEGIN_STRING: &[u8] = #begin_string;

            #[repr(u16)]
            pub enum FieldTag {
                #(#fields_names = #fields_numbers,)*
            }

            pub mod fields {
                use crate::{
                    types::*,
                };

                #(#enums)*
            }

            use fields::MsgType;

            #(#structs_defs)*

            #[derive(Debug)]
            pub enum Message {
                #(#name(#name),)*
            }

            impl Message {
                fn serialize(&self, serializer: &mut Serializer) {
                    match self {
                        #(Message::#name(msg) => msg.serialize(serializer),)*
                    }
                }

                fn deserialize(deserializer: &mut Deserializer, msg_type: MsgType) -> Result<Message, DeserializeError> {
                    match msg_type {
                        #(MsgType::#name => Ok(Message::#name(#name::deserialize(deserializer)?)),)*
                        #[allow(unreachable_patterns)]
                        _ => Err(deserializer.reject(None, RejectReason::InvalidMsgType)),
                    }
                }

                pub fn msg_type(&self) -> MsgType {
                    match self {
                        #(Message::#name(_) => MsgType::#name,)*
                    }
                }
            }

            #[derive(Debug)]
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

                pub fn deserialize(mut deserializer: Deserializer) -> Result<FixtMessage, DeserializeError> {
                    let header = Header::deserialize(&mut deserializer)?;
                    let body = Message::deserialize(&mut deserializer, header.msg_type)?;
                    let trailer = Trailer::deserialize(&mut deserializer)?;

                    Ok(FixtMessage {
                        header,
                        body,
                        trailer,
                    })
                }

                pub fn from_raw_message(raw_message: RawMessage) -> Result<FixtMessage, DeserializeError> {
                    let deserializer = Deserializer::from_raw_message(raw_message);
                    FixtMessage::deserialize(deserializer)
                }

                pub fn from_bytes(input: &[u8]) -> Result<FixtMessage, DeserializeError> {
                    let (_, raw_msg) = raw_message(input)
                        .map_err(|_| DeserializeError::GarbledMessage("Message not well formed".into()))?;
                    let deserializer = Deserializer::from_raw_message(raw_msg);
                    FixtMessage::deserialize(deserializer)
                }

                // TODO: Like chrono::Format::DelayedFormat
                pub fn dbg_fix_str(&self) -> impl Display {
                    let mut output = self.serialize();
                    for i in 0..output.len() {
                        if output[i] == b'\x01' {
                            output[i] = b'|';
                        }
                    }
                    String::from_utf8_lossy(&output).into_owned()
                }
            }
        }
    }
}

pub fn _formatted(tokens_stream: &TokenStream) {
    use std::io::prelude::*;
    use std::process::{Command, Stdio};
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
