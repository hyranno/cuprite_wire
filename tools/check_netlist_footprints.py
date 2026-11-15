#!/usr/bin/env python3
"""Check that the footprints and pads referenced by KiCad netlists exist.

pcbnew's netlist reader is not available from Python or kicad-cli, so this
script covers what can be checked automatically: every `footprint` of a
`comp` must be loadable from the installed footprint libraries, and every
`pin` of a `node` must be a pad number of that footprint.

Usage: python3 tools/check_netlist_footprints.py [--footprint-dir DIR] NETLIST...
"""

import argparse
import re
import sys

import pcbnew

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


def check(path, footprint_dir):
    root = parse(open(path, encoding="utf-8").read())
    problems = []
    pads_by_ref = {}
    for comp in children(children(root, "components")[0], "comp"):
        ref = value(comp, "ref")
        library, name = value(comp, "footprint").split(":", 1)
        footprint = pcbnew.FootprintLoad(f"{footprint_dir}/{library}.pretty", name)
        if footprint is None:
            problems.append(f"{ref}: footprint {library}:{name} not found")
            continue
        pads_by_ref[ref] = {pad.GetNumber() for pad in footprint.Pads()}
    for net in children(children(root, "nets")[0], "net"):
        for node in children(net, "node"):
            ref, pin = value(node, "ref"), value(node, "pin")
            if ref in pads_by_ref and pin not in pads_by_ref[ref]:
                problems.append(f"{ref}: pad {pin!r} (net {value(net, 'name')!r}) not in the footprint")
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--footprint-dir", default="/usr/share/kicad/footprints")
    parser.add_argument("netlists", nargs="+")
    args = parser.parse_args()
    failed = False
    for path in args.netlists:
        problems = check(path, args.footprint_dir)
        for problem in problems:
            print(f"{path}: {problem}")
        failed |= bool(problems)
        if not problems:
            print(f"{path}: ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
