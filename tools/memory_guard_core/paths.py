"""Canonical paths for memory-guard control-plane state."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path


def active_guard_marker_dir(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return the active-marker directory under the admitted artifact root."""

    source = os.environ if environ is None else environ
    raw_root = source.get("MOLT_EXT_ROOT", "").strip()
    if not raw_root:
        raw_root = next(
            (
                candidate.strip()
                for candidate in source.get(
                    "MOLT_EXTERNAL_ARTIFACT_ROOTS", ""
                ).split(os.pathsep)
                if candidate.strip()
            ),
            "",
        )
    if raw_root:
        root = Path(raw_root).expanduser()
        if not root.is_absolute():
            root = repo_root / root
    else:
        root = repo_root
    return root.resolve(strict=False) / "tmp" / "memory_guard" / "active"
