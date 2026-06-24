from __future__ import annotations

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
        "cpython_ratio": 2.0,
        "cold_ratio": 2.0,
        "warm_ratio": 2.0,
        "warm_speedup": 2.0,
        "cold_speedup": 2.0,
        "startup_tax_ms": 5.0,
        "verdict": schema.VERDICT_GREEN,
        "binary_size_kib": 512.0,
        "molt_peak_rss_mib": 18.0,
        "compile_time_s": 0.4,
        "stable": True,
        "red": False,
        "status": "green",
        "pypy_ratio": None,
        "codon_ratio": None,
        "codon_equivalent": None,
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

    assert schema.validate_scoreboard_doc(doc) == []
    flattened = schema.flatten_cells(doc)
    assert flattened == [cell]
    perf_cell = schema.PerfCell.from_payload(flattened[0])
    assert perf_cell.benchmark == "tests/benchmarks/bench_fib.py"
    assert perf_cell.verdict == schema.VERDICT_GREEN
    assert perf_cell.red is False


def test_schema_rejects_unknown_verdict_and_classification() -> None:
    cell = _cell(verdict="MAYBE_FAST", classification="SORT_OF_GREEN")

    problems = schema.validate_cell_payload(cell)

    assert any("unknown verdict" in problem for problem in problems)
    assert any("unknown classification" in problem for problem in problems)
