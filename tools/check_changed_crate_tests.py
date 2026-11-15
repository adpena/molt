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
  cargo_reach's `dependency_reach` (root Cargo inputs and toolchain
  files reach every member);
* sources: every `cargo test` gate of a `[[rule]]` and every `cargo test`
  `[[command]]` of the `pre-push` tier; a source names packages with `-p`;
* cells: per source naming a reached crate, the source's build flags
  (profile, `--config`, target, features) with `--lib` and without its name
  filters, so the cell runs the whole lib, plus every integration test
  target (`--test NAME`) the source declares and a selected crate owns; a qualified feature of a package
  the cell does not select is dropped (Cargo refuses it) and the cell is
  reported as narrowed; `molt-runtime` is tested by its declared runtime gate
  (tools/run_runtime_test_gate.py), which owns its fresh-process witnesses;
  when a changed path selects that gate's own rule, the landing runs it as a
  gate and this gate does not run it again;
  a crate no source names runs its default-truth unit,
  `cargo test --profile dev-fast -p C --lib` (`DEFAULT_TRUTH_PROFILE`);
* exclusions: an experimental backend's crate (`excluded_backend` in
  config/release_acceptance_matrix.toml) runs in no presubmit cell, as proof-plan
  validation requires of every static command; it is reported unverified and
  the schedule runs it;
* accounting: each `Running unittests` binary is bound to its package through
  Cargo's compiler-artifact records, and its `test result:` line to that
  package; each declared integration binary must report a summary too. A cell that exits non-zero, a selected crate without a summary,
  or a declared crate that runs 0 tests fails the gate;
* schedule: cells run beside each other (`cell_workers`). Cargo's build lock
  serializes their builds on the shared target and releases it before tests
  run, so one cell's tests overlap the next cell's build. Each cell's output
  prints whole when it finishes;
* retry: when every failure is a named failed test, the last line lists each
  failed cell's tests (`failed_tests`); `--rerun-failed LOG` reruns only those
  tests, one at a time, from that run's stdout log. The landing reruns them
  alone instead of the whole gate (tools/molt_dev_gates.py).

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
import threading
import tomllib
from concurrent.futures import ThreadPoolExecutor, as_completed
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
from tools.cargo_reach import (  # noqa: E402
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
from tools.proof_plan import (  # noqa: E402
    ProofPlan,
    excluded_backend_crates,
    excluded_backends,
)
from tools.toolchain_env import ToolchainNotProvisioned, toolchain_environment  # noqa: E402
from tools.runtime_test_gate_plan import parse_arguments as runtime_gate_arguments  # noqa: E402

COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")
SCHEMA = "molt.changed-crate-tests.v1"
PLAN_PATH = Path("tools") / "proof_plan.toml"
LOCAL_TIER = "pre-push"
RUNTIME_GATE = "tools/run_runtime_test_gate.py"
# dev-fast and the built-in test profile both inherit dev (Cargo.toml), so
# debug assertions and overflow checks match (HF-205's concern); dev-fast
# shares the build the runtime gate and the declared dev-fast cells make,
# where the test profile rebuilt every dependency (3 min 17 s for one
# two-test crate in the 2026-11-14 batch a7705a729f).
DEFAULT_TRUTH_PROFILE = "dev-fast"
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
# An integration test binary: `Running tests/NAME.rs (target/.../NAME-hash)`.
_RUNNING_INTEGRATION = re.compile(
    r"^\s*Running (?P<source>tests/\S+\.rs) \((?P<path>.+)\)\s*$"
)
# Members whose lib has no unit tests in any declared cell (measured). A
# 0-test total fails every other declared member: it means the cell compiled
# the tests out (molt-backend-wasm's tests need test-util). An entry that runs
# tests fails too, so the list only shrinks to the truth.
LIBS_WITHOUT_TESTS: frozenset[str] = frozenset({"molt-backend"})
_SUMMARY = re.compile(
    r"test result: (?P<status>ok|FAILED)\. (?P<passed>\d+) passed; "
    r"(?P<failed>\d+) failed; (?P<ignored>\d+) ignored; "
)
# One libtest outcome line, `test NAME ... ok`, whole only when the run
# captures test output (the rerun drops --nocapture for this).
_PASSED = re.compile(r"^test (?P<name>\S+) \.\.\. ok\s*$")
# libtest's last `failures:` block lists each failed test, indented, before
# the binary's summary, also under --nocapture.
_FAILED_NAME = re.compile(r"^    (?P<name>\S+)\s*$")
# A cell's process tree peaks near 5 GiB (the 2026-11-15 batches); Cargo's
# build lock bounds concurrent builds, so a few cells cover the test runs.
CELL_BYTES = 6 << 30
CELL_WORKERS = 3
RERUN_OPTION = "--rerun-failed"
# A cell problem a failed test alone explains; any other problem (a build
# failure, a missing summary) reruns the whole gate.
_TEST_PROBLEM = re.compile(r"^(cell exited \d+|integration test \S+: \d+ failed)$")
_FAILED_INTEGRATION = re.compile(
    r"^integration test (?P<name>\S+): (?P<count>\d+) failed$"
)


@dataclass(frozen=True, slots=True)
class Library:
    """A member's lib target as its manifest declares it."""

    present: bool
    # `[lib] test = false` or `harness = false`: no libtest summary exists.
    harnessed: bool
    # Its integration test targets: `[[test]]` names and, unless
    # `autotests = false`, each `tests/*.rs` file.
    tests: frozenset[str] = frozenset()


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
    tests: tuple[str, ...] = ()  # declared integration test targets

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
    # Integration test targets a source declares and a selected package owns.
    tests: set[str] = field(default_factory=set)

    @property
    def label(self) -> str:
        return "+".join(self.sources)

    def argv(self) -> tuple[str, ...]:
        argv = ["cargo", "test", *self.key.build]
        if self.key.features:
            argv += ["--features", ",".join(self.key.features)]
        for name in sorted(self.packages):
            argv += ["-p", self.packages[name]]
        argv += ["--lib"]
        for name in sorted(self.tests):
            argv += ["--test", name]
        argv += ["--no-fail-fast", "--message-format=json-render-diagnostics"]
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
    runner: tuple[str, ...] | None  # the runtime gate argv, when this gate runs it
    runner_packages: frozenset[str]
    unverified: tuple[str, ...]
    # The runtime gate's packages were reached, and a changed path selects the
    # gate's own rule, so the landing runs it as a gate of its own.
    runner_in_batch: bool = False


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
    tests = tuple(
        sorted({value for name, value in invocation.option_values if name == "--test"})
    )
    return Source(
        source_id,
        packages,
        tuple(build),
        features,
        libtest,
        skips,
        unrunnable,
        tuple(toolchains),
        tests,
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
    found = runtime_gate_rule(plan)
    if found is None:
        return None
    argv, _globs = found
    return argv, frozenset(
        image.package for image in SEMANTIC_IMAGES if not image.cases
    )


def runtime_gate_rule(plan: Mapping) -> tuple[tuple[str, ...], tuple[str, ...]] | None:
    """The semantic runtime gate's argv and the globs of the rule that owns it."""
    for rule in plan.get("rule", ()):
        for gate in rule.get("gates", ()):
            argv = tuple(shlex.split(gate))
            if RUNTIME_GATE not in argv:
                continue
            arguments = runtime_gate_arguments(
                list(argv[argv.index(RUNTIME_GATE) + 1 :])
            )
            if arguments.policy == "semantic":
                return argv, tuple(rule.get("globs", ()))
    return None


def runtime_rule_selected(plan: Mapping, paths: Iterable[str]) -> bool:
    """Whether a changed path selects the runtime gate's rule, as the landing
    policy matches rules (`molt_dev_policy`: a rule matches the paths it globs)."""
    from tools.dirty_tree_policy import is_ignored as glob_matches

    found = runtime_gate_rule(plan)
    return found is not None and any(glob_matches(path, found[1]) for path in paths)


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
        tests = {
            entry["name"]
            for entry in data.get("test", ())
            if isinstance(entry, dict) and isinstance(entry.get("name"), str)
        }
        if data.get("package", {}).get("autotests", True) is not False:
            tests.update(path.stem for path in (manifest.parent / "tests").glob("*.rs"))
        result[name] = Library(
            present=present,
            harnessed=lib.get("test", True) is not False
            and lib.get("harness", True) is not False,
            tests=frozenset(tests),
        )
    return result


def select(
    sources: Iterable[Source],
    reach: Reach,
    libs: Mapping[str, Library],
    runner: tuple[tuple[str, ...], frozenset[str]] | None,
    *,
    experimental: frozenset[str],
    runner_in_batch: bool = False,
) -> Selection:
    """The cells that run every reached crate's whole lib in its declared cells.

    `experimental` names the excluded backends' crates; none gets a cell.
    """
    runner_argv, runner_owned = runner if runner is not None else (None, frozenset())
    reached = frozenset(reach.packages)
    unverified = [
        f"{project}: separate Cargo project; no declared lib-test cell"
        for project in sorted(reach.projects)
    ]
    testable: set[str] = set()
    for name in sorted(reached):
        if name in experimental:
            unverified.append(
                f"{name}: experimental backend; the schedule runs it "
                "(config/release_acceptance_matrix.toml)"
            )
            continue
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
        # Cargo refuses a `--test` target that no selected package owns.
        owned = {test for name in selected for test in libs[name].tests}
        cell.tests.update(set(source.tests) & owned)
        if source.id not in cell.sources:
            cell.sources.append(source.id)
        cell.dropped.update(dropped)
    for name in sorted(testable - named - runner_owned):
        key = CellKey(("--locked", "--profile", DEFAULT_TRUTH_PROFILE), (), ())
        cell = cells.setdefault(key, Cell(key))
        cell.packages[name] = name
        cell.toolchains.update(("rustc", "cargo"))
        if "default-truth" not in cell.sources:
            cell.sources.append("default-truth")
    reached_runner = bool(testable & runner_owned)
    return Selection(
        reach=reach,
        cells=tuple(cells.values()),
        runner=runner_argv if reached_runner and not runner_in_batch else None,
        runner_packages=frozenset(testable & runner_owned),
        unverified=tuple(unverified),
        runner_in_batch=reached_runner and runner_in_batch,
    )


@dataclass(slots=True)
class CellResult:
    argv: tuple[str, ...]
    returncode: int
    counts: dict[str, dict[str, int]]  # package -> passed/failed/ignored
    problems: list[str]
    # Cargo target selector (`--lib` or `--test NAME`) -> failed test names.
    failed: dict[str, list[str]] = field(default_factory=dict)
    # Test names that reported ok, any target.
    passed: set[str] = field(default_factory=set)


def account(
    lines: Iterable[str],
    packages: Mapping[str, str],
    root: Path,
    tests: Iterable[str] = (),
) -> CellResult:
    """Bind each lib test binary's summary to its package, and require a
    summary from each declared integration test binary (argv/returncode are
    filled by the caller)."""
    members = {
        manifest.resolve(): name
        for name, manifest in workspace_member_packages(root).items()
    }
    binaries: dict[str, str] = {}
    counts: dict[str, dict[str, int]] = {}
    problems: list[str] = []
    failed: dict[str, list[str]] = {}
    passed: set[str] = set()
    current: str | None = None
    integration: str | None = None
    integration_seen: set[str] = set()
    listed: list[str] | None = None  # names of the latest `failures:` block
    for line in lines:
        text = line.rstrip("\n")
        if passing := _PASSED.match(text):
            passed.add(passing["name"])
            continue
        if text == "failures:":
            listed = []
            continue
        if listed is not None and (name := _FAILED_NAME.match(text)):
            listed.append(name["name"])
            continue
        if (summary := _SUMMARY.search(text)) and int(summary["failed"]):
            target = (
                "--lib"
                if current is not None
                else f"--test {integration}"
                if integration is not None
                else None
            )
            if target is not None and listed and len(listed) == int(summary["failed"]):
                failed.setdefault(target, []).extend(listed)
        if (
            _SUMMARY.search(text)
            or _RUNNING.match(text)
            or _RUNNING_INTEGRATION.match(text)
        ):
            listed = None
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
            integration = None
            if current is None or current not in packages:
                problems.append(f"test binary {binary} maps to no selected package")
                current = None
            continue
        if running := _RUNNING_INTEGRATION.match(line):
            current = None
            integration = Path(running["source"]).stem
            continue
        if (summary := _SUMMARY.search(line)) and integration is not None:
            integration_seen.add(integration)
            if int(summary["failed"]):
                problems.append(
                    f"integration test {integration}: {summary['failed']} failed"
                )
            integration = None
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
    for name in sorted(set(tests) - integration_seen):
        problems.append(f"integration test {name}: no summary (build or run failed)")
    return CellResult((), 0, counts, problems, failed, passed)


def rerun_argv(cell: Cell, failed: Mapping[str, Sequence[str]]) -> tuple[str, ...]:
    """The cell's build, limited to its failed targets and tests, one at a time.

    The package selection and features stay the cell's, so Cargo reuses its
    build; `--exact` names each failed test, and a name another selected
    binary also holds only runs there too.
    """
    argv = ["cargo", "test", *cell.key.build]
    if cell.key.features:
        argv += ["--features", ",".join(cell.key.features)]
    for name in sorted(cell.packages):
        argv += ["-p", cell.packages[name]]
    for target in sorted(failed):
        argv += shlex.split(target)
    argv += ["--no-fail-fast", "--message-format=json-render-diagnostics", "--"]
    # Captured output keeps each `test NAME ... ok` line whole.
    argv += [
        flag
        for flag in cell.key.libtest
        if not flag.startswith("--test-threads") and flag != "--nocapture"
    ]
    argv += ["--test-threads=1", "--exact"]
    argv += sorted({name for names in failed.values() for name in names})
    return tuple(argv)


def run_cell(
    cell: Cell,
    *,
    root: Path,
    plan: ProofPlan,
    echo: list[str] | None = None,
) -> CellResult:
    """Run ``cell``; its human output goes to ``echo`` when given, else
    straight to stdout."""
    return run_cargo_test(
        cell.argv(),
        cell.toolchains,
        cell.packages,
        cell.tests,
        root=root,
        plan=plan,
        echo=echo,
    )


def run_cargo_test(
    argv: tuple[str, ...],
    toolchains: Iterable[str],
    packages: Mapping[str, str],
    tests: Iterable[str],
    *,
    root: Path,
    plan: ProofPlan,
    echo: list[str] | None = None,
) -> CellResult:
    env = dict(os.environ)
    # The sources' own toolchains (LLVM SDK, WASI SDK), projected exactly as
    # tools/toolchain_env.py projects them for CI and changed-crate Clippy.
    env.update(
        toolchain_environment(
            plan, plan.toolchain_closure(sorted(toolchains)), environ=env
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
                if echo is None:
                    sys.stdout.write(line)
                    sys.stdout.flush()
                else:
                    echo.append(line)
            lines.append(line)
    finally:
        returncode = COMMANDS.wait_owned(process, timeout=30.0)
    result = account(lines, packages, root, tests)
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
    paths = changed_paths(base)
    reach = dependency_reach(Workspace.load(root), paths, base_reader(base))
    return select(
        plan_sources(plan, host=host_triple()),
        reach,
        libraries(root),
        runtime_gate(plan),
        experimental=excluded_backend_crates(excluded_backends(root)),
        runner_in_batch=runtime_rule_selected(plan, paths),
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
    elif selection.runner_in_batch:
        lines.append(
            "changed-crate-tests: runtime gate for "
            + ", ".join(sorted(selection.runner_packages))
            + ": a changed path selects its own rule, so the landing runs it as "
            "a gate; not run again here"
        )
    lines.extend(
        f"changed-crate-tests: unverified: {item}" for item in selection.unverified
    )
    return lines


def cell_workers(count: int) -> int:
    """Cells this host runs at once (``CELL_BYTES`` each, at most
    ``CELL_WORKERS``), within the memory slice the landing exports."""
    from molt import host_capacity

    return max(
        1,
        min(
            count,
            CELL_WORKERS,
            host_capacity.worker_count(
                bytes_per_worker=CELL_BYTES,
                headroom_bytes=host_capacity.headroom_bytes(
                    host_capacity.memory_capacity()
                ),
            ),
        ),
    )


def failed_tests(
    cells: Sequence[Cell], results: Mapping[str, CellResult]
) -> list[dict] | None:
    """Each failed cell's rerun of its failed tests, or None when a failure
    is not a named failed test (a build failure, a missing summary)."""
    reruns: list[dict] = []
    for cell in cells:
        result = results[cell.label]
        if not result.problems:
            continue
        if not result.failed or not all(
            _TEST_PROBLEM.match(problem) for problem in result.problems
        ):
            return None
        # Every failed test is named: the lib summaries and each failed
        # integration summary count exactly the names their blocks listed.
        failed_libs = sum(counts["failed"] for counts in result.counts.values())
        if failed_libs != len(result.failed.get("--lib", ())):
            return None
        for problem in result.problems:
            if (named := _FAILED_INTEGRATION.match(problem)) and int(
                named["count"]
            ) != len(result.failed.get(f"--test {named['name']}", ())):
                return None
        reruns.append(
            {
                "cell": cell.label,
                "argv": list(rerun_argv(cell, result.failed)),
                "toolchains": sorted(cell.toolchains),
                "packages": dict(cell.packages),
                "tests": sorted(
                    {name for names in result.failed.values() for name in names}
                ),
            }
        )
    return reruns or None


def rerun_failed(log: Path) -> int:
    """Rerun, one at a time, the failed tests the run that wrote ``log``
    names; pass only when each reports ok."""
    record = None
    for line in reversed(log.read_text(encoding="utf-8").splitlines()):
        if line.startswith("{"):
            record = json.loads(line)
            break
    if (
        not isinstance(record, dict)
        or record.get("schema") != SCHEMA
        or not record.get("failed_tests")
    ):
        print(f"changed-crate-tests: {log} names no failed tests", file=sys.stderr)
        return 2
    proof_plan = ProofPlan.load(ROOT / PLAN_PATH)
    failures: list[str] = []
    flaky: list[str] = []
    for rerun in record["failed_tests"]:
        argv = tuple(rerun["argv"])
        print(f"changed-crate-tests: rerun [{rerun['cell']}]: {shlex.join(argv)}")
        result = run_cargo_test(
            argv,
            rerun["toolchains"],
            {},
            (),
            root=ROOT,
            plan=proof_plan,
        )
        missing = sorted(set(rerun["tests"]) - result.passed)
        if result.returncode != 0 or missing:
            failures.append(
                f"[{rerun['cell']}] failed alone too: "
                + ", ".join(missing or rerun["tests"])
            )
        else:
            flaky.extend(rerun["tests"])
    for failure in failures:
        print(f"changed-crate-tests: FAILED {failure}", file=sys.stderr)
    if not failures:
        print(
            "changed-crate-tests: every failed test passed alone (load or a "
            "flake, not the candidate): " + ", ".join(flaky)
        )
    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "status": "failed" if failures else "passed",
                "rerun_of": str(log),
                "flaky": flaky,
                "failures": failures,
            },
            sort_keys=True,
        )
    )
    return 1 if failures else 0


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
    parser.add_argument(
        RERUN_OPTION,
        type=Path,
        metavar="LOG",
        help="rerun only the failed tests the run that wrote this stdout log names",
    )
    args = parser.parse_args(argv)
    if args.rerun_failed is not None:
        try:
            return rerun_failed(args.rerun_failed)
        except ToolchainNotProvisioned as exc:
            print(f"changed-crate-tests: {exc}", file=sys.stderr)
            return 2
    selection = selection_for(args.base)
    for line in describe(selection):
        print(line)
    if not selection.cells and selection.runner is None:
        print("changed-crate-tests: no lib test cell reached; nothing to run")
        print(
            json.dumps(
                {
                    "schema": SCHEMA,
                    "status": "passed",
                    "cells": [],
                    "unverified": list(selection.unverified),
                    "failures": [],
                },
                sort_keys=True,
            )
        )
        return 0
    if args.dry_run:
        return 0
    proof_plan = ProofPlan.load(ROOT / PLAN_PATH)
    failures: list[str] = []
    records: list[dict] = []
    totals: dict[str, int] = {}
    declared: set[str] = set()
    results: dict[str, CellResult] = {}
    printing = threading.Lock()

    def run(cell: Cell) -> CellResult:
        echo: list[str] = []
        with printing:
            print(f"changed-crate-tests: running [{cell.label}]", flush=True)
        try:
            return run_cell(cell, root=ROOT, plan=proof_plan, echo=echo)
        finally:
            with printing:
                print(f"changed-crate-tests: output [{cell.label}]")
                sys.stdout.writelines(echo)
                sys.stdout.flush()

    runner_returncode: int | None = None
    workers = cell_workers(len(selection.cells) + (selection.runner is not None))
    print(f"changed-crate-tests: {workers} cell(s) at once")
    with ThreadPoolExecutor(max_workers=workers) as pool:
        # The runtime gate is one more job beside the cells.
        runner = (
            pool.submit(run_runner, selection.runner, root=ROOT)
            if selection.runner is not None
            else None
        )
        pending = {pool.submit(run, cell): cell for cell in selection.cells}
        try:
            for future in as_completed(pending):
                results[pending[future].label] = future.result()
        except ToolchainNotProvisioned as exc:
            for future in pending:
                future.cancel()
            print(f"changed-crate-tests: {exc}", file=sys.stderr)
            return 2
        if runner is not None:
            runner_returncode = runner.result()
    for cell in selection.cells:
        result = results[cell.label]
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
    test_failures_only = bool(failures)
    for name in sorted(declared):
        ran = totals.get(name, 0)
        if ran == 0 and name not in LIBS_WITHOUT_TESTS:
            failures.append(
                f"{name}: a declared crate ran 0 lib tests in all its cells"
            )
            test_failures_only = False
        elif ran and name in LIBS_WITHOUT_TESTS:
            failures.append(
                f"{name}: ran {ran} lib tests; remove it from LIBS_WITHOUT_TESTS"
            )
            test_failures_only = False
    for name in sorted(set(totals) - declared):
        if totals[name] == 0:
            print(
                f"changed-crate-tests: {name}: 0 lib tests (no plan command names it)"
            )
    if selection.runner is not None:
        assert runner_returncode is not None
        records.append(
            {
                "sources": ["runtime-gate"],
                "argv": list(selection.runner),
                "returncode": runner_returncode,
                "packages": sorted(selection.runner_packages),
            }
        )
        if runner_returncode != 0:
            failures.append(f"runtime gate exited {runner_returncode}")
            test_failures_only = False
    reruns = failed_tests(selection.cells, results) if test_failures_only else None
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
                **({"failed_tests": reruns} if reruns else {}),
            },
            sort_keys=True,
        )
    )
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
