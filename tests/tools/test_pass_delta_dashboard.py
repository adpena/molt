from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import pass_delta_dashboard as dashboard  # noqa: E402


def _delta(**overrides):
    base = {
        "blocks_delta": 0,
        "ops_delta": 0,
        "typed_values_delta": 0,
        "repr_delta": {},
        "lost_repr_values": {},
        "gained_repr_values": {},
        "op_delta": {},
        "added_box_ops": 0,
        "removed_box_ops": 0,
        "added_unbox_ops": 0,
        "removed_unbox_ops": 0,
        "added_generic_calls": 0,
        "removed_generic_calls": 0,
        "added_runtime_helper_calls": 0,
        "removed_runtime_helper_calls": 0,
        "added_rc_events": 0,
        "removed_rc_events": 0,
        "added_exception_events": 0,
        "removed_exception_events": 0,
        "added_type_guard_ops": 0,
        "removed_type_guard_ops": 0,
        "added_heap_alloc_ops": 0,
        "removed_heap_alloc_ops": 0,
        "call_results_typed_repr_delta": 0,
        "call_results_dynbox_delta": 0,
    }
    base.update(overrides)
    return base


def _record(function: str, pass_name: str, **delta_overrides):
    return {
        "schema_version": 1,
        "kind": "molt_tir_pass_delta",
        "function": function,
        "pass": pass_name,
        "mutation_class": "OpsOnly",
        "host": {
            "os": "windows",
            "arch": "x86_64",
            "family": "windows",
            "pointer_width": 64,
        },
        "target": {"target": "NativeCranelift", "profile": "ReleaseFast"},
        "stats": {
            "name": pass_name,
            "values_changed": 0,
            "attrs_changed": 0,
            "ops_removed": 0,
            "ops_added": 0,
            "facts_changed": 0,
            "total_changes": 0,
        },
        "before": {"ops": 1},
        "after": {"ops": 1},
        "delta": _delta(**delta_overrides),
    }


def test_dashboard_aggregates_pass_risk_counters() -> None:
    records = [
        _record(
            "bench_etl_orders_main",
            "unboxing",
            added_box_ops=2,
            added_generic_calls=1,
            lost_repr_values={"MaybeBigInt": 3},
            call_results_dynbox_delta=1,
        ),
        _record(
            "bench_etl_orders_main",
            "refcount_elim",
            added_rc_events=4,
            added_runtime_helper_calls=1,
        ),
    ]

    summary = dashboard.summarize_records(records, source="fixture.jsonl")

    assert summary["kind"] == "molt_tir_pass_delta_dashboard"
    assert summary["records_seen"] == 2
    assert summary["records_considered"] == 2
    by_pass = {row["pass"]: row for row in summary["by_pass"]}
    assert by_pass["unboxing"]["added_box_ops"] == 2
    assert by_pass["unboxing"]["lost_repr_values"] == {"MaybeBigInt": 3}
    assert by_pass["unboxing"]["score"] == 7
    assert by_pass["refcount_elim"]["added_rc_events"] == 4
    assert summary["totals"]["added_runtime_helper_calls"] == 1
    assert summary["host_targets"] == [
        {
            "host": {
                "os": "windows",
                "arch": "x86_64",
                "family": "windows",
                "pointer_width": 64,
            },
            "target": {"target": "NativeCranelift", "profile": "ReleaseFast"},
        }
    ]


def test_load_records_validates_schema(tmp_path: Path) -> None:
    path = tmp_path / "pass_delta.jsonl"
    good = _record("fn", "pass")
    bad = dict(good)
    bad["schema_version"] = 99
    path.write_text(
        json.dumps(good) + "\n" + json.dumps(bad) + "\n",
        encoding="utf-8",
    )

    with pytest.raises(dashboard.PassDeltaError, match="schema_version"):
        dashboard.load_records(path)


def test_benchmark_filter_matches_function_substring() -> None:
    records = [
        _record("bench_etl_orders_main", "unboxing", added_box_ops=1),
        _record("bench_exception_heavy_main", "drop_insertion", added_rc_events=9),
    ]

    summary = dashboard.summarize_records(
        records,
        source="fixture.jsonl",
        benchmark="bench_exception_heavy",
    )

    assert summary["records_seen"] == 2
    assert summary["records_considered"] == 1
    assert summary["by_pass"][0]["pass"] == "drop_insertion"
    assert summary["totals"]["added_rc_events"] == 9
