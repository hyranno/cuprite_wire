#!/usr/bin/env python3
"""PreToolUse hook for Bash: guard destructive `git push`.

Policy:
- Non-destructive pushes (new branch, fast-forward) are left to the normal
  permission flow (no output, exit 0).
- Force pushes are denied unless every commit they would discard from the
  remote branch is authored by `git config user.email`.
- Force pushes to protected branches are always denied.
- Branch/tag deletion, --mirror, --prune, tag overwrites, wildcard refspecs and
  commands that cannot be parsed reliably are escalated to "ask".

Which repositories can be pushed to at all is restricted by the GitHub token,
not by this hook.
"""

import json
import os
import re
import shlex
import subprocess
import sys

PROTECTED_BRANCHES = {"main", "master"}
LS_REMOTE_TIMEOUT = 30

# git global options that take a separate argument
GIT_GLOBAL_WITH_ARG = {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path"}
# git push options that take a separate argument
PUSH_WITH_ARG = {"-o", "--push-option", "--repo", "--receive-pack", "--exec"}
SEPARATORS = {"&&", "||", ";", "|", "&", "\n", "(", ")", ";;", "|&"}
# prefixes that run the rest of the segment as a command (their options are skipped)
PREFIX_WRAPPERS = {"sudo", "nohup", "nice", "time", "command", "exec", "builtin", "timeout"}
# shells whose `-c SCRIPT` is parsed recursively
SHELLS = {"sh", "bash", "zsh", "dash", "ksh"}
# interpreters whose inline code cannot be parsed; asked when it mentions git push
INTERPRETERS = {"python", "python3", "node", "perl", "ruby"}
# commands that run another command given as arguments
ARG_RUNNERS = {"xargs", "find", "parallel", "watch", "flock"}
HEREDOC_RE = re.compile(
    r"(?<!<)<<(?!<)(-?)\s*(?:'([^']*)'|\"([^\"]*)\"|\\?([A-Za-z_][A-Za-z0-9_]*))"
)
GIT_PUSH_RE = re.compile(r"\bgit\b.*\bpush\b", re.S)
MAX_DEPTH = 5


class Decision(Exception):
    def __init__(self, kind, reason):
        super().__init__(reason)
        self.kind = kind
        self.reason = reason


def ask(reason):
    raise Decision("ask", reason)


def deny(reason):
    raise Decision("deny", reason)


def git(cwd, *args, timeout=None):
    result = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, timeout=timeout
    )
    return result.returncode, result.stdout.strip(), result.stderr.strip()


def strip_heredocs(command):
    """Drop here-document bodies; they are data, not commands."""
    lines = command.split("\n")
    kept, pending = [], []
    for line in lines:
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


def split_segments(command):
    lexer = shlex.shlex(command, posix=True, punctuation_chars="();<>|&\n")
    lexer.whitespace = " \t\r"
    lexer.whitespace_split = True
    segments, current = [], []
    for token in lexer:
        if token in SEPARATORS:
            if current:
                segments.append(current)
            current = []
        else:
            current.append(token)
    if current:
        segments.append(current)
    return segments


def parse_git_invocation(tokens, cwd):
    """Return (cwd, subcommand_args) if tokens are a git command, else None."""
    i = 0
    # skip `env VAR=x`, plain `VAR=x` prefixes
    while i < len(tokens) and (tokens[i] == "env" or re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", tokens[i])):
        i += 1
    if i >= len(tokens) or os.path.basename(tokens[i]) != "git":
        return None
    i += 1
    while i < len(tokens) and tokens[i].startswith("-"):
        opt = tokens[i]
        if opt in GIT_GLOBAL_WITH_ARG:
            if i + 1 >= len(tokens):
                return None
            if opt == "-C":
                cwd = os.path.join(cwd, tokens[i + 1])
            i += 2
        else:
            i += 1
    return cwd, tokens[i:]


def current_branch(cwd):
    code, out, _ = git(cwd, "symbolic-ref", "--quiet", "--short", "HEAD")
    if code != 0:
        ask("git push from a detached HEAD without an explicit destination")
    return out


def default_remote_and_branch(cwd):
    code, out, _ = git(cwd, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{push}")
    if code == 0 and "/" in out:
        remote, branch = out.split("/", 1)
        return remote, branch
    return None, current_branch(cwd)


def normalize_branch(dst, cwd):
    if dst == "HEAD":
        return current_branch(cwd)
    if dst.startswith("refs/tags/"):
        ask(f"git push overwrites tag {dst}")
    if dst.startswith("refs/heads/"):
        return dst[len("refs/heads/"):]
    if dst.startswith("refs/"):
        ask(f"git push to non-branch ref {dst}")
    return dst


def check_push(cwd, args):
    force_all = False
    remote = None
    refspecs = []
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--":
            refspecs_raw = args[i + 1:]
            if remote is None and refspecs_raw:
                remote, refspecs_raw = refspecs_raw[0], refspecs_raw[1:]
            refspecs.extend(refspecs_raw)
            break
        if arg.startswith("--"):
            name = arg.split("=", 1)[0]
            if name in ("--force", "--force-with-lease", "--force-if-includes"):
                force_all = True
            elif name in ("--delete", "--mirror", "--prune"):
                ask(f"git push {name} removes remote refs")
            elif name in ("--tags", "--follow-tags"):
                pass
            elif name in PUSH_WITH_ARG and "=" not in arg:
                i += 1
            i += 1
            continue
        if arg.startswith("-") and len(arg) > 1:
            flags = arg[1:]
            if "f" in flags:
                force_all = True
            if "d" in flags:
                ask("git push -d removes remote refs")
            if flags.endswith("o"):
                i += 1
            i += 1
            continue
        if remote is None:
            remote = arg
        else:
            refspecs.append(arg)
        i += 1

    targets = []  # (force, src, branch)
    if not refspecs:
        default_remote, branch = default_remote_and_branch(cwd)
        remote = remote or default_remote or "origin"
        targets.append((force_all, "HEAD", branch))
    for spec in refspecs:
        force = force_all or spec.startswith("+")
        spec = spec.lstrip("+")
        if "*" in spec:
            ask(f"git push with wildcard refspec {spec}")
        if spec.startswith(":"):
            ask(f"git push {spec} deletes a remote ref")
        src, _, dst = spec.partition(":")
        if not dst:
            dst = current_branch(cwd) if src == "HEAD" else src
        if src.startswith("refs/tags/") or dst.startswith("refs/tags/"):
            if force:
                ask(f"git push force-updates tag {spec}")
            continue
        targets.append((force, src, normalize_branch(dst, cwd)))

    for force, src, branch in targets:
        if force:
            check_force(cwd, remote, src, branch)


def check_force(cwd, remote, src, branch):
    if branch in PROTECTED_BRANCHES:
        deny(f"Force push to protected branch '{branch}' is not allowed.")

    code, author, _ = git(cwd, "config", "user.email")
    if code != 0 or not author:
        deny("Force push denied: git config user.email is not set.")

    code, local_sha, _ = git(cwd, "rev-parse", "--verify", "--quiet", f"{src}^{{commit}}")
    if code != 0:
        ask(f"Cannot resolve local ref '{src}' for force push")

    try:
        code, out, err = git(
            cwd, "ls-remote", "--", remote, f"refs/heads/{branch}", timeout=LS_REMOTE_TIMEOUT
        )
    except subprocess.TimeoutExpired:
        deny(f"Force push denied: ls-remote {remote} timed out.")
    if code != 0:
        deny(f"Force push denied: cannot query {remote}: {err}")
    if not out:
        return  # new branch: nothing is discarded
    remote_sha = out.split()[0]

    code, _, _ = git(cwd, "cat-file", "-e", f"{remote_sha}^{{commit}}")
    if code != 0:
        deny(
            f"Force push denied: remote '{branch}' is at {remote_sha[:12]}, which is not "
            f"available locally. Run `git fetch {remote}` and review it first."
        )

    code, out, err = git(cwd, "log", "--format=%H %ae", remote_sha, "--not", local_sha)
    if code != 0:
        deny(f"Force push denied: git log failed: {err}")
    foreign = [line for line in out.splitlines() if line.split(" ", 1)[1] != author]
    if foreign:
        listing = "\n".join(f"  {line[:12]} {line.split(' ', 1)[1]}" for line in foreign[:10])
        deny(
            f"Force push to '{branch}' would discard commits not authored by {author}:\n{listing}"
        )


def unwrap(tokens):
    """Strip `sudo`/`timeout 5`/... prefixes and return the wrapped command."""
    while tokens and tokens[0] in PREFIX_WRAPPERS:
        head, tokens = tokens[0], tokens[1:]
        while tokens and tokens[0].startswith("-"):
            tokens = tokens[1:]
        if head == "timeout" and tokens:
            tokens = tokens[1:]  # duration
    return tokens


def check_segment(tokens, cwd, depth):
    tokens = unwrap(tokens)
    if not tokens:
        return
    head = os.path.basename(tokens[0])
    if head in SHELLS:
        for i, t in enumerate(tokens[1:], 1):
            if re.fullmatch(r"-[a-z]*c[a-z]*", t) and i + 1 < len(tokens):
                check_command(tokens[i + 1], cwd, depth + 1)
                return
        return
    if head == "eval":
        check_command(" ".join(tokens[1:]), cwd, depth + 1)
        return
    if head in INTERPRETERS:
        if any(GIT_PUSH_RE.search(t) for t in tokens[1:]):
            ask(f"git push inside {head} code cannot be checked")
        return
    if head in ARG_RUNNERS:
        if "git" in tokens and "push" in tokens:
            ask(f"git push run through {head} cannot be checked")
        return
    parsed = parse_git_invocation(tokens, cwd)
    if parsed is None:
        return
    git_cwd, sub = parsed
    if not sub or sub[0] != "push":
        return
    if any("$" in t or "`" in t for t in sub):
        ask("git push with shell expansions cannot be checked")
    check_push(git_cwd, sub[1:])


def check_command(command, cwd, depth):
    if depth > MAX_DEPTH:
        ask("Too deeply nested command containing git push")
    try:
        segments = split_segments(strip_heredocs(command))
    except ValueError:
        ask("Could not parse a command containing git push")
    for tokens in segments:
        if tokens[0] == "cd" and len(tokens) == 2:
            cwd = os.path.join(cwd, os.path.expanduser(tokens[1]))
            continue
        check_segment(tokens, cwd, depth)


def main():
    data = json.load(sys.stdin)
    if data.get("tool_name") != "Bash":
        return
    command = data.get("tool_input", {}).get("command", "")
    if not re.search(r"\bgit\b", command) or "push" not in command:
        return
    cwd = data.get("cwd") or os.getcwd()

    try:
        check_command(command, cwd, 0)
    except Exception as e:  # fail closed: escalate unexpected errors to the user
        if not isinstance(e, Decision):
            e = Decision("ask", f"guard-git-push hook error: {e!r}")
        d = e
        json.dump(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": d.kind,
                    "permissionDecisionReason": d.reason,
                }
            },
            sys.stdout,
        )


if __name__ == "__main__":
    main()
