use std::{collections::HashSet, rc::Rc, sync::LazyLock};

use convert_case::{Case, Casing};
use easyfix_dictionary::{self as dict, BasicType};
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

fn to_snake_case_ident(name: &str) -> Ident {
    let mut name = name.to_case(Case::Snake);
    if is_reserved(&name) {
        // TODO: or maybe `r#reserved`?
        name.push('_');
    }
    Ident::new(&name, Span::call_site())
}

fn to_upper_snake_case_ident(name: &str) -> Ident {
    Ident::new(&name.to_case(Case::UpperCamel), Span::call_site())
}

static RESERVED: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "Self",
        "_",
        "abstract",
        "as",
        "async",
        "await",
        "become",
        "box",
        "break",
        "const",
        "continue",
        "crate",
        "do",
        "dyn",
        "else",
        "enum",
        "extern",
        "false",
        "final",
        "fn",
        "for",
        "gen",
        "if",
        "impl",
        "in",
        "let",
        "loop",
        "macro",
        "macro_rules",
        "match",
        "mod",
        "move",
        "mut",
        "override",
        "priv",
        "pub",
        "raw",
        "ref",
        "return",
        "safe",
        "self",
        "static",
        "struct",
        "super",
        "trait",
        "true",
        "try",
        "type",
        "typeof",
        "union",
        "unsafe",
        "unsized",
        "use",
        "virtual",
        "where",
        "while",
        "yield",
    ])
});

fn is_reserved(input: &str) -> bool {
    RESERVED.contains(input)
}

/// BasicType variants that can be represented as Rust enumerations.
/// Only 6 of 27 BasicType variants support enumerations.
#[derive(Debug, Clone, Copy)]
pub enum EnumerableType {
    Int,
    NumInGroup,
    Char,
    String,
    MultipleCharValue,
    MultipleStringValue,
}

impl EnumerableType {
    /// Single validation point. Replaces scattered panic sites.
    pub fn try_from_basic_type(bt: BasicType) -> Option<EnumerableType> {
        match bt {
            BasicType::Int => Some(EnumerableType::Int),
            BasicType::NumInGroup => Some(EnumerableType::NumInGroup),
            BasicType::Char => Some(EnumerableType::Char),
            BasicType::String => Some(EnumerableType::String),
            BasicType::MultipleCharValue => Some(EnumerableType::MultipleCharValue),
            BasicType::MultipleStringValue => Some(EnumerableType::MultipleStringValue),
            _ => None,
        }
    }

    /// Returns the underlying `BasicType` this enumerable type was created from.
    pub fn to_basic_type(self) -> BasicType {
        match self {
            EnumerableType::Int => BasicType::Int,
            EnumerableType::NumInGroup => BasicType::NumInGroup,
            EnumerableType::Char => BasicType::Char,
            EnumerableType::String => BasicType::String,
            EnumerableType::MultipleCharValue => BasicType::MultipleCharValue,
            EnumerableType::MultipleStringValue => BasicType::MultipleStringValue,
        }
    }

    fn is_multiple(&self) -> bool {
        matches!(self, Self::MultipleCharValue | Self::MultipleStringValue)
    }

    fn gen_type(&self, enum_name: &Ident) -> TokenStream {
        if self.is_multiple() {
            quote! { Vec<fields::#enum_name> }
        } else {
            // TODO: in case of enum based on NumInGroup, it seems that max
            //       group members count should be limited to max enum value
            quote! { fields::#enum_name }
        }
    }

    fn serialize_call(&self) -> TokenStream {
        if self.is_multiple() {
            quote! { serializer.serialize_enum_collection }
        } else {
            quote! { serializer.serialize_enum }
        }
    }

    fn deserialize_call(&self) -> TokenStream {
        match self {
            EnumerableType::Int => quote! { deserializer.deserialize_int_enum() },
            EnumerableType::NumInGroup => quote! { deserializer.deserialize_num_in_group_enum() },
            EnumerableType::Char => quote! { deserializer.deserialize_char_enum() },
            EnumerableType::String => quote! { deserializer.deserialize_string_enum() },
            EnumerableType::MultipleCharValue => {
                quote! { deserializer.deserialize_multiple_char_value_enum() }
            }
            EnumerableType::MultipleStringValue => {
                quote! { deserializer.deserialize_multiple_string_value_enum() }
            }
        }
    }

    pub fn literal(&self, value: &str) -> Literal {
        match self {
            EnumerableType::String | EnumerableType::MultipleStringValue => {
                Literal::byte_string(value.as_bytes())
            }
            EnumerableType::Char | EnumerableType::MultipleCharValue => {
                Literal::u8_suffixed(*value.as_bytes().first().expect("Invalid variant value"))
            }
            EnumerableType::Int => {
                Literal::i64_suffixed(value.parse().expect("Invalid variant value"))
            }
            EnumerableType::NumInGroup => {
                Literal::u8_suffixed(value.parse().expect("Invalid variant value"))
            }
        }
    }
}

/// Code generation helpers for BasicType.
/// Wraps BasicType to provide token generation without modifying the dictionary crate.
#[derive(Debug)]
struct BasicTypeCodeGen(BasicType);

impl BasicTypeCodeGen {
    fn rust_type(&self) -> TokenStream {
        match self.0 {
            BasicType::Amt => quote! { Amt },
            BasicType::Boolean => quote! { Boolean },
            BasicType::Char => quote! { Char },
            BasicType::Country => quote! { Country },
            BasicType::Currency => quote! { Currency },
            BasicType::Data => quote! { Data },
            BasicType::Exchange => quote! { Exchange },
            BasicType::Float => quote! { Float },
            BasicType::Int => quote! { Int },
            BasicType::Language => quote! { Language },
            BasicType::Length => quote! { Length },
            BasicType::LocalMktDate => quote! { LocalMktDate },
            BasicType::MonthYear => quote! { MonthYear },
            BasicType::MultipleCharValue => quote! { MultipleCharValue },
            BasicType::MultipleStringValue => quote! { MultipleStringValue },
            BasicType::NumInGroup => quote! { NumInGroup },
            BasicType::Percentage => quote! { Percentage },
            BasicType::Price => quote! { Price },
            BasicType::PriceOffset => quote! { PriceOffset },
            BasicType::Qty => quote! { Qty },
            BasicType::SeqNum => quote! { SeqNum },
            BasicType::String => quote! { FixString },
            BasicType::Tenor => quote! { Tenor },
            BasicType::TzTimeOnly => quote! { TzTimeOnly },
            BasicType::TzTimestamp => quote! { TzTimestamp },
            BasicType::UtcDateOnly => quote! { UtcDateOnly },
            BasicType::UtcTimeOnly => quote! { UtcTimeOnly },
            BasicType::UtcTimestamp => quote! { UtcTimestamp },
            BasicType::XmlData => quote! { XmlData },
        }
    }

    fn serialize_call(&self) -> TokenStream {
        match self.0 {
            BasicType::Amt => quote! { serializer.serialize_amt },
            BasicType::Boolean => quote! { serializer.serialize_boolean },
            BasicType::Char => quote! { serializer.serialize_char },
            BasicType::Country => quote! { serializer.serialize_country },
            BasicType::Currency => quote! { serializer.serialize_currency },
            BasicType::Exchange => quote! { serializer.serialize_exchange },
            BasicType::Float => quote! { serializer.serialize_float },
            BasicType::Int => quote! { serializer.serialize_int },
            BasicType::Language => quote! { serializer.serialize_language },
            BasicType::Length => quote! { serializer.serialize_length },
            BasicType::LocalMktDate => quote! { serializer.serialize_local_mkt_date },
            BasicType::MonthYear => quote! { serializer.serialize_month_year },
            BasicType::MultipleCharValue => {
                quote! { serializer.serialize_multiple_char_value }
            }
            BasicType::MultipleStringValue => {
                quote! { serializer.serialize_multiple_string_value }
            }
            BasicType::Percentage => quote! { serializer.serialize_percentage },
            BasicType::Price => quote! { serializer.serialize_price },
            BasicType::PriceOffset => quote! { serializer.serialize_price_offset },
            BasicType::Qty => quote! { serializer.serialize_qty },
            BasicType::SeqNum => quote! { serializer.serialize_seq_num },
            BasicType::String => quote! { serializer.serialize_string },
            BasicType::Tenor => quote! { serializer.serialize_tenor },
            BasicType::TzTimeOnly => quote! { serializer.serialize_tz_timeonly },
            BasicType::TzTimestamp => quote! { serializer.serialize_tz_timestamp },
            BasicType::UtcDateOnly => quote! { serializer.serialize_utc_date_only },
            BasicType::UtcTimeOnly => quote! { serializer.serialize_utc_time_only },
            BasicType::UtcTimestamp => quote! { serializer.serialize_utc_timestamp },
            // These types are handled by dedicated code paths (RawData, Group)
            // and should never appear as standalone fields in BasicTypeCodeGen.
            BasicType::Data | BasicType::XmlData | BasicType::NumInGroup => {
                unreachable!("BasicTypeCodeGen should not be used for {:?}", self.0)
            }
        }
    }

    fn deserialize_call(&self) -> TokenStream {
        match self.0 {
            BasicType::Amt => quote! { deserializer.deserialize_amt() },
            BasicType::Boolean => quote! { deserializer.deserialize_boolean() },
            BasicType::Char => quote! { deserializer.deserialize_char() },
            BasicType::Country => quote! { deserializer.deserialize_country() },
            BasicType::Currency => quote! { deserializer.deserialize_currency() },
            BasicType::Exchange => quote! { deserializer.deserialize_exchange() },
            BasicType::Float => quote! { deserializer.deserialize_float() },
            BasicType::Int => quote! { deserializer.deserialize_int() },
            BasicType::Language => quote! { deserializer.deserialize_language() },
            BasicType::Length => quote! { deserializer.deserialize_length() },
            BasicType::LocalMktDate => quote! { deserializer.deserialize_local_mkt_date() },
            BasicType::MonthYear => quote! { deserializer.deserialize_month_year() },
            BasicType::MultipleCharValue => {
                quote! { deserializer.deserialize_multiple_char_value() }
            }
            BasicType::MultipleStringValue => {
                quote! { deserializer.deserialize_multiple_string_value() }
            }
            BasicType::Percentage => quote! { deserializer.deserialize_percentage() },
            BasicType::Price => quote! { deserializer.deserialize_price() },
            BasicType::PriceOffset => quote! { deserializer.deserialize_price_offset() },
            BasicType::Qty => quote! { deserializer.deserialize_qty() },
            BasicType::SeqNum => quote! { deserializer.deserialize_seq_num() },
            BasicType::String => quote! { deserializer.deserialize_string() },
            BasicType::Tenor => quote! { deserializer.deserialize_tenor() },
            BasicType::TzTimeOnly => quote! { deserializer.deserialize_tz_timeonly() },
            BasicType::TzTimestamp => quote! { deserializer.deserialize_tz_timestamp() },
            BasicType::UtcDateOnly => quote! { deserializer.deserialize_utc_date_only() },
            BasicType::UtcTimeOnly => quote! { deserializer.deserialize_utc_time_only() },
            BasicType::UtcTimestamp => quote! { deserializer.deserialize_utc_timestamp() },
            // These types are handled by dedicated code paths (RawData, Group)
            // and should never appear as standalone fields in BasicTypeCodeGen.
            BasicType::Data | BasicType::XmlData | BasicType::NumInGroup => {
                unreachable!("BasicTypeCodeGen should not be used for {:?}", self.0)
            }
        }
    }
}

/// Special FIX tags that require non-standard handling during serialization
/// and deserialization. Centralizes all magic-number tag checks.
#[derive(Debug, Clone, Copy)]
enum SpecialTag {
    BeginString, // 8
    BodyLength,  // 9
    CheckSum,    // 10
    MsgSeqNum,   // 34
    MsgType,     // 35
}

impl SpecialTag {
    fn from_tag(tag: u16) -> Option<SpecialTag> {
        match tag {
            8 => Some(SpecialTag::BeginString),
            9 => Some(SpecialTag::BodyLength),
            10 => Some(SpecialTag::CheckSum),
            34 => Some(SpecialTag::MsgSeqNum),
            35 => Some(SpecialTag::MsgType),
            _ => None,
        }
    }

    /// Tags that skip the required-field check in struct initialization.
    fn skips_required_check(&self) -> bool {
        matches!(
            self,
            SpecialTag::BeginString
                | SpecialTag::BodyLength
                | SpecialTag::CheckSum
                | SpecialTag::MsgType
        )
    }
}

#[derive(Debug)]
enum DataType {
    BasicType(BasicTypeCodeGen),
    EnumerableType(Ident, EnumerableType),
}

impl DataType {
    fn new(field: &dict::Field) -> DataType {
        match (
            field.variants().is_empty(),
            EnumerableType::try_from_basic_type(field.data_type()),
        ) {
            (false, None) => {
                if matches!(field.data_type(), BasicType::Boolean) {
                    // TOOD: Y/N case in booleans. Fix this in directory
                    DataType::BasicType(BasicTypeCodeGen(BasicType::Boolean))
                } else {
                    panic!(
                        "found variants in non enumerable type: {:?}",
                        field.data_type()
                    )
                }
            }
            (false, Some(et)) => {
                DataType::EnumerableType(to_upper_snake_case_ident(field.name()), et)
            }
            (true, _) => DataType::BasicType(BasicTypeCodeGen(field.data_type())),
        }
    }

    fn gen_serialize_call(&self) -> TokenStream {
        match self {
            DataType::BasicType(bt) => bt.serialize_call(),
            DataType::EnumerableType(_, et) => et.serialize_call(),
        }
    }

    fn gen_deserialize_call(&self) -> TokenStream {
        match self {
            DataType::BasicType(bt) => bt.deserialize_call(),
            DataType::EnumerableType(_, et) => et.deserialize_call(),
        }
    }
}

struct Field {
    name: Ident,
    number: u16,
    data_type: DataType,
}

impl Field {
    fn new(field: &dict::Field) -> Field {
        let name = to_snake_case_ident(field.name());
        let number = field.number();
        let data_type = DataType::new(field);
        Field {
            name,
            number,
            data_type,
        }
    }

    fn gen_type(&self) -> TokenStream {
        match &self.data_type {
            DataType::BasicType(bt) => bt.rust_type(),
            DataType::EnumerableType(name, et) => et.gen_type(name),
        }
    }

    fn gen_definition(&self, required: bool) -> TokenStream {
        let name = &self.name;
        let data_type = self.gen_type();
        let doc_comment = format!("Tag {}.", self.number);
        if required {
            quote! {
                #[doc = #doc_comment]
                pub #name: #data_type
            }
        } else {
            quote! {
                #[doc = #doc_comment]
                pub #name: Option<#data_type>
            }
        }
    }

    fn gen_serialize(&self, required: bool) -> TokenStream {
        let name = &self.name;
        let tag = Literal::byte_string(format!("{}=", self.number).as_bytes());

        let serialize_call = self.data_type.gen_serialize_call();

        match SpecialTag::from_tag(self.number) {
            Some(SpecialTag::BodyLength) => quote! { serializer.serialize_body_len() },
            Some(SpecialTag::CheckSum) => quote! { serializer.serialize_checksum() },
            _ => {
                if required {
                    quote! {
                        serializer.output_mut().extend_from_slice(#tag);
                        #serialize_call(&self.#name);
                        serializer.output_mut().push(b'\x01');
                    }
                } else {
                    quote! {
                        if let Some(#name) = &self.#name {
                            serializer.output_mut().extend_from_slice(#tag);
                            #serialize_call(#name);
                            serializer.output_mut().push(b'\x01');
                        }
                    }
                }
            }
        }
    }

    fn gen_deserialize(&self) -> TokenStream {
        self.data_type.gen_deserialize_call()
    }

    /// Generate mutable optional variables set to None for further
    /// processig in deserializer loop.
    ///
    /// Variables for special tags like 8, 9, 10 are ignored here as they
    /// has already known values.
    ///
    /// ```rust,ignore
    /// let mut next_expected_msg_seq_num: Option<SeqNum> = None;
    /// let mut session_status: Option<fields::SessionStatus> = None;
    /// let mut default_appl_ver_id: Option<fields::DefaultApplVerId> = None;
    /// let mut text: Option<FixString> = None;
    /// let check_sum = deserializer.check_sum();
    /// ```
    fn gen_opt_variables(&self) -> TokenStream {
        let name = &self.name;
        let data_type = self.gen_type();
        // TODO: Consider filtering out special tags, they are handled manually in other places
        match SpecialTag::from_tag(self.number) {
            Some(SpecialTag::BeginString | SpecialTag::BodyLength | SpecialTag::MsgType) => {
                quote! {}
            }
            Some(SpecialTag::CheckSum) => quote! { let #name = deserializer.check_sum(); },
            // MsgSeqNum (34) uses the normal path
            Some(SpecialTag::MsgSeqNum) | None => {
                quote! { let mut #name: Option<#data_type> = None; }
            }
        }
    }

    /// Generate `fn deserialize()` main loop match arm body.
    ///
    /// ```rust,ignore
    /// while let Some(tag) = deserializer.deserialize_tag_num()? {
    ///     match tag {
    ///         <tag_number>u16 => {
    ///             #gen_deserialize_value()
    ///         }
    ///     }
    /// }
    /// ```
    // TODO: rename to gen_deserialize_match_arm_body?
    fn gen_deserialize_value(&self) -> TokenStream {
        let name = &self.name;
        let num = self.number;
        let deserialize = self.gen_deserialize();
        assert!(
            !matches!(
                self.data_type,
                DataType::BasicType(BasicTypeCodeGen(
                    BasicType::NumInGroup | BasicType::Data | BasicType::XmlData
                ))
            ),
            "Unexpected data type {:?} in {}/{}",
            self.data_type,
            self.name,
            self.number
        );
        match SpecialTag::from_tag(self.number) {
            // TODO:  fillter out special tags
            Some(
                SpecialTag::BeginString
                | SpecialTag::BodyLength
                | SpecialTag::CheckSum
                | SpecialTag::MsgType,
            ) => quote! {
                // this tags was processed separately
                return Err(deserializer.reject(Some(#num), SessionRejectReasonBase::TagAppearsMoreThanOnce));
            },
            Some(SpecialTag::MsgSeqNum) => quote! {
                if #name.is_some() {
                    return Err(deserializer.reject(Some(#num), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                }
                let msg_seq_num_value = #deserialize?;
                deserializer.set_seq_num(msg_seq_num_value);
                #name = Some(msg_seq_num_value);
            },
            None => quote! {
                if #name.is_some() {
                    return Err(deserializer.reject(Some(#num), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                }
                #name = Some(#deserialize?);
            },
        }
    }

    fn gen_deserialize_match_entries(&self) -> Vec<TokenStream> {
        let tag = self.number;
        let deserialize_value = self.gen_deserialize_value();
        vec![quote! {
            #tag => { #deserialize_value }
        }]
    }

    /// Generate code used to initialize structure.
    ///
    /// ```rust,ignore
    /// Logon {
    ///     encrypt_method: encrypt_method.ok_or_else(|| {
    ///         deserializer.reject(Some(98u16), SessionRejectReasonBase::RequiredTagMissing)
    ///     })?,
    ///     heart_bt_int: heart_bt_int.ok_or_else(|| {
    ///         deserializer.reject(Some(108u16), SessionRejectReasonBase::RequiredTagMissing)
    ///     })?,
    ///     raw_data,
    ///     reset_seq_num_flag,
    ///     ...
    /// }
    /// ```
    fn gen_deserialize_struct_entries(&self, required: bool) -> TokenStream {
        let name = &self.name;
        let num = self.number;
        let skips_required =
            SpecialTag::from_tag(self.number).is_some_and(|s| s.skips_required_check());
        if required && !skips_required {
            quote! {
                #name: #name.ok_or_else(|| deserializer.reject(Some(#num), SessionRejectReasonBase::RequiredTagMissing))?
            }
        } else {
            quote! {
                #name
            }
        }
    }
}

enum RawDataType {
    Data,
    XmlData,
}

impl RawDataType {
    fn try_from_basic_type(bt: BasicType) -> Option<RawDataType> {
        match bt {
            BasicType::Data => Some(RawDataType::Data),
            BasicType::XmlData => Some(RawDataType::XmlData),
            _ => None,
        }
    }

    fn rust_type(&self) -> TokenStream {
        match self {
            RawDataType::Data => BasicTypeCodeGen(BasicType::Data).rust_type(),
            RawDataType::XmlData => BasicTypeCodeGen(BasicType::XmlData).rust_type(),
        }
    }
}

struct RawData {
    length_name: Ident,
    length_number: u16,
    data_name: Ident,
    data_number: u16,
    raw_data_type: RawDataType,
}

impl RawData {
    fn new(length: Rc<dict::Field>, data: Rc<dict::Field>) -> RawData {
        let raw_data_type = RawDataType::try_from_basic_type(data.data_type()).unwrap();
        RawData {
            length_name: to_snake_case_ident(length.name()),
            length_number: length.number(),
            data_name: to_snake_case_ident(data.name()),
            data_number: data.number(),
            raw_data_type,
        }
    }

    fn tag_num(&self) -> u16 {
        self.length_number
    }

    /// Generate member definition for use in structs definitions.
    fn gen_definition(&self, required: bool) -> TokenStream {
        let data_name = &self.data_name;
        let raw_data_type = self.raw_data_type.rust_type();
        let data_doc_comment = format!("Tag {}.", self.data_number);
        if required {
            quote! {
                #[doc = #data_doc_comment]
                pub #data_name: #raw_data_type
            }
        } else {
            quote! {
                #[doc = #data_doc_comment]
                pub #data_name: Option<#raw_data_type>
            }
        }
    }

    fn gen_serialize(&self, required: bool) -> TokenStream {
        let len_num = Literal::byte_string(format!("{}=", self.length_number).as_bytes());
        let data_name = &self.data_name;
        let data_num = Literal::byte_string(format!("{}=", self.data_number).as_bytes());
        let serialize_value = match self.raw_data_type {
            RawDataType::Data => quote! { serializer.serialize_data },
            RawDataType::XmlData => quote! { serializer.serialize_xml },
        };
        if required {
            quote! {
                serializer.output_mut().extend_from_slice(#len_num);
                serializer.serialize_length(&(self.#data_name.len() as u16));
                serializer.output_mut().push(b'\x01');
                serializer.output_mut().extend_from_slice(#data_num);
                #serialize_value(&self.#data_name);
                serializer.output_mut().push(b'\x01');
            }
        } else {
            quote! {
                if let Some(#data_name) = &self.#data_name {
                    serializer.output_mut().extend_from_slice(#len_num);
                    serializer.serialize_length(&(#data_name.len() as u16));
                    serializer.output_mut().push(b'\x01');
                    serializer.output_mut().extend_from_slice(#data_num);
                    #serialize_value(#data_name);
                    serializer.output_mut().push(b'\x01');
                }
            }
        }
    }

    /// Generate mutable optional variables set to None for further
    /// processig in deserializer loop.
    ///
    /// Variables for special tags like 8, 9, 10 has already known values.
    fn gen_opt_variables(&self) -> TokenStream {
        let len_name = &self.length_name;
        let data_name = &self.data_name;
        let raw_data_type = self.raw_data_type.rust_type();

        quote! {
            let mut #len_name: Option<Length> = None;
            let mut #data_name: Option<#raw_data_type> = None;
        }
    }

    fn gen_deserialize_value(&self) -> TokenStream {
        let len_name = &self.length_name;
        let len_num = self.length_number;
        let data_name = &self.data_name;
        let data_num = self.data_number;
        let raw_data_deserialize = match self.raw_data_type {
            RawDataType::Data => quote! { deserializer.deserialize_data },
            RawDataType::XmlData => quote! { deserializer.deserialize_xml },
        };
        quote! {
            if #len_name.is_some() {
                return Err(deserializer.reject(Some(#len_num), SessionRejectReasonBase::TagAppearsMoreThanOnce));
            }
            let len = deserializer.deserialize_length()?;
            #len_name = Some(len);
            if deserializer.deserialize_tag_num()?.ok_or_else(|| {
                deserializer.reject(Some(#data_num), SessionRejectReasonBase::RequiredTagMissing)
            })? != #data_num
            {
                return Err(deserializer.reject(Some(#len_num), SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder));
            }
            // This should never happen, as error would be
            // returned in #name.is_some() case.
            if #data_name.is_some() {
                return Err(deserializer.reject(Some(#len_num), SessionRejectReasonBase::TagAppearsMoreThanOnce));
            }
            #data_name = Some(#raw_data_deserialize(len as usize)?);
        }
    }

    // TODO: no Vec needed
    fn gen_deserialize_match_entries(&self) -> Vec<TokenStream> {
        let dv = self.gen_deserialize_value();
        let len_num = self.length_number;
        let data_num = self.data_number;
        vec![
            quote! { #len_num => { #dv } },
            quote! {
                #data_num => {
                    return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder));
                }
            },
        ]
    }

    /// Generate code used to initialize structure.
    fn gen_deserialize_struct_entries(&self, required: bool) -> TokenStream {
        let data_name = &self.data_name;
        let data_num = self.data_number;
        if required {
            quote! {
                #data_name: #data_name.ok_or_else(|| deserializer.reject(Some(#data_num), SessionRejectReasonBase::RequiredTagMissing))?
            }
        } else {
            quote! {
                #data_name
            }
        }
    }
}

/// Compute expected tag numbers for group field ordering validation.
/// Returns tags of all fields within a group that have match arms in deserialize.
fn compute_expected_tags(members: &[dict::Member]) -> Vec<u16> {
    // TODO: revrite to not allocate
    members
        .iter()
        .flat_map(|m| match m.definition() {
            dict::MemberDefinition::Field(f) => vec![f.number()],
            dict::MemberDefinition::RawData { length, .. } => vec![length.number()],
            dict::MemberDefinition::Group(g) => vec![g.num_in_group().number()],
            dict::MemberDefinition::Component(_) => {
                panic!("Components should have been flattened before code generation")
            }
        })
        .collect()
}

struct Group {
    name: Ident,
    data_type: Ident,
    num_in_group_name: Ident,
    num_in_group_number: u16,
    expected_tags: Vec<u16>,
}

impl Group {
    pub fn new(group: &dict::Group) -> Group {
        Group {
            name: to_snake_case_ident(group.name()),
            data_type: to_upper_snake_case_ident(group.name()),
            num_in_group_name: to_snake_case_ident(group.num_in_group().name()),
            num_in_group_number: group.num_in_group().number(),
            expected_tags: compute_expected_tags(group.members()),
        }
    }

    fn tag_num(&self) -> u16 {
        self.num_in_group_number
    }

    /// Generate member definition for use in structs definitions.
    fn gen_definition(&self, required: bool) -> TokenStream {
        let name = &self.name;
        let data_type = &self.data_type;
        let doc_comment = format!("Tag {}.", self.num_in_group_number);
        if required {
            quote! {
                #[doc = #doc_comment]
                pub #name: Vec<#data_type>
            }
        } else {
            quote! {
                #[doc = #doc_comment]
                pub #name: Option<Vec<#data_type>>
            }
        }
    }

    fn gen_serialize(&self, required: bool) -> TokenStream {
        let group_name = &self.name;
        let num_in_group_tag =
            Literal::byte_string(format!("{}=", self.num_in_group_number).as_bytes());
        if required {
            quote! {
                serializer.output_mut().extend_from_slice(#num_in_group_tag);
                // TODO: possible overflow (impossible in practice)
                serializer.serialize_num_in_group(&(self.#group_name.len() as NumInGroup));
                serializer.output_mut().push(b'\x01');
                for entry in &self.#group_name {
                    entry.serialize(serializer);
                }
            }
        } else {
            quote! {
                if let Some(#group_name) = &self.#group_name {
                    serializer.output_mut().extend_from_slice(#num_in_group_tag);
                    serializer.serialize_num_in_group(&(#group_name.len() as NumInGroup));
                    serializer.output_mut().push(b'\x01');
                    for entry in #group_name {
                        entry.serialize(serializer);
                    }
                }
            }
        }
    }

    /// Generate mutable optional variables set to None for further
    /// processig in deserializer loop.
    ///
    /// Variables for special tags like 8, 9, 10 has already known values.
    fn gen_opt_variables(&self) -> TokenStream {
        let num_in_group_name = &self.num_in_group_name;
        let name = &self.name;
        let data_type = &self.data_type;

        quote! {
            let mut #num_in_group_name: Option<NumInGroup> = None;
            let mut #name: Option<Vec<#data_type>> = None;
        }
    }

    fn gen_deserialize_value(&self) -> TokenStream {
        let num_in_gropup_name = &self.num_in_group_name;
        let num_in_group_number = &self.num_in_group_number;
        let name = &self.name;
        let data_type = &self.data_type;
        let expected_tags = &self.expected_tags;
        let group_name_local = Ident::new(&format!("{}_local", name), Span::call_site());
        let deserialize = quote! { #data_type::deserialize(deserializer, num_in_group_tag, expected_tags, last_run) };
        quote! {
            if #num_in_gropup_name.is_some() {
                return Err(deserializer.reject(Some(#num_in_group_number), SessionRejectReasonBase::TagAppearsMoreThanOnce));
            }
            let len = deserializer.deserialize_num_in_group()?;
            #num_in_gropup_name = Some(len);
            if #name.is_some() {
                return Err(deserializer.reject(Some(#num_in_group_number), SessionRejectReasonBase::TagAppearsMoreThanOnce));
            }
            let num_in_group_tag = #num_in_group_number;
            let expected_tags = &[#(#expected_tags),*];
            let mut #group_name_local = Vec::with_capacity(len as usize);
            let last_run = false;
            for _ in 0..len - 1 {
                #group_name_local.push(#deserialize?);
            }
            let last_run = true;
            #group_name_local.push(#deserialize?);
            #name = Some(#group_name_local);
        }
    }

    fn gen_deserialize_match_entries(&self) -> Vec<TokenStream> {
        let deserialize_value = self.gen_deserialize_value();
        let tag = self.num_in_group_number;
        vec![quote! { #tag => {#deserialize_value }}]
    }

    /// Generate code used to initialize structure.
    fn gen_deserialize_struct_entries(&self, required: bool) -> TokenStream {
        let name = &self.name;
        let num = &self.num_in_group_number;
        if required {
            quote! {
                #name: #name.ok_or_else(|| deserializer.reject(Some(#num), SessionRejectReasonBase::RequiredTagMissing))?
            }
        } else {
            quote! {
                #name
            }
        }
    }
}

enum MemberDefinition {
    Field(Field),
    RawData(RawData),
    Group(Group),
}

impl MemberDefinition {
    fn new(member_def: &dict::MemberDefinition) -> MemberDefinition {
        match member_def {
            dict::MemberDefinition::Field(field) => MemberDefinition::Field(Field::new(field)),
            dict::MemberDefinition::RawData { length, data } => {
                MemberDefinition::RawData(RawData::new(length.clone(), data.clone()))
            }
            dict::MemberDefinition::Component(_component) => {
                unreachable!("dictinary should be flatenned")
            }
            dict::MemberDefinition::Group(group) => MemberDefinition::Group(Group::new(group)),
        }
    }

    fn tag_num(&self) -> u16 {
        match self {
            MemberDefinition::Field(field) => field.number,
            MemberDefinition::RawData(raw_data) => raw_data.tag_num(),
            MemberDefinition::Group(group) => group.tag_num(),
        }
    }

    /// Generate member definition for use in structs definitions.
    fn gen_definition(&self, required: bool) -> TokenStream {
        match self {
            MemberDefinition::Field(field) => field.gen_definition(required),
            MemberDefinition::RawData(raw_data) => raw_data.gen_definition(required),
            MemberDefinition::Group(group) => group.gen_definition(required),
        }
    }

    fn gen_serialize(&self, required: bool) -> TokenStream {
        match self {
            MemberDefinition::Field(field) => field.gen_serialize(required),
            MemberDefinition::RawData(raw_data) => raw_data.gen_serialize(required),
            MemberDefinition::Group(group) => group.gen_serialize(required),
        }
    }

    /// Generate mutable optional variables set to None for further
    /// processig in deserializer loop.
    ///
    /// Variables for special tags like 8, 9, 10 has already known values.
    fn gen_opt_variables(&self) -> TokenStream {
        match self {
            MemberDefinition::Field(field) => field.gen_opt_variables(),
            MemberDefinition::RawData(raw_data) => raw_data.gen_opt_variables(),
            MemberDefinition::Group(group) => group.gen_opt_variables(),
        }
    }

    fn gen_deserialize_value(&self) -> TokenStream {
        match self {
            MemberDefinition::Field(field) => field.gen_deserialize_value(),
            MemberDefinition::RawData(raw_data) => raw_data.gen_deserialize_value(),
            MemberDefinition::Group(group) => group.gen_deserialize_value(),
        }
    }

    fn gen_deserialize_match_entries(&self) -> Vec<TokenStream> {
        match self {
            MemberDefinition::Field(field) => field.gen_deserialize_match_entries(),
            MemberDefinition::RawData(raw_data) => raw_data.gen_deserialize_match_entries(),
            MemberDefinition::Group(group) => group.gen_deserialize_match_entries(),
        }
    }

    /// Generate code used to initialize structure.
    fn gen_deserialize_struct_entries(&self, required: bool) -> TokenStream {
        match self {
            MemberDefinition::Field(field) => field.gen_deserialize_struct_entries(required),
            MemberDefinition::RawData(raw_data) => {
                raw_data.gen_deserialize_struct_entries(required)
            }
            MemberDefinition::Group(group) => group.gen_deserialize_struct_entries(required),
        }
    }
}

pub struct Member {
    required: bool,
    definition: Rc<MemberDefinition>,
}

impl Member {
    pub fn new(member: &dict::Member) -> Member {
        Member {
            required: member.required(),
            definition: Rc::new(MemberDefinition::new(member.definition())),
        }
    }

    pub fn tag_num(&self) -> u16 {
        self.definition.tag_num()
    }

    /// Check if this member is a field with the specified underlying basic type.
    /// Works for both plain fields and enum fields (checks the enum's backing type).
    /// Returns false for groups or raw data.
    pub fn has_basic_type(&self, expected: BasicType) -> bool {
        match &*self.definition {
            MemberDefinition::Field(field) => match &field.data_type {
                DataType::BasicType(BasicTypeCodeGen(bt)) => *bt == expected,
                DataType::EnumerableType(_, et) => et.to_basic_type() == expected,
            },
            _ => false,
        }
    }

    /// Generate member definition for use in structs definitions.
    pub fn gen_definition(&self) -> TokenStream {
        self.definition.gen_definition(self.required)
    }

    pub fn gen_serialize(&self) -> TokenStream {
        self.definition.gen_serialize(self.required)
    }

    /// Generate mutable optional variables set to None for further
    /// processig in deserializer loop.
    ///
    /// Variables for special tags like 8, 9, 10 has already known values.
    pub fn gen_opt_variables(&self) -> TokenStream {
        self.definition.gen_opt_variables()
    }

    pub fn gen_deserialize_value(&self) -> TokenStream {
        self.definition.gen_deserialize_value()
    }

    pub fn gen_deserialize_match_entries(&self) -> Vec<TokenStream> {
        self.definition.gen_deserialize_match_entries()
    }

    /// Generate code used to initialize structure.
    pub fn gen_deserialize_struct_entries(&self) -> TokenStream {
        self.definition
            .gen_deserialize_struct_entries(self.required)
    }
}
