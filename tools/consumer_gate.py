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
import sys
import tempfile
import tomllib
from collections.abc import Sequence
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402

# Git probes and the importer-test child run under the developer-test guard.
_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV_TEST")

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
    working-tree bytes of ``paths`` (a deleted path is removed)."""
    with tempfile.TemporaryDirectory(prefix="molt-consumer-gate-") as scratch:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(scratch) / "index"))
        _git(root, "read-tree", "HEAD", env=env)
        if paths:
            _git(root, "add", "--all", "--", *paths, env=env)
        return _git(root, "write-tree", env=env)


def changed_paths(root: Path, tree: str) -> list[str]:
    output = _git(root, "diff", "--name-only", "HEAD", tree)
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


def _needs_receipt(root: Path, changed: list[str]) -> bool:
    return bool(changed) and not set(changed) <= generator_outputs(root)


def static_gates(root: Path, tree: str) -> list[str]:
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

    head = _git(root, "rev-parse", "HEAD")
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


def run(root: Path, paths: list[str], *, pytest_args: list[str] | None) -> int:
    from tools.test_dependency_graph import git_blob_reader, load_graph

    tree = commit_tree(root, paths)
    changed = changed_paths(root, tree)
    statics = static_gates(root, tree)
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
        changed, before=git_blob_reader(root, "HEAD")
    )
    landing_built = landing_built_tests(root, readers.importers)
    run_now = [test for test in readers.importers if test not in landing_built]
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
    if run_now:
        if pytest_args is None:
            argv = presubmit_pytest_argv(root, run_now)
        else:
            argv = [sys.executable, "-m", "pytest", "-q", *pytest_args, *run_now]
        from molt import host_capacity

        # The importer run is suite-sized: it takes the host's one heavy-work
        # slot, behind any landing, instead of sharing the machine with it.
        with host_capacity.host_work_slot(
            host_capacity.host_work_dir(root),
            label=f"consumer gate ({root.name}, {len(run_now)} test files)",
            landing=False,
            say=lambda message: print(message, flush=True),
        ) as admission:
            if admission.waited_seconds >= 1:
                print(
                    f"consumer-gate: admitted after {admission.waited_seconds:.0f} s",
                    flush=True,
                )
            print("consumer-gate: " + " ".join(argv), flush=True)
            # The importer tests are the holder's job: a test that runs a
            # consumer gate in a child is admitted inside this slot.
            env = {**os.environ, **host_capacity.held_slot_environment()}
            failed = _COMMANDS.run(argv, cwd=root, env=env).returncode != 0
        if failed:
            print("consumer-gate: importer tests failed; no receipt", file=sys.stderr)
            return 1
    if commit_tree(root, paths) != tree:
        print("consumer-gate: the change moved while its tests ran", file=sys.stderr)
        return 1
    path = receipt_path(root, tree)
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "schema": SCHEMA,
        "tree": tree,
        "changed": changed,
        "static_gates": statics,
        "importers": run_now,
        "landing_built_importers": {
            test: list(deps) for test, deps in landing_built.items()
        },
        "data_readers": list(readers.data_readers),
    }
    path.write_text(json.dumps(payload, indent=1) + "\n", encoding="utf-8")
    print(f"consumer-gate: receipt {path}")
    return 0


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
    parser.add_argument("paths", nargs="*", help="the paths the commit names")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    if args.mode == "run":
        return run(root, args.paths, pytest_args=args.pytest_arg)
    return check(root, args.paths)


if __name__ == "__main__":
    raise SystemExit(main())
