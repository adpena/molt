"""Compiler input-source authority, independent of guest projects and outputs."""

from __future__ import annotations

import os
from pathlib import Path


MOLT_SOURCE_ROOT_ENV = "MOLT_SOURCE_ROOT"
_DEFAULT_COMPILER_SOURCE_ROOT = Path(__file__).resolve().parents[2]


def resolve_path_override(env_var: str) -> Path | None:
    """Resolve an optional environment path without requiring it to exist."""
    override = os.environ.get(env_var)
    if not override:
        return None
    path = Path(override).expanduser()
    if not path.is_absolute():
        path = Path.cwd() / path
    return path.resolve(strict=False)


def compiler_source_root_override() -> Path | None:
    """Preserve explicit source selection, including invalid paths.

    Source-marker validation belongs to the consuming boundary. An invalid
    explicit selection must never silently fall back to another source tree.
    """
    return resolve_path_override(MOLT_SOURCE_ROOT_ENV)


def compiler_source_root() -> Path:
    """Return compiler inputs, never a writable artifact or guest-project root."""
    return compiler_source_root_override() or _DEFAULT_COMPILER_SOURCE_ROOT
