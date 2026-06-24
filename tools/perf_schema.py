#!/usr/bin/env python3
"""Schema authority for Molt performance scoreboard artifacts.

The measurement runner in ``perf_scoreboard.py`` owns execution. This module
owns the durable JSON contract that board projections, CI gates, history tools,
and tests can import without importing the full runner.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Mapping

SCHEMA_VERSION = 3

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

GATE_FAILING_VERDICTS = frozenset(
    {
        VERDICT_FAIL_ENGINE,
        VERDICT_FAIL_COLD_BUDGET,
        VERDICT_BUILD_FAILED,
        VERDICT_RUN_ERROR,
        VERDICT_UNSTABLE,
    }
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
        "cpython_ratio",
        "cold_ratio",
        "warm_ratio",
        "warm_speedup",
        "cold_speedup",
        "startup_tax_ms",
        "verdict",
        "binary_size_kib",
        "molt_peak_rss_mib",
        "compile_time_s",
        "stable",
        "red",
        "status",
        "pypy_ratio",
        "codon_ratio",
        "codon_equivalent",
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
    red: bool
    status: str
    stable: bool
    cpython_ratio: float | None
    cold_ratio: float | None
    warm_ratio: float | None
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

    @staticmethod
    def from_payload(payload: Mapping[str, Any]) -> "PerfCell":
        problems = validate_cell_payload(payload)
        if problems:
            raise ValueError("; ".join(problems))
        return PerfCell(
            benchmark=str(payload["benchmark"]),
            target=str(payload["target"]),
            backend=str(payload["backend"]),
            profile=str(payload["profile"]),
            verdict=str(payload["verdict"]),
            red=bool(payload["red"]),
            status=str(payload["status"]),
            stable=bool(payload["stable"]),
            cpython_ratio=_optional_float(payload.get("cpython_ratio")),
            cold_ratio=_optional_float(payload.get("cold_ratio")),
            warm_ratio=_optional_float(payload.get("warm_ratio")),
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


def validate_cell_payload(cell: Mapping[str, Any]) -> list[str]:
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
    if cell.get("verdict") not in _ALL_VERDICTS:
        problems.append(
            f"cell {cell.get('benchmark')} has unknown verdict {cell.get('verdict')!r}"
        )
    classification = cell.get("classification")
    if classification is not None and classification not in CLASSIFY_STATES:
        problems.append(
            f"cell {cell.get('benchmark')} has unknown classification {classification!r}"
        )
    return problems


def validate_scoreboard_doc(doc: Mapping[str, Any]) -> list[str]:
    """Return schema contract violations for a scoreboard document."""

    problems: list[str] = []
    missing = REQUIRED_TOP_LEVEL_KEYS - set(doc)
    if missing:
        problems.append(f"missing top-level keys: {sorted(missing)}")
    pmiss = REQUIRED_PROVENANCE_FIELDS - set(_mapping(doc.get("provenance")))
    if pmiss:
        problems.append(f"provenance missing fields: {sorted(pmiss)}")
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
        cell_problems = validate_cell_payload(cell)
        if cell_problems:
            problems.extend(cell_problems)
            break
    return problems


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _optional_float(value: Any) -> float | None:
    if value is None:
        return None
    if isinstance(value, (int, float)):
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
