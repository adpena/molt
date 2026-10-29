#!/usr/bin/env python3
"""Lint every Cargo package a change reaches with the proof plan's clippy cells.

CI runs Clippy with Cargo-owned warning denial in all integration tiers, but no
landing gate did, so lints reached main unseen (HF-318). This gate closes that
path. The pre-push lint commands in tools/proof_plan.toml and their canonical Cargo
operations stay the authority for features, targets and profiles; the gate runs
each registered command whose packages the change reaches. An equivalent declared host cell supplies its own
identity and execution budget; commands without one remain unchanged:

* a changed path inside a workspace member touches that member, and the change
  reaches every member that depends on it (any dependency kind), since an API
  change can raise a lint in a dependent;
* lock and understood local-patch changes reach consumers in the union of the
  base and current locked graphs; policy changes and unproven identities reach
  the whole workspace, as does shared Rust source outside Cargo projects;
* a path inside a separate Cargo project (excluded or standalone) reaches only
  that project; rust-toolchain.toml, clippy.toml and .cargo/ reach them all;
* a `--workspace` command covers every member, a `-p` command its packages,
  and a command with its own `cwd` the separate project there.

Each selected command runs with its declared toolchains projected exactly as
tools/toolchain_env.py projects them, so the gate and CI lint the same cells.
The declared `--config build.warnings="deny"` rejects emitted lint warnings,
including cached diagnostics, without changing Clippy compiler arguments.
Cargo replays cached results for unchanged crates; a warm target pays only for
the reached crates. A successful gate runs every selected cell; the first
failed cell stops the gate without launching later cells.
"""

from __future__ import annotations

import argparse
import json
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

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from molt.cargo_execution_policy import (  # noqa: E402
    admit_cargo_build,
    load_ci_cargo_policy,
)
from molt.cargo_workspace import (  # noqa: E402
    require_cargo_workspace_root,
    workspace_manifest_facts,
    workspace_member_packages,
)
from tools.command_execution import CommandExecutor  # noqa: E402
from tools.proof_command_argv import _basename  # noqa: E402
from tools.proof_queue_pkg import command_admission  # noqa: E402
from tools.runtime_test_gate_plan import lint_cargo_operation  # noqa: E402
from molt.cli.cargo_source_closure import cargo_locked_dependency_projection  # noqa: E402
from tools.git_identity import (  # noqa: E402
    branch_changed_paths,
    immutable_git_environment,
    require_git_object_id,
)
from tools.proof_plan import (  # noqa: E402
    ProofCommand,
    ProofPlan,
    _normalized_arch,
    _normalized_os,
)
from tools.toolchain_env import (  # noqa: E402
    ToolchainNotProvisioned,
    toolchain_environment,
)

COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")
LOCAL_TIER = "pre-push"
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


def clippy_argv(command: ProofCommand) -> tuple[str, ...] | None:
    """Project lint scope while keeping the registered execution command intact."""
    argv = command.argv
    if _basename(argv[0]) in {"cargo", "cargo.exe"}:
        invocation = command_admission.parse_cargo_invocation(argv)
        return argv if invocation.subcommand == "clippy" else None
    if "cargo" not in command.toolchains:
        return None
    envelope = command_admission.envelope_for_command(argv)
    arguments = command_admission.runtime_gate_arguments(envelope)
    if arguments is None:
        return None
    operation = lint_cargo_operation(arguments.policy, arguments.profile, cargo="cargo")
    if operation is None:
        return None
    invocation = command_admission.parse_cargo_invocation(operation.command)
    if invocation.subcommand != "clippy":
        raise ValueError(f"{command.id}: lint operation does not invoke Cargo clippy")
    return operation.command


def clippy_packages(command: ProofCommand) -> frozenset[str] | None:
    """Packages a workspace clippy operation lints; None means every member."""
    argv = clippy_argv(command)
    if argv is None:
        raise ValueError(f"{command.id}: command has no Cargo clippy operation")
    invocation = command_admission.parse_cargo_invocation(argv)
    if "--workspace" in invocation.flags:
        return None
    packages = {
        value for name, value in invocation.option_values if name == "--package"
    }
    if not packages:
        raise ValueError(
            f"{command.id}: a clippy command must name --workspace, -p or a cwd"
        )
    return frozenset(packages)


def _execution_contract(command: ProofCommand) -> str:
    """Ignore only scheduling coordinates; new execution fields must match too."""
    return json.dumps(
        {
            key: value
            for key, value in command.data.items()
            if key
            not in {
                "id",
                "family",
                "cell",
                "tiers",
                "timeout_budget",
                "timeout_seconds",
            }
        },
        sort_keys=True,
    )


def local_clippy_commands(plan: ProofPlan) -> tuple[ProofCommand, ...]:
    host = (_normalized_os(), _normalized_arch())
    host_cells = {
        cell.id
        for cell in plan.matrix_cells
        if (cell.data["os"], cell.data["arch"]) == host
    }
    clippy = tuple(
        command for command in plan.commands if clippy_argv(command) is not None
    )
    host_commands: dict[str, list[ProofCommand]] = {}
    for command in clippy:
        if command.data["cell"] in host_cells:
            host_commands.setdefault(_execution_contract(command), []).append(command)
    selected: dict[str, ProofCommand] = {}
    for command in clippy:
        if LOCAL_TIER not in command.data.get("tiers", ()):
            continue
        if command.data["cell"] not in host_cells:
            equivalent = host_commands.get(_execution_contract(command), ())
            if len(equivalent) > 1:
                raise ValueError(
                    f"{command.id}: multiple equivalent clippy cells for "
                    f"{host[0]}/{host[1]}: "
                    + ", ".join(candidate.id for candidate in equivalent)
                )
            if equivalent:
                command = equivalent[0]
        selected[command.id] = command
    return tuple(selected.values())


def select(
    commands: Iterable[ProofCommand], change: Reach
) -> tuple[tuple[ProofCommand, str], ...]:
    """Commands the change reaches, each with the reason it was selected."""
    selected: list[tuple[ProofCommand, str]] = []
    for command in commands:
        cwd = command.data.get("cwd")
        if cwd is not None:
            if change.toolchain:
                selected.append((command, "Rust toolchain or clippy configuration"))
            elif PurePosixPath(cwd).as_posix() in change.projects:
                selected.append((command, f"standalone project {cwd}"))
            continue
        covered = clippy_packages(command)
        if not change.packages:
            continue
        if change.whole_workspace:
            selected.append((command, "whole workspace"))
        elif covered is None:
            selected.append((command, ", ".join(sorted(change.packages))))
        elif reached := sorted(covered & change.packages):
            selected.append((command, ", ".join(reached)))
    return tuple(selected)


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


def run(command: ProofCommand, plan: ProofPlan) -> int:
    env = dict(os.environ)
    env.update(
        toolchain_environment(plan, plan.required_toolchains(command), environ=env)
    )
    cwd = ROOT / str(command.data.get("cwd", "."))
    operation = clippy_argv(command)
    if operation is None:
        raise ValueError(f"{command.id}: command has no Cargo clippy operation")
    if operation == command.argv:
        admit_cargo_build(operation, cwd=cwd, env=env)
    # A registered driver admits its actual Cargo operation after resolving
    # its managed C ABI; do not feed Python argv to Cargo admission here.
    # The plan's timeout is CI's, which starts from a restored warm cache. A
    # landing's first clippy in a checkout target is cold, so the gate takes
    # the Cargo execution policy's cold budget when it is the larger one.
    cold = load_ci_cargo_policy().execution_budgets.timeout_seconds("cold")
    result = COMMANDS.run(
        command.argv,
        cwd=cwd,
        env=env,
        timeout=float(max(int(command.data["timeout_seconds"]), cold)),
    )
    return result.returncode


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument(
        "--base",
        default="origin/main",
        help="compare against the merge base with this ref (default: origin/main)",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="print the selection; run nothing"
    )
    args = parser.parse_args(argv)
    plan = ProofPlan.load()
    paths = changed_paths(args.base)
    workspace = Workspace.load(ROOT)

    def read_base(path: str) -> str:
        nonlocal base_commit
        if base_commit is None:
            base_commit = require_git_object_id(
                _git(["merge-base", args.base, "HEAD"]).strip()
            )
        return _git(["show", f"{base_commit}:{path}"])

    base_commit: str | None = None
    change = dependency_reach(workspace, paths, read_base)
    selected = select(local_clippy_commands(plan), change)
    if not selected:
        print("changed-clippy: no Cargo package reached; nothing to lint")
        return 0
    try:
        # Admit every selected project before compiling any of them. A later
        # standalone failure must not follow a full workspace clippy build.
        for command, _reason in selected:
            require_cargo_workspace_root(
                ROOT / str(command.data.get("cwd", ".")) / "Cargo.toml"
            )
    except ValueError as exc:
        print(f"changed-clippy: {exc}", file=sys.stderr)
        return 1
    for command, reason in selected:
        print(f"changed-clippy: {command.id} ({reason})")
    if args.dry_run:
        return 0
    for command, _reason in selected:
        print(f"changed-clippy: running {' '.join(command.argv)}", flush=True)
        try:
            returncode = run(command, plan)
        except ToolchainNotProvisioned as exc:
            print(f"changed-clippy: {command.id}: {exc}", file=sys.stderr)
            return 2
        if returncode != 0:
            print(f"changed-clippy: FAILED {command.id}", file=sys.stderr)
            return 1
    print(f"changed-clippy: OK ({len(selected)} command(s))")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
