#!/usr/bin/env python3
"""Which Cargo packages a set of changed paths reaches.

The one reverse-dependency reach authority: the changed-Clippy and
changed-crate-test selectors and the landing tool's main-move interaction
check all ask it. It owns path-to-package ownership (`Workspace`) and the
reverse-dependency closure (`reach`, `dependency_reach`); it runs no Cargo
command.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tomllib
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from enum import Enum
from pathlib import Path, PurePosixPath
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.command_execution import CommandExecutor  # noqa: E402
from tools.git_identity import (  # noqa: E402
    branch_changed_paths,
    immutable_git_environment,
    require_git_object_id,
)
from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from molt.cargo_workspace import (  # noqa: E402
    workspace_manifest_facts,
    workspace_member_packages,
)
from molt.cli.cargo_source_closure import cargo_locked_dependency_projection  # noqa: E402

COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")
WORKSPACE_INPUTS = frozenset({"Cargo.toml", "Cargo.lock"})
TOOLCHAIN_INPUTS = frozenset({"rust-toolchain.toml", "clippy.toml"})
TOOLCHAIN_INPUT_DIRS = (".cargo/",)
GIT_TIMEOUT_SECONDS = 60


class Owner(Enum):
    """The kind of Cargo project that owns a path."""

    MEMBER = "member"  # a workspace member; carries its package name
    PATCHED = "patched"  # a crate that replaces a registry crate
    PROJECT = "project"  # a separate (excluded or standalone) Cargo project


@dataclass(frozen=True, slots=True)
class Workspace:
    """Member directories, package names and reverse dependency edges."""

    root: Path
    members: Mapping[str, str]  # root-relative POSIX directory -> package name
    dependents: Mapping[str, frozenset[str]]  # package -> packages depending on it
    patched: frozenset[str]  # root-relative directories that replace a registry crate

    @classmethod
    def load(cls, root: Path) -> "Workspace":
        root = root.resolve()
        packages = workspace_member_packages(root)
        by_manifest = {manifest.resolve(): name for name, manifest in packages.items()}
        members = {
            manifest.parent.relative_to(root).as_posix(): name
            for manifest, name in by_manifest.items()
        }
        dependents: dict[str, set[str]] = {name: set() for name in packages}
        for edge in workspace_manifest_facts(root).dependencies:
            source = by_manifest.get(edge.source_manifest.resolve())
            target = by_manifest.get(edge.dependency_manifest.resolve())
            if source is not None and target is not None:
                dependents[target].add(source)
        with (root / "Cargo.toml").open("rb") as stream:
            patch_tables = tomllib.load(stream).get("patch", {})
        patched = frozenset(
            (root / spec["path"]).resolve().relative_to(root).as_posix()
            for table in patch_tables.values()
            for spec in table.values()
            if isinstance(spec, dict) and isinstance(spec.get("path"), str)
        )
        return cls(
            root=root,
            members=members,
            dependents={name: frozenset(names) for name, names in dependents.items()},
            patched=patched,
        )

    def owner_of(self, path: str) -> tuple[Owner, str] | None:
        """The deepest Cargo project containing `path`, with the member's
        package name or the project's directory; None outside every project."""
        parts = PurePosixPath(path).parts
        for depth in range(len(parts) - 1, 0, -1):
            directory = "/".join(parts[:depth])
            if directory in self.patched:
                return Owner.PATCHED, directory
            if (name := self.members.get(directory)) is not None:
                return Owner.MEMBER, name
            if (self.root / directory / "Cargo.toml").is_file():
                return Owner.PROJECT, directory
        return None

    def reverse_closure(self, names: Iterable[str]) -> frozenset[str]:
        reached = set(names)
        pending = list(reached)
        while pending:
            for dependent in self.dependents[pending.pop()]:
                if dependent not in reached:
                    reached.add(dependent)
                    pending.append(dependent)
        return frozenset(reached)


@dataclass(frozen=True, slots=True)
class Reach:
    """What a set of changed paths reaches."""

    packages: frozenset[str]
    whole_workspace: bool
    toolchain: bool
    projects: frozenset[str]  # separate Cargo projects a path lies in


def reach(workspace: Workspace, paths: Iterable[str]) -> Reach:
    touched: set[str] = set()
    projects: set[str] = set()
    whole_workspace = False
    toolchain = False
    for path in sorted(set(paths)):
        if path in TOOLCHAIN_INPUTS or path.startswith(TOOLCHAIN_INPUT_DIRS):
            toolchain = whole_workspace = True
            continue
        if path in WORKSPACE_INPUTS:
            whole_workspace = True
            continue
        match workspace.owner_of(path):
            case (Owner.MEMBER, name):
                touched.add(name)
            case (Owner.PROJECT, directory):
                projects.add(directory)
            case (Owner.PATCHED, _):
                # A patched crate replaces a registry crate that no local
                # dependency edge names, so its dependents are unknown here.
                whole_workspace = True
            case None if path.endswith(".rs"):
                # Shared build support and other #[path] sources.
                whole_workspace = True
    packages = (
        frozenset(workspace.members.values())
        if whole_workspace
        else workspace.reverse_closure(touched)
    )
    return Reach(packages, whole_workspace, toolchain, frozenset(projects))


def dependency_reach(
    workspace: Workspace, paths: Iterable[str], read_base: Callable[[str], str]
) -> Reach:
    """Refine root/patch changes using both conservative locked package closures.

    Unclassified manifest policy or incomplete identity evidence remains global.
    Command feature/target/profile contracts are never rewritten.
    """
    paths = tuple(paths)
    ordinary = reach(workspace, paths)
    if ordinary.toolchain:
        return ordinary
    if not any(
        path in WORKSPACE_INPUTS
        or any(path.startswith(directory + "/") for directory in workspace.patched)
        or path.endswith("Cargo.toml")
        or (ordinary.whole_workspace and path.endswith(".rs"))
        for path in paths
    ):
        return ordinary

    def current(path: str) -> str:
        return (workspace.root / path).read_text(encoding="utf-8")

    def patches(document: dict[str, Any]) -> dict[tuple[str, str], dict[str, Any]]:
        result: dict[tuple[str, str], dict[str, Any]] = {}
        for source, table in document.get("patch", {}).items():
            for name, spec in table.items():
                if not isinstance(spec, dict):
                    raise ValueError("unclassified Cargo patch specification")
                result[(source, name)] = spec
        return result

    def patch_path(spec: dict[str, Any]) -> str | None:
        value = spec.get("path")
        if value is None:
            return None
        if not isinstance(value, str):
            raise ValueError("invalid patch path")
        path = PurePosixPath(value)
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("patch path outside source custody")
        return path.as_posix()

    try:
        before = tomllib.loads(read_base("Cargo.toml"))
        after = tomllib.loads(current("Cargo.toml"))
        if {k: v for k, v in before.items() if k != "patch"} != {
            k: v for k, v in after.items() if k != "patch"
        }:
            raise ValueError("workspace policy changed")
        old_patches, new_patches = patches(before), patches(after)
        directories: set[str] = set()
        identities: set[tuple[str, str, str]] = set()
        for key in old_patches.keys() | new_patches.keys():
            old, new = old_patches.get(key, {}), new_patches.get(key, {})
            old_path, new_path = patch_path(old), patch_path(new)
            for directory in (old_path, new_path):
                if directory is not None:
                    directories.add(directory)
            changed = old != new or any(
                path.startswith(directory + "/")
                for directory in (old_path, new_path)
                if directory is not None
                for path in paths
            )
            if not changed:
                continue
            for spec, directory, reader in (
                (old, old_path, read_base),
                (new, new_path, current),
            ):
                if not spec:
                    continue
                if directory is None:
                    raise ValueError("changed nonlocal patch")
                package = tomllib.loads(reader(directory + "/Cargo.toml"))["package"]
                name, version = package.get("name"), package.get("version")
                if not isinstance(name, str) or not isinstance(version, str):
                    raise ValueError("patch package identity unavailable")
                identities.add((name, version, ""))
        remaining = [
            path
            for path in paths
            if path not in WORKSPACE_INPUTS
            and not any(path.startswith(directory + "/") for directory in directories)
        ]
        local = reach(workspace, remaining)
        if local.whole_workspace:
            return local
        old_lock = tomllib.loads(read_base("Cargo.lock"))
        new_lock = tomllib.loads(current("Cargo.lock"))
        touched = set(local.packages)
        found: set[tuple[str, str, str]] = set()
        for directory, name in workspace.members.items():
            manifest_path = directory + "/Cargo.toml"
            old_projection = cargo_locked_dependency_projection(
                tomllib.loads(read_base(manifest_path)), old_lock
            )
            new_projection = cargo_locked_dependency_projection(
                tomllib.loads(current(manifest_path)), new_lock
            )
            keys = {
                (record["name"], record["version"], record.get("source", ""))
                for projection in (old_projection, new_projection)
                for record in projection["packages"]
            }
            found.update(keys & identities)
            if old_projection != new_projection or keys & identities:
                touched.add(name)
        if found != identities:
            raise ValueError("changed patch has no proven locked consumer")
        if old_lock != new_lock and not touched:
            # The identity projection deliberately ignores unreachable records.
            # Cargo still owns whole-lock validation, so an unclassified change
            # must not turn into a successful gate without any Cargo command.
            raise ValueError("lockfile changed outside proven consumer closures")
        return Reach(frozenset(touched), False, False, local.projects)
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        AttributeError,
        subprocess.CalledProcessError,
    ) as exc:
        print(f"changed-clippy: conservative workspace reach: {exc}")
        return reach(workspace, (*paths, "Cargo.lock"))


def _git(arguments: list[str]) -> str:
    output = COMMANDS.check_output(
        ["git", *arguments],
        cwd=ROOT,
        text=True,
        encoding="utf-8",
        timeout=GIT_TIMEOUT_SECONDS,
        env=immutable_git_environment(os.environ),
    )
    if not isinstance(output, str):
        raise TypeError("git output must decode as text")
    return output


def changed_paths(base: str) -> tuple[str, ...]:
    """Committed changes since the merge base with `base`, plus the worktree."""
    return tuple(branch_changed_paths(base, run_git=_git))


def base_reader(base: str) -> Callable[[str], str]:
    """Read a file at the merge base of `base` and HEAD; the merge base is
    resolved once, on the first read (dependency_reach reads only for root
    Cargo inputs)."""
    base_commit: str | None = None

    def read_base(path: str) -> str:
        nonlocal base_commit
        if base_commit is None:
            base_commit = require_git_object_id(
                _git(["merge-base", base, "HEAD"]).strip()
            )
        return _git(["show", f"{base_commit}:{path}"])

    return read_base
