#!/usr/bin/env python3
"""PreToolUse hook for Bash: refuse commands that change files through the
shell instead of the Edit tool (or Write, for a new file).

Blocked:
  * `sed -i` / `sed --in-place`, `perl -i` / `-pi`;
  * an inline interpreter script (python, python3, perl, ruby, node) that
    writes files: `open(..., 'w'/'a'/'x')`, `write_text`, `write_bytes`,
    `shutil.copy*` / `shutil.move`, `os.rename` / `os.replace`, `fs.writeFile`;
  * output redirection (`>`, `>>`, `&>`) or `tee` into a path outside /tmp
    and /dev;
  * `cp`, `mv`, `install` or `dd of=` with a destination outside /tmp and /dev.

Allowed: writes under /tmp (the scratchpad lives there) and /dev, `2>&1`,
heredocs fed to commands such as `git commit -F -`, git itself, cargo, and
every read-only command.

Exit 2 with a message on stderr blocks the call and tells Claude why.
"""

import json
import re
import shlex
import sys

ALLOWED_PREFIXES = ("/tmp/", "/dev/")
INTERPRETERS = re.compile(r"(?:^|[\s;&|(])(?:python3?|perl|ruby|node)(?:\s|$)")
INTERPRETER_WRITES = re.compile(
    r"""open\([^)]*['"][wax]b?\+?['"]"""
    r"|\.write_text\(|\.write_bytes\("
    r"|shutil\.(?:copy\w*|move)\("
    r"|os\.(?:rename|replace)\("
    r"|fs\.writeFile"
)
SED_IN_PLACE = re.compile(r"(?:^|[\s;&|(])sed\s(?:[^;&|]*\s)?(?:-[a-zA-Z]*i|--in-place)")
PERL_IN_PLACE = re.compile(r"(?:^|[\s;&|(])perl\s(?:[^;&|]*\s)?-[a-zA-Z]*i")


def strip_heredoc_bodies(command):
    """Drop the bodies of heredocs; their text is data, not shell."""
    lines = command.split("\n")
    out = []
    terminator = None
    for line in lines:
        if terminator is not None:
            if line.strip() == terminator:
                terminator = None
            continue
        out.append(line)
        match = re.search(r"<<-?\s*['\"]?(\w+)['\"]?", line)
        if match:
            terminator = match.group(1)
    return "\n".join(out)


def strip_quotes(text):
    """Blank out quoted strings so a `>` inside one is not a redirection."""
    text = re.sub(r"'[^']*'", "''", text)
    return re.sub(r'"(?:\\.|[^"\\])*"', '""', text)


def expand_simple_assignments(text):
    """Resolve `NAME=value` assignments made in the same command, so
    `G=/tmp/x.log; ... > $G` is seen as a write under /tmp."""
    values = dict(re.findall(r"(?:^|[\s;&(])([A-Za-z_]\w*)=([^\s;&|]+)", text))

    def resolve(match):
        name = match.group(1) or match.group(2)
        return values.get(name, match.group(0))

    return re.sub(r"\$\{(\w+)\}|\$(\w+)", resolve, text)


def outside_allowed(path):
    path = path.strip("'\"")
    if not path or path.startswith("&"):
        return False
    return not path.startswith(ALLOWED_PREFIXES)


def blocked_reason(command):
    if INTERPRETERS.search(command) and INTERPRETER_WRITES.search(command):
        return "an inline interpreter script writes files"

    shell = strip_quotes(strip_heredoc_bodies(command))
    if SED_IN_PLACE.search(shell):
        return "`sed -i` edits a file in place"
    if PERL_IN_PLACE.search(shell):
        return "`perl -i` edits a file in place"

    shell = expand_simple_assignments(shell)
    for match in re.finditer(r"(?<![<>&\d])(?:\d|&)?>>?(?!&)\s*([^\s;&|()<>]+)", shell):
        if outside_allowed(match.group(1)):
            return f"output is redirected into `{match.group(1)}`"
    for match in re.finditer(r"(?:^|[\s;&|(])tee\s+((?:-\w+\s+)*)([^\s;&|()<>]+)", shell):
        if outside_allowed(match.group(2)):
            return f"`tee` writes `{match.group(2)}`"
    for match in re.finditer(r"(?:^|[\s;&|(])(cp|mv|install)\s+([^;&|\n]+)", shell):
        try:
            words = [w for w in shlex.split(match.group(2)) if not w.startswith("-")]
        except ValueError:
            words = match.group(2).split()
        if words and outside_allowed(words[-1]):
            return f"`{match.group(1)}` writes `{words[-1]}`"
    for match in re.finditer(r"(?:^|[\s;&|(])dd\s[^;&|\n]*\bof=([^\s;&|]+)", shell):
        if outside_allowed(match.group(1)):
            return f"`dd` writes `{match.group(1)}`"
    return None


def main():
    payload = json.load(sys.stdin)
    command = payload.get("tool_input", {}).get("command", "")
    reason = blocked_reason(command)
    if reason is None:
        return 0
    sys.stderr.write(
        f"Blocked: {reason}. In this repository files are changed with the "
        "Edit tool (or Write for a new file), never through the shell, so "
        "every change is visible and exact. Writes under /tmp and /dev are "
        "allowed.\n"
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
