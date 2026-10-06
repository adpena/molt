"""Custody layout: which durable root owns a Molt checkout's state.

A checkout family is `<root>/molt-src` plus `<root>/worktrees/<name>`. Every
member resolves to the custody root `<root>`, which owns build artifacts,
toolchains, guard state, and scratch. A clone outside that layout is its own
custody root; its scratch then goes to a per-checkout folder under the host
temp root, never into the source tree, where runtime fixtures and source
custody reject it.

This module is path logic only. It reads no environment, so low-level custody
code (the memory guard, temporary-artifact custody) shares it with RunContext
without importing the DX layer.
"""

from __future__ import annotations

import hashlib
import tempfile
from pathlib import Path

from molt.path_custody import host_path_is_within

MAIN_CHECKOUT_DIRNAME = "molt-src"
WORKTREES_DIRNAME = "worktrees"


def custody_root(repo_root: str | Path) -> Path:
    """Return the durable custody root of a checkout or worktree."""

    root = Path(repo_root).expanduser().resolve()
    if root.name == MAIN_CHECKOUT_DIRNAME:
        return root.parent
    if root.parent.name == WORKTREES_DIRNAME:
        return root.parent.parent
    return root


def out_of_tree_scratch_root(source_root: str | Path) -> Path:
    """Return the per-checkout scratch folder under the host temp root."""

    resolved = Path(source_root).expanduser().resolve()
    identity = hashlib.sha256(str(resolved).encode("utf-8")).hexdigest()[:12]
    return Path(tempfile.gettempdir()).resolve() / f"molt-{identity}"


def scratch_root(artifact_root: str | Path, source_root: str | Path) -> Path:
    """Return scratch for an artifact root: `<artifact root>/tmp`, out of tree."""

    scratch = Path(artifact_root) / "tmp"
    if host_path_is_within(scratch, source_root):
        return out_of_tree_scratch_root(source_root)
    return scratch


def unconfigured_state_root(repo_root: str | Path) -> Path:
    """Return where unconfigured state goes: the custody root, never the tree."""

    source = Path(repo_root).expanduser().resolve()
    root = custody_root(source)
    return root if root != source else out_of_tree_scratch_root(source)
