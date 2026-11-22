#!/usr/bin/env python3
"""PreToolUse hook for Bash: keep force pushes from discarding unseen commits.

Policy:
- A force push is allowed only as `--force-with-lease ... --force-if-includes`.
  With both, git itself rejects the push when the remote branch has commits
  that never reached the local reflog (pushed from another machine or by
  someone else), so nothing is discarded without having been seen first.
- Plain `--force`, `-f` and `+refspec` are denied with a hint to use the form
  above.
- Everything else, including commands this hook cannot parse, is passed
  through to the normal permission flow. This is a guard against accidents,
  not against deliberate evasion; it never asks.

Branch protection (main) is enforced by GitHub and which repositories can be
pushed to at all by the GitHub token, not by this hook.
"""

import json
import os
import re
import shlex
import sys

SEPARATORS = {"&&", "||", ";", "|", "&", "\n", "(", ")", ";;", "|&"}
# git global options that take a separate argument
GIT_GLOBAL_WITH_ARG = {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path"}
# git push options that take a separate argument
PUSH_WITH_ARG = {"-o", "--push-option", "--repo", "--receive-pack", "--exec"}
HEREDOC_RE = re.compile(
    r"(?<!<)<<(?!<)(-?)\s*(?:'([^']*)'|\"([^\"]*)\"|\\?([A-Za-z_][A-Za-z0-9_]*))"
)
MAX_DEPTH = 5
SAFE_FORM = "git push --force-with-lease --force-if-includes ..."


def strip_heredocs(command):
    """Drop here-document bodies; they are data, not commands."""
    kept, pending = [], []
    for line in command.split("\n"):
        if pending:
            strip_tabs, delim = pending[0]
            if (line.lstrip("\t") if strip_tabs else line) == delim:
                pending.pop(0)
            continue
        kept.append(line)
        for m in HEREDOC_RE.finditer(line):
            delim = next(g for g in m.groups()[1:] if g is not None)
            pending.append((m.group(1) == "-", delim))
    return "\n".join(kept)


def tokenize(command):
    lexer = shlex.shlex(command, posix=True, punctuation_chars="();<>|&\n")
    lexer.whitespace = " \t\r"
    lexer.whitespace_split = True
    return list(lexer)


def push_args(tokens, i):
    """If tokens[i] starts a `git ... push` invocation, return the push args."""
    if os.path.basename(tokens[i]) != "git":
        return None
    i += 1
    while i < len(tokens) and tokens[i].startswith("-"):
        i += 2 if tokens[i] in GIT_GLOBAL_WITH_ARG else 1
    if i >= len(tokens) or tokens[i] != "push":
        return None
    end = i + 1
    while end < len(tokens) and tokens[end] not in SEPARATORS:
        end += 1
    return tokens[i + 1:end]


def check_push(args):
    """Return a deny reason, or None if the push is not an unsafe force push."""
    plain_force = lease = if_includes = False
    plus_specs = []
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--":
            plus_specs += [a for a in args[i + 1:] if a.startswith("+")]
            break
        name = arg.split("=", 1)[0]
        if name == "--force":
            plain_force = True
        elif name == "--force-with-lease":
            lease = True
        elif name == "--force-if-includes":
            if_includes = True
        elif arg.startswith("--"):
            if name in PUSH_WITH_ARG and "=" not in arg:
                i += 1
        elif arg.startswith("-") and len(arg) > 1:
            if "f" in arg[1:]:
                plain_force = True
            if arg.endswith("o"):
                i += 1
        elif arg.startswith("+"):
            plus_specs.append(arg)
        i += 1

    if plain_force:
        return f"Plain force push is not allowed; use `{SAFE_FORM}`."
    if plus_specs:
        return f"Forced refspec {plus_specs[0]} is not allowed; use `{SAFE_FORM}`."
    if lease and not if_includes:
        return (
            "--force-with-lease without --force-if-includes can discard remote commits "
            f"fetched but never reviewed; use `{SAFE_FORM}`."
        )
    return None


def check_command(command, depth=0):
    if depth > MAX_DEPTH:
        return None
    try:
        tokens = tokenize(strip_heredocs(command))
    except ValueError:
        return None  # unparseable: leave it to the normal permission flow
    for i, token in enumerate(tokens):
        args = push_args(tokens, i)
        if args is not None:
            reason = check_push(args)
        elif i > 0 and (tokens[i - 1] == "eval" or re.fullmatch(r"-[a-z]*c", tokens[i - 1])):
            # `bash -c "git push ..."`, `eval '...'`
            reason = check_command(token, depth + 1)
        else:
            reason = None
        if reason:
            return reason
    return None


def main():
    data = json.load(sys.stdin)
    if data.get("tool_name") != "Bash":
        return
    command = data.get("tool_input", {}).get("command", "")
    if "git" not in command or "push" not in command:
        return
    reason = check_command(command)
    if reason:
        json.dump(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            },
            sys.stdout,
        )


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # fail open, but make the bug visible
        print(f"guard-git-push hook error: {e!r}", file=sys.stderr)
        sys.exit(1)
