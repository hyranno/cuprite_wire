#!/usr/bin/env python3
"""PostToolUse hook for Edit/Write: format the workspace after a Rust file of it is edited.

Runs `cargo fmt --all` rather than rustfmt on the edited file, so that the hook formats exactly
what `cargo xtask fmt` checks: files outside the crates' module trees, such as the trybuild cases
in tests/ui/ (their expected compiler messages contain line numbers), keep their layout.
A file that does not parse yet is left as it is. The hook never blocks and prints nothing.
"""

import json
import os
import subprocess
import sys


def main():
    try:
        event = json.load(sys.stdin)
    except ValueError:
        return
    path = (event.get("tool_input") or {}).get("file_path") or ""
    if not path.endswith(".rs"):
        return
    project = os.path.realpath(os.environ.get("CLAUDE_PROJECT_DIR") or event.get("cwd") or ".")
    if os.path.commonpath([project, os.path.realpath(path)]) != project:
        return
    subprocess.run(
        ["cargo", "fmt", "--all"],
        cwd=project,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


if __name__ == "__main__":
    main()
