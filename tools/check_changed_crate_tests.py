#!/usr/bin/env python3
"""Run the whole lib tests of every Cargo package a change reaches, in each
cell the proof plan declares for it.

HF-205 reached main because the landing tested `molt-backend-native` only in
the `release-fast` cell of its batch rule, while CI's compiler-authorities
command tests it in `dev-fast`, where debug assertions select another
refcount storage. This gate derives every lib-test cell from the plan, as
tools/check_changed_clippy.py derives lint cells (docs/design/landing_queue.md,
section 9.4.2):

* reach: the members a change touches and their reverse dependencies, through
  check_changed_clippy's `dependency_reach` (root Cargo inputs and toolchain
  files reach every member);
* sources: every `cargo test` gate of a `[[rule]]` and every `cargo test`
  `[[command]]` of the `pre-push` tier; a source names packages with `-p`;
* cells: per source naming a reached crate, the source's build flags
  (profile, `--config`, target, features) with `--lib` and without its name
  filters, so the cell runs the whole lib; a qualified feature of a package
  the cell does not select is dropped (Cargo refuses it) and the cell is
  reported as narrowed; `molt-runtime` is tested by its declared runtime gate
  (tools/run_runtime_test_gate.py), which owns its fresh-process witnesses;
  a crate no source names runs its default-truth unit, `cargo test -p C --lib`;
* accounting: each `Running unittests` binary is bound to its package through
  Cargo's compiler-artifact records, and its `test result:` line to that
  package. A cell that exits non-zero, a selected crate without a summary,
  or a declared crate that runs 0 tests fails the gate.

The last line is one JSON object (`molt.changed-crate-tests.v1`).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import tomllib
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from molt.cargo_execution_policy import admit_cargo_build  # noqa: E402
from molt.cargo_workspace import workspace_member_packages  # noqa: E402
from tools.check_changed_clippy import (  # noqa: E402
    Reach,
    Workspace,
    base_reader,
    changed_paths,
    dependency_reach,
)
from tools.command_execution import CommandExecutor  # noqa: E402
from tools.proof_queue_pkg.command_admission import parse_cargo_invocation  # noqa: E402
from tools.run_cargo_test_truth import compiler_artifact_record  # noqa: E402
from tools.runtime_test_gate_plan import SEMANTIC_IMAGES  # noqa: E402
from tools.proof_plan import ProofPlan  # noqa: E402
from tools.toolchain_env import ToolchainNotProvisioned, toolchain_environment  # noqa: E402
from tools.runtime_test_gate_plan import parse_arguments as runtime_gate_arguments  # noqa: E402

COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")
SCHEMA = "molt.changed-crate-tests.v1"
PLAN_PATH = Path("tools") / "proof_plan.toml"
LOCAL_TIER = "pre-push"
RUNTIME_GATE = "tools/run_runtime_test_gate.py"
# A cold dev-fast or release-fast build of the backend family plus its lib
# tests fits; the landing's own ceiling bounds the whole gate.
CELL_TIMEOUT_SECONDS = 3600.0
RUNNER_TIMEOUT_SECONDS = 7200.0
# Cargo selectors the cell replaces with `--lib`, and flags it owns itself.
_TARGET_FLAGS = frozenset(
    {"--lib", "--bins", "--tests", "--examples", "--benches", "--all-targets", "--doc"}
)
_TARGET_OPTIONS = frozenset({"--bin", "--test", "--example", "--bench"})
_DROPPED_FLAGS = frozenset({"--timings", "--no-fail-fast", "--workspace", "--all"})
_DROPPED_OPTIONS = _TARGET_OPTIONS | {
    "--package",
    "--exclude",
    "--features",
    "--message-format",
}
# Libtest options a cell keeps; positional name filters and the rest drop.
_LIBTEST_FLAGS = frozenset({"--include-ignored", "--ignored", "--nocapture"})
_LIBTEST_VALUES = frozenset({"--skip", "--test-threads"})
_RUNNING = re.compile(r"^\s*Running unittests \S+ \((?P<path>.+)\)\s*$")
# Members whose lib has no unit tests in any declared cell (measured). A
# 0-test total fails every other declared member: it means the cell compiled
# the tests out (molt-backend-wasm's tests need test-util). An entry that runs
# tests fails too, so the list only shrinks to the truth.
LIBS_WITHOUT_TESTS: frozenset[str] = frozenset({"molt-backend"})
_SUMMARY = re.compile(
    r"test result: (?P<status>ok|FAILED)\. (?P<passed>\d+) passed; "
    r"(?P<failed>\d+) failed; (?P<ignored>\d+) ignored; "
)


@dataclass(frozen=True, slots=True)
class Library:
    """A member's lib target as its manifest declares it."""

    present: bool
    # `[lib] test = false` or `harness = false`: no libtest summary exists.
    harnessed: bool


@dataclass(frozen=True, slots=True)
class Source:
    """One declared `cargo test` invocation of the plan."""

    id: str
    packages: tuple[str, ...]  # -p specs as written (name or name@version)
    build: tuple[str, ...]  # Cargo tokens the cell keeps, features excluded
    features: tuple[str, ...]
    libtest: tuple[str, ...]  # kept libtest flags and --test-threads
    skips: tuple[str, ...]
    unrunnable: str | None = None  # why this host cannot run it
    toolchains: tuple[str, ...] = ()  # the source's declared toolchains

    @property
    def names(self) -> dict[str, str]:
        """Package name -> spec as written."""
        return {spec.partition("@")[0]: spec for spec in self.packages}


@dataclass(frozen=True, slots=True)
class CellKey:
    build: tuple[str, ...]
    features: tuple[str, ...]
    libtest: tuple[str, ...]


@dataclass(slots=True)
class Cell:
    key: CellKey
    packages: dict[str, str] = field(default_factory=dict)  # name -> spec
    skips: set[str] = field(default_factory=set)
    sources: list[str] = field(default_factory=list)
    dropped: set[str] = field(default_factory=set)
    # Packages a declared source names; a 0-test total fails only for them.
    declared: set[str] = field(default_factory=set)
    # The union of its sources' declared toolchains, projected when it runs.
    toolchains: set[str] = field(default_factory=set)

    @property
    def label(self) -> str:
        return "+".join(self.sources)

    def argv(self) -> tuple[str, ...]:
        argv = ["cargo", "test", *self.key.build]
        if self.key.features:
            argv += ["--features", ",".join(self.key.features)]
        for name in sorted(self.packages):
            argv += ["-p", self.packages[name]]
        argv += ["--lib", "--no-fail-fast", "--message-format=json-render-diagnostics"]
        libtest = [*self.key.libtest]
        for name in sorted(self.skips):
            libtest += ["--skip", name]
        if libtest:
            argv += ["--", *libtest]
        return tuple(argv)


@dataclass(frozen=True, slots=True)
class Selection:
    reach: Reach
    cells: tuple[Cell, ...]
    runner: tuple[str, ...] | None  # the runtime gate argv, when reached
    runner_packages: frozenset[str]
    unverified: tuple[str, ...]


def _libtest_options(
    forwarded: Sequence[str],
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """(kept libtest flags and --test-threads, --skip names); filters drop."""
    kept: list[str] = []
    skips: list[str] = []
    index = 0
    while index < len(forwarded):
        token = forwarded[index]
        name, equal, value = token.partition("=")
        if name in _LIBTEST_VALUES:
            if not equal:
                index += 1
                value = forwarded[index] if index < len(forwarded) else ""
            if name == "--skip":
                skips.append(value)
            else:
                kept.append(f"{name}={value}")
        elif token in _LIBTEST_FLAGS:
            kept.append(token)
        index += 1
    return tuple(sorted(set(kept))), tuple(skips)


def parse_source(
    source_id: str,
    argv: Sequence[str],
    *,
    host: str,
    toolchains: Sequence[str] = (),
) -> Source | None:
    """A `cargo test` execution as a cell source; None for anything else."""
    if not argv or Path(argv[0]).name not in {"cargo", "cargo.exe"}:
        return None
    invocation = parse_cargo_invocation(argv)
    if invocation.subcommand != "test" or invocation.proof_kind != "test-execution":
        return None
    packages = tuple(
        value for name, value in invocation.option_values if name == "--package"
    )
    features = tuple(
        token
        for name, value in invocation.option_values
        if name == "--features"
        for token in re.split(r"[ ,]+", value)
        if token
    )
    build = [
        *sorted(invocation.flags - _TARGET_FLAGS - _DROPPED_FLAGS),
        *(
            token
            for name, value in invocation.option_values
            if name not in _DROPPED_OPTIONS
            for token in (name, value)
        ),
    ]
    targets = {value for name, value in invocation.option_values if name == "--target"}
    unrunnable = (
        f"--target {', '.join(sorted(targets))} is not this host ({host})"
        if targets - {host}
        else None
    )
    libtest, skips = _libtest_options(invocation.forwarded)
    return Source(
        source_id,
        packages,
        tuple(build),
        features,
        libtest,
        skips,
        unrunnable,
        tuple(toolchains),
    )


def plan_sources(plan: Mapping, *, host: str) -> tuple[Source, ...]:
    """Every runnable-or-not `cargo test` execution the plan declares locally."""
    sources: list[Source] = []
    for rule in plan.get("rule", ()):
        for gate in rule.get("gates", ()):
            source = parse_source(
                f"rule:{rule['name']}",
                shlex.split(gate),
                host=host,
                toolchains=rule.get("toolchains", ()),
            )
            if source is not None:
                sources.append(source)
    for command in plan.get("command", ()):
        if LOCAL_TIER not in command.get("tiers", ()):
            continue
        source = parse_source(
            f"command:{command['id']}",
            command["argv"],
            host=host,
            toolchains=command.get("toolchains", ()),
        )
        if source is not None:
            sources.append(source)
    return tuple(sources)


def runtime_gate(plan: Mapping) -> tuple[tuple[str, ...], frozenset[str]] | None:
    """The rule gate that runs the semantic runtime gate, and the packages whose
    whole libtest it runs (an image without a case list)."""
    owned = frozenset(image.package for image in SEMANTIC_IMAGES if not image.cases)
    for rule in plan.get("rule", ()):
        for gate in rule.get("gates", ()):
            argv = tuple(shlex.split(gate))
            if RUNTIME_GATE not in argv:
                continue
            arguments = runtime_gate_arguments(
                list(argv[argv.index(RUNTIME_GATE) + 1 :])
            )
            if arguments.policy == "semantic":
                return argv, owned
    return None


def libraries(root: Path) -> dict[str, Library]:
    """Each workspace member's lib target, from its manifest."""
    result: dict[str, Library] = {}
    for name, manifest in workspace_member_packages(root).items():
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        lib = data.get("lib")
        present = (
            isinstance(lib, dict) or (manifest.parent / "src" / "lib.rs").is_file()
        )
        lib = lib if isinstance(lib, dict) else {}
        result[name] = Library(
            present=present,
            harnessed=lib.get("test", True) is not False
            and lib.get("harness", True) is not False,
        )
    return result


def select(
    sources: Iterable[Source],
    reach: Reach,
    libs: Mapping[str, Library],
    runner: tuple[tuple[str, ...], frozenset[str]] | None,
) -> Selection:
    """The cells that run every reached crate's whole lib in its declared cells."""
    runner_argv, runner_owned = runner if runner is not None else (None, frozenset())
    reached = frozenset(reach.packages)
    unverified = [
        f"{project}: separate Cargo project; no declared lib-test cell"
        for project in sorted(reach.projects)
    ]
    testable: set[str] = set()
    for name in sorted(reached):
        lib = libs.get(name)
        if lib is None or not lib.present:
            unverified.append(
                f"{name}: no lib target; its bins and tests stay with the batch"
            )
            continue
        if not lib.harnessed:
            unverified.append(f"{name}: lib has test = false or harness = false")
            continue
        testable.add(name)
    cells: dict[CellKey, Cell] = {}
    named: set[str] = set()
    for source in sources:
        names = source.names
        selected = sorted((testable & names.keys()) - runner_owned)
        if not selected:
            continue
        # A crate a source names is declared even where that source cannot
        # run here: it never falls back to the default-truth unit.
        named.update(selected)
        if source.unrunnable is not None:
            unverified.extend(
                f"{name} [{source.id}]: {source.unrunnable}" for name in selected
            )
            continue
        features: list[str] = []
        dropped: list[str] = []
        for token in source.features:
            owner, slash, _feature = token.partition("/")
            if not slash and len(names) == 1:
                # One package: Cargo applies a bare feature to it; spelling it
                # qualified lets equal builds from different sources merge.
                (owner,) = names
                token = f"{owner}/{token}"
            elif not slash:
                dropped.append(token)
                continue
            (features if owner in selected else dropped).append(token)
        key = CellKey(source.build, tuple(sorted(set(features))), source.libtest)
        cell = cells.setdefault(key, Cell(key))
        for name in selected:
            cell.packages[name] = names[name]
            cell.declared.add(name)
        cell.skips.update(source.skips)
        cell.toolchains.update(source.toolchains)
        if source.id not in cell.sources:
            cell.sources.append(source.id)
        cell.dropped.update(dropped)
    for name in sorted(testable - named - runner_owned):
        key = CellKey(("--locked",), (), ())
        cell = cells.setdefault(key, Cell(key))
        cell.packages[name] = name
        cell.toolchains.update(("rustc", "cargo"))
        if "default-truth" not in cell.sources:
            cell.sources.append("default-truth")
    return Selection(
        reach=reach,
        cells=tuple(cells.values()),
        runner=runner_argv if testable & runner_owned else None,
        runner_packages=frozenset(testable & runner_owned),
        unverified=tuple(unverified),
    )


@dataclass(slots=True)
class CellResult:
    argv: tuple[str, ...]
    returncode: int
    counts: dict[str, dict[str, int]]  # package -> passed/failed/ignored
    problems: list[str]


def account(
    lines: Iterable[str], packages: Mapping[str, str], root: Path
) -> CellResult:
    """Bind each lib test binary's summary to its package (argv/returncode
    are filled by the caller)."""
    members = {
        manifest.resolve(): name
        for name, manifest in workspace_member_packages(root).items()
    }
    binaries: dict[str, str] = {}
    counts: dict[str, dict[str, int]] = {}
    problems: list[str] = []
    current: str | None = None
    for line in lines:
        record = compiler_artifact_record(line, source_context="mixed-test-output")
        if record is not None:
            profile = record.get("profile")
            executable = record.get("executable")
            manifest = record.get("manifest_path")
            if (
                isinstance(profile, dict)
                and profile.get("test") is True
                and isinstance(executable, str)
                and isinstance(manifest, str)
            ):
                name = members.get(Path(manifest).resolve())
                if name is not None:
                    binaries[Path(executable).name] = name
            continue
        if running := _RUNNING.match(line):
            binary = Path(running["path"].strip()).name
            current = binaries.get(binary)
            if current is None or current not in packages:
                problems.append(f"test binary {binary} maps to no selected package")
                current = None
            continue
        if (summary := _SUMMARY.search(line)) and current is not None:
            if current in counts:
                problems.append(f"{current}: more than one lib test summary")
            counts[current] = {
                key: int(summary[key]) for key in ("passed", "failed", "ignored")
            }
            current = None
    for name in sorted(packages):
        if name not in counts:
            problems.append(f"{name}: no lib test summary (build or run failed)")
    return CellResult((), 0, counts, problems)


def run_cell(cell: Cell, *, root: Path, plan: ProofPlan) -> CellResult:
    argv = cell.argv()
    env = dict(os.environ)
    # The sources' own toolchains (LLVM SDK, WASI SDK), projected exactly as
    # tools/toolchain_env.py projects them for CI and changed-crate Clippy.
    env.update(
        toolchain_environment(
            plan, plan.toolchain_closure(sorted(cell.toolchains)), environ=env
        )
    )
    admit_cargo_build(argv, cwd=root, env=env)
    process = COMMANDS.start_guarded(
        argv,
        cwd=root,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        timeout=CELL_TIMEOUT_SECONDS,
    )
    lines: list[str] = []
    assert process.stdout is not None
    try:
        for line in process.stdout:
            if not line.startswith('{"reason":'):  # Cargo's JSON records
                sys.stdout.write(line)
                sys.stdout.flush()
            lines.append(line)
    finally:
        returncode = COMMANDS.wait_owned(process, timeout=30.0)
    result = account(lines, cell.packages, root)
    result.argv = argv
    result.returncode = returncode
    if returncode != 0:
        result.problems.insert(0, f"cell exited {returncode}")
    return result


def run_runner(argv: Sequence[str], *, root: Path) -> int:
    print(f"changed-crate-tests: running {shlex.join(argv)}", flush=True)
    return COMMANDS.run(
        list(argv), cwd=root, env=dict(os.environ), timeout=RUNNER_TIMEOUT_SECONDS
    ).returncode


def host_triple() -> str:
    output = COMMANDS.check_output(
        ["rustc", "-vV"], cwd=ROOT, text=True, encoding="utf-8", timeout=60
    )
    for line in str(output).splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ").strip()
    raise ValueError("rustc -vV reported no host triple")


def selection_for(base: str, *, root: Path = ROOT) -> Selection:
    plan = tomllib.loads((root / PLAN_PATH).read_text(encoding="utf-8"))
    reach = dependency_reach(
        Workspace.load(root), changed_paths(base), base_reader(base)
    )
    return select(
        plan_sources(plan, host=host_triple()),
        reach,
        libraries(root),
        runtime_gate(plan),
    )


def describe(selection: Selection) -> list[str]:
    reach = selection.reach
    lines = [
        "changed-crate-tests: reached "
        + (
            f"the whole workspace ({len(reach.packages)} members)"
            if reach.whole_workspace
            else ", ".join(sorted(reach.packages)) or "no Cargo package"
        )
    ]
    for cell in selection.cells:
        lines.append(
            f"changed-crate-tests: cell [{cell.label}]: {shlex.join(cell.argv())}"
        )
        if cell.dropped:
            lines.append(
                "changed-crate-tests:   narrowed: features of unselected packages "
                "not applied: " + ", ".join(sorted(cell.dropped))
            )
    if selection.runner is not None:
        lines.append(
            "changed-crate-tests: runtime gate for "
            + ", ".join(sorted(selection.runner_packages))
            + ": "
            + shlex.join(selection.runner)
        )
    lines.extend(
        f"changed-crate-tests: unverified: {item}" for item in selection.unverified
    )
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument(
        "--base",
        default="origin/main",
        help="compare against the merge base with this ref (default: origin/main)",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="print the cells; run nothing"
    )
    args = parser.parse_args(argv)
    selection = selection_for(args.base)
    for line in describe(selection):
        print(line)
    if not selection.cells and selection.runner is None:
        print("changed-crate-tests: no lib test cell reached; nothing to run")
        print(json.dumps({"schema": SCHEMA, "cells": [], "status": "passed"}))
        return 0
    if args.dry_run:
        return 0
    proof_plan = ProofPlan.load(ROOT / PLAN_PATH)
    failures: list[str] = []
    records: list[dict] = []
    totals: dict[str, int] = {}
    declared: set[str] = set()
    for cell in selection.cells:
        print(f"changed-crate-tests: running [{cell.label}]", flush=True)
        try:
            result = run_cell(cell, root=ROOT, plan=proof_plan)
        except ToolchainNotProvisioned as exc:
            print(f"changed-crate-tests: [{cell.label}]: {exc}", file=sys.stderr)
            return 2
        declared.update(cell.declared)
        for name, counts in sorted(result.counts.items()):
            totals[name] = totals.get(name, 0) + counts["passed"] + counts["failed"]
            print(
                f"changed-crate-tests: {name} [{cell.label}]: {counts['passed']} passed; "
                f"{counts['failed']} failed; {counts['ignored']} ignored"
            )
        failures.extend(f"[{cell.label}] {problem}" for problem in result.problems)
        records.append(
            {
                "sources": cell.sources,
                "argv": list(result.argv),
                "returncode": result.returncode,
                "counts": result.counts,
                "narrowed": sorted(cell.dropped),
            }
        )
    for name in sorted(declared):
        ran = totals.get(name, 0)
        if ran == 0 and name not in LIBS_WITHOUT_TESTS:
            failures.append(
                f"{name}: a declared crate ran 0 lib tests in all its cells"
            )
        elif ran and name in LIBS_WITHOUT_TESTS:
            failures.append(
                f"{name}: ran {ran} lib tests; remove it from LIBS_WITHOUT_TESTS"
            )
    for name in sorted(set(totals) - declared):
        if totals[name] == 0:
            print(
                f"changed-crate-tests: {name}: 0 lib tests (no plan command names it)"
            )
    if selection.runner is not None:
        returncode = run_runner(selection.runner, root=ROOT)
        records.append(
            {
                "sources": ["runtime-gate"],
                "argv": list(selection.runner),
                "returncode": returncode,
                "packages": sorted(selection.runner_packages),
            }
        )
        if returncode != 0:
            failures.append(f"runtime gate exited {returncode}")
    for failure in failures:
        print(f"changed-crate-tests: FAILED {failure}", file=sys.stderr)
    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "status": "failed" if failures else "passed",
                "cells": records,
                "unverified": list(selection.unverified),
                "failures": failures,
            },
            sort_keys=True,
        )
    )
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
