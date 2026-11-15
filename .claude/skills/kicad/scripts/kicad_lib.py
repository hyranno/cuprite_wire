#!/usr/bin/env python3
"""Look up KiCad symbol / footprint libraries from the command line.

Subcommands:
  search PATTERN [--fp] [--lib LIB]   regex search over symbol (or footprint) names,
                                      descriptions and keywords
  sym LIB:NAME                        show properties, units and pins of a symbol
                                      (inheritance via `extends` is resolved)
  fp LIB:NAME                         show attributes and pads of a footprint
  libs [--fp]                         list library nicknames

Library directories default to the system install and can be overridden with
KICAD9_SYMBOL_DIR / KICAD9_FOOTPRINT_DIR. Standard library only.
"""

import argparse
import os
import re
import sys
from pathlib import Path

SYM_DIR = Path(os.environ.get("KICAD9_SYMBOL_DIR", "/usr/share/kicad/symbols"))
FP_DIR = Path(os.environ.get("KICAD9_FOOTPRINT_DIR", "/usr/share/kicad/footprints"))


# ---------------------------------------------------------------- S-expression

_TOKEN = re.compile(r'\s*(?:(\()|(\))|"((?:[^"\\]|\\.)*)"|([^\s()"]+))', re.S)


class Str(str):
    """A quoted string atom (distinguished from bare symbols)."""


def parse_sexpr(text: str):
    stack = [[]]
    for m in _TOKEN.finditer(text):
        lp, rp, quoted, atom = m.groups()
        if lp:
            stack.append([])
        elif rp:
            done = stack.pop()
            stack[-1].append(done)
        elif quoted is not None:
            stack[-1].append(Str(re.sub(r"\\(.)", r"\1", quoted)))
        elif atom is not None:
            stack[-1].append(atom)
    return stack[0]


def children(node, head):
    return [c for c in node[1:] if isinstance(c, list) and c and c[0] == head]


def child(node, head):
    found = children(node, head)
    return found[0] if found else None


def flag(node, name):
    """True if `name` appears as a bare atom or as `(name yes)`."""
    for c in node[1:]:
        if c == name:
            return True
        if isinstance(c, list) and c and c[0] == name:
            return len(c) < 2 or c[1] == "yes"
    return False


# ---------------------------------------------------------------- helpers

def split_id(lib_id: str):
    if ":" not in lib_id:
        sys.exit(f"expected LIB:NAME, got {lib_id!r}")
    return lib_id.split(":", 1)


def extract_toplevel_block(text: str, head: str, name: str):
    """Return the text of a top-level `(head "name" ...)` block (one tab indent)."""
    m = re.search(rf'^\t\({head} "{re.escape(name)}"\s*$', text, re.M)
    if not m:
        return None
    depth, i = 0, m.start() + 1
    in_str = esc = False
    for j in range(i, len(text)):
        ch = text[j]
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
        elif ch == '"':
            in_str = True
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return text[i:j + 1]
    return None


def load_symbol(lib: str, name: str):
    path = SYM_DIR / f"{lib}.kicad_sym"
    if not path.exists():
        sys.exit(f"symbol library not found: {path}")
    text = path.read_text(encoding="utf-8")
    block = extract_toplevel_block(text, "symbol", name)
    if block is None:
        sys.exit(f"symbol {name!r} not found in {path}")
    node = parse_sexpr(block)[0]
    chain = [node]
    while (ext := child(node, "extends")) is not None:
        block = extract_toplevel_block(text, "symbol", ext[1])
        if block is None:
            sys.exit(f"parent symbol {ext[1]!r} not found")
        node = parse_sexpr(block)[0]
        chain.append(node)
    return chain


def unit_style(subname: str, base: str):
    # sub-symbol names are BASE_<unit>_<bodystyle>; unit 0 = common to all units
    m = re.fullmatch(re.escape(base) + r"_(\d+)_(\d+)", subname)
    return (int(m.group(1)), int(m.group(2))) if m else (None, None)


# ---------------------------------------------------------------- commands

def cmd_sym(args):
    lib, name = split_id(args.id)
    chain = load_symbol(lib, name)
    # properties: child overrides parent
    props = {}
    for node in reversed(chain):
        for p in children(node, "property"):
            props[p[1]] = p[2]
    root = chain[-1]  # graphics and pins live in the root (non-extending) symbol
    print(f"{lib}:{name}")
    if len(chain) > 1:
        print("  extends: " + " -> ".join(n[1] for n in chain[1:]))
    for k, v in props.items():
        if v:
            print(f"  {k}: {v}")
    for key in ("power", "in_bom", "on_board", "exclude_from_sim"):
        sub = child(chain[0], key) or child(root, key)
        if sub is not None:
            print(f"  {key}: {sub[1] if len(sub) > 1 else 'yes'}")
    pn = child(root, "pin_numbers")
    if pn is not None and flag(pn, "hide"):
        print("  pin_numbers: hidden")

    rows = []
    for sub in children(root, "symbol"):
        unit, style = unit_style(sub[1], root[1])
        for pin in children(sub, "pin"):
            etype, shape = pin[1], pin[2]
            num = child(pin, "number")[1]
            pname = child(pin, "name")[1]
            alts = [a[1] for a in children(pin, "alternate")]
            rows.append((unit, style, num, pname, etype, shape, flag(pin, "hide"), alts))
    if not rows:
        print("  (no pins)")
        return
    units = sorted({r[0] for r in rows if r[0]})
    print(f"  units: {len(units) or 1}  (unit 0 = shared by all units)")
    print(f"  {'unit':>4} {'style':>5}  {'num':<6} {'name':<20} {'type':<14} flags")
    def key(r):
        n = r[2]
        return (r[0] or 0, r[1] or 0, (0, int(n), "") if n.isdigit() else (1, 0, n))
    for unit, style, num, pname, etype, shape, hidden, alts in sorted(rows, key=key):
        flags = []
        if hidden:
            flags.append("hidden")
        if shape != "line":
            flags.append(shape)
        if alts:
            flags.append("alt=" + "/".join(alts))
        print(f"  {unit!s:>4} {style!s:>5}  {num:<6} {pname:<20} {etype:<14} {' '.join(flags)}")


def cmd_fp(args):
    lib, name = split_id(args.id)
    path = FP_DIR / f"{lib}.pretty" / f"{name}.kicad_mod"
    if not path.exists():
        sys.exit(f"footprint not found: {path}")
    node = parse_sexpr(path.read_text(encoding="utf-8"))[0]
    print(f"{lib}:{name}")
    for key in ("descr", "tags"):
        sub = child(node, key)
        if sub is not None:
            print(f"  {key}: {sub[1]}")
    attr = child(node, "attr")
    if attr is not None:
        print("  attr: " + " ".join(str(a) for a in attr[1:]))
    pads = children(node, "pad")
    print(f"  pads: {len(pads)}")
    print(f"  {'num':<6} {'type':<12} {'shape':<10} {'at':<20} size")
    def key(p):
        n = p[1]
        return (0, int(n), "") if n.isdigit() else (1, 0, n)
    for p in sorted(pads, key=key):
        at = child(p, "at")
        size = child(p, "size")
        at_s = " ".join(str(v) for v in at[1:]) if at else ""
        size_s = "x".join(str(v) for v in size[1:]) if size else ""
        print(f"  {p[1] or '(none)':<6} {p[2]:<12} {p[3]:<10} {at_s:<20} {size_s}")


_SYM_HEAD = re.compile(r'^\t\(symbol "([^"]+)"')
_PROP = re.compile(r'^\t\t\(property "(Description|ki_keywords)" "((?:[^"\\]|\\.)*)"')


def iter_symbols(lib_filter):
    for path in sorted(SYM_DIR.glob("*.kicad_sym")):
        if lib_filter and not re.search(lib_filter, path.stem, re.I):
            continue
        name, info = None, []
        with path.open(encoding="utf-8") as f:
            for line in f:
                if m := _SYM_HEAD.match(line):
                    if name:
                        yield path.stem, name, " ".join(info)
                    name, info = m.group(1), []
                elif name and (m := _PROP.match(line)):
                    info.append(m.group(2))
        if name:
            yield path.stem, name, " ".join(info)


def iter_footprints(lib_filter):
    for d in sorted(FP_DIR.glob("*.pretty")):
        if lib_filter and not re.search(lib_filter, d.stem, re.I):
            continue
        for f in sorted(d.glob("*.kicad_mod")):
            yield d.stem, f.stem, ""


def cmd_search(args):
    rx = re.compile(args.pattern, re.I)
    it = iter_footprints(args.lib) if args.fp else iter_symbols(args.lib)
    n = 0
    for lib, name, info in it:
        if rx.search(name) or (args.desc and rx.search(info)):
            print(f"{lib}:{name}" + (f"  -- {info[:120]}" if info else ""))
            n += 1
            if n >= args.limit:
                print(f"... (stopped at --limit {args.limit})")
                break


def cmd_libs(args):
    if args.fp:
        names = sorted(p.stem for p in FP_DIR.glob("*.pretty"))
    else:
        names = sorted(p.stem for p in SYM_DIR.glob("*.kicad_sym"))
    print("\n".join(names))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sp = ap.add_subparsers(dest="cmd", required=True)
    p = sp.add_parser("search", help="regex search by name")
    p.add_argument("pattern")
    p.add_argument("--fp", action="store_true", help="search footprints instead of symbols")
    p.add_argument("--lib", help="regex filter on library nickname")
    p.add_argument("--desc", action="store_true", help="also match description/keywords (symbols)")
    p.add_argument("--limit", type=int, default=50)
    p.set_defaults(func=cmd_search)
    p = sp.add_parser("sym", help="show a symbol")
    p.add_argument("id", help="LIB:NAME")
    p.set_defaults(func=cmd_sym)
    p = sp.add_parser("fp", help="show a footprint")
    p.add_argument("id", help="LIB:NAME")
    p.set_defaults(func=cmd_fp)
    p = sp.add_parser("libs", help="list libraries")
    p.add_argument("--fp", action="store_true")
    p.set_defaults(func=cmd_libs)
    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
