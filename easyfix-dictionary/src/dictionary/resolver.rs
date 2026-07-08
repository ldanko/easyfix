use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    vec,
};

use easyfix_core::{
    basic_types::{FixStr, FixString},
    fix_str,
};

use super::{
    Version,
    error::{Error, ValidationError},
    types::{Component, Field, Group, Member, MemberDefinition, Message},
};
use crate::{xml, xml::BasicType};

/// Identity of a component that wraps a single group, passed down so the
/// group can take over the component's name and documentation.
#[derive(Clone, Copy)]
pub(super) struct ParentComponent<'a> {
    pub(super) name: &'a FixStr,
    pub(super) doc: Option<&'a str>,
}

/// Resolved dictionary elements ready to be consumed by `Dictionary`.
pub(super) struct Elements {
    pub(super) fields: HashMap<FixString, Rc<Field>>,
    pub(super) components: HashMap<FixString, Rc<Component>>,
    pub(super) groups: HashMap<FixString, Rc<Group>>,
}

/// Resolves raw XML definitions into domain types.
///
/// Converts raw XML field, component, and group definitions into their
/// domain-model equivalents, handling deduplication via `Rc`, reference
/// resolution by name, and circular-reference detection.
///
/// This is a transient structure: create it, call its resolve/create
/// methods, then consume it via [`Resolver::into_elements`].
pub(super) struct Resolver {
    raw_fields: HashMap<FixString, xml::Field>,
    fields: HashMap<FixString, Rc<Field>>,
    raw_components: HashMap<FixString, xml::Component>,
    components: HashMap<FixString, Rc<Component>>,
    groups: HashMap<FixString, Rc<Group>>,
}

impl Resolver {
    pub(super) fn new(
        raw_fields: Vec<xml::Field>,
        raw_components: Vec<xml::Component>,
    ) -> Result<Resolver, Error> {
        let mut names: HashSet<FixString> = HashSet::new();
        let mut raw_fields_map = HashMap::with_capacity(raw_fields.len());
        for field in raw_fields {
            if !names.insert(field.name.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedField(
                    field.name.to_string(),
                )));
            }
            raw_fields_map.insert(field.name.clone(), field);
        }

        let mut raw_components_map = HashMap::with_capacity(raw_components.len());
        for comp in raw_components {
            if !names.insert(comp.name.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedComponent(
                    comp.name.to_string(),
                )));
            }
            raw_components_map.insert(comp.name.clone(), comp);
        }

        Ok(Resolver {
            raw_fields: raw_fields_map,
            fields: HashMap::new(),
            raw_components: raw_components_map,
            components: HashMap::new(),
            groups: HashMap::new(),
        })
    }

    /// Consumes the resolver and returns all resolved elements.
    pub(super) fn into_elements(self) -> Elements {
        Elements {
            fields: self.fields,
            components: self.components,
            groups: self.groups,
        }
    }

    fn create_field(&mut self, name: &FixStr) -> Result<Rc<Field>, Error> {
        if let Some(field) = self.fields.get(name) {
            return Ok(field.clone());
        }

        let (field_name, raw_field) = self
            .raw_fields
            .remove_entry(name)
            .ok_or_else(|| Error::Validation(ValidationError::UnknownField(name.to_string())))?;
        let field = Rc::new(Field::from(raw_field));
        self.fields.insert(field_name, field.clone());

        Ok(field)
    }

    fn create_component(
        &mut self,
        name: FixString,
        visited: &mut HashSet<FixString>,
    ) -> Result<Rc<Component>, Error> {
        if !visited.insert(name.clone()) {
            return Err(Error::Validation(ValidationError::CircularReference(
                name.into(),
            )));
        }
        if let Some(component) = self.components.get(&name) {
            return Ok(component.clone());
        }

        let raw_component = self.raw_components.remove(&name).ok_or_else(|| {
            Error::Validation(ValidationError::UnknownComponent(name.to_string()))
        })?;
        if raw_component.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer(
                name.into(),
            )));
        }

        let mut branch_visited = visited.clone();
        let doc = raw_component.doc;
        let members = self.create_members_impl(
            raw_component.members,
            Some(ParentComponent {
                name: &name,
                doc: doc.as_deref(),
            }),
            &mut branch_visited,
        )?;

        visited.remove(&name);
        let component = Rc::new(Component { name, doc, members });
        self.components
            .insert(component.name.clone(), component.clone());

        Ok(component)
    }

    /// Creates a group from the XML representation.
    ///
    /// The group name drops the counter field's "No" prefix (`NoHops` ->
    /// `Hops`), except when `parent_component` is set: a group that is the
    /// sole member of a component takes that component's name and
    /// documentation instead (`Parties`, not `PartyIDs`). See [`Group`] for
    /// the naming convention.
    fn create_group(
        &mut self,
        raw_group: xml::Group,
        parent_component: Option<ParentComponent<'_>>,
        visited: &mut HashSet<FixString>,
    ) -> Result<Rc<Group>, Error> {
        // Determine the group name
        let group_name = if let Some(parent_component) = parent_component {
            // Use parent component name when component contains only this group
            parent_component.name.to_owned()
        } else if raw_group.name.as_bytes().starts_with(b"No") {
            // Strip "No" prefix: "NoHops" -> "Hops"
            // SAFETY: a subslice of a valid FixStr is still printable ASCII.
            let group_name =
                unsafe { FixStr::from_ascii_unchecked(&raw_group.name.as_bytes()[2..]) }.to_owned();
            if !visited.insert(group_name.clone()) {
                return Err(Error::Validation(ValidationError::CircularReference(
                    group_name.into(),
                )));
            }
            group_name
        } else {
            // Use name as-is if it doesn't follow "No" convention
            let group_name = raw_group.name.clone();
            if !visited.insert(group_name.clone()) {
                return Err(Error::Validation(ValidationError::CircularReference(
                    group_name.into(),
                )));
            }
            group_name
        };

        // The group takes over a wrapping component's documentation the same
        // way it takes over its name; fall back to the group's own doc.
        let doc = parent_component
            .and_then(|parent_component| parent_component.doc.map(String::from))
            .or(raw_group.doc);

        let mut branch_visited = visited.clone();
        let group = Rc::new(Group {
            num_in_group: self.create_field(&raw_group.name)?,
            members: self.create_members_impl(raw_group.members, None, &mut branch_visited)?,
            name: group_name,
            doc,
        });

        if self
            .groups
            .insert(group.name.clone(), group.clone())
            .is_some()
        {
            return Err(Error::Validation(ValidationError::DuplicatedGroup(
                group.name.to_string(),
            )));
        }

        visited.remove(&group.name);

        Ok(group)
    }

    /// If the next member in the iterator is a Data or XmlData field, consumes it
    /// and returns the created field. Otherwise returns `None` without advancing
    /// the iterator.
    fn try_take_data_field(
        &mut self,
        iter: &mut std::iter::Peekable<vec::IntoIter<xml::Member>>,
    ) -> Result<Option<Rc<Field>>, Error> {
        let Some(xml::Member::Field(next_ref)) = iter.peek() else {
            return Ok(None);
        };
        let field = self.create_field(&next_ref.name)?;
        if !matches!(field.data_type(), BasicType::Data | BasicType::XmlData) {
            return Ok(None);
        }
        iter.next();
        Ok(Some(field))
    }

    // Create members from raw XML members, detecting Length+Data/XmlData pairs
    fn create_members_impl(
        &mut self,
        raw_members: Vec<xml::Member>,
        parent_component: Option<ParentComponent<'_>>,
        visited: &mut HashSet<FixString>,
    ) -> Result<Vec<Member>, Error> {
        let raw_members_len = raw_members.len();
        let parent_component_for_group = if raw_members_len == 1 {
            parent_component
        } else {
            None
        };

        let mut members = Vec::with_capacity(raw_members_len);
        let mut iter = raw_members.into_iter().peekable();

        while let Some(raw_member) = iter.next() {
            let (required, definition) = match raw_member {
                xml::Member::Field(field_ref) => {
                    let field = self.create_field(&field_ref.name)?;
                    let data = if field.data_type() == BasicType::Length {
                        self.try_take_data_field(&mut iter)?
                    } else {
                        None
                    };
                    let definition = if let Some(data) = data {
                        MemberDefinition::RawData {
                            length: field,
                            data,
                        }
                    } else {
                        MemberDefinition::Field(field)
                    };
                    (field_ref.required, definition)
                }
                xml::Member::Component(member_ref) => {
                    let component = self.create_component(member_ref.name, visited)?;
                    // A component wrapping a single group is semantically
                    // a group - flatten it so consumers see MemberDefinition::Group
                    // directly, with the required flag from the usage site.
                    if let [
                        Member {
                            definition: MemberDefinition::Group(group),
                            ..
                        },
                    ] = component.members.as_slice()
                    {
                        (member_ref.required, MemberDefinition::Group(group.clone()))
                    } else {
                        (member_ref.required, MemberDefinition::Component(component))
                    }
                }
                xml::Member::Group(group) => (
                    group.required,
                    MemberDefinition::Group(self.create_group(
                        group.clone(),
                        parent_component_for_group,
                        visited,
                    )?),
                ),
            };
            members.push(Member {
                required,
                definition,
            });
        }

        Ok(members)
    }

    pub(super) fn create_members(
        &mut self,
        raw_members: Vec<xml::Member>,
        parent_component: Option<ParentComponent<'_>>,
    ) -> Result<Vec<Member>, Error> {
        let mut visited = HashSet::new();
        self.create_members_impl(raw_members, parent_component, &mut visited)
    }

    pub(super) fn create_message(&mut self, msg: xml::Message) -> Result<Message, Error> {
        let members = self.create_members(msg.members, None)?;
        if members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyMessage(
                msg.name.into(),
            )));
        }

        Ok(Message {
            name: msg.name,
            msg_type: msg.msg_type,
            msg_cat: msg.msg_cat,
            doc: msg.doc,
            members,
        })
    }

    pub(super) fn check_unused_elements(&self) -> Result<(), Error> {
        // MsgType is skipped because in app dictionaries it may be defined
        // in <fields> but not referenced by any message - the header that uses
        // it is defined separately in the FIXT dictionary.
        if let Some(field) = self.raw_fields.values().find(|f| f.name != "MsgType") {
            Err(Error::Validation(ValidationError::UnusedField(
                field.name.to_string(),
                field.number,
            )))
        } else if let Some(component) = self.raw_components.values().next() {
            Err(Error::Validation(ValidationError::UnusedComponent(
                component.name.to_string(),
            )))
        } else {
            Ok(())
        }
    }

    pub(super) fn register_unused_elements(&mut self) -> Result<(), Error> {
        for (field_name, raw_field) in self.raw_fields.drain() {
            let field = Rc::new(Field::from(raw_field));
            self.fields.insert(field_name, field.clone());
        }

        let mut raw_components = std::mem::take(&mut self.raw_components);
        for (name, raw_component) in raw_components.drain() {
            if raw_component.members.is_empty() {
                return Err(Error::Validation(ValidationError::EmptyContainer(
                    name.into(),
                )));
            }
            let doc = raw_component.doc;
            let members = self.create_members(
                raw_component.members,
                Some(ParentComponent {
                    name: &name,
                    doc: doc.as_deref(),
                }),
            )?;
            let component = Rc::new(Component { name, doc, members });
            self.components
                .insert(component.name.clone(), component.clone());
        }

        Ok(())
    }
}

pub(super) fn check_required_fields(
    header: &Component,
    trailer: &Component,
    version: Version,
) -> Result<(), Error> {
    const REQUIRED_IN_ORDER: [(&FixStr, u16, BasicType); 3] = [
        (fix_str!("BeginString"), 8, BasicType::String),
        (fix_str!("BodyLength"), 9, BasicType::Length),
        (fix_str!("MsgType"), 35, BasicType::String),
    ];

    // Fields the standard header must carry somewhere, in no particular order.
    // FIX Session Layer §8.5 marks all four Req'd = Y, while TagValue §4.3.3
    // mandates a position only for the first three fields and CheckSum(10) -
    // "except where noted, fields within a message can be defined in any
    // sequence" - so these are matched by name wherever they sit.
    const REQUIRED_OUT_OF_ORDER: [(&FixStr, u16, BasicType); 4] = [
        (fix_str!("SenderCompID"), 49, BasicType::String),
        (fix_str!("TargetCompID"), 56, BasicType::String),
        (fix_str!("MsgSeqNum"), 34, BasicType::SeqNum),
        (fix_str!("SendingTime"), 52, BasicType::UtcTimestamp),
    ];

    fn check_field(
        member: &Member,
        expected_name: &FixStr,
        expected_tag: u16,
        expected_type: BasicType,
    ) -> Result<(), ValidationError> {
        if !matches!(
            member.definition(),
            MemberDefinition::Field(field)
                if field.name() == expected_name
                    && field.number() == expected_tag
                    && field.data_type() == expected_type)
        {
            Err(ValidationError::InvalidRequiredField(
                expected_name.to_string(),
                expected_tag,
                expected_type,
            ))
        } else if !member.required() {
            Err(ValidationError::OptionalRequiredField(
                expected_name.to_string(),
                expected_tag,
            ))
        } else {
            Ok(())
        }
    }

    if header.members.is_empty() {
        if version.is_fix() && version >= Version::FIX50 {
            // Header must be empty for FIX 5.0 and higher as it is defined in FIXT dictionary
            return Ok(());
        } else {
            return Err(Error::Validation(ValidationError::EmptyContainer(
                "Header".into(),
            )));
        }
    }

    // No length pre-check: the walk below already reports the first required
    // field the header runs out of, as `MissingRequiredField`.
    let mut iter = header.members.iter();

    for (expected_name, expected_tag, expected_type) in REQUIRED_IN_ORDER {
        let Some(member) = iter.next() else {
            return Err(Error::Validation(ValidationError::MissingRequiredField(
                expected_name.to_string(),
                expected_tag,
            )));
        };
        check_field(member, expected_name, expected_tag, expected_type)?;
    }

    let mut required: HashMap<&FixStr, (u16, BasicType)> = REQUIRED_OUT_OF_ORDER
        .iter()
        .map(|&(name, tag, data_type)| (name, (tag, data_type)))
        .collect();

    for member in &header.members {
        let Some((expected_tag, expected_type)) = required.remove(member.name()) else {
            continue;
        };
        check_field(member, member.name(), expected_tag, expected_type)?;
    }

    if let Some((field_name, (field_tag, _))) = required.into_iter().next() {
        return Err(Error::Validation(ValidationError::MissingRequiredField(
            field_name.to_string(),
            field_tag,
        )));
    }

    const CHECKSUM: &FixStr = fix_str!("CheckSum");
    const CHECKSUM_TAG: u16 = 10;
    let checksum = trailer
        .members
        .last()
        .ok_or_else(|| ValidationError::MissingRequiredField(CHECKSUM.to_string(), CHECKSUM_TAG))?;
    check_field(checksum, CHECKSUM, CHECKSUM_TAG, BasicType::String)?;

    Ok(())
}
