#!/usr/bin/env python3
"""Cargo target runner that isolates process-global resource-limit tests.

Cargo still owns compilation, target discovery, environment projection, and
the complete ``--no-fail-fast`` workspace traversal. This runner changes only
the execution custody of the resource-enforcement integration binary: each
test receives a fresh process, so address-space limits and global allocators
cannot leak into sibling tests or hide later failures behind SIGABRT/ENOMEM.
"""

from __future__ import annotations

import sys
from pathlib import Path
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


RESOURCE_TEST_TARGET = "resource_enforcement"


def is_resource_test_binary(executable: str) -> bool:
    stem = Path(executable).stem
    return stem == RESOURCE_TEST_TARGET or stem.startswith(f"{RESOURCE_TEST_TARGET}-")


def listed_tests(executable: str, inherited_args: list[str]) -> list[str]:
    process = _COMMANDS.run(
        [executable, "--list", "--format", "terse", *inherited_args],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if process.stdout:
        print(process.stdout, end="")
    if process.stderr:
        print(process.stderr, end="", file=sys.stderr)
    if process.returncode != 0:
        raise RuntimeError(
            f"resource test discovery failed with exit code {process.returncode}"
        )
    tests = []
    for line in process.stdout.splitlines():
        identity, separator, kind = line.rpartition(": ")
        if separator and kind == "test" and identity:
            tests.append(identity)
    if not tests:
        raise RuntimeError("resource test discovery returned zero tests")
    return tests


def run_resource_tests(executable: str, inherited_args: list[str]) -> int:
    failed = False
    for identity in listed_tests(executable, inherited_args):
        process = _COMMANDS.run(
            [
                executable,
                "--exact",
                identity,
                "--test-threads=1",
                *inherited_args,
            ],
            check=False,
        )
        if process.returncode != 0:
            # A signal/abort can terminate the test harness before it prints a
            # normal FAILED row. Emit the exact identity so the parent truth
            # receipt never degrades this into an unattributable process red.
            print(f"test {identity} ... FAILED")
            print(
                f"isolated resource test process exited with {process.returncode}: "
                f"{identity}"
            )
            failed = True
    return 1 if failed else 0


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if not args:
        print("cargo test binary runner requires an executable", file=sys.stderr)
        return 2
    executable, *inherited_args = args
    if is_resource_test_binary(executable):
        try:
            return run_resource_tests(executable, inherited_args)
        except (OSError, RuntimeError) as exc:
            print(f"cargo-test-binary-runner: {exc}", file=sys.stderr)
            return 2
    try:
        return _COMMANDS.run(args, check=False).returncode
    except OSError as exc:
        print(f"cargo-test-binary-runner: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
