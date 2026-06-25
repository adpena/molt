from __future__ import annotations

import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import fact_graph_dump as fg  # noqa: E402


def _graph() -> dict:
    return {
        "schema_version": 2,
        "kind": "molt_tir_fact_graph",
        "function": "sample",
        "values": [
            {
                "value": 0,
                "producer": {"kind": "parameter", "block": 0, "source_site": None},
                "facts": [
                    {
                        "kind": "tir_type",
                        "value": "I64",
                        "confidence": "proven",
                        "producer": "block_arg",
                        "event_id": None,
                        "source_site": None,
                        "guards": [],
                        "invalidators": ["value_types"],
                        "backend_lowering_status": "type-guides-representation",
                        "test_coverage": "unit",
                        "perf_relevance": "typed values steer carriers",
                    },
                    {
                        "kind": "repr_floor",
                        "value": "MaybeBigInt",
                        "confidence": "proven_floor",
                        "producer": "Repr::default_for(TirType)",
                        "event_id": None,
                        "source_site": None,
                        "guards": [],
                        "invalidators": ["value_types", "repr_lattice"],
                        "backend_lowering_status": "conservative-carrier-floor",
                        "test_coverage": "unit",
                        "perf_relevance": "explains boxed integer carrier",
                    },
                ],
                "consumers": [
                    {
                        "kind": "op_operand",
                        "block": 0,
                        "op_index": 0,
                        "opcode": "Add",
                        "operand_index": 0,
                        "role": "operand[0]",
                        "source_site": {"line": 7, "col": 4, "end_col": 9},
                    }
                ],
            },
            {
                "value": 1,
                "producer": {
                    "kind": "op_result",
                    "block": 0,
                    "op_index": 0,
                    "opcode": "Call",
                    "result_index": 0,
                    "source_site": {"line": 7, "col": 4, "end_col": 9},
                },
                "facts": [
                    {
                        "kind": "call.target",
                        "value": "Opaque",
                        "confidence": "unknown",
                        "producer": "CallFactsTable::build_local",
                        "event_id": "sample:bb0:op0:Call:result0:call.target",
                        "source_site": {"line": 7, "col": 4, "end_col": 9},
                        "guards": [],
                        "invalidators": ["AnalysisId::CallFacts:ops_sensitive"],
                        "backend_lowering_status": "advisory",
                        "test_coverage": "unit",
                        "perf_relevance": "generic call fallback",
                    },
                    {
                        "kind": "allocation.heap_root",
                        "value": "escape_alloc_site",
                        "confidence": "proven",
                        "producer": "op_kinds.escape_alloc_site_opcodes",
                        "event_id": "sample:bb0:op0:Call:result0:allocation.heap_root",
                        "source_site": {"line": 7, "col": 4, "end_col": 9},
                        "guards": [],
                        "invalidators": ["op_kinds.toml"],
                        "backend_lowering_status": "diagnostic",
                        "test_coverage": "unit",
                        "perf_relevance": "allocation attribution",
                    },
                ],
                "consumers": [],
            },
        ],
        "edges": [
            {
                "from_value": 0,
                "to_value": 1,
                "kind": "op_operand_to_result",
                "consumer": {
                    "kind": "op_operand",
                    "block": 0,
                    "op_index": 0,
                    "opcode": "Add",
                    "operand_index": 0,
                    "role": "operand[0]",
                    "source_site": {"line": 7, "col": 4, "end_col": 9},
                },
            }
        ],
        "summary": {
            "value_count": 2,
            "fact_count": 4,
            "edge_count": 1,
            "call_fact_count": 1,
            "source_site_value_count": 2,
            "allocation_ownership_fact_count": 1,
        },
    }


def test_validate_graph_accepts_compiler_emitted_contract() -> None:
    doc = _graph()

    fg.validate_graph(doc)

    text = fg.summarize_graph(doc)
    assert "molt_tir_fact_graph schema=2" in text
    assert "%1 producer=op_result:bb0:op0:Call" in text


def test_why_boxed_reports_only_boxed_repr_values() -> None:
    doc = _graph()

    rows = fg.boxed_rows(doc)
    text = fg.summarize_graph(doc, why_boxed=True)

    assert [row["value"] for row in rows] == [0]
    assert "%0 producer=parameter:bb0" in text
    assert "%1 producer=" not in text


def test_validate_graph_rejects_stale_summary() -> None:
    doc = _graph()
    doc["summary"]["fact_count"] = 999

    try:
        fg.validate_graph(doc)
    except fg.FactGraphError as exc:
        assert "summary.fact_count=999, expected 4" in str(exc)
    else:
        raise AssertionError("invalid summary accepted")


def test_validate_graph_rejects_duplicate_event_ids() -> None:
    doc = _graph()
    facts = doc["values"][1]["facts"]
    facts[1]["event_id"] = facts[0]["event_id"]

    try:
        fg.validate_graph(doc)
    except fg.FactGraphError as exc:
        assert "event_id is duplicated" in str(exc)
    else:
        raise AssertionError("duplicate event_id accepted")


def test_cli_validates_and_emits_json(tmp_path: Path, capsys) -> None:
    path = tmp_path / "graph.json"
    path.write_text(json.dumps(_graph()), encoding="utf-8")

    rc = fg.main([str(path), "--json"])
    payload = json.loads(capsys.readouterr().out)

    assert rc == 0
    assert payload["kind"] == "molt_tir_fact_graph"
