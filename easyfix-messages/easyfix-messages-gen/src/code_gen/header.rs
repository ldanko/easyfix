use easyfix_dictionary::{BasicType, Version};
use proc_macro2::TokenStream;
use quote::quote;

use super::member::Member;

/// Header generator
pub struct Header {
    members: Vec<Member>,
}

impl Header {
    pub fn new(members: Vec<Member>) -> Header {
        Header { members }
    }

    pub fn generate(&self, version: Version) -> TokenStream {
        // Tag 35 (MsgType) is not stored in Header — it's derived from the body.
        let members_definitions = self
            .members
            .iter()
            .filter(|m| m.tag_num() != 35)
            .map(|member| member.gen_definition());
        // Tags 8 (BeginString), 9 (BodyLength), and 35 (MsgType) are serialized
        // by Message::serialize() — not by Header::serialize().
        let serialize = self
            .members
            .iter()
            .filter(|m| !matches!(m.tag_num(), 8 | 9 | 35))
            .map(|member| member.gen_serialize());
        let deserialize = self.generate_deserialize();
        let header_base_conversions = self.generate_header_base_conversions(version);
        let header_self_access_impl = self.generate_header_access_for_header_impl(version);
        // TODO: move this to Message section
        let header_access_impl = self.generate_header_access_impl(version);

        quote! {
            #[derive(Clone, Debug, Default)]
            #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
            #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
            pub struct Header {
                #(#members_definitions,)*
            }

            impl Header{
                pub(crate) fn serialize(&self, serializer: &mut Serializer) {
                    #(#serialize;)*
                }

                #deserialize
            }

            #header_base_conversions

            #header_self_access_impl

            #header_access_impl
        }
    }

    fn generate_deserialize(&self) -> TokenStream {
        let variables_definitions = self.members.iter().map(|member| member.gen_opt_variables());
        let de_match_entries = self
            .members
            .iter()
            .flat_map(|member| member.gen_deserialize_match_entries());
        // Tag 35 (MsgType) is not stored in Header — skip it in struct initialization.
        let de_header_entries = self
            .members
            .iter()
            .filter(|m| m.tag_num() != 35)
            .map(|member| member.gen_deserialize_struct_entries());
        quote! {
            fn deserialize(
                deserializer: &mut Deserializer,
                begin_string: FixString,
                body_length: Length,
            ) -> Result<Header, DeserializeError> {
                #(#variables_definitions)*

                while let Some(tag) = deserializer.deserialize_tag_num()? {
                    match tag {
                        #(#de_match_entries,)*
                        tag => {
                            if FieldTag::from_tag_num(tag).is_some() {
                                deserializer.put_tag(tag);
                                break;
                            } else {
                                return Err(deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag));
                            }
                        },
                    }
                }

                Ok(Header {
                    #(#de_header_entries,)*
                })
            }
        }
    }

    /// Generate `From` conversions between `HeaderBase` (easyfix-core) and the
    /// generated `Header` struct.
    ///
    /// - `From<&'a Header> for HeaderBase<'a>` — incoming, zero-copy via `Cow::Borrowed`
    /// - `From<HeaderBase<'_>> for Header` — outgoing, consumes `Cow`, defaults remaining fields
    fn generate_header_base_conversions(&self, version: Version) -> TokenStream {
        let members_by_tag: std::collections::HashMap<u16, &Member> =
            self.members.iter().map(|m| (m.tag_num(), m)).collect();

        // Validate always-present HeaderBase fields.
        let expected: &[(u16, &str, BasicType)] = &[
            (8, "BeginString", BasicType::String),
            (49, "SenderCompID", BasicType::String),
            (56, "TargetCompID", BasicType::String),
            (34, "MsgSeqNum", BasicType::SeqNum),
            (52, "SendingTime", BasicType::UtcTimestamp),
            (43, "PossDupFlag", BasicType::Boolean),
        ];

        for &(tag, name, expected_bt) in expected {
            let member = members_by_tag.get(&tag).unwrap_or_else(|| {
                panic!("Header is missing tag {tag} ({name}) needed for HeaderBase conversion")
            });
            assert!(
                member.has_basic_type(expected_bt),
                "Header tag {tag} ({name}) has unexpected type, expected {expected_bt:?}",
            );
        }

        // Version-conditional: OrigSendingTime (tag 122, FIX 4.0+)
        let has_orig_sending_time = members_by_tag.contains_key(&122);
        if has_orig_sending_time {
            let member = members_by_tag.get(&122).unwrap();
            assert!(
                member.has_basic_type(BasicType::UtcTimestamp),
                "Header tag 122 (OrigSendingTime) has unexpected type, expected UtcTimestamp",
            );
        }
        if has_orig_sending_time && version < Version::FIX40 {
            panic!("OrigSendingTime (tag 122) is not valid before FIX 4.0");
        }

        // Version-conditional: ApplVerID (tag 1128, FIXT 1.1)
        let has_appl_ver_id = members_by_tag.contains_key(&1128);
        if has_appl_ver_id {
            let member = members_by_tag.get(&1128).unwrap();
            assert!(
                member.has_basic_type(BasicType::String),
                "Header tag 1128 (ApplVerID) has unexpected type, expected String",
            );
        }
        if version == Version::FIXT11 {
            assert!(
                has_appl_ver_id,
                "FIXT 1.1 Header must have ApplVerID (tag 1128)"
            );
        }

        // --- Incoming: From<&'a Header> for HeaderBase<'a> ---
        // ApplVerId is an enum backed by String — uses as_fix_str()
        // for zero-copy borrowing. Other String fields borrow via Deref.

        let incoming_orig_sending_time = if has_orig_sending_time {
            quote! { header.orig_sending_time }
        } else {
            quote! { None }
        };

        let incoming_appl_ver_id = if has_appl_ver_id {
            quote! { header.appl_ver_id.as_ref().map(|v| Cow::Borrowed(v.as_fix_str())) }
        } else {
            quote! { None }
        };

        // --- Outgoing: From<HeaderBase<'_>> for Header ---
        // ApplVerId enum is built from FixStr via from_fix_str().

        let outgoing_orig_sending_time = if has_orig_sending_time {
            quote! { orig_sending_time: base.orig_sending_time, }
        } else {
            quote! {}
        };

        let outgoing_appl_ver_id = if has_appl_ver_id {
            quote! {
                appl_ver_id: base.appl_ver_id.map(|v| {
                    fields::ApplVerId::from_fix_str(&v)
                        .expect("HeaderBase appl_ver_id must be a valid ApplVerId")
                }),
            }
        } else {
            quote! {}
        };

        quote! {
            impl<'a> From<&'a Header> for HeaderBase<'a> {
                fn from(header: &'a Header) -> Self {
                    HeaderBase {
                        begin_string: Cow::Borrowed(&header.begin_string),
                        sender_comp_id: Cow::Borrowed(&header.sender_comp_id),
                        target_comp_id: Cow::Borrowed(&header.target_comp_id),
                        msg_seq_num: header.msg_seq_num,
                        sending_time: header.sending_time,
                        poss_dup_flag: header.poss_dup_flag,
                        orig_sending_time: #incoming_orig_sending_time,
                        appl_ver_id: #incoming_appl_ver_id,
                    }
                }
            }

            impl From<HeaderBase<'_>> for Header {
                fn from(base: HeaderBase<'_>) -> Header {
                    Header {
                        begin_string: base.begin_string.into_owned(),
                        sender_comp_id: base.sender_comp_id.into_owned(),
                        target_comp_id: base.target_comp_id.into_owned(),
                        msg_seq_num: base.msg_seq_num,
                        sending_time: base.sending_time,
                        poss_dup_flag: base.poss_dup_flag,
                        #outgoing_orig_sending_time
                        #outgoing_appl_ver_id
                        ..Default::default()
                    }
                }
            }
        }
    }

    /// Generate `impl HeaderAccess for Header`.
    ///
    /// Same as the Message impl but delegates to `self.*` instead of
    /// `self.header.*`, allowing users to work with a borrowed `&Header`
    /// directly (e.g. when the message body is mutably borrowed).
    fn generate_header_access_for_header_impl(&self, _version: Version) -> TokenStream {
        let members_by_tag: std::collections::HashMap<u16, &Member> =
            self.members.iter().map(|m| (m.tag_num(), m)).collect();

        let has_orig_sending_time = members_by_tag.contains_key(&122);
        let has_appl_ver_id = members_by_tag.contains_key(&1128);

        // --- Getters ---

        let get_orig_sending_time = if has_orig_sending_time {
            quote! { self.orig_sending_time }
        } else {
            quote! { None }
        };

        let get_appl_ver_id = if has_appl_ver_id {
            quote! { self.appl_ver_id.as_ref().map(|v| v.as_fix_str()) }
        } else {
            quote! { None }
        };

        // --- Setters ---

        let set_orig_sending_time = if has_orig_sending_time {
            quote! { self.orig_sending_time = value; }
        } else {
            quote! { let _ = value; }
        };

        let set_appl_ver_id = if has_appl_ver_id {
            quote! {
                self.appl_ver_id = value.map(|v| {
                    fields::ApplVerId::from_fix_str(&v)
                        .expect("HeaderAccess::set_appl_ver_id: invalid ApplVerId value")
                });
            }
        } else {
            quote! { let _ = value; }
        };

        quote! {
            impl HeaderAccess for Header {
                fn begin_string(&self) -> &FixStr {
                    &self.begin_string
                }

                fn sender_comp_id(&self) -> &FixStr {
                    &self.sender_comp_id
                }

                fn target_comp_id(&self) -> &FixStr {
                    &self.target_comp_id
                }

                fn msg_seq_num(&self) -> SeqNum {
                    self.msg_seq_num
                }

                fn sending_time(&self) -> UtcTimestamp {
                    self.sending_time
                }

                fn poss_dup_flag(&self) -> Option<Boolean> {
                    self.poss_dup_flag
                }

                fn orig_sending_time(&self) -> Option<UtcTimestamp> {
                    #get_orig_sending_time
                }

                fn appl_ver_id(&self) -> Option<&FixStr> {
                    #get_appl_ver_id
                }

                fn set_begin_string(&mut self, value: FixString) {
                    self.begin_string = value;
                }

                fn set_sender_comp_id(&mut self, value: FixString) {
                    self.sender_comp_id = value;
                }

                fn set_target_comp_id(&mut self, value: FixString) {
                    self.target_comp_id = value;
                }

                fn set_msg_seq_num(&mut self, value: SeqNum) {
                    self.msg_seq_num = value;
                }

                fn set_sending_time(&mut self, value: UtcTimestamp) {
                    self.sending_time = value;
                }

                fn set_poss_dup_flag(&mut self, value: Option<Boolean>) {
                    self.poss_dup_flag = value;
                }

                fn set_orig_sending_time(&mut self, value: Option<UtcTimestamp>) {
                    #set_orig_sending_time
                }

                fn set_appl_ver_id(&mut self, value: Option<FixString>) {
                    #set_appl_ver_id
                }
            }
        }
    }

    /// Generate `impl HeaderAccess for Message`.
    ///
    /// Getters delegate to `self.header.*` fields. Setters assign to `self.header.*`
    /// fields. Enum-backed fields (ApplVerID) use `as_fix_str()` / `from_fix_str()`.
    /// Version-conditional fields (OrigSendingTime, ApplVerID) return `None` / no-op
    /// when absent from the generated Header.
    pub fn generate_header_access_impl(&self, _version: Version) -> TokenStream {
        let members_by_tag: std::collections::HashMap<u16, &Member> =
            self.members.iter().map(|m| (m.tag_num(), m)).collect();

        let has_orig_sending_time = members_by_tag.contains_key(&122);
        let has_appl_ver_id = members_by_tag.contains_key(&1128);

        // --- Getters ---

        let get_orig_sending_time = if has_orig_sending_time {
            quote! { self.header.orig_sending_time }
        } else {
            quote! { None }
        };

        let get_appl_ver_id = if has_appl_ver_id {
            quote! { self.header.appl_ver_id.as_ref().map(|v| v.as_fix_str()) }
        } else {
            quote! { None }
        };

        // --- Setters ---

        let set_orig_sending_time = if has_orig_sending_time {
            quote! { self.header.orig_sending_time = value; }
        } else {
            quote! { let _ = value; }
        };

        let set_appl_ver_id = if has_appl_ver_id {
            quote! {
                self.header.appl_ver_id = value.map(|v| {
                    fields::ApplVerId::from_fix_str(&v)
                        .expect("HeaderAccess::set_appl_ver_id: invalid ApplVerId value")
                });
            }
        } else {
            quote! { let _ = value; }
        };

        quote! {
            impl HeaderAccess for Message {
                fn begin_string(&self) -> &FixStr {
                    &self.header.begin_string
                }

                fn sender_comp_id(&self) -> &FixStr {
                    &self.header.sender_comp_id
                }

                fn target_comp_id(&self) -> &FixStr {
                    &self.header.target_comp_id
                }

                fn msg_seq_num(&self) -> SeqNum {
                    self.header.msg_seq_num
                }

                fn sending_time(&self) -> UtcTimestamp {
                    self.header.sending_time
                }

                fn poss_dup_flag(&self) -> Option<Boolean> {
                    self.header.poss_dup_flag
                }

                fn orig_sending_time(&self) -> Option<UtcTimestamp> {
                    #get_orig_sending_time
                }

                fn appl_ver_id(&self) -> Option<&FixStr> {
                    #get_appl_ver_id
                }

                fn set_begin_string(&mut self, value: FixString) {
                    self.header.begin_string = value;
                }

                fn set_sender_comp_id(&mut self, value: FixString) {
                    self.header.sender_comp_id = value;
                }

                fn set_target_comp_id(&mut self, value: FixString) {
                    self.header.target_comp_id = value;
                }

                fn set_msg_seq_num(&mut self, value: SeqNum) {
                    self.header.msg_seq_num = value;
                }

                fn set_sending_time(&mut self, value: UtcTimestamp) {
                    self.header.sending_time = value;
                }

                fn set_poss_dup_flag(&mut self, value: Option<Boolean>) {
                    self.header.poss_dup_flag = value;
                }

                fn set_orig_sending_time(&mut self, value: Option<UtcTimestamp>) {
                    #set_orig_sending_time
                }

                fn set_appl_ver_id(&mut self, value: Option<FixString>) {
                    #set_appl_ver_id
                }
            }
        }
    }
}
