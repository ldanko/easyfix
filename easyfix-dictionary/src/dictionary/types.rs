use std::rc::Rc;

use crate::{
    xml,
    xml::{BasicType, MsgCat, MsgType},
};

/// An enumerated value variant for a FIX field.
///
/// Some FIX fields have a predefined set of valid values, each with a
/// specific meaning. For example, the `Side` field (tag 54) has variants
/// like `1` (Buy) and `2` (Sell).
#[derive(Clone, Debug)]
pub struct Variant {
    /// Name of this variant (e.g., "Buy", "Sell")
    name: String,
    /// The raw value from the field definition (e.g., "1", "2")
    value: String,
}

impl Variant {
    /// Returns the name of this variant
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the raw value of this variant
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl From<xml::Value> for Variant {
    fn from(v: xml::Value) -> Self {
        Variant {
            name: v.description,
            value: v.value_enum,
        }
    }
}

/// A field definition in the FIX dictionary.
///
/// Fields are the basic elements of FIX messages, representing individual
/// data points with specific types and possible enumerated variants.
#[derive(Clone, Debug)]
pub struct Field {
    /// The tag number that identifies this field
    pub(super) number: u16,
    /// The human-readable name of this field
    pub(super) name: String,
    /// The data type of this field
    pub(super) data_type: BasicType,
    /// Enumerated variants for this field
    pub(super) variants: Vec<Variant>,
}

impl Field {
    /// Returns the tag number that identifies this field
    pub fn number(&self) -> u16 {
        self.number
    }

    /// Returns the name of this field
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the data type of this field
    pub fn data_type(&self) -> BasicType {
        self.data_type
    }

    /// Returns the enumerated variants for this field
    pub fn variants(&self) -> &[Variant] {
        &self.variants
    }
}

impl From<xml::Field> for Field {
    fn from(f: xml::Field) -> Self {
        Field {
            number: f.number,
            name: f.name,
            data_type: f.data_type,
            variants: f
                .values
                .unwrap_or_default()
                .into_iter()
                .map(Variant::from)
                .collect(),
        }
    }
}

/// The shared definition of a field, component, or group.
///
/// This enum represents **what** a member is, separate from **how** it's used.
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

// # Why `Rc<>`?
//
// In FIX dictionaries, components and groups are typically defined once but
// referenced many times. For example, a component like "Instrument" or "Parties"
// might appear in dozens of different message types. Using `Rc<>` allows:
//
// - **Memory efficiency**: The definition exists in memory only once
// - **Consistency**: All references see the same definition
// - **Shared ownership**: Multiple messages can reference the same component
#[derive(Clone, Debug)]
pub enum MemberDefinition {
    /// A field member (primitive element)
    Field(Rc<Field>),

    /// A raw data member (Length + Data/XmlData pair)
    ///
    /// In the FIX protocol, fields of type Data or XmlData must be immediately
    /// preceded by an associated Length field. This variant represents that pair
    /// as a single semantic unit, since the data may contain arbitrary bytes
    /// including the SOH delimiter.
    RawData {
        /// The Length field that specifies the byte count
        length: Rc<Field>,
        /// The Data or XmlData field containing the raw bytes
        data: Rc<Field>,
    },

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
            MemberDefinition::RawData { data, .. } => &data.name,
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
/// Both messages share the same component definition, but each
/// has its own `Member` instance with a different `required` flag.
#[derive(Clone, Debug)]
pub struct Member {
    /// Whether this member is required (mandatory) in its parent container
    pub(super) required: bool,

    /// The underlying definition of this member
    pub(super) definition: MemberDefinition,
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

    /// Returns the length and data field references if this member is raw data
    ///
    /// # Returns
    ///
    /// `Some((&Field, &Field))` as (length, data) if this member is raw data, `None` otherwise
    pub fn as_raw_data(&self) -> Option<(&Field, &Field)> {
        match &self.definition {
            MemberDefinition::RawData { length, data } => Some((length, data)),
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

    /// Returns true if this member is a raw data pair (Length + Data/XmlData)
    pub fn is_raw_data(&self) -> bool {
        matches!(self.definition, MemberDefinition::RawData { .. })
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
/// # Naming Convention
///
/// In XML dictionaries, groups follow a naming convention:
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
    pub(super) name: String,

    /// The NumInGroup counter field (with "No" prefix)
    pub(super) num_in_group: Rc<Field>,

    /// Members (fields, components, and nested groups) within this group
    pub(super) members: Vec<Member>,
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
    pub(super) name: String,

    /// Members (fields, groups, and other components) within this component
    pub(super) members: Vec<Member>,
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
    pub(super) name: String,

    /// Message type identifier (e.g., "0" for Heartbeat)
    pub(super) msg_type: MsgType,

    /// Category of the message (Admin or App)
    pub(super) msg_cat: MsgCat,

    /// Members (fields, components, and groups) that make up this message
    pub(super) members: Vec<Member>,
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
