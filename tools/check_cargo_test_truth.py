#!/usr/bin/env python3
"""Fail closed when Cargo test binaries can be masked or silently unexecuted."""

from __future__ import annotations

import shlex
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PROOF_PLAN = ROOT / "tools" / "proof_plan.toml"
RUNNER = ROOT / "tools" / "run_cargo_test_truth.py"
_CANONICAL = "cargo test --locked --workspace --tests --no-fail-fast"
_RUNNER_ID = "rust.test.default-truth"
_RUNNER_ARGV = ["uv", "run", "--frozen", "python3", "tools/run_cargo_test_truth.py"]
_CARGO_EXECUTABLES = frozenset({"cargo", "cargo.exe"})
_COMPILE_SUBCOMMANDS = frozenset({"build", "check", "clippy", "test"})
# Each selects exactly one test executable when it appears once.
_SINGLE_TARGET_SELECTORS = frozenset({"--lib", "--doc", "--bin", "--test", "--bench"})
_MULTI_TARGET_SELECTORS = frozenset(
    {"--workspace", "--all", "--all-targets", "--tests", "--bins", "--benches"}
)


def _cargo_arguments(argv: Sequence[str]) -> tuple[str, list[str]] | None:
    """Return (subcommand, cargo's own arguments) for a Cargo compile invocation.

    Arguments after `--` belong to the compiled binaries, not to Cargo.
    """
    for index, token in enumerate(argv):
        if Path(token).name not in _CARGO_EXECUTABLES:
            continue
        rest = list(argv[index + 1 :])
        while rest and rest[0].startswith("+"):
            rest.pop(0)
        if not rest or rest[0] not in _COMPILE_SUBCOMMANDS:
            return None
        subcommand, arguments = rest[0], rest[1:]
        if "--" in arguments:
            arguments = arguments[: arguments.index("--")]
        return subcommand, arguments
    return None


def _cargo_invocations(
    plan: Mapping[str, object],
) -> Iterator[tuple[str, str, list[str]]]:
    """Yield (owner, subcommand, arguments) for every Cargo compile in the plan.

    Commands carry `argv` lists; local rules carry shell `gates` strings. The
    TOML is parsed, so comments and prose never count as commands.
    """
    for table_name, entries in plan.items():
        if not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            owner = f"{table_name} {entry.get('id') or entry.get('name')!r}"
            argv = entry.get("argv")
            if isinstance(argv, list):
                found = _cargo_arguments([str(item) for item in argv])
                if found is not None:
                    yield owner, *found
            for gate in entry.get("gates") or ():
                found = _cargo_arguments(shlex.split(str(gate)))
                if found is not None:
                    yield owner, *found


def _selects_one_executable(arguments: Sequence[str]) -> bool:
    if any(argument in _MULTI_TARGET_SELECTORS for argument in arguments):
        return False
    packages = sum(argument in {"-p", "--package"} for argument in arguments)
    selectors = sum(argument in _SINGLE_TARGET_SELECTORS for argument in arguments)
    return packages <= 1 and selectors == 1


def _display_path(path: Path) -> Path:
    try:
        return path.relative_to(ROOT)
    except ValueError:
        return path


def violations() -> list[str]:
    failures = []
    plan = tomllib.loads(PROOF_PLAN.read_text(encoding="utf-8"))
    runner_commands = [
        command
        for command in plan.get("command", [])
        if command.get("id") == _RUNNER_ID
    ]
    if len(runner_commands) != 1 or runner_commands[0].get("argv") != _RUNNER_ARGV:
        failures.append(
            f"{_display_path(PROOF_PLAN)} must contain exactly one {_RUNNER_ID!r} "
            f"command with argv {_RUNNER_ARGV!r}"
        )
    if (
        RUNNER.read_text(encoding="utf-8").count(
            '"test",\n    "--locked",\n    "--workspace",\n    "--tests",\n    "--no-fail-fast",'
        )
        != 1
    ):
        failures.append(
            f"{RUNNER.relative_to(ROOT)} must execute exactly {_CANONICAL!r}"
        )
    for owner, subcommand, arguments in _cargo_invocations(plan):
        command = " ".join(["cargo", subcommand, *arguments])
        if "--locked" not in arguments:
            failures.append(
                f"{_display_path(PROOF_PLAN)}: {owner}: Cargo compilation "
                f"command lacks --locked dependency authority: {command}"
            )
        if (
            subcommand == "test"
            and not _selects_one_executable(arguments)
            and "--no-fail-fast" not in arguments
        ):
            failures.append(
                f"{_display_path(PROOF_PLAN)}: {owner}: multi-executable Cargo "
                f"test command lacks --no-fail-fast: {command}"
            )
    return failures


def main() -> int:
    failures = violations()
    if failures:
        print("cargo-test-truth: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("cargo-test-truth: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
