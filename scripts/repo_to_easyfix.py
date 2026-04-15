#!/usr/bin/env python3
"""Convert FIX Repository 2010 Edition XML to easyfix dictionary XML.

Reads the FIX Repository 2010 Edition files (Fields.xml, Enums.xml,
Components.xml, Messages.xml, MsgContents.xml) and generates XML
dictionaries compatible with easyfix-dictionary.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import xml.etree.ElementTree as ET
from collections import defaultdict
from dataclasses import dataclass, field
from typing import Union


# Components that have the same name as fields - need renaming to avoid collision
# The easyfix-dictionary resolver treats field and component names as shared namespace
COMPONENT_RENAME_MAP = {
    "SecurityXML": "SecurityXMLBlock",
    "DerivativeSecurityXML": "DerivativeSecurityXMLBlock",
    "RateSource": "RateSourceBlock",
}


# Type mapping from repository types to UPPERCASE types used by easyfix
TYPE_MAP = {
    "String": "STRING",
    "char": "CHAR",
    "int": "INT",
    "float": "FLOAT",
    "Boolean": "BOOLEAN",
    "data": "DATA",
    "Length": "LENGTH",
    "SeqNum": "SEQNUM",
    "NumInGroup": "NUMINGROUP",
    "Qty": "QTY",
    "Price": "PRICE",
    "Amt": "AMT",
    "UTCTimestamp": "UTCTIMESTAMP",
    "UTCTimeOnly": "UTCTIMEONLY",
    "UTCDateOnly": "UTCDATEONLY",
    "LocalMktDate": "LOCALMKTDATE",
    "MonthYear": "MONTHYEAR",
    "MultipleCharValue": "MULTIPLECHARVALUE",
    "MultipleStringValue": "MULTIPLESTRINGVALUE",
    "Currency": "CURRENCY",
    "Exchange": "EXCHANGE",
    "Country": "COUNTRY",
    "Language": "LANGUAGE",
    "Percentage": "PERCENTAGE",
    "PriceOffset": "PRICEOFFSET",
    "TZTimeOnly": "TZTIMEONLY",
    "TZTimestamp": "TZTIMESTAMP",
    "XMLData": "XMLDATA",
    # Aliases
    "MultipleValueString": "MULTIPLESTRINGVALUE",
    "Tenor": "STRING",
    "Pattern": "STRING",
    "Reserved100Plus": "STRING",
    "Reserved1000Plus": "STRING",
    "Reserved4000Plus": "STRING",
    "DayOfMonth": "INT",
    "long": "INT",
    "TagNum": "INT",
}


def rename_component(name: str) -> str:
    """Rename component if it conflicts with a field name."""
    return COMPONENT_RENAME_MAP.get(name, name)


def camel_to_upper_snake(name: str) -> str:
    """Convert CamelCase/PascalCase to UPPER_SNAKE_CASE.

    Handles:
    - Buy -> BUY
    - TestRequest -> TEST_REQUEST
    - NoneOther -> NONE_OTHER
    - FIX50SP2 -> FIX50SP2
    - IOI -> IOI
    - PerUnit -> PER_UNIT
    - PKCS -> PKCS
    - PGP_DES -> PGP_DES (already has underscores)
    """
    if not name:
        return name

    # If already contains underscores, just uppercase it
    if "_" in name:
        return name.upper()

    # Insert underscores before transitions:
    # - lowercase/digit -> uppercase: aB -> a_B, 0A -> 0_A
    # - uppercase -> uppercase+lowercase: ABc -> A_Bc (end of acronym)
    result = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    result = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", result)
    return result.upper()


def parse_position(pos_str: str) -> tuple[float, ...]:
    """Parse position string for sorting.

    Positions like "1", "2", "8.1", "11.21" need numeric sorting.
    """
    parts = pos_str.split(".")
    result = []
    for p in parts:
        try:
            result.append(float(p))
        except ValueError:
            result.append(0.0)
    while len(result) < 3:
        result.append(0.0)
    return tuple(result)


def msg_type_sort_key(msgtype: str) -> tuple[int, int, int]:
    """Sort key for message types: digits, uppercase, lowercase, two-char."""
    if len(msgtype) == 1:
        c = msgtype[0]
        if c.isdigit():
            return (0, ord(c), 0)
        elif c.isupper():
            return (1, ord(c), 0)
        else:
            return (2, ord(c), 0)
    else:
        return (3, ord(msgtype[0]), ord(msgtype[1]))


# ---------------------------------------------------------------------------
# Input model (Repository)
# ---------------------------------------------------------------------------

@dataclass
class RepoField:
    tag: int
    name: str
    type_name: str

    @property
    def is_numingroup(self) -> bool:
        return self.type_name == "NumInGroup"


@dataclass
class RepoEnum:
    tag: int
    value: str
    symbolic_name: str
    sort: int


@dataclass
class RepoComponent:
    id: str
    name: str
    component_type: str

    @property
    def is_repeating(self) -> bool:
        return "Repeating" in self.component_type


@dataclass
class RepoMessage:
    component_id: str
    name: str
    msg_type: str
    category_id: str

    @property
    def is_session(self) -> bool:
        return self.category_id == "Session"


@dataclass
class RepoMsgContent:
    component_id: str
    tag_text: str
    position: str
    required: str
    indent: int

    @property
    def is_field_ref(self) -> bool:
        return self.tag_text.isdigit()

    @property
    def tag_number(self) -> int:
        return int(self.tag_text)

    @property
    def is_standard(self) -> bool:
        return self.tag_text in ("StandardHeader", "StandardTrailer")


class Repository:
    """Loads and indexes FIX Repository XML files."""

    def __init__(self, repo_dir: str, version: str):
        base_dir = os.path.join(repo_dir, version, "Base")

        if not os.path.isdir(base_dir):
            print(f"Error: directory not found: {base_dir}", file=sys.stderr)
            sys.exit(1)

        self.fields_by_tag: dict[int, RepoField] = {}
        self.fields_by_name: dict[str, RepoField] = {}
        self.enums_by_tag: dict[int, list[RepoEnum]] = defaultdict(list)
        self.components_by_id: dict[str, RepoComponent] = {}
        self.components_by_name: dict[str, RepoComponent] = {}
        self.messages: list[RepoMessage] = []
        self.contents_by_id: dict[str, list[RepoMsgContent]] = defaultdict(list)

        self._load_fields(base_dir)
        self._load_enums(base_dir)
        self._load_components(base_dir)
        self._load_messages(base_dir)
        self._load_msg_contents(base_dir)

    def _load_fields(self, base_dir: str) -> None:
        tree = ET.parse(os.path.join(base_dir, "Fields.xml"))
        for elem in tree.getroot().findall("Field"):
            f = RepoField(
                tag=int(elem.find("Tag").text),
                name=elem.find("Name").text,
                type_name=elem.find("Type").text,
            )
            self.fields_by_tag[f.tag] = f
            self.fields_by_name[f.name] = f

    def _load_enums(self, base_dir: str) -> None:
        tree = ET.parse(os.path.join(base_dir, "Enums.xml"))
        for elem in tree.getroot().findall("Enum"):
            tag = int(elem.find("Tag").text)
            value = elem.find("Value").text
            sym_el = elem.find("SymbolicName")
            symbolic = sym_el.text if sym_el is not None and sym_el.text else ""
            sort_el = elem.find("Sort")
            sort_val = int(sort_el.text) if sort_el is not None and sort_el.text else 0
            if symbolic:
                self.enums_by_tag[tag].append(
                    RepoEnum(tag=tag, value=value, symbolic_name=symbolic, sort=sort_val)
                )

    def _load_components(self, base_dir: str) -> None:
        tree = ET.parse(os.path.join(base_dir, "Components.xml"))
        for elem in tree.getroot().findall("Component"):
            c = RepoComponent(
                id=elem.find("ComponentID").text,
                name=elem.find("Name").text,
                component_type=elem.find("ComponentType").text,
            )
            self.components_by_id[c.id] = c
            self.components_by_name[c.name] = c

    def _load_messages(self, base_dir: str) -> None:
        tree = ET.parse(os.path.join(base_dir, "Messages.xml"))
        for elem in tree.getroot().findall("Message"):
            self.messages.append(RepoMessage(
                component_id=elem.find("ComponentID").text,
                name=elem.find("Name").text,
                msg_type=elem.find("MsgType").text,
                category_id=elem.find("CategoryID").text,
            ))

    def _load_msg_contents(self, base_dir: str) -> None:
        tree = ET.parse(os.path.join(base_dir, "MsgContents.xml"))
        for elem in tree.getroot().findall("MsgContent"):
            indent_el = elem.find("Indent")
            mc = RepoMsgContent(
                component_id=elem.find("ComponentID").text,
                tag_text=elem.find("TagText").text,
                position=elem.find("Position").text,
                required=elem.find("Reqd").text,
                indent=int(indent_el.text) if indent_el is not None and indent_el.text else 0,
            )
            self.contents_by_id[mc.component_id].append(mc)
        for cid in self.contents_by_id:
            self.contents_by_id[cid].sort(key=lambda mc: parse_position(mc.position))


# ---------------------------------------------------------------------------
# Output model (Dictionary)
# ---------------------------------------------------------------------------

@dataclass
class FieldMember:
    name: str
    required: str


@dataclass
class ComponentRef:
    name: str
    required: str


@dataclass
class GroupMember:
    name: str
    required: str
    members: list[Member]


Member = Union[FieldMember, ComponentRef, GroupMember]


@dataclass
class EnumValue:
    enum: str
    description: str


@dataclass
class FieldDef:
    number: int
    name: str
    type: str
    values: list[EnumValue]


@dataclass
class MessageDef:
    name: str
    msgtype: str
    msgcat: str
    members: list[Member]


@dataclass
class ComponentDef:
    name: str
    members: list[Member]


@dataclass
class Dictionary:
    fix_type: str
    major: int
    minor: int
    servicepack: int
    header: list[Member]
    trailer: list[Member]
    messages: list[MessageDef]
    components: list[ComponentDef]
    fields: list[FieldDef]


# ---------------------------------------------------------------------------
# Resolver (Repository -> Dictionary)
# ---------------------------------------------------------------------------

class Resolver:
    """Transforms Repository data into a Dictionary."""

    def __init__(self, repo: Repository, fallback: Repository | None = None):
        self.repo = repo
        self.fallback = fallback

    def _lookup_field(self, tag: int) -> RepoField | None:
        f = self.repo.fields_by_tag.get(tag)
        if f is None and self.fallback:
            f = self.fallback.fields_by_tag.get(tag)
        return f

    def _lookup_field_by_name(self, name: str) -> RepoField | None:
        f = self.repo.fields_by_name.get(name)
        if f is None and self.fallback:
            f = self.fallback.fields_by_name.get(name)
        return f

    def _lookup_component(self, name: str) -> RepoComponent | None:
        c = self.repo.components_by_name.get(name)
        if c is None and self.fallback:
            c = self.fallback.components_by_name.get(name)
        return c

    def _get_component_by_id(self, comp_id: str) -> RepoComponent | None:
        c = self.repo.components_by_id.get(comp_id)
        if c is None and self.fallback:
            c = self.fallback.components_by_id.get(comp_id)
        return c

    def _get_contents(self, comp_id: str) -> list[RepoMsgContent]:
        contents = self.repo.contents_by_id.get(comp_id, [])
        if not contents and self.fallback:
            contents = self.fallback.contents_by_id.get(comp_id, [])
        return contents

    def _is_group_component(self, comp_id: str) -> bool:
        """Determine if a component is a repeating group.

        A component is a repeating group if:
        1. Its ComponentType contains "Repeating", OR
        2. Its first field is of type NumInGroup
        """
        comp = self._get_component_by_id(comp_id)
        if comp is not None and comp.is_repeating:
            return True

        contents = self._get_contents(comp_id)
        if contents:
            first = contents[0]
            if first.is_field_ref:
                f = self._lookup_field(first.tag_number)
                if f and f.is_numingroup:
                    return True
        return False

    def _resolve_flat_members(self, contents: list[RepoMsgContent]) -> list[Member]:
        """Build a flat list of members from contents."""
        members: list[Member] = []
        for mc in contents:
            if mc.is_standard:
                continue
            required = "Y" if mc.required == "1" else "N"
            if mc.is_field_ref:
                f = self._lookup_field(mc.tag_number)
                if f:
                    members.append(FieldMember(name=f.name, required=required))
            else:
                members.append(ComponentRef(name=mc.tag_text, required=required))
        return members

    def _resolve_group(self, contents: list[RepoMsgContent]) -> list[Member]:
        """Build a group member from contents where first entry is NUMINGROUP."""
        if not contents:
            return []

        first = contents[0]
        counter_required = "Y" if first.required == "1" else "N"

        if first.is_field_ref:
            f = self._lookup_field(first.tag_number)
            counter_name = f.name if f else None
        else:
            counter_name = first.tag_text

        if not counter_name:
            return []

        group_members: list[Member] = []
        for mc in contents[1:]:
            if mc.is_standard:
                continue
            required = "Y" if mc.required == "1" else "N"
            if mc.is_field_ref:
                f = self._lookup_field(mc.tag_number)
                if f:
                    group_members.append(FieldMember(name=f.name, required=required))
            else:
                group_members.append(ComponentRef(name=mc.tag_text, required=required))

        return [GroupMember(name=counter_name, required=counter_required, members=group_members)]

    def _resolve_component_body(self, comp_id: str) -> list[Member]:
        """Resolve a component's contents into member list."""
        contents = self._get_contents(comp_id)
        if not contents:
            return []

        if self._is_group_component(comp_id):
            return self._resolve_group(contents)
        else:
            return self._resolve_flat_members(contents)

    def _resolve_header(self) -> list[Member]:
        """Resolve StandardHeader into member list, inlining groups."""
        comp = self.repo.components_by_name.get("StandardHeader")
        if not comp:
            return []

        contents = self._get_contents(comp.id)
        members: list[Member] = []

        for mc in contents:
            if mc.is_standard:
                continue
            required = "Y" if mc.required == "1" else "N"
            if mc.is_field_ref:
                f = self._lookup_field(mc.tag_number)
                if f:
                    members.append(FieldMember(name=f.name, required=required))
            else:
                ref_comp = self._lookup_component(mc.tag_text)
                if ref_comp and self._is_group_component(ref_comp.id):
                    # Inline the group directly into header
                    group_members = self._resolve_component_body(ref_comp.id)
                    for gm in group_members:
                        if isinstance(gm, GroupMember):
                            gm.required = required
                    members.extend(group_members)
                else:
                    members.append(ComponentRef(name=mc.tag_text, required=required))

        return members

    def _resolve_trailer(self) -> list[Member]:
        """Resolve StandardTrailer into member list."""
        comp = self.repo.components_by_name.get("StandardTrailer")
        if not comp:
            return []
        contents = self._get_contents(comp.id)
        return self._resolve_flat_members(contents)

    def _resolve_message_members(self, msg: RepoMessage) -> list[Member]:
        """Resolve a message's MsgContents into its member list."""
        contents = self._get_contents(msg.component_id)
        return self._resolve_flat_members(contents)

    def _collect_referenced_tags(
        self,
        messages: list[MessageDef],
        components: list[ComponentDef],
        header: list[Member],
        trailer: list[Member],
    ) -> set[int]:
        """Collect all field tag numbers referenced anywhere."""
        tags: set[int] = set()

        def _collect(members: list[Member]) -> None:
            for m in members:
                if isinstance(m, FieldMember):
                    f = self._lookup_field_by_name(m.name)
                    if f:
                        tags.add(f.tag)
                elif isinstance(m, GroupMember):
                    f = self._lookup_field_by_name(m.name)
                    if f:
                        tags.add(f.tag)
                    _collect(m.members)

        for msg in messages:
            _collect(msg.members)
        for comp in components:
            _collect(comp.members)
        _collect(header)
        _collect(trailer)
        return tags

    def _build_field_defs(self, referenced_tags: set[int], is_fixt: bool) -> list[FieldDef]:
        """Build field definitions for all referenced tags."""
        # Merge field registries
        all_fields: dict[int, RepoField] = dict(self.repo.fields_by_tag)
        if self.fallback:
            for tag, f in self.fallback.fields_by_tag.items():
                if tag not in all_fields:
                    all_fields[tag] = f

        # Merge enum registries
        all_enums: dict[int, list[RepoEnum]] = defaultdict(list)
        for tag, enums in self.repo.enums_by_tag.items():
            all_enums[tag] = list(enums)
        if is_fixt and self.fallback:
            # Merge MsgType enums from app version
            if 35 in self.fallback.enums_by_tag:
                existing_values = {e.value for e in all_enums.get(35, [])}
                for entry in self.fallback.enums_by_tag[35]:
                    if entry.value not in existing_values:
                        all_enums[35].append(entry)

        fields: list[FieldDef] = []
        for tag in sorted(referenced_tags):
            if tag not in all_fields:
                continue
            f = all_fields[tag]
            mapped_type = TYPE_MAP.get(f.type_name, f.type_name.upper())

            enums = all_enums.get(tag, self.repo.enums_by_tag.get(tag, []))
            if not enums and self.fallback:
                enums = self.fallback.enums_by_tag.get(tag, [])

            enums_sorted = sorted(enums, key=lambda e: (e.sort, e.value))
            values = [
                EnumValue(enum=e.value, description=camel_to_upper_snake(e.symbolic_name))
                for e in enums_sorted
            ]

            fields.append(FieldDef(number=tag, name=f.name, type=mapped_type, values=values))

        return fields

    def resolve(self, version_str: str) -> Dictionary:
        """Main entry point: resolve Repository into Dictionary."""
        is_fixt = version_str.startswith("FIXT")

        # Parse version string
        if is_fixt:
            fix_type = "FIXT"
            parts = version_str.replace("FIXT.", "").split(".")
            major = int(parts[0])
            minor = int(parts[1]) if len(parts) > 1 else 0
            servicepack = 0
        else:
            fix_type = "FIX"
            rest = version_str.replace("FIX.", "")
            sp_match = re.match(r"(\d+)\.(\d+)(?:SP(\d+))?", rest)
            if sp_match:
                major = int(sp_match.group(1))
                minor = int(sp_match.group(2))
                servicepack = int(sp_match.group(3)) if sp_match.group(3) else 0
            else:
                major, minor, servicepack = 5, 0, 2

        # Header and trailer
        header: list[Member] = []
        trailer: list[Member] = []
        if is_fixt:
            header = self._resolve_header()
            trailer = self._resolve_trailer()

        # Build messages
        messages: list[MessageDef] = []
        for msg in self.repo.messages:
            if not is_fixt and msg.is_session:
                continue
            msgcat = "admin" if msg.is_session else "app"
            members = self._resolve_message_members(msg)
            messages.append(MessageDef(
                name=msg.name, msgtype=msg.msg_type,
                msgcat=msgcat, members=members,
            ))
        # Remove messages with no body members (e.g., XMLnonFIX)
        messages = [m for m in messages if m.members]
        messages.sort(key=lambda m: msg_type_sort_key(m.msgtype))

        # Build components (exclude StandardHeader/StandardTrailer and components
        # that are inlined into header)
        header_inlined: set[str] = set()
        if is_fixt:
            std_header = self.repo.components_by_name.get("StandardHeader")
            if std_header:
                for mc in self._get_contents(std_header.id):
                    if not mc.is_field_ref and not mc.is_standard:
                        ref_comp = self._lookup_component(mc.tag_text)
                        if ref_comp and self._is_group_component(ref_comp.id):
                            header_inlined.add(mc.tag_text)

        components: list[ComponentDef] = []
        for cid, comp in self.repo.components_by_id.items():
            if comp.name in ("StandardHeader", "StandardTrailer"):
                continue
            if comp.name in header_inlined:
                continue
            body = self._resolve_component_body(cid)
            components.append(ComponentDef(name=comp.name, members=body))

        # Collect referenced field tags
        referenced_tags = self._collect_referenced_tags(messages, components, header, trailer)

        # Build field definitions
        fields = self._build_field_defs(referenced_tags, is_fixt)

        return Dictionary(
            fix_type=fix_type,
            major=major,
            minor=minor,
            servicepack=servicepack,
            header=header,
            trailer=trailer,
            messages=messages,
            components=components,
            fields=fields,
        )


# ---------------------------------------------------------------------------
# Serialization (Dictionary -> XML)
# ---------------------------------------------------------------------------

def _serialize_members(parent: ET.Element, members: list[Member]) -> None:
    """Recursively add member elements to an ET parent."""
    for m in members:
        if isinstance(m, FieldMember):
            ET.SubElement(parent, "field", name=m.name, required=m.required)
        elif isinstance(m, ComponentRef):
            ET.SubElement(parent, "component", name=rename_component(m.name), required=m.required)
        elif isinstance(m, GroupMember):
            group_el = ET.SubElement(parent, "group", name=m.name, required=m.required)
            _serialize_members(group_el, m.members)


def serialize(dictionary: Dictionary) -> str:
    """Serialize a Dictionary to XML string."""
    root = ET.Element("fix",
                       type=dictionary.fix_type,
                       major=str(dictionary.major),
                       minor=str(dictionary.minor),
                       servicepack=str(dictionary.servicepack))

    # Header
    header_el = ET.SubElement(root, "header")
    if dictionary.header:
        _serialize_members(header_el, dictionary.header)

    # Messages
    messages_el = ET.SubElement(root, "messages")
    for msg in dictionary.messages:
        msg_el = ET.SubElement(messages_el, "message",
                               name=msg.name, msgtype=msg.msgtype, msgcat=msg.msgcat)
        _serialize_members(msg_el, msg.members)

    # Trailer
    trailer_el = ET.SubElement(root, "trailer")
    if dictionary.trailer:
        _serialize_members(trailer_el, dictionary.trailer)

    # Components
    components_el = ET.SubElement(root, "components")
    for comp in dictionary.components:
        comp_el = ET.SubElement(components_el, "component", name=rename_component(comp.name))
        _serialize_members(comp_el, comp.members)

    # Fields
    fields_el = ET.SubElement(root, "fields")
    for f in dictionary.fields:
        field_el = ET.SubElement(fields_el, "field",
                                  number=str(f.number), name=f.name, type=f.type)
        for v in f.values:
            ET.SubElement(field_el, "value", enum=v.enum, description=v.description)

    ET.indent(root, space=" ")
    return ET.tostring(root, encoding="unicode") + "\n"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Convert FIX Repository to easyfix dictionary XML")
    parser.add_argument("--repo-dir", required=True,
                        help="Path to FIX Repository 2010 Edition root directory")
    parser.add_argument("--version", required=True,
                        help="FIX version (e.g., FIXT.1.1, FIX.5.0SP2)")
    parser.add_argument("--output", required=True,
                        help="Output XML file path")
    parser.add_argument("--fallback-version", default=None,
                        help="Fallback version for cross-version references")
    args = parser.parse_args()

    print(f"Loading {args.version} from {args.repo_dir}...")
    repo = Repository(args.repo_dir, args.version)

    fallback = None
    fallback_version = args.fallback_version
    if fallback_version is None and args.version == "FIXT.1.1":
        fallback_version = "FIX.5.0SP2"

    if fallback_version:
        print(f"Loading fallback {fallback_version}...")
        fallback = Repository(args.repo_dir, fallback_version)

    print("Generating XML...")
    resolver = Resolver(repo, fallback)
    dictionary = resolver.resolve(args.version)
    xml_output = serialize(dictionary)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w", encoding="utf-8") as f:
        f.write(xml_output)

    msg_count = len(dictionary.messages)
    field_count = len(dictionary.fields)
    group_count = sum(
        1 for f in dictionary.fields if f.type == "NUMINGROUP"
    )
    comp_count = len(dictionary.components)
    print(f"Written to {args.output}")
    print(f"  Messages: {msg_count}, Components: {comp_count}, "
          f"Fields: {field_count}, Groups: {group_count}")


if __name__ == "__main__":
    main()
