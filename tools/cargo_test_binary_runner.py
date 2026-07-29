#!/usr/bin/env python3
"""Bounded, receipt-producing custody for every Cargo test binary.

Cargo remains the compilation and target-discovery authority.  This runner
owns the executable boundary: every binary has one wall-clock budget, captured
stdout/stderr, process-tree memory custody, exact termination metadata, and an
atomic receipt.  An abnormal libtest exit is reduced deterministically to an
exact test (or an explicit interaction set) instead of surfacing as Cargo's
unattributable wrapper status.

The resource-enforcement integration binary keeps its declared semantic
isolation: each test runs in a fresh process because those tests intentionally
mutate process-global address-space and allocator policy.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from collections.abc import Collection
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

RESOURCE_TEST_TARGET = "resource_enforcement"
MAX_DIAGNOSTIC_EXECUTIONS = 24
MAX_EXACT_TEST_TIMEOUT_SECONDS = 30.0
# CreateProcessW accepts at most 32,767 UTF-16 code units for the complete
# command line.  Leave room for quoting expansion and the parent environment's
# executable prefix instead of discovering the limit as an unattributed
# WinError 206 after the original test binary already failed.
MAX_DIAGNOSTIC_COMMAND_CHARS = 30_000
RECEIPT_TAIL_BYTES = 16_384
_ACTIVE_EVIDENCE_DIR: Path | None = None
_FALLBACK_EVIDENCE_TEMP: tempfile.TemporaryDirectory[str] | None = None
_TEST_RESULT_RE = re.compile(r"^test (.+?) \.\.\. (ok|FAILED)(?:\s|$)", re.MULTILINE)
_STARTED_TEST_RE = re.compile(r"^test (.+?) \.\.\.\s*$", re.MULTILINE)
_WINDOWS_EXCEPTION_NAMES = {
    0x40000015: "STATUS_FATAL_APP_EXIT",
    0xC0000005: "STATUS_ACCESS_VIOLATION",
    0xC000001D: "STATUS_ILLEGAL_INSTRUCTION",
    0xC0000094: "STATUS_INTEGER_DIVIDE_BY_ZERO",
    0xC00000FD: "STATUS_STACK_OVERFLOW",
    0xC0000374: "STATUS_HEAP_CORRUPTION",
    0xC0000409: "STATUS_STACK_BUFFER_OVERRUN_OR_FAST_FAIL",
    0xC0000602: "STATUS_FAIL_FAST_EXCEPTION",
}
_POSIX_SIGNAL_NAMES = {
    6: "SIGABRT",
    9: "SIGKILL",
    11: "SIGSEGV",
    15: "SIGTERM",
}


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def termination_payload(returncode: int, *, timed_out: bool) -> dict[str, object]:
    if timed_out:
        return {"kind": "timeout", "returncode": returncode}
    if returncode < 0:
        number = -returncode
        name = _POSIX_SIGNAL_NAMES.get(number)
        if name is None:
            try:
                name = signal.Signals(number).name
            except ValueError:
                name = f"signal-{number}"
        return {
            "kind": "signal",
            "returncode": returncode,
            "signal": number,
            "name": name,
        }
    unsigned = returncode & 0xFFFFFFFF
    if unsigned in _WINDOWS_EXCEPTION_NAMES or unsigned & 0xC0000000 == 0xC0000000:
        return {
            "kind": "windows-exception",
            "returncode": returncode,
            "code": f"0x{unsigned:08X}",
            "raw_code": unsigned,
            "name": _WINDOWS_EXCEPTION_NAMES.get(
                unsigned, f"NTSTATUS_0x{unsigned:08X}"
            ),
            "severity": "error",
            "facility": (unsigned >> 16) & 0xFFF,
        }
    return {"kind": "exit", "returncode": returncode}


def _rss_kb(record: object | None) -> int | None:
    value = getattr(record, "rss_kb", None)
    return value if isinstance(value, int) else None


@dataclass(frozen=True, slots=True)
class BinaryExecution:
    argv: tuple[str, ...]
    returncode: int
    stdout: str
    stderr: str
    elapsed_seconds: float
    timed_out: bool
    peak_process_rss_kb: int | None
    peak_tree_rss_kb: int | None
    stdout_evidence: Path | None = None
    stderr_evidence: Path | None = None

    @property
    def succeeded(self) -> bool:
        return not self.timed_out and self.returncode == 0

    @property
    def termination(self) -> dict[str, object]:
        return termination_payload(self.returncode, timed_out=self.timed_out)

    def receipt(self) -> dict[str, object]:
        stdout_bytes, stdout_sha256 = _stream_identity(self.stdout_evidence, self.stdout)
        stderr_bytes, stderr_sha256 = _stream_identity(self.stderr_evidence, self.stderr)
        payload: dict[str, object] = {
            "argv": list(self.argv),
            "returncode": self.returncode,
            "termination": self.termination,
            "timed_out": self.timed_out,
            "elapsed_seconds": round(self.elapsed_seconds, 6),
            "peak_process_rss_kb": self.peak_process_rss_kb,
            "peak_tree_rss_kb": self.peak_tree_rss_kb,
            "stdout_bytes": stdout_bytes,
            "stderr_bytes": stderr_bytes,
            "stdout_sha256": stdout_sha256,
            "stderr_sha256": stderr_sha256,
            "stdout_evidence": None if self.stdout_evidence is None else str(self.stdout_evidence),
            "stderr_evidence": None if self.stderr_evidence is None else str(self.stderr_evidence),
            "stdout_tail": self.stdout[-RECEIPT_TAIL_BYTES:],
            "stderr_tail": self.stderr[-RECEIPT_TAIL_BYTES:],
        }
        return payload


def _file_identity(path: Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    return size, digest.hexdigest()


def _executable_identity(executable: str) -> tuple[str, int | None, str | None]:
    path = Path(executable).resolve()
    if not path.is_file():
        return str(path), None, None
    size, digest = _file_identity(path)
    return str(path), size, digest


def _stream_identity(path: Path | None, tail: str) -> tuple[int, str]:
    if path is not None:
        return _file_identity(path)
    encoded = tail.encode("utf-8")
    return len(encoded), hashlib.sha256(encoded).hexdigest()


def _publish_fallback_evidence(path: Path, text: str) -> None:
    with path.open("x", encoding="utf-8") as handle:
        handle.write(text)


def _execution_lines(execution: BinaryExecution):
    for path, fallback in (
        (execution.stdout_evidence, execution.stdout),
        (execution.stderr_evidence, execution.stderr),
    ):
        if path is None:
            yield from fallback.splitlines(keepends=True)
        else:
            with path.open("r", encoding="utf-8", errors="replace") as handle:
                yield from handle


def execute_binary(argv: list[str], timeout_seconds: float) -> BinaryExecution:
    global _FALLBACK_EVIDENCE_TEMP
    evidence_dir = _ACTIVE_EVIDENCE_DIR
    if evidence_dir is None:
        if _FALLBACK_EVIDENCE_TEMP is None:
            _FALLBACK_EVIDENCE_TEMP = tempfile.TemporaryDirectory(
                prefix="molt-cargo-test-binary-evidence-"
            )
        evidence_dir = Path(_FALLBACK_EVIDENCE_TEMP.name)
    evidence_id = uuid.uuid4().hex
    stdout_evidence = evidence_dir / f"{evidence_id}.stdout.log"
    stderr_evidence = evidence_dir / f"{evidence_id}.stderr.log"
    started = time.monotonic()
    try:
        process = _COMMANDS.run(
            argv,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout_seconds,
            stdout_capture_path=stdout_evidence,
            stderr_capture_path=stderr_evidence,
            capture_tail_bytes=RECEIPT_TAIL_BYTES,
        )
    except subprocess.TimeoutExpired as exc:
        guarded_result = getattr(exc, "guarded_result", None)
        stdout_text = _text(exc.output)
        stderr_text = _text(exc.stderr)
        if not stdout_evidence.exists():
            _publish_fallback_evidence(stdout_evidence, stdout_text)
        if not stderr_evidence.exists():
            _publish_fallback_evidence(stderr_evidence, stderr_text)
        return BinaryExecution(
            argv=tuple(argv),
            returncode=124,
            stdout=stdout_text,
            stderr=stderr_text,
            elapsed_seconds=float(
                getattr(guarded_result, "elapsed_s", None)
                or (time.monotonic() - started)
            ),
            timed_out=True,
            peak_process_rss_kb=_rss_kb(getattr(guarded_result, "peak", None)),
            peak_tree_rss_kb=_rss_kb(getattr(guarded_result, "peak_total", None)),
            stdout_evidence=stdout_evidence,
            stderr_evidence=stderr_evidence,
        )
    stdout_text = _text(getattr(process, "stdout", None))
    stderr_text = _text(getattr(process, "stderr", None))
    if not stdout_evidence.exists():
        _publish_fallback_evidence(stdout_evidence, stdout_text)
    if not stderr_evidence.exists():
        _publish_fallback_evidence(stderr_evidence, stderr_text)
    return BinaryExecution(
        argv=tuple(argv),
        returncode=int(process.returncode),
        stdout=stdout_text,
        stderr=stderr_text,
        elapsed_seconds=float(
            getattr(process, "elapsed_s", None) or (time.monotonic() - started)
        ),
        timed_out=bool(getattr(process, "timed_out", False)),
        peak_process_rss_kb=_rss_kb(getattr(process, "peak", None)),
        peak_tree_rss_kb=_rss_kb(getattr(process, "peak_total", None)),
        stdout_evidence=stdout_evidence,
        stderr_evidence=stderr_evidence,
    )


def _emit_output(execution: BinaryExecution) -> None:
    if execution.stdout:
        print(execution.stdout, end="")
    if execution.stderr:
        print(execution.stderr, end="", file=sys.stderr)


def is_resource_test_binary(executable: str) -> bool:
    stem = Path(executable).stem
    return stem == RESOURCE_TEST_TARGET or stem.startswith(f"{RESOURCE_TEST_TARGET}-")


def _remaining(deadline: float) -> float:
    return max(0.0, deadline - time.monotonic())


def _diagnostic_reserve(total_timeout: float) -> float:
    """Reserve bounded attribution time inside the one binary deadline."""
    return min(60.0, max(1.0, total_timeout / 5.0))


def _baseline_timeout(total_timeout: float, deadline: float) -> float:
    remaining = _remaining(deadline)
    reserve = min(remaining / 2.0, _diagnostic_reserve(total_timeout))
    return remaining - reserve


def _diagnostic_timeout(total_timeout: float, deadline: float) -> float:
    # A healthy runtime libtest completes in seconds. Short diagnostic slices
    # preserve enough of the one deadline for logarithmic reduction of a hang
    # instead of spending the entire reserve on the first wedged partition.
    return min(_remaining(deadline), max(1.0, min(5.0, total_timeout / 20.0)))


def _exact_test_timeout(total_timeout: float, deadline: float) -> float:
    """Return a defensible full exact-test budget or zero when unavailable."""
    required = min(
        MAX_EXACT_TEST_TIMEOUT_SECONDS,
        max(10.0, total_timeout / 10.0),
    )
    remaining = _remaining(deadline)
    return required if remaining >= required else 0.0


def _test_results(output: str, status: str) -> list[str]:
    return [
        identity
        for identity, found in _TEST_RESULT_RE.findall(output)
        if found == status
    ]


def _structured_test_results(output: str) -> list[dict[str, str]]:
    return [
        {
            "identity": identity,
            "status": "pass" if status == "ok" else "fail",
        }
        for identity, status in _TEST_RESULT_RE.findall(output)
    ]


def _test_results_for_execution(execution: BinaryExecution, status: str) -> list[str]:
    rows: list[str] = []
    for line in _execution_lines(execution):
        rows.extend(
            identity
            for identity, found in _TEST_RESULT_RE.findall(line)
            if found == status
        )
    return rows


def _structured_results_for_execution(
    execution: BinaryExecution,
) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for line in _execution_lines(execution):
        rows.extend(_structured_test_results(line))
    return rows


def _started_tests_for_execution(execution: BinaryExecution) -> list[str]:
    rows: list[str] = []
    for line in _execution_lines(execution):
        rows.extend(_STARTED_TEST_RE.findall(line))
    return rows


def _confirmed_failure_identities(
    reported_failures: list[str], diagnosis: dict[str, object] | None
) -> set[str]:
    confirmed = set(reported_failures)
    if diagnosis is None:
        return confirmed
    if diagnosis.get("kind") in {"isolated-test", "reported-test-failure"}:
        identity = diagnosis.get("identity")
        if isinstance(identity, str):
            confirmed.add(identity)
    return confirmed


def _canonical_test_results(
    executions: list[BinaryExecution], *, resource_isolation: bool
) -> list[dict[str, str]]:
    if not executions:
        return []
    if not resource_isolation:
        return _structured_results_for_execution(executions[0])
    results: list[dict[str, str]] = []
    for execution in executions[1:]:
        try:
            exact_index = execution.argv.index("--exact")
            identity = execution.argv[exact_index + 1]
        except (ValueError, IndexError):
            continue
        if execution.succeeded:
            results.append({"identity": identity, "status": "pass"})
        elif _exact_reproduction_kind(execution, identity) is not None:
            results.append({"identity": identity, "status": "fail"})
    return results


def listed_tests(
    executable: str,
    inherited_args: list[str],
    *,
    timeout_seconds: float,
) -> tuple[list[str], BinaryExecution]:
    process = execute_binary(
        [
            executable,
            *_canonical_list_args(inherited_args),
            "--list",
            "--format",
            "terse",
        ],
        timeout_seconds,
    )
    if not process.succeeded:
        raise RuntimeError(
            "test discovery failed: "
            f"termination={json.dumps(process.termination, sort_keys=True)}"
        )
    tests = []
    for line in process.stdout.splitlines():
        identity, separator, kind = line.rpartition(": ")
        if separator and kind == "test" and identity:
            tests.append(identity)
    if not tests:
        raise RuntimeError("test discovery returned zero tests")
    return tests, process


def _subset_argv(
    executable: str,
    inherited_args: list[str],
    all_tests: list[str],
    selected: list[str],
) -> list[str] | None:
    selected_set = set(selected)
    canonical_args = _canonical_diagnostic_args(
        inherited_args, preserve_selection_domain=True
    )
    argv = [executable, *canonical_args, "--nocapture"]
    for identity in all_tests:
        if identity not in selected_set:
            argv.extend(("--skip", identity))
    if len(subprocess.list2cmdline(argv)) > MAX_DIAGNOSTIC_COMMAND_CHARS:
        return None
    return argv


def _serial_argv(executable: str, inherited_args: list[str]) -> list[str]:
    return [
        executable,
        *_canonical_diagnostic_args(inherited_args, preserve_selection_domain=True),
        "--test-threads=1",
        "--nocapture",
    ]


def _exact_argv(
    executable: str,
    inherited_args: list[str],
    identity: str,
    *,
    allowed_tests: Collection[str],
) -> list[str]:
    if identity not in allowed_tests:
        raise RuntimeError(
            f"exact diagnostic identity escaped listed selection domain: {identity}"
        )
    return [
        executable,
        *_canonical_diagnostic_args(inherited_args, preserve_selection_domain=False),
        "--exact",
        identity,
        "--test-threads=1",
        "--nocapture",
    ]


@dataclass(frozen=True, slots=True)
class LibtestSelectionDomain:
    domain_args: tuple[str, ...]
    mode_args: tuple[str, ...]
    passthrough_args: tuple[str, ...]

    def diagnostic_args(self, *, preserve_domain: bool) -> list[str]:
        return [
            *self.passthrough_args,
            *self.mode_args,
            *(self.domain_args if preserve_domain else ()),
        ]

    def list_args(self) -> list[str]:
        without_format: list[str] = []
        index = 0
        passthrough = list(self.passthrough_args)
        while index < len(passthrough):
            if passthrough[index] == "--format":
                index += 2
                continue
            if passthrough[index].startswith("--format="):
                index += 1
                continue
            without_format.append(passthrough[index])
            index += 1
        return [*without_format, *self.mode_args, *self.domain_args]


def _parse_libtest_selection(inherited_args: list[str]) -> LibtestSelectionDomain:
    domain: list[str] = []
    modes: list[str] = []
    passthrough: list[str] = []
    value_options = {"--color", "--format", "--logfile"}
    execution_value_options = {"--shuffle-seed", "--test-threads"}
    execution_flags = {"--list", "--nocapture", "--shuffle"}
    mode_flags = {"--ignored", "--include-ignored", "--exclude-should-panic"}
    seen_modes: set[str] = set()
    index = 0
    while index < len(inherited_args):
        argument = inherited_args[index]
        if argument in execution_flags:
            index += 1
            continue
        if argument in mode_flags:
            if argument not in seen_modes:
                modes.append(argument)
                seen_modes.add(argument)
            index += 1
            continue
        if argument in execution_value_options:
            index += 2
            continue
        if any(
            argument.startswith(f"{option}=") for option in execution_value_options
        ):
            index += 1
            continue
        if argument == "--exact":
            domain.append(argument)
            index += 1
            continue
        if argument == "--skip":
            domain.append(argument)
            if index + 1 < len(inherited_args):
                domain.append(inherited_args[index + 1])
            index += 2
            continue
        if argument.startswith("--skip="):
            domain.append(argument)
            index += 1
            continue
        if argument in value_options:
            passthrough.append(argument)
            if index + 1 < len(inherited_args):
                passthrough.append(inherited_args[index + 1])
            index += 2
            continue
        if not argument.startswith("-"):
            domain.append(argument)
            index += 1
            continue
        passthrough.append(argument)
        index += 1
    return LibtestSelectionDomain(tuple(domain), tuple(modes), tuple(passthrough))


def _canonical_diagnostic_args(
    inherited_args: list[str], *, preserve_selection_domain: bool
) -> list[str]:
    return _parse_libtest_selection(inherited_args).diagnostic_args(
        preserve_domain=preserve_selection_domain
    )


def _canonical_list_args(inherited_args: list[str]) -> list[str]:
    """Preserve test selection while owning list/output execution controls."""
    return _parse_libtest_selection(inherited_args).list_args()


def _exact_reproduction_kind(
    exact: BinaryExecution, identity: str
) -> str | None:
    if exact.timed_out:
        return None
    if identity in _test_results_for_execution(exact, "FAILED"):
        return "reported-test-failure"
    started = _started_tests_for_execution(exact)
    if (
        identity in started
        and exact.termination.get("kind") in {"signal", "windows-exception"}
    ):
        return "isolated-test"
    return None


def _diagnose_serial_last_started(
    executable: str,
    inherited_args: list[str],
    candidates: list[str],
    *,
    total_timeout_seconds: float,
    deadline: float,
    executions: list[BinaryExecution],
) -> dict[str, object]:
    timeout = _diagnostic_timeout(total_timeout_seconds, deadline)
    if timeout <= 0 or len(executions) >= MAX_DIAGNOSTIC_EXECUTIONS:
        return {"kind": "budget-exhausted", "candidate_tests": candidates}
    serial = execute_binary(_serial_argv(executable, inherited_args), timeout)
    executions.append(serial)
    failed = _test_results_for_execution(serial, "FAILED")
    started = _started_tests_for_execution(serial)
    identity = failed[0] if failed else (started[-1] if started else None)
    if identity is None:
        return {
            "kind": (
                "parallel-or-order-interaction"
                if serial.succeeded
                else "unattributed-serial-abnormal-exit"
            ),
            "candidate_tests": candidates,
            "serial_termination": serial.termination,
        }

    exact: BinaryExecution | None = None
    timeout = _exact_test_timeout(total_timeout_seconds, deadline)
    if timeout > 0 and len(executions) < MAX_DIAGNOSTIC_EXECUTIONS:
        exact = execute_binary(
            _exact_argv(
                executable,
                inherited_args,
                identity,
                allowed_tests=candidates,
            ),
            timeout,
        )
        executions.append(exact)
    if exact is None:
        kind = "budget-exhausted"
    elif exact.succeeded:
        kind = "prior-state-interaction"
    elif exact.timed_out:
        kind = "exact-timeout"
    else:
        kind = _exact_reproduction_kind(exact, identity) or "exact-runner-failure"
    return {
        "kind": kind,
        "identity": identity,
        "candidate_tests": candidates,
        "serial_termination": serial.termination,
        "exact_termination": None if exact is None else exact.termination,
    }


def diagnose_abnormal_exit(
    executable: str,
    inherited_args: list[str],
    *,
    total_timeout_seconds: float,
    deadline: float,
) -> tuple[dict[str, object], list[BinaryExecution]]:
    executions: list[BinaryExecution] = []
    timeout = _diagnostic_timeout(total_timeout_seconds, deadline)
    if timeout <= 0:
        return {"kind": "budget-exhausted", "candidate_tests": []}, executions
    try:
        tests, discovery = listed_tests(
            executable,
            inherited_args,
            timeout_seconds=timeout,
        )
    except RuntimeError as exc:
        return {"kind": "discovery-failed", "error": str(exc)}, executions
    executions.append(discovery)
    candidates = tests

    while len(candidates) > 1 and len(executions) < MAX_DIAGNOSTIC_EXECUTIONS:
        midpoint = (len(candidates) + 1) // 2
        partitions = (candidates[:midpoint], candidates[midpoint:])
        failed_partition: list[str] | None = None
        for partition in partitions:
            if not partition or len(executions) >= MAX_DIAGNOSTIC_EXECUTIONS:
                continue
            timeout = _diagnostic_timeout(total_timeout_seconds, deadline)
            if timeout <= 0:
                return {
                    "kind": "budget-exhausted",
                    "candidate_tests": candidates,
                }, executions
            subset_argv = _subset_argv(executable, inherited_args, tests, partition)
            if subset_argv is None:
                return _diagnose_serial_last_started(
                    executable,
                    inherited_args,
                    candidates,
                    total_timeout_seconds=total_timeout_seconds,
                    deadline=deadline,
                    executions=executions,
                ), executions
            execution = execute_binary(subset_argv, timeout)
            executions.append(execution)
            print(
                "cargo-test-binary-runner: diagnostic "
                f"candidate_count={len(partition)} "
                f"termination={json.dumps(execution.termination, sort_keys=True)}"
            )
            failed = _test_results_for_execution(execution, "FAILED")
            if failed:
                return {
                    "kind": "reported-test-failure",
                    "identity": failed[0],
                    "candidate_tests": partition,
                }, executions
            if execution.timed_out:
                return {
                    "kind": "diagnostic-timeout",
                    "candidate_tests": partition,
                    "termination": execution.termination,
                    "note": (
                        "bounded partition slice timed out; attribution remains "
                        "unknown until an exact test reproduces within its full budget"
                    ),
                }, executions
            if not execution.succeeded:
                failed_partition = partition
                break
        if failed_partition is None:
            timeout = _diagnostic_timeout(total_timeout_seconds, deadline)
            if timeout <= 0:
                return {
                    "kind": "budget-exhausted",
                    "candidate_tests": candidates,
                }, executions
            return _diagnose_serial_last_started(
                executable,
                inherited_args,
                candidates,
                total_timeout_seconds=total_timeout_seconds,
                deadline=deadline,
                executions=executions,
            ), executions
        candidates = failed_partition

    if len(candidates) != 1:
        return {
            "kind": "diagnostic-execution-limit",
            "candidate_tests": candidates,
        }, executions

    identity = candidates[0]
    timeout = _exact_test_timeout(total_timeout_seconds, deadline)
    exact: BinaryExecution | None = None
    if timeout > 0 and len(executions) < MAX_DIAGNOSTIC_EXECUTIONS:
        exact = execute_binary(
            _exact_argv(
                executable,
                inherited_args,
                identity,
                allowed_tests=tests,
            ),
            timeout,
        )
        executions.append(exact)
    if exact is None:
        kind = "budget-exhausted"
    elif exact.succeeded:
        kind = "prior-state-interaction"
    elif exact.timed_out:
        kind = "exact-timeout"
    else:
        kind = _exact_reproduction_kind(exact, identity) or "exact-runner-failure"
    return {
        "kind": kind,
        "identity": identity,
        "candidate_tests": candidates,
        "exact_termination": None if exact is None else exact.termination,
    }, executions


def run_resource_tests(
    executable: str,
    inherited_args: list[str],
    *,
    total_timeout_seconds: float,
    deadline: float,
) -> tuple[int, dict[str, object], list[BinaryExecution]]:
    timeout = _diagnostic_timeout(total_timeout_seconds, deadline)
    tests, discovery = listed_tests(
        executable,
        inherited_args,
        timeout_seconds=timeout,
    )
    executions = [discovery]
    failed: list[str] = []
    structural: list[dict[str, object]] = []
    for identity in tests:
        remaining = _remaining(deadline)
        if remaining <= 0:
            return (
                1,
                {
                    "kind": "resource-isolation-timeout",
                    "failed_tests": failed,
                    "unexecuted_tests": tests[len(executions) - 1 :],
                },
                executions,
            )
        process = execute_binary(
            _exact_argv(
                executable,
                inherited_args,
                identity,
                allowed_tests=tests,
            ),
            remaining,
        )
        executions.append(process)
        _emit_output(process)
        reproduction_kind = _exact_reproduction_kind(process, identity)
        if reproduction_kind is not None:
            print(f"test {identity} ... FAILED")
            print(
                "isolated resource test process failed: "
                f"identity={identity} "
                f"termination={json.dumps(process.termination, sort_keys=True)}"
            )
            failed.append(identity)
        elif not process.succeeded:
            structural.append(
                {
                    "identity": identity,
                    "termination": process.termination,
                }
            )
    return (
        (1 if failed or structural else 0),
        {
            "kind": "resource-process-isolation",
            "failed_tests": failed,
            "structural_failures": structural,
        },
        executions,
    )


def _receipt_path(receipt_dir: Path, executable: str, invocation_id: str) -> Path:
    identity = hashlib.sha256(str(Path(executable).resolve()).encode()).hexdigest()[:16]
    stem = re.sub(r"[^A-Za-z0-9_.-]+", "-", Path(executable).stem).strip("-")
    return receipt_dir / f"{stem}-{identity}-{invocation_id}.json"


def write_receipt(path: Path, payload: dict[str, object]) -> None:
    """Publish one immutable receipt without replacing prior evidence."""
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".tmp",
        delete=False,
    ) as handle:
        handle.write(encoded)
        handle.flush()
        os.fsync(handle.fileno())
        temporary = Path(handle.name)
    try:
        # Linking the fully-written temporary file is an atomic create-if-absent
        # on the canonical NTFS/POSIX proof roots. A collision preserves the
        # existing invocation instead of overwriting crash evidence.
        os.link(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout-seconds", type=float, required=True)
    parser.add_argument("--receipt-dir", type=Path, required=True)
    parser.add_argument("--run-id")
    parser.add_argument("--source-identity-json")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def main(argv: list[str] | None = None) -> int:
    global _ACTIVE_EVIDENCE_DIR
    args = _parser().parse_args(sys.argv[1:] if argv is None else argv)
    command = list(args.command)
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        print("cargo test binary runner requires an executable", file=sys.stderr)
        return 2
    if args.timeout_seconds <= 0:
        print("cargo test binary runner timeout must be positive", file=sys.stderr)
        return 2
    source_identity: dict[str, object] | None = None
    if args.source_identity_json is not None:
        try:
            decoded_identity = json.loads(args.source_identity_json)
        except json.JSONDecodeError as exc:
            print(f"invalid Cargo truth source identity: {exc}", file=sys.stderr)
            return 2
        if (
            not isinstance(decoded_identity, dict)
            or decoded_identity.get("schema") != "molt.git-source.v1"
        ):
            print("invalid Cargo truth source identity schema", file=sys.stderr)
            return 2
        source_identity = decoded_identity

    executable, *inherited_args = command
    executable_resolved, executable_size, executable_sha256 = _executable_identity(executable)
    invocation_id = uuid.uuid4().hex
    _ACTIVE_EVIDENCE_DIR = args.receipt_dir / "evidence" / invocation_id
    _ACTIVE_EVIDENCE_DIR.mkdir(parents=True, exist_ok=False)
    started_at = _utc_now()
    started = time.monotonic()
    deadline = started + args.timeout_seconds
    executions: list[BinaryExecution] = []
    diagnosis: dict[str, object] | None = None
    reported_failures: list[str] = []
    baseline_timeout: float | None = None
    returncode = 2
    try:
        if is_resource_test_binary(executable):
            returncode, diagnosis, executions = run_resource_tests(
                executable,
                inherited_args,
                total_timeout_seconds=args.timeout_seconds,
                deadline=deadline,
            )
            failures = diagnosis.get("failed_tests")
            if isinstance(failures, list):
                reported_failures = [
                    identity for identity in failures if isinstance(identity, str)
                ]
        else:
            baseline_timeout = _baseline_timeout(args.timeout_seconds, deadline)
            baseline = execute_binary(command, baseline_timeout)
            executions.append(baseline)
            _emit_output(baseline)
            returncode = 0 if baseline.succeeded else 1
            reported_failures = _test_results_for_execution(baseline, "FAILED")
            if returncode != 0 and not reported_failures:
                diagnosis, diagnostic_executions = diagnose_abnormal_exit(
                    executable,
                    inherited_args,
                    total_timeout_seconds=args.timeout_seconds,
                    deadline=deadline,
                )
                executions.extend(diagnostic_executions)
                identity = diagnosis.get("identity")
                if (
                    diagnosis.get("kind")
                    in {"isolated-test", "reported-test-failure"}
                    and isinstance(identity, str)
                ):
                    print(f"test {identity} ... FAILED")
                print(
                    "cargo-test-binary-runner: abnormal-exit-diagnosis="
                    + json.dumps(diagnosis, sort_keys=True)
                )
    except (OSError, RuntimeError) as exc:
        diagnosis = {"kind": "runner-error", "error": str(exc)}
        print(f"cargo-test-binary-runner: {exc}", file=sys.stderr)
        returncode = 2

    failure_identities = _confirmed_failure_identities(reported_failures, diagnosis)
    resource_isolation = is_resource_test_binary(executable)
    receipt = {
        "schema": "molt.cargo-test-binary.v1",
        "run_id": args.run_id,
        "source_identity": source_identity,
        "invocation_id": invocation_id,
        "started_at": started_at,
        "finished_at": _utc_now(),
        "duration_seconds": round(time.monotonic() - started, 6),
        "timeout_seconds": args.timeout_seconds,
        "baseline_timeout_seconds": (
            None if is_resource_test_binary(executable) else baseline_timeout
        ),
        "diagnostic_reserve_seconds": _diagnostic_reserve(args.timeout_seconds),
        "executable": executable,
        "executable_resolved": executable_resolved,
        "executable_size": executable_size,
        "executable_sha256": executable_sha256,
        "inherited_args": inherited_args,
        "resource_process_isolation": resource_isolation,
        "status": "success" if returncode == 0 else "failed",
        "returncode": returncode,
        "reported_failures": sorted(set(reported_failures)),
        "failure_identities": sorted(failure_identities),
        "test_results": _canonical_test_results(
            executions, resource_isolation=resource_isolation
        ),
        "diagnosis": diagnosis,
        "baseline_termination": executions[0].termination if executions else None,
        "executions": [execution.receipt() for execution in executions],
    }
    receipt_path = _receipt_path(args.receipt_dir, executable, invocation_id)
    write_receipt(receipt_path, receipt)
    print(f"cargo-test-binary-runner: receipt={receipt_path}")
    return returncode


if __name__ == "__main__":
    raise SystemExit(main())
