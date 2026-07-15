"""Deterministic proof-log diagnostics and failure classification."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sqlite3
import time
from pathlib import Path
from typing import Mapping, Sequence

from tools.proof_queue_pkg import custody, state

DIAGNOSTIC_LOG_TAIL_BYTES = 256 * 1024

RUNNING_CHILD_MISSING_STALE_LOG_SECONDS = 180.0

RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS = 60.0

DIAGNOSTIC_EVIDENCE_MAX_CHARS = 640

TERMINAL_STALE_DIAGNOSTIC_IDS = frozenset({"running-proof-child-missing"})

STATIC_PYMOD_EXEC_RE = re.compile(
    r"(?:ImportError:\s+|Original error was:\s*)"
    r"(?P<module>[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)"
    r": static-link PyModuleDef Py_mod_exec slot returned non-zero"
    r"(?P<detail>[^\r\n]*)"
)

UNDEFINED_SYMBOL_RE = re.compile(
    r"(?:wasm-ld: error: .*?undefined symbol:|undefined symbol:)\s+"
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_@.$]*)"
)

RUNTIME_WASM_MISSING_EXPORTS_RE = re.compile(
    r"Runtime wasm (?:build produced artifact|artifact) missing required "
    r"exports[:;]?\s*(?P<symbols>[^\r\n]*)"
)

RUNTIME_EXPORT_AUTHORITY_UNKNOWN_NAME_RE = re.compile(
    r"ValueError: unknown WASM runtime import/export name: "
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_@.$]*)"
)

UNSUPPORTED_DIRECT_CALL_RE = re.compile(
    r"(?is)(?:unsupported|not supported|not linkable).*?"
    r"(?:direct call|direct-call).*?"
    r"(?P<symbol>[A-Za-z_][A-Za-z0-9_.]*)"
)

DIAGNOSTIC_JSON_RE = re.compile(r"diagnostic_json=(?P<path>\S+)")

QUEUE_COLD_SINGLE_CARGO_PROOF_RE = re.compile(
    r"proof queue refuses cold-prone single-test Cargo proofs "
    r"\('(?P<filter>[^']+)' under --lib\)"
)

PACT_WITNESS_FIXTURE_MISSING_RE = re.compile(
    r"missing Pact fixture:\s+(?P<path>[^\r\n]+)"
)

NATIVE_ARTIFACT_CUSTODY_RE = re.compile(
    r"External static package native-artifact custody errors:\s+(?P<detail>[^\r\n]+)"
)

NATIVE_RUNTIME_IMPORT_CUSTODY_RE = re.compile(
    r"External static package native-artifact custody errors:\s+"
    r"(?P<package>[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)"
    r": (?P<detail>sealed extension manifest lacks a "
    r"'runtime_python_import_modules' field[^\r\n]*)"
)

NATIVE_ARTIFACT_ABI_SURFACE_RE = re.compile(
    r"runtime ABI symbol '(?P<symbol>[^']+)' is not in the generated "
    r"WASM ABI/link import surface"
)

NATIVE_SUPPORT_CUSTODY_RE = re.compile(
    r"reachable native support source imports native package modules without source "
    r"or artifact custody:\s+(?P<detail>[^\r\n]+)"
)

SOURCE_EXTENSION_NM_MISSING_RE = re.compile(
    r"unable to read global symbol table for compiled extension object "
    r"(?P<object>[^\r\n;]+); canonical LLVM/WASI nm authority is unavailable"
)

STDLIB_PROFILE_REFUSAL_RE = re.compile(
    r"Profile '(?P<profile>[^']+)' excludes the '(?P<feature>[^']+)' "
    r"runtime feature"
)

MOLT_RUNTIME_INVALID_OBJECT_HEADER_RE = re.compile(
    r"(?m)^molt fatal: invalid object header(?P<detail>[^\r\n]*)"
)

RUST_COMPILER_ERROR_RE = re.compile(
    r"(?m)^error(?:\[(?P<code>E\d{4})\])?: (?P<message>[^\r\n]+)"
)

RUST_TEST_RESULT_FAILED_RE = re.compile(
    r"(?m)^test result: FAILED\.(?P<detail>[^\r\n]*)"
)

RUST_CARGO_TEST_FAILED_RE = re.compile(
    r"(?m)^error: test failed, to rerun pass `(?P<rerun>[^`]+)`"
)

RUST_FAILED_TEST_LINE_RE = re.compile(
    r"(?m)^test (?P<name>[A-Za-z0-9_:<>_.-]+) \.\.\. FAILED\r?$"
)

RUNTIME_WASM_RUST_TARGET_MISSING_RE = re.compile(
    r"(?m)^Runtime wasm build requires Rust target (?P<target>[A-Za-z0-9_-]+), "
    r"but the active Rust toolchain does not provide it\. "
    r"Run: (?P<command>rustup target add [^\r\n]+)\r?$"
)

WASM_TOOLCHAIN_CONTRACT_IMPORT_MISSING_RE = re.compile(
    r"(?m)^failed to import WASM toolchain contract: "
    r"No module named ['\"](?P<module>[A-Za-z0-9_.]+)['\"]\r?$"
)

PYTHON_EXCEPTION_RE = re.compile(
    r"(?m)^(?P<type>[A-Za-z_][A-Za-z0-9_.]*(?:Error|Exception)):\s+(?P<message>.+)$"
)

PYTHON_IMPORT_MISSING_RE = re.compile(
    r"(?:^|\\n|\n)(?P<type>ModuleNotFoundError|ImportError):\s+"
    r"No module named ['\"](?P<module>[A-Za-z0-9_.]+)['\"]"
)

SOURCE_LEASE_CHANGED_RE = re.compile(
    r"(?m)^Failed to read module (?P<module>.*): "
    r"Source lease for (?P<lease>.+) changed (?P<detail>[^\r\n]+)\r?$"
)

PARTIAL_MODULE_PUBLICATION_RE = re.compile(
    r"ImportError: cannot import partially initialized module "
    r"'(?P<module>[^']+)' before its publication "
    r"\(circular import during module allocation\)"
)

MOLT_DIFF_FAIL_RE = re.compile(
    r"(?m)^\[FAIL\]\s+(?P<case>\S+)\s+\((?P<target>[^)]+)\)\s+"
    r"(?P<detail>[^\r\n]+)"
)

MOLT_DIFF_STDOUT_LINE_RE = re.compile(
    r"(?m)^  (?P<label>CPython|Molt)\s+stdout: (?P<value>[^\r\n]+)"
)

PYTEST_FAILED_RE = re.compile(r"(?m)^FAILED\s+(?P<nodeid>\S+)")

NATIVE_IMPORT_BOOTSTRAP_NODE_PREFIX = (
    "tests/test_native_import_bootstrap_regressions.py::"
)

NATIVE_CALL_LANE_SCOPES = (
    "tests/test_native_import_bootstrap_regressions.py",
    "runtime/molt-runtime/src/call/function.rs",
    "runtime/molt-backend-native/src/native_backend/function_compiler/fc/modules.rs",
    "runtime/molt-runtime/src/call/class_init.rs",
    "runtime/molt-runtime/src/builtins/containers.rs",
    "runtime/molt-runtime/src/builtins/exceptions.rs",
    "runtime/molt-runtime/src/object/mod.rs",
)

PYTEST_ERROR_RE = re.compile(
    r"(?m)^ERROR\s+(?P<nodeid>\S+)(?:\s+-\s+(?P<detail>[^\r\n]+))?"
)

PYTEST_PROGRESS_LINE_RE = re.compile(r"^[.FEfsxX]+(?:\s+\[\s*\d+%\])?$")

PYTEST_ASSERTION_RE = re.compile(r"(?m)^E\s+(?P<error>AssertionError[^\r\n]*)")

PYTEST_EXCEPTION_LINE_RE = re.compile(
    r"(?m)^E\s+(?P<error>[A-Za-z_][A-Za-z0-9_.]*(?:Error|Exception):[^\r\n]*)"
)

MEMORY_GUARD_ORPHANED_RE = re.compile(
    r"memory_guard: orphaned child processes detected after command exit; "
    r"(?P<detail>[^\r\n]+)"
)

MEMORY_GUARD_TIMEOUT_RE = re.compile(
    r"memory_guard: timeout after (?P<timeout>[0-9.]+)s; "
    r"(?P<detail>[^\r\n]+)"
)

MEMORY_GUARD_CARGO_QUARANTINE_RE = re.compile(
    r"memory_guard: quarantined Cargo incremental state after "
    r"(?P<reason>[^:]+): "
    r"(?P<detail>[^\r\n]*\breceipt=(?P<receipt>.*?)(?: errors=\d+)?)"
    r"(?=\r?\n|$)"
)

AUDIT_ERROR_DIAGNOSTICS = frozenset(
    {
        "memory-guard-summary-incomplete",
        "memory-guard-timeout",
        "native-call-lane-memory-guard-timeout",
        "proof-log-missing",
        "queue-preexecution-failure",
    }
)

AUDIT_WARNING_DIAGNOSTICS = frozenset(
    {
        "queue-infra-warning",
        "memory-guard-orphan-cleanup",
        "nested-memory-guard-orphan-cleanup",
        "queue-policy-rejection",
        "runtime-wasm-rust-target-missing",
        "wasm-toolchain-contract-import-missing",
        "running-pytest-failures-observed",
        "running-pytest-current-test-missing",
    }
)

FRONTIER_SUPERSEDING_EDGE_KINDS = frozenset({"reruns", "supersedes"})

FRONTIER_SUPERSEDING_CHILD_STATUSES = frozenset(
    {"queued", "dispatched", "running", "passed", "failed"}
)



def _elapsed_since(started_at: str | None, elapsed_s: float | None = None) -> str:
    if elapsed_s is not None:
        return f"{elapsed_s:.1f}s"
    if not started_at:
        return "?"
    try:
        started = dt.datetime.fromisoformat(started_at)
    except ValueError:
        return "?"
    if started.tzinfo is None:
        started = started.replace(tzinfo=dt.UTC)
    elapsed = max(0.0, (dt.datetime.now(dt.UTC) - started).total_seconds())
    return f"{elapsed:.1f}s"



def _running_age_seconds(started_at: str | None) -> float | None:
    """Wall-clock seconds since ``started_at``, or None if unparseable."""
    if not started_at:
        return None
    try:
        started = dt.datetime.fromisoformat(started_at)
    except ValueError:
        return None
    if started.tzinfo is None:
        started = started.replace(tzinfo=dt.UTC)
    return max(0.0, (dt.datetime.now(dt.UTC) - started).total_seconds())



def _format_duration(seconds: float) -> str:
    if seconds < 60.0:
        return f"{seconds:.1f}s"
    if seconds < 3600.0:
        return f"{seconds / 60.0:.1f}m"
    return f"{seconds / 3600.0:.1f}h"



def _last_nonempty_log_line(path: Path) -> str | None:
    try:
        size = path.stat().st_size
        with path.open("rb") as handle:
            handle.seek(max(0, size - 65536))
            text = handle.read().decode("utf-8", errors="replace")
    except OSError:
        return None
    for line in reversed(text.splitlines()):
        stripped = line.strip()
        if stripped:
            return state._shorten(stripped)
    return None



def _first_log_line_containing(log_tail: str, needle: str) -> str | None:
    for line in log_tail.splitlines():
        if needle in line:
            return state._shorten(line)
    return None



def _read_log_tail(path: Path, *, limit: int = DIAGNOSTIC_LOG_TAIL_BYTES) -> str:
    try:
        size = path.stat().st_size
        with path.open("rb") as handle:
            handle.seek(max(0, size - limit))
            return handle.read().decode("utf-8", errors="replace")
    except OSError:
        return ""



def _read_json_object(path: Path) -> dict[str, object]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return raw if isinstance(raw, dict) else {}



def _log_age_seconds(path: Path) -> float | None:
    try:
        return max(0.0, time.time() - path.stat().st_mtime)
    except OSError:
        return None



def _is_guard_command(command: object) -> bool:
    if isinstance(command, str):
        return "memory_guard.py" in command.replace("\\", "/")
    if not isinstance(command, list):
        return False
    return any(
        isinstance(part, str)
        and part.replace("\\", "/").endswith("tools/memory_guard.py")
        for part in command
    )



def _row_command_mentions_pytest(row: sqlite3.Row) -> bool:
    try:
        command = json.loads(row["command_json"])
    except (TypeError, json.JSONDecodeError):
        return False
    if not isinstance(command, list):
        return False
    return any(
        isinstance(part, str)
        and part.replace("\\", "/").lower().rsplit("/", 1)[-1]
        in {"pytest", "pytest.exe"}
        for part in command
    )



def _pytest_sections_from_summary(summary_json: object) -> list[dict[str, object]]:
    if not summary_json:
        return []
    summary = _read_json_object(Path(str(summary_json)))
    candidates: list[object] = [summary.get("pytest")]
    repro = summary.get("repro")
    if isinstance(repro, dict):
        candidates.append(repro.get("pytest"))
    return [item for item in candidates if isinstance(item, dict)]



def _pytest_current_status_line(summary_json: object) -> str | None:
    for pytest_section in _pytest_sections_from_summary(summary_json):
        current_test_file = pytest_section.get("current_test_file")
        if isinstance(current_test_file, dict):
            payload = current_test_file.get("payload")
            if isinstance(payload, dict):
                nodeid = payload.get("nodeid")
                if isinstance(nodeid, str) and nodeid.strip():
                    phase = payload.get("phase")
                    line = f"  pytest_current={nodeid.strip()}"
                    if isinstance(phase, str) and phase.strip():
                        line += f" phase={phase.strip()}"
                    return line
            path = current_test_file.get("path")
            if current_test_file.get("missing") is True and isinstance(path, str):
                return f"  pytest_current=missing path={path}"
            error = current_test_file.get("error")
            if isinstance(error, str) and error.strip():
                return (
                    f"  pytest_current=unreadable error={state._shorten(error.strip(), 120)}"
                )
        current = pytest_section.get("current_test")
        if isinstance(current, str) and current.strip():
            return f"  pytest_current={current.strip()}"
    return None



def _pytest_missing_current_test_file_evidence(summary_json: object) -> str | None:
    for pytest_section in _pytest_sections_from_summary(summary_json):
        current_test_file = pytest_section.get("current_test_file")
        if not isinstance(current_test_file, dict):
            continue
        if current_test_file.get("missing") is not True:
            continue
        path = current_test_file.get("path")
        if isinstance(path, str) and path.strip():
            return f"pytest_current_test_file_missing path={path.strip()}"
        return "pytest_current_test_file_missing"
    return None



def _pytest_progress_failure_counts(line: str | None) -> tuple[int, int] | None:
    if line is None:
        return None
    progress = line.strip()
    if PYTEST_PROGRESS_LINE_RE.fullmatch(progress) is None:
        return None
    failures = progress.count("F")
    errors = progress.count("E")
    if failures == 0 and errors == 0:
        return None
    return failures, errors



def _summary_child_process(summary_json: object) -> dict[str, object] | None:
    if not summary_json:
        return None
    summary = _read_json_object(Path(str(summary_json)))
    child = summary.get("child_process")
    return child if isinstance(child, dict) else None



def _summary_host_platform(summary_json: object) -> str | None:
    if not summary_json:
        return None
    summary = _read_json_object(Path(str(summary_json)))
    repro = summary.get("repro")
    if not isinstance(repro, dict):
        return None
    host = repro.get("host")
    if not isinstance(host, dict):
        return None
    platform = host.get("platform")
    return platform if isinstance(platform, str) and platform.strip() else None



def _summary_payload_host_platform(summary: Mapping[str, object]) -> str | None:
    repro = summary.get("repro")
    if not isinstance(repro, dict):
        return None
    host = repro.get("host")
    if not isinstance(host, dict):
        return None
    platform = host.get("platform")
    return platform if isinstance(platform, str) and platform.strip() else None



def _memory_guard_child_runner_evidence(summary_json: object) -> str | None:
    child = _summary_child_process(summary_json)
    if child is None:
        return None
    command = child.get("command")
    if not _is_guard_command(command):
        return None
    label = (
        "windows_memory_guard_child_runner"
        if _summary_host_platform(summary_json) == "win32"
        else "memory_guard_child_process"
    )
    pid = child.get("pid")
    if isinstance(pid, int) and pid > 0:
        return f"child_process={label} pid={pid}"
    return f"child_process={label}"



def _memory_guard_child_runner_status_line(summary_json: object) -> str | None:
    evidence = _memory_guard_child_runner_evidence(summary_json)
    if evidence is None:
        return None
    return f"  guard_child={evidence}"



def _memory_guard_child_descendant_status_line(summary_json: object) -> str | None:
    child = _summary_child_process(summary_json)
    if child is None:
        return None
    child_pid = child.get("pid")
    if not isinstance(child_pid, int) or child_pid <= 0:
        return None
    if not _is_guard_command(child.get("command")):
        return None
    try:
        from tools import memory_guard

        samples = memory_guard.sample_processes()
        descendants = memory_guard.descendant_pids(samples, child_pid)
    except Exception:
        return None
    if not descendants:
        return None
    line = f"  guard_descendants={len(descendants)}"
    sample_evidence = _descendant_sample_evidence(samples, descendants)
    if sample_evidence is not None:
        line += f" {sample_evidence}"
    return line



def _descendant_sample_evidence(
    samples: Mapping[int, object],
    descendants: set[int],
    *,
    limit: int = 3,
) -> str | None:
    snippets: list[str] = []
    for pid in sorted(descendants):
        sample = samples.get(pid)
        command = getattr(sample, "command", None)
        if not isinstance(command, str) or not command.strip():
            continue
        if _low_signal_descendant_command(command):
            continue
        snippets.append(f"{pid}:{state._shorten(command, 96)}")
        if len(snippets) >= limit:
            break
    if not snippets:
        return None
    suffix = (
        ""
        if len(descendants) <= len(snippets)
        else f" +{len(descendants) - len(snippets)} more"
    )
    return "descendant_samples=" + "; ".join(snippets) + suffix



def _low_signal_descendant_command(command: str) -> bool:
    normalized = command.replace("\\", "/").strip().strip('"').casefold()
    return normalized.endswith("/conhost.exe") or normalized == "conhost.exe"



def _running_pytest_failures_observed_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] != "running":
        return None
    if not _row_command_mentions_pytest(row):
        return None
    log_path = Path(row["log_path"])
    last_log_line = _last_nonempty_log_line(log_path)
    counts = _pytest_progress_failure_counts(last_log_line)
    if counts is None:
        return None
    failures, errors = counts
    evidence_parts = [
        f"last_pytest_progress={last_log_line}",
        f"failures={failures}",
        f"errors={errors}",
    ]
    log_age_s = _log_age_seconds(log_path)
    if log_age_s is not None:
        evidence_parts.append(f"last_log_age={_format_duration(log_age_s)}")
    child_runner = _memory_guard_child_runner_evidence(row["summary_json"])
    if child_runner is not None:
        evidence_parts.append(child_runner)
    return _diagnostic(
        signal_id="running-pytest-failures-observed",
        severity="warning",
        summary=(
            "Running pytest proof has already emitted failure/error progress markers."
        ),
        evidence=" ".join(evidence_parts),
        next_action=(
            "Keep the row running for the full pytest failure report; do not "
            "classify this as infra-only current-test opacity or interrupt via "
            "Codex stdin."
        ),
        scopes=("tools/proof_queue.py",),
        artifacts=(str(row["log_path"]),),
    )



def _running_pytest_current_test_missing_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] != "running":
        return None
    if not _row_command_mentions_pytest(row):
        return None
    log_path = Path(row["log_path"])
    log_age_s = _log_age_seconds(log_path)
    if (
        log_age_s is None
        or log_age_s < RUNNING_PYTEST_CURRENT_TEST_MISSING_STALE_SECONDS
    ):
        return None
    evidence = _pytest_missing_current_test_file_evidence(row["summary_json"])
    if evidence is None:
        return None
    evidence_parts: list[str] = []
    child_runner = _memory_guard_child_runner_evidence(row["summary_json"])
    if child_runner is not None:
        evidence_parts.append(child_runner)
    evidence_parts.append(evidence)
    evidence_parts.append(f"last_log_age={_format_duration(log_age_s)}")
    last_log_line = _last_nonempty_log_line(log_path)
    pytest_progress = (
        last_log_line
        if last_log_line is not None
        and PYTEST_PROGRESS_LINE_RE.fullmatch(last_log_line.strip()) is not None
        else None
    )
    if pytest_progress is not None:
        evidence_parts.append(f"last_pytest_progress={pytest_progress}")
        summary = (
            "Running pytest proof emitted progress output but has no current-test "
            "marker."
        )
        next_action = (
            "Treat this as current-test custody opacity after pytest started, "
            "not collection/startup opacity. Inspect pytest guard plugin/env "
            "wiring once, then rerun with a focused selector if the row does not "
            "finish; do not interrupt via Codex stdin."
        )
    else:
        if last_log_line:
            evidence_parts.append(f"last_log={last_log_line}")
        summary = (
            "Running pytest proof has no current-test marker while its queue log "
            "is quiet."
        )
        next_action = (
            "Treat this as pre-test or collection/startup opacity. Inspect the "
            "pytest startup path, uv/cache contention, or Windows memory-guard "
            "child-runner descendant once, then rerun with a first-failure or "
            "collection-focused proof; do not interrupt via Codex stdin."
        )
    evidence = " ".join(evidence_parts)
    return _diagnostic(
        signal_id="running-pytest-current-test-missing",
        severity="infra",
        summary=summary,
        evidence=evidence,
        next_action=next_action,
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )



def _running_child_missing_diagnostic(row: sqlite3.Row) -> dict[str, object] | None:
    if row["status"] != "running":
        return None
    log_age_s = _log_age_seconds(Path(row["log_path"]))
    if log_age_s is None or log_age_s < RUNNING_CHILD_MISSING_STALE_LOG_SECONDS:
        return None
    summary = _read_json_object(Path(row["summary_json"]))
    child = summary.get("child_process")
    if not isinstance(child, dict):
        if summary.get("status") == "running" and summary.get("returncode") is None:
            evidence = (
                f"summary_json={row['summary_json']} summary_status=running "
                f"child_process=null returncode=null "
                f"last_log_age={_format_duration(log_age_s)}"
            )
            recorded_at = summary.get("recorded_at")
            if isinstance(recorded_at, str) and recorded_at.strip():
                evidence += f" recorded_at={recorded_at.strip()}"
            return _diagnostic(
                signal_id="running-proof-launch-summary-stale",
                severity="infra",
                summary=(
                    "Running proof row still has only the guard launch summary "
                    "after its log went stale."
                ),
                evidence=evidence,
                next_action=(
                    "Treat the row as custody-incomplete evidence. Inspect the "
                    "log and guard summary, then use `prune-stale` or rerun "
                    "through the same queue lane; do not interrupt via Codex stdin."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        return None
    child_pid = child.get("pid")
    if not isinstance(child_pid, int) or child_pid <= 0:
        return None
    child_is_guard_command = _is_guard_command(child.get("command"))
    if not child_is_guard_command:
        return None
    host_platform = _summary_payload_host_platform(summary)
    child_runner_label = (
        "windows_memory_guard_child_runner"
        if host_platform == "win32"
        else "memory_guard_child_process"
    )
    if not custody._pid_alive(child_pid):
        evidence = (
            f"summary_json={row['summary_json']} child_pid={child_pid} "
            f"last_log_age={_format_duration(log_age_s)}"
        )
        if host_platform == "win32":
            evidence += f" child_process={child_runner_label}"
            return _diagnostic(
                signal_id="running-proof-windows-child-runner-missing",
                severity="infra",
                summary=(
                    "Running proof row's Windows memory-guard child runner is "
                    "no longer live while the queue-owned guard still owns the row."
                ),
                evidence=evidence,
                next_action=(
                    "Treat this as Windows memory-guard custody opacity, not a "
                    "terminal stale signal while the queue-owned guard is live. "
                    "Wait for the guard's final summary, or let prune-stale "
                    "reclaim it only after the guard exits or the running-age "
                    "ceiling is exceeded."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        summary_text = (
            "Running proof row's nested memory guard child is no longer live."
        )
    else:
        try:
            from tools import memory_guard

            samples = memory_guard.sample_processes()
            descendants = memory_guard.descendant_pids(samples, child_pid)
        except Exception as exc:  # pragma: no cover - sampler failure is host-specific.
            return _diagnostic(
                signal_id="running-proof-custody-sampler-failed",
                severity="infra",
                summary="Running proof row could not be inspected by the process sampler.",
                evidence=(
                    f"summary_json={row['summary_json']} child_pid={child_pid} "
                    f"sampler_error={type(exc).__name__}: {exc}"
                ),
                next_action=(
                    "Inspect the memory-guard summary and process custody manually, "
                    "then fix the sampler if this host shape repeats."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
            )
        if descendants:
            sample_evidence = _descendant_sample_evidence(samples, descendants)
            evidence = (
                f"summary_json={row['summary_json']} child_pid={child_pid} "
                f"last_log_age={_format_duration(log_age_s)} "
                f"descendants={len(descendants)}"
            )
            if sample_evidence is not None:
                evidence += f" {sample_evidence}"
            return _diagnostic(
                signal_id="running-proof-log-stale-live-child",
                severity="infra",
                summary=(
                    "Running proof row has a stale log, but its nested memory "
                    "guard still owns live work descendants."
                ),
                evidence=evidence,
                next_action=(
                    "Do not prune or interrupt this row from the diagnostic alone. "
                    "Inspect the child command or add one queue note if the row is "
                    "compile-dominated, then let the owner timeout/finish or rerun "
                    "with a better-shaped proof."
                ),
                scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        evidence = (
            f"summary_json={row['summary_json']} child_pid={child_pid} "
            f"last_log_age={_format_duration(log_age_s)} descendants=0"
        )
        summary_text = (
            "Running proof row has a live nested memory guard but no visible "
            "work child beneath it."
        )
    return _diagnostic(
        signal_id="running-proof-child-missing",
        severity="infra",
        summary=summary_text,
        evidence=evidence,
        next_action=(
            "Treat the row as custody-incomplete evidence. Inspect the log and "
            "memory-guard summary, then use `prune-stale` or rerun through the "
            "same queue lane; do not interrupt via Codex stdin."
        ),
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )



def _finished_incomplete_memory_guard_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] in state.RUNNING:
        return None
    summary = _read_json_object(Path(row["summary_json"]))
    summary_status = summary.get("status")
    if summary_status not in {"running", "child_running"}:
        return None
    if summary.get("returncode") is not None:
        return None
    evidence_parts = [
        f"row_status={row['status']}",
        f"row_returncode={row['returncode']}",
        f"summary_status={summary_status}",
        "summary_returncode=null",
    ]
    elapsed_s = row["elapsed_s"]
    if isinstance(elapsed_s, (int, float)):
        evidence_parts.append(f"row_elapsed={_format_duration(float(elapsed_s))}")
    limits = summary.get("limits")
    repro = summary.get("repro")
    if not isinstance(limits, dict) and isinstance(repro, dict):
        limits = repro.get("limits")
    if isinstance(limits, dict):
        timeout_s = limits.get("timeout_s")
        if isinstance(timeout_s, (int, float)):
            evidence_parts.append(
                f"configured_timeout={_format_duration(float(timeout_s))}"
            )
    child = summary.get("child_process")
    if isinstance(child, dict):
        command = child.get("command")
        child_runner = (
            _memory_guard_child_runner_evidence(row["summary_json"])
            if _is_guard_command(command)
            else None
        )
        if child_runner is not None:
            evidence_parts.append(child_runner)
        else:
            child_pid = child.get("pid")
            if isinstance(child_pid, int) and child_pid > 0:
                evidence_parts.append(f"child_pid={child_pid}")
    recorded_at = summary.get("recorded_at")
    if isinstance(recorded_at, str) and recorded_at.strip():
        evidence_parts.append(f"recorded_at={recorded_at.strip()}")
    log_age_s = _log_age_seconds(Path(row["log_path"]))
    if log_age_s is not None:
        evidence_parts.append(f"last_log_age={_format_duration(log_age_s)}")
    last_log_line = _last_nonempty_log_line(Path(row["log_path"]))
    if last_log_line:
        evidence_parts.append(f"last_log={last_log_line}")
    evidence_parts.append(f"summary_json={row['summary_json']}")
    evidence = " ".join(evidence_parts)
    return _diagnostic(
        signal_id="memory-guard-summary-incomplete",
        severity="infra",
        summary=(
            "Terminal proof row has only a non-final memory-guard running summary."
        ),
        evidence=evidence,
        next_action=(
            "Treat this row as queue-custody incomplete evidence, not product "
            "proof. Inspect the queue log and summary once, then rerun through "
            "the same queue lane or fix memory_guard final-summary lifecycle if "
            "the pattern repeats."
        ),
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )



def _finished_worker_exit_without_summary_diagnostic(
    row: sqlite3.Row,
) -> dict[str, object] | None:
    if row["status"] in state.RUNNING:
        return None
    summary = _read_json_object(Path(row["summary_json"]))
    if summary.get("status") != "guard_worker_exited_without_final_summary":
        return None
    evidence_parts = [
        f"row_status={row['status']}",
        f"row_returncode={row['returncode']}",
    ]
    worker_returncode = summary.get("worker_returncode")
    if worker_returncode is not None:
        evidence_parts.append(f"worker_returncode={worker_returncode}")
    previous_status = None
    incident = summary.get("incident")
    if isinstance(incident, dict):
        raw_previous_status = incident.get("previous_status")
        if isinstance(raw_previous_status, str) and raw_previous_status.strip():
            previous_status = raw_previous_status.strip()
            evidence_parts.append(f"previous_status={previous_status}")
    signal_payload = summary.get("worker_exit_signal") or summary.get("exit_signal")
    if isinstance(signal_payload, dict):
        signal_name = signal_payload.get("name")
        signal_number = signal_payload.get("signal")
        if isinstance(signal_name, str) and signal_name.strip():
            evidence_parts.append(f"worker_signal={signal_name.strip()}")
        elif isinstance(signal_number, int):
            evidence_parts.append(f"worker_signal={signal_number}")
    child = summary.get("child_process")
    if isinstance(child, dict):
        command = child.get("command")
        child_runner = (
            _memory_guard_child_runner_evidence(row["summary_json"])
            if _is_guard_command(command)
            else None
        )
        if child_runner is not None:
            evidence_parts.append(child_runner)
        else:
            child_pid = child.get("pid")
            if isinstance(child_pid, int) and child_pid > 0:
                evidence_parts.append(f"child_pid={child_pid}")
    recorded_at = summary.get("recorded_at")
    if isinstance(recorded_at, str) and recorded_at.strip():
        evidence_parts.append(f"recorded_at={recorded_at.strip()}")
    last_log_line = _last_nonempty_log_line(Path(row["log_path"]))
    if last_log_line:
        evidence_parts.append(f"last_log={last_log_line}")
    evidence_parts.append(f"summary_json={row['summary_json']}")
    return _diagnostic(
        signal_id="memory-guard-worker-exit-without-final-summary",
        severity="infra",
        summary=(
            "Terminal proof row was preserved by the memory_guard wrapper after "
            "the internal guard worker exited before writing the final summary."
        ),
        evidence=" ".join(evidence_parts),
        next_action=(
            "Treat this row as guard-custody failure evidence, not product proof. "
            "Inspect the worker signal/source and child logs once, then rerun "
            "through the same queue lane after the guard lifecycle is fixed."
        ),
        scopes=("tools/proof_queue.py", "tools/memory_guard.py"),
        artifacts=(str(row["summary_json"]), str(row["log_path"])),
    )



def _active_log_status(row: sqlite3.Row) -> list[str]:
    path = Path(row["log_path"])
    try:
        stat = path.stat()
    except OSError:
        if row["status"] == "queued":
            return [f"  log={path} (queued; proof command not launched yet)"]
        if row["status"] == "dispatched":
            return [f"  log={path} (dispatched; waiting for detached runner)"]
        return [f"  log={path} (missing)"]
    age = _format_duration(max(0.0, time.time() - stat.st_mtime))
    lines = [f"  log={path}", f"  last_log_age={age}"]
    last = _last_nonempty_log_line(path)
    if last:
        lines[-1] = f"{lines[-1]} last={last}"
    pytest_line = (
        _pytest_current_status_line(row["summary_json"])
        if _row_command_mentions_pytest(row)
        else None
    )
    if pytest_line:
        lines.append(pytest_line)
    child_runner_line = _memory_guard_child_runner_status_line(row["summary_json"])
    if child_runner_line:
        lines.append(child_runner_line)
    child_descendant_line = _memory_guard_child_descendant_status_line(
        row["summary_json"]
    )
    if child_descendant_line:
        lines.append(child_descendant_line)
    return lines



def _diagnostic(
    *,
    signal_id: str,
    severity: str,
    summary: str,
    evidence: str,
    next_action: str,
    scopes: Sequence[str] = (),
    artifacts: Sequence[str] = (),
) -> dict[str, object]:
    return {
        "signal_id": signal_id,
        "severity": severity,
        "summary": summary,
        "evidence": state._shorten(evidence, DIAGNOSTIC_EVIDENCE_MAX_CHARS),
        "next_action": next_action,
        "scopes": list(scopes),
        "artifacts": list(artifacts),
    }



def _pytest_timeout_context(summary_json: object) -> tuple[str, str | None] | None:
    for pytest_section in _pytest_sections_from_summary(summary_json):
        current_test_file = pytest_section.get("current_test_file")
        payload: object = None
        if isinstance(current_test_file, dict):
            payload = current_test_file.get("payload")
        if not isinstance(payload, dict):
            continue
        nodeid = payload.get("nodeid")
        if not isinstance(nodeid, str) or not nodeid.strip():
            continue
        phase = payload.get("phase")
        phase_text = phase.strip() if isinstance(phase, str) and phase.strip() else None
        return nodeid.strip(), phase_text
    return None



def _diagnostics_have_terminal_stale_signal(
    diagnostics: Sequence[dict[str, object]],
) -> bool:
    return any(
        diagnostic.get("signal_id") in TERMINAL_STALE_DIAGNOSTIC_IDS
        for diagnostic in diagnostics
    )



def _diagnostics_have_signal(
    diagnostics: Sequence[dict[str, object]], signal_id: str
) -> bool:
    return any(diagnostic.get("signal_id") == signal_id for diagnostic in diagnostics)



def _format_diagnostic_summary(diagnostics: list[dict[str, object]]) -> str | None:
    if not diagnostics:
        return None
    first = diagnostics[0]
    return (
        f"{first['signal_id']} [{first['severity']}]: {state._shorten(str(first['summary']))}"
    )



def _diagnostic_artifacts(diagnostics: Sequence[dict[str, object]]) -> list[str]:
    if not diagnostics:
        return []
    artifacts = diagnostics[0].get("artifacts", [])
    if not isinstance(artifacts, list):
        return []
    return [str(path) for path in artifacts]



def _print_status_diagnostics(row: sqlite3.Row) -> None:
    diagnostics = _run_diagnostics(row)
    diagnostic_summary = _format_diagnostic_summary(diagnostics)
    if diagnostic_summary:
        print(f"  diagnosis={diagnostic_summary}")
    artifacts = _diagnostic_artifacts(diagnostics)
    if artifacts:
        print(f"  artifacts={', '.join(artifacts)}")



def _diagnosis_note_body(row: sqlite3.Row, diagnostics: list[dict[str, object]]) -> str:
    if diagnostics:
        first = diagnostics[0]
        artifact_text = ""
        artifacts = _diagnostic_artifacts(diagnostics)
        if artifacts:
            artifact_text = " artifacts: " + ", ".join(artifacts)
        return (
            f"diagnosis: {row['run_id']} {row['status']} rc={row['returncode']} "
            f"{first['signal_id']}: {first['summary']}{artifact_text} "
            f"next: {first['next_action']}"
        )
    return (
        f"diagnosis: {row['run_id']} {row['status']} rc={row['returncode']} "
        "has no queue diagnostic signals."
    )



def _audit_issue(
    *,
    signal_id: str,
    severity: str,
    summary: str,
    next_action: str,
    run_id: str | None = None,
    evidence: str = "",
    artifacts: Sequence[str] = (),
) -> dict[str, object]:
    return {
        "signal_id": signal_id,
        "severity": severity,
        "run_id": run_id,
        "summary": summary,
        "evidence": state._shorten(evidence, 320),
        "next_action": next_action,
        "artifacts": list(artifacts),
    }



def _audit_severity_for_diagnostic(row: sqlite3.Row, signal_id: str) -> str | None:
    if signal_id == "memory-guard-summary-incomplete" and row["status"] == "stale":
        return "warning"
    if signal_id in AUDIT_ERROR_DIAGNOSTICS:
        return "error"
    if signal_id in AUDIT_WARNING_DIAGNOSTICS:
        return "warning"
    return None



def _frontier_failure(
    row: sqlite3.Row, diagnostics: list[dict[str, object]]
) -> dict[str, object] | None:
    if _diagnostics_have_signal(diagnostics, "memory-guard-summary-incomplete"):
        return None
    for item in diagnostics:
        if str(item["severity"]) != "error":
            continue
        signal_id = str(item["signal_id"])
        if (
            signal_id in AUDIT_ERROR_DIAGNOSTICS
            or signal_id in AUDIT_WARNING_DIAGNOSTICS
        ):
            continue
        return {
            "run_id": row["run_id"],
            "logical_id": row["logical_id"],
            "diagnostic": signal_id,
            "summary": item["summary"],
            "evidence": item["evidence"],
            "next_action": item["next_action"],
            "log_path": row["log_path"],
            "finished_at": row["finished_at"],
        }
    return None



def _frontier_superseded(dag: dict[str, list[dict[str, object]]]) -> bool:
    for edge in dag.get("children", []):
        if str(edge["kind"]) not in FRONTIER_SUPERSEDING_EDGE_KINDS:
            continue
        if str(edge["child_status"]) in FRONTIER_SUPERSEDING_CHILD_STATUSES:
            return True
    return False



def _audit_rows(
    conn: sqlite3.Connection, args: argparse.Namespace
) -> list[sqlite3.Row]:
    conn.row_factory = sqlite3.Row
    active = list(
        conn.execute(
            f"SELECT * FROM proof_runs WHERE status IN ({state.ACTIVE_SQL_STATUSES}) "
            "ORDER BY started_at"
        )
    )
    if args.all:
        historical = list(
            conn.execute(
                f"SELECT * FROM proof_runs WHERE status NOT IN ({state.ACTIVE_SQL_STATUSES}) "
                "ORDER BY rowid DESC"
            )
        )
    else:
        historical = list(
            conn.execute(
                """
                SELECT * FROM proof_runs
                WHERE status NOT IN ('queued', 'dispatched', 'running')
                ORDER BY rowid DESC
                LIMIT ?
                """,
                (args.limit,),
            )
        )
    seen: set[str] = set()
    rows: list[sqlite3.Row] = []
    for row in [*active, *historical]:
        run_id = str(row["run_id"])
        if run_id in seen:
            continue
        seen.add(run_id)
        rows.append(row)
    return rows



def _diagnose_row(conn: sqlite3.Connection, args: argparse.Namespace) -> sqlite3.Row:
    conn.row_factory = sqlite3.Row
    if args.run_id:
        row = conn.execute(
            "SELECT * FROM proof_runs WHERE run_id = ?",
            (args.run_id,),
        ).fetchone()
    elif args.logical_id:
        row = conn.execute(
            """
            SELECT * FROM proof_runs
            WHERE logical_id = ?
            ORDER BY rowid DESC
            LIMIT 1
            """,
            (args.logical_id,),
        ).fetchone()
    else:
        row = conn.execute(
            "SELECT * FROM proof_runs ORDER BY rowid DESC LIMIT 1"
        ).fetchone()
    if row is None:
        selector = args.run_id or args.logical_id or "latest proof run"
        raise SystemExit(f"unknown proof run selector {selector!r}")
    return row

SOURCE_EXTENSION_BUILD_PLAN_MISSING_RE = re.compile(
    r"source extension build plan not found: (?P<path>[^\r\n\"]+)"
)

SOURCE_EXTENSION_COMPILE_HEADER_MISSING_RE = re.compile(
    r"Failed compiling (?P<source>[^:\r\n]+):[\s\S]*?fatal error: "
    r"'(?P<header>[^']+)' file not found"
)

SOURCE_EXTENSION_CIMPORT_HEADER_MISMATCH_RE = re.compile(
    r"(?P<evidence>Failed compiling (?P<source>[^:\r\n]+):[\s\S]*?"
    r"(?:call to undeclared function "
    r"'(?P<symbol>PyDataType_[^']+|_PyUFuncObject_GET_ITEM_DATA)'|"
    r"member reference type 'int' is not a pointer)[\s\S]*?)"
    r"(?=\n\n|proof_queue finished|$)"
)

SOURCE_EXTENSION_CPYTHON_ABI_DECL_MISSING_RE = re.compile(
    r"(?P<evidence>Failed compiling (?P<source>[^:\r\n]+):[\s\S]*?"
    r"call to undeclared function '(?P<symbol>_?Py[A-Za-z0-9_]+)'[\s\S]*?"
    r"Python\.h[\s\S]*?)"
    r"(?=\n\n|proof_queue finished|$)"
)

SOURCE_EXTENSION_CYTHON_REGENERATION_FAILED_RE = re.compile(
    r"Standalone `cython -3` regeneration of (?P<source>[^`]+) failed: "
    r"(?P<error>[^\r\n\"]+)"
)

CPYTHON_ABI_PYMOD_GIL_SLOT_RE = re.compile(
    r"(?P<evidence>Failed compiling [^\r\n]+:[\s\S]*?"
    r"incompatible integer to pointer conversion[\s\S]*?"
    r"Py_mod_multiple_interpreters[\s\S]*?"
    r"Py_MOD_PER_INTERPRETER_GIL_SUPPORTED[\s\S]*?)"
    r"(?=\n\n|proof_queue finished|$)"
)

def _run_diagnostics(row: sqlite3.Row) -> list[dict[str, object]]:
    log_tail = _read_log_tail(Path(row["log_path"]))
    diagnostics: list[dict[str, object]] = []
    running_pytest_failures = _running_pytest_failures_observed_diagnostic(row)
    if running_pytest_failures is not None:
        diagnostics.append(running_pytest_failures)
    running_pytest_missing = _running_pytest_current_test_missing_diagnostic(row)
    if running_pytest_missing is not None:
        diagnostics.append(running_pytest_missing)
    running_child_missing = _running_child_missing_diagnostic(row)
    if running_child_missing is not None:
        diagnostics.append(running_child_missing)
    if row["status"] == "blocked":
        diagnostics.append(
            _diagnostic(
                signal_id="proof-dependency-blocked",
                severity="operator",
                summary="The proof did not run because a dependency edge did not pass.",
                evidence=(
                    _first_log_line_containing(
                        log_tail, "proof queue blocked by dependency"
                    )
                    or f"log_path={row['log_path']}"
                ),
                next_action=(
                    "Inspect the run DAG parents in evidence/status, fix or supersede "
                    "the failed dependency, then queue a new rerun edge."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )
        return diagnostics
    if not log_tail and row["status"] not in {"passed", "queued", "running"}:
        return [
            _diagnostic(
                signal_id="proof-log-missing",
                severity="infra",
                summary="The proof row is terminal but its queue log is missing.",
                evidence=f"log_path={row['log_path']}",
                next_action=(
                    "Treat this as incomplete evidence; inspect the queue DB and "
                    "rerun through the same queue lane after preserving the row id."
                ),
            )
        ]

    if (
        "proof queue refuses raw `cargo` commands" in log_tail
        or "proof queue refuses `uv run` commands" in log_tail
    ):
        diagnostics.append(
            _diagnostic(
                signal_id="queue-policy-rejection",
                severity="operator",
                summary="The queue rejected a noncanonical command before proof execution.",
                evidence=_last_nonempty_log_line(Path(row["log_path"])) or "",
                next_action=(
                    "Resubmit through the queue-native cargo lane or the active "
                    "uv contract; this row is DX policy evidence, not product proof."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    match = QUEUE_COLD_SINGLE_CARGO_PROOF_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="queue-cold-single-cargo-proof",
                severity="operator",
                summary=(
                    "The queue rejected a cold-prone single-test Cargo proof "
                    f"for filter {match.group('filter')}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Batch the relevant crate shard in one compile, warm the "
                    "target dir first, or resubmit with --allow-warm-single-test "
                    "only after recording warm-target evidence in the queue note."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    fatal_queue_failure = (
        "proof queue fatal infrastructure failure" in log_tail
        or "proof queue failed before command execution" in log_tail
    )
    if fatal_queue_failure:
        diagnostics.append(
            _diagnostic(
                signal_id="queue-preexecution-failure",
                severity="infra",
                summary=(
                    "The queue hit a fatal infrastructure failure before "
                    "launching the proof command, but the row was made terminal "
                    "and logged."
                ),
                evidence=(
                    _first_log_line_containing(
                        log_tail, "proof queue fatal infrastructure failure"
                    )
                    or _first_log_line_containing(
                        log_tail, "proof queue failed before command execution"
                    )
                    or _last_nonempty_log_line(Path(row["log_path"]))
                    or ""
                ),
                next_action=(
                    "Fix the queue custody bug, then resubmit or run the same "
                    "queued lane; do not treat this row as product proof."
                ),
                scopes=("tools/proof_queue.py",),
            )
        )

    if (
        not fatal_queue_failure
        and "proof queue nonfatal infrastructure failure" in log_tail
    ):
        diagnostics.append(
            _diagnostic(
                signal_id="queue-infra-warning",
                severity="infra",
                summary=(
                    "The proof command ran, but queue-side observability had a "
                    "nonfatal infrastructure failure."
                ),
                evidence=(
                    _first_log_line_containing(
                        log_tail, "proof queue nonfatal infrastructure failure"
                    )
                    or _last_nonempty_log_line(Path(row["log_path"]))
                    or ""
                ),
                next_action=(
                    "Preserve the proof result, then fix the queue projection or "
                    "note append issue before it becomes hidden collaboration debt."
                ),
                scopes=("tools/proof_queue.py", "docs/agent/PROOF_QUEUE.md"),
            )
        )

    if (
        "[scoreboard] machine NOT quiescent" in log_tail
        and "[scoreboard] refusing non-authoritative measurement before starting benchmark builds"
        in log_tail
    ):
        evidence_parts = [
            _first_log_line_containing(
                log_tail, "[scoreboard] machine NOT quiescent"
            )
            or "",
            _first_log_line_containing(
                log_tail,
                "[scoreboard] refusing non-authoritative measurement",
            )
            or "",
        ]
        diagnostics.append(
            _diagnostic(
                signal_id="perf-scoreboard-not-quiescent",
                severity="operator",
                summary=(
                    "The canonical perf scoreboard failed closed before "
                    "benchmarking because the machine never became quiescent."
                ),
                evidence="\n".join(part for part in evidence_parts if part),
                next_action=(
                    "Let active build/proof work drain or schedule an exclusive "
                    "perf window, then rerun the same canonical scoreboard from "
                    "current origin/main. Do not use --allow-nonauthoritative for "
                    "release or acceptance evidence."
                ),
                scopes=(
                    "tools/perf_scoreboard.py",
                    "tools/proof_queue.py",
                    "docs/agent/ORCHESTRATION.md",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    rust_test_result = RUST_TEST_RESULT_FAILED_RE.search(log_tail)
    rust_cargo_test_failed = RUST_CARGO_TEST_FAILED_RE.search(log_tail)
    if rust_test_result is not None or rust_cargo_test_failed is not None:
        failed_tests = tuple(
            dict.fromkeys(
                match.group("name")
                for match in RUST_FAILED_TEST_LINE_RE.finditer(log_tail)
            )
        )
        evidence_parts: list[str] = []
        if rust_test_result is not None:
            evidence_parts.append(rust_test_result.group(0))
        if rust_cargo_test_failed is not None:
            evidence_parts.append(rust_cargo_test_failed.group(0))
        if failed_tests:
            listed = ", ".join(failed_tests[:5])
            if len(failed_tests) > 5:
                listed += f", ... (+{len(failed_tests) - 5} more)"
            evidence_parts.append(f"failed_tests={listed}")
        diagnostics.append(
            _diagnostic(
                signal_id="rust-test-failure",
                severity="error",
                summary=(
                    "Rust proof compiled and reached test execution, but "
                    f"cargo test reported {len(failed_tests) or 'failed'} "
                    "test failure(s)."
                ),
                evidence=" ".join(evidence_parts),
                next_action=(
                    "Fix the failing Rust test or the product contract it protects, "
                    "then rerun the same queue lane. This row reached test "
                    "execution; do not classify it as a compiler failure."
                ),
                scopes=("runtime/", "tools/proof_queue.py"),
            )
        )

    match = RUST_COMPILER_ERROR_RE.search(log_tail)
    if (
        match is not None
        and rust_test_result is None
        and rust_cargo_test_failed is None
    ):
        code = match.group("code") or "rustc"
        message = match.group("message").strip()
        diagnostics.append(
            _diagnostic(
                signal_id="rust-compiler-error",
                severity="error",
                summary=f"Rust proof failed during compilation at {code}: {message}.",
                evidence=match.group(0),
                next_action=(
                    "Fix the Rust compiler error before rerunning the proof; this "
                    "row did not reach the intended runtime assertion."
                ),
                scopes=("runtime/", "tools/proof_queue.py"),
            )
        )

    match = RUNTIME_WASM_RUST_TARGET_MISSING_RE.search(log_tail)
    if match is not None:
        target = match.group("target")
        diagnostics.append(
            _diagnostic(
                signal_id="runtime-wasm-rust-target-missing",
                severity="infra",
                summary=(
                    "Runtime WASM build reached execution without Rust target "
                    f"{target} available."
                ),
                evidence=match.group(0),
                next_action=(
                    "Install the checked-in Rust toolchain target, then rerun "
                    "through the wasm proof-queue resource family. If a queued "
                    "wasm row reaches this after preflight, fix the queue "
                    "toolchain preflight or resource-family classification."
                ),
                scopes=(
                    "rust-toolchain.toml",
                    "src/molt/cli/wasm_toolchain.py",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = WASM_TOOLCHAIN_CONTRACT_IMPORT_MISSING_RE.search(log_tail)
    if match is not None:
        module = match.group("module")
        diagnostics.append(
            _diagnostic(
                signal_id="wasm-toolchain-contract-import-missing",
                severity="infra",
                summary=(
                    "WASM proof preflight could not import the toolchain "
                    f"contract because Python module {module} is missing."
                ),
                evidence=match.group(0),
                next_action=(
                    "Repair active uv/project provisioning before resubmitting "
                    "the WASM row; this failed before the proof command ran, so "
                    "do not treat it as product evidence or rerun a heavy build "
                    "unchanged."
                ),
                scopes=(
                    "tools/proof_queue.py",
                    "src/molt/cli/wasm_toolchain.py",
                    "pyproject.toml",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_NM_MISSING_RE.search(log_tail)
    if match is not None:
        object_path = Path(match.group("object"))
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-nm-missing",
                severity="infra",
                summary=(
                    "Source-extension object-symbol scan could not read "
                    f"{object_path.name} because canonical LLVM/WASI nm "
                    "authority was unavailable."
                ),
                evidence=match.group(0),
                next_action=(
                    "Repair or install the complete managed LLVM/WASI tool family "
                    "under MOLT_TARGET_ROOT; the compiler/linker/symbol-reader "
                    "family is one authority, not a per-command override."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "src/molt/cli/backend_cache.py",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_BUILD_PLAN_MISSING_RE.search(log_tail)
    if match is not None:
        source_plan_path = Path(match.group("path").strip())
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-build-plan-missing",
                severity="infra",
                summary=(
                    "Source-extension build could not find the declared "
                    f"source plan {source_plan_path.name}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Route this through source-extension package custody and "
                    "toolchain provisioning: derive Meson/Cython build metadata, "
                    "generated headers, include roots, and build-root resolution "
                    "from the package's own build system; do not hand-author "
                    "package metadata or rerun the same row unchanged."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_COMPILE_HEADER_MISSING_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-compile-header-missing",
                severity="infra",
                summary=(
                    "Source-extension compile could not resolve required header "
                    f"{match.group('header')!r} while compiling "
                    f"{match.group('source').strip()}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Fix the shared source-extension build-plan/provisioning "
                    "authority so generated headers and include roots are "
                    "derived from package build metadata and preserved in the "
                    "source plan; do not copy headers or patch compiler commands "
                    "by hand."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_CYTHON_REGENERATION_FAILED_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-cython-regeneration-failed",
                severity="infra",
                summary=(
                    "Source-extension Cython regeneration failed for "
                    f"{match.group('source').strip()}: "
                    f"{match.group('error').strip()}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Fix the shared source-extension Cython provisioning "
                    "authority so regeneration uses the package's declared "
                    "build metadata, generated dependency graph, include roots, "
                    "and toolchain configuration; do not add a package-specific "
                    "standalone Cython command."
                ),
                scopes=(
                    "src/molt/cli/source_extensions.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                    "tools/proof_queue.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_CIMPORT_HEADER_MISMATCH_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol") or "package C accessor"
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-cimport-header-mismatch",
                severity="infra",
                summary=(
                    "Source-extension compile used Cython pxd facts that do not "
                    f"match the C header include surface while compiling "
                    f"{match.group('source').strip()} ({symbol})."
                ),
                evidence=match.group("evidence"),
                next_action=(
                    "Keep cimport .pxd roots and package C header include roots "
                    "under the same build-interpreter package custody. Derive "
                    "both from source cimports and package build hooks; do not "
                    "pin an older Cython, copy package headers, or add a "
                    "package-specific source-plan/header overlay."
                ),
                scopes=(
                    "src/molt/cli/source_extension_cython.py",
                    "src/molt/cli/commands.py",
                    "docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_EXTENSION_CPYTHON_ABI_DECL_MISSING_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            _diagnostic(
                signal_id="source-extension-cpython-abi-declaration-missing",
                severity="error",
                summary=(
                    "Source-extension compile requires CPython ABI declaration "
                    f"{symbol}, but Molt's cpython-abi header does not expose it."
                ),
                evidence=match.group("evidence"),
                next_action=(
                    "Route to the cpython-abi owner to add the missing "
                    "declaration, macro, or helper as a shared C-API primitive. "
                    "Do not relax compiler diagnostics, pin an older Cython, or "
                    "patch the package source/source-plan around the missing ABI."
                ),
                scopes=(
                    "runtime/molt-cpython-abi/include/Python.h",
                    "runtime/molt-cpython-abi/",
                    "src/molt/cli/source_extensions.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = CPYTHON_ABI_PYMOD_GIL_SLOT_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="cpython-abi-pymod-gil-slot-token-mismatch",
                severity="error",
                summary=(
                    "CPython-ABI header exposes Py_MOD_PER_INTERPRETER_GIL_SUPPORTED "
                    "as an integer token where PyModuleDef_Slot.value expects a "
                    "pointer-shaped value."
                ),
                next_action=(
                    "Route to the cpython-abi owner to make the Py_mod_multiple_interpreters "
                    "slot token ABI-compatible as a reusable C-API primitive; "
                    "do not work around this in a package source-plan or compiler "
                    "command."
                ),
                scopes=(
                    "runtime/molt-cpython-abi/include/Python.h",
                    "runtime/molt-cpython-abi/",
                ),
                evidence=match.group("evidence"),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = STATIC_PYMOD_EXEC_RE.search(log_tail)
    if match is not None:
        module = match.group("module")
        detail = match.group("detail").strip(" ;")
        artifacts = tuple(
            match.group("path") for match in DIAGNOSTIC_JSON_RE.finditer(log_tail)
        )
        if detail:
            next_action = (
                "Fix the pending Python/C-API error surfaced by module exec, then "
                "rerun the same queue lane as a rerun edge."
            )
        else:
            next_action = (
                "Do not rerun the heavy lane until the module-exec primitive "
                "changes. Inspect the extension's Py_mod_exec body and route the "
                "missing C-API/ABI primitive through shared runtime authority."
            )
        if artifacts:
            next_action += " Start with the diagnostic_json artifact."
        diagnostics.append(
            _diagnostic(
                signal_id="static-pymodexec-nonzero",
                severity="error",
                summary=(
                    f"Static-linked extension module {module} reached Py_mod_exec "
                    "and returned non-zero."
                ),
                evidence=match.group(0),
                next_action=next_action,
                scopes=(
                    "runtime/molt-cpython-abi/",
                    "runtime/molt-runtime/src/cpython_abi_hooks.rs",
                    "src/molt/cli/external_native.py",
                ),
                artifacts=artifacts,
            )
        )

    match = RUNTIME_EXPORT_AUTHORITY_UNKNOWN_NAME_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            _diagnostic(
                signal_id="wasm-runtime-export-authority-unknown-name",
                severity="error",
                summary=(
                    "A required runtime export obligation is not declared by "
                    f"the generated WASM link authority: {symbol}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Declare the symbol through the generated WASM ABI link "
                    "authority (wasm_abi_manifest/gen_wasm_abi CPython ABI "
                    "surface), not by relaxing the export-name validator or "
                    "hand-editing generated files."
                ),
                scopes=(
                    "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml",
                    "tools/gen_wasm_abi.py",
                    "src/molt/_wasm_runtime_exports.py",
                ),
            )
        )

    match = RUNTIME_WASM_MISSING_EXPORTS_RE.search(log_tail)
    if match is not None:
        symbols = tuple(
            symbol.strip()
            for symbol in match.group("symbols").split(",")
            if symbol.strip()
        )
        listed = ", ".join(symbols[:6])
        if len(symbols) > 6:
            listed += f", ... (+{len(symbols) - 6} more)"
        diagnostics.append(
            _diagnostic(
                signal_id="runtime-wasm-missing-required-exports",
                severity="error",
                summary=(
                    "Runtime wasm build cannot satisfy required runtime "
                    f"exports: {listed or 'unlisted symbols'}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Thread the obligations through the shared runtime export "
                    "authority (wasm_runtime_shared_export_link_args plus the "
                    "generated WASM ABI manifest) and keep the defining archive "
                    "retained in the runtime build; do not hand-edit the "
                    "artifact or bypass export validation."
                ),
                scopes=(
                    "src/molt/_wasm_runtime_exports.py",
                    "src/molt/cli/runtime_build.py",
                    "runtime/molt-cpython-abi/build.rs",
                ),
            )
        )

    match = UNDEFINED_SYMBOL_RE.search(log_tail)
    if match is not None:
        symbol = match.group("symbol")
        diagnostics.append(
            _diagnostic(
                signal_id="native-undefined-symbol",
                severity="error",
                summary=f"Native/WASM link failed on unresolved symbol {symbol}.",
                evidence=match.group(0),
                next_action=(
                    "Add the symbol to the shared ABI/object-closure authority or "
                    "make package admission fail closed before link; do not patch "
                    "a package-local shim."
                ),
                scopes=(
                    "runtime/molt-cpython-abi/",
                    "src/molt/cli/external_native.py",
                    "tools/proof_queue.py",
                ),
            )
        )

    match = UNSUPPORTED_DIRECT_CALL_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="unsupported-direct-call",
                severity="error",
                summary="The compiler reached an unsupported direct-call boundary.",
                evidence=match.group(0),
                next_action=(
                    "Move the callable into package/import/native symbol closure "
                    "authority or fail closed at admission with this exact callable."
                ),
                scopes=("src/molt/cli/", "runtime/molt-backend-wasm/src/"),
            )
        )

    if "candidate_outputs.npz" in log_tail and any(
        token in log_tail.lower() for token in ("not found", "no such file", "missing")
    ):
        diagnostics.append(
            _diagnostic(
                signal_id="pact-candidate-output-missing",
                severity="error",
                summary="Pact acceptance did not produce candidate_outputs.npz.",
                evidence="candidate_outputs.npz was referenced with a missing-file signal",
                next_action=(
                    "Treat this as failed acceptance, not parity evidence. Use the "
                    "named pact-witness-acceptance lane after the structural fix."
                ),
                scopes=("tools/pact_witness_acceptance.py", "collab/pact/"),
            )
        )

    match = PACT_WITNESS_FIXTURE_MISSING_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="pact-witness-fixture-missing",
                severity="error",
                summary=(
                    "Pact acceptance failed after build/link because the Kernel A "
                    "fixture was not available to the run directory."
                ),
                evidence=match.group(0),
                next_action=(
                    "Make the acceptance runner regenerate the deterministic "
                    "fixture/reference oracle inside the run directory, then "
                    "rerun the named pact-witness-acceptance lane; do not check "
                    "binary fixture outputs into source."
                ),
                scopes=(
                    "tools/pact_witness_acceptance.py",
                    "collab/pact/pact_witness_kernel/make_fixture.py",
                    "collab/pact/pact_witness_kernel/field_solve.py",
                ),
            )
        )

    match = NATIVE_RUNTIME_IMPORT_CUSTODY_RE.search(log_tail)
    if match is not None:
        package = match.group("package")
        diagnostics.append(
            _diagnostic(
                signal_id="external-native-runtime-import-custody",
                severity="error",
                summary=(
                    f"Sealed external package {package} cannot prove runtime "
                    "Python imports because its manifest is missing "
                    "runtime_python_import_modules."
                ),
                evidence=match.group(0),
                next_action=(
                    "Reproduce the configured extension set from live upstream "
                    "Meson custody so the atomic seal persists "
                    "runtime_python_import_modules; do not rerun the heavy "
                    "pact-witness-acceptance lane until package admission passes."
                ),
                scopes=(
                    "src/molt/cli/external_native.py",
                    "src/molt/cli/extension_seal.py",
                    "src/molt/cli/source_extension_producer.py",
                    "tools/pact_seal_witness_roots.py",
                ),
            )
        )

    match = NATIVE_ARTIFACT_CUSTODY_RE.search(log_tail)
    if match is not None and not _diagnostics_have_signal(
        diagnostics, "external-native-runtime-import-custody"
    ):
        missing_abi_symbols = tuple(
            dict.fromkeys(
                symbol_match.group("symbol")
                for symbol_match in NATIVE_ARTIFACT_ABI_SURFACE_RE.finditer(
                    match.group("detail")
                )
            )
        )
        if missing_abi_symbols:
            listed = ", ".join(missing_abi_symbols[:6])
            if len(missing_abi_symbols) > 6:
                listed += f", ... (+{len(missing_abi_symbols) - 6} more)"
            diagnostics.append(
                _diagnostic(
                    signal_id="external-native-abi-link-surface-missing",
                    severity="error",
                    summary=(
                        "External native object closure requires runtime ABI "
                        f"link imports missing from the generated WASM surface: {listed}."
                    ),
                    evidence=match.group(0),
                    next_action=(
                        "Route the missing symbols through the generated WASM ABI "
                        "manifest/link-import authority and link validation; do not "
                        "paper over them with prefix admission or package-local shims."
                    ),
                    scopes=(
                        "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml",
                        "tools/gen_wasm_abi.py",
                        "src/molt/cli/external_native.py",
                        "tests/test_gen_wasm_abi.py",
                        "tests/test_wasm_link_validation.py",
                    ),
                )
            )
        else:
            diagnostics.append(
                _diagnostic(
                    signal_id="external-native-artifact-custody",
                    severity="error",
                    summary=(
                        "External native package admission failed because a declared "
                        "callable export is not backed by a native method, direct "
                        "symbol, or sealed provider module."
                    ),
                    evidence=match.group(0),
                    next_action=(
                        "Fix package-native object closure or provider-module custody; "
                        "do not rerun the heavy lane until the manifest/source authority "
                        "can prove the callable without a facade."
                    ),
                    scopes=(
                        "src/molt/cli/external_native.py",
                        "src/molt/cli/source_extensions.py",
                    ),
                )
            )

    match = NATIVE_SUPPORT_CUSTODY_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="external-native-support-custody",
                severity="error",
                summary=(
                    "Reachable native package support modules lack source or "
                    "artifact custody."
                ),
                evidence=match.group(0),
                next_action=(
                    "Publish reachable source-recompiled artifacts or sealed "
                    "source-plan custody for these support modules; package "
                    "visibility alone is not execution authority."
                ),
                scopes=(
                    "src/molt/cli/external_native.py",
                    "src/molt/cli/source_extensions.py",
                ),
            )
        )

    match = STDLIB_PROFILE_REFUSAL_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="stdlib-profile-refusal",
                severity="error",
                summary=(
                    f"Runtime feature {match.group('feature')} is reachable but "
                    f"excluded by profile {match.group('profile')}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Move the reached feature requirement through canonical "
                    "reachability/profile selection instead of broadening a profile "
                    "or hiding the missing feature in the proof command."
                ),
                scopes=(
                    "src/molt/cli/runtime_features.py",
                    "src/molt/cli/module_stdlib_policy.py",
                ),
            )
        )

    match = MOLT_RUNTIME_INVALID_OBJECT_HEADER_RE.search(log_tail)
    if match is not None:
        detail = match.group("detail").strip()
        site_match = re.search(r"\bin (?P<site>[A-Za-z_][A-Za-z0-9_]*)", detail)
        site = site_match.group("site") if site_match is not None else None
        diagnostics.append(
            _diagnostic(
                signal_id="molt-runtime-invalid-object-header",
                severity="error",
                summary=(
                    "Molt runtime aborted on an invalid object header"
                    + (f" in {site}" if site else "")
                    + "."
                ),
                evidence=match.group(0),
                next_action=(
                    "Treat this as runtime object-lifetime corruption, not a "
                    "generic pytest failure. Inspect the owning refcount/borrow "
                    "boundary named by the fatal site and rerun the same queue "
                    "lane only after that ownership bug changes."
                ),
                scopes=(
                    "runtime/molt-runtime/",
                    "runtime/molt-backend-native/src/",
                    "tools/proof_queue.py",
                ),
            )
        )

    match = SOURCE_LEASE_CHANGED_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="source-lease-changed-during-proof",
                severity="operator",
                summary=(
                    "A source file changed while the compiler was reading it; "
                    "the proof row is contaminated evidence."
                ),
                evidence=match.group(0),
                next_action=(
                    "Do not interpret downstream failures from this row as the "
                    "current product frontier. Let active edits settle, then "
                    "rerun the same queue lane from a stable git snapshot."
                ),
                scopes=(
                    match.group("module").strip(),
                    "tools/proof_queue.py",
                ),
            )
        )

    partial_module_match = PARTIAL_MODULE_PUBLICATION_RE.search(log_tail)
    if partial_module_match is not None:
        diff_fail_match = None
        for candidate in MOLT_DIFF_FAIL_RE.finditer(
            log_tail, 0, partial_module_match.start()
        ):
            diff_fail_match = candidate
        failing_case = (
            diff_fail_match.group("case") if diff_fail_match is not None else None
        )
        summary = (
            "Import failed because module "
            f"{partial_module_match.group('module')} was observed before publication."
        )
        evidence = partial_module_match.group(0)
        scopes = [
            "runtime/molt-runtime/src/builtins/module_table.rs",
            "runtime/molt-runtime/src/builtins/modules.rs",
            "src/molt/cli/backend_ir.py",
        ]
        if failing_case is not None:
            summary = f"{summary} Failing fixture: {failing_case}."
            evidence = f"{diff_fail_match.group(0)}\n{evidence}"
            scopes.insert(0, failing_case)
        diagnostics.append(
            _diagnostic(
                signal_id="import-partial-module-publication",
                severity="error",
                summary=summary,
                evidence=evidence,
                next_action=(
                    "Route this to the import/bootstrap module-state owner; do "
                    "not patch the frozen import layer from an unrelated lane."
                ),
                scopes=tuple(scopes),
            )
        )

    diff_fail_match = MOLT_DIFF_FAIL_RE.search(log_tail)
    if (
        diff_fail_match is not None
        and not _diagnostics_have_signal(
            diagnostics, "import-partial-module-publication"
        )
        and MEMORY_GUARD_TIMEOUT_RE.search(log_tail) is None
    ):
        failing_case = diff_fail_match.group("case").replace("\\", "/")
        stdout_lines = [
            f"{match.group('label')} stdout={match.group('value')}"
            for match in MOLT_DIFF_STDOUT_LINE_RE.finditer(log_tail)
        ]
        evidence_parts = [diff_fail_match.group(0)]
        if stdout_lines:
            evidence_parts.extend(stdout_lines[:2])
        diagnostics.append(
            _diagnostic(
                signal_id="molt-diff-output-mismatch",
                severity="error",
                summary=(
                    "molt_diff found a "
                    f"{diff_fail_match.group('detail')} in "
                    f"{failing_case} "
                    f"on {diff_fail_match.group('target')}."
                ),
                evidence="\n".join(evidence_parts),
                next_action=(
                    "Treat this as the current product frontier. Fix the "
                    "semantic authority named by the fixture, then rerun the "
                    "same queue lane instead of relabeling the row as infra."
                ),
                scopes=(failing_case, "tests/molt_diff.py"),
            )
        )

    match = PYTEST_ERROR_RE.search(log_tail)
    if match is not None:
        exception_line = PYTEST_EXCEPTION_LINE_RE.search(log_tail)
        detail = (
            exception_line.group("error")
            if exception_line is not None
            else (match.group("detail") or match.group(0))
        )
        diagnostics.append(
            _diagnostic(
                signal_id="pytest-error",
                severity="error",
                summary=f"Pytest proof errored while running {match.group('nodeid')}.",
                evidence=detail,
                next_action=(
                    "Fix the collection/import/setup error before interpreting "
                    "the proof lane; this row did not reach the protected assertion."
                ),
                scopes=("tests/", "tools/proof_queue.py"),
            )
        )

    match = PYTEST_FAILED_RE.search(log_tail)
    if match is not None:
        nodeid = match.group("nodeid")
        assertion = PYTEST_ASSERTION_RE.search(log_tail)
        detail = assertion.group("error") if assertion is not None else match.group(0)
        if nodeid.startswith(NATIVE_IMPORT_BOOTSTRAP_NODE_PREFIX):
            diagnostics.append(
                _diagnostic(
                    signal_id="native-call-lane-pytest-failure",
                    severity="error",
                    summary=(
                        "Native call-lane proof failed at "
                        f"{nodeid}; this lane is owned by the R1 integrator."
                    ),
                    evidence=detail,
                    next_action=(
                        "Route this row to the native call-lane owner. Do not patch "
                        "call/function.rs, fc/modules.rs, class_init.rs, containers, "
                        "exceptions, object/mod.rs, or the native import regression "
                        "test from an unrelated Codex lane."
                    ),
                    scopes=NATIVE_CALL_LANE_SCOPES,
                )
            )
        else:
            diagnostics.append(
                _diagnostic(
                    signal_id="pytest-failure",
                    severity="error",
                    summary=f"Pytest proof failed at {nodeid}.",
                    evidence=detail,
                    next_action=(
                        "Fix the failing test or the changed contract it protects, "
                        "then rerun the same focused queue lane."
                    ),
                    scopes=("tests/",),
                )
            )

    match = PYTHON_IMPORT_MISSING_RE.search(log_tail)
    if match is not None and not diagnostics:
        missing_module = match.group("module")
        diagnostics.append(
            _diagnostic(
                signal_id="proof-python-import-missing",
                severity="infra",
                summary=(
                    "Proof command used a Python environment missing import "
                    f"{missing_module}."
                ),
                evidence=match.group(0).replace("\\n", "\n"),
                next_action=(
                    "Run the proof command through RunContext/uv active project "
                    "provisioning, or fix the tool to launch its Molt CLI child "
                    "with the active project-environment Python. Do not hand-install "
                    "packages into an accidental host interpreter."
                ),
                scopes=(
                    "tools/proof_queue.py",
                    "tools/dx_build_timer.py",
                    "tools/run_context_env.py",
                    "pyproject.toml",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = PYTHON_EXCEPTION_RE.search(log_tail)
    if match is not None and not diagnostics:
        diagnostics.append(
            _diagnostic(
                signal_id="python-exception",
                severity="error",
                summary=(
                    f"Python proof command raised {match.group('type')}: "
                    f"{match.group('message').strip()}"
                ),
                evidence=match.group(0),
                next_action=(
                    "Inspect the traceback once, then either fix the product "
                    "failure or promote the recurring pattern into a narrower "
                    "queue diagnostic."
                ),
                scopes=("tools/proof_queue.py",),
            )
        )

    match = MEMORY_GUARD_TIMEOUT_RE.search(log_tail)
    if match is not None:
        pytest_context = _pytest_timeout_context(row["summary_json"])
        pytest_suffix = ""
        evidence = match.group(0)
        next_action_context = "the last active phase"
        if pytest_context is not None:
            nodeid, phase = pytest_context
            pytest_suffix = f" while pytest was in {nodeid}"
            if phase is not None:
                pytest_suffix += f" ({phase})"
            evidence += f" pytest_nodeid={nodeid}"
            if phase is not None:
                evidence += f" pytest_phase={phase}"
            next_action_context = f"{nodeid}"
        if (
            pytest_context is not None
            and nodeid.startswith(NATIVE_IMPORT_BOOTSTRAP_NODE_PREFIX)
        ):
            diagnostics.append(
                _diagnostic(
                    signal_id="native-call-lane-memory-guard-timeout",
                    severity="error",
                    summary=(
                        "Native call-lane proof timed out after "
                        f"{match.group('timeout')}s{pytest_suffix}; this lane is "
                        "owned by the R1 integrator."
                    ),
                    evidence=evidence,
                    next_action=(
                        "Route this timeout row to the native call-lane owner. "
                        "Treat it as incomplete evidence and do not rerun the same "
                        "shape unchanged from an unrelated Codex lane."
                    ),
                    scopes=(
                        "tools/memory_guard.py",
                        "tools/proof_queue.py",
                        *NATIVE_CALL_LANE_SCOPES,
                    ),
                    artifacts=(str(row["summary_json"]), str(row["log_path"])),
                )
            )
        else:
            diagnostics.append(
                _diagnostic(
                    signal_id="memory-guard-timeout",
                    severity="error",
                    summary=(
                        "Memory guard terminated the proof after "
                        f"{match.group('timeout')}s{pytest_suffix}."
                    ),
                    evidence=evidence,
                    next_action=(
                        "Treat this proof result as incomplete. Inspect "
                        f"{next_action_context} once, then reshape the proof, warm "
                        "the target dir, or raise --timeout only for intentional "
                        "long-running work."
                    ),
                    scopes=(
                        "tools/memory_guard.py",
                        "tools/proof_queue.py",
                    ),
                    artifacts=(str(row["summary_json"]), str(row["log_path"])),
                )
            )

    match = MEMORY_GUARD_ORPHANED_RE.search(log_tail)
    if match is not None:
        quarantine_match = MEMORY_GUARD_CARGO_QUARANTINE_RE.search(log_tail)
        evidence = match.group(0)
        artifacts: tuple[str, ...] = ()
        if quarantine_match is not None:
            receipt = quarantine_match.group("receipt").strip()
            evidence += f" cargo_quarantine_receipt={receipt}"
            artifacts = (receipt,)
        nested_guard = (
            "guarded_exec:" in log_tail
            or "MOLT_TEST_SUITE guarded command" in log_tail
        )
        diagnostics.append(
            _diagnostic(
                signal_id=(
                    "nested-memory-guard-orphan-cleanup"
                    if nested_guard
                    else "memory-guard-orphan-cleanup"
                ),
                severity="warning",
                summary=(
                    "Nested guarded_exec memory guard cleaned up orphaned child "
                    "processes after its guarded command exited."
                    if nested_guard
                    else "Memory guard cleaned up orphaned child processes after "
                    "the proof command exited."
                ),
                evidence=evidence,
                next_action=(
                    "Preserve the proof result, then harden the nested guarded "
                    "command lifecycle or move intentional warm daemons inside a "
                    "suite sentinel that drains at scope exit."
                    if nested_guard
                    else "Preserve the proof result, then harden the child process "
                    "lifecycle or run intentional warm daemons inside a suite "
                    "sentinel that drains at scope exit."
                ),
                scopes=(
                    "tools/guarded_exec.py",
                    "tools/memory_guard.py",
                    "tools/proof_queue.py",
                ),
                artifacts=artifacts,
            )
        )

    incomplete_memory_guard = _finished_incomplete_memory_guard_diagnostic(row)
    if incomplete_memory_guard is not None:
        diagnostics.insert(0, incomplete_memory_guard)
    worker_exit_without_summary = (
        _finished_worker_exit_without_summary_diagnostic(row)
    )
    if worker_exit_without_summary is not None:
        diagnostics.insert(0, worker_exit_without_summary)

    if row["status"] == "failed" and not diagnostics:
        last = _last_nonempty_log_line(Path(row["log_path"])) or ""
        diagnostics.append(
            _diagnostic(
                signal_id="unclassified-failed-proof",
                severity="unknown",
                summary="The proof failed without a recognized queue diagnostic.",
                evidence=last,
                next_action=(
                    "Inspect the log tail once, then add a deterministic diagnosis "
                    "rule before this failure pattern becomes tribal knowledge."
                ),
                scopes=("tools/proof_queue.py",),
            )
        )
    return diagnostics
