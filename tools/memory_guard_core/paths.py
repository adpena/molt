"""Canonical paths for memory-guard control-plane state."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path


def memory_guard_state_root(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path | None:
    """The custody-external root queue-guarded proofs give the guard for its state.

    Every file the memory guard family writes about itself (outer summaries,
    the harness command profile) lands under this root when it is set, so the
    watched source tree never sees an apparatus write.
    """
    source = os.environ if environ is None else environ
    state_root = source.get("MOLT_MEMORY_GUARD_STATE_ROOT", "").strip()
    if not state_root:
        return None
    root = Path(state_root).expanduser()
    if not root.is_absolute():
        root = repo_root / root
    return root.resolve(strict=False)


def harness_command_profile_log_path(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Default location of the harness guard's structured command profile."""
    state_root = memory_guard_state_root(repo_root, environ)
    if state_root is not None:
        return state_root / "harness_memory_guard" / "commands.jsonl"
    return repo_root / "logs" / "harness_memory_guard" / "commands.jsonl"


def pytest_outer_guard_summary_dir(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return the outer-guard summary directory under the admitted state root.

    Queue-guarded proofs set MOLT_MEMORY_GUARD_STATE_ROOT to a custody-external
    root; the guard's own state files must never land inside the watched
    source tree, where live custody would report them as input mutations.
    Outside a guarded run the repository tmp/ root remains the home.
    """
    state_root = memory_guard_state_root(repo_root, environ)
    if state_root is not None:
        return state_root / "pytest-memory-guard"
    return repo_root / "tmp" / "pytest-memory-guard"


def active_guard_marker_dir(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return the active-marker directory under the admitted artifact root."""

    source = os.environ if environ is None else environ
    state_root = source.get("MOLT_MEMORY_GUARD_STATE_ROOT", "").strip()
    if state_root:
        root = Path(state_root).expanduser()
        if not root.is_absolute():
            root = repo_root / root
        return root.resolve(strict=False) / "active"
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
    return root.resolve(strict=False) / "tmp" / "memory_guard" / "active"
