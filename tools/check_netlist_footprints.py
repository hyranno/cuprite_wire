#!/usr/bin/env python3
"""Check that the footprints and pads referenced by KiCad netlists exist.

Every `footprint` of a `comp` must be loadable from the installed footprint
libraries, and every `pin` of a `node` must be a pad number of that footprint.
The import into pcbnew itself is checked by `check_netlist_import.py`.

Usage: python3 tools/check_netlist_footprints.py [--footprint-dir DIR] NETLIST...
"""

import argparse
import sys

import pcbnew

from kicad_netlist import read


def check(path, footprint_dir):
    netlist = read(path)
    problems = []
    pads_by_ref = {}
    for ref, comp in netlist.components.items():
        library, name = comp.footprint.split(":", 1)
        footprint = pcbnew.FootprintLoad(f"{footprint_dir}/{library}.pretty", name)
        if footprint is None:
            problems.append(f"{ref}: footprint {library}:{name} not found")
            continue
        pads_by_ref[ref] = {pad.GetNumber() for pad in footprint.Pads()}
    for (ref, pin), node in netlist.nodes.items():
        if ref in pads_by_ref and pin not in pads_by_ref[ref]:
            problems.append(f"{ref}: pad {pin!r} (net {node.net!r}) not in the footprint")
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
