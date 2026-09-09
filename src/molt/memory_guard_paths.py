"""Canonical paths for packaged and repository memory-guard control-plane state."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path


def memory_guard_state_root(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return the sole control-state root for one guarded process tree."""

    source = os.environ if environ is None else environ
    state_root = source.get("MOLT_MEMORY_GUARD_STATE_ROOT", "").strip()
    if state_root:
        root = Path(state_root).expanduser()
        if not root.is_absolute():
            root = repo_root / root
        return root.resolve(strict=False)
    raw_root = source.get("MOLT_EXT_ROOT", "").strip()
    if not raw_root:
        raw_root = next(
            (
                candidate.strip()
                for candidate in source.get("MOLT_EXTERNAL_ARTIFACT_ROOTS", "").split(
                    os.pathsep
                )
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
    return root.resolve(strict=False) / "tmp" / "memory_guard"


def active_guard_marker_dir(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return the active-marker directory under the admitted artifact root."""

    return memory_guard_state_root(repo_root, environ) / "active"


def pytest_guard_summary_dir(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return pytest custody beside, never inside, memory-guard state.

    Queue runs provide an external ``MOLT_MEMORY_GUARD_STATE_ROOT``.  Projecting
    pytest state from that same root prevents current-test and sentinel receipts
    from mutating the source checkout while keeping one custody authority for
    ordinary, externally rooted and proof-queue executions.
    """

    return memory_guard_state_root(repo_root, environ).parent / "pytest-memory-guard"


def harness_guard_artifact_dir(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Keep command profiles and harness events beside canonical guard state.

    Queue custody supplies an external state root. All default harness outputs
    must follow that root too: an incident log inside source would invalidate
    the proof whose failure it is recording.
    """

    return memory_guard_state_root(repo_root, environ).parent / "harness_memory_guard"
