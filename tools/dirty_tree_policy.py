from __future__ import annotations

import re
from collections.abc import Iterable

# Tracked paths excluded from dirty-tree gates by default: WASM checksum
# sidecars legitimately churn when partner/runtime artifacts are regenerated.
DEFAULT_DIRTY_TREE_IGNORE_GLOBS = (
    "wasm/molt_runtime.wasm.sha256",
    "wasm/molt_runtime_reloc.wasm.sha256",
)

_GLOB_CACHE: dict[str, "re.Pattern[str]"] = {}


def glob_to_regex(glob: str) -> "re.Pattern[str]":
    """Translate a repo-relative POSIX glob to an anchored regex with real `**`."""
    out: list[str] = ["^"]
    i = 0
    n = len(glob)
    while i < n:
        c = glob[i]
        if c == "*":
            if i + 1 < n and glob[i + 1] == "*":
                i += 2
                if i < n and glob[i] == "/":
                    i += 1
                    out.append("(?:[^/]+/)*")
                else:
                    out.append(".*")
                continue
            out.append("[^/]*")
            i += 1
            continue
        if c == "?":
            out.append("[^/]")
            i += 1
            continue
        out.append(re.escape(c))
        i += 1
    out.append("$")
    return re.compile("".join(out))


def glob_match(path: str, glob: str) -> bool:
    norm = path.replace("\\", "/")
    pat = _GLOB_CACHE.get(glob)
    if pat is None:
        pat = glob_to_regex(glob)
        _GLOB_CACHE[glob] = pat
    return pat.match(norm) is not None


def is_ignored(path: str, ignore_globs: Iterable[str]) -> bool:
    return any(glob_match(path, glob) for glob in ignore_globs)


def short_status_path(line: str) -> str:
    """Extract the destination path from a `git status --short` line."""
    path = line[3:] if len(line) > 3 else ""
    if " -> " in path:
        return path.rsplit(" -> ", 1)[-1]
    return path


def filter_status_lines(
    status_lines: Iterable[str],
    ignore_globs: Iterable[str] = DEFAULT_DIRTY_TREE_IGNORE_GLOBS,
) -> tuple[list[str], list[str]]:
    kept: list[str] = []
    ignored: list[str] = []
    for line in status_lines:
        path = short_status_path(line)
        if path and is_ignored(path, ignore_globs):
            ignored.append(line)
        else:
            kept.append(line)
    return kept, ignored
