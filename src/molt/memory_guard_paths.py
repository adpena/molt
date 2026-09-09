"""Canonical paths for packaged and repository memory-guard control-plane state."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path


def memory_guard_state_root(
    repo_root: Path,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return the sole control-state root for one guarded process tree.

    Resolve against the effective command environment at custody boundaries,
    never at module import: DX may project hosted-CI or queue roots after the
    guard modules have loaded. An explicit environment is complete authority;
    do not merge a reader's ambient environment into a child's custody.
    """

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


def pytest_custody_artifact_path(
    repo_root: Path,
    kind: str,
    suffix: str,
    *,
    environ: Mapping[str, str] | None = None,
    pid: int | None = None,
) -> Path:
    """Name one custody record under the effective pytest evidence root."""
    safe_kind = "".join(ch if ch.isalnum() else "-" for ch in kind.lower()).strip("-")
    safe_suffix = "".join(ch if ch.isalnum() else "-" for ch in suffix.lower()).strip(
        "-"
    )
    return pytest_guard_summary_dir(repo_root, environ) / (
        f"{safe_kind or 'pytest'}-{os.getpid() if pid is None else pid}_{safe_suffix}.json"
    )


def pytest_custody_path_is_canonical(
    repo_root: Path,
    path: Path,
    *,
    environ: Mapping[str, str] | None = None,
) -> bool:
    """Validate custody against the command's root, not the observer's root."""
    try:
        path.resolve(strict=False).relative_to(
            pytest_guard_summary_dir(repo_root, environ).resolve(strict=False)
        )
    except ValueError:
        return False
    return True


def canonical_pytest_current_test_file_path(
    repo_root: Path,
    raw_path: str | None,
    *,
    fallback_kind: str,
    environ: Mapping[str, str] | None = None,
    fallback_pid: int | None = None,
) -> Path:
    """Preserve admitted parent selection; otherwise allocate the caller's role."""
    if raw_path:
        path = Path(raw_path).expanduser()
        if not path.is_absolute():
            path = repo_root / path
        path = path.resolve(strict=False)
        if pytest_custody_path_is_canonical(repo_root, path, environ=environ):
            return path
    return pytest_custody_artifact_path(
        repo_root,
        fallback_kind,
        "current-test",
        environ=environ,
        pid=fallback_pid,
    )
