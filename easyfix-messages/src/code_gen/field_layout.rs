use std::collections::{BTreeMap, BTreeSet};

use easyfix_dictionary::{self as dict, Dictionary};
use proc_macro2::{Literal, TokenStream};
use quote::quote;

#[derive(Clone, Copy)]
enum MessageSection {
    Header,
    Body,
    Trailer,
}

#[derive(Default)]
struct FieldLayouts {
    header_tags: BTreeSet<u16>,
    trailer_tags: BTreeSet<u16>,
    data_lengths: BTreeMap<u16, Option<u16>>,
}

impl FieldLayouts {
    fn collect_tag(&mut self, tag: u16, section: MessageSection) {
        match section {
            MessageSection::Header => {
                self.header_tags.insert(tag);
            }
            MessageSection::Trailer => {
                self.trailer_tags.insert(tag);
            }
            MessageSection::Body => {}
        }
    }

    fn collect(&mut self, members: &[dict::Member], section: MessageSection) {
        for member in members {
            match member.definition() {
                dict::MemberDefinition::Field(field) => {
                    self.collect_tag(field.number(), section);
                }
                dict::MemberDefinition::RawData { length, data } => {
                    self.collect_tag(length.number(), section);
                    self.collect_tag(data.number(), section);
                    self.data_lengths
                        .entry(data.number())
                        .and_modify(|tag| {
                            if *tag != Some(length.number()) {
                                *tag = None;
                            }
                        })
                        .or_insert(Some(length.number()));
                }
                dict::MemberDefinition::Group(group) => {
                    self.collect_tag(group.num_in_group().number(), section);
                    self.collect(group.members(), section);
                }
                dict::MemberDefinition::Component(_) => {
                    panic!("components must be flattened before collecting field layouts");
                }
            }
        }
    }

    fn collect_dictionary(&mut self, dictionary: &Dictionary) {
        self.collect(dictionary.header().members(), MessageSection::Body);
        self.collect(dictionary.trailer().members(), MessageSection::Body);
        for message in dictionary.messages() {
            self.collect(message.members(), MessageSection::Body);
        }
    }
}

pub(super) fn generate(
    dictionary: &Dictionary,
    app_dictionary: Option<&Dictionary>,
    fields: &[&dict::Field],
) -> TokenStream {
    let mut layouts = FieldLayouts::default();
    layouts.collect(dictionary.header().members(), MessageSection::Header);
    layouts.collect(dictionary.trailer().members(), MessageSection::Trailer);
    layouts.collect_dictionary(dictionary);
    if let Some(app_dictionary) = app_dictionary {
        layouts.collect_dictionary(app_dictionary);
    }

    let header_arm = generate_tag_arm(
        &layouts.header_tags,
        quote! { Some(MessageSection::Header) },
    );
    let trailer_arm = generate_tag_arm(
        &layouts.trailer_tags,
        quote! { Some(MessageSection::Trailer) },
    );
    let delimited_tags = fields
        .iter()
        .filter(|field| {
            !matches!(
                field.data_type(),
                dict::BasicType::Data | dict::BasicType::XmlData
            )
        })
        .map(|field| field.number())
        .collect();
    let delimited_arm = generate_tag_arm(&delimited_tags, quote! { FieldEncoding::Delimited });
    // Unknown or conflicting length associations cannot establish a boundary.
    let data_arms = layouts.data_lengths.iter().filter_map(|(data, length)| {
        let length = Literal::u16_suffixed((*length)?);
        let data = Literal::u16_suffixed(*data);
        Some(quote! { #data => FieldEncoding::Data { length_tag: #length }, })
    });

    quote! {
        fn section(tag: TagNum) -> Option<MessageSection> {
            match tag {
                #header_arm
                #trailer_arm
                _ => Self::from_tag_num(tag).map(|_| MessageSection::Body),
            }
        }

        fn field_layout(tag: TagNum) -> Option<FieldLayout> {
            let encoding = match tag {
                #delimited_arm
                #(#data_arms)*
                _ => return None,
            };
            let section = Self::section(tag)?;
            Some(FieldLayout { section, encoding })
        }
    }
}

fn generate_tag_arm(tags: &BTreeSet<u16>, layout: TokenStream) -> TokenStream {
    if tags.is_empty() {
        return quote! {};
    }
    let tags = tags.iter().copied().map(Literal::u16_suffixed);
    quote! { #(#tags)|* => #layout, }
}

pub(super) fn generate_diagnostics() -> TokenStream {
    quote! {
        #[cold]
        fn find_out_of_order_field(
            deserializer: &Deserializer,
            section: MessageSection,
        ) -> Option<TagNum> {
            let mut cursor = deserializer.field_cursor();
            let mut last_section = None;
            let mut previous_field: Option<(TagNum, FieldCursor<'_>)> = None;

            loop {
                let tag = cursor.next_tag()?;
                let field = FieldTag::field_layout(tag)?;
                if let Some(last) = last_section {
                    if field.section < last {
                        return Some(tag);
                    }
                } else if field.section <= section {
                    return None;
                }
                last_section = Some(field.section);

                match field.encoding {
                    FieldEncoding::Delimited => {
                        previous_field = Some((tag, cursor));
                        cursor.read_delimited()?;
                    }
                    FieldEncoding::Data { length_tag } => {
                        let (previous_tag, mut length_cursor) = previous_field.take()?;
                        if previous_tag != length_tag {
                            return None;
                        }
                        cursor.read_data(length_cursor.read_length()?)?;
                    }
                }
            }
        }

        #[cold]
        fn missing_required_field(
            deserializer: &mut Deserializer,
            section: MessageSection,
            missing_tag: TagNum,
        ) -> DeserializeErrorKind {
            let (tag, reason) = match find_out_of_order_field(deserializer, section) {
                Some(tag) => (tag, SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder),
                None => (missing_tag, SessionRejectReasonBase::RequiredTagMissing),
            };
            deserializer.reject(Some(tag), reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, process};

    use super::*;

    #[test]
    fn nested_members_and_shared_length_associations_are_collected() {
        let xml = r#"
<fix type='FIX' major='4' minor='4' servicepack='0'>
  <header>
    <field name='BeginString' required='Y'/>
    <field name='SecureDataLen' required='N'/>
    <field name='SecureData' required='N'/>
    <group name='NoHops' required='N'>
      <field name='HopCompID' required='N'/>
    </group>
  </header>
  <trailer>
    <field name='SignatureLength' required='N'/>
    <field name='Signature' required='N'/>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message name='First' msgtype='D' msgcat='app'>
      <group name='NoOuter' required='N'>
        <group name='NoInner' required='N'>
          <field name='NestedLength' required='N'/>
          <field name='NestedData' required='N'/>
        </group>
      </group>
      <field name='SharedLength' required='N'/>
      <field name='FirstData' required='N'/>
      <field name='MixedLength' required='N'/>
      <field name='MixedData' required='N'/>
    </message>
    <message name='Second' msgtype='G' msgcat='app'>
      <field name='SharedLength' required='N'/>
      <field name='SecondData' required='N'/>
      <field name='MixedLength' required='N'/>
    </message>
  </messages>
  <components/>
  <fields>
    <field name='BeginString' number='8' type='STRING'/>
    <field name='CheckSum' number='10' type='STRING'/>
    <field name='SignatureLength' number='93' type='LENGTH'/>
    <field name='Signature' number='89' type='DATA'/>
    <field name='SecureDataLen' number='90' type='LENGTH'/>
    <field name='SecureData' number='91' type='DATA'/>
    <field name='NoHops' number='627' type='NUMINGROUP'/>
    <field name='HopCompID' number='628' type='STRING'/>
    <field name='NoOuter' number='6000' type='NUMINGROUP'/>
    <field name='NoInner' number='6001' type='NUMINGROUP'/>
    <field name='NestedLength' number='6002' type='LENGTH'/>
    <field name='NestedData' number='6003' type='XMLDATA'/>
    <field name='SharedLength' number='6004' type='LENGTH'/>
    <field name='FirstData' number='6005' type='DATA'/>
    <field name='SecondData' number='6006' type='DATA'/>
    <field name='MixedLength' number='6007' type='LENGTH'/>
    <field name='MixedData' number='6008' type='DATA'/>
  </fields>
</fix>
"#;
        let path = env::temp_dir().join(format!("easyfix_field_layout_{}.xml", process::id()));
        fs::write(&path, xml).unwrap();
        let dictionary = Dictionary::new(path.to_str().unwrap())
            .unwrap()
            .flatten()
            .unwrap();
        fs::remove_file(&path).unwrap();

        let mut layouts = FieldLayouts::default();
        layouts.collect(dictionary.header().members(), MessageSection::Header);
        layouts.collect(dictionary.trailer().members(), MessageSection::Trailer);
        layouts.collect_dictionary(&dictionary);
        assert_eq!(layouts.header_tags, BTreeSet::from([8, 90, 91, 627, 628]));
        assert_eq!(layouts.trailer_tags, BTreeSet::from([10, 89, 93]));
        let expected_lengths = BTreeMap::from([
            (91, Some(90)),
            (89, Some(93)),
            (6003, Some(6002)),
            (6005, Some(6004)),
            (6006, Some(6004)),
            (6008, Some(6007)),
        ]);
        assert_eq!(layouts.data_lengths, expected_lengths);

        // Repeated definitions neither duplicate nor invalidate associations.
        let first = dictionary.messages().next().unwrap();
        layouts.collect(first.members(), MessageSection::Body);
        assert_eq!(layouts.data_lengths, expected_lengths);
    }
}
