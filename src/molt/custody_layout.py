"""Custody layout: which durable root owns a Molt checkout's state.

A checkout family is `<root>/molt-src` plus `<root>/worktrees/<name>`. Every
member resolves to the custody root `<root>`, which owns build artifacts,
toolchains, guard state, and scratch. A clone outside that layout is its own
custody root; its scratch then goes to a per-checkout folder under the host
temp root, never into the source tree, where runtime fixtures and source
custody reject it.

This module is path logic only. It reads no environment: `molt.dx` resolves
the artifact root and the scratch storage from the environment and passes
them here.
"""

from __future__ import annotations

import hashlib
import tempfile
from pathlib import Path

from molt.path_custody import host_path_is_within

MAIN_CHECKOUT_DIRNAME = "molt-src"
WORKTREES_DIRNAME = "worktrees"
# Disk scratch is this child of the artifact root.
SCRATCH_DIRNAME = "tmp"


def custody_root(repo_root: str | Path) -> Path:
    """Return the durable custody root of a checkout or worktree."""

    root = Path(repo_root).expanduser().resolve()
    if root.name == MAIN_CHECKOUT_DIRNAME:
        return root.parent
    if root.parent.name == WORKTREES_DIRNAME:
        return root.parent.parent
    return root


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
