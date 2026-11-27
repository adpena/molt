#!/usr/bin/env python3
from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
from enum import Enum

import json
import math
import sys
from pathlib import Path
from typing import Literal, cast

from tools.perf_schema import (
    CLASS_DIMENSIONAL_WIN,
    CLASS_GREEN,
    CLASS_INFRA,
    CLASS_RED_NOISY,
    CLASS_RED_STABLE,
    CLASS_TIE,
    PlannedCell,
    flatten_cells,
    output_parity_passes,
    planned_cell_from_observation,
    validate_board,
)
from tools.perf_scoreboard_model import (
    SCOREBOARD_DIR,
    VERDICT_BUILD_FAILED,
    VERDICT_CPY_INCOMPAT,
    VERDICT_FAIL_COLD_BUDGET,
    VERDICT_FAIL_ENGINE,
    VERDICT_FAIL_STALE,
    VERDICT_GREEN,
    VERDICT_RUN_BLOCKED,
    VERDICT_RUN_ERROR,
    VERDICT_UNSTABLE,
    VERDICT_WARN_COLD_FLOOR,
    Cell,
    PhaseStats,
    ScoreboardSchemaError,
    _VERDICT_DERIVED_NOTES,
    _budget_ms_for,
    _load_cold_start_budgets,
    _robust_cell_stable,
    _warm_speedup_ci,
)


AGENT_SIGNAL_KIND: Literal["molt.benchmark-signals.v1"] = "molt.benchmark-signals.v1"


class SignalKind(str, Enum):
    OUTPUT_MISMATCH = "output_mismatch"
    BUILD_FAILURE = "build_failure"
    RUNTIME_FAILURE = "runtime_failure"
    MEASUREMENT_FAILURE = "measurement_failure"
    UNSUPPORTED = "unsupported"
    PERFORMANCE_LOSS = "performance_loss"
    QUALIFICATION = "qualification"
    MISSING_COVERAGE = "missing_coverage"


class SignalSeverity(str, Enum):
    CORRECTNESS = "correctness"
    DIAGNOSTIC = "diagnostic"
    PERFORMANCE = "performance"
    QUALIFICATION = "qualification"
    COVERAGE = "coverage"


class SignalConfidence(str, Enum):
    OBSERVED = "observed"
    QUALIFIED = "qualified_snapshot"
    UNQUALIFIED = "unqualified"


class SignalAction(str, Enum):
    REPRODUCE_OUTPUT = "reproduce_output"
    INSPECT_BUILD = "inspect_build"
    INSPECT_RUNTIME = "inspect_runtime"
    REPAIR_MEASUREMENT = "repair_measurement"
    REVIEW_SUPPORT = "review_support"
    PROFILE_LOSS = "profile_loss"
    QUALIFY_MEASUREMENT = "qualify_measurement"
    RUN_MISSING = "run_missing"


@dataclass(frozen=True)
class AgentSignal:
    kind: SignalKind
    severity: SignalSeverity
    confidence: SignalConfidence
    action_code: SignalAction
    finding: str
    action: str
    coordinate: PlannedCell | None
    evidence_pointer: str
    log_artifact: str | None
    warm_speedup: float | None = None
    repeat_median_warm: float | None = None

    def as_record(self) -> dict[str, object]:
        """Project closed internal facts onto the existing JSON boundary."""
        return {
            "kind": self.kind.value,
            "severity": self.severity.value,
            "confidence": self.confidence.value,
            "action_code": self.action_code.value,
            "finding": self.finding,
            "action": self.action,
            "coordinate": self.coordinate.as_record() if self.coordinate else None,
            "evidence_pointer": self.evidence_pointer,
            "log_artifact": self.log_artifact,
            "warm_speedup": self.warm_speedup,
            "repeat_median_warm": self.repeat_median_warm,
        }


@dataclass(frozen=True)
class AgentSignalCoverage:
    selected: int
    attempted: int
    parity_qualified: int
    measured: int
    release_qualified: int
    not_run: tuple[PlannedCell, ...]

    def as_record(self) -> dict[str, object]:
        return {
            "selected": self.selected,
            "attempted": self.attempted,
            "parity_qualified": self.parity_qualified,
            "measured": self.measured,
            "release_qualified": self.release_qualified,
            "not_run": len(self.not_run),
        }


@dataclass(frozen=True)
class AgentSignalReport:
    evidence_path: str
    generated_at: str
    source_revisions: tuple[tuple[str, str], ...]
    methodology_schema: str | None
    methodology_version: int | None
    methodology_sha256: str | None
    methodology_locked: bool
    qualification_problems: tuple[str, ...]
    coverage: AgentSignalCoverage
    signals: tuple[AgentSignal, ...]
    total: int
    omitted: int

    def as_record(self) -> dict[str, object]:
        return {
            "kind": AGENT_SIGNAL_KIND,
            "snapshot": {
                "evidence_path": self.evidence_path,
                "generated_at": self.generated_at,
                "source_revisions": dict(self.source_revisions),
                "methodology_schema": self.methodology_schema,
                "methodology_version": self.methodology_version,
                "methodology_sha256": self.methodology_sha256,
                "methodology_locked": self.methodology_locked,
                "qualification_problems": list(self.qualification_problems),
                "scope": "retained_snapshot",
                "current_main_proof": False,
            },
            "coverage": self.coverage.as_record(),
            "signals": [signal.as_record() for signal in self.signals],
            "total": self.total,
            "omitted": self.omitted,
        }


def _signal_number(value: object) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    try:
        number = float(value)
    except OverflowError:
        return None
    return number if math.isfinite(number) else None


def _signal_mapping(value: object) -> Mapping[str, object]:
    # Stored JSON objects are open at this boundary; facts are closed on projection.
    return cast(Mapping[str, object], value) if isinstance(value, Mapping) else {}


def _signal_text(value: object) -> str | None:
    return value if isinstance(value, str) else None


def _signal_pointer(
    doc: Mapping[str, object], coordinate: PlannedCell, cell: Mapping[str, object]
) -> str:
    def escape(value: str) -> str:
        return value.replace("~", "~0").replace("/", "~1")

    keys = (
        coordinate.benchmark,
        coordinate.target,
        coordinate.backend,
        coordinate.profile,
    )
    node: object = doc.get("scoreboard")
    for key in keys:
        node = _signal_mapping(node).get(key)
    if node is not cell:
        raise ScoreboardSchemaError(
            "signals require coordinate-bound evidence pointers",
            ["scoreboard nesting does not match the recorded cell coordinate"],
        )
    return "/scoreboard/" + "/".join(escape(value) for value in keys)


def _signal_repeat_problems(cell: Mapping[str, object]) -> tuple[str, ...]:
    passes = cell.get("repeat_passes")
    ratios = cell.get("repeat_warm_speedups")
    if (
        type(passes) is not int
        or passes < 2
        or not isinstance(ratios, list)
        or len(ratios) != passes
        or any(
            (number := _signal_number(ratio)) is None or number <= 0 for ratio in ratios
        )
    ):
        return (
            "repeat evidence needs at least two matching finite positive pass ratios",
        )
    lo = _signal_number(cell.get("repeat_ci_lo"))
    hi = _signal_number(cell.get("repeat_ci_hi"))
    if lo is None or hi is None or lo > hi:
        return ("repeat confidence interval is missing, nonfinite, or reversed",)
    try:
        _, _, expected_lo, expected_hi = _warm_speedup_ci(cast(list[float], ratios))
    except (OverflowError, ValueError):
        return (
            "repeat confidence interval cannot be represented by the recorded ratios",
        )
    if lo != expected_lo or hi != expected_hi:
        return ("repeat confidence interval disagrees with recorded pass ratios",)
    return ()


def _signal_cell_qualification(cell: Mapping[str, object]) -> tuple[str, ...]:
    problems: list[str] = []
    if cell.get("verdict") == VERDICT_FAIL_STALE:
        problems.append("stale cell evidence requires remeasurement")
    if any(
        cell.get(field) is not True for field in ("build_ok", "molt_ok", "cpython_ok")
    ):
        problems.append("build and both runtimes must succeed")
    parity = _signal_mapping(cell.get("output_parity"))
    if parity.get("reference_runtime") != "cpython" or not output_parity_passes(parity):
        problems.append("exact passed CPython parity is required")
    if cell.get("stable") is not True or cell.get("measured_quiescent") is not True:
        problems.append("stable quiescent observations are required")
    ratio = _signal_number(cell.get("warm_speedup"))
    if ratio is None or ratio <= 0:
        problems.append("a finite positive CPython/Molt point ratio is required")
    problems.extend(_signal_repeat_problems(cell))
    return tuple(problems)


def _signal_output_mismatch(parity: Mapping[str, object]) -> bool:
    if not (
        parity.get("checked") is True
        and parity.get("reference_runtime") == "cpython"
        and parity.get("reference_stable") is True
        and parity.get("molt_stable") is True
        and parity.get("reference_returncode") == 0
        and parity.get("ok") is False
    ):
        return False
    return any(
        parity.get(f"{axis}_match") is False
        and parity.get(f"reference_{axis}_sha256") != parity.get(f"molt_{axis}_sha256")
        for axis in ("stdout", "stderr")
    ) or (parity.get("exit_match") is False and parity.get("molt_returncode") != 0)


def _signal_sort_key(signal: AgentSignal) -> tuple[int, float, PlannedCell, str]:
    rank = tuple(SignalSeverity).index(signal.severity)
    return (
        rank,
        signal.repeat_median_warm
        if signal.repeat_median_warm is not None
        else signal.warm_speedup
        if signal.warm_speedup is not None
        else math.inf,
        signal.coordinate or PlannedCell("<board>", "native", "native", "<board>"),
        signal.kind.value,
    )


def build_agent_signals(
    doc: Mapping[str, object],
    *,
    evidence_path: str,
    limit: int = 8,
    expected_source_sha: str | None = None,
) -> AgentSignalReport:
    """Read retained evidence once; never probe Git or infer a temporal regression."""
    from tools.perf_authority import (
        DEFAULT_STALE_DAYS,
        doc_age_days,
        perf_methodology_problems,
        scoreboard_revision_fields,
    )
    from tools.git_identity import is_git_object_id

    if type(limit) is not int or limit <= 0:
        raise ValueError("signal limit must be a positive integer")
    problems = validate_board(doc)
    if problems:
        sampled = [
            problem if len(problem) <= 240 else problem[:237] + "..."
            for problem in problems[:3]
        ]
        if len(problems) > 3:
            sampled.append(f"... {len(problems) - 3} more schema problems")
        raise ScoreboardSchemaError(
            "signals require a current-schema retained scoreboard", sampled
        )
    provenance = _signal_mapping(doc.get("provenance"))
    revisions = scoreboard_revision_fields(doc)
    recorded_sources = (doc.get("git_rev"), provenance.get("local_head_sha"))
    if len({revision for _, revision in revisions}) > 1:
        raise ValueError(
            "signals source identities disagree: "
            + ", ".join(field for field, _ in revisions)
        )
    if expected_source_sha is not None:
        if not is_git_object_id(expected_source_sha):
            raise ValueError("expected source must be a full lowercase Git SHA")
        if any(revision != expected_source_sha for revision in recorded_sources):
            raise ValueError(
                "retained scoreboard source does not match expected source "
                + expected_source_sha
            )
    board_problems = perf_methodology_problems(provenance)
    if provenance.get("authoritative") is not True:
        board_problems.append("snapshot is not authoritative")
    if any(
        not isinstance(revision, str) or not is_git_object_id(revision)
        for revision in recorded_sources
    ):
        board_problems.append(
            "both recorded source fields must name full Git object ids"
        )
    if provenance.get("dirty_tree") is not False:
        board_problems.append("snapshot records a dirty or unknown source tree")
    origin = provenance.get("origin_sha")
    if not isinstance(origin, str) or not is_git_object_id(origin):
        board_problems.append("snapshot lacks a full recorded origin revision")
    if (
        provenance.get("merge_base_sha") != provenance.get("local_head_sha")
        or provenance.get("diverges_from_origin") is True
    ):
        board_problems.append(
            "snapshot source lineage contradicts authoritative admission"
        )
    age = doc_age_days(_signal_text(doc.get("generated_at")))
    if age is None or age < 0 or age > DEFAULT_STALE_DAYS:
        board_problems.append("snapshot timestamp is missing, future, or stale")
    quiescence = _signal_mapping(provenance.get("quiescence"))
    if quiescence.get("quiet") is not True:
        board_problems.append("snapshot does not record machine quiescence")
    raw_coverage = _signal_mapping(doc.get("coverage"))
    missing = tuple(
        PlannedCell.from_payload(_signal_mapping(value))
        for value in cast(list[object], raw_coverage["not_run"])
    )
    coverage = AgentSignalCoverage(
        selected=cast(int, raw_coverage["selected"]),
        attempted=cast(int, raw_coverage["attempted"]),
        parity_qualified=cast(int, raw_coverage["parity_qualified"]),
        measured=cast(int, raw_coverage["measured"]),
        release_qualified=cast(int, raw_coverage["release_qualified"]),
        not_run=missing,
    )
    signals: list[AgentSignal] = []
    if board_problems:
        signals.append(
            AgentSignal(
                SignalKind.QUALIFICATION,
                SignalSeverity.QUALIFICATION,
                SignalConfidence.UNQUALIFIED,
                SignalAction.QUALIFY_MEASUREMENT,
                "; ".join(board_problems),
                "Remeasure this source with a locked methodology on a quiet host.",
                None,
                "/provenance",
                None,
            )
        )
    for cell in flatten_cells(doc):
        coordinate = planned_cell_from_observation(cell)
        pointer = _signal_pointer(doc, coordinate, cell)
        log = _signal_text(cell.get("log_artifact"))
        ratio = _signal_number(cell.get("warm_speedup"))
        parity = _signal_mapping(cell.get("output_parity"))
        failure = _signal_mapping(cell.get("molt_failure"))
        status = failure.get("status")
        # A single primary finding per coordinate prevents duplicate axis causes.
        kind: SignalKind
        severity = SignalSeverity.DIAGNOSTIC
        confidence = SignalConfidence.OBSERVED
        action_code: SignalAction
        finding: str
        action: str
        if (
            cell.get("molt_failure_timed_out") is True
            or cell.get("molt_failure_guard_violation") is not None
            or failure.get("infrastructure_failure") is not None
            or status
            in (
                "timeout",
                "infrastructure_error",
                "rss_limit_exceeded",
                "orphaned_processes_cleaned",
            )
        ):
            kind, action_code = (
                SignalKind.MEASUREMENT_FAILURE,
                SignalAction.REPAIR_MEASUREMENT,
            )
            finding = "Guard, timeout, or measurement infrastructure failure; compiler cause unproven."
            action = "Inspect the failure receipt and repair measurement admission before comparing performance."
        elif cell.get("run_blocked") is True:
            kind, action_code = SignalKind.UNSUPPORTED, SignalAction.REVIEW_SUPPORT
            finding = "Execution was unsupported, blocked, or refused by policy."
            action = "Check the declared support contract and recorded refusal."
        elif cell.get("build_ok") is not True:
            kind, action_code = SignalKind.BUILD_FAILURE, SignalAction.INSPECT_BUILD
            finding = "Molt build failed; root cause is not established."
            action = (
                "Inspect the build failure receipt and log, then reproduce the build."
            )
        elif cell.get("verdict") == VERDICT_CPY_INCOMPAT:
            kind, action_code = (
                SignalKind.QUALIFICATION,
                SignalAction.QUALIFY_MEASUREMENT,
            )
            severity, confidence = (
                SignalSeverity.QUALIFICATION,
                SignalConfidence.UNQUALIFIED,
            )
            finding = "Molt-only output contract has no CPython performance floor."
            action = (
                "Use the declared Molt contract; do not infer a CPython win or loss."
            )
        elif (
            cell.get("cpython_ok") is False and cell.get("molt_ok") is True
        ) or parity.get("reference_stable") is False:
            kind, action_code = (
                SignalKind.MEASUREMENT_FAILURE,
                SignalAction.REPAIR_MEASUREMENT,
            )
            finding = "CPython reference failed or produced unstable output; no compiler correctness claim."
            action = "Restore a successful stable CPython reference before comparison."
        elif _signal_output_mismatch(parity):
            kind, action_code = (
                SignalKind.OUTPUT_MISMATCH,
                SignalAction.REPRODUCE_OUTPUT,
            )
            severity = SignalSeverity.CORRECTNESS
            finding = "Checked stable CPython output differs from compiled output."
            action = "Reproduce the recorded output mismatch and minimize a differential case."
            pointer += "/output_parity"
        elif (
            cell.get("molt_ok") is not True or cell.get("verdict") == VERDICT_RUN_ERROR
        ):
            kind, action_code = SignalKind.RUNTIME_FAILURE, SignalAction.INSPECT_RUNTIME
            finding = (
                "Molt run or output comparison failed; root cause is not established."
            )
            action = "Inspect the run receipt and observable output before diagnosing a compiler defect."
        else:
            local_problems = _signal_cell_qualification(cell)
            hi = _signal_number(cell.get("repeat_ci_hi"))
            lo = _signal_number(cell.get("repeat_ci_lo"))
            loss = (ratio is not None and ratio < 1) or (hi is not None and hi < 1)
            if (
                not board_problems
                and not local_problems
                and cell.get("classification") == CLASS_RED_STABLE
                and cell.get("repeat_stability") == "STABLE_BELOW"
                and hi is not None
                and hi < 1
            ):
                kind, action_code = (
                    SignalKind.PERFORMANCE_LOSS,
                    SignalAction.PROFILE_LOSS,
                )
                severity, confidence = (
                    SignalSeverity.PERFORMANCE,
                    SignalConfidence.QUALIFIED,
                )
                finding = f"Qualified snapshot fresh-process loss: CPython/Molt point={ratio:.3g}x; CI upper={hi:.3g}."
                action = "Profile this workload and test an attributed optimization with an interleaved A/B."
            elif (
                not board_problems
                and not local_problems
                and cell.get("classification") == CLASS_GREEN
                and cell.get("repeat_stability") == "STABLE_ABOVE"
                and lo is not None
                and lo > 1
                and cell.get("verdict") == VERDICT_GREEN
            ):
                continue
            else:
                kind, action_code = (
                    SignalKind.QUALIFICATION,
                    SignalAction.QUALIFY_MEASUREMENT,
                )
                severity, confidence = (
                    SignalSeverity.QUALIFICATION,
                    SignalConfidence.UNQUALIFIED,
                )
                if loss:
                    finding = f"Observed fresh-process loss CPython/Molt={_fmt(ratio)}x requires qualification."
                else:
                    finding = "Cell comparison requires qualification before optimization claims."
                if local_problems:
                    finding += " " + "; ".join(local_problems) + "."
                action = "Collect successful parity-checked, quiet, stable repeat evidence with a decisive confidence interval."
                # The board signal already explains shared qualification gaps.
                if (
                    board_problems
                    and not local_problems
                    and not loss
                    and cell.get("verdict") == VERDICT_GREEN
                ):
                    continue
        median = None
        if confidence is SignalConfidence.QUALIFIED:
            median, _, _, _ = _warm_speedup_ci(
                cast(list[float], cell["repeat_warm_speedups"])
            )
        signals.append(
            AgentSignal(
                kind,
                severity,
                confidence,
                action_code,
                finding,
                action,
                coordinate,
                pointer,
                log,
                ratio,
                median,
            )
        )
    for index, coordinate in enumerate(missing):
        signals.append(
            AgentSignal(
                SignalKind.MISSING_COVERAGE,
                SignalSeverity.COVERAGE,
                SignalConfidence.OBSERVED,
                SignalAction.RUN_MISSING,
                "Declared coordinate was not run; no timing or correctness observation exists.",
                "Run this selected coordinate under the recorded methodology.",
                coordinate,
                f"/coverage/not_run/{index}",
                None,
            )
        )
    signals.sort(key=_signal_sort_key)
    return AgentSignalReport(
        evidence_path=evidence_path,
        generated_at=cast(str, doc["generated_at"]),
        source_revisions=revisions,
        methodology_schema=_signal_text(provenance.get("methodology_schema")),
        methodology_version=cast(int, provenance["methodology_version"])
        if type(provenance.get("methodology_version")) is int
        else None,
        methodology_sha256=_signal_text(provenance.get("methodology_sha256")),
        methodology_locked=provenance.get("methodology_locked") is True,
        qualification_problems=tuple(board_problems),
        coverage=coverage,
        signals=tuple(signals[:limit]),
        total=len(signals),
        omitted=max(0, len(signals) - limit),
    )


def print_agent_signals(
    report: AgentSignalReport, *, json_output: bool = False
) -> None:
    if json_output:
        print(json.dumps(report.as_record(), indent=2))
        return
    print("AGENT SIGNALS — retained snapshot; no current-main proof")
    print(f"Evidence: {report.evidence_path}  generated_at={report.generated_at}")
    print(
        "Source: "
        + (
            ", ".join(
                f"{field}={revision}" for field, revision in report.source_revisions
            )
            or "unknown"
        )
    )
    print(
        f"Methodology: {report.methodology_schema} v{report.methodology_version} {report.methodology_sha256} locked={report.methodology_locked}"
    )
    print(
        f"Coverage: selected={report.coverage.selected} attempted={report.coverage.attempted} measured={report.coverage.measured} not_run={len(report.coverage.not_run)}"
    )
    print(
        f"Signals: total={report.total} shown={len(report.signals)} omitted={report.omitted}"
    )
    for signal in report.signals:
        coordinate = signal.coordinate
        label = (
            f"{coordinate.benchmark} [{coordinate.target}/{coordinate.backend}/{coordinate.profile}]"
            if coordinate
            else "board"
        )
        print(
            f"[{signal.severity.value}/{signal.confidence.value}] {label}: {signal.finding}"
        )
        print(f"  {signal.action_code.value}: {signal.action}")
        print(f"  #{signal.evidence_pointer} log={signal.log_artifact or '-'}")


def _validate_board_for_emit(doc: dict, *, context: str) -> None:
    problems = validate_board(doc)
    if problems:
        raise ScoreboardSchemaError(context, problems)


def _write_scoreboard_doc(
    path: Path,
    doc: dict,
    *,
    context: str,
    validate_inputs: Callable[[], None] | None = None,
) -> None:
    _write_scoreboard_doc_atomic(
        path,
        doc,
        context=context,
        validate_inputs=validate_inputs,
    )


def _write_scoreboard_doc_atomic(
    path: Path,
    doc: dict,
    *,
    context: str,
    validate_inputs: Callable[[], None] | None = None,
) -> None:
    _validate_board_for_emit(doc, context=context)
    from molt.file_publication import atomic_write_bytes, durable_replace

    def commit(staged: Path, destination: Path) -> None:
        if validate_inputs is not None:
            validate_inputs()
        durable_replace(staged, destination)

    atomic_write_bytes(
        path, (json.dumps(doc, indent=2) + "\n").encode("utf-8"), replace=commit
    )


def _print_schema_error(exc: ScoreboardSchemaError) -> None:
    print(f"[schema] {exc.context} FAILED:", file=sys.stderr)
    for problem in exc.problems:
        print(f"    - {problem}", file=sys.stderr)


def print_summary(doc: dict) -> None:
    cells = _flatten_cells(doc)
    by_verdict: dict[str, list[dict]] = {}
    for c in cells:
        by_verdict.setdefault(c.get("verdict", "pending"), []).append(c)

    # --- Authoritative-tree header (council ruling A + B) -------------------
    prov = doc.get("provenance", {})
    authoritative = prov.get("authoritative", True)
    print("\n" + "=" * 100)
    print("CPYTHON FLOOR SCOREBOARD — two-dimensional (warm ≠ cold)")
    print(
        f"  origin/main = {_short(prov.get('origin_sha'))}   "
        f"local HEAD = {_short(prov.get('local_head_sha'))}   "
        f"methodology = v{prov.get('methodology_version')} "
        f"{_short(prov.get('methodology_sha256'))}"
    )
    print(
        f"  cpython={doc['host']['cpython_baseline']}  "
        f"pypy={doc['host'].get('pypy') or '-'}  "
        f"codon={doc['host'].get('codon') or '-'}"
    )
    # --- Quiescence line (#69 Rule 2) --------------------------------------
    q = prov.get("quiescence") or {}
    if q:
        if q.get("quiet"):
            print(
                f"  QUIESCENT: load={q.get('loadavg_1m')} (<= {q.get('loadavg_threshold')}"
                f", ncpu={q.get('ncpu')})  runnable={q.get('runnable_signal')}  "
                f"builds=0  thermal={'ok' if q.get('thermal_ok') else q.get('thermal_ok')}"
            )
        else:
            print(f"  *** NOT QUIESCENT: {'; '.join(q.get('reasons', []))} ***")
            if prov.get("require_quiescent"):
                print(
                    "      NON-AUTHORITATIVE: machine not quiet; do not optimize "
                    "from this red list (EXPLORATORY only)"
                )
    if authoritative:
        print(
            "  AUTHORITATIVE: revision on origin/main, clean, tool unmodified, machine quiescent"
        )
    else:
        print("  *** WARNING: benchmark is NON-AUTHORITATIVE ***")
        print(f"      reason: {prov.get('authoritative_reason', 'unknown')}")
    print("=" * 100)

    # --- Full table (verdict-ordered) --------------------------------------
    rank = {
        VERDICT_FAIL_STALE: 0,
        VERDICT_FAIL_ENGINE: 1,
        VERDICT_BUILD_FAILED: 1,
        VERDICT_RUN_ERROR: 1,
        VERDICT_UNSTABLE: 2,
        VERDICT_FAIL_COLD_BUDGET: 3,
        VERDICT_WARN_COLD_FLOOR: 4,
        VERDICT_RUN_BLOCKED: 5,
        VERDICT_CPY_INCOMPAT: 5,
        VERDICT_GREEN: 6,
    }
    cells.sort(
        key=lambda c: (
            rank.get(c.get("verdict"), 7),
            -(c.get("warm_speedup") or 0.0),
            c["benchmark"],
        )
    )
    hdr = (
        f"{'VERDICT':<17}{'REPEAT':>7}  {'FIRST':>7}  {'DELTAms':>7}  "
        f"{'PYPY':>6}  {'CODON':>6}  {'SIZEKiB':>8}  BENCHMARK [backend/profile]"
    )
    print("TIMING SCOPE: fresh process per sample; cache state uncontrolled.")
    print("FIRST = first invocation; REPEAT = median after discarded launches.")
    print(
        "DELTAms = first minus repeat; includes noise and cache effects, "
        "not isolated startup."
    )
    print(hdr)
    print("-" * 100)
    for c in cells:
        print(
            f"{c.get('verdict', '?'):<17}"
            f"{_fmt(c.get('warm_speedup')):>7}  "
            f"{_fmt(c.get('cold_speedup')):>7}  "
            f"{_fmt(c.get('startup_tax_ms'), 0):>7}  "
            f"{_fmt(c.get('pypy_ratio')):>6}  "
            f"{_fmt(c.get('codon_ratio')):>6}  "
            f"{_fmt(c.get('binary_size_kib'), 0):>8}  "
            f"{c['benchmark']} [{c['backend']}/{c['profile']}]"
        )
    print("-" * 100)

    from tools.perf_authority import scoreboard_release_eligibility

    admission = scoreboard_release_eligibility(doc)
    print(
        "STATISTICAL COMPARISON; canonical-core E2 eligible="
        + str(admission["eligible"])
    )
    for problem in admission["problems"]:
        print("E2 INELIGIBLE: " + problem)
    s = doc["summary"]
    coverage = doc.get("coverage")
    if coverage is not None:
        print(
            f"COVERAGE: selected={coverage['selected']} "
            f"attempted={coverage['attempted']} "
            f"parity_qualified={coverage['parity_qualified']} "
            f"measured={coverage['measured']} "
            f"release_qualified={coverage['release_qualified']}"
        )
        for coordinate in coverage["not_run"]:
            print(
                f"UNRUN: {coordinate['benchmark']} "
                f"[{coordinate['target']}/{coordinate['backend']}/{coordinate['profile']}]"
            )
    print(
        f"TOTAL={s['cells_total']}  GREEN={s['cells_green']}  "
        f"FAIL_ENGINE={s.get('cells_fail_engine', 0)}  "
        f"FAIL_COLD_BUDGET={s.get('cells_fail_cold_budget', 0)}  "
        f"WARN_COLD_FLOOR={s.get('cells_warn_cold_floor', 0)}  "
        f"UNSTABLE={s['cells_unstable']}  BUILD_FAIL={s['cells_build_failed']}  "
        f"RUN_ERROR={s['cells_error']}  CPY_INCOMPAT={s.get('cells_cpython_incompatible', 0)}  "
        f"STALE={s.get('cells_fail_stale', 0)}"
    )

    # --- Repeated fresh-process losses ------------------------------------
    warm_reds = by_verdict.get(VERDICT_FAIL_ENGINE, [])
    print(f"\nREPEATED FRESH-PROCESS LOSSES ({len(warm_reds)}) — FAIL_ENGINE:")
    print(
        "  (timing includes startup; attribution fields are hypotheses requiring a profile)"
    )
    for c in sorted(warm_reds, key=lambda c: c.get("warm_speedup") or 0.0):
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  hypothesis: {c.get('suspected_missing_fact', '?')}"
        )

    # --- First-minus-repeat budget failures -------------------------------
    cold_reds = by_verdict.get(VERDICT_FAIL_COLD_BUDGET, [])
    print(
        f"\nFIRST-MINUS-REPEAT BUDGET FAILURES ({len(cold_reds)}) "
        "— observed delta over budget:"
    )
    for c in sorted(cold_reds, key=lambda c: -(c.get("startup_tax_ms") or 0.0)):
        print(
            f"    first={_fmt(c.get('cold_speedup'))}x  delta={_fmt(c.get('startup_tax_ms'), 0)}ms"
            f" (budget {_fmt(c.get('cold_budget_ms'), 0)}ms)  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  -> {c.get('suspected_startup_component', '?')}"
        )

    # --- WARN_COLD_FLOOR (cold<=1 but warm>1, tax within budget) -----------
    warn_cold = by_verdict.get(VERDICT_WARN_COLD_FLOOR, [])
    if warn_cold:
        print(
            f"\nFIRST-INVOCATION WARNINGS ({len(warn_cold)}) "
            "— repeated processes win, first invocation loses "
            "with observed delta within budget (cause unproven):"
        )
        for c in sorted(warn_cold, key=lambda c: -(c.get("startup_tax_ms") or 0.0))[
            :12
        ]:
            print(
                f"    first={_fmt(c.get('cold_speedup'))}x  repeat={_fmt(c.get('warm_speedup'))}x"
                f"  delta={_fmt(c.get('startup_tax_ms'), 0)}ms  {c['benchmark']} "
                f"[{c['backend']}/{c['profile']}]"
            )
        if len(warn_cold) > 12:
            print(
                f"    ... and {len(warn_cold) - 12} more (full list in JSON verdict_breakdown)"
            )

    # --- BACKEND ERRORS / NON-AUTHORITATIVE --------------------------------
    errs = (
        by_verdict.get(VERDICT_BUILD_FAILED, [])
        + by_verdict.get(VERDICT_RUN_ERROR, [])
        + by_verdict.get(VERDICT_UNSTABLE, [])
    )
    stale = by_verdict.get(VERDICT_FAIL_STALE, [])
    if errs or stale:
        print(f"\nBACKEND ERRORS / NON-AUTHORITATIVE ({len(errs) + len(stale)}):")
        for c in errs:
            origin_rerun = "yes" if not authoritative else "no"
            failure = _molt_failure_summary(c)
            print(
                f"    {c.get('verdict'):<16} {c['benchmark']} [{c['backend']}/{c['profile']}]"
                f"  stale?={'yes' if not authoritative else 'no'}  "
                f"origin_rerun_needed?={origin_rerun}"
                + (f"  ({c.get('note')})" if c.get("note") else "")
                + (f"  failure={failure}" if failure else "")
            )
        for c in stale[:5]:
            print(
                f"    FAIL_STALE       {c['benchmark']} [{c['backend']}/{c['profile']}]"
                "  stale?=yes  origin_rerun_needed?=yes"
            )
        if len(stale) > 5:
            print(
                f"    ... and {len(stale) - 5} more stale cells (whole board non-authoritative)"
            )

    # --- REGRESSIONS FROM LAST GREEN (filled by the baseline-diff caller) ---
    regressions = doc.get("_regressions_from_last_green")
    if regressions:
        print(f"\nREGRESSIONS FROM LAST GREEN ({len(regressions)}):")
        for m in regressions:
            print(f"    {m}")

    # --- GREENS WORTH PROTECTING (>2x — do not reopen a won class) ----------
    greens = by_verdict.get(VERDICT_GREEN, [])
    protected = sorted(
        (c for c in greens if (c.get("warm_speedup") or 0.0) > 2.0),
        key=lambda c: -(c.get("warm_speedup") or 0.0),
    )
    print(f"\nGREENS WORTH PROTECTING ({len(protected)}) — won classes, do NOT reopen:")
    for c in protected:
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]"
        )

    # --- 5-STATE CLASSIFICATION (#69 --classify) ---------------------------
    summary = doc.get("summary", {})
    if summary.get("classify_active"):
        by_class: dict[str, list[dict]] = {}
        for c in cells:
            cls = c.get("classification")
            if cls:
                by_class.setdefault(cls, []).append(c)
        cb = summary.get("classification_breakdown", {})
        print(
            f"\n5-STATE CLASSIFICATION (#69): RED_STABLE={len(cb.get(CLASS_RED_STABLE, []))}  "
            f"RED_NOISY={len(cb.get(CLASS_RED_NOISY, []))}  TIE={len(cb.get(CLASS_TIE, []))}  "
            f"GREEN={len(cb.get(CLASS_GREEN, []))}  "
            f"DIMENSIONAL_WIN={len(cb.get(CLASS_DIMENSIONAL_WIN, []))}  "
            f"INFRA={len(cb.get(CLASS_INFRA, []))}"
        )
        red_stable = sorted(
            by_class.get(CLASS_RED_STABLE, []),
            key=lambda c: c.get("warm_speedup") or 0.0,
        )
        print(
            f"\n  CONFIRMED FRESH-PROCESS LOSSES — RED_STABLE ({len(red_stable)}) "
            "[quiescent + stable + CI below 1.00; profile to attribute cost]:"
        )
        for c in red_stable:
            cp = c.get("cycle_profile") or {}
            top = cp.get("top_symbols") or []
            cyc = (
                f" -> CYCLES top: {top[0]['symbol']}"
                if top
                else (f" -> {cp.get('note')}" if cp.get("note") else "")
            )
            print(
                f"    {_fmt(c.get('warm_speedup'))}x  "
                f"CI=[{_fmt(c.get('repeat_ci_lo'))},{_fmt(c.get('repeat_ci_hi'))}]  "
                f"{c['benchmark']} [{c['backend']}/{c['profile']}]{cyc}"
            )
        noisy = by_class.get(CLASS_RED_NOISY, [])
        if noisy:
            print(
                f"\n  RED_NOISY ({len(noisy)}) — unconfirmed comparison, "
                "contaminated/unstable/missing repeat CI — re-measure quiet:"
            )
            for c in sorted(noisy, key=lambda c: c.get("warm_speedup") or 0.0)[:20]:
                print(
                    f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
                    f"[{c['backend']}/{c['profile']}]  ({c.get('classification_reason')})"
                )
        ties = by_class.get(CLASS_TIE, [])
        if ties:
            print(f"\n  TIE ({len(ties)}) — CI crosses 1.00 (neither win nor loss):")
            for c in sorted(ties, key=lambda c: c["benchmark"])[:20]:
                print(
                    f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
                    f"[{c['backend']}/{c['profile']}]"
                )
        dims = by_class.get(CLASS_DIMENSIONAL_WIN, [])
        if dims:
            print(
                f"\n  DIMENSIONAL_WIN ({len(dims)}) — Rule 4 (no warm flip, dimension improved):"
            )
            for c in dims:
                print(
                    f"    {c['benchmark']} [{c['backend']}/{c['profile']}]  "
                    f"({c.get('classification_reason')})"
                )

    # --- FASTEST NEXT UNLOCK -----------------------------------------------
    unlock = _fastest_next_unlock(warm_reds, cold_reds)
    print(f"\nFASTEST NEXT UNLOCK: {unlock}")

    if doc["benchmarks_deferred"]:
        print(f"\nDEFERRED / CPY-INCOMPATIBLE ({len(doc['benchmarks_deferred'])}):")
        for d in doc["benchmarks_deferred"][:8]:
            print(f"  - {d['benchmark']}: {d['reason']}")
        if len(doc["benchmarks_deferred"]) > 8:
            print(f"  ... and {len(doc['benchmarks_deferred']) - 8} more")
    print("=" * 100 + "\n")


def _short(sha: str | None) -> str:
    if not sha:
        return "-"
    return sha[:12]


def _fastest_next_unlock(warm_reds: list[dict], cold_reds: list[dict]) -> str:
    """Suggest cost attribution for the largest recorded process comparison gap."""
    if warm_reds:
        worst = min(warm_reds, key=lambda c: c.get("warm_speedup") or 1e9)
        return (
            f"profile fresh-process cost for {worst['benchmark']} "
            f"[{worst['backend']}/{worst['profile']}] "
            f"({_fmt(worst.get('warm_speedup'))}x) — hypothesis: "
            f"{worst.get('suspected_missing_fact', '?')}"
        )
    if cold_reds:
        worst = max(cold_reds, key=lambda c: c.get("startup_tax_ms") or 0.0)
        return (
            f"profile first-minus-repeat delta: {worst['benchmark']} "
            f"delta={_fmt(worst.get('startup_tax_ms'), 0)}ms "
            f"— {worst.get('suspected_startup_component', 'cause unproven')}"
        )
    return "no reds — protect the greens; widen the suite for the next class"


def _molt_failure_summary(cell: dict) -> str | None:
    status = cell.get("molt_failure_status")
    detail = cell.get("molt_failure_detail")
    message = cell.get("molt_failure_message")
    parts = [str(value) for value in (status, detail) if value]
    if message:
        text = str(message).replace("\n", " ")
        parts.append(text[:160] + ("..." if len(text) > 160 else ""))
    return " | ".join(parts) if parts else None


def _flatten_cells(doc: dict) -> list[dict]:
    return [dict(cell) for cell in flatten_cells(doc)]


def _fmt(v: float | None, places: int = 2) -> str:
    if v is None:
        return "-"
    if places == 0:
        return f"{v:.0f}"
    return f"{v:.{places}f}"


def diff_against_baseline(
    doc: dict, baseline_path: Path
) -> tuple[list[str], list[str]]:
    """Return (newly_red, regressed_still_green) message lists vs a prior board."""
    try:
        prior = json.loads(baseline_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return ([f"baseline unreadable: {exc}"], [])

    prior_cells = {_cell_key(c): c for c in _flatten_cells(prior)}
    newly_red: list[str] = []
    regressed: list[str] = []
    for c in _flatten_cells(doc):
        key = _cell_key(c)
        old = prior_cells.get(key)
        if old is None:
            continue
        from tools.perf_schema import cells_comparable

        if not cells_comparable(c, old):
            newly_red.append(
                f"{key}: INCOMPARABLE workload or execution host; no regression comparison"
            )
            continue
        new_ratio = c.get("warm_speedup")
        old_ratio = old.get("warm_speedup")
        # "Newly gating" = a green-or-warn cell that became a hard gate fail.
        from tools.perf_authority import release_cell_problems

        was_green = not release_cell_problems(old)
        now_fails = bool(release_cell_problems(c))
        if now_fails and was_green:
            newly_red.append(
                f"{key}: NEWLY {c.get('verdict', 'RED')}  "
                f"{_fmt(old_ratio)} -> {_fmt(new_ratio)}"
            )
        elif (
            new_ratio is not None
            and old_ratio is not None
            and not now_fails
            and new_ratio < old_ratio * 0.95  # >5% slower but still passing
        ):
            regressed.append(
                f"{key}: regressed-but-passing  {_fmt(old_ratio)} -> {_fmt(new_ratio)} "
                f"({(new_ratio / old_ratio - 1) * 100:+.1f}%)"
            )
    return newly_red, regressed


def _cell_key(c: dict) -> str:
    return f"{c['benchmark']} [{c['backend']}/{c['profile']}]"


def _latest_baseline(exclude: Path | None = None) -> Path | None:
    """The most recent committed board, EXCLUDING in-progress ``.partial.json``
    checkpoints and an optional explicit path (the board being written now).

    Without the ``.partial`` exclusion a diff would compare a board against its
    own mid-sweep checkpoint (a near-self-diff that hides every regression).
    """
    if not SCOREBOARD_DIR.exists():
        return None
    exclude_resolved = exclude.resolve() if exclude is not None else None
    candidates = [
        p
        for p in sorted(SCOREBOARD_DIR.glob("cpython_*.json"))
        if not p.name.endswith(".partial.json")
        and (exclude_resolved is None or p.resolve() != exclude_resolved)
    ]
    return candidates[-1] if candidates else None


def _gate_exit_code(
    doc: dict,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
    """Fail unless every required cell demonstrates a repeated CPython win.

    ``no_gate`` is an explicit exploratory opt-out, never release evidence.
    Historical summary flags cannot override the measured cells.
    """
    if no_gate:
        return 0
    from tools.perf_authority import scoreboard_release_eligibility

    if not scoreboard_release_eligibility(doc)["eligible"]:
        return 1
    cells = _flatten_cells(doc)
    if not cells or doc.get("benchmarks_deferred"):
        return 1
    provenance = doc.get("provenance", {})
    if not allow_nonauthoritative and provenance.get("authoritative") is not True:
        return 1
    return 0


def _finalize_with_board_context(
    cells: list[Cell], doc_like: dict, *, allow_nonauthoritative: bool = False
) -> None:
    """Re-finalize stored cells using budgets + the board's own authoritative flag.

    For rebuild/merge we re-run the classifier so a stored board reflects the
    CURRENT verdict logic. The cold-start budget comes from the live budget
    file; the authoritative flag comes from the stored provenance (a stored
    board does not re-derive authoritativeness — it was already stamped).
    ``allow_nonauthoritative`` mirrors the run path: a non-authoritative board's
    cells classify on their REAL numbers (not FAIL_STALE) so a reader can
    re-derive verdicts for local analysis; the board's stored
    ``authoritative=false`` is untouched. We also RE-DERIVE ``stable`` from the
    stored per-runtime stats so a board measured by an older tool picks up the
    current robust-stability rule without re-running any benchmark.
    """
    budgets = _load_cold_start_budgets()
    stored_auth = doc_like.get("provenance", {}).get("authoritative", True)
    effective_auth = stored_auth or allow_nonauthoritative
    for cell in cells:
        # Drop any verdict-DERIVED note (FAIL_STALE / robustness) before
        # re-deriving so a stale note from a prior finalize (e.g. a board that
        # was once stamped FAIL_STALE) does not leak into the new verdict.
        if cell.note in _VERDICT_DERIVED_NOTES or (
            cell.note and cell.note.startswith("non-authoritative tree")
        ):
            cell.note = None
        _rederive_stability(cell)
        cell.finalize(
            budget_ms=_budget_ms_for(budgets, cell.backend, cell.profile),
            authoritative=effective_auth,
        )


def _rederive_stability(cell: Cell) -> None:
    """Recompute ``cell.stable`` from the stored molt/cpython stats dicts.

    ``finalize`` does not recompute stability (it is set at measurement time),
    so a rebuild-summary/merge must re-derive it to apply the current robust
    rule. No-op if the stored stats are absent.
    """
    if not cell.molt_stats or not cell.cpython_stats:
        return
    molt = _phasestats_from_dict(cell.molt_stats)
    cpy = _phasestats_from_dict(cell.cpython_stats)
    if molt is None or cpy is None:
        return
    cell.stable = _robust_cell_stable(molt, cpy)


def _phasestats_from_dict(d: dict) -> PhaseStats | None:
    import dataclasses

    if not isinstance(d, dict):
        return None
    known = {f.name for f in dataclasses.fields(PhaseStats)}
    return PhaseStats(**{k: v for k, v in d.items() if k in known})


def _proc_summary(procs: object) -> str:
    """One-line summary of a build-process list (pid:exe pairs)."""
    if not isinstance(procs, list) or not procs:
        return "0"
    parts = []
    for p in procs[:6]:
        if isinstance(p, dict):
            cmd = (p.get("cmd") or "").split()
            parts.append(f"{p.get('pid')}:{cmd[0] if cmd else '?'}")
    suffix = ", ".join(parts)
    extra = f" (+{len(procs) - 6} more)" if len(procs) > 6 else ""
    return f"{len(procs)} [{suffix}{extra}]"


def _print_provenance(provenance: dict) -> None:
    """Emit the FULL provenance block (#69 --print-provenance).

    Prints every field a reader needs to certify (or reject) a board's
    authority: the origin/candidate SHAs, dirty/daemon/cache identity, the
    cold/warm + repeat/variance posture, AND the council's NEW quiescence
    fields (``active_molt_processes``, ``active_cargo_or_rustc_processes``,
    ``loadavg_1m``, ``ncpu``, ``runnable_signal``). This is the human-auditable
    twin of the JSON provenance block; it does not re-measure anything.
    """
    p = provenance
    q = p.get("quiescence") or {}
    print("\n" + "=" * 100)
    print("PROVENANCE (full) — #69 measurement-hygiene block")
    print("=" * 100)
    # --- Tree / artifact identity (council ruling A) -----------------------
    print("  [tree identity]")
    print(f"    origin_sha (origin/main)     = {p.get('origin_sha')}")
    print(f"    candidate_sha (local HEAD)   = {p.get('local_head_sha')}")
    print(f"    merge_base_sha               = {p.get('merge_base_sha')}")
    print(f"    dirty_tree                   = {p.get('dirty_tree')}")
    print(f"    diverges_from_origin         = {p.get('diverges_from_origin')}")
    print(f"    methodology_version          = {p.get('methodology_version')}")
    print(f"    methodology_sha256           = {p.get('methodology_sha256')}")
    print(f"    methodology_locked           = {p.get('methodology_locked')}")
    print("  [backend_binary_identity (daemon / stale-cache guard)]")
    bbi = p.get("backend_binary_identity") or {}
    if bbi:
        for lane, ident in sorted(bbi.items()):
            print(f"    {lane:<24} = {ident}")
    else:
        print("    (none recorded)")
    print("  [runtime_identity (measured runtime library per lane and native variant)]")
    runtimes = p.get("runtime_identity") or {}
    if runtimes:
        for lane, ident in sorted(runtimes.items()):
            print(f"    {lane:<24} = {ident}")
    else:
        print("    (none recorded)")
    # --- Quiescence (#69 Rule 2) — the NEW fields, named explicitly ---------
    print("  [quiescence (#69 Rule 2)]")
    print(f"    require_quiescent            = {p.get('require_quiescent')}")
    print(f"    quiescent                    = {p.get('quiescent')}")
    print(
        f"    active_molt_processes        = {_proc_summary(p.get('active_molt_processes'))}"
    )
    print(
        "    active_cargo_or_rustc_processes = "
        f"{_proc_summary(p.get('active_cargo_or_rustc_processes'))}"
    )
    print(f"    loadavg_1m                   = {p.get('loadavg_1m')}")
    print(f"    loadavg_threshold            = {q.get('loadavg_threshold')}")
    print(f"    ncpu                         = {p.get('ncpu')}")
    print(f"    runnable_signal              = {p.get('runnable_signal')}")
    print(
        f"    thermal_ok                   = {q.get('thermal_ok')}  ({q.get('thermal_note')})"
    )
    if q.get("reasons"):
        print(f"    NON-QUIET reasons            = {'; '.join(q.get('reasons', []))}")
    # --- Authority verdict --------------------------------------------------
    print("  [authority]")
    print(f"    authoritative                = {p.get('authoritative')}")
    print(f"    authoritative_reason         = {p.get('authoritative_reason')}")
    print("=" * 100)


def _checkpoint(
    path: Path,
    cells: list[Cell],
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
    *,
    planned_cells: tuple[PlannedCell, ...],
    provenance: dict | None = None,
    cpython_identity: dict | None = None,
    pypy_version: str | None = None,
    codon_version: str | None = None,
    execution_host: dict | None = None,
    validate_inputs: Callable[[], None] | None = None,
) -> None:
    from tools.perf_scoreboard import build_scoreboard_doc

    checkpoint_provenance = dict(provenance or {})
    checkpoint_provenance.update(
        authoritative=False,
        authoritative_reason="partial checkpoint; source custody remains open",
    )
    doc = build_scoreboard_doc(
        cells,
        planned_cells=planned_cells,
        run_state="partial",
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=samples,
        warmup=warmup,
        provenance=checkpoint_provenance,
        cpython_identity=cpython_identity,
        pypy_version=pypy_version,
        codon_version=codon_version,
        execution_host=execution_host,
    )
    _write_scoreboard_doc_atomic(
        path,
        doc,
        context=f"checkpoint {path}",
        validate_inputs=validate_inputs,
    )
