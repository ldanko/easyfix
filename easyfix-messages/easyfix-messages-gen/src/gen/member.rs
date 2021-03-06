use convert_case::{Case, Casing};
use easyfix_dictionary::BasicType;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

#[derive(Debug)]
pub enum Type {
    BasicType(BasicType),
    Group(Ident),
    Enum((Ident, BasicType)),
}

impl Type {
    pub fn basic_type(basic_type: BasicType) -> Type {
        Type::BasicType(basic_type)
    }

    pub fn group(name: &str) -> Type {
        Type::Group(Ident::new(
            &name.to_case(Case::UpperCamel),
            Span::call_site(),
        ))
    }

    pub fn enumeration(name: &str, basic_type: BasicType) -> Type {
        Type::Enum((
            Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            basic_type,
        ))
    }

    pub fn gen_type(&self) -> TokenStream {
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
            Type::BasicType(BasicType::String) => quote! { FixString },
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
            Type::BasicType(BasicType::Data) => None,
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
            Type::BasicType(BasicType::XmlData) => None,
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
            Type::Group(name) => {
                quote! { #name::deserialize(deserializer, first_run, expected_tags) }
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
pub struct SimpleMember {
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

    pub fn field(name: &str, tag: u16, required: bool, type_: BasicType) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::basic_type(type_))
    }

    fn enum_field(name: &str, tag: u16, required: bool, type_: BasicType) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::enumeration(name, type_))
    }

    pub fn length(name: &str, tag: u16, required: bool) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::basic_type(BasicType::Length))
    }

    pub fn num_in_group(name: &str, tag: u16, required: bool) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::basic_type(BasicType::NumInGroup))
    }

    /// Create `SimpleMember` object of `Group` type.
    ///
    /// # Arguments
    ///
    /// * `name` - group name
    /// * `tag` - tag number of NumInGroup associated field
    /// * `required` - if group presence is required
    pub fn group(name: &str, tag: u16, required: bool) -> SimpleMember {
        SimpleMember::new(name, tag, required, Type::group(name))
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
                    return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
                }
            }),
            // MsgSeqNum
            (_, 34) => Some(quote! {
                #tag => {
                    if #name.is_some() {
                        return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
                    }
                    let msg_seq_num_value = #deserialize?;
                    deserializer.set_seq_num(msg_seq_num_value);
                    #name = Some(msg_seq_num_value);
                }
            }),
            (Type::BasicType(BasicType::Length), _) => Some(quote! {
                #tag => {
                    if #name.is_some() {
                        return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
                    }
                    #name = Some(#deserialize?);
                }
            }),
            (Type::BasicType(BasicType::NumInGroup), _) => Some(quote! {
                #tag => {
                    return Err(deserializer.reject(Some(tag), SessionRejectReason::TagSpecifiedOutOfRequiredOrder));
                }
            }),
            (Type::Group(_), _) => None,
            (Type::BasicType(BasicType::Data | BasicType::XmlData), _) => Some(quote! {
                #tag => {
                    return Err(deserializer.reject(Some(tag), SessionRejectReason::TagSpecifiedOutOfRequiredOrder));
                }
            }),
            _ => Some(quote! {
                #tag => {
                    if #name.is_some() {
                        return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
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
                #name: #name.ok_or_else(|| deserializer.reject(Some(#tag), SessionRejectReason::RequiredTagMissing))?
            }
        } else {
            quote! {
                #name
            }
        }
    }
}

#[derive(Debug)]
pub struct CustomLengthMember {
    len: SimpleMember,
    value: SimpleMember,
}

#[derive(Debug)]
pub struct GroupMember {
    num_in_group: SimpleMember,
    group_body: SimpleMember,
    expected_tags: Vec<(u16, bool)>,
}

#[derive(Debug)]
pub enum MemberDesc {
    Simple(SimpleMember),
    CustomLength(CustomLengthMember),
    Group(GroupMember),
}

impl MemberDesc {
    pub fn simple(name: &str, tag: u16, required: bool, type_: BasicType) -> MemberDesc {
        MemberDesc::Simple(SimpleMember::field(name, tag, required, type_))
    }

    pub fn enumeration(name: &str, tag: u16, required: bool, type_: BasicType) -> MemberDesc {
        MemberDesc::Simple(SimpleMember::enum_field(name, tag, required, type_))
    }

    pub fn custom_length(len: SimpleMember, value: SimpleMember) -> MemberDesc {
        MemberDesc::CustomLength(CustomLengthMember { len, value })
    }

    pub fn group(
        num_in_group: SimpleMember,
        group_body: SimpleMember,
        expected_tags: Vec<(u16, bool)>,
    ) -> MemberDesc {
        MemberDesc::Group(GroupMember {
            num_in_group,
            group_body,
            expected_tags,
        })
    }

    fn _name(&self) -> &Ident {
        match self {
            MemberDesc::Simple(member) => &member.name,
            MemberDesc::CustomLength(member) => &member.len.name,
            MemberDesc::Group(member) => &member.num_in_group.name,
        }
    }

    pub fn tag_num(&self) -> u16 {
        match self {
            MemberDesc::Simple(member) => member.tag,
            MemberDesc::CustomLength(member) => member.len.tag,
            MemberDesc::Group(member) => member.num_in_group.tag,
        }
    }

    pub fn required(&self) -> bool {
        match self {
            MemberDesc::Simple(member) => member.required,
            MemberDesc::CustomLength(member) => member.len.required,
            MemberDesc::Group(member) => member.num_in_group.required,
        }
    }

    /// Generate member definition for use in structs definitions.
    pub fn gen_definition(&self) -> Option<TokenStream> {
        match self {
            MemberDesc::Simple(member) => Some(member.gen_definition()),
            MemberDesc::CustomLength(_) | MemberDesc::Group(_) => None,
        }
    }

    pub fn gen_serialize(&self) -> Option<TokenStream> {
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
                let serialize_value = match value_type {
                    Type::BasicType(BasicType::Data) => quote! { serializer.serialize_data },
                    Type::BasicType(BasicType::XmlData) => quote! { serializer.serialize_xml },
                    t => panic!("Unexpected type {:?} after `Length` field", t),
                };
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
                ..
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
    pub fn gen_opt_variables(&self) -> TokenStream {
        match self {
            MemberDesc::Simple(member) => member.gen_opt_variables(),
            MemberDesc::CustomLength(member) => member.len.gen_opt_variables(),
            MemberDesc::Group(member) => member.num_in_group.gen_opt_variables(),
        }
    }

    pub fn gen_deserialize_match_entries(&self) -> Option<TokenStream> {
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
                            return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
                        }
                        // deserialize_data()/deserialize_xml() expects
                        // the name of variable below is `len`
                        let len = #deserialize?;
                        #name = Some(len);
                        if deserializer.deserialize_tag_num()?.ok_or_else(|| {
                            deserializer.reject(Some(#next_member_tag), SessionRejectReason::RequiredTagMissing)
                        })? != #next_member_tag
                        {
                            return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagSpecifiedOutOfRequiredOrder));
                        }
                        // This should never happen, as error would be
                        // returned in #name.is_some() case.
                        if #next_member_name.is_some() {
                            return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
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
                expected_tags,
            }) => {
                let deserialize = type_.gen_deserialize();
                let group_deserialize = group_type.gen_deserialize();
                let group_name_local =
                    Ident::new(&format!("{}_local", group_name), Span::call_site());
                let expected_tags: Vec<_> = expected_tags
                    .iter()
                    .map(|(expected_tag, required)| quote! { Some((#expected_tag, #required)) })
                    .collect();
                Some(quote! {
                    #tag => {
                        if #name.is_some() {
                            return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
                        }
                        let len = #deserialize?;
                        #name = Some(len);
                        if #group_name.is_some() {
                            return Err(deserializer.reject(Some(#tag), SessionRejectReason::TagAppearsMoreThanOnce));
                        }
                        let expected_tags = &mut [#(#expected_tags),*];
                        let mut #group_name_local = Vec::with_capacity(len as usize);
                        let first_run = true;
                        #group_name_local.push(#group_deserialize?);
                        let first_run = false;
                        for _ in 1..len {
                            #group_name_local.push(#group_deserialize?);
                        }
                        #group_name = Some(#group_name_local);
                    }
                })
            }
        }
    }

    /// Generate code used to initialize structure.
    pub fn gen_deserialize_struct_entries(&self) -> Option<TokenStream> {
        match self {
            MemberDesc::Simple(member) => Some(member.gen_deserialize_struct_entries()),
            MemberDesc::CustomLength(_) | MemberDesc::Group(_) => None,
        }
    }
}
