"""Canonical Python spellings of the shipped Molt CLI payload."""

from __future__ import annotations

import os
from pathlib import Path

MOLT_MODULE_TARGETS = frozenset(
    {"molt", "molt.cli", "molt.__main__", "molt.cli.__main__", "molt.cli.entrypoint"}
)
MOLT_SCRIPT_TARGETS = frozenset(
    {
        "src/molt",
        "src/molt/cli",
        "src/molt/__main__.py",
        "src/molt/cli/__main__.py",
        "src/molt/cli/entrypoint.py",
    }
)


def is_molt_cli_payload(mode: str, target: str | None, *, repo_root: Path) -> bool:
    if mode == "module":
        return target in MOLT_MODULE_TARGETS
    if mode != "script" or target is None:
        return False
    candidate = Path(target)
    if candidate.is_absolute():
        try:
            target = (
                candidate.resolve(strict=False)
                .relative_to(repo_root.resolve())
                .as_posix()
            )
        except ValueError:
            return False
    normalized = os.path.normpath(target).replace("\\", "/")
    return normalized in MOLT_SCRIPT_TARGETS
