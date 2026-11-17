#!/usr/bin/env python3
"""Compare a netlist with what KiCad's own exporter writes for the same circuit.

For each netlist, an equivalent schematic is synthesized: one symbol per component
(pins, pin names and types from the netlist nodes; value, footprint, fields and
flags from the component; the component's tstamps as the symbol's unique id),
connected by wires, with a global label for every global net name and a local
label (in its sheet) for every local one. Unnamed nets get no label, so KiCad names them itself.
`kicad-cli sch export netlist --format kicadsexpr` then exports the schematic, and
both netlists are compared after removing what cuprite_wire intentionally omits
(see KNOWN_DIFFERENCES). The remaining differences in element order, quoting and
omission rules are printed as a diff.

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

Usage: python3 tools/compare_kicad_cli_netlist.py [--keep DIR] [--spice] NETLIST...
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

from kicad_netlist import TOKEN, children, parse, read, value

# Items KiCad writes and cuprite_wire intentionally does not (doc/kicad_netlist_format.md §8).
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

PIN_PITCH = 2.54
COMPONENT_PITCH = 60.96
PIN_Y = 101.6  # every pin end is on this line
HUB_Y = 25.4  # every net's hub is on this line


def kicad_env(root):
    return {
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "data"),
        "HOME": str(root / "home"),
    }


def quote(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


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
        pins[ref].append((pad, node.pinfunction, node.pintype))
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
        for field_name, text in [*comp.fields.items(), *sim_fields.get(ref, [])]:
            out.append(prop(field_name, text, (x, y)))
        for i, (pad, _, _) in enumerate(sorted(pins[ref])):
            ends[("pin", ref, pad)] = (x + i * PIN_PITCH, PIN_Y)
            out.append(f"(pin {quote(pad)} (uuid {quote(new_uuid())}))")
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


def parse_raw(text):
    """Parse an S-expression into nested lists, keeping atoms as written (quotes included)."""
    stack = [[]]
    for token in TOKEN.findall(text):
        if token == "(":
            stack.append([])
        elif token == ")":
            done = stack.pop()
            stack[-1].append(done)
        else:
            stack[-1].append(token)
    return stack[0][0]


def normalize(tree, ours):
    """Drops what is listed in KNOWN_DIFFERENCES from KiCad's netlist (`tree`)."""
    has_libsource = {value(c, "ref"): bool(children(c, "libsource"))
                     for c in children(children(ours, "components")[0], "comp")}
    tree = [item for item in tree
            if not (isinstance(item, list) and item[0] in ("design", "libparts", "libraries"))]
    for comp in children(children(tree, "components")[0], "comp"):
        ref = value(comp, "ref")
        field_names = {value(f, "name") for fields in children(comp, "fields") for f in fields[1:]}
        kept = []
        for item in comp[1:]:
            if not isinstance(item, list):
                kept.append(item)
            elif item[0] == "fields":
                fields = [f for f in item[1:] if value(f, "name") not in DROPPED_FIELDS]
                if fields:
                    kept.append(["fields", *fields])
            elif item[0] == "libsource":
                if has_libsource.get(ref):
                    kept.append([i for i in item if not (isinstance(i, list) and i[0] == "description")])
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


def compare(path, work, env):
    project = {"name": "compare", "uuid": new_uuid()}
    files, notes = synthesize(path, project)
    for name, text in files.items():
        (work / name).write_text(text)
    schematic = work / "compare.kicad_sch"
    exported = work / "kicad.net"
    run = subprocess.run(["kicad-cli", "sch", "export", "netlist", "--format", "kicadsexpr",
                          "-o", str(exported), str(schematic)],
                         env={**os.environ, **env}, capture_output=True, text=True)
    if run.returncode or not exported.exists():
        return [f"kicad-cli failed: {run.stdout}{run.stderr}"], notes
    ours = parse_raw(Path(path).read_text(encoding="utf-8"))
    theirs = normalize(parse_raw(exported.read_text(encoding="utf-8")), ours)
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
        if not tokens or line.startswith("*"):
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


def compare_spice(path, work, env):
    deck = Path(path).with_suffix(".cir")
    if not deck.exists():
        raise ValueError(f"no SPICE deck {deck}")
    elements, models, subckts = read_deck(deck)
    library = (work / "models.lib").resolve()
    library.write_text("\n".join(models) + "\n")
    sim, ours = sim_fields(read(path), elements, subckts, library)
    project = {"name": "compare", "uuid": new_uuid()}
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
    diff = list(difflib.unified_diff(sorted(ours), sorted(theirs), str(deck), "kicad-cli", lineterm=""))
    if not diff and ours != theirs:
        notes.append("element order differs: kicad-cli writes " + ", ".join(t.split()[0] for t in theirs))
    return diff, notes


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--keep", metavar="DIR",
                        help="keep the synthesized schematics and KiCad's netlists in DIR")
    parser.add_argument("--spice", action="store_true",
                        help="compare the SPICE deck NETLIST.cir with kicad-cli's SPICE export")
    parser.add_argument("netlists", nargs="+")
    args = parser.parse_args()

    root = Path(args.keep) if args.keep else Path(tempfile.mkdtemp(prefix="netlist-compare-"))
    root.mkdir(parents=True, exist_ok=True)
    env = kicad_env(root / "kicad")
    if not args.spice:
        print("Ignored (intentional differences):")
        for what, why in KNOWN_DIFFERENCES.items():
            print(f"  {what}: {why}")
    failed = False
    try:
        for i, path in enumerate(args.netlists):
            work = root / f"{i}-{Path(path).stem}"
            work.mkdir(exist_ok=True)
            try:
                diff, notes = (compare_spice if args.spice else compare)(path, work, env)
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
