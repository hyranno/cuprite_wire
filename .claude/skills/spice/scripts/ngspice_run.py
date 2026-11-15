#!/usr/bin/env python3
"""Run a SPICE deck through the ngspice shared library (libngspice).

Why not the `ngspice` CLI: in batch/pipe mode the CLI copies its input with glibc
tmpfile(), which always writes to /tmp and ignores $TMPDIR. /tmp is read-only in the
Claude bash sandbox, so the CLI fails with "tmpfile(): Read-only file system".
The shared library does not take that path.

Usage:
  ngspice_run.py DECK [-c CMD]... [-o LOG] [--cwd DIR]

The deck is loaded with `source`, so a `.control` block in it runs as in batch mode.
Each -c command runs afterwards (e.g. -c run -c "print all" for decks without .control).
Relative paths in the deck (.include, wrdata) resolve against --cwd (default: the
current directory). Exit status is 1 if ngspice reported an error.
"""

import argparse
import ctypes
import ctypes.util
import os
import sys
from pathlib import Path

_SEND_CHAR = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_void_p)
_SEND_STAT = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_void_p)
_EXIT = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.c_bool, ctypes.c_bool,
                         ctypes.c_int, ctypes.c_void_p)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("deck")
    ap.add_argument("-c", "--command", action="append", default=[],
                    help="ngspice command to run after loading the deck (repeatable)")
    ap.add_argument("-o", "--log", help="also write the output to this file")
    ap.add_argument("--cwd", help="working directory for relative paths")
    args = ap.parse_args()

    deck = Path(args.deck).resolve()
    if any(ch.isspace() for ch in str(deck)):
        # `source` takes the rest of the line literally, quotes included
        sys.exit(f"deck path must not contain whitespace: {deck}")
    if args.cwd:
        os.chdir(args.cwd)

    name = ctypes.util.find_library("ngspice") or "libngspice.so.0"
    lib = ctypes.CDLL(name)

    lines, errors = [], []

    def on_char(raw, _id, _user):
        text = raw.decode(errors="replace")
        stream, _, msg = text.partition(" ")
        lines.append(msg)
        print(msg, file=sys.stderr if stream == "stderr" else sys.stdout, flush=True)
        low = msg.lower()
        if stream == "stderr" and ("error" in low or "fatal" in low):
            errors.append(msg)
        return 0

    def on_stat(_raw, _id, _user):
        return 0

    def on_exit(_status, _unload, _quit, _id, _user):
        return 0

    # keep references so the callbacks are not garbage collected
    callbacks = (_SEND_CHAR(on_char), _SEND_STAT(on_stat), _EXIT(on_exit))
    lib.ngSpice_Init(callbacks[0], callbacks[1], callbacks[2], None, None, None, None)

    lib.ngSpice_Command(f"source {deck}".encode())
    for cmd in args.command:
        lib.ngSpice_Command(cmd.encode())

    if args.log:
        Path(args.log).write_text("\n".join(lines) + "\n", encoding="utf-8")
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
