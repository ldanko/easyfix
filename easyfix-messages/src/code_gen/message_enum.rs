use proc_macro2::{Ident, Literal, TokenStream};
use quote::quote;

use super::{admin, serde_derives};

/// Generate the `FieldTag` enum and its impls.
pub fn generate_field_tag(
    fields_names: &[Ident],
    fields_numbers: &[u16],
    serde_serialize: bool,
    serde_deserialize: bool,
) -> TokenStream {
    let fields_names_as_bytes = fields_names
        .iter()
        .map(|f| Literal::byte_string(f.to_string().as_bytes()));
    let fields_numbers_literals = fields_numbers.iter().copied().map(Literal::u16_suffixed);

    let serde_derives = serde_derives(serde_serialize, serde_deserialize);

    quote! {
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #serde_derives
        #[repr(u16)]
        pub enum FieldTag {
            #(#fields_names = #fields_numbers,)*
        }

        impl fmt::Display for FieldTag {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_fix_str().as_utf8())
            }
        }

        #[allow(dead_code)]
        impl FieldTag {
            pub const fn from_tag_num(tag_num: TagNum) -> Option<FieldTag> {
                match tag_num {
                    #(#fields_numbers_literals => Some(FieldTag::#fields_names),)*
                    _ => None,
                }
            }

            pub const fn as_bytes(&self) -> &'static [u8] {
                match self {
                    #(FieldTag::#fields_names => #fields_names_as_bytes,)*
                }
            }

            pub const fn as_fix_str(&self) -> &'static FixStr {
                unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
            }
        }

        impl ToFixString for FieldTag {
            fn to_fix_string(&self) -> FixString {
                self.as_fix_str().to_owned()
            }
        }
    }
}

/// Generate the `Body` enum with serialize/deserialize dispatch,
/// From impls, msg_type(), and msg_cat().
pub fn generate_message_enum(
    names: &[&Ident],
    serde_serialize: bool,
    serde_deserialize: bool,
) -> TokenStream {
    let admin_base_dispatch = admin::generate_admin_base_dispatch();
    let names_str = names.iter().map(|name| Literal::string(&name.to_string()));

    let impl_from_msg = names.iter().map(|name| {
        quote! {
            impl From<#name> for Body {
                fn from(msg: #name) -> Body {
                    Body::#name(msg)
                }
            }
        }
    });

    let serde_derives = serde_derives(serde_serialize, serde_deserialize);

    quote! {
        #[allow(dead_code)]
        #[derive(Clone, Debug)]
        #serde_derives
        #[allow(clippy::large_enum_variant)]
        pub enum Body {
            #(#names(#names),)*
        }

        #[allow(dead_code)]
        impl Body {
            fn serialize(&self, serializer: &mut Serializer) -> Result<(), SerializeError> {
                match self {
                    #(Body::#names(msg) => msg.serialize(serializer),)*
                }
            }

            fn deserialize(
                deserializer: &mut Deserializer,
                msg_type: MsgType
            ) -> Result<Box<Body>, DeserializeError> {
                match msg_type {
                    #(
                        MsgType::#names => Ok(#names::deserialize(deserializer)?),
                    )*
                }
            }

            pub const fn msg_type(&self) -> MsgType {
                match self {
                    #(Body::#names(_) => MsgType::#names,)*
                }
            }

            pub const fn msg_cat(&self) -> MsgCat {
                match self {
                    #(Body::#names(msg) => msg.msg_cat(),)*
                }
            }

            pub const fn name(&self) -> &'static str {
                match self {
                    #(Body::#names(_) => #names_str,)*
                }
            }

            #admin_base_dispatch
        }

        #(#impl_from_msg)*
    }
}

/// Generate the `Message` struct and its impls.
pub fn generate_fixt_message(serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
    let serde_derives = serde_derives(serde_serialize, serde_deserialize);

    quote! {
        #[allow(dead_code)]
        #[derive(Clone, Debug)]
        #serde_derives
        pub struct Message {
            pub header: Header,
            pub body: Box<Body>,
            pub trailer: Trailer,
        }

        #[allow(dead_code)]
        impl Message {
            pub fn deserialize(mut deserializer: Deserializer) -> Result<Box<Message>, DeserializeError> {
                let begin_string = deserializer.begin_string();
                if begin_string != VERSION.begin_str() {
                    return Err(DeserializeError::Garbled(GarbledReason::BeginStringMismatch));
                }

                let body_length = deserializer.body_length();

                // The FIX framing rule requires MsgType(35) as the third tag —
                // any other outcome (malformed tag number, wrong tag, EOF) is
                // the same protocol violation.
                if !matches!(deserializer.deserialize_tag_num(), Ok(Some(35))) {
                    return Err(DeserializeError::Garbled(GarbledReason::MsgTypeNotThirdTag));
                }
                let msg_type_range = deserializer.deserialize_msg_type()?;
                let msg_type_fixstr = deserializer.range_to_fixstr(msg_type_range);
                let Ok(msg_type) = MsgType::try_from(msg_type_fixstr) else {
                    return Err(deserializer.reject(Some(35), SessionRejectReasonBase::InvalidMsgType));
                };

                let header = Header::deserialize(&mut deserializer, body_length)
                    .map_err(|err| {
                        if let DeserializeError::Reject { reason, .. } = err
                            && reason == SessionRejectReasonBase::RequiredTagMissing
                            && let Ok(Some(tag)) = deserializer.deserialize_tag_num()
                        {
                            deserializer
                                .reject(Some(tag), SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder)
                        } else {
                            err
                        }
                    })?;

                let body = Body::deserialize(&mut deserializer, msg_type)?;

                let trailer = Trailer::deserialize(&mut deserializer)?;

                Ok(Box::new(Message {
                    header,
                    body,
                    trailer,
                }))
            }

            // TODO: Like chrono::Format::DelayedFormat
            pub fn dbg_fix_str(&self) -> impl fmt::Display {
                let mut buf = vec![0u8; 4096];
                let len = self.serialize(&mut buf).expect("serialize failed");
                buf.truncate(len);
                for byte in buf.iter_mut() {
                    if *byte == b'\x01' {
                        *byte = b'|';
                    }
                }
                String::from_utf8_lossy(&buf).into_owned()
            }

            pub const fn msg_type(&self) -> MsgType {
                self.body.msg_type()
            }
        }

        impl SessionMessage for Message {
            fn from_raw_message(raw: RawMessage<'_>) -> Result<Box<Self>, DeserializeError> {
                let deserializer = Deserializer::from_raw_message(raw);
                Message::deserialize(deserializer)
            }

            fn serialize(&self, buf: &mut [u8]) -> Result<usize, SerializeError> {
                let mut serializer = Serializer::new(buf);
                // Framing tags (8, 9, 35) are written here, not in Header::serialize().
                // Tag 8: BeginString (compile-time const per generated crate)
                serializer.put_slice(b"8=")?;
                serializer.serialize_string(VERSION.begin_str())?;
                serializer.put_soh()?;
                // Tag 9: BodyLength (placeholder, patched by serialize_checksum)
                serializer.serialize_body_len()?;
                // Tag 35: MsgType (derived from body, not stored in Header)
                serializer.put_slice(b"35=")?;
                serializer.serialize_enum(&self.body.msg_type())?;
                serializer.put_soh()?;
                // Remaining header fields (34, 49, 52, etc.)
                self.header.serialize(&mut serializer)?;
                self.body.serialize(&mut serializer)?;
                self.trailer.serialize(&mut serializer)?;
                Ok(serializer.pos())
            }

            fn header(&self) -> HeaderBase<'_> {
                HeaderBase::from(&self.header)
            }

            fn try_as_admin(&self) -> Option<AdminBase<'_>> {
                self.body.try_as_admin_base()
            }

            fn msg_type(&self) -> MsgTypeField {
                self.body.msg_type().raw_value()
            }

            fn msg_cat(&self) -> MsgCat {
                self.body.msg_cat()
            }

            fn name(&self) -> &'static str {
                self.body.name()
            }

            fn from_admin(header: HeaderBase<'static>, admin: AdminBase<'static>) -> Self {
                Message {
                    header: Header::from(header),
                    body: Box::new(Body::from(admin)),
                    trailer: Trailer::default(),
                }
            }
        }
    }
}
