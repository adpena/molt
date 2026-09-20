#!/usr/bin/env python3
"""Schema authority for Molt performance scoreboard artifacts.

The measurement runner in ``perf_scoreboard.py`` owns execution. This module
owns the durable JSON contract that board projections, CI gates, history tools,
and tests can import without importing the full runner.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any, cast

from compat.comparison import (
    COMPARISON_LAW_VERSION,
    ComparisonMode,
    ExitLaw,
    Outputs,
    Verdict,
    compare_outputs,
)

SCHEMA_VERSION = 4
RED_THRESHOLD = 1.00
UNSTABLE_CV = 0.20

VERDICT_GREEN = "GREEN"
VERDICT_FAIL_ENGINE = "FAIL_ENGINE"
VERDICT_FAIL_COLD_BUDGET = "FAIL_COLD_BUDGET"
VERDICT_WARN_COLD_FLOOR = "WARN_COLD_FLOOR"
VERDICT_FAIL_STALE = "FAIL_STALE"
VERDICT_BUILD_FAILED = "BUILD_FAILED"
VERDICT_RUN_ERROR = "RUN_ERROR"
VERDICT_UNSTABLE = "UNSTABLE"
VERDICT_RUN_BLOCKED = "RUN_BLOCKED"
VERDICT_CPY_INCOMPAT = "CPY_INCOMPATIBLE"

CLASS_RED_STABLE = "RED_STABLE"
CLASS_RED_NOISY = "RED_NOISY"
CLASS_TIE = "TIE"
CLASS_GREEN = "GREEN_STABLE"
CLASS_DIMENSIONAL_WIN = "DIMENSIONAL_WIN"
CLASS_INFRA = "INFRA"

CLASSIFY_STATES = frozenset(
    {
        CLASS_RED_STABLE,
        CLASS_RED_NOISY,
        CLASS_TIE,
        CLASS_GREEN,
        CLASS_DIMENSIONAL_WIN,
        CLASS_INFRA,
    }
)

FACT_CLASSES = frozenset(
    {
        "op_kinds",
        "operand_ownership",
        "finalizer_sensitive",
        "call_facts",
        "typed_callable_target",
        "shape_facts",
        "ownership_lattice",
        "exception_region",
        "repr_tir_type_lattice",
        "class_identity_version",
    }
)

PYPY_ADVANTAGE_CLASSES = frozenset(
    {
        "ic_tiering",
        "class_version_guard",
        "borrow_inference",
        "generator_fusion",
        "shape_propagation",
        "loop_specialization",
    }
)

REFERENCE_CLASSES = frozenset(
    {
        "dynamic",
        "static_equiv",
        "numeric",
        "io",
        "molt_internal",
    }
)

CODON_SEMANTICS = frozenset(
    {
        "equivalent",
        "non_equivalent",
        "n/a",
    }
)

GATE_FAILING_VERDICTS = frozenset(
    {
        VERDICT_FAIL_ENGINE,
        VERDICT_FAIL_COLD_BUDGET,
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_UNSTABLE,
    }
)


def verdict_fails_gate(verdict: str, *, fail_stale: bool = True) -> bool:
    """Return whether a verdict is a hard gate failure for the current policy."""

    return verdict in GATE_FAILING_VERDICTS or (
        fail_stale and verdict == VERDICT_FAIL_STALE
    )


REQUIRED_TOP_LEVEL_KEYS = frozenset(
    {
        "schema_version",
        "kind",
        "generated_at",
        "git_rev",
        "provenance",
        "host",
        "direction",
        "red_threshold",
        "verdict_legend",
        "methodology",
        "reserved_columns",
        "summary",
        "benchmarks_run",
        "benchmarks_deferred",
        "scoreboard",
    }
)

REQUIRED_PROVENANCE_FIELDS = frozenset(
    {
        "origin_sha",
        "local_head_sha",
        "merge_base_sha",
        "dirty_tree",
        "benchmark_tool_sha",
        "backend_binary_identity",
        "stdlib_cache_key",
        "authoritative",
    }
)

REQUIRED_HOST_FIELDS = frozenset(
    {
        "platform",
        "python_runner",
        "cpython_baseline",
    }
)

MODERN_HOST_FIELDS = frozenset(
    {
        "machine",
        "arch",
        "pointer_bits",
        "cpython_oracle",
    }
)

REQUIRED_CPYTHON_ORACLE_FIELDS = frozenset(
    {
        "cmd",
        "executable",
        "implementation",
        "version",
        "sys_platform",
        "machine",
        "arch",
        "pointer_bits",
    }
)

REQUIRED_SUMMARY_FIELDS = frozenset(
    {
        "cells_fail_engine",
        "cells_fail_cold_budget",
        "cells_warn_cold_floor",
        "cells_fail_stale",
        "verdict_breakdown",
        "gate_fails",
    }
)

REQUIRED_CELL_FIELDS = frozenset(
    {
        "benchmark",
        "target",
        "backend",
        "profile",
        "build_ok",
        "run_blocked",
        "molt_ok",
        "cpython_ok",
        "cold_molt_s",
        "cold_cpython_s",
        "warm_molt_s",
        "warm_cpython_s",
        "warm_speedup",
        "cold_speedup",
        "startup_tax_ms",
        "verdict",
        "binary_size_kib",
        "molt_peak_rss_mib",
        "compile_time_s",
        "stable",
        "pypy_ratio",
        "codon_ratio",
        "codon_equivalent",
        "cpython_peak_rss_mib",
        "output_parity",
        "log_artifact",
    }
)

REQUIRED_OUTPUT_PARITY_FIELDS = frozenset(
    {
        "checked",
        "ok",
        "reference_runtime",
        "comparison_law_version",
        "mode",
        "stderr_mode",
        "exit_law",
        "reason",
        "detail",
        "stdout_match",
        "stderr_match",
        "exit_match",
        "reference_returncode",
        "molt_returncode",
        "reference_stdout_sha256",
        "molt_stdout_sha256",
        "reference_stderr_sha256",
        "molt_stderr_sha256",
        "reference_stable",
        "molt_stable",
        "reference_observation_count",
        "molt_observation_count",
        "mismatch_observation",
        "mismatch_observation_sha256",
    }
)

OUTPUT_PARITY_COMPARISON_LAW_VERSION = COMPARISON_LAW_VERSION


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8", "surrogatepass")).hexdigest()


OutputObservation = tuple[str, str | None, str | None, int | None]


def _observation_hash(stdout: str, stderr: str, returncode: int) -> str:
    payload = json.dumps(
        [stdout, stderr, returncode], ensure_ascii=False, separators=(",", ":")
    )
    return _sha256_text(payload)


def output_parity_evidence(
    *,
    reference_observations: Sequence[OutputObservation],
    molt_observations: Sequence[OutputObservation],
) -> dict[str, object]:
    """Build a stable, hash-only receipt via the shared parity law."""

    base: dict[str, object] = {
        "checked": False,
        "ok": None,
        "reference_runtime": "cpython",
        "comparison_law_version": COMPARISON_LAW_VERSION,
        "mode": ComparisonMode.EXACT.value,
        "stderr_mode": "exact",
        "exit_law": ExitLaw.EXACT.value,
        "reason": "comparison_unavailable",
        "detail": "cold-run observable output was not fully captured",
        "stdout_match": None,
        "stderr_match": None,
        "exit_match": None,
        "reference_returncode": None,
        "molt_returncode": None,
        "reference_stdout_sha256": None,
        "molt_stdout_sha256": None,
        "reference_stderr_sha256": None,
        "molt_stderr_sha256": None,
        "reference_stable": None,
        "molt_stable": None,
        "reference_observation_count": len(reference_observations),
        "molt_observation_count": len(molt_observations),
        "mismatch_observation": None,
        "mismatch_observation_sha256": None,
    }
    observations = [*reference_observations, *molt_observations]
    incomplete = next(
        (
            observation
            for observation in observations
            if any(value is None for value in observation[1:])
        ),
        None,
    )
    if not reference_observations or not molt_observations or incomplete is not None:
        if incomplete is not None:
            base["detail"] = (
                f"observable output was not fully captured for {incomplete[0]}"
            )
            base["mismatch_observation"] = incomplete[0]
        return base

    reference = reference_observations[0]
    candidate = molt_observations[0]
    reference_outputs = Outputs(
        cast(str, reference[1]), cast(str, reference[2]), cast(int, reference[3])
    )
    candidate_outputs = Outputs(
        cast(str, candidate[1]), cast(str, candidate[2]), cast(int, candidate[3])
    )
    verdict = compare_outputs(
        reference_outputs,
        candidate_outputs,
        mode=ComparisonMode.EXACT,
        stderr_mode="exact",
        exit_law=ExitLaw.EXACT,
    )

    def first_instability(
        samples: Sequence[OutputObservation],
    ) -> tuple[OutputObservation, Verdict] | None:
        expected = Outputs(
            cast(str, samples[0][1]),
            cast(str, samples[0][2]),
            cast(int, samples[0][3]),
        )
        for sample in samples[1:]:
            sample_verdict = compare_outputs(
                expected,
                Outputs(
                    cast(str, sample[1]),
                    cast(str, sample[2]),
                    cast(int, sample[3]),
                ),
                mode=ComparisonMode.EXACT,
                stderr_mode="exact",
                exit_law=ExitLaw.EXACT,
            )
            if not sample_verdict.equal:
                return sample, sample_verdict
        return None

    reference_instability = first_instability(reference_observations)
    molt_instability = first_instability(molt_observations)
    reference_stable = reference_instability is None
    molt_stable = molt_instability is None
    ok = reference_stable and molt_stable and verdict.equal
    mismatch_observation: str | None = None
    mismatch_observation_sha256: str | None = None
    if reference_instability is not None:
        sample, unstable_verdict = reference_instability
        reason = "reference_unstable"
        detail = f"{sample[0]} differs from {reference[0]}: {unstable_verdict.detail}"
        mismatch_observation = sample[0]
        mismatch_observation_sha256 = _observation_hash(
            cast(str, sample[1]), cast(str, sample[2]), cast(int, sample[3])
        )
    elif molt_instability is not None:
        sample, unstable_verdict = molt_instability
        reason = "molt_unstable"
        detail = f"{sample[0]} differs from {candidate[0]}: {unstable_verdict.detail}"
        mismatch_observation = sample[0]
        mismatch_observation_sha256 = _observation_hash(
            cast(str, sample[1]), cast(str, sample[2]), cast(int, sample[3])
        )
    elif verdict.equal:
        reason = "match"
        detail = ""
    elif not verdict.stdout_ok:
        reason = "stdout_mismatch"
        detail = verdict.detail
    elif not verdict.stderr_ok:
        reason = "stderr_mismatch"
        detail = verdict.detail
    elif not verdict.exit_ok:
        reason = "exit_mismatch"
        detail = verdict.detail
    else:  # defensive: Verdict.equal is the conjunction of these axes.
        reason = "comparison_mismatch"
        detail = verdict.detail
    if not ok and mismatch_observation is None:
        mismatch_observation = candidate[0]
        mismatch_observation_sha256 = _observation_hash(
            cast(str, candidate[1]),
            cast(str, candidate[2]),
            cast(int, candidate[3]),
        )
    return {
        **base,
        "checked": True,
        "ok": ok,
        "reason": reason,
        "detail": detail,
        "stdout_match": verdict.stdout_ok,
        "stderr_match": verdict.stderr_ok,
        "exit_match": verdict.exit_ok,
        "reference_returncode": reference[3],
        "molt_returncode": candidate[3],
        "reference_stdout_sha256": _sha256_text(cast(str, reference[1])),
        "molt_stdout_sha256": _sha256_text(cast(str, candidate[1])),
        "reference_stderr_sha256": _sha256_text(cast(str, reference[2])),
        "molt_stderr_sha256": _sha256_text(cast(str, candidate[2])),
        "reference_stable": reference_stable,
        "molt_stable": molt_stable,
        "mismatch_observation": mismatch_observation,
        "mismatch_observation_sha256": mismatch_observation_sha256,
    }


_ALL_VERDICTS = frozenset(
    {
        VERDICT_GREEN,
        VERDICT_FAIL_ENGINE,
        VERDICT_FAIL_COLD_BUDGET,
        VERDICT_WARN_COLD_FLOOR,
        VERDICT_FAIL_STALE,
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_UNSTABLE,
        VERDICT_RUN_BLOCKED,
        VERDICT_CPY_INCOMPAT,
    }
)

_MEASURED_RUN_VERDICTS = frozenset(
    {
        VERDICT_GREEN,
        VERDICT_FAIL_ENGINE,
        VERDICT_FAIL_COLD_BUDGET,
        VERDICT_WARN_COLD_FLOOR,
        VERDICT_UNSTABLE,
    }
)

_MEASURED_RUN_FACT_FIELDS = frozenset(
    {
        "binary_size_kib",
        "compile_time_s",
        "cold_molt_s",
        "cold_cpython_s",
        "warm_molt_s",
        "warm_cpython_s",
        "warm_speedup",
        "cold_speedup",
        "startup_tax_ms",
        "molt_peak_rss_mib",
        "cpython_peak_rss_mib",
    }
)

_MOLT_FAILURE_VERDICTS = frozenset({VERDICT_BUILD_FAILED, VERDICT_RUN_ERROR})

_MOLT_FAILURE_MIRROR_FIELDS = {
    "molt_failure_phase": "phase",
    "molt_failure_status": "status",
    "molt_failure_detail": "detail",
    "molt_failure_message": "message",
    "molt_failure_returncode": "returncode",
    "molt_failure_timed_out": "timed_out",
    "molt_failure_elapsed_s": "elapsed_s",
    "molt_failure_signal": "signal",
    "molt_failure_guard_violation": "guard_violation",
    "molt_failure_orphaned_process_groups": "orphaned_process_groups",
}


@dataclass(frozen=True)
class PerfCell:
    """Validated scoreboard-cell identity and gate facts.

    The complete JSON cell may carry additional measurement and attribution
    fields; this dataclass captures the mandatory cross-tool contract.
    """

    benchmark: str
    target: str
    backend: str
    profile: str
    verdict: str
    stable: bool
    warm_speedup: float | None
    cold_speedup: float | None
    startup_tax_ms: float | None
    binary_size_kib: float | None
    molt_peak_rss_mib: float | None
    compile_time_s: float | None
    pypy_ratio: float | None
    codon_ratio: float | None
    codon_equivalent: bool | None
    output_parity: dict[str, Any] | None
    log_artifact: str | None
    fact_class: str | None
    suspected_missing_fact: str | None
    pypy_advantage_class: str | None
    reference_class: str | None
    codon_semantics: str | None
    attribution_confidence: float | None
    molt_failure_phase: str | None
    molt_failure_status: str | None
    molt_failure_detail: str | None
    molt_failure_message: str | None
    molt_failure_returncode: int | None
    molt_failure_timed_out: bool | None
    molt_failure_elapsed_s: float | None
    molt_failure_signal: dict[str, Any] | None
    molt_failure_guard_violation: dict[str, Any] | None
    molt_failure_orphaned_process_groups: list[int] | None

    @staticmethod
    def from_payload(payload: Mapping[str, Any]) -> "PerfCell":
        problems = validate_cell(payload)
        if problems:
            raise ValueError("; ".join(problems))
        return PerfCell(
            benchmark=str(payload["benchmark"]),
            target=str(payload["target"]),
            backend=str(payload["backend"]),
            profile=str(payload["profile"]),
            verdict=str(payload["verdict"]),
            stable=bool(payload["stable"]),
            warm_speedup=_optional_float(payload.get("warm_speedup")),
            cold_speedup=_optional_float(payload.get("cold_speedup")),
            startup_tax_ms=_optional_float(payload.get("startup_tax_ms")),
            binary_size_kib=_optional_float(payload.get("binary_size_kib")),
            molt_peak_rss_mib=_optional_float(payload.get("molt_peak_rss_mib")),
            compile_time_s=_optional_float(payload.get("compile_time_s")),
            pypy_ratio=_optional_float(payload.get("pypy_ratio")),
            codon_ratio=_optional_float(payload.get("codon_ratio")),
            codon_equivalent=_optional_bool(payload.get("codon_equivalent")),
            output_parity=_optional_dict(payload.get("output_parity")),
            log_artifact=_optional_str(payload.get("log_artifact")),
            fact_class=_optional_str(payload.get("fact_class")),
            suspected_missing_fact=_optional_str(payload.get("suspected_missing_fact")),
            pypy_advantage_class=_optional_str(payload.get("pypy_advantage_class")),
            reference_class=_optional_str(payload.get("reference_class")),
            codon_semantics=_optional_str(payload.get("codon_semantics")),
            attribution_confidence=_optional_float(
                payload.get("attribution_confidence")
            ),
            molt_failure_phase=_optional_str(payload.get("molt_failure_phase")),
            molt_failure_status=_optional_str(payload.get("molt_failure_status")),
            molt_failure_detail=_optional_str(payload.get("molt_failure_detail")),
            molt_failure_message=_optional_str(payload.get("molt_failure_message")),
            molt_failure_returncode=_optional_int(
                payload.get("molt_failure_returncode")
            ),
            molt_failure_timed_out=_optional_bool(
                payload.get("molt_failure_timed_out")
            ),
            molt_failure_elapsed_s=_optional_float(
                payload.get("molt_failure_elapsed_s")
            ),
            molt_failure_signal=_optional_dict(payload.get("molt_failure_signal")),
            molt_failure_guard_violation=_optional_dict(
                payload.get("molt_failure_guard_violation")
            ),
            molt_failure_orphaned_process_groups=_optional_int_list(
                payload.get("molt_failure_orphaned_process_groups")
            ),
        )


def flatten_cells(doc: Mapping[str, Any]) -> list[Mapping[str, Any]]:
    out: list[Mapping[str, Any]] = []
    scoreboard = doc.get("scoreboard")
    if not isinstance(scoreboard, Mapping):
        return out
    for targets in scoreboard.values():
        if not isinstance(targets, Mapping):
            continue
        for backends in targets.values():
            if not isinstance(backends, Mapping):
                continue
            for profiles in backends.values():
                if not isinstance(profiles, Mapping):
                    continue
                for cell in profiles.values():
                    if isinstance(cell, Mapping):
                        out.append(cell)
    return out


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(ch in "0123456789abcdef" for ch in value)
    )


def output_parity_evidence_problems(
    value: object,
    *,
    require_checked: bool = False,
    require_pass: bool = False,
) -> list[str]:
    """Validate hash-only evidence produced by the shared comparison law.

    This validates the durable receipt shape; observable equivalence itself is
    decided only by ``tools.compat.comparison.compare_outputs``.
    """

    if value is None:
        return ["is missing"] if require_checked or require_pass else []
    if not isinstance(value, Mapping):
        return ["must be an object or null"]
    missing = REQUIRED_OUTPUT_PARITY_FIELDS - set(value)
    if missing:
        return [f"missing fields: {sorted(missing)}"]

    problems: list[str] = []
    checked = value.get("checked")
    ok = value.get("ok")
    if not isinstance(checked, bool):
        problems.append(f"checked must be bool, got {checked!r}")
    if ok is not None and not isinstance(ok, bool):
        problems.append(f"ok must be bool or null, got {ok!r}")
    if value.get("reference_runtime") != "cpython":
        problems.append("reference_runtime must be 'cpython'")
    if value.get("comparison_law_version") != OUTPUT_PARITY_COMPARISON_LAW_VERSION:
        problems.append("comparison_law_version must name the canonical comparison law")
    for field, expected in (
        ("mode", "exact"),
        ("stderr_mode", "exact"),
        ("exit_law", "exact"),
    ):
        if value.get(field) != expected:
            problems.append(f"{field} must be {expected!r}")
    reason = value.get("reason")
    if not isinstance(reason, str) or not reason:
        problems.append("reason must be a non-empty string")
    if not isinstance(value.get("detail"), str):
        problems.append("detail must be a string")

    axis_fields = ("stdout_match", "stderr_match", "exit_match")
    stability_fields = ("reference_stable", "molt_stable")
    hash_fields = (
        "reference_stdout_sha256",
        "molt_stdout_sha256",
        "reference_stderr_sha256",
        "molt_stderr_sha256",
    )
    rc_fields = ("reference_returncode", "molt_returncode")
    for field in ("reference_observation_count", "molt_observation_count"):
        count = value.get(field)
        if not isinstance(count, int) or isinstance(count, bool) or count < 0:
            problems.append(f"{field} must be a non-negative int")
    if checked is True:
        reference_count = value.get("reference_observation_count")
        molt_count = value.get("molt_observation_count")
        for field, count in (
            ("reference_observation_count", reference_count),
            ("molt_observation_count", molt_count),
        ):
            if isinstance(count, int) and not isinstance(count, bool) and count == 0:
                problems.append(f"{field} must be at least 1 when checked=true")
        if (
            isinstance(reference_count, int)
            and not isinstance(reference_count, bool)
            and isinstance(molt_count, int)
            and not isinstance(molt_count, bool)
            and reference_count != molt_count
        ):
            problems.append(
                "reference and molt observation counts must match when checked=true"
            )
        for field in axis_fields:
            if not isinstance(value.get(field), bool):
                problems.append(f"{field} must be bool when checked=true")
        for field in stability_fields:
            if not isinstance(value.get(field), bool):
                problems.append(f"{field} must be bool when checked=true")
        for field in hash_fields:
            if not _is_sha256(value.get(field)):
                problems.append(f"{field} must be a lowercase SHA-256 digest")
        for field in rc_fields:
            field_value = value.get(field)
            if not isinstance(field_value, int) or isinstance(field_value, bool):
                problems.append(f"{field} must be int when checked=true")
        for channel in ("stdout", "stderr"):
            reference_hash = value.get(f"reference_{channel}_sha256")
            molt_hash = value.get(f"molt_{channel}_sha256")
            if (
                value.get(f"{channel}_match") is True
                and _is_sha256(reference_hash)
                and _is_sha256(molt_hash)
                and reference_hash != molt_hash
            ):
                problems.append(
                    f"{channel}_match=true contradicts unequal representative {channel} hashes"
                )
        reference_returncode = value.get("reference_returncode")
        molt_returncode = value.get("molt_returncode")
        if (
            value.get("exit_match") is True
            and isinstance(reference_returncode, int)
            and not isinstance(reference_returncode, bool)
            and isinstance(molt_returncode, int)
            and not isinstance(molt_returncode, bool)
            and reference_returncode != molt_returncode
        ):
            problems.append(
                "exit_match=true contradicts unequal representative return codes"
            )
        axes = [value.get(field) for field in (*axis_fields, *stability_fields)]
        if all(isinstance(axis, bool) for axis in axes):
            expected_ok = all(axes)
            if ok is not expected_ok:
                problems.append(
                    f"ok must equal comparison axes plus stability ({expected_ok})"
                )
        mismatch_observation = value.get("mismatch_observation")
        mismatch_hash = value.get("mismatch_observation_sha256")
        if ok is True:
            if mismatch_observation is not None or mismatch_hash is not None:
                problems.append(
                    "matching evidence must not name a mismatch observation"
                )
        else:
            if not isinstance(mismatch_observation, str) or not mismatch_observation:
                problems.append("failed evidence must name a mismatch observation")
            if not _is_sha256(mismatch_hash):
                problems.append("failed evidence must hash the mismatch observation")
    elif checked is False:
        if ok is not None:
            problems.append("ok must be null when checked=false")
        for field in (*axis_fields, *stability_fields, *hash_fields, *rc_fields):
            if value.get(field) is not None:
                problems.append(f"{field} must be null when checked=false")
        mismatch_observation = value.get("mismatch_observation")
        if mismatch_observation is not None and (
            not isinstance(mismatch_observation, str) or not mismatch_observation
        ):
            problems.append("mismatch_observation must be non-empty or null")
        if value.get("mismatch_observation_sha256") is not None:
            problems.append(
                "mismatch_observation_sha256 must be null when checked=false"
            )

    if require_checked and checked is not True:
        problems.append("must be checked for a measured runnable verdict")
    if require_pass and ok is not True:
        problems.append("must pass for a measured runnable verdict")
    return problems


def output_parity_passes(value: object) -> bool:
    return not output_parity_evidence_problems(
        value,
        require_checked=True,
        require_pass=True,
    )


def validate_cell(cell: Mapping[str, Any]) -> list[str]:
    problems: list[str] = []
    missing = REQUIRED_CELL_FIELDS - set(cell)
    if missing:
        problems.append(
            f"cell {cell.get('benchmark')} missing fields: {sorted(missing)}"
        )
        return problems
    if cell.get("verdict") in (None, "pending"):
        problems.append(
            f"cell {cell.get('benchmark')} has unfinalized verdict {cell.get('verdict')!r}"
        )
    log_artifact = cell.get("log_artifact")
    if not isinstance(log_artifact, str) or not log_artifact:
        problems.append(f"cell {cell.get('benchmark')} has missing log_artifact")
    if cell.get("verdict") not in _ALL_VERDICTS:
        problems.append(
            f"cell {cell.get('benchmark')} has unknown verdict {cell.get('verdict')!r}"
        )
    classification = cell.get("classification")
    if classification is not None and classification not in CLASSIFY_STATES:
        problems.append(
            f"cell {cell.get('benchmark')} has unknown classification {classification!r}"
        )
    fact_class = cell.get("fact_class")
    if fact_class is not None and fact_class not in FACT_CLASSES:
        problems.append(
            f"cell {cell.get('benchmark')} has unknown fact_class {fact_class!r}"
        )
    pypy_advantage_class = cell.get("pypy_advantage_class")
    if (
        pypy_advantage_class is not None
        and pypy_advantage_class not in PYPY_ADVANTAGE_CLASSES
    ):
        problems.append(
            f"cell {cell.get('benchmark')} has unknown pypy_advantage_class "
            f"{pypy_advantage_class!r}"
        )
    reference_class = cell.get("reference_class")
    if reference_class is not None and reference_class not in REFERENCE_CLASSES:
        problems.append(
            f"cell {cell.get('benchmark')} has unknown reference_class {reference_class!r}"
        )
    codon_semantics = cell.get("codon_semantics")
    if codon_semantics is not None and codon_semantics not in CODON_SEMANTICS:
        problems.append(
            f"cell {cell.get('benchmark')} has unknown codon_semantics "
            f"{codon_semantics!r}"
        )
    suspected_missing_fact = cell.get("suspected_missing_fact")
    if suspected_missing_fact is not None and (
        not isinstance(suspected_missing_fact, str)
        or not suspected_missing_fact.strip()
    ):
        problems.append(
            f"cell {cell.get('benchmark')} has invalid suspected_missing_fact "
            f"{suspected_missing_fact!r}"
        )
    attribution_confidence = cell.get("attribution_confidence")
    if attribution_confidence is not None:
        if not _is_number(attribution_confidence):
            problems.append(
                f"cell {cell.get('benchmark')} has non-numeric "
                f"attribution_confidence {attribution_confidence!r}"
            )
        elif not 0.0 <= float(attribution_confidence) <= 1.0:
            problems.append(
                f"cell {cell.get('benchmark')} has out-of-range "
                f"attribution_confidence {attribution_confidence!r}"
            )
    if fact_class is not None and not cell.get("suspected_missing_fact"):
        problems.append(
            f"cell {cell.get('benchmark')} has fact_class without suspected_missing_fact"
        )
    verdict = str(cell.get("verdict"))
    parity_required = verdict in _MEASURED_RUN_VERDICTS
    parity_problems = output_parity_evidence_problems(
        cell.get("output_parity"),
        require_checked=parity_required,
        require_pass=parity_required,
    )
    problems.extend(
        f"cell {cell.get('benchmark')} output_parity {problem}"
        for problem in parity_problems
    )
    if verdict in _MOLT_FAILURE_VERDICTS:
        problems.extend(_validate_molt_failure_cell(cell, verdict))
    if (
        verdict == VERDICT_RUN_ERROR
        and cell.get("molt_failure_status") == "output_mismatch"
    ):
        parity = cell.get("output_parity")
        if not isinstance(parity, Mapping):
            problems.append(
                f"cell {cell.get('benchmark')} is output_mismatch without "
                "structured output_parity evidence"
            )
        elif parity.get("ok") is True:
            problems.append(
                f"cell {cell.get('benchmark')} is output_mismatch with "
                "output_parity.ok=true"
            )
    if verdict in _MEASURED_RUN_VERDICTS:
        problems.extend(_validate_measured_run_cell(cell, verdict))
    if classification == CLASS_RED_STABLE:
        problems.extend(_validate_red_stable_cell(cell))
    return problems


def validate_board(doc: Mapping[str, Any]) -> list[str]:
    """Return schema contract violations for a scoreboard document."""

    problems: list[str] = []
    missing = REQUIRED_TOP_LEVEL_KEYS - set(doc)
    if missing:
        problems.append(f"missing top-level keys: {sorted(missing)}")
    if doc.get("schema_version") != SCHEMA_VERSION:
        problems.append(
            f"schema_version must be {SCHEMA_VERSION}, got "
            f"{doc.get('schema_version')!r}"
        )
    pmiss = REQUIRED_PROVENANCE_FIELDS - set(_mapping(doc.get("provenance")))
    if pmiss:
        problems.append(f"provenance missing fields: {sorted(pmiss)}")
    problems.extend(_validate_host_payload(_mapping(doc.get("host"))))
    smiss = REQUIRED_SUMMARY_FIELDS - set(_mapping(doc.get("summary")))
    if smiss:
        problems.append(f"summary missing 2-D fields: {sorted(smiss)}")
    try:
        json.loads(json.dumps(doc))
    except (TypeError, ValueError) as exc:
        problems.append(f"doc is not JSON-serializable: {exc}")
    cells = flatten_cells(doc)
    if not cells:
        problems.append("no cells emitted")
    for cell in cells:
        cell_problems = validate_cell(cell)
        if cell_problems:
            problems.extend(cell_problems)
            break
    return problems


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _validate_host_payload(host: Mapping[str, Any]) -> list[str]:
    problems: list[str] = []
    missing = REQUIRED_HOST_FIELDS - set(host)
    if missing:
        problems.append(f"host missing fields: {sorted(missing)}")
        return problems

    for field in sorted(REQUIRED_HOST_FIELDS):
        value = host.get(field)
        if not isinstance(value, str) or not value:
            problems.append(f"host field {field} must be a non-empty string")

    has_modern_field = any(field in host for field in MODERN_HOST_FIELDS)
    if not has_modern_field:
        return problems

    modern_missing = MODERN_HOST_FIELDS - set(host)
    if modern_missing:
        problems.append(f"host missing current oracle fields: {sorted(modern_missing)}")
        return problems

    machine = host.get("machine")
    arch = host.get("arch")
    pointer_bits = host.get("pointer_bits")
    if not isinstance(machine, str) or not machine:
        problems.append("host field machine must be a non-empty string")
    if not isinstance(arch, str) or not arch:
        problems.append("host field arch must be a non-empty string")
    if not _is_pointer_width(pointer_bits):
        problems.append(
            f"host field pointer_bits must be 32 or 64, got {pointer_bits!r}"
        )

    oracle = _mapping(host.get("cpython_oracle"))
    if not oracle:
        problems.append("host.cpython_oracle must be an object")
        return problems
    problems.extend(_validate_cpython_oracle(host, oracle))
    return problems


def _validate_cpython_oracle(
    host: Mapping[str, Any], oracle: Mapping[str, Any]
) -> list[str]:
    problems: list[str] = []
    missing = REQUIRED_CPYTHON_ORACLE_FIELDS - set(oracle)
    if missing:
        problems.append(f"host.cpython_oracle missing fields: {sorted(missing)}")
        return problems

    for field in (
        "executable",
        "implementation",
        "version",
        "sys_platform",
        "machine",
        "arch",
    ):
        value = oracle.get(field)
        if not isinstance(value, str) or not value:
            problems.append(f"host.cpython_oracle.{field} must be a non-empty string")

    cmd = oracle.get("cmd")
    if (
        not isinstance(cmd, list)
        or not cmd
        or any(not isinstance(part, str) or not part for part in cmd)
    ):
        problems.append("host.cpython_oracle.cmd must be a non-empty string list")
    elif cmd[0] != oracle.get("executable"):
        problems.append(
            "host.cpython_oracle.cmd[0] must be the resolved executable, "
            f"got {cmd[0]!r} vs {oracle.get('executable')!r}"
        )

    if oracle.get("implementation") != "CPython":
        problems.append(
            "host.cpython_oracle.implementation must be 'CPython', "
            f"got {oracle.get('implementation')!r}"
        )
    if oracle.get("version") != host.get("cpython_baseline"):
        problems.append(
            "host.cpython_oracle.version must match host.cpython_baseline, "
            f"got {oracle.get('version')!r} vs {host.get('cpython_baseline')!r}"
        )
    if oracle.get("sys_platform") != host.get("platform"):
        problems.append(
            "host.cpython_oracle.sys_platform must match host.platform, "
            f"got {oracle.get('sys_platform')!r} vs {host.get('platform')!r}"
        )
    if oracle.get("arch") != host.get("arch"):
        problems.append(
            "host.cpython_oracle.arch must match host.arch, "
            f"got {oracle.get('arch')!r} vs {host.get('arch')!r}"
        )
    if oracle.get("pointer_bits") != host.get("pointer_bits"):
        problems.append(
            "host.cpython_oracle.pointer_bits must match host.pointer_bits, "
            f"got {oracle.get('pointer_bits')!r} vs {host.get('pointer_bits')!r}"
        )
    if not _is_pointer_width(oracle.get("pointer_bits")):
        problems.append(
            "host.cpython_oracle.pointer_bits must be 32 or 64, "
            f"got {oracle.get('pointer_bits')!r}"
        )
    return problems


def _validate_measured_run_cell(cell: Mapping[str, Any], verdict: str) -> list[str]:
    problems: list[str] = []
    benchmark = cell.get("benchmark")
    if cell.get("build_ok") is not True:
        problems.append(
            f"cell {benchmark} has measured verdict {verdict} without build_ok"
        )
    if cell.get("run_blocked") is not False:
        problems.append(
            f"cell {benchmark} has measured verdict {verdict} while run_blocked"
        )
    if cell.get("molt_ok") is not True or cell.get("cpython_ok") is not True:
        problems.append(
            f"cell {benchmark} has measured verdict {verdict} without both runtimes ok"
        )
    missing = sorted(
        field for field in _MEASURED_RUN_FACT_FIELDS if not _is_number(cell.get(field))
    )
    if missing:
        problems.append(
            f"cell {benchmark} has measured verdict {verdict} missing numeric facts: "
            f"{missing}"
        )
        return problems
    warm = float(cell["warm_speedup"])
    cold = float(cell["cold_speedup"])
    stable = cell.get("stable")
    if not isinstance(stable, bool):
        problems.append(f"cell {benchmark} has non-bool stable flag {stable!r}")
    if verdict == VERDICT_GREEN:
        if stable is not True:
            problems.append(f"cell {benchmark} is GREEN without stable=true")
        if warm <= RED_THRESHOLD or cold <= RED_THRESHOLD:
            problems.append(
                f"cell {benchmark} is GREEN with warm/cold speedup at-or-below floor"
            )
    elif verdict == VERDICT_FAIL_ENGINE and warm > RED_THRESHOLD:
        problems.append(
            f"cell {benchmark} is FAIL_ENGINE with warm_speedup above floor"
        )
    elif verdict == VERDICT_FAIL_COLD_BUDGET:
        budget = cell.get("cold_budget_ms")
        tax = float(cell["startup_tax_ms"])
        if not _is_number(budget):
            problems.append(
                f"cell {benchmark} is FAIL_COLD_BUDGET without numeric cold_budget_ms"
            )
        elif tax <= float(budget):
            problems.append(
                f"cell {benchmark} is FAIL_COLD_BUDGET without tax above budget"
            )
    elif verdict == VERDICT_WARN_COLD_FLOOR:
        if warm <= RED_THRESHOLD or cold > RED_THRESHOLD:
            problems.append(
                f"cell {benchmark} is WARN_COLD_FLOOR without warm win and cold floor loss"
            )
    elif verdict == VERDICT_UNSTABLE and stable is not False:
        problems.append(f"cell {benchmark} is UNSTABLE without stable=false")
    return problems


def _validate_molt_failure_cell(
    cell: Mapping[str, Any],
    verdict: str,
) -> list[str]:
    problems: list[str] = []
    benchmark = cell.get("benchmark")
    payload = cell.get("molt_failure")
    if not isinstance(payload, Mapping):
        problems.append(f"cell {benchmark} is {verdict} without a molt_failure payload")
        return problems

    for field in ("phase", "status"):
        value = payload.get(field)
        if not isinstance(value, str) or not value:
            problems.append(
                f"cell {benchmark} has invalid molt_failure.{field} {value!r}"
            )

    expected_phase = "build" if verdict == VERDICT_BUILD_FAILED else "run"
    if payload.get("phase") != expected_phase:
        problems.append(
            f"cell {benchmark} is {verdict} with molt_failure.phase "
            f"{payload.get('phase')!r}, expected {expected_phase!r}"
        )

    for flat_field, payload_field in _MOLT_FAILURE_MIRROR_FIELDS.items():
        flat = cell.get(flat_field)
        nested = payload.get(payload_field)
        if flat != nested:
            problems.append(
                f"cell {benchmark} has {flat_field}={flat!r} but "
                f"molt_failure.{payload_field}={nested!r}"
            )

    if not isinstance(cell.get("molt_failure_timed_out"), bool):
        problems.append(
            f"cell {benchmark} has non-bool molt_failure_timed_out "
            f"{cell.get('molt_failure_timed_out')!r}"
        )
    returncode = cell.get("molt_failure_returncode")
    if returncode is not None and not _is_int(returncode):
        problems.append(
            f"cell {benchmark} has invalid molt_failure_returncode {returncode!r}"
        )
    elapsed_s = cell.get("molt_failure_elapsed_s")
    if elapsed_s is not None and not _is_number(elapsed_s):
        problems.append(
            f"cell {benchmark} has invalid molt_failure_elapsed_s {elapsed_s!r}"
        )
    groups = cell.get("molt_failure_orphaned_process_groups")
    if not isinstance(groups, list) or not all(_is_int(value) for value in groups):
        problems.append(
            f"cell {benchmark} has invalid molt_failure_orphaned_process_groups "
            f"{groups!r}"
        )
    return problems


def _validate_red_stable_cell(cell: Mapping[str, Any]) -> list[str]:
    problems: list[str] = []
    benchmark = cell.get("benchmark")
    if cell.get("measured_quiescent") is not True:
        problems.append(
            f"cell {benchmark} is RED_STABLE without measured_quiescent=true"
        )
    lo = cell.get("repeat_ci_lo")
    hi = cell.get("repeat_ci_hi")
    if not _is_number(lo) or not _is_number(hi):
        problems.append(f"cell {benchmark} is RED_STABLE without numeric repeat CI")
        return problems
    if float(lo) >= RED_THRESHOLD or float(hi) >= RED_THRESHOLD:
        problems.append(
            f"cell {benchmark} is RED_STABLE without repeat CI clearing below floor"
        )
    return problems


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_pointer_width(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value in {32, 64}


def _optional_float(value: Any) -> float | None:
    if value is None:
        return None
    if _is_number(value):
        return float(value)
    raise ValueError(f"expected number or null, got {value!r}")


def _optional_int(value: Any) -> int | None:
    if value is None:
        return None
    if _is_int(value):
        return value
    raise ValueError(f"expected int or null, got {value!r}")


def _optional_dict(value: Any) -> dict[str, Any] | None:
    if value is None:
        return None
    if isinstance(value, dict):
        return dict(value)
    raise ValueError(f"expected dict or null, got {value!r}")


def _optional_int_list(value: Any) -> list[int] | None:
    if value is None:
        return None
    if isinstance(value, list) and all(_is_int(item) for item in value):
        return [int(item) for item in value]
    raise ValueError(f"expected int list or null, got {value!r}")


def _optional_bool(value: Any) -> bool | None:
    if value is None:
        return None
    if isinstance(value, bool):
        return value
    raise ValueError(f"expected bool or null, got {value!r}")


def _optional_str(value: Any) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        return value
    raise ValueError(f"expected str or null, got {value!r}")
