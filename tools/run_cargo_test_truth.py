#!/usr/bin/env python3
"""Run Cargo's canonical workspace tests and enforce the exact known-red set."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import check_suite_honesty

ROOT = Path(__file__).resolve().parents[1]
CANONICAL_COMMAND = ("cargo", "test", "--workspace", "--tests", "--no-fail-fast")


def host_context() -> dict[str, str]:
    platform = {"win32": "windows", "darwin": "macos"}.get(sys.platform, "linux")
    return {"platform": platform, "target": "default"}


def parse_test_results(output: str, context: dict[str, str]) -> list[dict]:
    rows: dict[str, dict] = {}
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line.startswith("test ") or " ... " not in line:
            continue
        identity, status = line[5:].rsplit(" ... ", 1)
        identity = identity.removesuffix(" - should panic")
        if status == "ok":
            rows[identity] = {"identity": identity, "status": "pass", "context": context}
        elif status == "FAILED":
            rows[identity] = {"identity": identity, "status": "fail", "context": context}
    return list(rows.values())


def verdict(output: str, returncode: int, context: dict[str, str]) -> list[str]:
    data = check_suite_honesty.load_manifest()
    problems = check_suite_honesty.validate_manifest(
        data, check_suite_honesty.load_too_dynamic_set()
    )
    rows = parse_test_results(output, context)
    problems += check_suite_honesty.execution_reality_check(data, rows)
    if returncode != 0 and not any(row["status"] == "fail" for row in rows):
        problems.append(
            "canonical Cargo truth command failed without an attributable test identity "
            "(compile/link/process failure cannot be registered as a test red)"
        )
    if "could not compile" in output or "error[" in output:
        problems.append("canonical Cargo truth command contained a compiler error")
    return problems


def main() -> int:
    process = subprocess.Popen(
        CANONICAL_COMMAND,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
    )
    captured: list[str] = []
    assert process.stdout is not None
    for line in process.stdout:
        print(line, end="")
        captured.append(line)
    returncode = process.wait()
    problems = verdict("".join(captured), returncode, host_context())
    if problems:
        print("cargo-test-truth-runner: FAIL", file=sys.stderr)
        for problem in problems:
            print(f"- {problem}", file=sys.stderr)
        return 1
    print("cargo-test-truth-runner: OK (exact registered red set)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
