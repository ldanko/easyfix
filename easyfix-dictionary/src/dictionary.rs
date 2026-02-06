//! Core dictionary implementation and data structures.
//!
//! This module provides the main implementation of the FIX dictionary, including:
//! - The `Dictionary` struct for accessing fields, components, and messages
//! - The `DictionaryBuilder` for configuring and creating dictionaries
//! - Type definitions for FIX protocol elements (Message, Component, Group, etc.)
//! - FIX protocol types (`MsgType`, `MsgCat`, etc.)
//! - Error handling for dictionary operations

mod builder;
mod error;
mod resolver;
mod types;
mod version;

#[cfg(test)]
mod tests;

use std::{collections::HashMap, fs, rc::Rc};

use quick_xml::de::from_str;

use self::resolver::{Elements, Resolver, check_required_fields};
pub use self::{
    builder::DictionaryBuilder,
    error::{BuilderError, Error, ValidationError},
    types::{Component, Field, Group, Member, MemberDefinition, Message, Variant},
    version::Version,
};
use crate::xml;
pub use crate::xml::{BasicType, FixType, MsgCat, MsgType};

/// The main dictionary representing a FIX protocol specification.
///
/// A `Dictionary` provides access to all elements of a FIX protocol
/// specification, including fields, components, messages, and version information.
/// For modern FIX versions (FIXT/FIX5+), it may also contain subdictionaries
/// for different message categories.
#[derive(Debug)]
pub struct Dictionary {
    /// The version of this dictionary
    version: Version,

    /// Fields indexed by name
    fields_by_name: HashMap<String, Rc<Field>>,

    /// Fields indexed by ID (tag number)
    fields_by_id: HashMap<u16, Rc<Field>>,

    /// Groups indexed by name
    groups: HashMap<String, Rc<Group>>,

    /// Components indexed by name
    components: HashMap<String, Rc<Component>>,

    /// Messages in definition order (from XML)
    messages: Vec<Rc<Message>>,

    /// Messages indexed by name
    messages_by_name: HashMap<String, Rc<Message>>,

    /// Messages indexed by message type
    messages_by_id: HashMap<MsgType, Rc<Message>>,

    /// Standard header component definition
    header: Component,

    /// Standard trailer component definition
    trailer: Component,

    /// Application-level subdictionaries (for FIXT-based versions)
    subdictionaries: HashMap<Version, Dictionary>,
}

impl Dictionary {
    pub(crate) fn from_raw_dictionary(
        raw_dictionary: xml::Dictionary,
        _flatten: bool,
        strict_check: bool,
    ) -> Result<Dictionary, Error> {
        let version = Version::from_raw_dictionary(&raw_dictionary)?;
        let mut resolver = Resolver::new(raw_dictionary.fields, raw_dictionary.components)?;

        let header = Component {
            name: "Header".into(),
            members: resolver.create_members(raw_dictionary.header.members, None)?,
        };
        let trailer = Component {
            name: "Trailer".into(),
            members: resolver.create_members(raw_dictionary.trailer.members, None)?,
        };
        if strict_check {
            check_required_fields(&header, &trailer, version)?;
        }

        let mut messages = Vec::with_capacity(raw_dictionary.messages.len());
        let mut messages_by_name = HashMap::with_capacity(raw_dictionary.messages.len());
        let mut messages_by_id = HashMap::with_capacity(raw_dictionary.messages.len());
        for raw_msg in raw_dictionary.messages {
            let msg = Rc::new(resolver.create_message(raw_msg)?);
            if let Some(msg) = messages_by_name.insert(msg.name.clone(), msg.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedMessageName(
                    msg.name.clone(),
                )));
            }
            if let Some(msg) = messages_by_id.insert(msg.msg_type, msg.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedMessageType(
                    msg.msg_type,
                )));
            }
            messages.push(msg);
        }

        if strict_check {
            resolver.check_unused_elements()?;
        } else {
            resolver.register_unused_elements()?;
        }

        let Elements {
            fields: fields_by_name,
            components,
            groups,
        } = resolver.into_elements();

        let fields_by_id = fields_by_name
            .values()
            .map(|field| (field.number(), field.clone()))
            .collect();

        Ok(Dictionary {
            version,
            fields_by_name,
            fields_by_id,
            groups,
            components,
            messages,
            messages_by_name,
            messages_by_id,
            header,
            trailer,
            subdictionaries: HashMap::new(),
        })
    }

    /// Creates a new Dictionary by parsing a single XML specification file
    ///
    /// This is a convenience method for simple cases. For more complex scenarios,
    /// use the `DictionaryBuilder` instead.
    pub fn new(path: &str) -> Result<Dictionary, Error> {
        let xml = fs::read_to_string(path)?;
        let raw_dictionary: xml::Dictionary = from_str(&xml)?;

        Dictionary::from_raw_dictionary(raw_dictionary, false, false)
    }

    fn flatten_component(
        component_name: &str,
        output: &mut Vec<Member>,
        components_map: &HashMap<String, Rc<Component>>,
    ) -> Result<(), Error> {
        // Get the component
        let component = components_map.get(component_name).ok_or_else(|| {
            Error::Validation(ValidationError::UnknownComponent(component_name.to_owned()))
        })?;

        // Inline each member of the component with its own required flag.
        // The component's usage-site required flag is not used — components
        // are just grouping containers that disappear after flattening.
        for member in &component.members {
            match member.definition() {
                MemberDefinition::Component(nested_comp) => {
                    Self::flatten_component(&nested_comp.name, output, components_map)?;
                }

                MemberDefinition::Field(field) => {
                    output.push(Member {
                        required: member.required(),
                        definition: MemberDefinition::Field(field.clone()),
                    });
                }

                MemberDefinition::RawData { length, data } => {
                    output.push(Member {
                        required: member.required(),
                        definition: MemberDefinition::RawData {
                            length: length.clone(),
                            data: data.clone(),
                        },
                    });
                }

                MemberDefinition::Group(group) => {
                    let flattened_group = Self::flatten_group(group, components_map)?;

                    output.push(Member {
                        required: member.required(),
                        definition: MemberDefinition::Group(flattened_group),
                    });
                }
            }
        }

        Ok(())
    }

    // Helper function to flatten a list of members
    fn flatten_members(
        members: &[Member],
        components_map: &HashMap<String, Rc<Component>>,
    ) -> Result<Vec<Member>, Error> {
        let mut flattened_members = Vec::new();

        for member in members {
            match member.definition() {
                MemberDefinition::Component(component) => {
                    Self::flatten_component(
                        &component.name,
                        &mut flattened_members,
                        components_map,
                    )?;
                }

                MemberDefinition::Field(field) => {
                    flattened_members.push(Member {
                        required: member.required(),
                        definition: MemberDefinition::Field(field.clone()),
                    });
                }

                MemberDefinition::RawData { length, data } => {
                    flattened_members.push(Member {
                        required: member.required(),
                        definition: MemberDefinition::RawData {
                            length: length.clone(),
                            data: data.clone(),
                        },
                    });
                }

                MemberDefinition::Group(group) => {
                    let flattened_group = Self::flatten_group(group, components_map)?;

                    // Add the flattened group to the output
                    flattened_members.push(Member {
                        required: member.required(),
                        definition: MemberDefinition::Group(flattened_group),
                    });
                }
            }
        }

        Ok(flattened_members)
    }

    // Helper function to flatten a group
    fn flatten_group(
        group: &Rc<Group>,
        components_map: &HashMap<String, Rc<Component>>,
    ) -> Result<Rc<Group>, Error> {
        let flattened_members = Self::flatten_members(group.members(), components_map)?;

        // Create a new group with the flattened members
        let flattened_group = Rc::new(Group {
            name: group.name.clone(),
            num_in_group: group.num_in_group.clone(),
            members: flattened_members,
        });

        Ok(flattened_group)
    }

    pub fn flatten(&self) -> Result<Dictionary, Error> {
        // Create copies of messages with flattened components, preserving definition order
        let mut new_messages = Vec::with_capacity(self.messages.len());
        let mut new_messages_by_name = HashMap::with_capacity(self.messages_by_name.len());
        let mut new_messages_by_id = HashMap::with_capacity(self.messages_by_id.len());

        for msg in &self.messages {
            let flattened_members = Self::flatten_members(msg.members(), &self.components)?;
            let flattened_msg = Rc::new(Message {
                name: msg.name.clone(),
                msg_type: msg.msg_type,
                msg_cat: msg.msg_cat,
                members: flattened_members,
            });

            new_messages.push(flattened_msg.clone());
            new_messages_by_name.insert(msg.name.clone(), flattened_msg.clone());
            new_messages_by_id.insert(msg.msg_type, flattened_msg);
        }

        // Flatten the header and trailer
        let flattened_header_members =
            Self::flatten_members(&self.header.members, &self.components)?;
        let flattened_trailer_members =
            Self::flatten_members(&self.trailer.members, &self.components)?;

        let mut new_groups = HashMap::with_capacity(self.groups.len());
        for (name, group) in &self.groups {
            new_groups.insert(name.clone(), Self::flatten_group(group, &self.components)?);
        }

        let header = Component {
            name: self.header.name.clone(),
            members: flattened_header_members,
        };
        let trailer = Component {
            name: self.trailer.name.clone(),
            members: flattened_trailer_members,
        };

        let mut subdictionaries = HashMap::with_capacity(self.subdictionaries.len());
        for (version, subdictionary) in &self.subdictionaries {
            subdictionaries.insert(*version, subdictionary.flatten()?);
        }

        Ok(Dictionary {
            version: self.version,
            fields_by_name: self.fields_by_name.clone(),
            fields_by_id: self.fields_by_id.clone(),
            groups: new_groups,
            components: HashMap::new(),
            messages: new_messages,
            messages_by_name: new_messages_by_name,
            messages_by_id: new_messages_by_id,
            header,
            trailer,
            subdictionaries,
        })
    }

    /// Returns the version of this dictionary
    pub fn version(&self) -> Version {
        self.version
    }

    /// Returns the BeginString representation of this dictionary's version
    ///
    /// This is a convenience method that delegates to `version().begin_string()`.
    pub fn begin_string(&self) -> String {
        self.version.begin_string()
    }

    /// Looks up a field by name
    pub fn field_by_name(&self, name: &str) -> Option<&Field> {
        self.fields_by_name.get(name).map(|v| &**v)
    }

    /// Looks up a field by tag number
    pub fn field_by_id(&self, id: u16) -> Option<&Field> {
        self.fields_by_id.get(&id).map(|v| &**v)
    }

    /// Returns an iterator over all fields in this dictionary
    pub fn fields(&self) -> impl Iterator<Item = &Field> {
        self.fields_by_id.values().map(|v| &**v)
    }

    /// Looks up a component by name
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.get(name).map(|v| &**v)
    }

    /// Returns an iterator over all components in this dictionary
    pub fn components(&self) -> impl Iterator<Item = &Component> {
        self.components.values().map(|v| &**v)
    }

    /// Looks up a group by name
    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.get(name).map(|v| &**v)
    }

    /// Returns an iterator over all groups in this dictionary
    pub fn groups(&self) -> impl Iterator<Item = &Group> {
        self.groups.values().map(|v| &**v)
    }

    /// Looks up a message by name
    pub fn message_by_name(&self, name: &str) -> Option<&Message> {
        self.messages_by_name.get(name).map(|v| &**v)
    }

    /// Looks up a message by message type
    pub fn message_by_type(&self, msg_type: &[u8]) -> Option<&Message> {
        self.messages_by_id.get(msg_type).map(|v| &**v)
    }

    /// Returns an iterator over all messages in this dictionary, in definition order
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.messages.iter().map(|v| &**v)
    }

    /// Returns a reference to the standard header component
    ///
    /// The header component contains fields that appear at the beginning of every message.
    pub fn header(&self) -> &Component {
        &self.header
    }

    /// Returns a reference to the standard trailer component
    ///
    /// The trailer component contains fields that appear at the end of every message.
    pub fn trailer(&self) -> &Component {
        &self.trailer
    }

    /// Returns a subdictionary for the specified version, if available
    ///
    /// For FIXT-based FIX versions, this allows access to application-level dictionaries.
    pub fn subdictionary(&self, version: Version) -> Option<&Dictionary> {
        self.subdictionaries.get(&version)
    }

    /// Returns an iterator over all subdictionaries
    ///
    /// For FIXT-based FIX versions, this provides access to application-level dictionaries.
    pub fn subdictionaries(&self) -> impl Iterator<Item = &Dictionary> {
        self.subdictionaries.values()
    }
}
