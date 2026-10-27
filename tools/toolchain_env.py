#!/usr/bin/env python3
"""Project the proof plan's pinned toolchains into a shell (HF-231).

CI puts the managed LLVM SDK, the wasi-sdk and the pinned tool releases on
PATH in its setup actions. This command composes the same providers for a
local shell, scoped to what a selection declares, so a developer shell and a
hosted job see the same toolchain:

    eval "$(python3 tools/toolchain_env.py --command wasm.build.backend)"
    eval "$(python3 tools/toolchain_env.py --family wasm)"
    eval "$(python3 tools/toolchain_env.py --toolchain ld.lld --toolchain wasm-tools)"

Each ``[[toolchain_policy]]`` in ``tools/proof_plan.toml`` names its provider
in ``projection``: ``llvm-sdk`` (the verified LLVM SDK, as ``molt.llvm_toolchain
--verify --github-env`` exports it), ``wasi-sdk`` (the provisioned wasi-sdk, as
``--verify-wasm --github-env`` exports it), ``tool-release`` (a pinned
release from ``config/tool_releases.toml``) or ``binaryen`` (the pinned ``wasm-opt``).
Policies without a projection are host tools or manage themselves (rustup,
elan, uv). The WASI variables appear only when the selection needs the
wasi-sdk: injected into an unrelated run, they break C extension builds.

The command never downloads. A provider that is not installed is reported with
its install command, and the command exits 2 without printing an environment.

Executors need no shell projection: the proof executor, ``molt_dev.py
integrate`` and the generators check admit their toolchains through
``proof_plan.admit_toolchains``. Each command receives its own declared tools
from the pre-admission environment; generic SDK setup publishes availability
without selecting a Rust target's libc/CRT. The ``eval`` lines above explicitly
select an interactive environment for the declared command or tool set.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import sys
from typing import get_args

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from molt.dx import EnvRenderFormat, render_env  # noqa: E402
from tools.proof_plan import (  # noqa: E402
    ProofPlan,
    _not_provisioned,
    _topological_commands,
    project_toolchains,
)


class ToolchainNotProvisioned(RuntimeError):
    """One or more selected providers are not installed; carries install commands."""


def selected_toolchains(
    plan: ProofPlan,
    *,
    commands: list[str],
    family: str | None,
    matrix_cell: str | None,
    toolchains: list[str],
) -> tuple[str, ...]:
    """The toolchain closure of one selection, dependencies included."""

    names: list[str] = []
    if commands:
        for command_id in commands:
            for command in _topological_commands(plan, command_id=command_id):
                names.extend(plan.required_toolchains(command))
    elif family is not None:
        for command in _topological_commands(
            plan, family=family, matrix_cell=matrix_cell
        ):
            names.extend(plan.required_toolchains(command))
    else:
        names.extend(plan.toolchain_closure(toolchains))
    return tuple(dict.fromkeys(names))


def toolchain_environment(
    plan: ProofPlan,
    names: tuple[str, ...],
    *,
    environ: dict[str, str],
    root: Path = ROOT,
) -> dict[str, str]:
    """The variables that put the selected providers first; all must be installed."""
    projection = project_toolchains(
        plan.toolchain_policies, names, environ=environ, root=root
    )
    if projection.missing:
        raise ToolchainNotProvisioned(_not_provisioned(projection.missing))
    return projection.delta


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument(
        "--command", action="append", default=[], help="proof-plan command id"
    )
    selection.add_argument("--family", help="proof-plan family")
    selection.add_argument(
        "--toolchain", action="append", default=[], help="toolchain policy name"
    )
    parser.add_argument("--matrix-cell", help="with --family: one matrix cell")
    parser.add_argument(
        "--format",
        choices=get_args(EnvRenderFormat),
        default="posix",
        help="shell syntax of the output (default: posix)",
    )
    args = parser.parse_args(argv)
    if args.matrix_cell is not None and args.family is None:
        parser.error("--matrix-cell requires --family")
    plan = ProofPlan.load()
    try:
        names = selected_toolchains(
            plan,
            commands=args.command,
            family=args.family,
            matrix_cell=args.matrix_cell,
            toolchains=args.toolchain,
        )
        delta = toolchain_environment(plan, names, environ=dict(os.environ))
    except ToolchainNotProvisioned as exc:
        print(f"toolchain_env: {exc}", file=sys.stderr)
        return 2
    except ValueError as exc:
        print(f"toolchain_env: {exc}", file=sys.stderr)
        return 2
    print(render_env(delta, sorted(delta), args.format))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
