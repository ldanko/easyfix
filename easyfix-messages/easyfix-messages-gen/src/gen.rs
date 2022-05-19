use convert_case::{Case, Casing};
use easyfix_dictionary::{BasicType, Dictionary, Member, MemberKind};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;
use std::collections::HashMap;

#[derive(Debug)]
enum Type {
    BasicType(BasicType),
    Group(Ident),
    Enum((Ident, BasicType)),
}

impl Type {
    fn basic_type(basic_type: BasicType) -> Type {
        Type::BasicType(basic_type)
    }

    fn group(name: &str) -> Type {
        Type::Group(Ident::new(
            &name.to_case(Case::UpperCamel),
            Span::call_site(),
        ))
    }

    fn enumeration(name: &str, basic_type: BasicType) -> Type {
        Type::Enum((
            Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            basic_type,
        ))
    }

    fn gen_type(&self) -> TokenStream {
        match self {
            Type::BasicType(BasicType::Amt) => quote! { Amt },
            Type::BasicType(BasicType::Boolean) => quote! { Boolean },
            Type::BasicType(BasicType::Char) => quote! { Char },
            Type::BasicType(BasicType::Country) => quote! { Country },
            Type::BasicType(BasicType::Currency) => quote! { Currency },
            Type::BasicType(BasicType::Data) => quote! { Data },
            Type::BasicType(BasicType::Exchange) => quote! { Exchange },
            Type::BasicType(BasicType::Float) => quote! { Float },
            Type::BasicType(BasicType::Int) => quote! { Int },
            Type::BasicType(BasicType::Language) => quote! { Language },
            Type::BasicType(BasicType::Length) => quote! { Length },
            Type::BasicType(BasicType::LocalMktDate) => quote! { LocalMktDate },
            Type::BasicType(BasicType::MonthYear) => quote! { MonthYear },
            Type::BasicType(BasicType::MultipleCharValue) => quote! { MultipleCharValue },
            Type::BasicType(BasicType::MultipleStringValue) => quote! { MultipleStringValue },
            Type::BasicType(BasicType::NumInGroup) => quote! { NumInGroup },
            Type::BasicType(BasicType::Percentage) => quote! { Percentage },
            Type::BasicType(BasicType::Price) => quote! { Price },
            Type::BasicType(BasicType::PriceOffset) => quote! { PriceOffset },
            Type::BasicType(BasicType::Qty) => quote! { Qty },
            Type::BasicType(BasicType::SeqNum) => quote! { SeqNum },
            Type::BasicType(BasicType::String) => quote! { Str },
            Type::BasicType(BasicType::TzTimeOnly) => quote! { TzTimeOnly },
            Type::BasicType(BasicType::TzTimestamp) => quote! { TzTimestamp },
            Type::BasicType(BasicType::UtcDateOnly) => quote! { UtcDateOnly },
            Type::BasicType(BasicType::UtcTimeOnly) => quote! { UtcTimeOnly },
            Type::BasicType(BasicType::UtcTimestamp) => quote! { UtcTimestamp },
            Type::BasicType(BasicType::XmlData) => quote! { XmlData },
            Type::Group(name) => quote! { Vec<#name> },
            // TODO: in case of enum based on NumInGroup, it seems that max
            //       group members cound should be limited to max enum value
            Type::Enum((
                name,
                BasicType::Int | BasicType::NumInGroup | BasicType::Char | BasicType::String,
            )) => {
                quote! { fields::#name }
            }
            Type::Enum((name, BasicType::MultipleCharValue | BasicType::MultipleStringValue)) => {
                quote! { Vec<fields::#name> }
            }
            Type::Enum((name, basic_type)) => panic!(
                "Unexpected underlying type ({:?}) for {} enum",
                basic_type, name
            ),
        }
    }

    fn gen_serialize(&self) -> Option<TokenStream> {
        match self {
            Type::BasicType(BasicType::Amt) => Some(quote! { serializer.serialize_amt }),
            Type::BasicType(BasicType::Boolean) => Some(quote! { serializer.serialize_boolean }),
            Type::BasicType(BasicType::Char) => Some(quote! { serializer.serialize_char }),
            Type::BasicType(BasicType::Country) => Some(quote! { serializer.serialize_country }),
            Type::BasicType(BasicType::Currency) => Some(quote! { serializer.serialize_currency }),
            Type::BasicType(BasicType::Data) => Some(quote! { serializer.serialize_data }),
            Type::BasicType(BasicType::Exchange) => Some(quote! { serializer.serialize_exchange }),
            Type::BasicType(BasicType::Float) => Some(quote! { serializer.serialize_float }),
            Type::BasicType(BasicType::Int) => Some(quote! { serializer.serialize_int }),
            Type::BasicType(BasicType::Language) => Some(quote! { serializer.serialize_language }),
            Type::BasicType(BasicType::Length) => Some(quote! { serializer.serialize_length }),
            Type::BasicType(BasicType::LocalMktDate) => {
                Some(quote! { serializer.serialize_local_mkt_date })
            }
            Type::BasicType(BasicType::MonthYear) => {
                Some(quote! { serializer.serialize_month_year })
            }
            Type::BasicType(BasicType::MultipleCharValue) => {
                Some(quote! { serializer.serialize_multiple_char_value })
            }
            Type::BasicType(BasicType::MultipleStringValue) => {
                Some(quote! { serializer.serialize_multiple_string_value })
            }
            Type::BasicType(BasicType::NumInGroup) => {
                Some(quote! { serializer.serialize_num_in_group })
            }
            Type::BasicType(BasicType::Percentage) => {
                Some(quote! { serializer.serialize_percentage })
            }
            Type::BasicType(BasicType::Price) => Some(quote! { serializer.serialize_price }),
            Type::BasicType(BasicType::PriceOffset) => {
                Some(quote! { serializer.serialize_price_offset })
            }
            Type::BasicType(BasicType::Qty) => Some(quote! { serializer.serialize_qty }),
            Type::BasicType(BasicType::SeqNum) => Some(quote! { serializer.serialize_seq_num }),
            Type::BasicType(BasicType::String) => Some(quote! { serializer.serialize_string }),
            Type::BasicType(BasicType::TzTimeOnly) => {
                Some(quote! { serializer.serialize_tz_timeonly })
            }
            Type::BasicType(BasicType::TzTimestamp) => {
                Some(quote! { serializer.serialize_tz_timestamp })
            }
            Type::BasicType(BasicType::UtcDateOnly) => {
                Some(quote! { serializer.serialize_utc_date_only })
            }
            Type::BasicType(BasicType::UtcTimeOnly) => {
                Some(quote! { serializer.serialize_utc_time_only })
            }
            Type::BasicType(BasicType::UtcTimestamp) => {
                Some(quote! { serializer.serialize_utc_timestamp })
            }
            Type::BasicType(BasicType::XmlData) => Some(quote! { serializer.serialize_xml }),
            Type::Group(_) => None,
            Type::Enum((
                _,
                BasicType::Int | BasicType::NumInGroup | BasicType::Char | BasicType::String,
            )) => Some(quote! { serializer.serialize_enum }),
            Type::Enum((_, BasicType::MultipleCharValue | BasicType::MultipleStringValue)) => {
                Some(quote! { serializer.serialize_enum_collection })
            }
            Type::Enum((name, basic_type)) => panic!(
                "Unexpected underlying type ({:?}) for {} enum",
                basic_type, name
            ),
        }
    }

    fn gen_deserialize(&self) -> TokenStream {
        match self {
            Type::BasicType(BasicType::Amt) => quote! { deserializer.deserialize_amt() },
            Type::BasicType(BasicType::Boolean) => quote! { deserializer.deserialize_boolean() },
            Type::BasicType(BasicType::Char) => quote! { deserializer.deserialize_char() },
            Type::BasicType(BasicType::Country) => quote! { deserializer.deserialize_country() },
            Type::BasicType(BasicType::Currency) => quote! { deserializer.deserialize_currency() },
            // Note `len` argument for deserializer
            Type::BasicType(BasicType::Data) => {
                quote! { deserializer.deserialize_data(len as usize) }
            }
            Type::BasicType(BasicType::Exchange) => quote! { deserializer.deserialize_exchange() },
            Type::BasicType(BasicType::Float) => quote! { deserializer.deserialize_float() },
            Type::BasicType(BasicType::Int) => quote! { deserializer.deserialize_int() },
            Type::BasicType(BasicType::Language) => quote! { deserializer.deserialize_language() },
            Type::BasicType(BasicType::Length) => quote! { deserializer.deserialize_length() },
            Type::BasicType(BasicType::LocalMktDate) => {
                quote! { deserializer.deserialize_local_mkt_date() }
            }
            Type::BasicType(BasicType::MonthYear) => {
                quote! { deserializer.deserialize_month_year() }
            }
            Type::BasicType(BasicType::MultipleCharValue) => {
                quote! { deserializer.deserialize_multiple_char_value() }
            }
            Type::BasicType(BasicType::MultipleStringValue) => {
                quote! { deserializer.deserialize_multiple_string_value() }
            }
            Type::BasicType(BasicType::NumInGroup) => {
                quote! { deserializer.deserialize_num_in_group() }
            }
            Type::BasicType(BasicType::Percentage) => {
                quote! { deserializer.deserialize_percentage() }
            }
            Type::BasicType(BasicType::Price) => quote! { deserializer.deserialize_price() },
            Type::BasicType(BasicType::PriceOffset) => {
                quote! { deserializer.deserialize_price_offset() }
            }
            Type::BasicType(BasicType::Qty) => quote! { deserializer.deserialize_qty() },
            Type::BasicType(BasicType::SeqNum) => quote! { deserializer.deserialize_seq_num() },
            Type::BasicType(BasicType::String) => quote! { deserializer.deserialize_string() },
            Type::BasicType(BasicType::TzTimeOnly) => {
                quote! { deserializer.deserialize_tz_timeonly() }
            }
            Type::BasicType(BasicType::TzTimestamp) => {
                quote! { deserializer.deserialize_tz_timestamp() }
            }
            Type::BasicType(BasicType::UtcDateOnly) => {
                quote! { deserializer.deserialize_utc_date_only() }
            }
            Type::BasicType(BasicType::UtcTimeOnly) => {
                quote! { deserializer.deserialize_utc_time_only() }
            }
            Type::BasicType(BasicType::UtcTimestamp) => {
                quote! { deserializer.deserialize_utc_timestamp() }
            }
            // Note `len` argument for deserializer
            Type::BasicType(BasicType::XmlData) => {
                quote! { deserializer.deserialize_xml(len as usize) }
            }
            // TODO
            Type::Group(name) => {
                quote! { #name::deserialize(deserializer) }
            }
            Type::Enum((_, BasicType::Int)) => {
                quote! { deserializer.deserialize_int_enum() }
            }
            Type::Enum((_, BasicType::NumInGroup)) => {
                quote! { deserializer.deserialize_num_in_group_enum() }
            }
            Type::Enum((_, BasicType::Char)) => {
                quote! { deserializer.deserialize_char_enum() }
            }
            Type::Enum((_, BasicType::MultipleCharValue)) => {
                quote! { deserializer.deserialize_multiple_char_value_enum() }
            }
            Type::Enum((_, BasicType::String)) => {
                quote! { deserializer.deserialize_string_enum() }
            }
            Type::Enum((_, BasicType::MultipleStringValue)) => {
                quote! { deserializer.deserialize_multiple_string_value_enum() }
            }
            Type::Enum((name, basic_type)) => panic!(
                "enum ({}) based on {:?} type not supported",
                name, basic_type
            ),
        }
    }
}

// TODO: check agains all rust keywords
const RESERVED: &[&str] = &["yield"];

fn is_reserved(input: &str) -> bool {
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(input))
}

#[derive(Debug)]
struct SimpleMember {
    name: Ident,
    tag: u16,
    required: bool,
    type_: Type,
}

impl SimpleMember {
    fn new(name: &str, tag: u16, required: bool, type_: Type) -> SimpleMember {
        let mut name = name.to_case(Case::Snake);
        if is_reserved(&name) {
            name.push('_');
        }
        SimpleMember {
            name: Ident::new(&name, Span::call_site()),
            tag,
            required,
            type_,
        }
    }

    fn field(name: &str, tag: u16, required: bool, type_: BasicType) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::basic_type(type_))
    }

    fn enum_field(name: &str, tag: u16, required: bool, type_: BasicType) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::enumeration(name, type_))
    }

    fn length(name: &str, tag: u16, required: bool) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::basic_type(BasicType::Length))
    }

    fn num_in_group(name: &str, tag: u16, required: bool) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::basic_type(BasicType::NumInGroup))
    }

    fn group(name: &str, required: bool, type_: &str) -> SimpleMember {
        SimpleMember::new(name, 0, required, Type::group(type_))
    }

    /// Generate member definition for use in structs definitions.
    fn gen_definition(&self) -> TokenStream {
        let name = &self.name;
        let type_ = self.type_.gen_type();
        if self.required {
            quote! {
                #name: #type_
            }
        } else {
            quote! {
                #name: Option<#type_>
            }
        }
    }

    fn gen_serialize(&self) -> Option<TokenStream> {
        if let Some(serialize_type) = self.type_.gen_serialize() {
            if self.tag == 9 {
                Some(quote! { serializer.serialize_body_len() })
            } else if self.tag == 10 {
                Some(quote! { serializer.serialize_checksum() })
            } else {
                let name = &self.name;
                let tag = format!("{}=", self.tag);
                if self.required {
                    Some(quote! {
                        //serializer.serialize_tag_num(#tag);
                        serializer.output_mut().extend_from_slice(#tag.as_bytes());
                        #serialize_type(&self.#name);
                        serializer.output_mut().push(b'\x01');
                    })
                } else {
                    Some(quote! {
                        if let Some(#name) = &self.#name {
                            serializer.output_mut().extend_from_slice(#tag.as_bytes());
                            #serialize_type(#name);
                            serializer.output_mut().push(b'\x01');
                        }
                    })
                }
            }
        } else {
            None
        }
    }

    /// Generate mutable optional variables set to None for further
    /// processig in deserializer loop.
    ///
    /// Variables for special tags like 8, 9, 10 has already known values.
    fn gen_opt_variables(&self) -> TokenStream {
        let name = &self.name;
        let type_ = self.type_.gen_type();
        match self.tag {
            8 => quote! {
                let #name = {
                    let #name = deserializer.begin_string();
                    if #name != BEGIN_STRING {
                        return Err(DeserializeError::GarbledMessage("begin string mismatch".into()));
                    }
                    #name
                };
            },
            9 => quote! { let #name = deserializer.body_length(); },
            10 => quote! { let #name = deserializer.check_sum(); },
            35 => quote! {
                // Check if MsgType(35) is the third tag in a message.
                let #name = if let Some(35) = deserializer
                    .deserialize_tag_num()
                    .map_err(|e| DeserializeError::GarbledMessage(format!("failed to parse MsgType<35>: {}", e)))?
                {
                    deserializer.deserialize_string_enum()?
                } else {
                    return Err(DeserializeError::GarbledMessage("MsgType<35> not third tag".into()));
                };
            },
            _ => quote! { let mut #name: Option<#type_> = None; },
        }
    }

    fn gen_deserialize_match_entries(&self) -> Option<TokenStream> {
        let name = &self.name;
        let tag = self.tag;
        let deserialize = self.type_.gen_deserialize();
        match (&self.type_, self.tag) {
            (_, 8 | 9 | 10 | 35) => Some(quote! {
                #tag => {
                    return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                }
            }),
            // MsgSeqNum
            (_, 34) => Some(quote! {
                #tag => {
                    if #name.is_some() {
                        return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                    }
                    let msg_seq_num_value = #deserialize?;
                    deserializer.set_seq_num(msg_seq_num_value);
                    #name = Some(msg_seq_num_value);
                }
            }),
            (Type::BasicType(BasicType::Length), _) => Some(quote! {
                #tag => {
                    if #name.is_some() {
                        return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                    }
                    #name = Some(#deserialize?);
                }
            }),
            (Type::BasicType(BasicType::NumInGroup), _) => Some(quote! {
                #tag => {
                    return Err(deserializer.reject(Some(tag), RejectReason::TagSpecifiedOutOfRequiredOrder));
                }
            }),
            (Type::Group(_), _) => None,
            (Type::BasicType(BasicType::Data | BasicType::XmlData), _) => Some(quote! {
                #tag => {
                    return Err(deserializer.reject(Some(tag), RejectReason::TagSpecifiedOutOfRequiredOrder));
                }
            }),
            _ => Some(quote! {
                #tag => {
                    if #name.is_some() {
                        return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                    }
                    #name = Some(#deserialize?);
                }
            }),
        }
    }

    /// Generate code used to initialize structure.
    fn gen_deserialize_struct_entries(&self) -> TokenStream {
        let name = &self.name;
        let tag = self.tag;
        if self.required && !matches!(self.tag, 8 | 9 | 10 | 35) {
            quote! {
                #name: #name.ok_or_else(|| deserializer.reject(Some(#tag), RejectReason::RequiredTagMissing))?
            }
        } else {
            quote! {
                #name
            }
        }
    }

    /// Genrate code used to deserialize group.
    fn gen_deserialize_group_entries(&self) -> Option<TokenStream> {
        let name = &self.name;
        let tag = self.tag;
        let deserialize = self.type_.gen_deserialize();
        let deserialize = if self.required {
            quote! { #deserialize? }
        } else {
            quote! { Some(#deserialize?) }
        };
        match self.type_ {
            Type::BasicType(BasicType::Length) => Some(quote! {
                #name: {
                    if deserializer.deserialize_tag_num()? != Some(#tag) {
                        return Err(deserializer.reject(Some(#tag), RejectReason::RequiredTagMissing));
                    }
                    #deserialize
                }
            }),
            Type::BasicType(BasicType::NumInGroup) => Some(quote! {
                #name: {
                    return Err(deserializer.reject(Some(#tag), RejectReason::TagSpecifiedOutOfRequiredOrder));
                }
            }),
            Type::Group(_) => None,
            Type::BasicType(BasicType::Data | BasicType::XmlData) => None,
            _ => Some(quote! {
                #name: {
                    if deserializer.deserialize_tag_num()? != Some(#tag) {
                        return Err(deserializer.reject(Some(#tag), RejectReason::RequiredTagMissing));
                    }
                    #deserialize
                }
            }),
        }
    }
}

#[derive(Debug)]
struct CustomLengthMember {
    len: SimpleMember,
    value: SimpleMember,
}

#[derive(Debug)]
struct GroupMember {
    num_in_group: SimpleMember,
    group_body: SimpleMember,
}

#[derive(Debug)]
enum MemberDesc {
    Simple(SimpleMember),
    CustomLength(CustomLengthMember),
    Group(GroupMember),
}

impl MemberDesc {
    fn simple(name: &str, tag: u16, required: bool, type_: BasicType) -> MemberDesc {
        MemberDesc::Simple(SimpleMember::field(name, tag, required, type_))
    }

    fn enumeration(name: &str, tag: u16, required: bool, type_: BasicType) -> MemberDesc {
        MemberDesc::Simple(SimpleMember::enum_field(name, tag, required, type_))
    }

    fn custom_length(len: SimpleMember, value: SimpleMember) -> MemberDesc {
        MemberDesc::CustomLength(CustomLengthMember { len, value })
    }

    fn group(num_in_group: SimpleMember, group_body: SimpleMember) -> MemberDesc {
        MemberDesc::Group(GroupMember {
            num_in_group,
            group_body,
        })
    }

    fn _name(&self) -> &Ident {
        match self {
            MemberDesc::Simple(member) => &member.name,
            MemberDesc::CustomLength(member) => &member.len.name,
            MemberDesc::Group(member) => &member.num_in_group.name,
        }
    }

    /// Generate member definition for use in structs definitions.
    fn gen_definition(&self) -> Option<TokenStream> {
        match self {
            MemberDesc::Simple(member) => Some(member.gen_definition()),
            MemberDesc::CustomLength(_) | MemberDesc::Group(_) => None,
        }
    }

    fn gen_serialize(&self) -> Option<TokenStream> {
        match self {
            MemberDesc::Simple(member) => member.gen_serialize(),
            MemberDesc::CustomLength(CustomLengthMember {
                len:
                    SimpleMember {
                        tag: len_tag,
                        required,
                        ..
                    },
                value:
                    SimpleMember {
                        name: value_name,
                        tag: value_tag,
                        type_: value_type,
                        ..
                    },
            }) => {
                let len_tag = format!("{}=", len_tag);
                let value_tag = format!("{}=", value_tag);
                let serialize_value = value_type.gen_serialize();
                if *required {
                    Some(quote! {
                        serializer.output_mut().extend_from_slice(#len_tag.as_bytes());
                        serializer.serialize_length(&(self.#value_name.len() as u16));
                        serializer.output_mut().push(b'\x01');
                        serializer.output_mut().extend_from_slice(#value_tag.as_bytes());
                        #serialize_value(&self.#value_name);
                        serializer.output_mut().push(b'\x01');
                    })
                } else {
                    Some(quote! {
                        if let Some(#value_name) = &self.#value_name {
                            serializer.output_mut().extend_from_slice(#len_tag.as_bytes());
                            serializer.serialize_length(&(#value_name.len() as u16));
                            serializer.output_mut().push(b'\x01');
                            serializer.output_mut().extend_from_slice(#value_tag.as_bytes());
                            #serialize_value(#value_name);
                            serializer.output_mut().push(b'\x01');
                        }
                    })
                }
            }
            MemberDesc::Group(GroupMember {
                num_in_group:
                    SimpleMember {
                        tag: num_in_group_tag,
                        required,
                        ..
                    },
                group_body:
                    SimpleMember {
                        name: group_name, ..
                    },
            }) => {
                let num_in_group_tag = format!("{}=", num_in_group_tag);
                if *required {
                    Some(quote! {
                        serializer.output_mut().extend_from_slice(#num_in_group_tag.as_bytes());
                        serializer.serialize_num_in_group(&(self.#group_name.len() as NumInGroup));
                        serializer.output_mut().push(b'\x01');
                        for entry in &self.#group_name {
                            entry.serialize(serializer);
                        }
                    })
                } else {
                    Some(quote! {
                        if let Some(#group_name) = &self.#group_name {
                            serializer.output_mut().extend_from_slice(#num_in_group_tag.as_bytes());
                            serializer.serialize_num_in_group(&(#group_name.len() as NumInGroup));
                            serializer.output_mut().push(b'\x01');
                            for entry in #group_name {
                                entry.serialize(serializer);
                            }
                        }
                    })
                }
            }
        }
    }

    /// Generate mutable optional variables set to None for further
    /// processig in deserializer loop.
    ///
    /// Variables for special tags like 8, 9, 10 has already known values.
    fn gen_opt_variables(&self) -> TokenStream {
        match self {
            MemberDesc::Simple(member) => member.gen_opt_variables(),
            MemberDesc::CustomLength(member) => member.len.gen_opt_variables(),
            MemberDesc::Group(member) => member.num_in_group.gen_opt_variables(),
        }
    }

    fn gen_deserialize_match_entries(&self) -> Option<TokenStream> {
        match self {
            MemberDesc::Simple(member) => member.gen_deserialize_match_entries(),
            MemberDesc::CustomLength(CustomLengthMember {
                len: SimpleMember {
                    name, tag, type_, ..
                },
                value:
                    SimpleMember {
                        name: next_member_name,
                        tag: next_member_tag,
                        type_: next_member_type,
                        ..
                    },
            }) => {
                let deserialize = type_.gen_deserialize();
                let next_member_deserialize = next_member_type.gen_deserialize();
                Some(quote! {
                    #tag => {
                        if #name.is_some() {
                            return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                        }
                        // deserialize_data()/deserialize_xml() expects
                        // the name of variable below is `len`
                        let len = #deserialize?;
                        #name = Some(len);
                        if deserializer.deserialize_tag_num()?.ok_or_else(|| {
                            deserializer.reject(Some(#next_member_tag), RejectReason::RequiredTagMissing)
                        })? != #next_member_tag
                        {
                            return Err(deserializer.reject(Some(#tag), RejectReason::TagSpecifiedOutOfRequiredOrder));
                        }
                        // This should never happen, as error would be
                        // returned in #name.is_some() case.
                        if #next_member_name.is_some() {
                            return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                        }
                        #next_member_name = Some(#next_member_deserialize?);
                    }
                })
            }
            MemberDesc::Group(GroupMember {
                num_in_group:
                    SimpleMember {
                        name, tag, type_, ..
                    },
                group_body:
                    SimpleMember {
                        name: group_name,
                        type_: group_type,
                        ..
                    },
            }) => {
                let deserialize = type_.gen_deserialize();
                let group_deserialize = group_type.gen_deserialize();
                let group_name_local =
                    Ident::new(&format!("{}_local", group_name), Span::call_site());
                Some(quote! {
                    #tag => {
                        if #name.is_some() {
                            return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                        }
                        let len = #deserialize?;
                        #name = Some(len);
                        if #group_name.is_some() {
                            return Err(deserializer.reject(Some(#tag), RejectReason::TagAppearsMoreThanOnce));
                        }
                        let mut #group_name_local = Vec::with_capacity(len as usize);
                        for _ in 0..len {
                            #group_name_local.push(#group_deserialize?);
                        }
                        #group_name = Some(#group_name_local);
                    }
                })
            }
        }
    }

    /// Generate code used to initialize structure.
    fn gen_deserialize_struct_entries(&self) -> Option<TokenStream> {
        match self {
            MemberDesc::Simple(member) => Some(member.gen_deserialize_struct_entries()),
            MemberDesc::CustomLength(_) | MemberDesc::Group(_) => None,
        }
    }

    /// Genrate code used to deserialize group.
    fn gen_deserialize_group_entries(&self) -> Option<TokenStream> {
        match self {
            MemberDesc::Simple(member) => member.gen_deserialize_group_entries(),
            MemberDesc::CustomLength(CustomLengthMember {
                len: SimpleMember { tag, type_, .. },
                value:
                    SimpleMember {
                        name: next_member_name,
                        tag: next_member_tag,
                        type_: next_member_type,
                        required: next_member_required,
                    },
            }) => {
                let deserialize = type_.gen_deserialize();
                let next_member_deserialize = next_member_type.gen_deserialize();

                let next_member_deserialize = if *next_member_required {
                    quote! { #next_member_deserialize? }
                } else {
                    quote! { Some(#next_member_deserialize?) }
                };
                Some(quote! {
                    #next_member_name: {
                        // TODO: this is wrong for optional members (same for groups)
                        if deserializer.deserialize_tag_num()? != Some(#tag) {
                            return Err(deserializer.reject(Some(#tag), RejectReason::RequiredTagMissing));
                        }
                        // deserialize_data()/deserialize_xml() expects
                        // the name of variable below is `len`
                        let len = #deserialize?;

                        if deserializer.deserialize_tag_num()?.ok_or_else(|| {
                            deserializer.reject(Some(#next_member_tag), RejectReason::RequiredTagMissing)
                        })? != #next_member_tag
                        {
                            return Err(deserializer.reject(Some(#next_member_tag), RejectReason::TagSpecifiedOutOfRequiredOrder));
                        }
                        #next_member_deserialize
                    }
                })
            }
            MemberDesc::Group(GroupMember {
                num_in_group:
                    SimpleMember {
                        tag,
                        type_,
                        required,
                        ..
                    },
                group_body:
                    SimpleMember {
                        name: group_name,
                        type_: group_type,
                        ..
                    },
            }) => {
                let deserialize = type_.gen_deserialize();
                let group_deserialize = group_type.gen_deserialize();
                let group_name_local =
                    Ident::new(&format!("{}_local", group_name), Span::call_site());
                let group_name_ret = if *required {
                    quote! { #group_name_local }
                } else {
                    quote! { Some(#group_name_local) }
                };
                Some(quote! {
                    #group_name: {
                        if deserializer.deserialize_tag_num()? != Some(#tag) {
                            return Err(deserializer.reject(Some(#tag), RejectReason::RequiredTagMissing));
                        }
                        let len = #deserialize?;
                        let mut #group_name_local = Vec::with_capacity(len as usize);
                        for _ in 0..len {
                            #group_name_local.push(#group_deserialize?);
                        }
                        #group_name_ret
                    }
                })
            }
        }
    }
}

struct Struct {
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
    fn new(name: &str, members: Vec<MemberDesc>, msg_type: Option<Vec<u8>>) -> Struct {
        Struct {
            name: Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            members,
            msg_type,
        }
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

    fn generate_de_message(&self, msg_type: &[u8]) -> TokenStream {
        let name = &self.name;
        let _msg_type = Literal::byte_string(&msg_type);
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
        {
            let num_in_group_tag = 123;
            let _group_example = quote! {
                pub fn deserialize(deserializer: &mut Deserializer) -> Result<#name, DeserializeError> {
                    let f1 = None;
                    let f2 = None;
                    let len = deserializer.deserialize_tag_num()? == #num_in_group_tag {
                        deserializer.deserialize_num_in_group()?
                    } else {
                        return Err();
                    }
                    let grp = Vec::with_capacity(len);
                    for i in 0..len {
                        if let Some(last) = grp.last() {
                        } else {
                        }
                    }
                }
            };
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

    fn generate_de_group(&self) -> TokenStream {
        let group_name = &self.name;
        let mut de_group_entries = Vec::with_capacity(self.members.len());
        for member in &self.members {
            if let Some(de_group_entry) = member.gen_deserialize_group_entries() {
                de_group_entries.push(de_group_entry);
            }
        }
        quote! {
            // TODO: This assumes all group fields are required, which is not true!
            //let result = Vec::with_capacity(len as usize);
            //for i in 0..len {
            //    result.push(#group_entry_name {
            //        #(#de_group_entries,)*
            //    });
            //}
            Ok(#group_name {
                #(#de_group_entries,)*
            })
            //Ok(#group_name (result))
        }
    }

    fn generate(&self) -> TokenStream {
        let name = &self.name;

        let mut members_definitions = Vec::with_capacity(self.members.len());
        for member in &self.members {
            if let Some(member_def) = member.gen_definition() {
                members_definitions.push(member_def);
            }
        }

        let deserialize_body = if let Some(msg_type) = &self.msg_type {
            self.generate_de_message(msg_type)
        } else if self.name == "Header" {
            self.generate_de_header()
        } else if self.name == "Trailer" {
            self.generate_de_trailer()
        } else {
            self.generate_de_group()
        };

        let serialize = self.generate_serialize();

        quote! {
            #[derive(Debug)]
            pub struct #name {
                #(pub #members_definitions,)*
            }

            impl #name {
                fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                fn deserialize(deserializer: &mut Deserializer) -> Result<#name, DeserializeError> {
                    #deserialize_body
                }
            }
        }
    }
}

struct EnumDesc {
    name: Ident,
    type_: BasicType,
    // (VarianName, VariantValue, VariantValueAsBytes)
    values: Vec<(Ident, Literal, Literal)>,
}

impl EnumDesc {
    fn new(name: Ident, type_: BasicType, values: Vec<(Ident, Literal, Literal)>) -> EnumDesc {
        EnumDesc {
            name,
            type_,
            values,
        }
    }

    fn generate(&self) -> TokenStream {
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
            quote! { match input.as_slice() }
        } else {
            quote! { match input }
        };
        quote! {
            #[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
                type Error = RejectReason;

                fn try_from(input: #type_) -> Result<#name, RejectReason> {
                    #try_from_match_input {
                        #(#variant_value => Ok(#name::#variant_name),)*
                        _ => Err(RejectReason::ValueIsIncorrect),
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
                    ));
                    members_descs.push(MemberDesc::Simple(SimpleMember::group(
                        member.name(),
                        number_of_elements.required(),
                        component.name(),
                    )));
                    let mut group_members = Vec::new();
                    process_members(component.members(), dictionary, &mut group_members, groups);
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
                        //println!("Found Length member");
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
            if let Some(_) = struct_.msg_type {
                name.push(struct_.name.clone());
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
