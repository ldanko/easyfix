use std::collections::HashMap;

use convert_case::{Case, Casing};
use easyfix_dictionary::{self as dict, Dictionary, Version};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

mod admin;
mod enumeration;
mod group;
mod header;
mod member;
mod message;
mod message_enum;
mod trailer;

use self::{
    enumeration::EnumCodeGen,
    group::GroupCodeGen,
    header::Header,
    member::{EnumerableType, Member},
    message::MessageCodeGen,
    trailer::Trailer,
};

fn serde_derives(serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
    match (serde_serialize, serde_deserialize) {
        (true, true) => quote! {
            #[derive(serde::Serialize, serde::Deserialize)]
        },
        (true, false) => quote! {
            #[derive(serde::Serialize)]
        },
        (false, true) => quote! {
            #[derive(serde::Deserialize)]
        },
        (false, false) => quote! {},
    }
}

pub struct Generator {
    version: Version,
    header: Header,
    trailer: Trailer,
    messages: Vec<MessageCodeGen>,
    groups: Vec<GroupCodeGen>,
    enums: Vec<EnumCodeGen>,
    fields_names: Vec<Ident>,
    fields_numbers: Vec<u16>,
}

fn convert_members(members: &[dict::Member]) -> Vec<Member> {
    members.iter().map(Member::new).collect()
}

impl Generator {
    pub fn new(dictionary: &Dictionary) -> Generator {
        let dictionary = dictionary
            .flatten()
            .expect("Failed to flatten dictionary components");

        let header = Header::new(convert_members(dictionary.header().members()));
        let trailer = Trailer::new(convert_members(dictionary.trailer().members()));

        let app_dictionary = dictionary.subdictionary(Version::FIX50SP2);

        // Collect group definitions from dictionary API, deduped by name
        let mut groups_map: HashMap<String, GroupCodeGen> = HashMap::new();
        for group in dictionary.groups() {
            groups_map
                .entry(group.name().to_owned())
                .or_insert_with(|| {
                    GroupCodeGen::new(
                        group.name(),
                        group.num_in_group().number(),
                        convert_members(group.members()),
                    )
                });
        }
        if let Some(app_dict) = app_dictionary {
            for group in app_dict.groups() {
                groups_map
                    .entry(group.name().to_owned())
                    .or_insert_with(|| {
                        GroupCodeGen::new(
                            group.name(),
                            group.num_in_group().number(),
                            convert_members(group.members()),
                        )
                    });
            }
        }
        let mut groups: Vec<GroupCodeGen> = groups_map.into_values().collect();
        groups.sort_by_key(|g| g.num_in_group_tag());

        let all_messages = dictionary
            .messages()
            .chain(app_dictionary.into_iter().flat_map(|d| d.messages()));

        let mut messages = Vec::new();
        for msg in all_messages {
            let members_descs = convert_members(msg.members());

            messages.push(MessageCodeGen::new(
                msg.name(),
                members_descs,
                msg.msg_cat(),
            ));
        }

        // Collect all fields from FIXT dictionary and FIX50SP2 subdictionary, deduped by tag number
        let mut all_fields_map: HashMap<u16, &dict::Field> = HashMap::new();
        for field in dictionary.fields() {
            all_fields_map.entry(field.number()).or_insert(field);
        }
        if let Some(app_dict) = app_dictionary {
            for field in app_dict.fields() {
                all_fields_map.entry(field.number()).or_insert(field);
            }
        }

        let mut enums = Vec::new();
        for field in all_fields_map.values() {
            // Don't map booleans into YES/NO enumeration
            if let dict::BasicType::Boolean = field.data_type() {
                continue;
            }
            if !field.variants().is_empty() {
                let Some(enumerable_type) = EnumerableType::try_from_basic_type(field.data_type())
                else {
                    panic!(
                        "type {:?} can not be represented as enum",
                        field.data_type()
                    );
                };
                enums.push(EnumCodeGen::new(
                    field.name(),
                    field.number(),
                    enumerable_type,
                    field.variants().to_vec(),
                ));
            }
        }
        enums.sort_by_key(|e| e.tag());

        let mut fields: Vec<&dict::Field> = all_fields_map.into_values().collect();
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

        let version = dictionary.version();

        Generator {
            version,
            header,
            trailer,
            messages,
            groups,
            enums,
            fields_names,
            fields_numbers,
        }
    }

    pub fn generate_fields(&self, serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
        let enums = self
            .enums
            .iter()
            .map(|enum_| enum_.generate(serde_serialize, serde_deserialize));
        let base_enum_conversions = self
            .enums
            .iter()
            .map(|enum_| enum_.generate_base_enum_conversion());

        quote! {
            #(#enums)*

            #(#base_enum_conversions)*
        }
    }

    pub fn generate_groups(&self, serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
        let groups_defs = self
            .groups
            .iter()
            .map(|group| group.generate(serde_serialize, serde_deserialize));

        quote! {
            #(#groups_defs)*
        }
    }

    pub fn generate_messages(&self, serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
        let mut msg_names = Vec::new();

        // Generate Header and Trailer
        let header_def = self
            .header
            .generate(self.version, serde_serialize, serde_deserialize);
        let trailer_def = self.trailer.generate(serde_serialize, serde_deserialize);

        let admin_base_conversions =
            admin::generate_admin_base_conversions(&self.messages, self.version);

        let structs_defs = self
            .messages
            .iter()
            .map(|msg| msg.generate(serde_serialize, serde_deserialize));

        // Generate message structs
        for msg in &self.messages {
            msg_names.push(msg.name());
        }

        let begin_string = Literal::byte_string(&self.version.begin_string().into_bytes());
        let field_tag_def = message_enum::generate_field_tag(
            &self.fields_names,
            &self.fields_numbers,
            serde_serialize,
            serde_deserialize,
        );
        let message_enum_def =
            message_enum::generate_message_enum(&msg_names, serde_serialize, serde_deserialize);
        let fixt_message_def =
            message_enum::generate_fixt_message(serde_serialize, serde_deserialize);

        quote! {
            #[allow(unused_imports)]
            use easyfix_core::{
                basic_types::{
                    Amt, Boolean, Char, Country, Currency, Data, DayOfMonth, Decimal,
                    Exchange, FixStr, FixString, Float, Int, Language, Length,
                    LocalMktDate, LocalMktTime, MonthYear, MsgTypeField, MsgTypeValue,
                    MultipleCharValue, MultipleStringValue, NumInGroup, Percentage,
                    Price, PriceOffset, Qty, SeqNum, SessionRejectReasonField,
                    SessionRejectReasonValue, SessionStatusField, SessionStatusValue,
                    TagNum, Tenor, TenorUnit, TimePrecision, ToFixString, TzTimeOnly,
                    TzTimestamp, UtcDateOnly, UtcTimeOnly, UtcTimestamp, XmlData,
                },
                deserializer::{raw_message, DeserializeError, Deserializer, RawMessage},
                serializer::Serializer,
            };
            #[allow(unused_imports)]
            use easyfix_core::base_messages::{
                AdminBase, EncryptMethodBase, HeaderBase, HeartbeatBase, LogonBase, LogoutBase,
                RejectBase, ResendRequestBase, SessionRejectReasonBase, SequenceResetBase,
                TestRequestBase,
            };
            use easyfix_core::message::{HeaderAccess, SessionMessage};
            pub use easyfix_core::message::MsgCat;
            use std::borrow::Cow;
            use std::fmt;

            pub const BEGIN_STRING: &FixStr = unsafe { FixStr::from_ascii_unchecked(#begin_string) };

            #field_tag_def

            #header_def

            #trailer_def

            #(#structs_defs)*

            #admin_base_conversions

            #message_enum_def

            #fixt_message_def
        }
    }
}
