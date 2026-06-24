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
