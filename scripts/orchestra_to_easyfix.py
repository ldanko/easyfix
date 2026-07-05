#!/usr/bin/env python3
"""Convert FIX Orchestra XML to easyfix dictionary XML.

Reads a FIX Orchestra XML file (e.g., OrchestraFIXLatest.xml, OrchestraFIX44.xml,
FIXTSession.xml) and generates XML dictionaries compatible with easyfix-dictionary.
"""

from __future__ import annotations

import argparse
import os
import re
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from typing import Union


FIXR_NS = "http://fixprotocol.io/2020/orchestra/repository"

# Fields that should NOT have enum values emitted even if they have a codeSet
# in Orchestra. These are fields where the existing easyfix code treats them as
# plain STRING fields, not enums.
STRIP_ENUM_FIELDS = {
    "RefMsgType",  # field 372 - free-form STRING that takes MsgType values
}

# Components/groups that have the same name as fields - need renaming to avoid collision.
# The easyfix-dictionary resolver treats field and component names as a shared namespace.
# Suffix "Block" is added to component names that collide with field names.
COMPONENT_RENAME_MAP = {
    "SecurityXML": "SecurityXMLBlock",
    "DerivativeSecurityXML": "DerivativeSecurityXMLBlock",
    "RateSource": "RateSourceBlock",
    "LegSecurityXML": "LegSecurityXMLBlock",
    "UnderlyingSecurityXML": "UnderlyingSecurityXMLBlock",
    "PaymentStreamFormula": "PaymentStreamFormulaBlock",
    "PaymentStreamFormulaImage": "PaymentStreamFormulaImageBlock",
    "PaymentStreamNonDeliverableSettlRateSource": "PaymentStreamNonDeliverableSettlRateSourceBlock",
    "LegPaymentStreamFormula": "LegPaymentStreamFormulaBlock",
    "LegPaymentStreamFormulaImage": "LegPaymentStreamFormulaImageBlock",
    "LegPaymentStreamNonDeliverableSettlRateSource": "LegPaymentStreamNonDeliverableSettlRateSourceBlock",
    "UnderlyingPaymentStreamFormula": "UnderlyingPaymentStreamFormulaBlock",
    "UnderlyingPaymentStreamFormulaImage": "UnderlyingPaymentStreamFormulaImageBlock",
    "UnderlyingPaymentStreamNonDeliverableSettlRateSource": "UnderlyingPaymentStreamNonDeliverableSettlRateSourceBlock",
    "SettlRateFallbackRateSource": "SettlRateFallbackRateSourceBlock",
    "LegSettlRateFallbackRateSource": "LegSettlRateFallbackRateSourceBlock",
    "UnderlyingSettlRateFallbackRateSource": "UnderlyingSettlRateFallbackRateSourceBlock",
    "ProvisionCashSettlQuoteSource": "ProvisionCashSettlQuoteSourceBlock",
    "LegProvisionCashSettlQuoteSource": "LegProvisionCashSettlQuoteSourceBlock",
    "UnderlyingProvisionCashSettlQuoteSource": "UnderlyingProvisionCashSettlQuoteSourceBlock",
}

# Mapping from Orchestra datatype names to easyfix type strings.
# Base types (int, float, char, String, data) and derived types that have
# a direct mapping are listed here. Derived types not listed will be resolved
# by walking the baseType chain until a known type is found.
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
    "LocalMktTime": "STRING",
    "XID": "STRING",
    "XIDREF": "STRING",
}


def rename_component(name: str) -> str:
    """Rename component if it conflicts with a field name."""
    return COMPONENT_RENAME_MAP.get(name, name)


def camel_to_upper_snake(name: str) -> str:
    """Convert CamelCase/PascalCase to UPPER_SNAKE_CASE."""
    if not name:
        return name
    if "_" in name:
        return name.upper()
    result = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    result = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", result)
    return result.upper()


def normalize_doc_text(text: str | None) -> str | None:
    """Normalize documentation text from an Orchestra documentation element.

    Line breaks in Orchestra documentation are pretty-printing artifacts,
    so lines are stripped and joined with single spaces.
    """
    if not text:
        return None
    lines = [line.strip() for line in text.splitlines()]
    result = " ".join(line for line in lines if line)
    return result if result else None


def is_trivial_enum_doc(doc: str, code_name: str) -> bool:
    """True when the doc adds nothing over the code name."""
    def norm(s: str) -> str:
        return re.sub(r"[^a-z0-9]", "", s.lower())
    return norm(doc) == norm(code_name)


def extract_doc(elem: ET.Element) -> str | None:
    """Extract documentation from an element's direct fixr:annotation child.

    Takes documentation with purpose="SYNOPSIS"; when there is none, falls
    back to documentation without a purpose attribute. ELABORATION and
    FIXML entries are ignored. Multiple entries are joined as paragraphs.
    """
    ann = elem.find(f"{{{FIXR_NS}}}annotation")
    if ann is None:
        return None
    synopsis: list[str] = []
    unpurposed: list[str] = []
    for doc_el in ann.findall(f"{{{FIXR_NS}}}documentation"):
        text = normalize_doc_text("".join(doc_el.itertext()))
        if not text:
            continue
        purpose = doc_el.get("purpose")
        if purpose == "SYNOPSIS":
            synopsis.append(text)
        elif purpose is None:
            unpurposed.append(text)
    docs = synopsis if synopsis else unpurposed
    return "\n".join(docs) if docs else None


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
# Output model (Dictionary) - same as repo_to_easyfix.py
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
    doc: str | None = None


Member = Union[FieldMember, ComponentRef, GroupMember]


@dataclass
class EnumValue:
    enum: str
    description: str
    doc: str | None = None


@dataclass
class FieldDef:
    number: int
    name: str
    type: str
    values: list[EnumValue]
    doc: str | None = None


@dataclass
class MessageDef:
    name: str
    msgtype: str
    msgcat: str
    members: list[Member]
    doc: str | None = None


@dataclass
class ComponentDef:
    name: str
    members: list[Member]
    doc: str | None = None


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


def prune_unused_components(
    components: list[ComponentDef],
    messages: list[MessageDef],
    header: list[Member],
    trailer: list[Member],
) -> list[ComponentDef]:
    """Drop components not transitively reachable from messages, header
    or trailer.

    Unreferenced components would be rejected by easyfix-dictionary strict
    validation as unused elements.
    """
    by_name = {c.name: c for c in components}

    def collect_refs(members: list[Member], out: list[str]) -> None:
        for m in members:
            if isinstance(m, ComponentRef):
                out.append(m.name)
            elif isinstance(m, GroupMember):
                collect_refs(m.members, out)

    pending: list[str] = []
    for msg in messages:
        collect_refs(msg.members, pending)
    collect_refs(header, pending)
    collect_refs(trailer, pending)

    reachable: set[str] = set()
    while pending:
        name = pending.pop()
        if name in reachable or name not in by_name:
            continue
        reachable.add(name)
        collect_refs(by_name[name].members, pending)

    return [c for c in components if c.name in reachable]


# ---------------------------------------------------------------------------
# Orchestra Parser
# ---------------------------------------------------------------------------

class OrchestraRepository:
    """Loads and indexes a FIX Orchestra XML file."""

    def __init__(self, path: str):
        self.tree = ET.parse(path)
        self.root = self.tree.getroot()

        # Repository attributes
        self.repo_name = self.root.get("name", "")
        self.repo_version = self.root.get("version", "")

        # Indexes
        self.datatypes: dict[str, str | None] = {}  # name -> baseType (None if root)
        self.codesets: dict[str, tuple[str, list[tuple[str, str, int, str | None]]]] = {}  # name -> (base_type, [(value, code_name, sort, doc)])
        self.codeset_by_id: dict[str, str] = {}  # id -> codeSet name
        self.fields_by_id: dict[str, tuple[str, str, str | None]] = {}  # id -> (name, type, doc)
        self.fields_by_name: dict[str, str] = {}  # name -> id
        self.components_by_id: dict[str, tuple[str, list, str | None]] = {}  # id -> (name, child_elements, doc)
        self.groups_by_id: dict[str, tuple[str, str, list, str | None]] = {}  # id -> (name, numInGroup_field_id, child_elements, doc)
        self.messages: list[tuple[str, str, str, list, str | None]] = []  # [(name, msgType, category, structure_elements, doc)]
        self.categories: dict[str, str] = {}  # category name -> section

        self._load()

    def _load(self) -> None:
        self._load_categories()
        self._load_datatypes()
        self._load_codesets()
        self._load_fields()
        self._load_components()
        self._load_groups()
        self._load_messages()

    def _load_categories(self) -> None:
        for cat in self.root.iter(f"{{{FIXR_NS}}}category"):
            name = cat.get("name", "")
            section = cat.get("section", "")
            if name:
                self.categories[name] = section

    def _load_datatypes(self) -> None:
        for dt in self.root.iter(f"{{{FIXR_NS}}}datatype"):
            name = dt.get("name", "")
            base = dt.get("baseType")
            if name:
                self.datatypes[name] = base

    def _load_codesets(self) -> None:
        for cs in self.root.iter(f"{{{FIXR_NS}}}codeSet"):
            cs_name = cs.get("name", "")
            cs_id = cs.get("id", "")
            cs_type = cs.get("type", "")
            codes: list[tuple[str, str, int, str | None]] = []
            for code in cs.findall(f"{{{FIXR_NS}}}code"):
                value = code.get("value", "")
                code_name = code.get("name", "")
                sort_str = code.get("sort", "0")
                try:
                    sort_val = int(sort_str)
                except ValueError:
                    sort_val = 0
                if code_name:
                    codes.append((value, code_name, sort_val, extract_doc(code)))
            if cs_name:
                self.codesets[cs_name] = (cs_type, codes)
            if cs_id:
                self.codeset_by_id[cs_id] = cs_name

    def _load_fields(self) -> None:
        for f in self.root.iter(f"{{{FIXR_NS}}}field"):
            fid = f.get("id", "")
            fname = f.get("name", "")
            ftype = f.get("type", "")
            if fid and fname:
                self.fields_by_id[fid] = (fname, ftype, extract_doc(f))
                self.fields_by_name[fname] = fid

    def _load_components(self) -> None:
        for comp in self.root.iter(f"{{{FIXR_NS}}}component"):
            cid = comp.get("id", "")
            cname = comp.get("name", "")
            if cid and cname:
                children = self._parse_member_refs(comp)
                self.components_by_id[cid] = (cname, children, extract_doc(comp))

    def _load_groups(self) -> None:
        for grp in self.root.iter(f"{{{FIXR_NS}}}group"):
            gid = grp.get("id", "")
            gname = grp.get("name", "")
            num_elem = grp.find(f"{{{FIXR_NS}}}numInGroup")
            num_id = num_elem.get("id", "") if num_elem is not None else ""
            if gid and gname:
                children = self._parse_member_refs(grp)
                self.groups_by_id[gid] = (gname, num_id, children, extract_doc(grp))

    def _load_messages(self) -> None:
        for msg in self.root.iter(f"{{{FIXR_NS}}}message"):
            mname = msg.get("name", "")
            mtype = msg.get("msgType", "")
            mcat = msg.get("category", "")
            structure = msg.find(f"{{{FIXR_NS}}}structure")
            children = self._parse_member_refs(structure) if structure is not None else []
            if mname and mtype:
                self.messages.append((mname, mtype, mcat, children, extract_doc(msg)))

    def _parse_member_refs(self, parent: ET.Element) -> list[tuple[str, str, str]]:
        """Parse fieldRef/componentRef/groupRef children of an element.

        Returns list of (kind, id, presence) tuples.
        kind is one of: "field", "component", "group"
        """
        members: list[tuple[str, str, str]] = []
        for child in parent:
            tag = child.tag
            if tag == f"{{{FIXR_NS}}}fieldRef":
                members.append(("field", child.get("id", ""), child.get("presence", "")))
            elif tag == f"{{{FIXR_NS}}}componentRef":
                members.append(("component", child.get("id", ""), child.get("presence", "")))
            elif tag == f"{{{FIXR_NS}}}groupRef":
                members.append(("group", child.get("id", ""), child.get("presence", "")))
        return members


# ---------------------------------------------------------------------------
# Resolver (Orchestra -> Dictionary)
# ---------------------------------------------------------------------------

class OrchestraResolver:
    """Transforms OrchestraRepository data into a Dictionary."""

    def __init__(self, repo: OrchestraRepository):
        self.repo = repo
        self._type_cache: dict[str, str] = {}
        self._std_header_id: str | None = None
        self._std_trailer_id: str | None = None

        # Find StandardHeader and StandardTrailer component IDs
        for cid, (cname, _, _) in self.repo.components_by_id.items():
            if cname == "StandardHeader":
                self._std_header_id = cid
            elif cname == "StandardTrailer":
                self._std_trailer_id = cid

    def _resolve_datatype(self, type_name: str) -> str:
        """Resolve a type name to an easyfix type string.

        Handles:
        - Direct datatype names (e.g., "int" -> "INT")
        - CodeSet references (e.g., "AdvSideCodeSet" -> base type of that codeSet)
        - baseType chains (e.g., "Tenor" -> "Pattern" -> "String" -> "STRING")
        """
        if type_name in self._type_cache:
            return self._type_cache[type_name]

        # Direct lookup in TYPE_MAP
        if type_name in TYPE_MAP:
            result = TYPE_MAP[type_name]
            self._type_cache[type_name] = result
            return result

        # Check if it's a codeSet name
        if type_name in self.repo.codesets:
            base_type = self.repo.codesets[type_name][0]
            result = self._resolve_datatype(base_type)
            self._type_cache[type_name] = result
            return result

        # Check if it's a datatype with baseType chain
        if type_name in self.repo.datatypes:
            base = self.repo.datatypes[type_name]
            if base is not None:
                result = self._resolve_datatype(base)
            else:
                result = type_name.upper()
            self._type_cache[type_name] = result
            return result

        # Fallback
        result = type_name.upper()
        self._type_cache[type_name] = result
        return result

    def _is_codeset(self, type_name: str) -> bool:
        return type_name in self.repo.codesets

    def _presence_to_required(self, presence: str) -> str:
        return "Y" if presence == "required" else "N"

    def _resolve_members(self, refs: list[tuple[str, str, str]],
                         skip_standard: bool = False) -> list[Member]:
        """Convert a list of (kind, id, presence) refs to Member objects.

        Group references are emitted as ComponentRef pointing to the group's
        component wrapper (the group name). The actual group content is emitted
        in the components section.
        """
        members: list[Member] = []
        for kind, ref_id, presence in refs:
            required = self._presence_to_required(presence)
            if kind == "field":
                field_info = self.repo.fields_by_id.get(ref_id)
                if field_info:
                    members.append(FieldMember(name=field_info[0], required=required))
            elif kind == "component":
                if skip_standard and ref_id in (self._std_header_id, self._std_trailer_id):
                    continue
                comp_info = self.repo.components_by_id.get(ref_id)
                if comp_info:
                    members.append(ComponentRef(name=comp_info[0], required=required))
            elif kind == "group":
                group_info = self.repo.groups_by_id.get(ref_id)
                if group_info:
                    gname = group_info[0]
                    members.append(ComponentRef(name=gname, required=required))
        return members

    def _resolve_group_as_component(self, gid: str) -> ComponentDef | None:
        """Resolve an Orchestra group into a ComponentDef wrapping a GroupMember."""
        group_info = self.repo.groups_by_id.get(gid)
        if not group_info:
            return None
        gname, num_id, grp_children, group_doc = group_info

        counter_info = self.repo.fields_by_id.get(num_id)
        counter_name = counter_info[0] if counter_info else gname

        group_members = self._resolve_members(grp_children)
        group_member = GroupMember(
            name=counter_name,
            required="N",
            members=group_members,
        )
        return ComponentDef(name=gname, members=[group_member], doc=group_doc)

    def _resolve_header(self) -> list[Member]:
        """Resolve StandardHeader into member list, inlining groups."""
        if self._std_header_id is None:
            return []
        comp_info = self.repo.components_by_id.get(self._std_header_id)
        if not comp_info:
            return []
        _, refs, _ = comp_info

        members: list[Member] = []
        for kind, ref_id, presence in refs:
            required = self._presence_to_required(presence)
            if kind == "field":
                field_info = self.repo.fields_by_id.get(ref_id)
                if field_info:
                    members.append(FieldMember(name=field_info[0], required=required))
            elif kind == "group":
                # Inline groups into header
                group_info = self.repo.groups_by_id.get(ref_id)
                if group_info:
                    gname, num_id, grp_children, group_doc = group_info
                    counter_info = self.repo.fields_by_id.get(num_id)
                    counter_name = counter_info[0] if counter_info else gname
                    group_members = self._resolve_members(grp_children)
                    members.append(GroupMember(
                        name=counter_name,
                        required=required,
                        members=group_members,
                        doc=group_doc,
                    ))
            elif kind == "component":
                comp = self.repo.components_by_id.get(ref_id)
                if comp:
                    members.append(ComponentRef(name=comp[0], required=required))
        return members

    def _resolve_trailer(self) -> list[Member]:
        """Resolve StandardTrailer into member list."""
        if self._std_trailer_id is None:
            return []
        comp_info = self.repo.components_by_id.get(self._std_trailer_id)
        if not comp_info:
            return []
        _, refs, _ = comp_info
        return self._resolve_members(refs)

    def _collect_referenced_field_ids(
        self,
        messages: list[MessageDef],
        components: list[ComponentDef],
        header: list[Member],
        trailer: list[Member],
    ) -> set[str]:
        """Collect all field IDs referenced anywhere."""
        ids: set[str] = set()

        def _collect(members: list[Member]) -> None:
            for m in members:
                if isinstance(m, FieldMember):
                    fid = self.repo.fields_by_name.get(m.name)
                    if fid:
                        ids.add(fid)
                elif isinstance(m, GroupMember):
                    fid = self.repo.fields_by_name.get(m.name)
                    if fid:
                        ids.add(fid)
                    _collect(m.members)

        for msg in messages:
            _collect(msg.members)
        for comp in components:
            _collect(comp.members)
        _collect(header)
        _collect(trailer)
        return ids

    def _build_msgtype_to_name_map(self) -> dict[str, str]:
        """Build a mapping from msgType value to message name.

        This is needed because the MsgType codeSet code names in Orchestra
        don't always match the message names (e.g., code "ExecutionAck" vs
        message "ExecutionAcknowledgement"). The easyfix code generator
        expects MsgType enum descriptions to match message names.
        """
        mapping: dict[str, str] = {}
        for mname, mtype, _, _, _ in self.repo.messages:
            mapping[mtype] = camel_to_upper_snake(mname)
        return mapping

    def _build_field_defs(self, referenced_ids: set[str],
                          messages: list[MessageDef]) -> list[FieldDef]:
        """Build FieldDef list from referenced field IDs."""
        msgtype_map = self._build_msgtype_to_name_map()

        fields: list[FieldDef] = []
        for fid in sorted(referenced_ids, key=lambda x: int(x)):
            field_info = self.repo.fields_by_id.get(fid)
            if not field_info:
                continue
            fname, ftype, fdoc = field_info
            tag = int(fid)

            mapped_type = self._resolve_datatype(ftype)

            # Collect enum values if field type is a codeSet
            values: list[EnumValue] = []
            if self._is_codeset(ftype) and fname not in STRIP_ENUM_FIELDS:
                _, codes = self.repo.codesets[ftype]
                codes_sorted = sorted(codes, key=lambda c: (c[2], c[0]))
                seen_descriptions: set[str] = set()
                for value, code_name, _, code_doc in codes_sorted:
                    # For MsgType (tag 35), use message names as descriptions
                    # to ensure the generated enum matches message struct names.
                    if fname == "MsgType" and value in msgtype_map:
                        desc = msgtype_map[value]
                    else:
                        desc = camel_to_upper_snake(code_name)
                    if desc in seen_descriptions:
                        continue
                    seen_descriptions.add(desc)
                    # Skip docs that add nothing over the code name
                    if code_doc and is_trivial_enum_doc(code_doc, code_name):
                        code_doc = None
                    values.append(EnumValue(
                        enum=value,
                        description=desc,
                        doc=code_doc,
                    ))

            fields.append(FieldDef(number=tag, name=fname, type=mapped_type,
                                   values=values, doc=fdoc))

        # Ensure MsgType field has values for all messages in this dictionary.
        # For the app layer (FIX 5.0+), the MsgType field may not have been
        # referenced directly, but it needs to contain values for each message.
        msgtype_field = None
        for f in fields:
            if f.name == "MsgType":
                msgtype_field = f
                break

        if msgtype_field is None and messages:
            # MsgType not referenced at all - add it with message-derived values
            msgtype_values = []
            seen: set[str] = set()
            for msg in messages:
                desc = camel_to_upper_snake(msg.name)
                if desc not in seen:
                    seen.add(desc)
                    msgtype_values.append(EnumValue(enum=msg.msgtype, description=desc))
            fields.append(FieldDef(number=35, name="MsgType", type="STRING", values=msgtype_values))
            fields.sort(key=lambda f: f.number)
        elif msgtype_field is not None and messages:
            # MsgType exists - ensure it has values for all messages in this dict
            existing_enums = {v.enum for v in msgtype_field.values}
            for msg in messages:
                if msg.msgtype not in existing_enums:
                    desc = camel_to_upper_snake(msg.name)
                    msgtype_field.values.append(EnumValue(enum=msg.msgtype, description=desc))
                    existing_enums.add(msg.msgtype)

        return fields

    def _parse_version(self) -> tuple[str, int, int, int]:
        """Parse repository name/version into (fix_type, major, minor, servicepack)."""
        name = self.repo.repo_name
        version = self.repo.repo_version

        if name.startswith("FIXT") or "FIXT" in name:
            fix_type = "FIXT"
            # Try to extract version from the version string like "FIX.5.0SP2_EP247"
            # For FIXT, we want FIXT 1.1
            m = re.search(r"FIXT\.?(\d+)\.(\d+)", name + version)
            if m:
                return fix_type, int(m.group(1)), int(m.group(2)), 0
            # Fallback for FIXT session layer
            return fix_type, 1, 1, 0

        if "Latest" in name or "Latest" in version:
            return "FIX", 5, 0, 2

        # Parse version like "FIX.4.4" or "FIX.5.0SP2"
        m = re.search(r"FIX\.?(\d+)\.(\d+)(?:SP(\d+))?", version)
        if m:
            major = int(m.group(1))
            minor = int(m.group(2))
            sp = int(m.group(3)) if m.group(3) else 0
            return "FIX", major, minor, sp

        m = re.search(r"FIX\.?(\d+)\.(\d+)(?:SP(\d+))?", name)
        if m:
            major = int(m.group(1))
            minor = int(m.group(2))
            sp = int(m.group(3)) if m.group(3) else 0
            return "FIX", major, minor, sp

        return "FIX", 5, 0, 2

    def resolve(self) -> Dictionary:
        """Main entry point: resolve Orchestra into Dictionary."""
        fix_type, major, minor, servicepack = self._parse_version()
        is_fixt = fix_type == "FIXT"
        is_standalone = not is_fixt and major < 5

        # Header and trailer - needed for FIXT and standalone FIX (4.x)
        header: list[Member] = []
        trailer: list[Member] = []
        if is_fixt or is_standalone:
            header = self._resolve_header()
            trailer = self._resolve_trailer()

        # Identify header-inlined groups (for excluding from components list)
        header_inlined_group_ids: set[str] = set()
        if (is_fixt or is_standalone) and self._std_header_id:
            comp_info = self.repo.components_by_id.get(self._std_header_id)
            if comp_info:
                for kind, ref_id, _ in comp_info[1]:
                    if kind == "group":
                        header_inlined_group_ids.add(ref_id)

        # Build messages
        # For FIX 5.0+ app layer (non-FIXT, non-standalone): skip session messages
        # For FIXT and standalone FIX 4.x: include all messages
        messages: list[MessageDef] = []
        for mname, mtype, mcat, structure_refs, mdoc in self.repo.messages:
            category_section = self.repo.categories.get(mcat, "")
            is_session = mcat == "Session" or category_section == "Session"

            if not is_fixt and not is_standalone and is_session:
                continue

            msgcat = "admin" if is_session else "app"
            members = self._resolve_members(structure_refs, skip_standard=True)
            messages.append(MessageDef(
                name=mname, msgtype=mtype,
                msgcat=msgcat, members=members,
                doc=mdoc,
            ))

        # Remove messages with no body members
        messages = [m for m in messages if m.members]
        messages.sort(key=lambda m: msg_type_sort_key(m.msgtype))

        # Build components (exclude StandardHeader/StandardTrailer)
        components: list[ComponentDef] = []
        for cid, (cname, refs, cdoc) in self.repo.components_by_id.items():
            if cname in ("StandardHeader", "StandardTrailer"):
                continue
            members = self._resolve_members(refs)
            components.append(ComponentDef(name=cname, members=members, doc=cdoc))

        # Add groups as components (each group becomes a component wrapping a GroupMember)
        for gid in self.repo.groups_by_id:
            comp = self._resolve_group_as_component(gid)
            if comp:
                components.append(comp)

        # Drop components nothing references, then collect field IDs only
        # from what remains so unused fields are not emitted either
        components = prune_unused_components(components, messages, header, trailer)
        kept_component_names = {c.name for c in components}

        # Collect referenced field IDs
        referenced_ids = self._collect_referenced_field_ids(messages, components, header, trailer)

        # Also collect counter field IDs for kept groups (their counter field
        # may resolve only through the group definition)
        for gid, (gname, num_id, children, _gdoc) in self.repo.groups_by_id.items():
            if gname not in kept_component_names:
                continue
            if num_id:
                referenced_ids.add(num_id)
            for kind, ref_id, _ in children:
                if kind == "field":
                    referenced_ids.add(ref_id)

        # Build field definitions
        fields = self._build_field_defs(referenced_ids, messages)

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
# Serialization (Dictionary -> XML) - same as repo_to_easyfix.py
# ---------------------------------------------------------------------------

def _set_doc(el: ET.Element, doc: str | None, include_docs: bool) -> None:
    """Set the doc attribute when docs are enabled and text is present."""
    if include_docs and doc:
        el.set("doc", doc)


def _serialize_members(parent: ET.Element, members: list[Member],
                       include_docs: bool = False) -> None:
    """Recursively add member elements to an ET parent."""
    for m in members:
        if isinstance(m, FieldMember):
            ET.SubElement(parent, "field", name=m.name, required=m.required)
        elif isinstance(m, ComponentRef):
            ET.SubElement(parent, "component", name=rename_component(m.name), required=m.required)
        elif isinstance(m, GroupMember):
            group_el = ET.SubElement(parent, "group", name=m.name, required=m.required)
            _set_doc(group_el, m.doc, include_docs)
            _serialize_members(group_el, m.members, include_docs)


def serialize(dictionary: Dictionary, include_docs: bool = False) -> str:
    """Serialize a Dictionary to XML string."""
    root = ET.Element("fix",
                       type=dictionary.fix_type,
                       major=str(dictionary.major),
                       minor=str(dictionary.minor),
                       servicepack=str(dictionary.servicepack))

    # Header
    header_el = ET.SubElement(root, "header")
    if dictionary.header:
        _serialize_members(header_el, dictionary.header, include_docs)

    # Messages
    messages_el = ET.SubElement(root, "messages")
    for msg in dictionary.messages:
        msg_el = ET.SubElement(messages_el, "message",
                               name=msg.name, msgtype=msg.msgtype, msgcat=msg.msgcat)
        _set_doc(msg_el, msg.doc, include_docs)
        _serialize_members(msg_el, msg.members, include_docs)

    # Trailer
    trailer_el = ET.SubElement(root, "trailer")
    if dictionary.trailer:
        _serialize_members(trailer_el, dictionary.trailer, include_docs)

    # Components
    components_el = ET.SubElement(root, "components")
    for comp in dictionary.components:
        comp_el = ET.SubElement(components_el, "component", name=rename_component(comp.name))
        _set_doc(comp_el, comp.doc, include_docs)
        _serialize_members(comp_el, comp.members, include_docs)

    # Fields
    fields_el = ET.SubElement(root, "fields")
    for f in dictionary.fields:
        field_el = ET.SubElement(fields_el, "field",
                                  number=str(f.number), name=f.name, type=f.type)
        _set_doc(field_el, f.doc, include_docs)
        for v in f.values:
            value_el = ET.SubElement(field_el, "value", enum=v.enum, description=v.description)
            _set_doc(value_el, v.doc, include_docs)

    ET.indent(root, space=" ")
    return ET.tostring(root, encoding="unicode") + "\n"


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Convert FIX Orchestra XML to easyfix dictionary XML")
    parser.add_argument("--input", required=True,
                        help="Path to Orchestra XML file")
    parser.add_argument("--output", required=True,
                        help="Output XML file path")
    parser.add_argument("--merge-app-msgtypes", default=None,
                        help="Path to app-layer Orchestra XML to merge MsgType values from "
                             "(used when generating FIXT session layer)")
    parser.add_argument("--docs", action="store_true",
                        help="Emit doc attributes with documentation from Orchestra annotations")
    args = parser.parse_args()

    print(f"Loading {args.input}...")
    repo = OrchestraRepository(args.input)
    print(f"  Repository: name={repo.repo_name}, version={repo.repo_version}")
    print(f"  Fields: {len(repo.fields_by_id)}, CodeSets: {len(repo.codesets)}, "
          f"Components: {len(repo.components_by_id)}, Groups: {len(repo.groups_by_id)}, "
          f"Messages: {len(repo.messages)}")

    print("Generating XML...")
    resolver = OrchestraResolver(repo)
    dictionary = resolver.resolve()

    # Merge MsgType values from app-layer Orchestra file
    if args.merge_app_msgtypes:
        print(f"Merging MsgType values from {args.merge_app_msgtypes}...")
        app_repo = OrchestraRepository(args.merge_app_msgtypes)
        msgtype_field = None
        for f in dictionary.fields:
            if f.name == "MsgType":
                msgtype_field = f
                break
        if msgtype_field:
            existing_enums = {v.enum for v in msgtype_field.values}
            added = 0
            for mname, mtype, _, _, _ in app_repo.messages:
                if mtype not in existing_enums:
                    desc = camel_to_upper_snake(mname)
                    msgtype_field.values.append(EnumValue(enum=mtype, description=desc))
                    existing_enums.add(mtype)
                    added += 1
            print(f"  Added {added} app-layer MsgType values")

    xml_output = serialize(dictionary, include_docs=args.docs)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w", encoding="utf-8") as f:
        f.write(xml_output)

    msg_count = len(dictionary.messages)
    field_count = len(dictionary.fields)
    group_count = sum(1 for f in dictionary.fields if f.type == "NUMINGROUP")
    comp_count = len(dictionary.components)
    print(f"Written to {args.output}")
    print(f"  Type: {dictionary.fix_type} {dictionary.major}.{dictionary.minor} SP{dictionary.servicepack}")
    print(f"  Messages: {msg_count}, Components: {comp_count}, "
          f"Fields: {field_count}, Groups: {group_count}")


if __name__ == "__main__":
    main()
