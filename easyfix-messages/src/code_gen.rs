use std::collections::{BTreeSet, HashMap};

use easyfix_core::basic_types::FixString;
use easyfix_dictionary::{self as dict, Dictionary, Version};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

mod admin;
mod enumeration;
mod group;
mod header;
mod ident;
mod member;
mod message;
mod message_enum;
mod trailer;

use self::{
    enumeration::EnumCodeGen,
    group::GroupCodeGen,
    header::Header,
    ident::ToIdent,
    member::{EnumerableType, Member},
    message::MessageCodeGen,
    trailer::Trailer,
};

/// Emit `#[doc = ...]` attributes holding the dictionary documentation
/// text, one attribute per line; empty when there is no documentation.
fn doc_text_attrs(doc: Option<&str>) -> TokenStream {
    match doc {
        Some(text) => {
            let lines = text.lines();
            quote! { #(#[doc = #lines])* }
        }
        None => quote! {},
    }
}

/// Emit `#[doc = ...]` attributes for an item: the dictionary documentation
/// text (if any) first - it becomes the rustdoc summary - then a blank
/// separator line, then the technical line (e.g. "Tag 11.").
fn doc_attrs(doc: Option<&str>, technical: &str) -> TokenStream {
    let text = doc_text_attrs(doc);
    let separator = if doc.is_some() {
        quote! { #[doc = ""] }
    } else {
        quote! {}
    };
    quote! {
        #text
        #separator
        #[doc = #technical]
    }
}

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
        let mut groups_map: HashMap<FixString, GroupCodeGen> = HashMap::new();
        for group in dictionary.groups() {
            groups_map
                .entry(group.name().to_owned())
                .or_insert_with(|| {
                    GroupCodeGen::new(
                        group.name(),
                        group.num_in_group().number(),
                        convert_members(group.members()),
                        group.doc(),
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
                            group.doc(),
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
                msg.msg_type(),
                members_descs,
                msg.msg_cat(),
                msg.doc(),
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
            // ApplVerID(1128) / DefaultApplVerID(1137) use the spec-owned
            // `easyfix_core::basic_types::ApplVerId` - no enum is generated
            // for them. The dictionary must not customize their codeset.
            if matches!(field.number(), 1128 | 1137) {
                let values: Vec<&str> = field
                    .variants()
                    .iter()
                    .map(|v| v.value().as_utf8())
                    .collect();
                validate_appl_ver_id_codeset(field.name().as_utf8(), field.number(), &values);
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
                    field.doc(),
                ));
            }
        }
        enums.sort_by_key(|e| e.tag());

        let mut fields: Vec<&dict::Field> = all_fields_map.into_values().collect();
        fields.sort_by_key(|f| f.number());
        let (fields_names, fields_numbers) = fields
            .iter()
            .map(|f| (f.name().to_pascal_ident(), f.number()))
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

        // `Version::FIX44` / `Version::FIXT11` / `Version::FIX50SP2` — the
        // identifier is the canonical BeginString with dots stripped.
        let version_const: String = self
            .version
            .begin_str()
            .as_utf8()
            .chars()
            .filter(|c| *c != '.')
            .collect();
        let version_ident = Ident::new(&version_const, Span::call_site());
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
            use std::{borrow::Cow, fmt};

            pub use easyfix_core::message::MsgCat;
            // Spec-owned codeset type used directly as the field type for
            // tags 1128/1137; re-exported so dictionary consumers can name
            // it from the generated crate.
            pub use easyfix_core::basic_types::ApplVerId;
            #[allow(unused_imports)]
            use easyfix_core::{
                base_messages::{
                    AdminBase, EncryptMethodBase, HeaderBase, HeartbeatBase, LogonBase, LogoutBase, RejectBase,
                    ResendRequestBase, SequenceResetBase, SessionRejectReasonBase, TestRequestBase,
                },
                basic_types::{
                    Amt, Boolean, Char, Country, Currency, Data, DayOfMonth, Decimal, Exchange, FixStr,
                    FixString, Float, Int, Language, Length, LocalMktDate, LocalMktTime, MonthYear,
                    MsgTypeField, MsgTypeValue, MultipleCharValue, MultipleStringValue, NumInGroup, Percentage,
                    Price, PriceOffset, Qty, SeqNum, SessionRejectReasonField, SessionRejectReasonValue,
                    SessionStatusField, SessionStatusValue, TagNum, Tenor, TenorUnit, TimePrecision,
                    ToFixString, TzTimeOnly, TzTimestamp, UtcDateOnly, UtcTimeOnly, UtcTimestamp, XmlData,
                },
                deserializer::{DeserializeError, Deserializer, GarbledReason, LogoutReason, RawMessage},
                message::{HeaderAccess, SessionMessage},
                serializer::{SerializeError, Serializer},
                version::Version,
            };

            pub const VERSION: Version = Version::#version_ident;

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

/// The 11 legal ApplVerIDCodeSet wire values (FIX Session Layer §11.2).
const APPL_VER_ID_CODESET: [&str; 11] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];

/// Hard generation-time check for tags 1128/1137: the ApplVerIDCodeSet is
/// closed by the standard, and the generated code represents these fields
/// with `easyfix_core::basic_types::ApplVerId`, which only speaks the spec
/// values. A dictionary that trims or extends the codeset would silently
/// change accept/reject behavior, so it is rejected loudly instead. An
/// empty `<value>` list is fine - the field type is forced by tag anyway.
fn validate_appl_ver_id_codeset(name: &str, tag: u16, values: &[&str]) {
    if values.is_empty() {
        return;
    }
    let declared: BTreeSet<&str> = values.iter().copied().collect();
    let expected: BTreeSet<&str> = APPL_VER_ID_CODESET.into_iter().collect();
    if declared != expected {
        let missing: Vec<&&str> = expected.difference(&declared).collect();
        let extra: Vec<&&str> = declared.difference(&expected).collect();
        panic!(
            "field {name}({tag}) customizes the ApplVerIDCodeSet \
             (missing: {missing:?}, extra: {extra:?}); the codeset is closed \
             by the FIX standard - custom application versions belong in \
             CstmApplVerID(1129) / DefaultCstmApplVerID(1408)"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use easyfix_dictionary::fix_str;

    use super::*;

    const DOC_DICT: &str = r#"
<?xml version='1.0' encoding='UTF-8'?>
<fix type='FIX' major='4' minor='4' servicepack='0'>
  <header>
    <field name='BeginString' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message msgcat='app' msgtype='D' name='NewOrderSingle' doc='Submits a new order.'>
      <field name='ClOrdID' required='Y'/>
      <field name='Side' required='Y'/>
      <group name='NoAllocs' required='N' doc='Allocations of the order.'>
        <field name='AllocAccount' required='N'/>
      </group>
    </message>
    <message msgcat='app' msgtype='G' name='OrderCancelReplaceRequest'>
      <field name='OrigClOrdID' required='Y'/>
    </message>
  </messages>
  <components/>
  <fields>
    <field name='BeginString' number='8' type='STRING'/>
    <field name='CheckSum' number='10' type='STRING'/>
    <field name='ClOrdID' number='11' type='STRING' doc='Unique order id.'/>
    <field name='OrigClOrdID' number='41' type='STRING'/>
    <field name='Side' number='54' type='CHAR' doc='Side of order.'>
      <value enum='1' description='BUY' doc='Buy order.'/>
      <value enum='2' description='SELL'/>
    </field>
    <field name='NoAllocs' number='78' type='NUMINGROUP'/>
    <field name='AllocAccount' number='79' type='STRING'/>
  </fields>
</fix>
"#;

    fn doc_dictionary(file_name: &str) -> Dictionary {
        let path =
            env::temp_dir().join(format!("easyfix_{}_{}.xml", file_name, std::process::id()));
        fs::write(&path, DOC_DICT).unwrap();
        let dictionary = Dictionary::new(path.to_str().unwrap())
            .unwrap()
            .flatten()
            .unwrap();
        fs::remove_file(&path).unwrap();
        dictionary
    }

    #[test]
    fn message_doc_comments_emitted() {
        let dictionary = doc_dictionary("message_doc");
        let msg = dictionary
            .message_by_name(fix_str!("NewOrderSingle"))
            .unwrap();
        let code = MessageCodeGen::new(
            msg.name(),
            msg.msg_type(),
            convert_members(msg.members()),
            msg.msg_cat(),
            msg.doc(),
        )
        .generate(false, false)
        .to_string();

        // Message doc on the struct, followed by the technical MsgType line
        assert!(code.contains("Submits a new order."));
        assert!(code.contains(r#"MsgType \"D\"."#));
        // Field doc on the struct member, followed by the technical tag line
        assert!(code.contains("Unique order id."));
        assert!(code.contains("Tag 11."));
        // Group member slot picks up the group doc
        assert!(code.contains("Allocations of the order."));
    }

    #[test]
    fn message_without_doc_has_no_doc_separator() {
        let dictionary = doc_dictionary("message_no_doc");
        let msg = dictionary
            .message_by_name(fix_str!("OrderCancelReplaceRequest"))
            .unwrap();
        let code = MessageCodeGen::new(
            msg.name(),
            msg.msg_type(),
            convert_members(msg.members()),
            msg.msg_cat(),
            msg.doc(),
        )
        .generate(false, false)
        .to_string();

        // Without doc attributes the output stays exactly as before:
        // no empty separator doc lines anywhere
        assert!(!code.contains(r#"doc = """#));
        assert!(code.contains(r#"MsgType \"G\"."#));
    }

    #[test]
    fn group_doc_comments_emitted() {
        let dictionary = doc_dictionary("group_doc");
        let group = dictionary.group(fix_str!("Allocs")).unwrap();
        let code = GroupCodeGen::new(
            group.name(),
            group.num_in_group().number(),
            convert_members(group.members()),
            group.doc(),
        )
        .generate(false, false)
        .to_string();

        assert!(code.contains("Allocations of the order."));
        assert!(code.contains("NumInGroup tag 78."));
    }

    #[test]
    fn enum_doc_comments_emitted() {
        let dictionary = doc_dictionary("enum_doc");
        let side = dictionary.field_by_id(54).unwrap();
        let code = EnumCodeGen::new(
            side.name(),
            side.number(),
            EnumerableType::try_from_basic_type(side.data_type()).unwrap(),
            side.variants().to_vec(),
            side.doc(),
        )
        .generate(false, false)
        .to_string();

        // Field doc on the enum type
        assert!(code.contains("Side of order."));
        // Variant doc, followed by the technical value line
        assert!(code.contains("Buy order."));
        assert!(code.contains(r#"Value \"1\""#));
    }

    #[test]
    fn full_appl_ver_id_codeset_accepted() {
        validate_appl_ver_id_codeset(
            "DefaultApplVerID",
            1137,
            &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"],
        );
    }

    #[test]
    fn empty_appl_ver_id_value_list_accepted() {
        validate_appl_ver_id_codeset("DefaultApplVerID", 1137, &[]);
    }

    #[test]
    #[should_panic(expected = "DefaultApplVerID(1137) customizes the ApplVerIDCodeSet")]
    fn trimmed_appl_ver_id_codeset_rejected() {
        validate_appl_ver_id_codeset("DefaultApplVerID", 1137, &["9"]);
    }

    #[test]
    #[should_panic(expected = "DefaultApplVerID(1137) customizes the ApplVerIDCodeSet")]
    fn extended_appl_ver_id_codeset_rejected() {
        validate_appl_ver_id_codeset(
            "DefaultApplVerID",
            1137,
            &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11"],
        );
    }
}
