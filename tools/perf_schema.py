#!/usr/bin/env python3
"""Schema authority for Molt performance scoreboard artifacts.

The measurement runner in ``perf_scoreboard.py`` owns execution. This module
owns the durable JSON contract that board projections, CI gates, history tools,
and tests can import without importing the full runner.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

SCHEMA_VERSION = 3
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
    log_artifact: str | None
    fact_class: str | None
    suspected_missing_fact: str | None
    pypy_advantage_class: str | None
    reference_class: str | None
    codon_semantics: str | None
    attribution_confidence: float | None

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
            log_artifact=_optional_str(payload.get("log_artifact")),
            fact_class=_optional_str(payload.get("fact_class")),
            suspected_missing_fact=_optional_str(payload.get("suspected_missing_fact")),
            pypy_advantage_class=_optional_str(payload.get("pypy_advantage_class")),
            reference_class=_optional_str(payload.get("reference_class")),
            codon_semantics=_optional_str(payload.get("codon_semantics")),
            attribution_confidence=_optional_float(
                payload.get("attribution_confidence")
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


def _is_pointer_width(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value in {32, 64}


def _optional_float(value: Any) -> float | None:
    if value is None:
        return None
    if _is_number(value):
        return float(value)
    raise ValueError(f"expected number or null, got {value!r}")


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
