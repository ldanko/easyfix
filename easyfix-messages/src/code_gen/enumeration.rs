use convert_case::{Case, Casing};
use easyfix_dictionary::Variant;
use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use super::{member::EnumerableType, serde_derives};

fn variant_ident(name: &str) -> Ident {
    let mut variant_name = name.to_case(Case::UpperCamel);
    if variant_name.as_bytes()[0].is_ascii_digit() {
        variant_name.insert(0, '_');
    }
    Ident::new(&variant_name, Span::call_site())
}

/// Defines a mapping from session-relevant traits/newtypes (in easyfix-core)
/// to generated enums. For each mapping, the generator produces:
/// - `impl TraitName for GeneratedEnum` (raw_value → Int)
/// - `From<NewtypeField> for GeneratedEnum` (using TryFrom + .expect())
/// - Build-time validation that all base enum variant values exist in the
///   generated enum
///
/// Variant matching is by **value** (the FIX wire value), not by name,
/// because the generated variant names come from XML descriptions (e.g.
/// "LOGONS_ARE_NOT_ALLOWED_AT_THIS_TIME" → `LogonsAreNotAllowedAtThisTime`)
/// while base variant names are short session-oriented names.
struct BaseEnumMapping {
    /// Tag number of the generated enum
    generated_enum_tag: u16,
    /// Name of the per-field trait in easyfix-core (e.g. "SessionStatusValue")
    trait_name: &'static str,
    /// Name of the newtype wrapper in easyfix-core (e.g. "SessionStatusField")
    field_type_name: &'static str,
    /// Base enum variant values that must exist in the generated enum.
    /// Used for build-time validation only.
    base_variant_values: &'static [&'static str],
}

const BASE_ENUM_MAPPINGS: &[BaseEnumMapping] = &[
    BaseEnumMapping {
        generated_enum_tag: 98, // EncryptMethod
        trait_name: "",         // No trait for EncryptMethod (kept as-is)
        field_type_name: "",
        base_variant_values: &["0"],
    },
    BaseEnumMapping {
        generated_enum_tag: 35, // MsgType
        trait_name: "MsgTypeValue",
        field_type_name: "MsgTypeField",
        base_variant_values: &["0", "1", "2", "3", "4", "5", "A"],
    },
    BaseEnumMapping {
        generated_enum_tag: 1409, // SessionStatus
        trait_name: "SessionStatusValue",
        field_type_name: "SessionStatusField",
        base_variant_values: &["0", "4", "9", "10"],
    },
    BaseEnumMapping {
        generated_enum_tag: 373, // SessionRejectReason
        trait_name: "SessionRejectReasonValue",
        field_type_name: "SessionRejectReasonField",
        base_variant_values: &[
            "0", "1", "2", "3", "4", "5", "6", "9", "10", "11", "13", "14", "15", "16",
        ],
    },
];

pub struct EnumCodeGen {
    name: Ident,
    tag: u16,
    enumerable_type: EnumerableType,
    variants: Vec<Variant>,
}
impl EnumCodeGen {
    pub fn new(
        name: &str,
        tag: u16,
        enumerable_type: EnumerableType,
        variants: Vec<Variant>,
    ) -> EnumCodeGen {
        EnumCodeGen {
            name: Ident::new(&name.to_case(Case::UpperCamel), Span::call_site()),
            tag,
            enumerable_type,
            variants,
        }
    }

    pub fn tag(&self) -> u16 {
        self.tag
    }

    /// If this enum has a corresponding trait/newtype in easyfix-core, generate:
    /// - `impl TraitName for GeneratedEnum` (raw_value → Int)
    /// - `From<NewtypeField> for GeneratedEnum` (using TryFrom + .expect())
    /// - For EncryptMethod (no trait): `From<EncryptMethodBase> for EncryptMethod` (legacy)
    ///
    /// Returns empty TokenStream if no mapping exists. Panics if a base
    /// variant value has no matching variant in the generated enum.
    pub fn generate_base_enum_conversion(&self) -> TokenStream {
        let Some(mapping) = BASE_ENUM_MAPPINGS
            .iter()
            .find(|m| m.generated_enum_tag == self.tag)
        else {
            return quote! {};
        };

        let generated_name = &self.name;

        // Build-time validation: all base variant values must exist in generated enum
        for &value in mapping.base_variant_values {
            self.variants
                .iter()
                .find(|v| v.value() == value)
                .unwrap_or_else(|| {
                    panic!(
                        "Base enum variant value {value} has no matching variant \
                         in generated enum {generated_name} (tag {}). The XML must define this value.",
                        self.tag
                    )
                });
        }

        // EncryptMethod: no trait/newtype, keep legacy From<BaseEnum> conversion
        if mapping.trait_name.is_empty() {
            return self.generate_legacy_base_enum_from();
        }

        // MsgType: string-typed enum, raw_value returns MsgTypeField
        if self.tag == 35 {
            return self.generate_msg_type_conversion(mapping);
        }

        let trait_name = Ident::new(mapping.trait_name, Span::call_site());
        let field_type_name = Ident::new(mapping.field_type_name, Span::call_site());

        // Generate raw_value() match arms for the trait impl
        let trait_match_arms = self.variants.iter().map(|v| {
            let variant_ident = variant_ident(v.name());
            let int_value: i64 = v.value().parse().unwrap_or_else(|_| {
                panic!(
                    "Enum {generated_name} variant {} has non-integer value {:?}, \
                     cannot implement {trait_name}",
                    v.name(),
                    v.value(),
                )
            });
            let int_lit = proc_macro2::Literal::i64_suffixed(int_value);
            quote! { #generated_name::#variant_ident => #int_lit, }
        });

        quote! {
            impl #trait_name for #generated_name {
                fn raw_value(&self) -> Int {
                    match self {
                        #(#trait_match_arms)*
                    }
                }
            }

            impl From<#field_type_name> for #generated_name {
                fn from(field: #field_type_name) -> #generated_name {
                    #generated_name::try_from(field.into_inner())
                        .expect("validated by field newtype")
                }
            }
        }
    }

    /// MsgType conversion: `impl MsgTypeValue for MsgType` + `From<MsgTypeField> for MsgType`.
    ///
    /// Unlike int-typed enums, MsgType's raw value is byte-based.
    /// `raw_value()` returns `MsgTypeField` (constructed from `as_bytes()`).
    /// `From<MsgTypeField>` uses `from_bytes` + `try_from` on `&FixStr`.
    fn generate_msg_type_conversion(&self, mapping: &BaseEnumMapping) -> TokenStream {
        let generated_name = &self.name;
        let trait_name = Ident::new(mapping.trait_name, Span::call_site());
        let field_type_name = Ident::new(mapping.field_type_name, Span::call_site());

        quote! {
            impl #trait_name for #generated_name {
                fn raw_value(&self) -> #field_type_name {
                    #field_type_name::from_bytes(self.as_bytes())
                        .expect("generated MsgType values are valid")
                }
            }

            impl From<#field_type_name> for #generated_name {
                fn from(field: #field_type_name) -> #generated_name {
                    #generated_name::from_bytes(field.as_bytes())
                        .expect("validated by MsgTypeField")
                }
            }
        }
    }

    /// Legacy conversion for EncryptMethod: `From<EncryptMethodBase> for EncryptMethod`.
    fn generate_legacy_base_enum_from(&self) -> TokenStream {
        // EncryptMethod has only one base variant: None = 0
        let generated_name = &self.name;
        let generated_variant = self
            .variants
            .iter()
            .find(|v| v.value() == "0")
            .expect("EncryptMethod must have a variant with value 0");
        let generated_variant_ident = variant_ident(generated_variant.name());

        quote! {
            impl From<EncryptMethodBase> for #generated_name {
                fn from(input: EncryptMethodBase) -> #generated_name {
                    match input {
                        EncryptMethodBase::None => #generated_name::#generated_variant_ident,
                    }
                }
            }
        }
    }

    pub fn generate(&self, serde_serialize: bool, serde_deserialize: bool) -> TokenStream {
        let name = &self.name;
        let try_from_type = match self.enumerable_type {
            EnumerableType::Int => quote! { Int },
            EnumerableType::NumInGroup => quote! { NumInGroup },
            EnumerableType::Char => quote! { Char },
            EnumerableType::String | EnumerableType::MultipleStringValue => quote! { &FixStr },
            EnumerableType::MultipleCharValue => quote! { Char },
        };
        let mut variant_def = Vec::with_capacity(self.variants.len());
        let mut variant_name = Vec::with_capacity(self.variants.len());
        let mut variant_value_as_bytes = Vec::with_capacity(self.variants.len());
        for variant in &self.variants {
            let v_name = variant_ident(variant.name());
            let v_value_as_bytes = Literal::byte_string(variant.value().as_bytes());
            // TODO: check in easyfix-dictionary if variant value can be expressed as FixString
            // (no utf-8, no control characteres, etc), if not check here before using it in
            // FixStr::from_ascii_unchecked

            let variant_doc_comment = format!("Value \"{}\"", variant.value());
            variant_def.push(quote! {
                #[doc = #variant_doc_comment]
                #v_name
            });
            variant_name.push(v_name);
            variant_value_as_bytes.push(v_value_as_bytes);
        }
        let variant_value = self
            .variants
            .iter()
            .map(|variant| self.enumerable_type.literal(variant.value()));

        let int_value_method = match self.enumerable_type {
            EnumerableType::Int => {
                let int_values = self
                    .variants
                    .iter()
                    .map(|v| Literal::i64_suffixed(v.value().parse().unwrap()));
                quote! {
                    pub const fn as_int(&self) -> Int {
                        match self {
                            #(#name::#variant_name => #int_values,)*
                        }
                    }
                }
            }
            EnumerableType::NumInGroup => {
                let int_values = self
                    .variants
                    .iter()
                    .map(|v| Literal::u8_suffixed(v.value().parse().unwrap()));
                quote! {
                    pub const fn as_num_in_group(&self) -> NumInGroup {
                        match self {
                            #(#name::#variant_name => #int_values,)*
                        }
                    }
                }
            }
            _ => quote! {},
        };
        let try_from_match_input = if matches!(
            self.enumerable_type,
            EnumerableType::String | EnumerableType::MultipleStringValue
        ) {
            quote! { match input.as_bytes() }
        } else {
            quote! { match input }
        };
        let serde_derives = serde_derives(serde_serialize, serde_deserialize);
        let derives = if name == "MsgType" {
            quote! {
                #[allow(dead_code)]
                #[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
                #serde_derives
            }
        } else {
            quote! {
                #[allow(dead_code)]
                #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
                #serde_derives
            }
        };
        quote! {
            #derives
            pub enum #name {
                #[default]
                #(#variant_def,)*
            }

            impl #name {
                // TODO: try_*
                pub const fn from_bytes(input: &[u8]) -> Option<#name> {
                    match input {
                        #(#variant_value_as_bytes => Some(#name::#variant_name),)*
                        _ => None,
                    }
                }

                // TODO: try_*
                pub const fn from_fix_str(input: &FixStr) -> Option<#name> {
                    #name::from_bytes(input.as_bytes())
                }

                pub const fn as_bytes(&self) -> &'static [u8] {
                    match self {
                        #(#name::#variant_name => #variant_value_as_bytes,)*
                    }
                }

                pub const fn as_fix_str(&self) -> &'static FixStr {
                    // Safety: value was checked when it was generated
                    unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
                }

                #int_value_method
            }

            impl ToFixString for #name {
                fn to_fix_string(&self) -> FixString {
                    self.as_fix_str().to_owned()
                }
            }

            impl TryFrom<#try_from_type> for #name {
                type Error = SessionRejectReasonBase;

                fn try_from(input: #try_from_type) -> Result<#name, SessionRejectReasonBase> {
                    #try_from_match_input {
                        #(#variant_value => Ok(#name::#variant_name),)*
                        _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
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
