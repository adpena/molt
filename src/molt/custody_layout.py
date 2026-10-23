"""Custody layout: which durable root owns a Molt checkout's state.

A checkout family is `<root>/molt-src` plus `<root>/worktrees/<name>`. Every
member resolves to the custody root `<root>`, which owns build artifacts,
toolchains, guard state, and scratch. A clone outside that layout is its own
custody root; its scratch then goes to a per-checkout folder under the host
temp root, never into the source tree, where runtime fixtures and source
custody reject it.

Git metadata fences automatic roots out of enclosing checkouts. This module
reads no environment and launches no Git process: `molt.dx` resolves artifact
and scratch overrides separately.
"""

from __future__ import annotations

import hashlib
import os
import re
import stat
import tempfile
from pathlib import Path

from molt.path_custody import host_path_is_within

MAIN_CHECKOUT_DIRNAME = "molt-src"
WORKTREES_DIRNAME = "worktrees"
# Disk scratch is this child of the artifact root.
SCRATCH_DIRNAME = "tmp"
# Git bundles of retired, unlanded branches: a child of the custody root.
BUNDLES_DIRNAME = "drift-bundles"
# The SPRAWL threshold (M67): at most this many live worktrees, and `lane
# start` refuses a lane that would pass it (owner, 2026-10-22: 12).
LIVE_WORKTREE_LIMIT = 12
# Lanes reuse the slot worktrees `<root>/worktrees/slot-01` to `slot-12`. A
# released slot stays registered and detached, so the next lane inherits its
# path-keyed Cargo target, sccache keys and checkout inputs warm (HF-491). A
# free slot is no live worktree.
LANE_SLOT_LIMIT = LIVE_WORKTREE_LIMIT
_LANE_SLOT_NAME = re.compile(r"slot-([0-9]{2})")
_GIT_PATH_BYTES = 64 * 1024


def _read_git_path(path: Path) -> str:
    with path.open("rb") as stream:
        raw = stream.read(_GIT_PATH_BYTES + 1)
    if len(raw) > _GIT_PATH_BYTES:
        raise ValueError(
            f"Git path metadata exceeds {_GIT_PATH_BYTES} byte limit: {path}"
        )
    # Git path records end with LF (or CRLF), not arbitrary whitespace.
    # Filesystem decoding preserves non-ASCII and Unix path bytes.
    return os.fsdecode(raw.removesuffix(b"\n").removesuffix(b"\r"))


def git_directories(repo_root: Path) -> tuple[Path, Path] | None:
    """Read the checkout's Git/common directories without launching Git.

    Absence is a plain source tree. Present but unreadable, malformed or stale
    metadata cannot authorize an automatic output placement.
    """
    dot_git = repo_root / ".git"
    try:
        try:
            metadata = dot_git.lstat()
        except FileNotFoundError:
            return None
        if stat.S_ISDIR(metadata.st_mode):
            git_dir = dot_git
        elif dot_git.is_dir():
            git_dir = dot_git.resolve()
        else:
            if not dot_git.is_file():
                raise ValueError(
                    f"Git checkout metadata is not a regular file: {dot_git}"
                )
            pointer = _read_git_path(dot_git)
            if not pointer.startswith("gitdir: ") or not (
                target := pointer.removeprefix("gitdir: ")
            ):
                raise ValueError(f"malformed Git checkout metadata: {dot_git}")
            git_dir = (repo_root / target).resolve()
        if not git_dir.is_dir() or not (git_dir / "HEAD").is_file():
            raise ValueError(f"stale Git checkout metadata: {dot_git}")
        commondir = git_dir / "commondir"
        try:
            commondir.lstat()
        except FileNotFoundError:
            common = git_dir
        else:
            if not commondir.is_file():
                raise ValueError(
                    f"Git common directory metadata is not a regular file: {commondir}"
                )
            common_target = _read_git_path(commondir)
            if not common_target:
                raise ValueError(f"malformed Git common directory: {commondir}")
            common = (git_dir / common_target).resolve()
            if not common.is_dir():
                raise ValueError(f"stale Git common directory: {commondir}")
    except (OSError, UnicodeError, RuntimeError) as exc:
        raise ValueError(f"cannot read Git checkout metadata: {dot_git}") from exc
    return git_dir, common


def enclosing_checkouts(path: str | Path) -> tuple[Path, ...]:
    """Return Git checkout boundaries from the given path outwards.

    A linked worktree's .git pointer is admitted by the same reader as DX HEAD
    inspection. Bare repositories have no working-tree marker and are not
    source checkout boundaries.
    """
    root = Path(path).expanduser().resolve()
    return tuple(
        candidate
        for candidate in (root, *root.parents)
        if git_directories(candidate) is not None
    )


def custody_root(repo_root: str | Path) -> Path:
    """Return automatic custody without entering another source checkout."""
    root = Path(repo_root).expanduser().resolve()
    if root.name == MAIN_CHECKOUT_DIRNAME:
        custody = root.parent
    elif root.parent.name == WORKTREES_DIRNAME:
        custody = root.parent.parent
    else:
        custody = root
    for checkout in enclosing_checkouts(root):
        # A standalone plain clone retains Cargo's repo-local output norm.
        # An enclosing checkout is another source owner, even for an unrelated
        # nested clone rather than a Git-linked worktree.
        if checkout != root and host_path_is_within(custody, checkout):
            custody = checkout.parent
    return custody


def family_worktrees_root(main_checkout: str | Path) -> Path | None:
    """`<root>/worktrees` for a main checkout at `<root>/molt-src`, else None.

    A lane worktree there resolves to the same custody root as the main
    checkout. A clone outside the family layout has no such place: its own
    directory would nest the worktrees inside its source tree.
    """
    root = Path(main_checkout).expanduser().resolve()
    if root.name != MAIN_CHECKOUT_DIRNAME:
        return None
    return root.parent / WORKTREES_DIRNAME


def lane_slot_path(worktrees_root: str | Path, index: int) -> Path:
    """The slot worktree path with number `index` (1-based)."""
    return Path(worktrees_root) / f"slot-{index:02d}"


def lane_slot_index(path: str | Path, worktrees_root: str | Path | None) -> int | None:
    """The slot number of `path` when it is a lane slot under `worktrees_root`."""
    if worktrees_root is None:
        return None
    candidate = Path(path)
    match = _LANE_SLOT_NAME.fullmatch(candidate.name)
    if match is None or not 1 <= int(match.group(1)) <= LANE_SLOT_LIMIT:
        return None
    if candidate.resolve().parent != Path(worktrees_root).resolve():
        return None
    return int(match.group(1))


def _identity_name(path: str | Path) -> str:
    resolved = Path(path).expanduser().resolve()
    identity = hashlib.sha256(str(resolved).encode("utf-8")).hexdigest()[:12]
    return f"molt-{identity}"


def out_of_tree_scratch_root(source_root: str | Path) -> Path:
    """Return the per-checkout scratch folder under the host temp root.

    `RunContext` exports this folder as the child's `TMPDIR`. A child whose
    temp root already lies in it resolves the same folder, so a parent and its
    children agree on one scratch root.
    """

    name = _identity_name(source_root)
    base = Path(tempfile.gettempdir()).resolve()
    for candidate in (base, *base.parents):
        if candidate.name == name:
            return candidate
    return base / name


def disk_scratch_roots_of(artifact_root: str | Path) -> tuple[Path, Path]:
    """Return both disk scratch roots an artifact root can have.

    A checkout outside the root keeps scratch at `<root>/tmp`; a checkout
    that is its own artifact root keeps it in its out-of-tree folder. An
    observer that sees only the artifact root (a reclaimer, a preflight)
    must look at both.
    """

    root = Path(artifact_root).expanduser().resolve()
    return (root / SCRATCH_DIRNAME, out_of_tree_scratch_root(root))


def scratch_root(
    artifact_root: str | Path,
    source_root: str | Path,
    *,
    memory_root: str | Path | None = None,
) -> Path:
    """Return the scratch root for an artifact root, never inside the checkout.

    Disk storage is `<artifact root>/tmp`, or the per-checkout host temp
    folder when that path falls inside the checkout. Memory storage is one
    folder per artifact root under the operator's memory-backed root.
    """

    if memory_root is not None:
        return Path(memory_root) / _identity_name(artifact_root)
    scratch = Path(artifact_root) / SCRATCH_DIRNAME
    if host_path_is_within(scratch, source_root):
        return out_of_tree_scratch_root(source_root)
    return scratch
