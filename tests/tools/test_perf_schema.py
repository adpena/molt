from __future__ import annotations

import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_schema as schema  # noqa: E402
import perf_scoreboard as scoreboard  # noqa: E402


def _cell(**overrides: object) -> dict[str, object]:
    cell: dict[str, object] = {
        "benchmark": "tests/benchmarks/bench_fib.py",
        "target": "native",
        "backend": "native",
        "profile": "release-fast",
        "build_ok": True,
        "run_blocked": False,
        "molt_ok": True,
        "cpython_ok": True,
        "cold_molt_s": 0.12,
        "cold_cpython_s": 0.24,
        "warm_molt_s": 0.10,
        "warm_cpython_s": 0.20,
        "warm_speedup": 2.0,
        "cold_speedup": 2.0,
        "startup_tax_ms": 5.0,
        "verdict": schema.VERDICT_GREEN,
        "binary_size_kib": 512.0,
        "molt_peak_rss_mib": 18.0,
        "compile_time_s": 0.4,
        "stable": True,
        "pypy_ratio": None,
        "codon_ratio": None,
        "codon_equivalent": None,
        "cpython_peak_rss_mib": 15.0,
        "output_parity": True,
        "log_artifact": "bench/scoreboard/logs/fib.log",
        "classification": schema.CLASS_GREEN,
    }
    cell.update(overrides)
    return cell


def _doc(cell: dict[str, object]) -> dict[str, object]:
    return {
        "schema_version": schema.SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": "2026-06-24T00:00:00+00:00",
        "git_rev": "a" * 40,
        "provenance": {
            "origin_sha": "a" * 40,
            "local_head_sha": "a" * 40,
            "merge_base_sha": "a" * 40,
            "dirty_tree": False,
            "benchmark_tool_sha": "b" * 40,
            "backend_binary_identity": {"native/release-fast": "sha|1|2"},
            "stdlib_cache_key": "cache",
            "authoritative": True,
        },
        "host": {"platform": "test"},
        "direction": "speedup = cpython_time / molt_time",
        "red_threshold": 1.0,
        "verdict_legend": {},
        "methodology": {},
        "reserved_columns": {},
        "summary": {
            "cells_fail_engine": 0,
            "cells_fail_cold_budget": 0,
            "cells_warn_cold_floor": 0,
            "cells_fail_stale": 0,
            "verdict_breakdown": {},
            "gate_fails": False,
        },
        "benchmarks_run": [cell["benchmark"]],
        "benchmarks_deferred": [],
        "scoreboard": {
            cell["benchmark"]: {
                cell["target"]: {cell["backend"]: {cell["profile"]: cell}}
            }
        },
    }


def test_perf_scoreboard_uses_schema_vocabulary_authority() -> None:
    assert scoreboard.SCHEMA_VERSION == schema.SCHEMA_VERSION
    assert scoreboard.VERDICT_FAIL_ENGINE == schema.VERDICT_FAIL_ENGINE
    assert scoreboard.CLASS_RED_STABLE == schema.CLASS_RED_STABLE
    assert scoreboard.GATE_FAILING_VERDICTS is schema.GATE_FAILING_VERDICTS


def test_schema_accepts_valid_board_and_materializes_cell() -> None:
    cell = _cell()
    doc = _doc(cell)

    assert schema.validate_board(doc) == []
    flattened = schema.flatten_cells(doc)
    assert flattened == [cell]
    perf_cell = schema.PerfCell.from_payload(flattened[0])
    assert perf_cell.benchmark == "tests/benchmarks/bench_fib.py"
    assert perf_cell.verdict == schema.VERDICT_GREEN
    assert perf_cell.stable is True
    assert perf_cell.warm_speedup == 2.0


def test_schema_rejects_unknown_verdict_and_classification() -> None:
    cell = _cell(verdict="MAYBE_FAST", classification="SORT_OF_GREEN")

    problems = schema.validate_cell(cell)

    assert any("unknown verdict" in problem for problem in problems)
    assert any("unknown classification" in problem for problem in problems)


def test_schema_rejects_measured_verdict_without_method_facts() -> None:
    cell = _cell(warm_molt_s=None)

    problems = schema.validate_cell(cell)

    assert any("missing numeric facts" in problem for problem in problems)


def test_schema_rejects_red_stable_without_quiescent_repeat_ci() -> None:
    cell = _cell(
        verdict=schema.VERDICT_FAIL_ENGINE,
        classification=schema.CLASS_RED_STABLE,
        measured_quiescent=False,
        repeat_ci_lo=None,
        repeat_ci_hi=None,
        warm_speedup=0.8,
        warm_molt_s=0.20,
        warm_cpython_s=0.16,
    )

    problems = schema.validate_cell(cell)

    assert any("measured_quiescent=true" in problem for problem in problems)
    assert any("numeric repeat CI" in problem for problem in problems)


def test_schema_accepts_checked_in_quiet_native_board_cells() -> None:
    doc = json.loads(
        (REPO_ROOT / "bench" / "scoreboard" / "quiet_native.json").read_text(
            encoding="utf-8"
        )
    )

    assert schema.validate_board(doc) == []
    cells = schema.flatten_cells(doc)
    assert cells
    assert all(schema.PerfCell.from_payload(cell).benchmark for cell in cells)
