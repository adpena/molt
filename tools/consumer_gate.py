#!/usr/bin/env python3
"""Run a change's static gates and first-hop importer tests before commit.

``run -- PATHS`` first runs the static gates the landing selects for the
change (the repository-wide policy checks that read every file as data, which
no importer trace selects), then traces the change with
``tools/test_dependency_graph.py`` (first-hop importers and data readers),
prints both lists for review, runs the importer test files with this
checkout's interpreter, and records a receipt
for the exact tree the commit would produce: ``HEAD`` plus the working-tree
bytes of ``PATHS``, which is what ``git commit --only -- PATHS`` commits.

``check -- PATHS`` refuses (exit 1) unless a passing receipt exists for that
tree; it reads no test and runs nothing. A change that touches only generator
outputs (``tools/generator_manifest.toml``) needs no receipt: the generator
gates check those bytes.

Presubmit runs importers; data readers are listed for review and run in
postsubmit (owner decision 2026-11-06, docs/design/landing_queue.md 9.10).
Prior art: pytest-testmon and Bazel ``rdeps`` select tests by dependency; this
tool only moves that selection before the commit.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402

if TYPE_CHECKING:
    from molt.host_capacity import HostWorkDemand
    from tools.molt_dev_policy import SelectedGate

# Git probes and the importer-test child run under the developer-test guard.
_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV_TEST")
# The importer run holds a share of the host (``host_capacity``). Unbounded,
# one hung test held it for 37 minutes at 0% CPU on 2026-11-17 and every
# other lane's gate queued behind it. Presubmit's p90 target is 10 minutes;
# three times that is a hang, not a slow suite. pytest's
# ``faulthandler_timeout`` (pyproject) dumps the stuck test's stacks first.
IMPORTER_RUN_TIMEOUT_S = 1800
# Each rerun (alone, then on the base) runs at most RERUN_LIMIT tests, about a
# minute each alone under load. Unbounded, the base rerun's hung backend
# daemon held the host slot for 41 minutes on 2026-11-18, the same queueing
# the importer bound above was added for.
RERUN_TIMEOUT_S = 600

SCHEMA = "molt.consumer-receipt.v1"


def _git(root: Path, *args: str, env: dict[str, str] | None = None) -> str:
    return _COMMANDS.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
        env=env,
        encoding="utf-8",
    ).stdout.strip()


def commit_tree(root: Path, paths: list[str]) -> str:
    """The tree ``git commit --only -- paths`` would record: HEAD plus the
    working-tree bytes of ``paths`` (a deleted path is removed).

    A path absent from both the working tree and HEAD (a deletion already
    committed) changes nothing; ``git add`` would refuse it as unmatched.
    """
    with tempfile.TemporaryDirectory(prefix="molt-consumer-gate-") as scratch:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(scratch) / "index"))
        _git(root, "read-tree", "HEAD", env=env)
        tracked = set(
            _git(root, "ls-files", "-z", "--", *paths, env=env).split("\0")
            if paths
            else ()
        )
        present = [
            path for path in paths if os.path.lexists(root / path) or path in tracked
        ]
        if present:
            _git(root, "add", "--all", "--", *present, env=env)
        return _git(root, "write-tree", env=env)


def changed_paths(root: Path, tree: str, *, base: str = "HEAD") -> list[str]:
    output = _git(root, "diff", "--name-only", base, tree)
    return [line for line in output.splitlines() if line]


def generator_outputs(root: Path) -> frozenset[str]:
    manifest = tomllib.loads(
        (root / "tools/generator_manifest.toml").read_text(encoding="utf-8")
    )
    return frozenset(
        output for row in manifest.get("generator", []) for output in row["outputs"]
    )


def receipt_path(root: Path, tree: str) -> Path:
    common = Path(_git(root, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    return common / "molt" / "consumer-receipts" / f"{tree}.json"


def proven_gates(
    root: Path, tree: str, *, exact: bool = False
) -> Callable[[SelectedGate], bool] | None:
    """The landing gates the receipt for ``tree`` already ran, or None.

    A static gate reads only the tree's files, so the receipt's run of the
    same command proves it. A pytest gate is proven when it skips slow
    tests, as the receipt's run did, and each of its targets is a file the
    receipt ran as an importer; a ``-k`` or ``-m`` selection only narrows
    those files. A test that needs landing-built artifacts is never a receipt
    importer, so its cohort is never proven.

    ``exact`` accepts only a receipt whose run saw exactly the tree's files
    (no other tracked edit and no untracked file in the checkout): the
    queue's batch, the landing's last run, asks for it. The lane precheck
    does not, because the batch runs after it.
    """
    try:
        payload = json.loads(receipt_path(root, tree).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if payload.get("schema") != SCHEMA or payload.get("tree") != tree:
        return None
    if exact and payload.get("exact") is not True:
        return None
    statics = frozenset(payload["static_gates"])
    importers = frozenset(payload["importers"])
    import shlex

    from tools.molt_dev_gates import display
    from molt.pytest_memory_guard_bootstrap import parse_pytest_arguments
    from tools.proof_command_argv import pytest_arguments

    def proven(gate: SelectedGate) -> bool:
        if gate.static:
            return display(gate.command) in statics
        if not gate.pytest:
            return False
        argv = (
            shlex.split(gate.command) if isinstance(gate.command, str) else gate.command
        )
        arguments = pytest_arguments(argv, root=root)
        if arguments is None or "--skip-slow" not in arguments:
            return False
        # The cohort's targets as pytest's own parser reads them. An option
        # that changes the configuration or conftests pytest loads makes the
        # run differ from the receipt's, and a module target or a path the
        # checkout lacks selects differently or fails: not proven.
        try:
            parsed = parse_pytest_arguments(arguments, plugin_options=True)
        except ValueError:
            return False
        if (
            parsed.pyargs
            or parsed.inifile is not None
            or parsed.override_ini
            or parsed.rootdir is not None
            or parsed.noconftest
            or parsed.confcutdir is not None
        ):
            return False
        targets = [Path(target.path) for target in parsed.targets]
        return bool(targets) and all(
            not target.is_absolute()
            and (root / target).is_file()
            and target.as_posix() in importers
            for target in targets
        )

    return proven


def checkout_matches(root: Path, tree: str) -> bool:
    """Whether the checkout holds exactly ``tree``'s files: no file of the
    tree differs on disk and no untracked, unignored file exists. A scratch
    index holds the tree, so a new file of the change counts as tracked."""
    with tempfile.TemporaryDirectory(prefix="molt-consumer-gate-") as scratch:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(scratch) / "index"))
        _git(root, "read-tree", tree, env=env)
        return not _git(root, "diff", "--name-only", env=env) and not _git(
            root, "ls-files", "--others", "--exclude-standard", env=env
        )


def _needs_receipt(root: Path, changed: list[str]) -> bool:
    return bool(changed) and not set(changed) <= generator_outputs(root)


def static_gates(root: Path, tree: str, *, base: str = "HEAD") -> list[str]:
    """The landing's static gates for this commit tree, as their commands.

    The selection is the landing's own (select_landing_gates over a dangling
    probe commit of ``tree``; no ref moves), so the repository-wide policy
    checks an importer trace cannot select (encoding pins, test ownership,
    subprocess guard coverage, generator freshness) run before any test.
    """
    if not (root / "tools" / "proof_plan.toml").is_file():
        return []
    from tools import molt_dev as drv
    from tools.molt_dev_gates import display
    from tools.molt_dev_policy import select_landing_gates

    head = _git(root, "rev-parse", base)
    # A dangling, never-published probe: it names its own identity so a host
    # with no Git user configured (a farm checkout) can create it.
    identity = {
        "GIT_AUTHOR_NAME": "molt consumer gate",
        "GIT_AUTHOR_EMAIL": "consumer-gate@molt.invalid",
        "GIT_COMMITTER_NAME": "molt consumer gate",
        "GIT_COMMITTER_EMAIL": "consumer-gate@molt.invalid",
    }
    probe = _git(
        root,
        "commit-tree",
        tree,
        "-p",
        head,
        "-m",
        "consumer-gate probe",
        env=dict(os.environ, **identity),
    )
    _policy, _config, _touched, gates, _rules = select_landing_gates(
        drv.Git(root), root, probe, head, extra=[]
    )
    return [display(gate.command) for gate in gates if gate.static]


def run_static_gates(root: Path, commands: list[str]) -> list[str]:
    """Run each static gate in the checkout; return the commands that failed."""
    import shlex

    failed = []
    for command in commands:
        argv = shlex.split(command)
        if argv and argv[0] == "python":
            argv[0] = sys.executable
        print(f"consumer-gate: static gate: {command}", flush=True)
        if _COMMANDS.run(argv, cwd=root).returncode != 0:
            failed.append(command)
    return failed


def landing_built_tests(
    root: Path, importers: Sequence[str]
) -> dict[str, tuple[str, ...]]:
    """Importer test files whose proof-plan command needs a compiler build first.

    A pytest command consumes what its dependency closure in
    tools/proof_plan.toml produces. Only a compiler build (resource class
    `compiler-build-resource`, for example the checkout's WASM runtime pair)
    makes an artifact that a pre-commit run cannot provide: the proof executor
    builds it from the committed tree, so the landing runs these tests.
    Ordering dependencies on checks keep a test in the pre-commit run. A
    checkout with no proof plan declares no builds.
    """
    from tools.proof_plan import ProofPlan

    manifest = root / "tools" / "proof_plan.toml"
    if not manifest.is_file():
        return {}
    commands = {command.id: command for command in ProofPlan.load(manifest).commands}

    def builds(command_id: str) -> tuple[str, ...]:
        found: list[str] = []
        pending = list(commands[command_id].dependencies)
        seen: set[str] = set()
        while pending:
            dependency = pending.pop()
            if dependency in seen:
                continue
            seen.add(dependency)
            if commands[dependency].data["resource_class"] == "compiler-build-resource":
                found.append(dependency)
            pending.extend(commands[dependency].dependencies)
        return tuple(sorted(found))

    owners: dict[str, tuple[str, ...]] = {}
    for command in commands.values():
        needed = builds(command.id) if command.pytest_targets else ()
        if not needed:
            continue
        for target in command.pytest_targets:
            owners.setdefault(target.split("::", 1)[0], needed)
    return {test: owners[test] for test in importers if test in owners}


def _importer_demand(argv: Sequence[str], files: int) -> HostWorkDemand:
    """The host share an importer run asks for (``molt.host_capacity``): one
    worker per test file, and never less than half the host, since one file
    can hold thousands of tests (tests/test_molt_dev.py holds about 2,700).
    Two gates of up to half the host's files run together; a larger one asks
    for more, up to the whole class."""
    from molt import host_capacity
    from tools.proof_plan import PYTEST_WORKER_MEMORY_OPTION

    prefix = PYTEST_WORKER_MEMORY_OPTION + "="
    unit = next(float(word[len(prefix) :]) for word in argv if word.startswith(prefix))
    half = (host_capacity.cpu_count() + 1) // 2
    return host_capacity.HostWorkDemand(
        workers=max(half, files), bytes_per_worker=max(1, int(unit * 1024**3))
    )


def presubmit_pytest_argv(root: Path, tests: Sequence[str]) -> list[str]:
    """The importer run as presubmit makes it: the landing's own formatter
    (``local_pytest_argv``: ``-n auto``, the tests' declared worker memory)
    with ``--skip-slow``, so a `slow` test runs in postsubmit, as it does for
    the landing, and never holds a commit."""
    from tools.molt_dev_policy import GateConfig
    from tools.proof_plan import local_pytest_argv

    units = GateConfig.load(root / "tools" / "proof_plan.toml").pytest_memory_units
    command = local_pytest_argv(tests, ("--skip-slow",), memory_units=units)
    assert command[0] == "python", command
    return [sys.executable, *command[1:]]


def run(
    root: Path,
    paths: list[str],
    *,
    pytest_args: list[str] | None,
    base: str = "HEAD",
    bypass_importers: str | None = None,
) -> int:
    """Gate the tree a commit of ``paths`` would record, against ``base``.

    ``base`` is HEAD for work about to be committed. A committed stack (a
    rebase onto a new main, a bypass push) names the main it lands on, so
    every commit since that main is the change.

    ``bypass_importers`` (a reason) is the owner's bounded bypass for work
    that unblocks development velocity (2026-11-13, 2026-11-15): the static
    gates and the change's own test files still run; the importer suites do
    not, and the receipt names the reason and every skipped importer. Such a
    receipt proves only the gates that ran (``proven_gates``), so the landing
    and postsubmit still run the rest; a red postsubmit is reverted within the
    hour.
    """
    from tools.test_dependency_graph import git_blob_reader, load_graph

    base = _git(root, "rev-parse", "--verify", f"{base}^{{commit}}")
    tree = commit_tree(root, paths)
    exact = checkout_matches(root, tree)
    changed = changed_paths(root, tree, base=base)
    statics = static_gates(root, tree, base=base)
    print(f"consumer-gate: {len(statics)} static gate(s) the landing selects")
    failed = run_static_gates(root, statics)
    if failed:
        print(
            "consumer-gate: static gates failed; no receipt:\n"
            + "\n".join(f"  {command}" for command in failed),
            file=sys.stderr,
        )
        return 1
    # A changed module selects the tests that reach the definitions it
    # changes, not every test below a conftest that uses one of its functions.
    readers = load_graph(root).direct_readers(
        changed, before=git_blob_reader(root, base)
    )
    landing_built = landing_built_tests(root, readers.importers)
    run_now = [test for test in readers.importers if test not in landing_built]
    bypassed: list[str] = []
    if bypass_importers is not None:
        own_tests = [
            path
            for path in changed
            if path.startswith("tests/")
            and path.endswith(".py")
            and PurePosixPath(path).name.startswith("test_")
            and (root / path).is_file()
        ]
        bypassed = [test for test in run_now if test not in own_tests]
        run_now = own_tests
        print(
            f"consumer-gate: BYPASS ({bypass_importers}): running the change's own "
            f"{len(own_tests)} test file(s); {len(bypassed)} importer file(s) are "
            "left to the landing and postsubmit",
            flush=True,
        )
    print(f"consumer-gate: {len(changed)} changed path(s), tree {tree[:12]}")
    for label, tests in (
        ("importers (run now)", run_now),
        (
            "importers that need landing-built artifacts (the landing runs them)",
            [f"{test} <- {', '.join(deps)}" for test, deps in landing_built.items()],
        ),
        ("data readers (review; postsubmit runs them)", readers.data_readers),
    ):
        print(f"# {label}: {len(tests)}")
        for test in tests:
            print(f"  {test}")
    flaky_reruns: list[str] | None = []
    slow_cases: list[tuple[str, float]] = []
    if run_now:
        if pytest_args is None:
            argv = presubmit_pytest_argv(root, run_now)
        else:
            argv = [sys.executable, "-m", "pytest", "-q", *pytest_args, *run_now]
        from molt import host_capacity

        # The importer run takes a share of the host's development capacity:
        # one pytest worker per test file, each of the files' declared memory.
        # A small gate leaves room beside it; a large one takes the class.
        demand = _importer_demand(argv, len(run_now)) if pytest_args is None else None
        with host_capacity.host_work_slot(
            host_capacity.host_work_dir(root),
            label=f"consumer gate ({root.name}, {len(run_now)} test files)",
            work=host_capacity.HostWorkClass.DEVELOPMENT,
            say=lambda message: print(message, flush=True),
            demand=demand,
        ) as admission:
            if admission.waited_seconds >= 1:
                print(
                    f"consumer-gate: admitted after {admission.waited_seconds:.0f} s",
                    flush=True,
                )
            # The importer tests are the holder's job: a test that runs a
            # consumer gate in a child is admitted inside this slot.
            env = {**os.environ, **host_capacity.held_slot_environment()}
            if admission.grant is not None:
                from tools.proof_command_argv import allocated_pytest_argv

                env[host_capacity.MEMORY_SLICE_ENV] = str(admission.grant.memory_bytes)
                argv, workers = allocated_pytest_argv(
                    argv, root=root, cpus=admission.grant.workers, environment=env
                )
                argv = list(argv)
                print(
                    f"consumer-gate: granted {admission.grant.workers} worker(s) and "
                    f"{admission.grant.memory_bytes / 1024**3:.1f} GiB; running {workers}",
                    flush=True,
                )
            print("consumer-gate: " + " ".join(argv), flush=True)
            report = Path(tempfile.mkdtemp(prefix="consumer-gate-")) / "junit.xml"
            try:
                failed = (
                    _COMMANDS.run(
                        [*argv, f"--junitxml={report}"],
                        cwd=root,
                        env=env,
                        timeout=IMPORTER_RUN_TIMEOUT_S,
                    ).returncode
                    != 0
                )
            except subprocess.TimeoutExpired:
                print(
                    f"consumer-gate: importer tests ran past {IMPORTER_RUN_TIMEOUT_S} s "
                    "and were stopped (a hang: the faulthandler stacks above name "
                    "the test); no receipt",
                    file=sys.stderr,
                )
                return 1
            slow_cases = _slow_cases(report)
            for name, seconds in slow_cases:
                print(
                    f"consumer-gate: unmarked slow test ({seconds:.0f} s): {name}; "
                    "mark it @pytest.mark.slow or make it fast",
                    file=sys.stderr,
                    flush=True,
                )
            preexisting: list[str] = []
            if failed:
                flaky_reruns, still_failing = _rerun_failures_alone(root, report, env)
                failed = flaky_reruns is None
                if failed and still_failing:
                    preexisting = _failures_on_base(root, base, still_failing, env)
                    failed = preexisting != still_failing
        if failed:
            print("consumer-gate: importer tests failed; no receipt", file=sys.stderr)
            return 1
        refused = [
            name
            for name, seconds in slow_cases
            if seconds >= SLOW_TEST_REFUSE_S and _case_in_paths(name, paths)
        ]
        if refused:
            print(
                f"consumer-gate: {len(refused)} unmarked test(s) this change "
                f"touches ran {SLOW_TEST_REFUSE_S:.0f} s or more; no receipt",
                file=sys.stderr,
            )
            return 1
    if commit_tree(root, paths) != tree:
        print("consumer-gate: the change moved while its tests ran", file=sys.stderr)
        return 1
    exact = exact and checkout_matches(root, tree)
    path = receipt_path(root, tree)
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "schema": SCHEMA,
        "tree": tree,
        "base": base,
        "changed": changed,
        "static_gates": statics,
        "importers": run_now,
        "landing_built_importers": {
            test: list(deps) for test, deps in landing_built.items()
        },
        "data_readers": list(readers.data_readers),
        # Tests that failed beside the suite's load and passed alone (Chromium's
        # retry rule); tests/ owners replace their wall-clock waits.
        "flaky_reruns": flaky_reruns,
        # Tests that fail on the base commit too (Chromium's "without patch"
        # retry): main's defects, not this change's; their owners fix main.
        "preexisting_failures": preexisting if run_now else [],
        # Unmarked tests that ran SLOW_TEST_S or more (AGENTS.md 60 s rule).
        "slow_unmarked": [[name, round(seconds, 1)] for name, seconds in slow_cases],
        # The owner's bounded bypass: its reason and the importers not run here.
        "bypass": bypass_importers,
        "bypassed_importers": bypassed,
        # The tests saw only the tree's files; the queue's batch reuses only
        # such a receipt (proven_gates(exact=True)).
        "exact": exact,
    }
    path.write_text(json.dumps(payload, indent=1) + "\n", encoding="utf-8")
    print(f"consumer-gate: receipt {path}")
    return 0


# More failures than this are a broken change, not load: no rerun.
RERUN_LIMIT = 10


# AGENTS.md: a test whose median is 60 s or more is marked `slow` and runs in
# postsubmit. Presubmit skips marked tests, so a case that reaches the limit
# here is unmarked. One run is not a median, so the gate names a case from
# SLOW_TEST_S and withholds its receipt only from SLOW_TEST_REFUSE_S, and only
# when the change touches that test's file: one run of another file's test on
# a loaded host says nothing about this change (2026-11-21: 3,039 importer
# tests passed and ten untouched ones, WASM link checks among them, ran
# 120-163 s at load average 10).
SLOW_TEST_S = 60.0
SLOW_TEST_REFUSE_S = 2 * SLOW_TEST_S


def _case_in_paths(name: str, paths: Sequence[str]) -> bool:
    """Whether the JUnit case ``name`` (``classname::name``) lives in one of
    ``paths``: its class name is a changed test module or a class inside one."""
    classname = name.split("::", 1)[0]
    for path in paths:
        if not path.endswith(".py"):
            continue
        module = path[: -len(".py")].replace("/", ".")
        if classname == module or classname.startswith(f"{module}."):
            return True
    return False


def _slow_cases(report: Path) -> list[tuple[str, float]]:
    """Each test case in a JUnit report that ran at least SLOW_TEST_S, with its
    time, slowest first; an unreadable report names none."""
    import xml.etree.ElementTree as ElementTree

    try:
        cases = ElementTree.parse(report).getroot().iter("testcase")
    except (OSError, ElementTree.ParseError):
        return []
    slow: list[tuple[str, float]] = []
    for case in cases:
        if case.find("skipped") is not None:
            continue
        try:
            seconds = float(case.get("time", "0"))
        except ValueError:
            continue
        if seconds >= SLOW_TEST_S:
            slow.append(
                (f"{case.get('classname', '')}::{case.get('name', '')}", seconds)
            )
    return sorted(slow, key=lambda item: -item[1])


def _failed_node_ids(root: Path, report: Path) -> list[str] | None:
    """The pytest node ids that failed or errored in a JUnit report, or None
    when the report is missing or names a case no test file owns."""
    import xml.etree.ElementTree as ElementTree

    try:
        cases = ElementTree.parse(report).getroot().iter("testcase")
    except (OSError, ElementTree.ParseError):
        return None
    ids: list[str] = []
    for case in cases:
        if case.find("failure") is None and case.find("error") is None:
            continue
        parts = case.get("classname", "").split(".")
        for split in range(len(parts), 0, -1):
            path = "/".join(parts[:split]) + ".py"
            if (root / path).is_file():
                ids.append("::".join([path, *parts[split:], case.get("name", "")]))
                break
        else:
            return None
    return ids


def _rerun_failures_alone(
    root: Path, report: Path, env: Mapping[str, str]
) -> tuple[list[str] | None, list[str]]:
    """Rerun the failed tests once, alone and in order, as the landing reruns
    a failed gate alone (Chromium's retry rule). Return the tests that passed
    alone (None when any still fails) and the tests that still fail; an
    unreadable report or more failures than RERUN_LIMIT still fail as a whole
    (no ids, no base retry)."""
    ids = _failed_node_ids(root, report)
    if ids is None:
        return None, []
    if not ids:
        return None, []
    if len(ids) > RERUN_LIMIT:
        return None, ids
    print(f"consumer-gate: rerunning {len(ids)} failed test(s) alone once", flush=True)
    alone = report.with_name("alone.junit.xml")
    rerun = [sys.executable, "-m", "pytest", "-q", *ids, f"--junitxml={alone}"]
    try:
        passed = (
            _COMMANDS.run(rerun, cwd=root, env=env, timeout=RERUN_TIMEOUT_S).returncode
            == 0
        )
    except subprocess.TimeoutExpired:
        print(
            f"consumer-gate: the rerun alone ran past {RERUN_TIMEOUT_S} s and was "
            "stopped; every rerun test still fails",
            file=sys.stderr,
        )
        return None, ids
    if passed:
        for test in ids:
            print(f"consumer-gate: flaky under load, passed alone: {test}", flush=True)
        return ids, []
    still = _failed_node_ids(root, alone)
    return None, still if still is not None else ids


def _failures_on_base(
    root: Path, base: str, tests: list[str], env: Mapping[str, str]
) -> list[str]:
    """The ``tests`` that also fail on ``base`` (Chromium's "without patch"
    retry), in their given order. They run in a temporary detached worktree
    of ``base`` with its sources first on the import path, so the base's code,
    not this checkout's editable install, is under test. A test absent at the
    base is not a base failure."""
    import shutil

    scratch = Path(tempfile.mkdtemp(prefix="consumer-gate-base-"))
    tree = scratch / "base"
    try:
        _git(root, "worktree", "add", "--detach", "--quiet", str(tree), base)
        present = [test for test in tests if (tree / test.split("::")[0]).is_file()]
        if not present:
            return []
        report = scratch / "base.junit.xml"
        print(
            f"consumer-gate: rerunning {len(present)} failing test(s) on base "
            f"{base[:12]}",
            flush=True,
        )
        base_env = {
            **env,
            "PYTHONPATH": os.pathsep.join(
                [str(tree / "src"), str(tree), env.get("PYTHONPATH", "")]
            ).rstrip(os.pathsep),
        }
        try:
            result = _COMMANDS.run(
                [
                    sys.executable,
                    "-m",
                    "pytest",
                    "-q",
                    *present,
                    f"--junitxml={report}",
                ],
                cwd=tree,
                env=base_env,
                timeout=RERUN_TIMEOUT_S,
            )
        except subprocess.TimeoutExpired:
            # A hang proves nothing about the base: no failure is main's.
            print(
                f"consumer-gate: the base rerun ran past {RERUN_TIMEOUT_S} s and was "
                "stopped; no failure counts as main's defect",
                file=sys.stderr,
            )
            return []
        if result.returncode == 0:
            return []
        failed_on_base = set(_failed_node_ids(tree, report) or ())
        on_base = [test for test in tests if test in failed_on_base]
        for test in on_base:
            print(
                f"consumer-gate: fails on base too (main's defect): {test}", flush=True
            )
        return on_base
    finally:
        if tree.exists():
            _git(root, "worktree", "remove", "--force", str(tree))
        shutil.rmtree(scratch, ignore_errors=True)


def check(root: Path, paths: list[str]) -> int:
    tree = commit_tree(root, paths)
    changed = changed_paths(root, tree)
    if not _needs_receipt(root, changed):
        return 0
    path = receipt_path(root, tree)
    try:
        receipt = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        receipt = None
    if (
        isinstance(receipt, dict)
        and receipt.get("schema") == SCHEMA
        and (receipt.get("tree") == tree)
    ):
        return 0
    shown = " ".join(paths) if paths else "<paths>"
    print(
        f"consumer-gate: no passing importer-test receipt for tree {tree[:12]} "
        f"({len(changed)} changed path(s)). Trace and run the tests that import "
        "the change first:\n"
        "  uv run --no-project python tools/venv_exec.py python tools/consumer_gate.py "
        f"run -- {shown}",
        file=sys.stderr,
    )
    return 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("mode", choices=("run", "check"))
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument(
        "--pytest-arg",
        action="append",
        default=None,
        help=(
            "pytest argument for run (repeatable); given, it replaces the "
            "presubmit options (-n auto, declared worker memory, --skip-slow)"
        ),
    )
    parser.add_argument(
        "--base",
        default="HEAD",
        help=(
            "run: the commit the change is measured from (default HEAD, the "
            "work about to be committed); a committed stack names its main"
        ),
    )
    parser.add_argument(
        "--bypass-importers",
        metavar="REASON",
        default=None,
        help=(
            "run: the owner's bounded bypass for velocity work; run the static "
            "gates and the change's own tests, record REASON and the skipped "
            "importers; the landing and postsubmit run the rest"
        ),
    )
    parser.add_argument("paths", nargs="*", help="the paths the commit names")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    if args.mode == "run":
        return run(
            root,
            args.paths,
            pytest_args=args.pytest_arg,
            base=args.base,
            bypass_importers=args.bypass_importers,
        )
    return check(root, args.paths)


if __name__ == "__main__":
    raise SystemExit(main())
