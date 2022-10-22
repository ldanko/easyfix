#![feature(type_alias_impl_trait)]

use std::{collections::HashMap, convert::TryFrom, fmt, ops::Deref, str::FromStr};

use anyhow::{anyhow, bail, Context as ErrorContext, Result};
use xmltree::{Element, XMLNode};

type ElementIterator<'a> = impl Iterator<Item = &'a Element>;

trait XmlHelper {
    fn get_attribute(&self, attribute: &str) -> Result<&str>;
    fn get_child_element(&self, child: &str) -> Result<&Element>;
    fn get_child_elements(&self) -> ElementIterator;
}

impl XmlHelper for Element {
    fn get_attribute(&self, attribute: &str) -> Result<&str> {
        self.attributes
            .get(attribute)
            .map(String::as_ref)
            .ok_or_else(|| anyhow!("no `{}` attribute in `{}` element", attribute, self.name))
    }

    fn get_child_element(&self, child: &str) -> Result<&Element> {
        self.get_child(child)
            .ok_or_else(|| anyhow!("no `{}` child in `{}` element", child, self.name))
    }

    fn get_child_elements(&self) -> ElementIterator {
        self.children.iter().filter_map(XMLNode::as_element)
    }
}

#[derive(Debug, PartialEq)]
pub struct Version {
    major: u32,
    minor: u32,
    service_pack: u32,
}

impl Version {
    fn from_xml(element: &Element) -> Result<Version> {
        Ok(Version {
            major: element
                .get_attribute("major")?
                .parse()
                .context("Failed to parse `major` number")?,
            minor: element
                .get_attribute("minor")?
                .parse()
                .context("Failed to parse `minor` number")?,
            service_pack: element
                .get_attribute("servicepack")?
                .parse()
                .context("Failed to parse `servicepack` number")?,
        })
    }

    pub fn major(&self) -> u32 {
        self.major
    }

    pub fn minor(&self) -> u32 {
        self.minor
    }

    pub fn service_pack(&self) -> u32 {
        self.service_pack
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.service_pack == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{} SP{}", self.major, self.minor, self.service_pack)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemberKind {
    Component,
    Field,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    name: String,
    required: bool,
    kind: MemberKind,
}

impl Member {
    fn from_xml(element: &Element) -> Result<Member> {
        let name = element.get_attribute("name")?;
        if !name.is_ascii() {
            bail!("Non ASCII characters in member name: {}", name);
        }
        let required = deserialize_yes_no(element.get_attribute("required")?)?;
        let kind = match element.name.as_ref() {
            "field" => MemberKind::Field,
            "component" | "group" => MemberKind::Component,
            name => bail!("Unexpected member kind `{}`", name),
        };
        Ok(Member {
            name: name.into(),
            required,
            kind,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn required(&self) -> bool {
        self.required
    }

    pub fn kind(&self) -> MemberKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum BasicType {
    Amt,
    Boolean,
    Char,
    Country,
    Currency,
    Data,
    Exchange,
    Float,
    Int,
    Language,
    Length,
    LocalMktDate,
    MonthYear,
    MultipleCharValue,
    MultipleStringValue,
    NumInGroup,
    Percentage,
    Price,
    PriceOffset,
    Qty,
    SeqNum,
    String,
    TzTimeOnly,
    TzTimestamp,
    UtcDateOnly,
    UtcTimeOnly,
    UtcTimestamp,
    XmlData,
}

impl TryFrom<&str> for BasicType {
    type Error = anyhow::Error;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        match input {
            "AMT" => Ok(BasicType::Amt),
            "BOOLEAN" => Ok(BasicType::Boolean),
            "CHAR" => Ok(BasicType::Char),
            "COUNTRY" => Ok(BasicType::Country),
            "CURRENCY" => Ok(BasicType::Currency),
            "DATA" => Ok(BasicType::Data),
            "EXCHANGE" => Ok(BasicType::Exchange),
            "FLOAT" => Ok(BasicType::Float),
            "INT" => Ok(BasicType::Int),
            "LANGUAGE" => Ok(BasicType::Language),
            "LENGTH" => Ok(BasicType::Length),
            "LOCALMKTDATE" => Ok(BasicType::LocalMktDate),
            "MONTHYEAR" => Ok(BasicType::MonthYear),
            "MULTIPLECHARVALUE" => Ok(BasicType::MultipleCharValue),
            "MULTIPLESTRINGVALUE" => Ok(BasicType::MultipleStringValue),
            "NUMINGROUP" => Ok(BasicType::NumInGroup),
            "PERCENTAGE" => Ok(BasicType::Percentage),
            "PRICE" => Ok(BasicType::Price),
            "PRICEOFFSET" => Ok(BasicType::PriceOffset),
            "QTY" => Ok(BasicType::Qty),
            "SEQNUM" => Ok(BasicType::SeqNum),
            "STRING" => Ok(BasicType::String),
            "TZTIMEONLY" => Ok(BasicType::TzTimeOnly),
            "TZTIMESTAMP" => Ok(BasicType::TzTimestamp),
            "UTCDATEONLY" => Ok(BasicType::UtcDateOnly),
            "UTCTIMEONLY" => Ok(BasicType::UtcTimeOnly),
            "UTCTIMESTAMP" => Ok(BasicType::UtcTimestamp),
            "XMLDATA" => Ok(BasicType::XmlData),
            other => Err(anyhow!("Unexpected type `{}`", other)),
        }
    }
}

impl FromStr for BasicType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TryFrom::try_from(s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    value: String,
    description: String,
}

impl Value {
    fn from_xml(element: &Element) -> Result<Value> {
        if element.name != "value" {
            bail!("Expected `value` node, found `{}`", element.name);
        }

        let value = element.get_attribute("enum")?;
        if !value.is_ascii() {
            bail!("Non ASCII characters in enum value: {}", value);
        }

        let description = element.get_attribute("description")?;
        if !description.is_ascii() {
            bail!("Non ASCII characters in enum description: {}", description);
        }

        Ok(Value {
            value: value.into(),
            description: description.into(),
        })
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    name: String,
    number: u16,
    type_: BasicType,
    values: Option<Vec<Value>>,
}

impl Field {
    fn from_xml(element: &Element) -> Result<Field> {
        let values = element
            .get_child_elements()
            .map(Value::from_xml)
            .collect::<Result<Vec<_>, _>>()?;
        let name = element.get_attribute("name")?;
        if !name.is_ascii() {
            bail!("Non ASCII characters in field name: {}", name);
        }
        Ok(Field {
            name: name.into(),
            number: element.get_attribute("number")?.parse()?,
            type_: element.get_attribute("type")?.parse()?,
            values: if values.is_empty() {
                None
            } else {
                Some(values)
            },
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn number(&self) -> u16 {
        self.number
    }

    pub fn type_(&self) -> BasicType {
        self.type_
    }

    pub fn values(&self) -> Option<&[Value]> {
        self.values.as_deref()
    }
}

fn deserialize_yes_no(input: &str) -> Result<bool> {
    match input {
        "Y" | "YES" | "y" | "yes" => Ok(true),
        "N" | "NO" | "n" | "no" => Ok(false),
        unexpected => Err(anyhow!(
            "parse yes/no failed, unexpected value `{}`",
            unexpected
        )),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    name: String,
    number_of_elements: Option<Member>,
    members: Vec<Member>,
}

impl Component {
    fn from_xml(element: &Element) -> Result<Component> {
        if element.name != "component" {
            bail!("Expected `component` node, found `{}`", element.name);
        }

        let name = element.get_attribute("name")?.to_owned();
        if !name.is_ascii() {
            bail!("Non ASCII characters in component name: {}", name);
        }

        let mut iter = element.get_child_elements().peekable();

        let number_of_elements = if let Some(child) = iter.peek() {
            if child.name == "group" {
                let member = Member::from_xml(child)?;
                iter = child.get_child_elements().peekable();
                Some(member)
            } else {
                None
            }
        } else {
            bail!("Empty member list in `{}` component", name)
        };

        let members = iter.map(Member::from_xml).collect::<Result<Vec<_>, _>>()?;

        Ok(Component {
            name,
            number_of_elements,
            members,
        })
    }

    // All groups are defined as separate component with one member - the group itself.
    // Except header (and possibly trailer) which has at least one group (`Hops`) defined inside.
    fn from_header_or_trailer(element: &Element) -> Result<(Component, Vec<Component>)> {
        let name = match element.name.as_str() {
            "header" => "Header".to_owned(),
            "trailer" => "Trailer".to_owned(),
            unexpected => bail!("Expected `header/trailer` node, found `{}`", unexpected),
        };

        let mut groups = Vec::new();
        let mut members = Vec::new();
        for member_element in element.get_child_elements() {
            if member_element.name == "group" {
                let group_name = member_element.get_attribute("name")?;
                let group_name = if group_name.starts_with("No") && group_name.ends_with('s') {
                    format!("{}Grp", &group_name[2..group_name.len() - 1])
                } else {
                    bail!("Malformed group name `{}`", group_name);
                };
                let number_of_elements = Some(Member::from_xml(member_element)?);
                let group_members = member_element
                    .get_child_elements()
                    .map(Member::from_xml)
                    .collect::<Result<Vec<_>, _>>()?;
                groups.push(Component {
                    name: group_name.clone(),
                    number_of_elements,
                    members: group_members,
                });
                let mut member_element = member_element.clone();
                if let Some(name) = member_element.attributes.get_mut("name") {
                    *name = group_name;
                }
                members.push(Member::from_xml(&member_element)?);
            } else {
                members.push(Member::from_xml(member_element)?);
            }
        }

        Ok((
            Component {
                name,
                number_of_elements: None,
                members,
            },
            groups,
        ))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn number_of_elements(&self) -> Option<&Member> {
        self.number_of_elements.as_ref()
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MsgCat {
    Admin,
    App,
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
enum MsgTypeBuf {
    Short([u8; 1]),
    Long([u8; 2]),
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct MsgType(MsgTypeBuf);

impl Deref for MsgType {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match self {
            MsgType(MsgTypeBuf::Short(b)) => b,
            MsgType(MsgTypeBuf::Long(b)) => b,
        }
    }
}

impl FromStr for MsgType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.is_ascii() {
            bail!("Non ASCII characters in message type: {}", s);
        }

        match s.as_bytes() {
            [] => Err(anyhow!("MsgType empty")),
            [b0 @ b'0'..=b'9' | b0 @ b'A'..=b'Z' | b0 @ b'a'..=b'z'] => {
                Ok(MsgType(MsgTypeBuf::Short([*b0])))
            }
            [b0 @ b'0'..=b'9' | b0 @ b'A'..=b'Z' | b0 @ b'a'..=b'z', b1 @ b'0'..=b'9' | b1 @ b'A'..=b'Z' | b1 @ b'a'..=b'z'] => {
                Ok(MsgType(MsgTypeBuf::Long([*b0, *b1])))
            }
            [_] | [_, _] => Err(anyhow!("Incorrect MsgType value: {}", s)),
            _ => Err(anyhow!("MsgType (`{}`) too long ({}`", s, s.len())),
        }
    }
}

impl FromStr for MsgCat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(MsgCat::Admin),
            "app" => Ok(MsgCat::App),
            other => Err(anyhow!("Unknown message category `{}`", other)),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Message {
    name: String,
    msg_cat: MsgCat,
    msg_type: MsgType,
    members: Vec<Member>,
}

impl Message {
    fn from_xml(element: &Element) -> Result<Message> {
        if element.name != "message" {
            bail!("Expected `message` node, found `{}`", element.name);
        }

        let name = element.get_attribute("name")?.to_owned();
        if !name.is_ascii() {
            bail!("Non ASCII characters in message name: {}", name);
        }
        let msg_cat = element.get_attribute("msgcat")?.parse()?;
        let msg_type = element.get_attribute("msgtype")?.parse()?;

        let members = element
            .get_child_elements()
            .map(Member::from_xml)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Message {
            name,
            msg_cat,
            msg_type,
            members,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn msg_cat(&self) -> MsgCat {
        self.msg_cat
    }

    pub fn msg_type(&self) -> MsgType {
        self.msg_type
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }
}

#[derive(Debug, PartialEq)]
pub struct Dictionary {
    fix_version: Option<Version>,
    fixt_version: Option<Version>,
    header: Option<Component>,
    trailer: Option<Component>,
    messages: HashMap<MsgType, Message>,
    flat_messages: HashMap<String, Message>,
    components: Vec<Component>,
    components_by_name: HashMap<String, Component>,
    fields: HashMap<u16, Field>,
    fields_by_name: HashMap<String, Field>,
}

impl Default for Dictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl Dictionary {
    pub fn new() -> Dictionary {
        Dictionary {
            fixt_version: None,
            fix_version: None,
            header: None,
            trailer: None,
            messages: HashMap::new(),
            flat_messages: HashMap::new(),
            components: Vec::new(),
            components_by_name: HashMap::new(),
            fields: HashMap::new(),
            fields_by_name: HashMap::new(),
        }
    }

    pub fn process_fixt_xml(&mut self, xml: &str) -> Result<()> {
        let root = Element::parse(xml.as_bytes()).context("Failed to parse FIXT description")?;

        let type_ = root.get_attribute("type")?;
        if type_ != "FIXT" {
            bail!("Unexpected FIX XML description type `{}`", type_);
        }

        if self.fixt_version.is_some() {
            bail!("FIXT XML already processed");
        } else {
            self.fixt_version = Some(Version::from_xml(&root)?);
        }

        let (header, header_groups) =
            Component::from_header_or_trailer(root.get_child_element("header")?)
                .context("Failed to process FIXT Header")?;
        self.header = Some(header);
        self.components.extend(header_groups);

        let (trailer, trailer_groups) = Component::from_header_or_trailer(
            root.get_child_element("trailer")
                .context("Failed to process FIXT trailer")?,
        )?;
        self.trailer = Some(trailer);
        self.components.extend(trailer_groups);

        self.process_common(&root)
    }

    // TODO: Allow adding different FIX versions
    pub fn process_fix_xml(&mut self, xml: &str) -> Result<()> {
        let root = Element::parse(xml.as_bytes()).context("Failed to parse FIX description")?;

        let type_ = root.get_attribute("type")?;
        if type_ != "FIX" {
            bail!("Unexpected FIX XML description type `{}`", type_);
        }

        if self.fix_version.is_some() {
            bail!("FIX XML already processed");
        } else {
            self.fix_version = Some(Version::from_xml(&root)?);
        }

        self.process_common(&root)
    }

    fn process_common(&mut self, root: &Element) -> Result<()> {
        self.messages.extend(
            root.get_child_element("messages")?
                .get_child_elements()
                .map(Message::from_xml)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|m| (m.msg_type, m)),
        );

        self.components.extend(
            root.get_child_element("components")?
                .get_child_elements()
                .map(Component::from_xml)
                .collect::<Result<Vec<_>>>()?
                .into_iter(),
        );
        self.components_by_name.extend(
            self.components
                .iter()
                .map(|c| (c.name().to_owned(), c.clone())),
        );

        self.fields.extend(
            root.get_child_element("fields")?
                .get_child_elements()
                .map(Field::from_xml)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|f| (f.number, f)),
        );

        self.fields_by_name.extend(
            self.fields
                .values()
                .map(|f| (f.name().to_owned(), f.to_owned())),
        );

        Ok(())
    }

    pub fn fixt_version(&self) -> Option<&Version> {
        self.fixt_version.as_ref()
    }

    pub fn fix_version(&self) -> Option<&Version> {
        self.fix_version.as_ref()
    }

    pub fn header(&self) -> Result<&Component> {
        self.header
            .as_ref()
            .ok_or_else(|| anyhow!("Missing header"))
    }

    pub fn trailer(&self) -> Result<&Component> {
        self.trailer
            .as_ref()
            .ok_or_else(|| anyhow!("Missing trailer"))
    }

    pub fn components(&self) -> &[Component] {
        &self.components
    }

    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components_by_name.get(name)
    }

    pub fn message(&self, name: &MsgType) -> Option<&Message> {
        self.messages.get(name)
    }

    pub fn messages(&self) -> &HashMap<MsgType, Message> {
        &self.messages
    }

    pub fn fields(&self) -> &HashMap<u16, Field> {
        &self.fields
    }

    pub fn fields_by_name(&self) -> &HashMap<String, Field> {
        &self.fields_by_name
    }
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::MsgType;

    #[test]
    fn parse_msg_type() {
        assert!(MsgType::from_str("").is_err());
        assert!(MsgType::from_str("\0").is_err());
        assert!(MsgType::from_str("\0\0").is_err());
        assert!(MsgType::from_str("\0\0\0").is_err());
        assert!(MsgType::from_str("A").is_ok());
        assert!(MsgType::from_str("AA").is_ok());
        assert!(MsgType::from_str("AAA").is_err());
        assert!(MsgType::from_str("\0A").is_err());
    }
}
