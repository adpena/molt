#!/usr/bin/env python3
"""Lint every Cargo package a change reaches with the proof plan's clippy cells.

CI runs `cargo clippy -D warnings` in the pre-push, pr and main tiers, but no
landing gate did, so lints reached main unseen (HF-318). This gate closes that
path. The pre-push `cargo clippy` commands in tools/proof_plan.toml stay the one
authority for features, targets and profiles; the gate runs each command whose
packages the change reaches. An equivalent declared host cell supplies its own
identity and execution budget; commands without one remain unchanged:

* a changed path inside a workspace member touches that member, and the change
  reaches every member that depends on it (any dependency kind), since an API
  change can raise a lint in a dependent;
* a root Cargo input, a patched crate, or Rust source outside every Cargo
  project (shared `#[path]` build support) reaches the whole workspace;
* a path inside a separate Cargo project (excluded or standalone) reaches only
  that project; rust-toolchain.toml, clippy.toml and .cargo/ reach them all;
* a `--workspace` command covers every member, a `-p` command its packages,
  and a command with its own `cwd` the separate project there.

Each selected command runs with its declared toolchains projected exactly as
tools/toolchain_env.py projects them, so the gate and CI lint the same cells.
Cargo replays cached results for unchanged crates; a warm target pays only for
the reached crates.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tomllib
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from enum import Enum
from pathlib import Path, PurePosixPath

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
from tools.git_identity import branch_changed_paths  # noqa: E402
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

COMMANDS = CommandExecutor.for_file(__file__)
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


def clippy_packages(command: ProofCommand) -> frozenset[str] | None:
    """Packages a workspace clippy command lints; None means every member."""
    argv = command.argv
    flags = argv[2 : argv.index("--")] if "--" in argv else argv[2:]
    if "--workspace" in flags:
        return None
    packages: set[str] = set()
    for index, flag in enumerate(flags):
        if flag in {"-p", "--package"}:
            packages.add(flags[index + 1])
        elif flag.startswith("--package="):
            packages.add(flag.removeprefix("--package="))
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
        command for command in plan.commands if command.argv[:2] == ("cargo", "clippy")
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
    admit_cargo_build(command.argv, cwd=cwd, env=env)
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
    change = reach(Workspace.load(ROOT), changed_paths(args.base))
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
    failed: list[str] = []
    for command, _reason in selected:
        print(f"changed-clippy: running {' '.join(command.argv)}", flush=True)
        try:
            returncode = run(command, plan)
        except ToolchainNotProvisioned as exc:
            print(f"changed-clippy: {command.id}: {exc}", file=sys.stderr)
            return 2
        if returncode != 0:
            failed.append(command.id)
    if failed:
        print(f"changed-clippy: FAILED {', '.join(failed)}", file=sys.stderr)
        return 1
    print(f"changed-clippy: OK ({len(selected)} command(s))")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
