#!/usr/bin/env python3
"""Check that pcbnew imports KiCad netlists without errors, using the real GUI.

pcbnew's netlist reader is not available from Python or kicad-cli, so this script
runs pcbnew on a virtual X server (Xvfb) and drives File -> Import -> Netlist...
from a scripting plugin inside pcbnew (`pcbnew_gui/import_netlist_plugin.py`).
For each netlist:

1. Import into an empty board, linking footprints by unique ids (tstamps).
   The report must have no errors (warnings are printed but do not fail),
   and the saved board must match the netlist: references, footprints,
   values, symbol and sheet paths, fields, DNP / exclude-from-BOM flags, and
   the net, pin function and pin type of every pad.
2. Move the footprints, import the same netlist again, and check that no
   footprint was replaced: unique ids and positions are kept.

With `--relink`, the netlists are taken in pairs (BEFORE AFTER), and step 2
imports AFTER, a revised circuit: every footprint whose path (sheet and symbol
unique ids) is still in the netlist must be kept in place even if its reference
or value changed, footprints of new components are added, and footprints of
removed components are left as they are. What happened to each footprint is
printed.

KiCad runs with a throwaway configuration under a temporary directory, so the
user's KiCad settings are neither used nor changed.

Usage: python3 tools/check_netlist_import.py [--keep DIR] NETLIST...
       python3 tools/check_netlist_import.py [--keep DIR] --relink BEFORE AFTER...
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from kicad_netlist import KICAD_TEMPLATE_DIR, kicad_version, project_footprint_libs, project_symbol_libs, read

PLUGIN = Path(__file__).resolve().parent / "pcbnew_gui" / "import_netlist_plugin.py"
TOTALS = re.compile(r"Total warnings: (\d+), errors: (\d+)\.")


class KicadEnv:
    """XDG directories for a throwaway KiCad configuration."""

    def __init__(self, root):
        self.root = Path(root)
        self.vars = {
            "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_CACHE_HOME": str(self.root / "cache"),
            "XDG_DATA_HOME": str(self.root / "data"),
            "HOME": str(self.root / "home"),
        }
        # KiCad keeps its settings in a directory per major version.
        settings = f"{kicad_version()[0]}.0"
        config = self.root / "config" / "kicad" / settings
        config.mkdir(parents=True, exist_ok=True)
        # The installed libraries and the project's (libs/).
        for table, libs in (("fp-lib-table", project_footprint_libs()), ("sym-lib-table", project_symbol_libs())):
            text = (KICAD_TEMPLATE_DIR / table).read_text().rstrip()
            assert text.endswith(")"), table
            entries = "".join(f'  (lib (name "{name}")(type "KiCad")(uri "{path}")(options "")(descr ""))\n'
                              for name, path in libs.items())
            (config / table).write_text(text[:-1] + entries + ")\n")
        # An existing kicad_common.json skips the first-run "Welcome to KiCad" dialog (KiCad 9);
        # KiCad 10 shows its "KiCad Setup" wizard unless these prompts are dismissed and
        # a design block library table exists.
        (config / "kicad_common.json").write_text(json.dumps(
            {"do_not_show_again": {"update_check_prompt": True, "data_collection_prompt": True}}) + "\n")
        (config / "design-block-lib-table").write_text("(design_block_lib_table\n  (version 7)\n)\n")
        plugins = self.root / "data" / "kicad" / settings / "scripting" / "plugins"
        plugins.mkdir(parents=True, exist_ok=True)
        shutil.copy(PLUGIN, plugins / PLUGIN.name)
        (self.root / "home").mkdir(exist_ok=True)


class Xvfb:
    """A virtual X server listening on TCP only (Unix sockets may be unavailable in sandboxes)."""

    def __enter__(self):
        read_fd, write_fd = os.pipe()
        self.process = subprocess.Popen(
            ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24",
             "-nolock", "-nolisten", "unix", "-nolisten", "local", "-listen", "tcp", "-ac"],
            pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        os.close(write_fd)
        with os.fdopen(read_fd) as pipe:
            number = pipe.readline().strip()
        if not number:
            self.process.kill()
            raise RuntimeError("Xvfb did not start")
        self.display = f"127.0.0.1:{number}"
        return self

    def __exit__(self, *exc):
        self.process.terminate()
        self.process.wait()


def run_import(env, display, netlist, board_in, out_dir, timeout):
    """Imports `netlist` into a copy of `board_in` in the pcbnew GUI; returns the plugin's result."""
    out_dir.mkdir(parents=True, exist_ok=True)
    board = out_dir / "board.kicad_pcb"
    shutil.copy(board_in, board)
    job = {
        "board": str(board.resolve()),
        "netlist": str(Path(netlist).resolve()),
        "board_out": str(out_dir / "imported.kicad_pcb"),
        "result": str(out_dir / "result.json"),
        "screenshot": str(out_dir / "report.png"),
    }
    (out_dir / "job.json").write_text(json.dumps(job, indent=2))
    with open(out_dir / "pcbnew.log", "w") as log:
        try:
            subprocess.run(["pcbnew", str(board)], stdout=log, stderr=subprocess.STDOUT,
                           env={**os.environ, **env.vars, "DISPLAY": display,
                                "NETLIST_IMPORT_JOB": str(out_dir / "job.json")},
                           timeout=timeout)
        except subprocess.TimeoutExpired:
            pass
    if not Path(job["result"]).exists():
        return {"ok": False, "error": f"pcbnew wrote no result (see {out_dir / 'pcbnew.log'})"}
    result = json.loads(Path(job["result"]).read_text())
    result["board"] = job["board_out"]
    return result


def read_report(result):
    """(errors, warnings) from the import report; failures to run count as errors."""
    if not result["ok"]:
        return [f"import failed: {result.get('error')}"], []
    totals = TOTALS.findall(result["report"])
    if not totals:
        return ["no totals in the import report"], []
    warnings, errors = map(int, totals[-1])
    lines = [line.strip() for line in result["problems"].splitlines()
             if line.strip() and not TOTALS.search(line)]
    if errors:
        return [f"report: {errors} error(s), {warnings} warning(s)"] + lines, []
    return [], lines


def board_problems(board, netlist, orphans=()):
    """Differences between an imported board and the netlist it was imported from.

    Footprints are matched to components by path (sheet and symbol unique ids), as pcbnew
    does. Footprints whose path is in `orphans` may stay on the board.
    """
    problems = []
    footprints = {fp.GetPath().AsString(): fp for fp in board.GetFootprints()}
    components = {comp.path: comp for comp in netlist.components.values()}
    for path in sorted(set(components) - set(footprints)):
        problems.append(f"{components[path].ref}: missing on the board")
    for path in sorted(set(footprints) - set(components) - set(orphans)):
        problems.append(f"{footprints[path].GetReference()}: on the board but not in the netlist")
    for ref, comp in sorted(netlist.components.items()):
        fp = footprints.get(comp.path)
        if fp is None:
            continue
        fields = {f.GetName(): f.GetText() for f in fp.GetFields()}
        checks = [
            ("reference", fp.GetReference(), comp.ref),
            ("footprint", fp.GetFPIDAsString(), comp.footprint),
            ("value", fp.GetValue(), comp.value),
            ("sheet name", fp.GetSheetname(), comp.sheet_names),
            ("dnp", fp.IsDNP(), "dnp" in comp.properties),
            ("exclude from BOM", fp.IsExcludedFromBOM(), "exclude_from_bom" in comp.properties),
        ]
        checks += [(f"field {name!r}", fields.get(name), text) for name, text in comp.fields.items()]
        for pad in fp.Pads():
            node = netlist.nodes.get((ref, pad.GetNumber()))
            what = f"pad {pad.GetNumber()}"
            if node is None:
                if pad.GetNetname() and not pad.GetNetname().startswith("unconnected-"):
                    problems.append(f"{ref}: {what} is on net {pad.GetNetname()!r}, expected none")
                continue
            checks += [
                (f"{what} net", pad.GetNetname(), node.net),
                (f"{what} pin function", pad.GetPinFunction(), node.pinfunction),
                (f"{what} pin type", pad.GetPinType(), node.pintype),
            ]
        for what, actual, expected in checks:
            if actual != expected:
                problems.append(f"{ref}: {what} is {actual!r}, expected {expected!r}")
    return problems


def placement(board):
    """{path: (reference, value, unique id, position)} of every footprint."""
    return {fp.GetPath().AsString(): (fp.GetReference(), fp.GetValue(), fp.m_Uuid.AsString(),
                                      (fp.GetPosition().x, fp.GetPosition().y))
            for fp in board.GetFootprints()}


def check(path, work, display, env, timeout, revised=None):
    """Imports `path` into an empty board, moves the footprints and imports `revised`
    (by default `path` again). Returns (problems, warnings, notes)."""
    import pcbnew  # needs the XDG variables set by main()

    netlist = read(path)
    revised = revised or path
    revised_netlist = read(revised)
    empty = work / "empty.kicad_pcb"
    pcbnew.SaveBoard(str(empty), pcbnew.BOARD())

    first = run_import(env, display, path, empty, work / "1-import", timeout)
    problems, warnings = read_report(first)
    if not first["ok"]:
        return problems, warnings, []
    board = pcbnew.LoadBoard(first["board"])
    problems += board_problems(board, netlist)

    # Place the footprints on a grid, then import again.
    for i, fp in enumerate(sorted(board.GetFootprints(), key=lambda fp: fp.GetReference())):
        fp.SetPosition(pcbnew.VECTOR2I(pcbnew.FromMM(20 + 10 * (i % 10)),
                                       pcbnew.FromMM(20 + 10 * (i // 10))))
    placed = work / "placed.kicad_pcb"
    pcbnew.SaveBoard(str(placed), board)
    before = placement(pcbnew.LoadBoard(str(placed)))

    second = run_import(env, display, revised, placed, work / "2-reimport", timeout)
    errors, more_warnings = read_report(second)
    problems += [f"reimport: {p}" for p in errors]
    warnings += [f"reimport: {w}" for w in more_warnings if w not in warnings]
    if not second["ok"]:
        return problems, warnings, []
    after_board = pcbnew.LoadBoard(second["board"])
    revised_paths = {comp.path for comp in revised_netlist.components.values()}
    orphans = set(before) - revised_paths
    problems += [f"reimport: {p}" for p in board_problems(after_board, revised_netlist, orphans)]
    after = placement(after_board)

    # Footprints are linked by path: a footprint whose path is still in the netlist must be kept
    # (same unique id and position) even if its reference changed; orphans are left as they are.
    notes = []
    for path in sorted(before.keys() & after.keys(), key=lambda p: before[p][0]):
        (old_ref, old_value, old_id, old_pos) = before[path]
        (new_ref, new_value, new_id, new_pos) = after[path]
        if old_id != new_id:
            problems.append(f"reimport: {old_ref}: footprint was replaced (unique id changed)")
        elif old_pos != new_pos:
            problems.append(f"reimport: {old_ref}: moved from {old_pos} to {new_pos}")
        elif path in orphans:
            notes.append(f"{old_ref}: no longer in the netlist, left on the board")
        elif (old_ref, old_value) != (new_ref, new_value):
            notes.append(f"{old_ref} ({old_value}): kept in place as {new_ref} ({new_value})")
    for path in sorted(after.keys() - before.keys(), key=lambda p: after[p][0]):
        notes.append(f"{after[path][0]} ({after[path][1]}): added")
    return problems, warnings, notes if revised != path else []


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--keep", metavar="DIR",
                        help="keep the boards, reports, screenshots and logs in DIR")
    parser.add_argument("--timeout", type=float, default=120,
                        help="seconds allowed for one pcbnew run (default: 120)")
    parser.add_argument("--relink", action="store_true",
                        help="take the netlists in pairs (BEFORE AFTER): import BEFORE, move the "
                             "footprints, then import AFTER, and check that footprints are linked "
                             "by unique ids")
    parser.add_argument("netlists", nargs="+")
    args = parser.parse_args()
    if args.relink:
        if len(args.netlists) % 2:
            parser.error("--relink needs pairs of netlists")
        jobs = list(zip(args.netlists[::2], args.netlists[1::2]))
    else:
        jobs = [(path, None) for path in args.netlists]

    root = Path(args.keep) if args.keep else Path(tempfile.mkdtemp(prefix="netlist-import-"))
    root.mkdir(parents=True, exist_ok=True)
    env = KicadEnv(root / "kicad")
    os.environ.update(env.vars)

    failed = False
    try:
        with Xvfb() as xvfb:
            for i, (path, revised) in enumerate(jobs):
                work = root / f"{i}-{Path(path).stem}"
                work.mkdir(exist_ok=True)
                problems, warnings, notes = check(path, work, xvfb.display, env, args.timeout,
                                                  revised)
                name = f"{path} -> {revised}" if revised else path
                for line in warnings + problems + notes:
                    print(f"{name}: {line}")
                failed |= bool(problems)
                if not problems:
                    print(f"{name}: ok")
    finally:
        if args.keep:
            print(f"outputs kept in {root}")
        else:
            shutil.rmtree(root, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
