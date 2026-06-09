"""Gate + robustness tests for tools/call_fact_coverage.py.

  1. EVIDENCE GATE: every fact in the CALL_FACTS registry cites a symbol that
     still exists in the live tree (the registry cannot silently rot).
  2. RATCHET GATE: the count of ATTACHED (call-op-recorded) facts never drops
     below the committed baseline — a fact may not silently un-attach.
  3. ROBUSTNESS: the typed_repr_report corpus parser computes typed-return %
     correctly from a synthetic dump.

Run: pytest -q tests/test_call_fact_coverage.py
CI : python3 tools/call_fact_coverage.py --check
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "tools" / "call_fact_coverage.py"
BASELINE = ROOT / "tools" / "call_fact_coverage_baseline.json"


def _load_tool():
    spec = importlib.util.spec_from_file_location("molt_test_call_fact_coverage", TOOL)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_call_fact_coverage"] = module
    spec.loader.exec_module(module)
    return module


CFC = _load_tool()


def test_registry_evidence_is_not_stale():
    """Every CALL_FACTS entry must cite a symbol present in the live tree."""
    c = CFC.census(ROOT)
    assert not c["stale_evidence"], (
        "CALL_FACTS evidence rotted — these facts cite a symbol no longer in "
        f"the tree: {c['stale_evidence']}. Update CALL_FACTS."
    )


def test_attached_facts_do_not_regress():
    c = CFC.census(ROOT)
    assert BASELINE.is_file(), "run --update-baseline first"
    base = json.loads(BASELINE.read_text())
    assert c["attached"] >= base["attached"], (
        f"call-fact representation regressed: attached {base['attached']} -> "
        f"{c['attached']} (a fact un-attached from the call op)"
    )


def test_council_named_facts_are_present():
    """The council's Q4 explicitly names direct/leaf/no-throw/no-alloc/inlinable;
    all must be in the registry so the scoreboard answers the question."""
    keys = {f.key for f in CFC.CALL_FACTS}
    for required in ("direct_target", "leaf", "no_throw", "no_alloc", "inlinable"):
        assert required in keys, f"council-named call fact missing: {required}"


def test_corpus_typed_return_parse():
    """typed-return % = non-dynbox result reprs / total, across call opcodes."""
    import tempfile

    doc = {
        "functions": [
            {"stats": {"opcodes": {
                "call": {"result_reprs": {"dynbox": 3, "i64": 1}, "boxed_result_values": 3},
                "call_method": {"result_reprs": {"i64": 4}},
                "add": {"result_reprs": {"i64": 99}},  # non-call, must be ignored
            }}},
        ]
    }
    with tempfile.TemporaryDirectory() as td:
        p = Path(td) / "rep.json"
        p.write_text(json.dumps(doc))
        out = CFC._corpus_typed_return([p])
    # call result reprs: dynbox 3 + i64 1 + i64 4 = 8 total; typed = 1 + 4 = 5
    assert out["call_result_reprs_total"] == 8
    assert out["call_result_reprs_typed"] == 5
    assert out["typed_return_pct"] == 62.5
    assert out["boxed_result_values"] == 3


def test_may_throw_read_from_registry_not_hardcoded():
    """The per-opcode may_throw must come from op_kinds.toml (authoritative)."""
    mt = CFC._call_opcode_may_throw(ROOT)
    assert "Call" in mt and mt["Call"] is True, mt


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
