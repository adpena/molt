#!/usr/bin/env python3
"""Fail when a pytest file has no CI owner beyond the recorded backlog.

CI runs only what the proof plan's commands name, so a test file that no
command names never runs in CI and can rot unseen. A file is owned when a plan
command's argv names it, one of its test nodes (``path::node``), or a
directory that contains it. The baseline records the files that had no owner
when this check landed; it may only shrink. A new test file must join a plan
command, and a file that gains an owner (or is deleted) must leave the baseline.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
BASELINE_NAME = "tools/test_ownership_baseline.json"
SCHEMA_VERSION = 1
# pyproject's addopts ignore these trees; their own runners own them.
EXCLUDED_ROOTS = ("tests/differential/", "tests/molt_only/", "tests/compliance/")


def test_files(root: Path) -> set[str]:
    files = set()
    for path in (root / "tests").rglob("test_*.py"):
        relative = path.relative_to(root).as_posix()
        if relative.startswith(EXCLUDED_ROOTS) or "/fixtures/" in relative:
            continue
        files.add(relative)
    return files


def plan_targets(root: Path) -> set[str]:
    """Every argv element of a plan command that names a path under tests/."""
    plan = tomllib.loads((root / "tools" / "proof_plan.toml").read_text("utf-8"))
    targets = set()
    for command in plan.get("command", []):
        for argument in command.get("argv", []):
            if isinstance(argument, str) and argument.startswith("tests"):
                targets.add(argument.split("::", 1)[0].rstrip("/"))
    return targets


def unowned(root: Path) -> set[str]:
    targets = plan_targets(root)
    result = set()
    for file in test_files(root):
        parents = {str(parent) for parent in PurePosixPath(file).parents}
        if file not in targets and not parents & targets:
            result.add(file)
    return result


def read_baseline(root: Path) -> set[str] | None:
    path = root / BASELINE_NAME
    if not path.exists():
        return None
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("schema") != SCHEMA_VERSION:
        raise SystemExit(
            f"{BASELINE_NAME}: unsupported schema {payload.get('schema')!r}"
        )
    return set(payload["unowned"])


def write_baseline(root: Path, files: set[str]) -> None:
    payload = {"schema": SCHEMA_VERSION, "unowned": sorted(files)}
    (root / BASELINE_NAME).write_text(
        json.dumps(payload, indent=1) + "\n", encoding="utf-8"
    )


def check(root: Path) -> list[str]:
    current = unowned(root)
    baseline = read_baseline(root)
    if baseline is None:
        return [f"{BASELINE_NAME} is missing; create it with --update"]
    errors = [
        f"{file}: no proof-plan command runs this test file; add it to the "
        "command that owns its subject in tools/proof_plan.toml"
        for file in sorted(current - baseline)
    ]
    errors.extend(
        f"{file}: now owned or deleted; remove it from {BASELINE_NAME} with --update"
        for file in sorted(baseline - current)
    )
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="the default")
    mode.add_argument(
        "--update",
        action="store_true",
        help="shrink the baseline to the current backlog; never grows it",
    )
    args = parser.parse_args(argv)
    root = args.root.resolve()
    current = unowned(root)
    if args.update:
        baseline = read_baseline(root)
        added = current - baseline if baseline is not None else set()
        if added:
            for file in sorted(added):
                print(
                    f"test-ownership: {file} has no owner; wire it, do not baseline it",
                    file=sys.stderr,
                )
            return 1
        write_baseline(root, current)
        print(f"test-ownership: baseline holds {len(current)} unowned test files")
        return 0
    errors = check(root)
    for error in errors:
        print(f"test-ownership: {error}", file=sys.stderr)
    if errors:
        return 1
    print(
        f"test-ownership: OK ({len(test_files(root)) - len(current)} owned, "
        f"{len(current)} in the recorded backlog)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
