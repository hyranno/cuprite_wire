#!/usr/bin/env python3
"""Compare a netlist with what KiCad's own exporter writes for the same circuit.

For each netlist, an equivalent schematic is synthesized: one symbol per component
(pins, pin names and types from the netlist nodes; value, footprint, fields and
flags from the component; the component's tstamps as the symbol's unique id),
connected by wires, with a global label for every global net name and a local
label (in its sheet) for every local one. Unnamed nets get no label, so KiCad names them itself.
A pin whose type has the suffix `+no_connect` gets a no-connect flag.
`kicad-cli sch export netlist --format kicadsexpr` then exports the schematic, and
both netlists are compared after removing what cuprite_wire intentionally omits
(see KNOWN_DIFFERENCES, and KICAD10_KNOWN_DIFFERENCES with KiCad 10). The remaining
differences in element order, quoting and omission rules are printed as a diff.

Every sheet path becomes a sheet file, with the sheet's unique id from the
netlist; sheets are numbered as pages in the order they appear in the netlist.
A net with a local name reaching other sheets is connected through hierarchical
labels and sheet pins (named `N<k>`, so they never name the net: Eeschema
prefers local labels). Eeschema cannot connect sheets without a label, so an
unnamed net crossing sheets is connected by global labels named after it, and
its name is reported as not compared. Only the label that named a net is
placed, so which of several labels names a net is not checked.

With --spice, the SPICE deck written for the same circuit (NETLIST with the suffix
`.cir`) is compared with `kicad-cli sch export netlist --format spice` instead. Every symbol
gets Sim.* fields from its element line in the deck (Sim.Pins maps the pads to the element's
nodes by their nets), parts without an element are excluded from the simulation, and inline
`.subckt`/`.model` definitions are written to a library for Sim.Library. Element lines are
compared after mapping KiCad's ground `GND` to `0`; their order is reported separately.
Only R, C, L and X (with an inline `.subckt`) elements are supported.

With --library-symbols, the components use their library symbols (`libsource`, looked up in
`libs/symbols` and the installed KiCad libraries) instead of synthesized ones, with one symbol
instance per unit, and every pin is connected by a label with its net's name placed on the pin
(single-pin nets of automatic names get none, so KiCad names them). This checks the pin numbers,
names and types of parts generated from the libraries by `kicad_part!`. With --spice as well,
the symbols keep their own Sim.* fields, so the elements KiCad derives from them are compared
with the ones cuprite_wire derived; built-in model names (`__<name>`) are compared as `__model`.
Only netlists of a single sheet are supported. Sim.* fields of a component in the netlist (written
for the `sim` block of `kicad_part!`) replace those of its symbol, so KiCad derives the element from
the mapping cuprite_wire wrote; `${CARGO_MANIFEST_DIR}` in them is this repository.
--symbol-dir adds a directory of symbol libraries (e.g. `crates/kicad_lib/tests/fixtures/symbols`).

Usage: python3 tools/compare_kicad_cli_netlist.py [--keep DIR] [--spice] [--library-symbols]
       [--symbol-dir DIR]... NETLIST...
"""

import argparse
import difflib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

import kicad_netlist
from kicad_netlist import (
    children, dump, kicad_version, parse, project_symbol_libs, quote, read, symbol_lib_path, unquote,
    value,
)

# Items KiCad writes and cuprite_wire intentionally does not (doc/kicad_netlist_usage.md section 4).
KNOWN_DIFFERENCES = {
    "design": "only `source` and `tool` are written; both differ by design",
    "libparts / libraries": "not written",
    "fields Footprint / Datasheet / Description": "not written",
    "libsource description": "not written",
    "libsource of components without one": "not written",
    "properties Sheetname / Sheetfile / ki_*": "not written",
    "properties repeating fields": "not written; pcbnew reads the fields",
}
DROPPED_PROPERTIES = re.compile(r'"(Sheetname|Sheetfile|ki_[^"]*)"')
DROPPED_FIELDS = {'"Footprint"', '"Datasheet"', '"Description"'}
# With --library-symbols: the simulation model is written to the SPICE deck, not to the .net.
LIBRARY_KNOWN_DIFFERENCES = {
    "Sim.* fields and properties": "not written to the .net (except for the `sim` block of kicad_part!)",
    "tstamps of units 2..": "multi-unit components have one unique id (one symbol instance)",
    "ki_* fields": "not written",
}
SIM_PROPERTY = re.compile(r'"Sim[._][^"]*"')
# KiCad 10 writes a newer form of the format; cuprite_wire writes the KiCad 9 form, which KiCad 10 reads.
KICAD10_KNOWN_DIFFERENCES = {
    "units of components": "not written (KiCad 10)",
    "empty groups / variants": "not written (KiCad 10)",
    "pin number suffix of pinfunction": "the pin name only, as KiCad 9 writes it (KiCad 10 appends `_<pin>`)",
}
# KiCad appends it to the pin type of a pin alone on a net with a no-connect flag.
NO_CONNECT_SUFFIX = "+no_connect"
SYMBOL_DIR_VARIABLE = re.compile(r"\$\{KICAD\d+_SYMBOL_DIR\}")

PIN_PITCH = 2.54
COMPONENT_PITCH = 60.96
PIN_Y = 101.6  # every pin end is on this line
HUB_Y = 25.4  # every net's hub is on this line


def kicad_env(root):
    return {
        # Golden netlists write paths in this repository as `${CARGO_MANIFEST_DIR}/...`.
        "CARGO_MANIFEST_DIR": str(Path(__file__).resolve().parent.parent),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "data"),
        "HOME": str(root / "home"),
    }


def mm(v):
    """A coordinate in mm, exact to 0.1 um (`:g` would round large values)."""
    return f"{v:.4f}".rstrip("0").rstrip(".")


def new_uuid():
    return str(uuid.uuid4())


def prop(name, text, at, hidden=True):
    hide = " hide" if hidden else ""
    return (f'(property {quote(name)} {quote(text)} (at {mm(at[0])} {mm(at[1])} 0) '
            f"(effects (font (size 1.27 1.27)){hide}))")


class Sheet:
    """One sheet instance of the synthesized schematic (a separate file for every instance)."""

    def __init__(self, names, tstamps, file):
        self.names = names  # sheet names from the root, e.g. ("stage1",)
        self.tstamps = tstamps  # sheet unique ids from the root
        self.file = file
        self.page = None  # set by synthesize()
        self.uuid = new_uuid()  # of the file
        self.components = []  # references
        self.children = []  # Sheets
        self.sheet_pins = []  # hierarchical label names, as pins on this sheet's symbol
        self.hubs = {}  # net index -> [end key]
        self.labels = []  # (kind, text, net index)


def no_connect_line(x, y):
    return f"(no_connect (at {mm(x)} {mm(y)}) (uuid {quote(new_uuid())}))"


def symbol_lines(lib_id, ref, signature):
    name = lib_id.split(":", 1)[1]
    prefix = re.match(r"[A-Za-z#]*", ref).group() or "X"
    out = [f"(symbol {quote(lib_id)} (in_bom yes) (on_board yes)",
           prop("Reference", prefix, (0, 0), hidden=False), prop("Value", name, (0, 0)),
           f"(symbol {quote(name + '_1_1')}"]
    for i, (pad, pin_name, pin_type) in enumerate(signature):
        out.append(f"(pin {pin_type} line (at {mm(i * PIN_PITCH)} 5.08 270) (length 2.54) "
                   f"(name {quote(pin_name or '~')} (effects (font (size 1.27 1.27)))) "
                   f"(number {quote(pad)} (effects (font (size 1.27 1.27)))))")
    out.append("))")
    return out


def synthesize(path, project, sim=None):
    """Returns ({file name: text} of a schematic equivalent to the netlist at `path`, notes).

    `sim` is an optional (ref -> [(field name, text)], refs excluded from the simulation).
    """
    root_tree = parse(open(path, encoding="utf-8").read())
    netlist = read(path)
    notes = []
    # pins[ref] = [(pad, name, type)] of the netlist nodes.
    pins = {ref: [] for ref in netlist.components}
    for (ref, pad), node in netlist.nodes.items():
        pins[ref].append((pad, node.pinfunction, node.pintype.removesuffix(NO_CONNECT_SUFFIX)))
    libsources = {}
    for comp in children(children(root_tree, "components")[0], "comp"):
        source = children(comp, "libsource")
        ref = value(comp, "ref")
        libsources[ref] = (f"{value(source[0], 'lib')}:{value(source[0], 'part')}" if source
                           else f"cw:{ref}")
    # One library symbol per lib id; its pins must be the same for every component using it.
    lib_symbols = {}
    for ref, lib_id in libsources.items():
        signature = sorted(pins[ref])
        if lib_symbols.setdefault(lib_id, (ref, signature))[1] != signature:
            raise ValueError(f"{ref} and {lib_symbols[lib_id][0]} use {lib_id} with different pins")

    # Sheets, keyed by their names from the root. The unique ids come from the components'
    # sheet paths; sheets without components get new ones.
    root = Sheet((), (), "compare.kicad_sch")
    sheets = {(): root}

    def sheet(names, tstamps=None):
        if names not in sheets:
            parent = sheet(names[:-1], tstamps[:-1] if tstamps else None)
            uid = tstamps[-1] if tstamps else new_uuid()
            child = Sheet(names, parent.tstamps + (uid,), f"sheet{len(sheets)}.kicad_sch")
            sheets[names] = child
            parent.children.append(child)
        return sheets[names]

    def split(sheet_path):
        return tuple(sheet_path.strip("/").split("/")) if sheet_path.strip("/") else ()

    # Sheets are created in the order of the components, and numbered as pages in that order:
    # Eeschema writes components sheet by sheet in page order.
    component_sheet = {}
    for ref, comp in netlist.components.items():
        tstamps = split(comp.path[: -len(comp.path.rsplit("/", 1)[1])])
        s = sheet(split(comp.sheet_names), tstamps)
        s.components.append(ref)
        component_sheet[ref] = s

    # Wire every net in every sheet it reaches to a hub, and label the hub.
    nets = {}
    for (ref, pad), node in sorted(netlist.nodes.items()):
        nets.setdefault(node.net, []).append((ref, pad))
    for k, (net, nodes) in enumerate(sorted(nets.items())):
        reached = {}
        for ref, pad in nodes:
            reached.setdefault(component_sheet[ref].names, []).append(("pin", ref, pad))
        auto = net.startswith(("Net-(", "unconnected-("))
        if auto and len(nodes) == 1:
            continue
        if auto and len(reached) > 1:
            notes.append(f"{net}: crosses sheets without a name; connected by global labels "
                         "named after it, so its name is not compared")
        if not net.startswith("/"):
            for names, ends in reached.items():
                sheet(names).hubs[k] = ends
                if not auto or len(reached) > 1:
                    sheet(names).labels.append(("global_label", net, k))
            continue
        # A local name: a label in its sheet, and hierarchical labels and sheet pins up to the
        # common ancestor of all the sheets the net reaches.
        label_sheet = sheet(split(net.rsplit("/", 1)[0]))
        label_sheet.labels.append(("label", net.rsplit("/", 1)[1], k))
        reached.setdefault(label_sheet.names, [])
        top = min(len(names) for names in reached)
        while len({names[:top] for names in reached}) > 1:
            top -= 1
        for names in list(reached):
            for depth in range(len(names), top, -1):
                reached.setdefault(names[:depth], [])
        for names, ends in reached.items():
            s = sheet(names)
            s.hubs[k] = list(ends)
            if len(names) > top:
                pin_name = f"N{k}"
                s.labels.append(("hierarchical_label", pin_name, k))
                s.sheet_pins.append(pin_name)
                sheet(names[:-1]).hubs.setdefault(k, [])
        for names in reached:
            if len(names) > top:
                sheet(names[:-1]).hubs[k].append(("sheet_pin", names, f"N{k}"))

    for page, s in enumerate(sheets.values(), 1):
        s.page = page
    files = {}
    for s in sheets.values():
        files[s.file] = sheet_text(s, project, netlist, pins, libsources, lib_symbols, sim)
    return files, notes


def sheet_text(s, project, netlist, pins, libsources, lib_symbols, sim=None):
    sim_fields, sim_excluded = sim or ({}, set())
    instance = "/" + "/".join((project["uuid"],) + s.tstamps)
    out = [f'(kicad_sch (version 20231120) (generator "cuprite_wire_compare")',
           f"(uuid {quote(project['uuid'] if not s.names else s.uuid)})", '(paper "A0")',
           "(lib_symbols"]
    for lib_id in sorted({libsources[ref] for ref in s.components}):
        out += symbol_lines(lib_id, *lib_symbols[lib_id])
    out.append(")")

    # Symbols and sheet symbols in a row; pin ends and sheet pins on the line PIN_Y.
    ends = {}
    no_connects = []
    x = 25.4
    for ref in s.components:
        comp = netlist.components[ref]
        y = PIN_Y + 5.08
        flags = comp.properties
        out.append(f"(symbol (lib_id {quote(libsources[ref])}) (at {mm(x)} {mm(y)} 0) (unit 1) "
                   f"(exclude_from_sim {'yes' if ref in sim_excluded else 'no'}) "
                   f"(in_bom {'no' if 'exclude_from_bom' in flags else 'yes'}) "
                   f"(on_board {'no' if 'exclude_from_board' in flags else 'yes'}) "
                   f"(dnp {'yes' if 'dnp' in flags else 'no'}) "
                   f"(uuid {quote(comp.path.rsplit('/', 1)[1])})")
        out.append(prop("Reference", ref, (x, y + 5), hidden=False))
        out.append(prop("Value", comp.value, (x, y + 7.5)))
        out.append(prop("Footprint", comp.footprint, (x, y)))
        out.append(prop("Datasheet", comp.datasheet, (x, y)))
        out.append(prop("Description", comp.description, (x, y)))
        for field_name, text in [*comp.fields.items(), *sim_fields.get(ref, [])]:
            out.append(prop(field_name, text, (x, y)))
        for i, (pad, _, _) in enumerate(sorted(pins[ref])):
            ends[("pin", ref, pad)] = (x + i * PIN_PITCH, PIN_Y)
            out.append(f"(pin {quote(pad)} (uuid {quote(new_uuid())}))")
            if netlist.nodes[(ref, pad)].pintype.endswith(NO_CONNECT_SUFFIX):
                no_connects.append(no_connect_line(*ends[("pin", ref, pad)]))
        out.append(f"(instances (project {quote(project['name'])} "
                   f"(path {quote(instance)} (reference {quote(ref)}) (unit 1)))))")
        x += COMPONENT_PITCH
    for child in s.children:
        width = (len(child.sheet_pins) + 1) * PIN_PITCH
        out.append(f"(sheet (at {mm(x)} {mm(PIN_Y)}) (size {mm(width)} 10.16) "
                   f"(stroke (width 0.1524) (type solid)) (fill (color 0 0 0 0.0000)) "
                   f"(uuid {quote(child.tstamps[-1])})")
        out.append(prop("Sheetname", child.names[-1], (x, PIN_Y - 1), hidden=False))
        out.append(prop("Sheetfile", child.file, (x, PIN_Y + 11)))
        for i, pin_name in enumerate(child.sheet_pins):
            end = (x + (i + 1) * PIN_PITCH, PIN_Y)
            ends[("sheet_pin", child.names, pin_name)] = end
            out.append(f"(pin {quote(pin_name)} passive (at {mm(end[0])} {mm(end[1])} 90) "
                       f"(effects (font (size 1.27 1.27))) (uuid {quote(new_uuid())}))")
        out.append(f"(instances (project {quote(project['name'])} "
                   f"(path {quote(instance)} (page {quote(str(child.page))})))))")
        x += width + COMPONENT_PITCH
    out += no_connects

    # Every net gets a hub on the line HUB_Y, wired to the ends on the line PIN_Y. All wire
    # ends are on these two lines, so none lies inside another wire and connects to it.
    hub = {k: (25.4 + k * PIN_PITCH, HUB_Y) for k in s.hubs}
    for k, keys in s.hubs.items():
        for key in keys:
            end = ends[key]
            out.append(f"(wire (pts (xy {mm(end[0])} {mm(end[1])}) (xy {mm(hub[k][0])} {mm(hub[k][1])})) "
                       f"(stroke (width 0) (type default)) (uuid {quote(new_uuid())}))")
    effects = "(effects (font (size 1.27 1.27)) (justify left bottom))"
    for kind, text, k in s.labels:
        shape = "" if kind == "label" else " (shape passive)"
        out.append(f"({kind} {quote(text)}{shape} (at {mm(hub[k][0])} {mm(hub[k][1])} 90) "
                   f"{effects} (uuid {quote(new_uuid())}))")
    if not s.names:
        out.append('(sheet_instances (path "/" (page "1")))')
    out.append(")")
    return "\n".join(out) + "\n"


def normalize(tree, ours, library_symbols=False):
    """Drops what is listed in KNOWN_DIFFERENCES (and LIBRARY_KNOWN_DIFFERENCES with
    `library_symbols`) from KiCad's netlist (`tree`)."""
    our_comps = {value(c, "ref"): c for c in children(children(ours, "components")[0], "comp")}
    has_libsource = {ref: bool(children(c, "libsource")) for ref, c in our_comps.items()}
    kicad10 = kicad_version()[0] >= 10
    tree = [item for item in tree
            if not (isinstance(item, list) and (item[0] in ("design", "libparts", "libraries")
                                                or kicad10 and item[0] in ("groups", "variants")
                                                and len(item) == 1))]
    if kicad10:
        for net in children(children(tree, "nets")[0], "net"):
            for node in children(net, "node"):
                for item in children(node, "pinfunction"):
                    suffix = "_" + unquote(value(node, "pin"))
                    if unquote(item[1]).endswith(suffix):
                        item[1] = quote(unquote(item[1])[:-len(suffix)])
    for comp in children(children(tree, "components")[0], "comp"):
        ref = value(comp, "ref")
        field_names = {value(f, "name") for fields in children(comp, "fields") for f in fields[1:]}
        # Sim.* fields cuprite_wire wrote (the `sim` block) are compared.
        our_fields = {value(f, "name") for fields in children(our_comps.get(ref, []), "fields") for f in fields[1:]}
        kept = []
        for item in comp[1:]:
            if not isinstance(item, list):
                kept.append(item)
            elif item[0] == "fields":
                fields = [f for f in item[1:] if value(f, "name") not in DROPPED_FIELDS
                          and not (library_symbols and (SIM_PROPERTY.fullmatch(value(f, "name") or "")
                                                        and value(f, "name") not in our_fields
                                                        or (value(f, "name") or "").startswith('"ki_')))]
                if fields:
                    kept.append(["fields", *fields])
            elif item[0] == "libsource":
                if has_libsource.get(ref):
                    kept.append([i for i in item if not (isinstance(i, list) and i[0] == "description")])
            elif library_symbols and item[0] == "tstamps":
                # The unit sharing the unique id of our component (KiCad sorts them).
                ours_tstamps = children(our_comps[ref], "tstamps") if ref in our_comps else []
                mine = ours_tstamps[0][1] if ours_tstamps else None
                kept.append([item[0], mine if mine in item[1:] else item[1]])
            elif kicad10 and item[0] == "units":
                pass
            elif item[0] == "property" and library_symbols and SIM_PROPERTY.fullmatch(value(item, "name") or ""):
                pass
            elif item[0] == "property" and (DROPPED_PROPERTIES.fullmatch(value(item, "name") or "")
                                            or value(item, "name") in field_names
                                            and value(item, "value") is not None):
                pass
            else:
                kept.append(item)
        comp[1:] = kept
    return tree


def lines(tree, depth=0):
    """One line per element with child elements, so that a diff shows order and quoting.

    Atoms and leaf elements such as (ref "R1") stay on the line of their parent.
    """
    result = ["  " * depth + f"({tree[0]}"]
    for item in tree[1:]:
        if not isinstance(item, list):
            result[-1] += f" {item}"
        elif all(not isinstance(i, list) for i in item):
            result[-1] += " (" + " ".join(item) + ")"
        else:
            result += lines(item, depth + 1)
    return result


def compare(path, work, env, library_symbols=False):
    project = {"name": "compare", "uuid": new_uuid()}
    files, notes = (synthesize_library(path, project) if library_symbols
                    else synthesize(path, project))
    for name, text in files.items():
        (work / name).write_text(text)
    schematic = work / "compare.kicad_sch"
    exported = work / "kicad.net"
    run = subprocess.run(["kicad-cli", "sch", "export", "netlist", "--format", "kicadsexpr",
                          "-o", str(exported), str(schematic)],
                         env={**os.environ, **env}, capture_output=True, text=True)
    if run.returncode or not exported.exists():
        return [f"kicad-cli failed: {run.stdout}{run.stderr}"], notes
    ours = parse(Path(path).read_text(encoding="utf-8"), raw=True)
    theirs = normalize(parse(exported.read_text(encoding="utf-8"), raw=True), ours, library_symbols)
    ours = [item for item in ours if not (isinstance(item, list) and item[0] == "design")]
    return list(difflib.unified_diff(lines(ours), lines(theirs), "cuprite_wire", "kicad-cli",
                                     lineterm="")), notes


def spice_node_name(net):
    """The SPICE node name of a net, by the rule of `spice_node_name` in src/spice/netlist.rs."""
    name = re.sub(r"[%(),\[\]<>~\s]", "_", net.replace("{slash}", "/"))
    if name == "0" or name.lower() == "gnd":
        return "0"
    return "/root/" + name[2:] if name.startswith("//") else name


def read_deck(path):
    """Returns (element lines as token lists, inline definitions, {subckt name: ports})."""
    elements, models, subckts, block = [], [], {}, None
    for line in Path(path).read_text(encoding="utf-8").splitlines()[1:]:  # the first line is the title
        tokens = line.split()
        # Continuation lines ('+') belong to the `.model` lines KiCad writes for built-in models.
        if not tokens or line.startswith(("*", "+")):
            continue
        keyword = tokens[0].lower()
        if block is not None:
            block.append(line)
            if keyword == ".ends":
                models.append("\n".join(block))
                block = None
        elif keyword == ".subckt":
            block = [line]
            subckts[tokens[1].lower()] = tokens[2:]
        elif keyword == ".model":
            models.append(line)
        elif not keyword.startswith("."):
            elements.append(tokens)
    return elements, models, subckts


def sim_fields(netlist, elements, subckts, library):
    """Sim.* fields reproducing the elements. Returns ((fields by ref, excluded refs), compared elements)."""
    fields, compared = {}, []
    for tokens in elements:
        name, device = tokens[0], tokens[0][0].upper()
        ref = name if name in netlist.components else name[1:]
        if ref not in netlist.components:
            raise ValueError(f"{name}: no component in the .net (a simulation-only part)")
        if ref in fields:
            raise ValueError(f"{ref}: several elements (multi-unit symbols are not synthesized)")
        if device in "RCL":
            nodes, ports = tokens[1:3], ["+", "-"]
            model = [("Sim.Device", device), ("Sim.Params", f"{device.lower()}={' '.join(tokens[3:])}")]
        elif device == "X":
            at = next((i for i, t in enumerate(tokens) if t.lower() in subckts), None)
            if at is None:
                raise ValueError(f"{name}: the subcircuit is not defined in the deck")
            nodes, ports = tokens[1:at], subckts[tokens[at].lower()]
            if tokens[at + 1:]:
                raise ValueError(f"{name}: subcircuit parameters are not supported")
            model = [("Sim.Device", "SUBCKT"), ("Sim.Library", str(library)), ("Sim.Name", tokens[at])]
        else:
            raise ValueError(f"{name}: device {device} is not supported")
        # Assign each node to a pad on that net.
        free = sorted((pad, spice_node_name(node.net)) for (r, pad), node in netlist.nodes.items() if r == ref)
        mapping = []
        for node, port in zip(nodes, ports):
            pad = next((p for p, n in free if n == node), None)
            if pad is None:
                raise ValueError(f"{name}: no pad of {ref} is on node {node}")
            free.remove((pad, node))
            mapping.append(f"{pad}={port}")
        fields[ref] = model + [("Sim.Pins", " ".join(mapping))]
        compared.append(" ".join(tokens))
    return (fields, set(netlist.components) - set(fields)), compared


def compare_spice(path, work, env, library_symbols=False):
    deck = Path(path).with_suffix(".cir")
    if not deck.exists():
        raise ValueError(f"no SPICE deck {deck}")
    elements, models, subckts = read_deck(deck)
    project = {"name": "compare", "uuid": new_uuid()}
    if library_symbols:
        netlist = read(path)
        simulated = {t[0] if t[0] in netlist.components else t[0][1:] for t in elements}
        ours = [normalize_models(" ".join(tokens)) for tokens in elements]
        files, notes = synthesize_library(path, project, set(netlist.components) - simulated)
    else:
        library = (work / "models.lib").resolve()
        library.write_text("\n".join(models) + "\n")
        sim, ours = sim_fields(read(path), elements, subckts, library)
        files, notes = synthesize(path, project, sim)
    for name, text in files.items():
        (work / name).write_text(text)
    exported = work / "kicad.cir"
    run = subprocess.run(["kicad-cli", "sch", "export", "netlist", "--format", "spice",
                          "-o", str(exported), str(work / "compare.kicad_sch")],
                         env={**os.environ, **env}, capture_output=True, text=True)
    if run.returncode or not exported.exists():
        return [f"kicad-cli failed: {run.stdout}{run.stderr}"], notes
    theirs = [" ".join("0" if t.lower() == "gnd" else t for t in tokens)
              for tokens in read_deck(exported)[0]]
    if library_symbols:
        theirs = [normalize_models(line) for line in theirs]
    diff = list(difflib.unified_diff(sorted(ours), sorted(theirs), str(deck), "kicad-cli", lineterm=""))
    if not diff and ours != theirs:
        notes.append("element order differs: kicad-cli writes " + ", ".join(t.split()[0] for t in theirs))
    return diff, notes


def normalize_models(line):
    """Built-in model names depend on the exporter (`__D1` in KiCad, `__<type>` in cuprite_wire)."""
    return re.sub(r"(?<=\s)__\S+", "__model", line)


_LIBRARIES = {}


def library_symbol(library, name):
    """The raw tree of a library symbol, with `extends` flattened and named `Library:Name`."""
    if library not in _LIBRARIES:
        path = symbol_lib_path(library)
        if not path.exists():
            raise ValueError(f"symbol library {library} not found")
        tree = parse(path.read_text(encoding="utf-8"), raw=True)
        _LIBRARIES[library] = {unquote(s[1]): s for s in children(tree, "symbol")}
    symbols = _LIBRARIES[library]
    if name not in symbols:
        raise ValueError(f"symbol {library}:{name} not found")
    symbol = [list(item) if isinstance(item, list) else item for item in symbols[name]]
    extends = children(symbol, "extends")
    if extends:
        parent = library_symbol(library, unquote(extends[0][1]))
        parent_name = unquote(parent[1]).split(":", 1)[1]
        overrides = {unquote(p[1]): p for p in children(symbol, "property")}
        flat = [parent[0], parent[1]]
        for item in parent[2:]:
            if isinstance(item, list) and item[0] == "property" and unquote(item[1]) in overrides:
                item = overrides.pop(unquote(item[1]))
            elif isinstance(item, list) and item[0] == "symbol":
                item = [item[0], '"' + name + unquote(item[1])[len(parent_name):] + '"', *item[2:]]
            elif isinstance(item, list) and children(symbol, item[0]) and item[0] != "property":
                item = children(symbol, item[0])[0]  # flags overridden by the derived symbol
            flat.append(item)
        flat[2:2] = list(overrides.values())
        symbol = flat
    symbol[1] = '"' + f"{library}:{name}" + '"'
    return symbol


def expand_stacked_pin(number):
    """The pads of a pin number: KiCad 10's stacked pin notation (`[1,3]`, `[1-4]`, `[A1-A3]`)
    expanded, else the number itself."""
    if not (number.startswith("[") and number.endswith("]")):
        return [number]
    pads = []
    for part in filter(None, (p.strip() for p in number[1:-1].split(","))):
        if "-" not in part:
            pads.append(part)
            continue
        ends = [re.fullmatch(r"(.*?)(\d+)", e.strip()) for e in part.split("-", 1)]
        if not all(ends) or ends[0][1] != ends[1][1] or int(ends[0][2]) > int(ends[1][2]):
            return [number]
        pads += [f"{ends[0][1]}{n}" for n in range(int(ends[0][2]), int(ends[1][2]) + 1)]
    return pads or [number]


def symbol_units(symbol):
    """{unit: [(pin number, x, y)]} of body style 1; unit 0 holds the pins shared by all units."""
    name = unquote(symbol[1]).split(":", 1)[1]
    units = {}
    for sub in children(symbol, "symbol"):
        unit, style = (int(n) for n in unquote(sub[1])[len(name) + 1:].split("_"))
        if style > 1:
            continue
        for pin in children(sub, "pin"):
            number = unquote(children(pin, "number")[0][1])
            at = children(pin, "at")[0]
            units.setdefault(unit, []).append((number, float(at[1]), float(at[2])))
    return units


def synthesize_library(path, project, sim_excluded=None):
    """Returns ({file name: text}, notes) of a one-sheet schematic using the library symbols.

    `sim_excluded`: references to exclude from the simulation (--spice); None keeps the flag of
    the symbol.
    """
    netlist = read(path)
    root_tree = parse(open(path, encoding="utf-8").read())
    notes = []
    comps = {value(c, "ref"): c for c in children(children(root_tree, "components")[0], "comp")}
    instance = "/" + project["uuid"]
    out = ['(kicad_sch (version 20231120) (generator "cuprite_wire_compare")',
           f"(uuid {quote(project['uuid'])})", '(paper "A0")', "(lib_symbols"]
    lib_ids = {}
    for ref, comp in comps.items():
        source = children(comp, "libsource")
        if not source:
            raise ValueError(f"{ref} has no libsource")
        lib_ids[ref] = (value(source[0], "lib"), value(source[0], "part"))
        if netlist.components[ref].sheet_names.strip("/"):
            raise ValueError("only netlists of a single sheet are supported")
    symbols = {lib_id: library_symbol(*lib_id) for lib_id in sorted(set(lib_ids.values()))}
    for symbol in symbols.values():
        out.append(dump(symbol))
    out.append(")")

    nets = {}
    for (ref, pad), node in netlist.nodes.items():
        nets.setdefault(node.net, []).append((ref, pad))
    project_libs = project_symbol_libs()
    x = 25.4
    y = 101.6
    for ref, comp in comps.items():
        lib_id = lib_ids[ref]
        symbol = symbols[lib_id]
        properties = {unquote(p[1]): unquote(p[2]) for p in children(symbol, "property")}
        component = netlist.components[ref]
        properties.update({"Reference": ref, "Value": component.value, "Footprint": component.footprint,
                           "Datasheet": component.datasheet, "Description": component.description})
        properties.update(netlist.components[ref].fields)
        if lib_id[0] in project_libs and "Sim.Library" in properties:
            # The project's models are next to its symbol library, not in KiCad's symbol directory.
            properties["Sim.Library"] = SYMBOL_DIR_VARIABLE.sub(
                lambda _: str(project_libs[lib_id[0]].parent), properties["Sim.Library"])
        flags = netlist.components[ref].properties
        sim_flag = "yes" if sim_excluded is not None and ref in sim_excluded else "no"
        if sim_excluded is None:
            sim_flag = next((f[1] for f in children(symbol, "exclude_from_sim")), "no")
        units = symbol_units(symbol)
        shared = units.pop(0, [])
        uid = netlist.components[ref].path.rsplit("/", 1)[1]
        for unit in sorted(units) or [1]:
            out.append(f"(symbol (lib_id {quote(':'.join(lib_id))}) (at {mm(x)} {mm(y)} 0) (unit {unit}) "
                       f"(exclude_from_sim {sim_flag}) "
                       f"(in_bom {'no' if 'exclude_from_bom' in flags else 'yes'}) "
                       f"(on_board {'no' if 'exclude_from_board' in flags else 'yes'}) "
                       f"(dnp {'yes' if 'dnp' in flags else 'no'}) (uuid {quote(uid)})")
            for i, (key, text) in enumerate(properties.items()):
                out.append(prop(key, text, (x, y + 10 + 2 * i), hidden=key != "Reference"))
            out.append(f"(instances (project {quote(project['name'])} "
                       f"(path {quote(instance)} (reference {quote(ref)}) (unit {unit})))))")
            # A label on every pin end (schematic y grows downwards).
            pins = units.get(unit, []) + (shared if unit == min(units, default=1) else [])
            for number, px, py in pins:
                # The pads of a stacked pin (KiCad 10) are connected; the first one has the net.
                node = netlist.nodes.get((ref, expand_stacked_pin(number)[0]))
                if node is None:
                    raise ValueError(f"{ref}: pin {number} of the symbol is not in the netlist")
                if node.pintype.endswith(NO_CONNECT_SUFFIX):
                    out.append(no_connect_line(x + px, y - py))
                net = node.net
                auto = net.startswith(("Net-(", "unconnected-("))
                if auto and len(nets[net]) == 1:
                    continue
                kind, text = ("label", net[1:]) if net.startswith("/") else ("global_label", net)
                shape = "" if kind == "label" else " (shape passive)"
                out.append(f"({kind} {quote(text)}{shape} (at {mm(x + px)} {mm(y - py)} 0) "
                           f"(effects (font (size 1.27 1.27)) (justify left bottom)) (uuid {quote(new_uuid())}))")
            x += COMPONENT_PITCH
            uid = new_uuid()
    multi_pin_auto = sorted(n for n, nodes in nets.items()
                            if n.startswith(("Net-(", "unconnected-(")) and len(nodes) > 1)
    if multi_pin_auto:
        notes.append("named by labels, so their names are not compared: " + ", ".join(multi_pin_auto))
    out.append('(sheet_instances (path "/" (page "1")))')
    out.append(")")
    return {"compare.kicad_sch": "\n".join(out) + "\n"}, notes


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--keep", metavar="DIR",
                        help="keep the synthesized schematics and KiCad's netlists in DIR")
    parser.add_argument("--spice", action="store_true",
                        help="compare the SPICE deck NETLIST.cir with kicad-cli's SPICE export")
    parser.add_argument("--library-symbols", action="store_true",
                        help="use the components' library symbols (and their Sim.* fields)")
    parser.add_argument("--symbol-dir", action="append", default=[], metavar="DIR",
                        help="another directory of symbol libraries, searched first")
    parser.add_argument("netlists", nargs="+")
    args = parser.parse_args()
    kicad_netlist.EXTRA_SYMBOL_DIRS.extend(Path(d).resolve() for d in args.symbol_dir)

    root = Path(args.keep) if args.keep else Path(tempfile.mkdtemp(prefix="netlist-compare-"))
    root.mkdir(parents=True, exist_ok=True)
    env = kicad_env(root / "kicad")
    if not args.spice:
        print("Ignored (intentional differences):")
        known = {**KNOWN_DIFFERENCES, **(LIBRARY_KNOWN_DIFFERENCES if args.library_symbols else {}),
                 **(KICAD10_KNOWN_DIFFERENCES if kicad_version()[0] >= 10 else {})}
        for what, why in known.items():
            print(f"  {what}: {why}")
    failed = False
    try:
        for i, path in enumerate(args.netlists):
            work = root / f"{i}-{Path(path).stem}"
            work.mkdir(exist_ok=True)
            try:
                diff, notes = (compare_spice if args.spice else compare)(path, work, env, args.library_symbols)
            except ValueError as e:
                print(f"{path}: skipped: {e}")
                continue
            for line in notes + diff:
                print(f"{path}: {line}")
            failed |= bool(diff)
            if not diff:
                print(f"{path}: same as kicad-cli")
    finally:
        if args.keep:
            print(f"outputs kept in {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
