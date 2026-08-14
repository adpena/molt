"""Runtime, Python, pytest, differential, and memory-guard rules."""

from __future__ import annotations

import re
import sqlite3
from pathlib import Path

from tools.proof_queue_pkg.diagnostic_evidence import (
    _finished_incomplete_memory_guard_diagnostic,
    _finished_worker_exit_without_summary_diagnostic,
    _last_nonempty_log_line,
    _pytest_timeout_context,
)
from tools.proof_queue_pkg.diagnostic_model import (
    _diagnostic,
    _diagnostics_have_signal,
)

MOLT_RUNTIME_INVALID_OBJECT_HEADER_RE = re.compile(
    r"(?m)^molt fatal: invalid object header(?P<detail>[^\r\n]*)"
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


def _append_diagnostics(
    row: sqlite3.Row,
    log_tail: str,
    diagnostics: list[dict[str, object]],
) -> None:
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
        if pytest_context is not None and nodeid.startswith(
            NATIVE_IMPORT_BOOTSTRAP_NODE_PREFIX
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
            "guarded_exec:" in log_tail or "MOLT_TEST_SUITE guarded command" in log_tail
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
    worker_exit_without_summary = _finished_worker_exit_without_summary_diagnostic(row)
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
