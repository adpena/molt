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
import sys
from collections.abc import Iterable
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
)
from tools.command_execution import CommandExecutor  # noqa: E402
from tools.proof_command_argv import _basename  # noqa: E402
from tools.proof_queue_pkg import command_admission  # noqa: E402
from tools.runtime_test_gate_plan import lint_cargo_operation  # noqa: E402
from tools.proof_plan import (  # noqa: E402
    ProofCommand,
    ProofPlan,
    _normalized_arch,
    _normalized_os,
)
from tools.cargo_reach import (  # noqa: E402
    base_reader,
    changed_paths,
    Reach,
    Workspace,
    dependency_reach,
)
from tools.toolchain_env import (  # noqa: E402
    ToolchainNotProvisioned,
    toolchain_environment,
)

COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")
LOCAL_TIER = "pre-push"


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
    change = dependency_reach(workspace, paths, base_reader(args.base))
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
