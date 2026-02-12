use proc_macro2::{Ident, Literal, TokenStream};
use quote::quote;

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

/// Generate the `Message` enum with serialize/deserialize dispatch,
/// From impls, msg_type(), and msg_cat().
pub fn generate_message_enum(names: &[&Ident]) -> TokenStream {
    let impl_from_msg = names.iter().map(|name| {
        quote! {
            impl From<#name> for Message {
                fn from(msg: #name) -> Message {
                    Message::#name(msg)
                }
            }
        }
    });

    quote! {
        #[derive(Clone, Debug)]
        #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
        #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
        #[allow(clippy::large_enum_variant)]
        pub enum Message {
            #(#names(#names),)*
        }

        impl Message {
            fn serialize(&self, serializer: &mut Serializer) {
                match self {
                    #(Message::#names(msg) => msg.serialize(serializer),)*
                }
            }

            fn deserialize(
                deserializer: &mut Deserializer,
                msg_type: MsgType
            ) -> Result<Box<Message>, DeserializeError> {
                match msg_type {
                    #(
                        MsgType::#names => Ok(#names::deserialize(deserializer)?),
                    )*
                }
            }

            pub const fn msg_type(&self) -> MsgType {
                match self {
                    #(Message::#names(_) => MsgType::#names,)*
                }
            }

            pub const fn msg_cat(&self) -> MsgCat {
                match self {
                    #(Message::#names(msg) => msg.msg_cat(),)*
                }
            }
        }

        #(#impl_from_msg)*
    }
}

/// Generate the `FixtMessage` struct and its impls.
pub fn generate_fixt_message() -> TokenStream {
    quote! {
        #[derive(Clone, Debug)]
        #[cfg_attr(feature = "serialize", derive(serde::Serialize))]
        #[cfg_attr(feature = "deserialize", derive(serde::Deserialize))]
        pub struct FixtMessage {
            pub header: Box<Header>,
            pub body: Box<Message>,
            pub trailer: Box<Trailer>,
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
                    .map_err(|e| DeserializeError::GarbledMessage(format!("failed to parse MsgType<35>: {e}")))?
                {
                    let msg_type_range = deserializer.deserialize_msg_type()?;
                    let msg_type_fixstr = deserializer.range_to_fixstr(msg_type_range);
                    let Ok(msg_type) = MsgType::try_from(msg_type_fixstr) else {
                        return Err(deserializer.reject(Some(35), ParseRejectReason::InvalidMsgtype));
                    };
                    msg_type
                } else {
                    return Err(DeserializeError::GarbledMessage("MsgType<35> not third tag".into()));
                };

                let header = Header::deserialize(&mut deserializer, begin_string, body_length, msg_type)
                    .map_err(|err| {
                        if let DeserializeError::Reject { reason, .. } = err
                            && matches!(reason, ParseRejectReason::RequiredTagMissing)
                            && let Ok(Some(tag)) = deserializer.deserialize_tag_num()
                        {
                            deserializer
                                .reject(Some(tag), ParseRejectReason::TagSpecifiedOutOfRequiredOrder)
                        } else {
                            err
                        }
                    })?;

                let body = Message::deserialize(&mut deserializer, msg_type)?;

                let trailer = Trailer::deserialize(&mut deserializer)?;

                Ok(Box::new(FixtMessage {
                    header,
                    body,
                    trailer
                }))
            }

            pub fn from_raw_message(raw_message: RawMessage) -> Result<Box<FixtMessage>, DeserializeError> {
                let deserializer = Deserializer::from_raw_message(raw_message);
                FixtMessage::deserialize(deserializer)
            }

            pub fn from_bytes(input: &[u8]) -> Result<Box<FixtMessage>, DeserializeError> {
                let (_, raw_msg) = raw_message(input)?;
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
