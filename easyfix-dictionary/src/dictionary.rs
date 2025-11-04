//! Core dictionary implementation and data structures.
//!
//! This module provides the main implementation of the FIX dictionary, including:
//! - The `Dictionary` struct for accessing fields, components, and messages
//! - The `DictionaryBuilder` for configuring and creating dictionaries
//! - Type definitions for FIX protocol elements (Message, Component, Group, etc.)
//! - Error handling for dictionary operations

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
    str::FromStr,
};

use quick_xml::de::from_str;
use strum_macros::AsRefStr;

use crate::xml;
pub use crate::xml::{BasicType, Field, FixType, MsgCat, MsgType, Value};

#[cfg(test)]
mod tests;

/// Enumeration of standard FIX protocol session reject reasons.
///
/// These values correspond to the standard session-level reject reasons
/// defined in the FIX protocol specification. They are used for validation
/// and error reporting during message parsing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum_macros::EnumIter, AsRefStr, Hash)]
pub enum ParseRejectReason {
    /// The value specified is incorrect for the field
    ValueIsIncorrect,
    /// Tag is specified without a value
    TagSpecifiedWithoutAValue,
    /// The field value has incorrect data format
    IncorrectDataFormatForValue,
    /// The same tag appears more than once in a message
    TagAppearsMoreThanOnce,
    /// Tag appears out of required order in message
    TagSpecifiedOutOfRequiredOrder,
    /// A required tag is missing from the message
    RequiredTagMissing,
    /// Incorrect NumInGroup count for repeating group
    IncorrectNumingroupCountForRepeatingGroup,
    /// Tag is not defined for this message type
    TagNotDefinedForThisMessageType,
    /// Tag is not defined in the FIX specification
    UndefinedTag,
    /// Repeating group fields are out of order
    RepeatingGroupFieldsOutOfOrder,
    /// Invalid tag number used in message
    InvalidTagNumber,
    /// Invalid message type
    InvalidMsgtype,
    /// SendingTime accuracy problem
    SendingtimeAccuracyProblem,
    /// CompID problem (SenderCompID or TargetCompID)
    CompidProblem,
}

/// Errors that can occur during dictionary operations.
///
/// This enum organizes errors into categories based on their source:
/// - I/O and parsing errors
/// - Dictionary validation errors
/// - Builder configuration errors
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input/output error during file operations
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// XML parsing error when reading dictionary files
    #[error("XML parsing error: {0}")]
    XmlParse(#[from] quick_xml::de::DeError),

    /// Dictionary validation failed
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

    /// Dictionary builder configuration error
    #[error("Builder error: {0}")]
    Builder(#[from] BuilderError),
}

/// Errors related to dictionary structure and content validation.
///
/// These errors indicate problems with the dictionary's structure, such as
/// missing references, duplicates, or invalid relationships between elements.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// Referenced field was not found in the dictionary
    #[error("Unknown field {0}")]
    UnknownField(String),

    /// Referenced component was not found in the dictionary
    #[error("Unknown component {0}")]
    UnknownComponent(String),

    /// A field with the same name already exists in the dictionary
    #[error("Duplicated field {0}")]
    DuplicatedField(String),

    /// A component with the same name already exists in the dictionary
    #[error("Duplicated component {0}")]
    DuplicatedComponent(String),

    /// A group with the same name already exists in the dictionary
    #[error("Duplicated group {0}")]
    DuplicatedGroup(String),

    /// A message with the same name already exists in the dictionary
    #[error("Duplicated message name {0}")]
    DuplicatedMessageName(String),

    /// A message with the same type already exists in the dictionary
    #[error("Duplicated message type {0}")]
    DuplicatedMessageType(MsgType),

    /// Component or group has no members defined
    #[error("Component/group {0} has no members")]
    EmptyContainer(String),

    /// Message has no fields or components defined
    #[error("Message {0} has no members")]
    EmptyMessage(String),

    /// Message has an unexpected category for this dictionary type
    #[error("Unexpected message category {0:?} ({1})")]
    UnexpectedMessageCategory(MsgCat, String),

    /// Field was defined in the dictionary but not used in any message, component, or group
    ///
    /// This error only occurs when strict validation is enabled with `with_strict_check(true)`.
    #[error("Unused field {0}({1})")]
    UnusedField(String, u16),

    /// Component was defined in the dictionary but not used in any message or other component
    ///
    /// This error only occurs when strict validation is enabled with `with_strict_check(true)`.
    #[error("Unused component {0}")]
    UnusedComponent(String),

    /// A required standard field in the header/trailer has incorrect properties
    ///
    /// This error occurs when a required FIX field (like BeginString or BodyLength)
    /// has the wrong name, tag number, or data type in the dictionary.
    /// This error only occurs when strict validation is enabled with `with_strict_check(true)`.
    #[error("Invalid required field {0}({1}) [{2:?}]")]
    InvalidRequiredField(String, u16, BasicType),

    /// A circular dependency was detected in component or group references
    ///
    /// This error occurs when components or groups reference each other in a way that
    /// creates an infinite loop. For example, if Component A contains Component B, and
    /// Component B contains Component A, this would create a circular reference.
    #[error("Circular reference found: {0}")]
    CircularReference(String),
}

/// Errors related to dictionary builder configuration.
///
/// These errors indicate problems with how the dictionary builder is being used,
/// such as missing required configuration or incompatible version combinations.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// FIX version not recognized or supported
    #[error("Unknown version {}", .0.begin_string())]
    UnknownVersion(Version),

    /// No dictionary was specified in the builder
    #[error("No dictionary specified")]
    Unspecified,

    /// Incompatible FIX version combinations
    #[error("Incompatible version combination")]
    IncompatibleVersion,
}

/// The shared definition of a field, component, or group.
///
/// This enum represents **what** a member is, separate from **how** it's used.
/// Definitions are wrapped in `Rc<>` to enable sharing across multiple references.
///
/// # Why `Rc<>`?
///
/// In FIX dictionaries, components and groups are typically defined once but
/// referenced many times. For example, a component like "Instrument" or "Parties"
/// might appear in dozens of different message types. Using `Rc<>` allows:
///
/// - **Memory efficiency**: The definition exists in memory only once
/// - **Consistency**: All references see the same definition
/// - **Shared ownership**: Multiple messages can reference the same component
///
/// # Relationship with `Member`
///
/// A `MemberDefinition` represents the **definition** (what it is), while a
/// `Member` combines this definition with **usage context** (whether it's
/// required in a particular message/component).
///
/// ```text
/// Component Definition (1x in memory)
///       ↓
///       ├─→ Member in Message A (required=true)
///       ├─→ Member in Message B (required=false)
///       └─→ Member in Message C (required=true)
/// ```
#[derive(Clone, Debug)]
pub enum MemberDefinition {
    /// A field member (primitive element)
    Field(Rc<Field>),

    /// A component member (reusable collection of fields/components/groups)
    Component(Rc<Component>),

    /// A group member (repeating section of fields/components)
    Group(Rc<Group>),
}

impl MemberDefinition {
    /// Returns the name of this member definition
    ///
    /// The name corresponds to the name of the underlying field, component, or group.
    pub fn name(&self) -> &str {
        match self {
            MemberDefinition::Field(field) => &field.name,
            MemberDefinition::Component(component) => &component.name,
            MemberDefinition::Group(group) => &group.name,
        }
    }
}

/// A member reference within a message, component, or group.
///
/// This struct represents a **usage** of a field, component, or group,
/// which is separate from its **definition**. This separation is important
/// because:
///
/// - **Definitions are shared**: A component like "Instrument" or "Parties"
///   is defined once but may be used in many different messages
/// - **Usage varies**: The same component can be required in one message
///   but optional in another
///
/// # Example
///
/// A component might be required in one context:
/// ```text
/// <message name='NewOrderSingle'>
///   <component name='Instrument' required='Y' />
/// </message>
/// ```
///
/// But optional in another:
/// ```text
/// <message name='OrderCancelRequest'>
///   <component name='Instrument' required='N' />
/// </message>
/// ```
///
/// Both messages share the same component definition (via `Rc`), but each
/// has its own `Member` instance with a different `required` flag.
#[derive(Clone, Debug)]
pub struct Member {
    /// Whether this member is required (mandatory) in its parent container
    required: bool,

    /// The underlying definition of this member
    definition: MemberDefinition,
}

impl Member {
    /// Returns a reference to the underlying definition of this member
    ///
    /// The definition provides access to the field, component, or group that
    /// this member represents.
    pub fn definition(&self) -> &MemberDefinition {
        &self.definition
    }

    /// Returns whether this member is required in its parent container
    ///
    /// Required members must be present in valid FIX messages.
    pub fn required(&self) -> bool {
        self.required
    }

    /// Returns the name of this member
    ///
    /// This is a convenience method that delegates to the underlying definition.
    /// Works for fields, components, and groups.
    pub fn name(&self) -> &str {
        self.definition.name()
    }

    /// Returns this member as a field reference if it is a field
    ///
    /// # Returns
    ///
    /// `Some(&Field)` if this member is a field, `None` otherwise
    pub fn as_field(&self) -> Option<&Field> {
        match &self.definition {
            MemberDefinition::Field(field) => Some(field),
            _ => None,
        }
    }

    /// Returns this member as a component reference if it is a component
    ///
    /// # Returns
    ///
    /// `Some(&Component)` if this member is a component, `None` otherwise
    pub fn as_component(&self) -> Option<&Component> {
        match &self.definition {
            MemberDefinition::Component(component) => Some(component),
            _ => None,
        }
    }

    /// Returns this member as a group reference if it is a group
    ///
    /// # Returns
    ///
    /// `Some(&Group)` if this member is a group, `None` otherwise
    pub fn as_group(&self) -> Option<&Group> {
        match &self.definition {
            MemberDefinition::Group(group) => Some(group),
            _ => None,
        }
    }

    /// Returns true if this member is a field
    pub fn is_field(&self) -> bool {
        matches!(self.definition, MemberDefinition::Field(_))
    }

    /// Returns true if this member is a component
    pub fn is_component(&self) -> bool {
        matches!(self.definition, MemberDefinition::Component(_))
    }

    /// Returns true if this member is a group
    pub fn is_group(&self) -> bool {
        matches!(self.definition, MemberDefinition::Group(_))
    }
}

/// Represents a repeating group in the FIX protocol.
///
/// A group is a collection of fields, components, and/or nested groups
/// that can appear multiple times within a message. Each group has a
/// counter field that specifies how many instances of the group appear.
///
/// # QuickFIX Naming Convention
///
/// In QuickFIX XML dictionaries, groups follow a naming convention:
/// - The counter field uses the "No" prefix: `NoHops`, `NoLegs`, `NoPartyIDs`
/// - The group name omits the prefix: `Hops`, `Legs`, `PartyIDs`
///
/// ## Example
///
/// From the XML:
/// ```xml
/// <group name='NoHops' required='N'>
///   <field name='HopCompID' required='N' />
///   <field name='HopSendingTime' required='N' />
/// </group>
/// ```
///
/// Creates a `Group` with:
/// - `name()` returns `"Hops"`
/// - `num_in_group()` returns the field `"NoHops"` (tag 627)
/// - `members()` contains `HopCompID` and `HopSendingTime`
#[derive(Debug)]
pub struct Group {
    /// Name of the group (without "No" prefix)
    name: String,

    /// The NumInGroup counter field (with "No" prefix)
    num_in_group: Rc<Field>,

    /// Members (fields, components, and nested groups) within this group
    members: Vec<Member>,
}

impl Group {
    /// Returns the name of this group
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a reference to the NumInGroup field that counts instances of this group
    ///
    /// In the FIX protocol, repeating groups start with a field that indicates
    /// how many instances of the group follow.
    pub fn num_in_group(&self) -> &Field {
        &self.num_in_group
    }

    /// Returns all members (fields, components, and nested groups) in this group
    pub fn members(&self) -> &[Member] {
        &self.members
    }
}

/// Represents a reusable component in the FIX protocol.
///
/// Components are reusable collections of fields, groups, and other components
/// that can be included in multiple message types or other components.
#[derive(Debug)]
pub struct Component {
    /// Name of the component
    name: String,

    /// Members (fields, groups, and other components) within this component
    members: Vec<Member>,
}

impl Component {
    /// Returns the name of this component
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns all members (fields, components, and groups) in this component
    pub fn members(&self) -> &[Member] {
        &self.members
    }
}

/// Represents a FIX protocol message definition.
///
/// A message is a complete unit of communication in the FIX protocol,
/// consisting of various fields, components, and groups.
#[derive(Debug)]
pub struct Message {
    /// Human-readable name of the message (e.g., "Heartbeat")
    name: String,

    /// Message type identifier (e.g., "0" for Heartbeat)
    msg_type: MsgType,

    /// Category of the message (Admin or App)
    msg_cat: MsgCat,

    /// Members (fields, components, and groups) that make up this message
    members: Vec<Member>,
}

impl Message {
    /// Returns the human-readable name of this message
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the message type identifier
    ///
    /// The message type is a short code (1-2 characters) that uniquely identifies
    /// this message type in the FIX protocol (e.g., "0" for Heartbeat)
    pub fn msg_type(&self) -> MsgType {
        self.msg_type
    }

    /// Returns the category of this message (Admin or App)
    pub fn msg_cat(&self) -> MsgCat {
        self.msg_cat
    }

    /// Returns all members (fields, components, and groups) in this message
    pub fn members(&self) -> &[Member] {
        &self.members
    }
}

/// Represents a specific version of the FIX protocol.
///
/// FIX versions are identified by a type (FIX or FIXT), major version,
/// minor version, and service pack level. This struct provides constants
/// for all standard FIX protocol versions and methods to work with them.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Version {
    /// The type of FIX protocol (FIX or FIXT)
    fix_type: FixType,

    /// Major version number
    major: u8,

    /// Minor version number
    minor: u8,

    /// Service pack level
    servicepack: u8,
}

impl Version {
    pub const FIX27: Version = Version {
        fix_type: FixType::Fix,
        major: 2,
        minor: 7,
        servicepack: 0,
    };
    pub const FIX30: Version = Version {
        fix_type: FixType::Fix,
        major: 3,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX40: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX41: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 1,
        servicepack: 0,
    };
    pub const FIX42: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 2,
        servicepack: 0,
    };
    pub const FIX43: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 3,
        servicepack: 0,
    };
    pub const FIX44: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 4,
        servicepack: 0,
    };
    pub const FIX50: Version = Version {
        fix_type: FixType::Fix,
        major: 5,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX50SP1: Version = Version {
        fix_type: FixType::Fix,
        major: 5,
        minor: 0,
        servicepack: 1,
    };
    pub const FIX50SP2: Version = Version {
        fix_type: FixType::Fix,
        major: 5,
        minor: 0,
        servicepack: 2,
    };
    pub const FIXT11: Version = Version {
        fix_type: FixType::Fixt,
        major: 1,
        minor: 1,
        servicepack: 0,
    };

    /// Returns a slice containing all known standard FIX protocol versions
    pub const fn known_versions() -> &'static [Version] {
        &[
            Version::FIX27,
            Version::FIX30,
            Version::FIX40,
            Version::FIX41,
            Version::FIX42,
            Version::FIX43,
            Version::FIX44,
            Version::FIX50,
            Version::FIX50SP1,
            Version::FIX50SP2,
            Version::FIXT11,
        ]
    }

    fn from_raw_dictionary(dictionary: &xml::Dictionary) -> Result<Version, Error> {
        let version = Version {
            fix_type: dictionary.fix_type,
            major: dictionary.major,
            minor: dictionary.minor,
            servicepack: dictionary.servicepack,
        };

        if !Version::known_versions().contains(&version) {
            return Err(Error::Builder(BuilderError::UnknownVersion(version)));
        }

        Ok(version)
    }

    /// Returns the type of this FIX version (FIX or FIXT)
    pub const fn fix_type(&self) -> FixType {
        self.fix_type
    }

    /// Returns true if this is a FIX (not FIXT) protocol version
    pub const fn is_fix(&self) -> bool {
        matches!(self.fix_type, FixType::Fix)
    }

    /// Returns true if this is a FIXT protocol version
    pub const fn is_fixt(&self) -> bool {
        matches!(self.fix_type, FixType::Fixt)
    }

    /// Returns the major version number
    pub const fn major(&self) -> u8 {
        self.major
    }

    /// Returns the minor version number
    pub const fn minor(&self) -> u8 {
        self.minor
    }

    /// Returns the service pack level
    pub const fn servicepack(&self) -> u8 {
        self.servicepack
    }

    /// Returns the BeginString representation of this version
    ///
    /// Formats the version as it appears in FIX messages (e.g., "FIX.4.4", "FIXT.1.1", "FIX.5.0SP2").
    pub fn begin_string(&self) -> String {
        if self.servicepack == 0 {
            // Basic format is TYPE.MAJOR.MINOR
            format!("{}.{}.{}", self.fix_type, self.major, self.minor)
        } else {
            // For non-zero servicepack, add SPx suffix
            format!(
                "{}.{}.{}SP{}",
                self.fix_type, self.major, self.minor, self.servicepack
            )
        }
    }
}

impl FromStr for Version {
    type Err = Error;

    /// Parse a BeginString value into a Version
    ///
    /// Accepts strings in the format:
    /// - "FIX.MAJOR.MINOR" (e.g., "FIX.4.4")
    /// - "FIXT.MAJOR.MINOR" (e.g., "FIXT.1.1")
    /// - "FIX.MAJOR.MINORSPx" (e.g., "FIX.5.0SP2")
    ///
    /// # Examples
    ///
    /// ```
    /// use easyfix_dictionary::Version;
    /// use std::str::FromStr;
    ///
    /// let v1 = Version::from_str("FIX.4.4").unwrap();
    /// assert_eq!(v1, Version::FIX44);
    ///
    /// let v2 = Version::from_str("FIXT.1.1").unwrap();
    /// assert_eq!(v2, Version::FIXT11);
    ///
    /// let v3 = Version::from_str("FIX.5.0SP2").unwrap();
    /// assert_eq!(v3, Version::FIX50SP2);
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `Error::Builder(BuilderError::UnknownVersion` if:
    /// - The string format is invalid
    /// - The version numbers cannot be parsed
    /// - The version is not in the list of known FIX versions
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split by '.' to get parts
        let parts: Vec<&str> = s.split('.').collect();

        if parts.len() != 3 {
            // Create a dummy version for the error message
            let dummy = Version {
                fix_type: FixType::Fix,
                major: 0,
                minor: 0,
                servicepack: 0,
            };
            return Err(Error::Builder(BuilderError::UnknownVersion(dummy)));
        }

        // Parse FIX type (FIX or FIXT)
        let fix_type = match parts[0] {
            "FIX" => FixType::Fix,
            "FIXT" => FixType::Fixt,
            _ => {
                let dummy = Version {
                    fix_type: FixType::Fix,
                    major: 0,
                    minor: 0,
                    servicepack: 0,
                };
                return Err(Error::Builder(BuilderError::UnknownVersion(dummy)));
            }
        };

        // Parse major version
        let major = parts[1].parse::<u8>().map_err(|_| {
            Error::Builder(BuilderError::UnknownVersion(Version {
                fix_type,
                major: 0,
                minor: 0,
                servicepack: 0,
            })
        })?;

        // Parse minor version and optional service pack
        let minor_and_sp = parts[2];
        let (minor, servicepack) = if let Some(sp_pos) = minor_and_sp.find("SP") {
            // Has service pack: "0SP2"
            let minor_str = &minor_and_sp[..sp_pos];
            let sp_str = &minor_and_sp[sp_pos + 2..];

            let minor = minor_str.parse::<u8>().map_err(|_| {
                Error::Builder(BuilderError::UnknownVersion(Version {
                    fix_type,
                    major,
                    minor: 0,
                    servicepack: 0,
                })
            })?;

            let sp = sp_str.parse::<u8>().map_err(|_| {
                Error::Builder(BuilderError::UnknownVersion(Version {
                    fix_type,
                    major,
                    minor,
                    servicepack: 0,
                })
            })?;

            (minor, sp)
        } else {
            // No service pack
            let minor = minor_and_sp.parse::<u8>().map_err(|_| {
                Error::Builder(BuilderError::UnknownVersion(Version {
                    fix_type,
                    major,
                    minor: 0,
                    servicepack: 0,
                })
            })?;

            (minor, 0)
        };

        // Create version
        let version = Version {
            fix_type,
            major,
            minor,
            servicepack,
        };

        // Validate it's a known version
        if !Version::known_versions().contains(&version) {
            return Err(Error::Builder(BuilderError::UnknownVersion(version)));
        }

        Ok(version)
    }
}

struct MembersDb {
    raw_fields: HashMap<String, Field>,
    fields: HashMap<String, Rc<Field>>,
    raw_components: HashMap<String, xml::Component>,
    components: HashMap<String, Rc<Component>>,
    groups: HashMap<String, Rc<Group>>,
}

impl MembersDb {
    fn new(
        raw_fields: Vec<xml::Field>,
        raw_components: Vec<xml::Component>,
    ) -> Result<MembersDb, Error> {
        let mut names: HashSet<String> = HashSet::new();
        let mut raw_fields_map = HashMap::with_capacity(raw_fields.len());
        for field in raw_fields {
            if !names.insert(field.name.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedField(field.name.clone())))
            }
            raw_fields_map.insert(field.name.clone(), field);
        }

        let mut raw_components_map = HashMap::with_capacity(raw_components.len());
        for comp in raw_components {
            if !names.insert(comp.name.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedComponent(comp.name.clone())))
            }
            raw_components_map.insert(comp.name.clone(), comp);
        }

        Ok(MembersDb {
            raw_fields: raw_fields_map,
            fields: HashMap::new(),
            raw_components: raw_components_map,
            components: HashMap::new(),
            groups: HashMap::new(),
        })
    }

    fn create_field(&mut self, name: &str) -> Result<Rc<Field>, Error> {
        if let Some(field) = self.fields.get(name) {
            return Ok(field.clone());
        }

        let (field_name, field) = self
            .raw_fields
            .remove_entry(name)
            .ok_or_else(|| Error::Validation(ValidationError::UnknownField(name.to_owned())))?;
        let field = Rc::new(field);
        self.fields.insert(field_name, field.clone());

        Ok(field)
    }

    fn create_component(
        &mut self,
        name: String,
        visited: &mut HashSet<String>,
    ) -> Result<Rc<Component>, Error> {
        if !visited.insert(name.clone()) {
            return Err(Error::Validation(ValidationError::CircularReference(name)))
        }
        if let Some(component) = self.components.get(&name) {
            return Ok(component.clone());
        }

        let raw_component = self
            .raw_components
            .remove(&name)
            .ok_or_else(|| Error::Validation(ValidationError::UnknownComponent(name.clone())))?;
        if raw_component.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer(name)))
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
    /// # QuickFIX Group Naming Convention
    ///
    /// QuickFIX XML dictionaries use a specific naming convention for repeating groups:
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
    ///
    /// # Arguments
    ///
    /// * `raw_group` - The XML group definition
    /// * `parent_component` - Optional component name when the group is wrapped by a single-member component
    /// * `visited` - Set of already visited components/groups for circular reference detection
    ///
    /// # Returns
    ///
    /// Returns `Rc<Group>` on success, or an error if:
    /// - A circular reference is detected
    /// - Referenced fields don't exist
    /// - The group has no members
    fn create_group(
        &mut self,
        raw_group: xml::Group,
        parent_component: Option<&str>,
        visited: &mut HashSet<String>,
    ) -> Result<Rc<Group>, Error> {
        // Determine the group name using QuickFIX convention
        let group_name = if let Some(parent_component) = parent_component {
            // Use parent component name when component contains only this group
            parent_component.to_owned()
        } else if raw_group.name.starts_with("No") {
            // Strip "No" prefix per QuickFIX convention: "NoHops" -> "Hops"
            let group_name = raw_group.name[2..].to_owned();
            if !visited.insert(group_name.clone()) {
                return Err(Error::Validation(ValidationError::CircularReference(group_name)))
            }
            group_name
        } else {
            // Use name as-is if it doesn't follow "No" convention
            let group_name = raw_group.name.clone();
            if !visited.insert(group_name.clone()) {
                return Err(Error::Validation(ValidationError::CircularReference(group_name)))
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
            return Err(Error::Validation(ValidationError::DuplicatedGroup(group.name.clone())))
        }

        visited.remove(&group.name);

        Ok(group)
    }

    // Create members from raw XML members, optionally flattening components
    fn create_members_impl(
        &mut self,
        raw_members: Vec<xml::Member>,
        parent_component: Option<&str>,
        visited: &mut HashSet<String>,
    ) -> Result<Vec<Member>, Error> {
        let raw_members_len = raw_members.len();
        let mut members = Vec::with_capacity(raw_members_len);
        for raw_member in raw_members {
            members.push(self.create_member(
                raw_member,
                // Use component name as group name only when there is only one item inside
                if raw_members_len == 1 {
                    parent_component
                } else {
                    None
                },
                visited,
            )?);
        }
        Ok(members)
    }

    fn create_members(
        &mut self,
        raw_members: Vec<xml::Member>,
        parent_component: Option<&str>,
    ) -> Result<Vec<Member>, Error> {
        let mut visited = HashSet::new();
        self.create_members_impl(raw_members, parent_component, &mut visited)
    }

    fn create_member(
        &mut self,
        member: xml::Member,
        parent_component: Option<&str>,
        visited: &mut HashSet<String>,
    ) -> Result<Member, Error> {
        match member {
            xml::Member::Field(member_ref) => Ok(Member {
                required: member_ref.required,
                definition: MemberDefinition::Field(self.create_field(&member_ref.name)?),
            }),
            xml::Member::Component(member_ref) => Ok(Member {
                required: member_ref.required,
                definition: MemberDefinition::Component(
                    self.create_component(member_ref.name, visited)?,
                ),
            }),
            xml::Member::Group(group) => Ok(Member {
                required: group.required,
                definition: MemberDefinition::Group(self.create_group(
                    group.clone(),
                    parent_component,
                    visited,
                )?),
            }),
        }
    }

    fn create_message(&mut self, msg: xml::Message) -> Result<Message, Error> {
        let members = self.create_members(msg.members, None)?;
        if members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyMessage(msg.name)))
        }

        Ok(Message {
            name: msg.name,
            msg_type: msg.msg_type,
            msg_cat: msg.msg_cat,
            members,
        })
    }

    fn check_unused_elements(&self) -> Result<(), Error> {
        if let Some(field) = self.raw_fields.values().next() {
            Err(Error::Validation(ValidationError::UnusedField(field.name.clone(), field.number)))
        } else if let Some(component) = self.raw_components.values().next() {
            Err(Error::Validation(ValidationError::UnusedComponent(component.name.clone())))
        } else {
            Ok(())
        }
    }

    fn register_unused_elements(&mut self) -> Result<(), Error> {
        for (field_name, field) in self.raw_fields.drain() {
            let field = Rc::new(field);
            self.fields.insert(field_name, field.clone());
        }

        let mut raw_components = std::mem::take(&mut self.raw_components);
        for (name, raw_component) in raw_components.drain() {
            if raw_component.members.is_empty() {
                return Err(Error::Validation(ValidationError::EmptyContainer(name)))
            }
            let members = self.create_members(raw_component.members, Some(&name))?;
            let component = Rc::new(Component { name, members });
            self.components
                .insert(component.name.clone(), component.clone());
        }

        Ok(())
    }
}

/// Builder for creating and configuring FIX dictionaries.
///
/// This builder provides a fluent interface for configuring and creating
/// a FIX protocol dictionary from XML specification files.
pub struct DictionaryBuilder {
    /// Path to the FIXT (transport) XML specification
    fixt_xml_path: Option<PathBuf>,

    /// Paths to FIX application-level XML specifications
    fix_xml_paths: Vec<PathBuf>,

    /// Custom text for rejection reasons
    custom_rejection_reasons: HashMap<ParseRejectReason, String>,

    /// Allow custom FIX versions not in the standard list
    allow_custom_version: bool,

    /// Apply strict validation during parsing
    strict_check: bool,

    /// Whether to flatten component hierarchies
    flatten_components: bool,
}

fn read_raw_dictionary(path: &Path) -> Result<xml::Dictionary, Error> {
    let xml = fs::read_to_string(path)?;
    Ok(from_str::<xml::Dictionary>(&xml)?)
}

fn read_raw_fixt_dictionary(path: &Path) -> Result<xml::Dictionary, Error> {
    let raw_dictionary = read_raw_dictionary(path)?;
    let version = Version::from_raw_dictionary(&raw_dictionary)?;

    if !version.is_fixt() || version < Version::FIXT11 {
        return Err(Error::Builder(BuilderError::IncompatibleVersion));
    }
    if raw_dictionary.header.members.is_empty() {
        return Err(Error::Validation(ValidationError::EmptyContainer("Header".into())))
    }
    if raw_dictionary.trailer.members.is_empty() {
        return Err(Error::Validation(ValidationError::EmptyContainer("Trailer".into())))
    }
    if let Some(msg) = raw_dictionary
        .messages
        .iter()
        .find(|msg| !matches!(msg.msg_cat, MsgCat::Admin))
    {
        return Err(Error::Validation(ValidationError::UnexpectedMessageCategory(
            msg.msg_cat,
            msg.name.clone(),
        ));
    }

    Ok(raw_dictionary)
}

fn read_raw_fix_dictionary(path: &Path) -> Result<xml::Dictionary, Error> {
    let raw_dictionary = read_raw_dictionary(path)?;
    let version = Version::from_raw_dictionary(&raw_dictionary)?;

    if !version.is_fix() {
        return Err(Error::Builder(BuilderError::IncompatibleVersion));
    } else if version >= Version::FIX50 {
        if !raw_dictionary.header.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer("Header".into())))
        }
        if !raw_dictionary.trailer.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer("Trailer".into())))
        }
        if let Some(msg) = raw_dictionary
            .messages
            .iter()
            .find(|msg| !matches!(msg.msg_cat, MsgCat::App))
        {
            return Err(Error::Validation(ValidationError::UnexpectedMessageCategory(
                msg.msg_cat,
                msg.name.clone(),
            ));
        }
    } else {
        if raw_dictionary.header.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer("Header".into())))
        }
        if raw_dictionary.trailer.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer("Trailer".into())))
        }
    }

    Ok(raw_dictionary)
}

impl Default for DictionaryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DictionaryBuilder {
    /// Creates a new, empty DictionaryBuilder with default settings
    ///
    /// Use the builder's methods to configure and then call `build()` to create
    /// the dictionary.
    pub fn new() -> DictionaryBuilder {
        DictionaryBuilder {
            fixt_xml_path: None,
            fix_xml_paths: Vec::new(),
            custom_rejection_reasons: HashMap::new(),
            allow_custom_version: false,
            strict_check: false,
            flatten_components: false,
        }
    }

    /// Sets custom text for rejection reasons
    ///
    /// This allows overriding the standard text for session-level reject reasons.
    pub fn with_custom_rejection_reason(
        mut self,
        custom_rejection_reasons: HashMap<ParseRejectReason, String>,
    ) -> Self {
        self.custom_rejection_reasons = custom_rejection_reasons;
        self
    }

    /// Sets whether to allow custom FIX versions not in the standard list
    ///
    /// By default, only standard FIX versions are accepted.
    pub fn allow_custom_version(mut self, allow_custom_version: bool) -> Self {
        self.allow_custom_version = allow_custom_version;
        self
    }

    /// Sets whether to apply strict validation during dictionary parsing
    ///
    /// When enabled, more rigorous checks are applied to the dictionary structure.
    pub fn with_strict_check(mut self, strict_check: bool) -> Self {
        self.strict_check = strict_check;
        self
    }

    /// Adds a FIX application-level XML specification file to the builder
    ///
    /// For FIX versions prior to 5.0, this is the only XML file needed.
    /// For FIX 5.0+, this should be used along with `with_fixt_xml()`.
    pub fn with_fix_xml(mut self, path: impl Into<PathBuf>) -> Self {
        self.fix_xml_paths.push(path.into());
        self
    }

    /// Adds multiple FIX application-level XML specification files to the builder
    ///
    /// This is useful when working with multiple FIX versions or custom extensions.
    pub fn with_fix_xmls(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.fix_xml_paths.extend(paths);
        self
    }

    /// Sets the FIXT transport layer XML specification file
    ///
    /// This is required for FIX 5.0+ versions, which separate transport (FIXT)
    /// from application (FIX) layer specifications.
    pub fn with_fixt_xml(mut self, path: impl Into<PathBuf>) -> Self {
        self.fixt_xml_path = Some(path.into());
        self
    }

    /// Sets whether to flatten component hierarchies
    ///
    /// When enabled, nested components are flattened into their parent containers.
    /// This simplifies the structure by removing intermediate component layers,
    /// resulting in direct field references in messages and groups.
    ///
    /// For example, if Message A contains Component B which contains Field C,
    /// flattening would make Message A directly contain Field C.
    ///
    /// This is useful when you want to simplify the structure and reduce indirection,
    /// particularly for code generation or processing that works better with
    /// flattened structures.
    pub fn flatten_components(mut self, flatten_components: bool) -> Self {
        self.flatten_components = flatten_components;
        self
    }

    /// Builds the Dictionary from the configured sources
    ///
    /// This method parses the XML files and constructs a complete FIX dictionary.
    /// It will return an error if the dictionary configuration is invalid or if
    /// parsing fails.
    pub fn build(self) -> Result<Dictionary, Error> {
        match (self.fixt_xml_path, self.fix_xml_paths.as_slice()) {
            (None, []) => Err(Error::Builder(BuilderError::Unspecified)),
            (None, [fix_xml_path]) => {
                // Legacy FIX version
                let dict = read_raw_fix_dictionary(fix_xml_path)?;
                let mut dictionary = Dictionary::from_raw_dictionary(
                    dict,
                    self.flatten_components,
                    self.strict_check,
                )?;
                // Apply custom rejection reasons
                dictionary.reject_reason_overrides = self.custom_rejection_reasons;
                if self.flatten_components {
                    Ok(dictionary.flatten()?)
                } else {
                    Ok(dictionary)
                }
            }
            (None, [_, ..]) => Err(Error::Builder(BuilderError::IncompatibleVersion)),
            (Some(fixt_xml_path), fix_xml_paths) => {
                let fixt = read_raw_fixt_dictionary(&fixt_xml_path)?;
                let mut fixt_dict = Dictionary::from_raw_dictionary(
                    fixt,
                    self.flatten_components,
                    self.strict_check,
                )?;

                for fix_xml_path in fix_xml_paths {
                    let fix = read_raw_fix_dictionary(fix_xml_path)?;
                    if fix.major < 5 {
                        return Err(Error::Builder(BuilderError::IncompatibleVersion));
                    }
                    let mut subdict = Dictionary::from_raw_dictionary(
                        fix,
                        self.flatten_components,
                        self.strict_check,
                    )?;
                    // Apply custom rejection reasons to subdictionaries
                    subdict.reject_reason_overrides = self.custom_rejection_reasons.clone();
                    fixt_dict.subdictionaries.insert(subdict.version, subdict);
                }

                // Apply custom rejection reasons
                fixt_dict.reject_reason_overrides = self.custom_rejection_reasons;
                if self.flatten_components {
                    Ok(fixt_dict.flatten()?)
                } else {
                    Ok(fixt_dict)
                }
            }
        }
    }
}

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

    /// Messages indexed by name
    messages_by_name: HashMap<String, Rc<Message>>,

    /// Messages indexed by message type
    messages_by_id: HashMap<MsgType, Rc<Message>>,

    /// Standard header component definition
    header: Component,

    /// Standard trailer component definition
    trailer: Component,

    /// Custom text overrides for reject reasons
    reject_reason_overrides: HashMap<ParseRejectReason, String>,

    /// Application-level subdictionaries (for FIXT-based versions)
    subdictionaries: HashMap<Version, Dictionary>,
}

fn check_required_fields(
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
        } else {
            return Err(Error::Validation(ValidationError::EmptyContainer("Header".into())))
        }
    } else if header.members.len() < REQUIRED_IN_ORDER.len() + REQUIRED_OUT_OF_ORDER.len() {
    }

    let mut iter = header.members.iter();

    for (expected_name, expected_tag, expected_type) in REQUIRED_IN_ORDER {
        let Some(field) = iter.next() else {
            return Err(Error::Validation(ValidationError::UnknownField(expected_name.to_string()));
        };

        if !matches!(
            field.definition(),
            MemberDefinition::Field(field)
                if field.name == expected_name
                    && field.number == expected_tag
                    && field.data_type == expected_type)
        {
            return Err(Error::Validation(ValidationError::InvalidRequiredField(
                expected_name.to_string(),
                expected_tag,
                expected_type,
            ));
        }
    }

    if let Some(checksum) = trailer.members.last() {
        if !matches!(
            checksum.definition(),
            MemberDefinition::Field(field)
                if field.name == "CheckSum"
                    && field.number == 10
                    && field.data_type == BasicType::String)
        {
            return Err(Error::Validation(ValidationError::InvalidRequiredField(
                "CheckSum".to_string(),
                10,
                BasicType::String,
            ));
        }
    }

    Ok(())
}

impl Dictionary {
    fn from_raw_dictionary(
        raw_dictionary: xml::Dictionary,
        _flatten: bool,
        strict_check: bool,
    ) -> Result<Dictionary, Error> {
        let version = Version::from_raw_dictionary(&raw_dictionary)?;
        let mut member_db = MembersDb::new(raw_dictionary.fields, raw_dictionary.components)?;

        let header = Component {
            name: "Header".into(),
            members: member_db.create_members(raw_dictionary.header.members, None)?,
        };
        let trailer = Component {
            name: "Trailer".into(),
            members: member_db.create_members(raw_dictionary.trailer.members, None)?,
        };
        if strict_check {
            check_required_fields(&header, &trailer, version)?;
        }

        let mut messages_by_name = HashMap::with_capacity(raw_dictionary.messages.len());
        let mut messages_by_id = HashMap::with_capacity(raw_dictionary.messages.len());
        for raw_msg in raw_dictionary.messages {
            let msg = Rc::new(member_db.create_message(raw_msg)?);
            if let Some(msg) = messages_by_name.insert(msg.name.clone(), msg.clone()) {
                return Err(Error::Validation(ValidationError::DuplicatedMessageName(msg.name.clone())))
            }
            if let Some(msg) = messages_by_id.insert(msg.msg_type, msg) {
                return Err(Error::Validation(ValidationError::DuplicatedMessageType(msg.msg_type)))
            }
        }

        if strict_check {
            member_db.check_unused_elements()?;
        } else {
            member_db.register_unused_elements()?;
        }

        let fields_by_id = member_db
            .fields
            .values()
            .map(|field| (field.number, field.clone()))
            .collect();

        Ok(Dictionary {
            version,
            fields_by_name: member_db.fields,
            fields_by_id,
            groups: member_db.groups,
            components: member_db.components,
            messages_by_name,
            messages_by_id,
            header,
            trailer,
            reject_reason_overrides: HashMap::new(),
            subdictionaries: HashMap::new(),
        })
    }

    /// Creates a new Dictionary by parsing a single XML specification file
    ///
    /// This is a convenience method for simple cases. For more complex scenarios,
    /// use the `DictionaryBuilder` instead.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the XML specification file
    ///
    /// # Returns
    ///
    /// A Result containing either the parsed Dictionary or an Error
    pub fn new(path: &str) -> Result<Dictionary, Error> {
        let xml = fs::read_to_string(path)?;
        let raw_dictionary: xml::Dictionary = from_str(&xml)?;

        Dictionary::from_raw_dictionary(raw_dictionary, false, false)
    }

    fn flatten_component(
        component_name: &str,
        parent_required: bool,
        output: &mut Vec<Member>,
        components_map: &HashMap<String, Rc<Component>>,
    ) -> Result<(), Error> {
        // Get the component
        let component = components_map
            .get(component_name)
            .ok_or_else(|| Error::Validation(ValidationError::UnknownComponent(component_name.to_owned()))?;

        // Process each member of the component
        for member in &component.members {
            match member.definition() {
                // If the member is itself a component, recursively flatten it
                MemberDefinition::Component(nested_comp) => {
                    // Calculate combined required flag - a member is required only if both
                    // the parent component AND the member itself are required
                    let required = parent_required && member.required();

                    // Recursively flatten this component
                    Self::flatten_component(&nested_comp.name, required, output, components_map)?;
                }

                // If it's a field, add it directly with combined required flag
                MemberDefinition::Field(field) => {
                    // Calculate combined required flag
                    let required = parent_required && member.required();

                    output.push(Member {
                        required,
                        definition: MemberDefinition::Field(field.clone()),
                    });
                }

                // If it's a group, we need to flatten any components inside the group
                MemberDefinition::Group(group) => {
                    let required = parent_required && member.required();
                    let flattened_group = Self::flatten_group(group, components_map)?;

                    // The group stays as a group, but we need to process its members
                    // to flatten components inside it
                    output.push(Member {
                        required,
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
                        member.required(),
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
        // Create copies of messages with flattened components
        let mut new_messages_by_name = HashMap::with_capacity(self.messages_by_name.len());
        let mut new_messages_by_id = HashMap::with_capacity(self.messages_by_id.len());

        for (name, msg) in &self.messages_by_name {
            let flattened_members = Self::flatten_members(msg.members(), &self.components)?;
            let flattened_msg = Rc::new(Message {
                name: msg.name.clone(),
                msg_type: msg.msg_type,
                msg_cat: msg.msg_cat,
                members: flattened_members,
            });

            new_messages_by_name.insert(name.clone(), flattened_msg.clone());
            new_messages_by_id.insert(msg.msg_type, flattened_msg);
        }

        // Flatten the header and trailer
        let flattened_header_members =
            Self::flatten_members(&self.header.members, &self.components)?;
        let flattened_trailer_members =
            Self::flatten_members(&self.trailer.members, &self.components)?;

        // Create new groups map with flattened groups
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
            messages_by_name: new_messages_by_name,
            messages_by_id: new_messages_by_id,
            header,
            trailer,
            reject_reason_overrides: self.reject_reason_overrides.clone(),
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
    ///
    /// # Arguments
    ///
    /// * `name` - The field name to look up (e.g., "BeginString")
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the Field if found
    pub fn field_by_name(&self, name: &str) -> Option<&Field> {
        self.fields_by_name.get(name).map(|v| &**v)
    }

    /// Looks up a field by tag number
    ///
    /// # Arguments
    ///
    /// * `id` - The field tag number to look up (e.g., 8 for BeginString)
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the Field if found
    pub fn field_by_id(&self, id: u16) -> Option<&Field> {
        self.fields_by_id.get(&id).map(|v| &**v)
    }

    /// Returns an iterator over all fields in this dictionary
    pub fn fields(&self) -> impl Iterator<Item = &Field> {
        self.fields_by_id.values().map(|v| &**v)
    }

    /// Looks up a component by name
    ///
    /// # Arguments
    ///
    /// * `name` - The component name to look up
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the Component if found
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.get(name).map(|v| &**v)
    }

    /// Returns an iterator over all components in this dictionary
    pub fn components(&self) -> impl Iterator<Item = &Component> {
        self.components.values().map(|v| &**v)
    }

    /// Looks up a group by name
    ///
    /// # Arguments
    ///
    /// * `name` - The group name to look up
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the Group if found
    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.get(name).map(|v| &**v)
    }

    /// Returns an iterator over all groups in this dictionary
    pub fn groups(&self) -> impl Iterator<Item = &Group> {
        self.groups.values().map(|v| &**v)
    }

    /// Looks up a message by name
    ///
    /// # Arguments
    ///
    /// * `name` - The message name to look up (e.g., "Heartbeat")
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the Message if found
    pub fn message_by_name(&self, name: &str) -> Option<&Message> {
        self.messages_by_name.get(name).map(|v| &**v)
    }

    /// Looks up a message by message type
    ///
    /// # Arguments
    ///
    /// * `msg_type` - The message type to look up (e.g., b"0" for Heartbeat, b"8" for ExecutionReport)
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the Message if found
    pub fn message_by_type(&self, msg_type: &[u8]) -> Option<&Message> {
        self.messages_by_id.get(msg_type).map(|v| &**v)
    }

    /// Returns an iterator over all messages in this dictionary
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.messages_by_id.values().map(|v| &**v)
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
    ///
    /// # Arguments
    ///
    /// * `version` - The version of the subdictionary to retrieve
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the subdictionary if found
    pub fn subdictionary(&self, version: Version) -> Option<&Dictionary> {
        self.subdictionaries.get(&version)
    }

    /// Returns custom text overrides for rejection reasons
    pub fn reject_reason_overrides(&self) -> &HashMap<ParseRejectReason, String> {
        &self.reject_reason_overrides
    }
}
