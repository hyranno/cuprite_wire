#!/usr/bin/env python3
"""Compare a netlist with what KiCad's own exporter writes for the same circuit.

For each netlist, an equivalent schematic is synthesized: one symbol per component
(pins, pin names and types from the netlist nodes; value, footprint, fields and
flags from the component; the component's tstamps as the symbol's unique id),
connected by wires, with a global label for every global net name and a local
label for every local one. Unnamed nets get no label, so KiCad names them itself.
`kicad-cli sch export netlist --format kicadsexpr` then exports the schematic, and
both netlists are compared after removing what cuprite_wire intentionally omits
(see KNOWN_DIFFERENCES). The remaining differences in element order, quoting and
omission rules are printed as a diff.

Only flat netlists (every component on the root sheet) are supported.

Usage: python3 tools/compare_kicad_cli_netlist.py [--keep DIR] NETLIST...
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


def synthesize(path, project):
    """Returns the text of a schematic equivalent to the netlist at `path`."""
    root = parse(open(path, encoding="utf-8").read())
    netlist = read(path)
    for comp in netlist.components.values():
        if comp.sheet_names != "/":
            raise ValueError(f"{comp.ref} is on sheet {comp.sheet_names!r}; "
                             "only flat netlists are supported")
    # pins[ref] = [(pad, name, type)] in pad order of the netlist nodes.
    pins = {ref: [] for ref in netlist.components}
    for (ref, pad), node in netlist.nodes.items():
        pins[ref].append((pad, node.pinfunction, node.pintype))
    libsources = {}
    for comp in children(children(root, "components")[0], "comp"):
        source = children(comp, "libsource")
        ref = value(comp, "ref")
        if source:
            libsources[ref] = f"{value(source[0], 'lib')}:{value(source[0], 'part')}"
        else:
            libsources[ref] = f"cw:{ref}"

    # One library symbol per lib id; its pins must be the same for every component using it.
    lib_symbols = {}
    for ref, lib_id in libsources.items():
        signature = sorted(pins[ref])
        if lib_symbols.setdefault(lib_id, (ref, signature))[1] != signature:
            raise ValueError(f"{ref} and {lib_symbols[lib_id][0]} use {lib_id} with different pins")

    out = [f'(kicad_sch (version 20231120) (generator "cuprite_wire_compare")',
           f"(uuid {quote(project['uuid'])})", '(paper "A0")', "(lib_symbols"]
    for lib_id, (ref, signature) in lib_symbols.items():
        name = lib_id.split(":", 1)[1]
        prefix = re.match(r"[A-Za-z#]*", ref).group() or "X"
        out.append(f"(symbol {quote(lib_id)} (in_bom yes) (on_board yes)")
        out.append(prop("Reference", prefix, (0, 0), hidden=False))
        out.append(prop("Value", name, (0, 0)))
        out.append(f"(symbol {quote(name + '_1_1')}")
        for i, (pad, pin_name, pin_type) in enumerate(signature):
            out.append(f"(pin {pin_type} line (at {mm(i * PIN_PITCH)} 5.08 270) (length 2.54) "
                       f"(name {quote(pin_name or '~')} (effects (font (size 1.27 1.27)))) "
                       f"(number {quote(pad)} (effects (font (size 1.27 1.27)))))")
        out.append("))")
    out.append(")")

    # Symbols in a row; pin ends on the line PIN_Y.
    pin_ends = {}
    for n, (ref, comp) in enumerate(sorted(netlist.components.items())):
        x, y = 25.4 + n * COMPONENT_PITCH, PIN_Y + 5.08
        flags = comp.properties
        out.append(f"(symbol (lib_id {quote(libsources[ref])}) (at {mm(x)} {mm(y)} 0) (unit 1) "
                   f"(in_bom {'no' if 'exclude_from_bom' in flags else 'yes'}) "
                   f"(on_board {'no' if 'exclude_from_board' in flags else 'yes'}) "
                   f"(dnp {'yes' if 'dnp' in flags else 'no'}) (uuid {quote(comp.path[1:])})")
        out.append(prop("Reference", ref, (x, y + 5), hidden=False))
        out.append(prop("Value", comp.value, (x, y + 7.5)))
        out.append(prop("Footprint", comp.footprint, (x, y)))
        for field_name, text in comp.fields.items():
            out.append(prop(field_name, text, (x, y)))
        for i, (pad, _, _) in enumerate(sorted(pins[ref])):
            pin_ends[(ref, pad)] = (x + i * PIN_PITCH, PIN_Y)
            out.append(f"(pin {quote(pad)} (uuid {quote(new_uuid())}))")
        out.append(f"(instances (project {quote(project['name'])} "
                   f"(path {quote('/' + project['uuid'])} (reference {quote(ref)}) (unit 1)))))")

    # Every net gets a hub on the line HUB_Y, wired to the pin ends on the line PIN_Y. All wire
    # ends are on these two lines, so none lies inside another wire and connects to it.
    nets = {}
    for (ref, pad), node in sorted(netlist.nodes.items()):
        nets.setdefault(node.net, []).append((ref, pad))
    for k, (net, nodes) in enumerate(sorted(nets.items())):
        auto = net.startswith(("Net-(", "unconnected-("))
        if auto and len(nodes) == 1:
            continue
        hub = (25.4 + k * PIN_PITCH, HUB_Y)
        for node in nodes:
            end = pin_ends[node]
            out.append(f"(wire (pts (xy {mm(end[0])} {mm(end[1])}) (xy {mm(hub[0])} {mm(hub[1])})) "
                       f"(stroke (width 0) (type default)) (uuid {quote(new_uuid())}))")
        if auto:
            continue
        effects = "(effects (font (size 1.27 1.27)) (justify left bottom))"
        if net.startswith("/"):
            out.append(f"(label {quote(net[1:])} (at {mm(hub[0])} {mm(hub[1])} 90) {effects} "
                       f"(uuid {quote(new_uuid())}))")
        else:
            out.append(f"(global_label {quote(net)} (shape passive) (at {mm(hub[0])} {mm(hub[1])} 90) "
                       f"{effects} (uuid {quote(new_uuid())}))")
    out.append('(sheet_instances (path "/" (page "1"))))')
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
    schematic = work / "compare.kicad_sch"
    schematic.write_text(synthesize(path, project))
    exported = work / "kicad.net"
    run = subprocess.run(["kicad-cli", "sch", "export", "netlist", "--format", "kicadsexpr",
                          "-o", str(exported), str(schematic)],
                         env={**os.environ, **env}, capture_output=True, text=True)
    if run.returncode or not exported.exists():
        return [f"kicad-cli failed: {run.stdout}{run.stderr}"]
    ours = parse_raw(Path(path).read_text(encoding="utf-8"))
    theirs = normalize(parse_raw(exported.read_text(encoding="utf-8")), ours)
    ours = [item for item in ours if not (isinstance(item, list) and item[0] == "design")]
    return list(difflib.unified_diff(lines(ours), lines(theirs), "cuprite_wire", "kicad-cli",
                                     lineterm=""))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--keep", metavar="DIR",
                        help="keep the synthesized schematics and KiCad's netlists in DIR")
    parser.add_argument("netlists", nargs="+")
    args = parser.parse_args()

    root = Path(args.keep) if args.keep else Path(tempfile.mkdtemp(prefix="netlist-compare-"))
    root.mkdir(parents=True, exist_ok=True)
    env = kicad_env(root / "kicad")
    print("Ignored (intentional differences):")
    for what, why in KNOWN_DIFFERENCES.items():
        print(f"  {what}: {why}")
    failed = False
    try:
        for i, path in enumerate(args.netlists):
            work = root / f"{i}-{Path(path).stem}"
            work.mkdir(exist_ok=True)
            try:
                diff = compare(path, work, env)
            except ValueError as e:
                print(f"{path}: skipped: {e}")
                continue
            for line in diff:
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
