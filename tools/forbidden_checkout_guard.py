#!/usr/bin/env python3
"""Pure path policy for the retired OneDrive Molt checkout."""

from __future__ import annotations

import ntpath
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

FORBIDDEN_ROOT = r"C:\Users\adpen\OneDrive\Documents\molt"
MUTATING_TOOLS = frozenset({"Write", "Edit", "MultiEdit", "NotebookEdit"})
_BUILD_RE = re.compile(
    r"(?i)(?:^|[;&|]\s*)(?:cargo|uv|python|python3|molt|cmake|ninja|meson|pip)\b"
)
_GIT_MUTATION_RE = re.compile(
    r"(?i)\bgit\s+(?:add|am|apply|branch\s+-[dD]|checkout|cherry-pick|clean|commit|merge|mv|rebase|reset|restore|revert|rm|stash|switch|tag|worktree)\b"
)
_WRITE_RE = re.compile(
    r"(?i)(?:^|[;&|]\s*)(?:set-content|add-content|out-file|new-item|copy-item|move-item|remove-item|mkdir|echo\b.*(?:>|>>)|tee\b)"
)
_WIN_PATH_RE = re.compile(r"(?i)[a-z]:[\\/][^\s\"']+")


@dataclass(frozen=True)
class Decision:
    block: bool = False
    rule: str = ""
    reason: str = ""


def _norm(path: str) -> str:
    return ntpath.normcase(ntpath.normpath(path.replace("/", "\\")))


def is_forbidden_path(path: str | Path) -> bool:
    try:
        candidate = _norm(str(path))
        root = _norm(FORBIDDEN_ROOT)
        return candidate == root or candidate.startswith(root + "\\")
    except Exception:
        return False


def _tool_paths(tool_input: Mapping[str, Any]) -> list[str]:
    return [
        str(tool_input[key])
        for key in ("file_path", "path", "notebook_path", "planFilePath")
        if isinstance(tool_input.get(key), str)
    ]


def _bash_targets_forbidden(command: str, cwd: str) -> bool:
    if is_forbidden_path(cwd):
        return bool(
            _BUILD_RE.search(command)
            or _GIT_MUTATION_RE.search(command)
            or _WRITE_RE.search(command)
        )
    return any(
        is_forbidden_path(match.group(0)) for match in _WIN_PATH_RE.finditer(command)
    )


def decide(tool_name: str, tool_input: Mapping[str, Any], cwd: str = "") -> Decision:
    try:
        if tool_name in MUTATING_TOOLS:
            if any(is_forbidden_path(path) for path in _tool_paths(tool_input)):
                return Decision(
                    True,
                    "forbidden-checkout-mutation",
                    "retired OneDrive checkout is read-only; use C:\\Molt",
                )
            return Decision()
        if tool_name == "Bash" and _bash_targets_forbidden(
            str(tool_input.get("command", "")), cwd
        ):
            return Decision(
                True,
                "forbidden-checkout-mutation",
                "build/git/write targets retired OneDrive checkout; use C:\\Molt",
            )
        return Decision()
    except Exception:
        return Decision()


def self_test() -> bool:
    bad = decide("Write", {"file_path": FORBIDDEN_ROOT + r"\x.txt"})
    good = decide("Write", {"file_path": r"C:\Molt\molt-src\x.txt"})
    build = decide("Bash", {"command": "cargo check"}, FORBIDDEN_ROOT)
    return bad.block and build.block and not good.block
