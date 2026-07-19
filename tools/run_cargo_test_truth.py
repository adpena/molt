#!/usr/bin/env python3
"""Run Cargo's canonical workspace tests and enforce the exact known-red set."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import check_suite_honesty

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

ROOT = Path(__file__).resolve().parents[1]
RECEIPT = ROOT / "proof-receipts" / "evidence" / "cargo-test-truth.json"
TARGET_RUNNER = ROOT / "tools" / "cargo_test_binary_runner.py"
LOCKED_WORKSPACES = (ROOT / "Cargo.toml", ROOT / "runtime" / "Cargo.toml")
CANONICAL_COMMAND = (
    "cargo",
    "test",
    "--locked",
    "--workspace",
    "--tests",
    "--no-fail-fast",
)


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
            rows[identity] = {
                "identity": identity,
                "status": "pass",
                "context": context,
            }
        elif status == "FAILED":
            rows[identity] = {
                "identity": identity,
                "status": "fail",
                "context": context,
            }
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


def host_target() -> str:
    process = _COMMANDS.run(
        ["rustc", "-vV"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if process.returncode != 0:
        raise RuntimeError(process.stderr.strip() or "rustc -vV failed")
    for line in process.stdout.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ").strip()
    raise RuntimeError("rustc -vV did not report a host target")


def target_runner_config(target: str) -> str:
    argv = [sys.executable, str(TARGET_RUNNER)]
    encoded = ",".join(json.dumps(item) for item in argv)
    return f"target.{target}.runner=[{encoded}]"


def run_streamed(command: tuple[str, ...]) -> tuple[int, str]:
    process = _COMMANDS.start_guarded(
        command,
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
    return process.wait(), "".join(captured)


def write_receipt(payload: dict) -> None:
    RECEIPT.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=RECEIPT.parent,
        prefix=f".{RECEIPT.name}.",
        suffix=".tmp",
        delete=False,
    ) as handle:
        handle.write(encoded)
        temporary = Path(handle.name)
    os.replace(temporary, RECEIPT)


def main() -> int:
    started = datetime.now(timezone.utc)
    context = host_context()
    phases: list[dict] = []
    combined: list[str] = []
    failed = False
    for manifest in LOCKED_WORKSPACES:
        command = (
            "cargo",
            "fetch",
            "--locked",
            "--manifest-path",
            str(manifest),
        )
        returncode, output = run_streamed(command)
        combined.append(output)
        phases.append(
            {
                "kind": "dependency-prefetch",
                "manifest": str(manifest.relative_to(ROOT)).replace("\\", "/"),
                "argv": list(command),
                "returncode": returncode,
            }
        )
        if returncode != 0:
            failed = True
            break

    target = ""
    if not failed:
        try:
            target = host_target()
            command = (
                *CANONICAL_COMMAND,
                "--config",
                target_runner_config(target),
            )
            returncode, output = run_streamed(command)
        except RuntimeError as exc:
            returncode = 2
            output = f"cargo-test-truth-runner: {exc}\n"
            print(output, end="", file=sys.stderr)
        combined.append(output)
        phases.append(
            {
                "kind": "workspace-test",
                "host_target": target,
                "argv": list(command) if target else list(CANONICAL_COMMAND),
                "returncode": returncode,
            }
        )
        failed = returncode != 0

    output = "".join(combined)
    rows = parse_test_results(output, context)
    failures = sorted(row["identity"] for row in rows if row.get("status") == "fail")
    problems = verdict(output, 1 if failed else 0, context)
    finished = datetime.now(timezone.utc)
    receipt = {
        "schema": "molt.cargo-test-truth.v1",
        "started_at": started.isoformat(),
        "finished_at": finished.isoformat(),
        "duration_seconds": round((finished - started).total_seconds(), 3),
        "context": context,
        "status": "success" if not problems else "failed",
        "phases": phases,
        "observed_test_count": len(rows),
        "failed_tests": failures,
        "problems": problems,
    }
    write_receipt(receipt)
    print(f"cargo-test-truth-runner: receipt={RECEIPT}")
    if problems:
        print("cargo-test-truth-runner: FAIL", file=sys.stderr)
        for problem in problems:
            print(f"- {problem}", file=sys.stderr)
        return 1
    print("cargo-test-truth-runner: OK (exact registered red set)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
