#!/usr/bin/env python3
"""Run a change's first-hop importer tests before the change is committed.

``run -- PATHS`` traces the change with ``tools/test_dependency_graph.py``
(first-hop importers and data readers), prints both lists for review, runs the
importer test files with this checkout's interpreter, and records a receipt
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
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

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


def run(root: Path, paths: list[str], *, pytest_args: list[str]) -> int:
    from tools.test_dependency_graph import load_graph

    tree = commit_tree(root, paths)
    changed = changed_paths(root, tree)
    readers = load_graph(root).direct_readers(changed)
    print(f"consumer-gate: {len(changed)} changed path(s), tree {tree[:12]}")
    for label, tests in (
        ("importers (run now)", readers.importers),
        ("data readers (review; postsubmit runs them)", readers.data_readers),
    ):
        print(f"# {label}: {len(tests)}")
        for test in tests:
            print(f"  {test}")
    if readers.importers:
        argv = [sys.executable, "-m", "pytest", "-q", *pytest_args]
        argv += list(readers.importers)
        print("consumer-gate: " + " ".join(argv), flush=True)
        if _COMMANDS.run(argv, cwd=root).returncode != 0:
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
        "importers": list(readers.importers),
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
        f"  uv run --python 3.12 python tools/consumer_gate.py run -- {shown}",
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
        default=[],
        help="extra pytest argument for run (repeatable), e.g. -n=auto",
    )
    parser.add_argument("paths", nargs="*", help="the paths the commit names")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    if args.mode == "run":
        return run(root, args.paths, pytest_args=args.pytest_arg)
    return check(root, args.paths)


if __name__ == "__main__":
    raise SystemExit(main())
