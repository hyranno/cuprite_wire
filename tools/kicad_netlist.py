"""Minimal reader for KiCad S-expression netlists (`(export (version "E") ...)`).

Shared by the netlist check scripts in this directory. Only the parts the checks
need are extracted.
"""

import re
from dataclasses import dataclass, field

TOKEN = re.compile(r'\(|\)|"(?:\\.|[^"\\])*"|[^\s()"]+')


def parse(text):
    """Parse an S-expression into nested lists of strings."""
    stack = [[]]
    for token in TOKEN.findall(text):
        if token == "(":
            stack.append([])
        elif token == ")":
            done = stack.pop()
            stack[-1].append(done)
        elif token.startswith('"'):
            stack[-1].append(re.sub(r"\\(.)", r"\1", token[1:-1]))
        else:
            stack[-1].append(token)
    return stack[0][0]


def children(node, name):
    return [child for child in node[1:] if isinstance(child, list) and child[0] == name]


def value(node, name):
    found = children(node, name)
    return found[0][1] if found else None


@dataclass
class Component:
    ref: str
    value: str
    footprint: str
    # Sheet path tstamps plus the symbol tstamp, as pcbnew stores it in a footprint's path.
    path: str
    sheet_names: str
    fields: dict  # name -> text ("" for a field without a value)
    properties: dict  # name -> value (None for a flag such as `dnp`)


@dataclass
class Node:
    net: str
    pinfunction: str  # "" if absent
    pintype: str


@dataclass
class Netlist:
    components: dict = field(default_factory=dict)  # ref -> Component
    nodes: dict = field(default_factory=dict)  # (ref, pad) -> Node


def read(path):
    root = parse(open(path, encoding="utf-8").read())
    netlist = Netlist()
    for comp in children(children(root, "components")[0], "comp"):
        ref = value(comp, "ref")
        sheet = children(comp, "sheetpath")[0]
        fields = children(comp, "fields")
        netlist.components[ref] = Component(
            ref=ref,
            value=value(comp, "value"),
            footprint=value(comp, "footprint"),
            path=value(sheet, "tstamps") + value(comp, "tstamps"),
            sheet_names=value(sheet, "names"),
            fields={value(f, "name"): (f[2] if len(f) > 2 else "")
                    for f in (children(fields[0], "field") if fields else [])},
            properties={value(p, "name"): value(p, "value") for p in children(comp, "property")},
        )
    for net in children(children(root, "nets")[0], "net"):
        for node in children(net, "node"):
            netlist.nodes[(value(node, "ref"), value(node, "pin"))] = Node(
                net=value(net, "name"),
                pinfunction=value(node, "pinfunction") or "",
                pintype=value(node, "pintype") or "",
            )
    return netlist
