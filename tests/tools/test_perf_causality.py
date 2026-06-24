from __future__ import annotations

import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_causality as causality  # noqa: E402
import perf_schema as schema  # noqa: E402


def test_hot_profile_fixture_reproduces_verified_attributions() -> None:
    doc = json.loads(
        (REPO_ROOT / "bench" / "scoreboard" / "hot_profile_native.json").read_text(
            encoding="utf-8"
        )
    )

    attributions = {
        Path(attr.benchmark).stem: attr
        for attr in causality.derive_hot_profile_attributions(doc)
    }

    exception_heavy = attributions["bench_exception_heavy"]
    assert exception_heavy.source == "cycle_profile"
    assert exception_heavy.fact_class == "exception_region"
    assert exception_heavy.suspected_missing_fact == "ExceptionRegion/ownership"
    assert exception_heavy.evidence_samples > 0
    assert exception_heavy.fact_class in schema.FACT_CLASSES

    etl_orders = attributions["bench_etl_orders"]
    assert etl_orders.source == "cycle_profile"
    assert etl_orders.fact_class == "shape_facts"
    assert etl_orders.suspected_missing_fact == "ShapeFacts/string-repr"
    assert etl_orders.evidence_samples > 0
    assert etl_orders.fact_class in schema.FACT_CLASSES


def test_benchmark_taxonomy_fallback_is_schema_vetted() -> None:
    attr = causality.derive_attribution("tests/benchmarks/bench_fib.py")

    assert attr.source == "benchmark_taxonomy"
    assert attr.fact_class == "repr_tir_type_lattice"
    assert attr.suspected_missing_fact == "Repr/TirType numeric lane"
    assert attr.fact_class in schema.FACT_CLASSES
    assert 0.0 <= attr.attribution_confidence <= 1.0


def test_pass_delta_dashboard_joins_cycle_confidence() -> None:
    attr = causality.derive_attribution(
        "tests/benchmarks/bench_exception_heavy.py",
        [
            {
                "symbol": "molt_runtime::builtins::exceptions::record_exception",
                "self_samples": 30,
            },
            {"symbol": "molt_runtime::loop_body", "self_samples": 70},
        ],
        pass_delta_dashboard={
            "risk_records": [
                {
                    "function": "bench_exception_heavy__molt_user_main",
                    "pass": "drop_insertion",
                    "signals": {
                        "added_exception_events": 2,
                        "added_rc_events": 3,
                    },
                    "lost_repr_values": {},
                    "score": 5,
                }
            ],
            "by_pass": [],
        },
    )

    assert attr.source == "cycle_profile"
    assert attr.evidence_sources == ("pass_delta_dashboard",)
    assert attr.pass_delta_score == 5
    assert attr.pass_delta_passes == ("drop_insertion",)
    assert attr.pass_delta_fact_classes == ("exception_region", "ownership_lattice")
    assert attr.attribution_confidence == 0.65


def test_call_fact_census_joins_only_fresh_call_fact_attribution() -> None:
    attr = causality.derive_attribution(
        "tests/benchmarks/bench_attr_dispatch.py",
        [
            {"symbol": "molt_generic_call", "self_samples": 3},
            {"symbol": "molt_runtime::loop_body", "self_samples": 7},
        ],
        pass_delta_dashboard={
            "risk_records": [
                {
                    "function": "bench_attr_dispatch__molt_user_main",
                    "pass": "call_facts",
                    "signals": {"added_generic_calls": 2},
                    "lost_repr_values": {},
                    "score": 2,
                }
            ],
            "by_pass": [],
        },
        call_fact_coverage={
            "census": {
                "attached": 5,
                "transient": 2,
                "stale_evidence": [],
            }
        },
    )

    assert attr.fact_class == "call_facts"
    assert attr.source == "cycle_profile"
    assert attr.evidence_sources == (
        "pass_delta_dashboard",
        "call_fact_coverage",
    )
    assert attr.call_fact_attached == 5
    assert attr.call_fact_transient == 2
    assert attr.attribution_confidence == 0.75


def test_cli_accepts_pass_delta_and_call_fact_artifacts(tmp_path: Path, capsys) -> None:
    profile = tmp_path / "hot_profile.json"
    pass_delta = tmp_path / "pass_delta.json"
    call_facts = tmp_path / "call_facts.json"
    profile.write_text(
        json.dumps(
            {
                "cells": [
                    {
                        "benchmark": "tests/benchmarks/bench_attr_dispatch.py",
                        "cycle_profile": {
                            "in_binary_top": [
                                {"symbol": "molt_generic_call", "self_samples": 3},
                                {
                                    "symbol": "molt_runtime::loop_body",
                                    "self_samples": 7,
                                },
                            ]
                        },
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    pass_delta.write_text(
        json.dumps(
            {
                "risk_records": [
                    {
                        "function": "bench_attr_dispatch__molt_user_main",
                        "pass": "call_facts",
                        "signals": {"added_generic_calls": 2},
                        "lost_repr_values": {},
                        "score": 2,
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    call_facts.write_text(
        json.dumps(
            {
                "census": {
                    "attached": 5,
                    "transient": 2,
                    "stale_evidence": [],
                }
            }
        ),
        encoding="utf-8",
    )

    rc = causality.main(
        [
            str(profile),
            "--pass-delta-dashboard",
            str(pass_delta),
            "--call-fact-coverage",
            str(call_facts),
            "--json",
        ]
    )
    captured = capsys.readouterr()
    payload = json.loads(captured.out)

    assert rc == 0
    assert payload[0]["source"] == "cycle_profile"
    assert payload[0]["evidence_sources"] == [
        "pass_delta_dashboard",
        "call_fact_coverage",
    ]
    assert payload[0]["call_fact_attached"] == 5


def test_by_pass_support_joins_without_replacing_primary_class() -> None:
    attr = causality.derive_attribution(
        "tests/benchmarks/bench_exception_heavy.py",
        [
            {
                "symbol": "molt_runtime::builtins::exceptions::record_exception",
                "self_samples": 9,
            }
        ],
        pass_delta_dashboard={
            "by_pass": [
                {
                    "pass": "drop_insertion",
                    "functions": ["bench_exception_heavy_main"],
                    "score": 4,
                    "added_rc_events": 4,
                }
            ]
        },
    )

    assert attr.source == "cycle_profile"
    assert attr.evidence_sources == ("pass_delta_dashboard",)
    assert attr.pass_delta_score == 4
    assert attr.pass_delta_passes == ("drop_insertion",)
    assert attr.pass_delta_fact_classes == ("ownership_lattice",)


def test_pass_delta_dashboard_support_joins_taxonomy_fallback() -> None:
    attr = causality.derive_attribution(
        "tests/benchmarks/bench_etl_orders.py",
        pass_delta_dashboard={
            "by_pass": [
                {
                    "pass": "unboxing",
                    "functions": ["bench_etl_orders_main"],
                    "score": 3,
                    "added_box_ops": 2,
                    "call_results_dynbox_delta": 1,
                }
            ]
        },
    )

    assert attr.source == "benchmark_taxonomy"
    assert attr.fact_class == "shape_facts"
    assert attr.evidence_sources == ("pass_delta_dashboard",)
    assert attr.pass_delta_score == 3
    assert attr.pass_delta_fact_classes == ("repr_tir_type_lattice",)


def test_pass_delta_evidence_join_when_compatible() -> None:
    dashboard = {
        "by_pass": [
            {
                "pass": "unboxing",
                "functions": ["bench_fib_main"],
                "score": 5,
                "added_box_ops": 2,
                "call_results_dynbox_delta": 1,
            },
            {
                "pass": "drop_insertion",
                "functions": ["bench_exception_heavy_main"],
                "score": 9,
                "added_exception_events": 9,
            },
        ]
    }
    attr = causality.derive_attribution(
        "tests/benchmarks/bench_fib.py",
        pass_delta_dashboard=dashboard,
    )

    assert attr.pass_delta_score == 5
    assert attr.pass_delta_passes == ("unboxing",)
    assert attr.pass_delta_fact_classes == ("repr_tir_type_lattice",)
    assert attr.evidence_sources == ("pass_delta_dashboard",)


def test_call_fact_coverage_joins_call_fact_attribution() -> None:
    attr = causality.derive_attribution(
        "tests/benchmarks/bench_attr_access.py",
        call_fact_coverage={"attached": 5, "transient": 2},
    )

    assert attr.fact_class == "call_facts"
    assert attr.source == "benchmark_taxonomy"
    assert attr.call_fact_attached == 5
    assert attr.call_fact_transient == 2
    assert attr.evidence_sources == ("call_fact_coverage",)


def test_pass_delta_incompatible_signal_does_not_override_primary_class() -> None:
    dashboard = {
        "by_pass": [
            {
                "pass": "drop_insertion",
                "functions": ["bench_fib_main"],
                "score": 9,
                "added_exception_events": 9,
            }
        ]
    }

    attr = causality.derive_attribution(
        "tests/benchmarks/bench_fib.py",
        pass_delta_dashboard=dashboard,
    )

    assert attr.fact_class == "repr_tir_type_lattice"
    assert attr.source == "benchmark_taxonomy"
    assert attr.pass_delta_score == 0
    assert attr.pass_delta_passes == ()
    assert attr.pass_delta_fact_classes == ()
    assert attr.evidence_sources == ()
    assert attr.attribution_confidence == 0.35


def test_pass_delta_adjacent_repr_signal_supports_shape_fact_attribution() -> None:
    dashboard = {
        "by_pass": [
            {
                "pass": "string_shape_lowering",
                "functions": ["bench_etl_orders_main"],
                "score": 3,
                "added_box_ops": 1,
            }
        ]
    }

    attr = causality.derive_attribution(
        "tests/benchmarks/bench_etl_orders.py",
        pass_delta_dashboard=dashboard,
    )

    assert attr.fact_class == "shape_facts"
    assert attr.source == "benchmark_taxonomy"
    assert attr.pass_delta_score == 3
    assert attr.pass_delta_fact_classes == ("repr_tir_type_lattice",)
    assert attr.attribution_confidence == 0.5
