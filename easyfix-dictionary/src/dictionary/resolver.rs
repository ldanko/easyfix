use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    vec,
};

use super::{
    Version,
    error::{Error, ValidationError},
    types::{Component, Field, Group, Member, MemberDefinition, Message},
};
use crate::{xml, xml::BasicType};

/// Resolved dictionary elements ready to be consumed by `Dictionary`.
pub(super) struct Elements {
    pub(super) fields: HashMap<String, Rc<Field>>,
    pub(super) components: HashMap<String, Rc<Component>>,
    pub(super) groups: HashMap<String, Rc<Group>>,
}

/// Resolves raw XML definitions into domain types.
///
/// Converts raw XML field, component, and group definitions into their
/// domain-model equivalents, handling deduplication via `Rc`, reference
/// resolution by name, and circular-reference detection.
///
/// This is a transient structure: create it, call its resolve/create
/// methods, then consume via `finish()`.
pub(super) struct Resolver {
    raw_fields: HashMap<String, xml::Field>,
    fields: HashMap<String, Rc<Field>>,
    raw_components: HashMap<String, xml::Component>,
    components: HashMap<String, Rc<Component>>,
    groups: HashMap<String, Rc<Group>>,
}

impl Resolver {
    pub(super) fn new(
        raw_fields: Vec<xml::Field>,
        raw_components: Vec<xml::Component>,
    ) -> Result<Resolver, Error> {
        let mut names: HashSet<String> = HashSet::new();
        let mut raw_fields_map = HashMap::with_capacity(raw_fields.len());
        for field in raw_fields {
            if !names.insert(field.name.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedField(
                    field.name.clone(),
                )));
            }
            raw_fields_map.insert(field.name.clone(), field);
        }

        let mut raw_components_map = HashMap::with_capacity(raw_components.len());
        for comp in raw_components {
            if !names.insert(comp.name.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedComponent(
                    comp.name.clone(),
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

    fn create_field(&mut self, name: &str) -> Result<Rc<Field>, Error> {
        if let Some(field) = self.fields.get(name) {
            return Ok(field.clone());
        }

        let (field_name, raw_field) = self
            .raw_fields
            .remove_entry(name)
            .ok_or_else(|| Error::Validation(ValidationError::UnknownField(name.to_owned())))?;
        let field = Rc::new(Field::from(raw_field));
        self.fields.insert(field_name, field.clone());

        Ok(field)
    }

    fn create_component(
        &mut self,
        name: String,
        visited: &mut HashSet<String>,
    ) -> Result<Rc<Component>, Error> {
        if !visited.insert(name.clone()) {
            return Err(Error::Validation(ValidationError::CircularReference(name)));
        }
        if let Some(component) = self.components.get(&name) {
            return Ok(component.clone());
        }

        let raw_component = self
            .raw_components
            .remove(&name)
            .ok_or_else(|| Error::Validation(ValidationError::UnknownComponent(name.clone())))?;
        if raw_component.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer(name)));
        }

        let mut branch_visited = visited.clone();
        let members =
            self.create_members_impl(raw_component.members, Some(&name), &mut branch_visited)?;

        visited.remove(&name);
        let component = Rc::new(Component { name, members });
        self.components
            .insert(component.name.clone(), component.clone());

        Ok(component)
    }

    /// Creates a group from the XML representation.
    ///
    /// # Group Naming Convention
    ///
    /// XML dictionaries use a specific naming convention for repeating groups:
    /// - The **counter field** is prefixed with "No" (e.g., "NoHops", "NoLegs", "NoPartyIDs")
    /// - The **group name** itself omits the "No" prefix (e.g., "Hops", "Legs", "PartyIDs")
    ///
    /// ## Example from FIXT11.xml:
    /// ```xml
    /// <header>
    ///   <group name='NoHops' required='N'>
    ///     <field name='HopCompID' required='N' />
    ///     <field name='HopSendingTime' required='N' />
    ///     <field name='HopRefID' required='N' />
    ///   </group>
    /// </header>
    /// ```
    ///
    /// This creates:
    /// - A group named **"Hops"** (without "No" prefix)
    /// - With a counter field **"NoHops"** (tag 627, NumInGroup type)
    /// - Containing three fields: HopCompID, HopSendingTime, HopRefID
    ///
    /// ## Special Case: Component-Wrapped Groups
    ///
    /// When a component contains only a single group, the group inherits the component's name:
    /// ```xml
    /// <component name='Parties'>
    ///   <group name='NoPartyIDs' required='N'>
    ///     <field name='PartyID' required='Y'/>
    ///     <field name='PartyRole' required='Y'/>
    ///   </group>
    /// </component>
    /// ```
    /// Here, the group will be named **"Parties"** (from the component), not "PartyIDs".
    fn create_group(
        &mut self,
        raw_group: xml::Group,
        parent_component: Option<&str>,
        visited: &mut HashSet<String>,
    ) -> Result<Rc<Group>, Error> {
        // Determine the group name
        let group_name = if let Some(parent_component) = parent_component {
            // Use parent component name when component contains only this group
            parent_component.to_owned()
        } else if raw_group.name.starts_with("No") {
            // Strip "No" prefix: "NoHops" -> "Hops"
            let group_name = raw_group.name[2..].to_owned();
            if !visited.insert(group_name.clone()) {
                return Err(Error::Validation(ValidationError::CircularReference(
                    group_name,
                )));
            }
            group_name
        } else {
            // Use name as-is if it doesn't follow "No" convention
            let group_name = raw_group.name.clone();
            if !visited.insert(group_name.clone()) {
                return Err(Error::Validation(ValidationError::CircularReference(
                    group_name,
                )));
            }
            group_name
        };

        let mut branch_visited = visited.clone();
        let group = Rc::new(Group {
            num_in_group: self.create_field(&raw_group.name)?,
            members: self.create_members_impl(raw_group.members, None, &mut branch_visited)?,
            name: group_name,
        });

        if self
            .groups
            .insert(group.name.clone(), group.clone())
            .is_some()
        {
            return Err(Error::Validation(ValidationError::DuplicatedGroup(
                group.name.clone(),
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
        parent_component: Option<&str>,
        visited: &mut HashSet<String>,
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
                    // a group — flatten it so consumers see MemberDefinition::Group
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
        parent_component: Option<&str>,
    ) -> Result<Vec<Member>, Error> {
        let mut visited = HashSet::new();
        self.create_members_impl(raw_members, parent_component, &mut visited)
    }

    pub(super) fn create_message(&mut self, msg: xml::Message) -> Result<Message, Error> {
        let members = self.create_members(msg.members, None)?;
        if members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyMessage(msg.name)));
        }

        Ok(Message {
            name: msg.name,
            msg_type: msg.msg_type,
            msg_cat: msg.msg_cat,
            members,
        })
    }

    pub(super) fn check_unused_elements(&self) -> Result<(), Error> {
        // MsgType is skipped because in app dictionaries it may be defined
        // in <fields> but not referenced by any message — the header that uses
        // it is defined separately in the FIXT dictionary.
        if let Some(field) = self.raw_fields.values().find(|f| f.name != "MsgType") {
            Err(Error::Validation(ValidationError::UnusedField(
                field.name.clone(),
                field.number,
            )))
        } else if let Some(component) = self.raw_components.values().next() {
            Err(Error::Validation(ValidationError::UnusedComponent(
                component.name.clone(),
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
                return Err(Error::Validation(ValidationError::EmptyContainer(name)));
            }
            let members = self.create_members(raw_component.members, Some(&name))?;
            let component = Rc::new(Component { name, members });
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
    const REQUIRED_IN_ORDER: [(&str, u16, BasicType); 3] = [
        ("BeginString", 8, BasicType::String),
        ("BodyLength", 9, BasicType::Length),
        ("MsgType", 35, BasicType::String),
    ];

    const REQUIRED_OUT_OF_ORDER: &[(&str, u16, BasicType)] = &[
        ("SenderCompID", 49, BasicType::String),
        ("TargetCompID", 56, BasicType::String),
        ("MsgSeqNum", 35, BasicType::SeqNum),
    ];

    if header.members.is_empty() {
        if version.is_fix() && version >= Version::FIX50 {
            // Header must be empty for FIX 5.0 and higher as it is defined in FIXT dictionary
            return Ok(());
        } else {
            return Err(Error::Validation(ValidationError::EmptyContainer(
                "Header".into(),
            )));
        }
    } else if header.members.len() < REQUIRED_IN_ORDER.len() + REQUIRED_OUT_OF_ORDER.len() {
    }

    let mut iter = header.members.iter();

    for (expected_name, expected_tag, expected_type) in REQUIRED_IN_ORDER {
        let Some(field) = iter.next() else {
            return Err(Error::Validation(ValidationError::UnknownField(
                expected_name.to_string(),
            )));
        };

        if !matches!(
            field.definition(),
            MemberDefinition::Field(field)
                if field.name() == expected_name
                    && field.number() == expected_tag
                    && field.data_type() == expected_type)
        {
            return Err(Error::Validation(ValidationError::InvalidRequiredField(
                expected_name.to_string(),
                expected_tag,
                expected_type,
            )));
        }
    }

    if let Some(checksum) = trailer.members.last() {
        if !matches!(
            checksum.definition(),
            MemberDefinition::Field(field)
                if field.name() == "CheckSum"
                    && field.number() == 10
                    && field.data_type() == BasicType::String)
        {
            return Err(Error::Validation(ValidationError::InvalidRequiredField(
                "CheckSum".to_string(),
                10,
                BasicType::String,
            )));
        }
    }

    Ok(())
}
