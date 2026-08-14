"""Ordered proof diagnostic classifier orchestration and queue preflight rules."""

from __future__ import annotations

import json
import re
import sqlite3
from pathlib import Path

from tools.proof_queue_pkg import (
    diagnostic_build_rules,
    diagnostic_link_rules,
    diagnostic_runtime_rules,
)
from tools.proof_queue_pkg.diagnostic_evidence import (
    _first_log_line_containing,
    _last_nonempty_log_line,
    _read_log_tail,
    _running_child_missing_diagnostic,
    _running_pytest_current_test_missing_diagnostic,
    _running_pytest_failures_observed_diagnostic,
)
from tools.proof_queue_pkg.diagnostic_model import _diagnostic

QUEUE_COLD_SINGLE_CARGO_PROOF_RE = re.compile(
    r"proof queue refuses cold-prone single-test Cargo proofs "
    r"\('(?P<filter>[^']+)' under --lib\)"
)
EXECUTION_CUSTODY_FAILURE_RE = re.compile(
    r"proof_queue execution custody: (?P<error>[^\r\n]+)"
)

SOURCE_BUILD_ENVIRONMENT_CUSTODY_MISSING_RE = re.compile(
    r"source build environment is not pre-provisioned from locked custody; "
    r"the producer never mutates its active interpreter\. Missing or "
    r"out-of-range requirements: (?P<requirements>[^\r\n]+)"
)

SOURCE_BUILD_CONSOLE_SCRIPT_PATH_CUSTODY_RE = re.compile(
    r"Unknown compiler\(s\): \[\['cython'\], \['cython3'\]\]"
    r"[\s\S]{0,4096}?Running `cython -V` gave [^\r\n]{0,1024}?"
    r"(?:WinError 2|No such file or directory)"
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

    custody_failure = EXECUTION_CUSTODY_FAILURE_RE.search(log_tail)
    if custody_failure is not None:
        try:
            receipt_context = json.loads(row["receipt_context_json"] or "{}")
        except (TypeError, json.JSONDecodeError):
            receipt_context = {}
        source_custody = receipt_context.get("source_custody", {})
        supervisor = receipt_context.get("process_supervisor", {})
        supervisor_receipt = (
            supervisor.get("receipt", {}) if isinstance(supervisor, dict) else {}
        )
        ineligible = (
            source_custody.get("ineligible_reasons", [])
            if isinstance(source_custody, dict)
            else []
        )
        violations = (
            supervisor_receipt.get("violations", [])
            if isinstance(supervisor_receipt, dict)
            else []
        )
        evidence_parts = [custody_failure.group(0)]
        if isinstance(ineligible, list) and ineligible:
            evidence_parts.append("ineligible=" + ",".join(map(str, ineligible)))
        if isinstance(violations, list) and violations:
            evidence_parts.append("violations=" + "; ".join(map(str, violations)))
        artifacts = [str(row["log_path"])]
        for candidate in (
            supervisor.get("receipt_file") if isinstance(supervisor, dict) else None,
            receipt_context.get("live_input_custody", {}).get("event_artifact")
            if isinstance(receipt_context.get("live_input_custody"), dict)
            else None,
        ):
            if isinstance(candidate, dict) and isinstance(candidate.get("path"), str):
                artifacts.append(str(candidate["path"]))
        unadmitted_images = [
            str(violation)
            for violation in violations
            if "unadmitted executable image" in str(violation).casefold()
        ]
        if unadmitted_images:
            diagnostics.append(
                _diagnostic(
                    signal_id="native-process-image-unadmitted",
                    severity="infra",
                    summary=(
                        "The native supervisor rejected an executable outside the "
                        "immutable process-image authority."
                    ),
                    evidence="; ".join(unadmitted_images),
                    next_action=(
                        "Classify the executable as a structurally required exact image, "
                        "capture its absolute path and content digest before arming, and "
                        "revalidate it after execution. Never admit by basename or directory."
                    ),
                    scopes=(
                        "tools/proof_queue_pkg/process_image_capture.py",
                        "tools/proof_queue_pkg/guarded_execution.py",
                        "tools/proof_supervisor/",
                    ),
                    artifacts=tuple(dict.fromkeys(artifacts)),
                )
            )
        else:
            diagnostics.append(
                _diagnostic(
                    signal_id="queue-execution-custody-failure",
                    severity="infra",
                    summary=(
                        "The proof command did not produce admissible execution custody."
                    ),
                    evidence=" ".join(evidence_parts),
                    next_action=(
                        "Inspect the durable custody events and supervisor violations, "
                        "then fix the control-plane output or executable authority before "
                        "rerunning. This row is infrastructure evidence, not product proof."
                    ),
                    scopes=(
                        "tools/proof_queue_pkg/guarded_execution.py",
                        "tools/proof_queue_pkg/execution_custody.py",
                        "tools/memory_guard.py",
                        "tools/proof_supervisor/",
                    ),
                    artifacts=tuple(dict.fromkeys(artifacts)),
                )
            )

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

    match = SOURCE_BUILD_ENVIRONMENT_CUSTODY_MISSING_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="source-build-environment-custody-missing",
                severity="infra",
                summary=(
                    "Source-extension production lacked the locked build "
                    f"environment for {match.group('requirements').strip()}."
                ),
                evidence=match.group(0),
                next_action=(
                    "Rerun the typed extension produce-set command: Molt now "
                    "provisions and attests the configured dependency group under "
                    "canonical custody before re-executing there. Do not install "
                    "these requirements into an ambient project interpreter."
                ),
                scopes=(
                    "src/molt/cli/source_build_environment.py",
                    "src/molt/cli/source_extension_producer.py",
                    "pyproject.toml",
                    "uv.lock",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    match = SOURCE_BUILD_CONSOLE_SCRIPT_PATH_CUSTODY_RE.search(log_tail)
    if match is not None:
        diagnostics.append(
            _diagnostic(
                signal_id="source-build-console-script-path-custody",
                severity="infra",
                summary=(
                    "Meson could not resolve the attested source-build "
                    "environment's Cython console script."
                ),
                evidence=match.group(0),
                next_action=(
                    "Put the locked environment's Scripts/bin directory first "
                    "in the producer child's executable PATH, then rerun with a "
                    "fresh build root. Never install Cython into an ambient "
                    "interpreter or pin an older version to mask PATH custody."
                ),
                scopes=(
                    "src/molt/cli/source_extension_producer.py",
                    "src/molt/cli/source_build_environment.py",
                    "tests/cli/test_source_extension_producer.py",
                ),
                artifacts=(str(row["summary_json"]), str(row["log_path"])),
            )
        )

    diagnostic_build_rules._append_diagnostics(
        row,
        log_tail,
        diagnostics,
        fatal_queue_failure=fatal_queue_failure,
    )
    diagnostic_link_rules._append_diagnostics(row, log_tail, diagnostics)
    diagnostic_runtime_rules._append_diagnostics(row, log_tail, diagnostics)
    return diagnostics
