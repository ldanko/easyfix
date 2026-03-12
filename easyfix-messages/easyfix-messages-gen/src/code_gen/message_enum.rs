use proc_macro2::{Ident, Literal, TokenStream};
use quote::quote;

use super::admin;

/// Generate the `FieldTag` enum and its impls.
pub fn generate_field_tag(fields_names: &[Ident], fields_numbers: &[u16]) -> TokenStream {
    let fields_names_as_bytes = fields_names
        .iter()
        .map(|f| Literal::byte_string(f.to_string().as_bytes()));
    let fields_numbers_literals = fields_numbers.iter().copied().map(Literal::u16_suffixed);

    quote! {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
        #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
        #[repr(u16)]
        pub enum FieldTag {
            #(#fields_names = #fields_numbers,)*
        }

        impl fmt::Display for FieldTag {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_fix_str().as_utf8())
            }
        }

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
pub fn generate_message_enum(names: &[&Ident]) -> TokenStream {
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

    quote! {
        #[derive(Clone, Debug)]
        #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
        #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
        #[allow(clippy::large_enum_variant)]
        pub enum Body {
            #(#names(#names),)*
        }

        impl Body {
            fn serialize(&self, serializer: &mut Serializer) {
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
pub fn generate_fixt_message() -> TokenStream {
    quote! {
        #[derive(Clone, Debug)]
        #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
        #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
        pub struct Message {
            pub header: Header,
            pub body: Box<Body>,
            pub trailer: Trailer,
        }

        impl Message {
            pub fn deserialize(mut deserializer: Deserializer) -> Result<Box<Message>, DeserializeError> {
                let begin_string = deserializer.begin_string();
                if begin_string != BEGIN_STRING {
                    return Err(DeserializeError::GarbledMessage("begin string mismatch".into()));
                }

                let body_length = deserializer.body_length();

                // Check if MsgType(35) is the third tag in a message.
                let msg_type = if let Some(35) = deserializer
                    .deserialize_tag_num()
                    .map_err(|e| DeserializeError::GarbledMessage(format!("failed to parse MsgType<35>: {e}")))?
                {
                    let msg_type_range = deserializer.deserialize_msg_type()?;
                    let msg_type_fixstr = deserializer.range_to_fixstr(msg_type_range);
                    let Ok(msg_type) = MsgType::try_from(msg_type_fixstr) else {
                        return Err(deserializer.reject(Some(35), SessionRejectReasonBase::InvalidMsgType));
                    };
                    msg_type
                } else {
                    return Err(DeserializeError::GarbledMessage("MsgType<35> not third tag".into()));
                };

                let header = Header::deserialize(&mut deserializer, begin_string, body_length)
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

            pub fn from_raw_message(raw_message: RawMessage) -> Result<Box<Message>, DeserializeError> {
                let deserializer = Deserializer::from_raw_message(raw_message);
                Message::deserialize(deserializer)
            }

            pub fn from_bytes(input: &[u8]) -> Result<Box<Message>, DeserializeError> {
                let (_, raw_msg) = raw_message(input)?;
                let deserializer = Deserializer::from_raw_message(raw_msg);
                Message::deserialize(deserializer)
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
        }

        impl SessionMessage for Message {
            fn from_raw_message(raw: RawMessage<'_>) -> Result<Self, DeserializeError> {
                let deserializer = Deserializer::from_raw_message(raw);
                Ok(*Message::deserialize(deserializer)?)
            }

            fn serialize(&self) -> Vec<u8> {
                let mut serializer = Serializer::new();
                // Framing tags (8, 9, 35) are written here, not in Header::serialize().
                // Tag 8: BeginString
                serializer.output_mut().extend_from_slice(b"8=");
                serializer.serialize_string(&self.header.begin_string);
                serializer.output_mut().push(b'\x01');
                // Tag 9: BodyLength (placeholder, patched by serialize_checksum)
                serializer.serialize_body_len();
                // Tag 35: MsgType (derived from body, not stored in Header)
                serializer.output_mut().extend_from_slice(b"35=");
                serializer.serialize_enum(&self.body.msg_type());
                serializer.output_mut().push(b'\x01');
                // Remaining header fields (34, 49, 52, etc.)
                self.header.serialize(&mut serializer);
                self.body.serialize(&mut serializer);
                self.trailer.serialize(&mut serializer);
                serializer.take()
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
