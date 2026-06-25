"""Gate + robustness tests for tools/structural_audit.py.

Two jobs (the tools/audit_op_kinds.py + tests/test_gen_op_kinds.py pattern):

  1. RATCHET GATE: the live tree's structural-debt metrics never EXCEED the
     committed baseline (tools/structural_audit_baseline.json). New god-file
     bloat, debt markers, or hand-maintained opcode classifications fail here.
  2. ROBUSTNESS: the Rust-scanning helpers that the gate depends on are unit-
     tested against synthetic inputs, so a parser regression cannot silently
     zero-out the metrics (a tool that finds nothing must be PROVEN to find
     nothing, never broken into finding nothing).

Run: pytest -q tests/test_structural_audit.py
CI : python3 tools/structural_audit.py --check  (the same gate, exit-coded)
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "tools" / "structural_audit.py"
BASELINE = ROOT / "tools" / "structural_audit_baseline.json"


def _load_tool():
    spec = importlib.util.spec_from_file_location("molt_test_structural_audit", TOOL)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_structural_audit"] = module
    spec.loader.exec_module(module)
    return module


SA = _load_tool()


# --- 1. the ratchet gate --------------------------------------------------


def test_baseline_exists():
    assert BASELINE.is_file(), (
        "no structural_audit_baseline.json — run "
        "`python3 tools/structural_audit.py --update-baseline`"
    )


def test_structural_debt_does_not_exceed_baseline():
    """The CI ratchet, as a pytest. Every metric may only go DOWN."""
    findings = SA.run_all(ROOT)
    metrics = SA.ratchet_metrics(findings)
    baseline = json.loads(BASELINE.read_text())
    regressions = {
        k: (baseline.get(k, 0.0), v)
        for k, v in metrics.items()
        if v > baseline.get(k, 0.0)
    }
    assert not regressions, (
        "structural ratchet regressed (new hand-maintained debt added):\n"
        + "\n".join(f"  {k}: {b} -> {c}" for k, (b, c) in regressions.items())
        + "\nResolve it, or justify and re-pin with --update-baseline."
    )


def test_tooling_gaps_reflect_current_fact_attribution_tools(tmp_path: Path):
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "call_fact_coverage.py").write_text("", encoding="utf-8")
    (tools / "perf_causality.py").write_text("", encoding="utf-8")

    gaps = dict(SA._tooling_gaps(tmp_path))

    assert "PARTIAL: fact-by-benchmark attribution" in gaps
    assert (
        "tools/perf_causality.py (#76 cycle-profile attribution"
        in gaps["PARTIAL: fact-by-benchmark attribution"]
    )
    assert "MISSING: pass-delta ledger" in gaps
    assert "perf_causality.py (not built)" not in "\n".join(gaps.values())


def test_tooling_gaps_credit_pass_delta_when_present(tmp_path: Path):
    tools = tmp_path / "tools"
    tools.mkdir()
    for rel in (
        "call_fact_coverage.py",
        "perf_causality.py",
        "pass_delta_dashboard.py",
    ):
        (tools / rel).write_text("", encoding="utf-8")

    gaps = dict(SA._tooling_gaps(tmp_path))

    assert "BUILT: fact-by-benchmark attribution substrate" in gaps
    assert "MISSING: pass-delta ledger" not in gaps


def test_tooling_gaps_credit_fact_graph_when_both_halves_exist(tmp_path: Path):
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "fact_graph_dump.py").write_text("", encoding="utf-8")
    fact_graph = tmp_path / "runtime" / "molt-tir" / "src" / "tir"
    fact_graph.mkdir(parents=True)
    (fact_graph / "fact_graph.rs").write_text("", encoding="utf-8")

    gaps = dict(SA._tooling_gaps(tmp_path))

    assert "BUILT: fact graph substrate" in gaps
    assert "MISSING: fact graph" not in gaps


def test_tooling_gaps_keep_fact_graph_missing_when_only_one_half_exists(tmp_path: Path):
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "fact_graph_dump.py").write_text("", encoding="utf-8")

    gaps = dict(SA._tooling_gaps(tmp_path))

    assert "MISSING: fact graph" in gaps
    assert "BUILT: fact graph substrate" not in gaps


def test_format_board_uses_root_specific_tooling_gaps(tmp_path: Path):
    tools = tmp_path / "tools"
    tools.mkdir()
    (tools / "call_fact_coverage.py").write_text("", encoding="utf-8")
    (tools / "perf_causality.py").write_text("", encoding="utf-8")

    board = SA.format_board([], SA.ratchet_metrics([]), root=tmp_path)

    assert "**PARTIAL: fact-by-benchmark attribution**" in board
    assert "perf_causality.py (not built)" not in board


# --- 2. robustness of the scanning helpers --------------------------------


def test_failloud_default_is_not_flagged():
    """A dispatch switchboard with a fail-loud default is the CORRECT pattern
    and must NOT be reported as drift."""
    rust = (
        "fn lower(op: &TirOp) {\n"
        "    match op.opcode {\n"
        "        OpCode::Add => emit_add(),\n"
        "        OpCode::Sub => emit_sub(),\n"
        "        OpCode::Mul => emit_mul(),\n"
        '        _ => panic!("unsupported opcode {:?}", op.opcode),\n'
        "    }\n"
        "}\n"
    )
    findings = _scan_rust_string(rust, "llvm_backend/lowering.rs")
    assert findings == [], f"fail-loud dispatch wrongly flagged: {findings}"


def test_emitter_default_is_not_flagged():
    """A default arm that emits code (mechanical lowering route) is not a
    semantic classification and must not be flagged."""
    rust = (
        "fn lower(&self, op: &TirOp) {\n"
        "    match op.opcode {\n"
        "        OpCode::A => self.a(),\n"
        "        OpCode::B => self.b(),\n"
        "        OpCode::C => self.c(),\n"
        "        _ => { let v = self.backend.generic_lower(op); v }\n"
        "    }\n"
        "}\n"
    )
    findings = _scan_rust_string(rust, "llvm_backend/lowering.rs")
    assert findings == [], f"emitter fallback wrongly flagged: {findings}"


def test_silent_classifier_default_is_flagged():
    """A classifier with a silent VALUE default IS the drift surface."""
    rust = (
        "fn opcode_is_special(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
        "        OpCode::A => true,\n"
        "        OpCode::B => true,\n"
        "        OpCode::C => true,\n"
        "        _ => false,\n"
        "    }\n"
        "}\n"
    )
    findings = _scan_rust_string(rust, "tir/passes/effects.rs")
    assert len(findings) == 1, f"expected 1 classifier finding, got {findings}"
    assert findings[0].probe == "semantic_fallthrough"
    assert "hand-classified" in findings[0].title


def test_exhaustive_match_is_not_flagged():
    """A match with NO wildcard is rustc-gated and must not be flagged."""
    rust = (
        "fn f(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
        "        OpCode::A => true,\n"
        "        OpCode::B => false,\n"
        "        OpCode::C => true,\n"
        "    }\n"
        "}\n"
    )
    findings = _scan_rust_string(rust, "tir/passes/effects.rs")
    assert findings == [], f"exhaustive match wrongly flagged: {findings}"


def test_nested_data_default_inside_exhaustive_opcode_match_is_not_flagged():
    """Nested `_ => fallback` data decoders inside an exhaustive opcode dispatch
    are not opcode-classifier defaults."""
    rust = (
        "fn emit(op: &TirOp) {\n"
        "    match op.opcode {\n"
        "        OpCode::ConstInt => {\n"
        '            let value = match op.attrs.get("value") {\n'
        "                Some(AttrValue::Int(v)) => *v,\n"
        "                _ => 0,\n"
        "            };\n"
        "            emit_const(value);\n"
        "        }\n"
        "        OpCode::Add => emit_add(),\n"
        "        OpCode::Sub => emit_sub(),\n"
        "    }\n"
        "}\n"
    )
    findings = _scan_rust_string(rust, "tir/lower_to_wasm.rs")
    assert findings == [], f"nested data fallback wrongly flagged: {findings}"


def test_enum_variant_extraction_handles_payloads():
    """Tuple/struct variants and discriminants must not break the parser, and
    commas inside payloads must not split variants."""
    rust = (
        "pub enum OpCode {\n"
        "    Add,\n"
        "    Call(String),\n"
        "    Phi { block: usize, args: Vec<u32> },\n"
        "    Const = 7,\n"
        '    #[doc = "x"]\n'
        "    Last,\n"
        "}\n"
    )
    variants = SA._count_enum_variants(rust, "OpCode")
    assert variants == {"Add", "Call", "Phi", "Const", "Last"}, variants


def test_large_single_cohesive_region_is_not_structural_god_file(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    src.mkdir(parents=True)
    (src / "cohesive.rs").write_text(_rust_impl("Cohesive", 420), encoding="utf-8")

    findings = SA.probe_structural_god_files(tmp_path, ceiling=100)

    assert findings == []


def test_large_multi_region_rust_file_is_structural_god_file(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    src.mkdir(parents=True)
    (src / "mixed.rs").write_text(
        "\n".join(
            [
                _rust_impl("Alpha", 260),
                _rust_fn("lower_alpha", 270),
                _rust_fn("emit_alpha", 280),
            ]
        ),
        encoding="utf-8",
    )

    findings = SA.probe_structural_god_files(tmp_path, ceiling=100)

    assert len(findings) == 1
    assert findings[0].probe == "structural_god_file"
    assert findings[0].location.endswith("mixed.rs")
    assert findings[0].metric >= 60
    assert "3 large top-level regions" in findings[0].title


def test_cfg_test_module_does_not_create_structural_god_file(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    src.mkdir(parents=True)
    tests_body = "\n".join("    // fixture line" for _ in range(420))
    (src / "fixtures.rs").write_text(
        "pub fn production() {}\n"
        "#[cfg(test)]\n"
        "mod tests {\n"
        f"{tests_body}\n"
        "}\n",
        encoding="utf-8",
    )

    findings = SA.probe_structural_god_files(tmp_path, ceiling=100)

    assert findings == []


def test_python_module_regions_drive_structural_god_file(tmp_path: Path):
    pkg = tmp_path / "src" / "molt"
    pkg.mkdir(parents=True)
    (pkg / "mixed.py").write_text(
        "\n".join(
            [
                _python_function("alpha", 260),
                _python_function("beta", 270),
                _python_class("Gamma", 280),
            ]
        ),
        encoding="utf-8",
    )

    findings = SA.probe_structural_god_files(
        tmp_path,
        ceiling=100,
        py_ceiling=100,
    )

    assert len(findings) == 1
    assert findings[0].location.endswith("mixed.py")
    assert findings[0].metric == 60
    assert "3 large top-level regions" in findings[0].title


def test_generated_large_file_is_not_structural_god_file(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    src.mkdir(parents=True)
    (src / "generated.rs").write_text(
        "// DO NOT EDIT\n"
        + "\n".join(
            [
                _rust_impl("Alpha", 260),
                _rust_fn("lower_alpha", 270),
                _rust_fn("emit_alpha", 280),
            ]
        ),
        encoding="utf-8",
    )

    findings = SA.probe_structural_god_files(tmp_path, ceiling=100)

    assert findings == []


def test_structural_god_metrics_are_ratchet_metrics():
    findings = [
        SA.Finding(
            probe="structural_god_file",
            severity="medium",
            title="4 large top-level regions (900 excess lines)",
            location="runtime/example.rs",
            detail="",
            suggested_action="",
            metric=900,
        )
    ]

    metrics = SA.ratchet_metrics(findings)

    assert metrics["structural_god_files"] == 1
    assert metrics["max_god_file_structural_score"] == 900
    assert metrics["god_file_large_regions"] == 4


def _scan_rust_string(rust: str, rel: str) -> list:
    """Drive probe_semantic_fallthroughs over an in-memory file by writing it to
    a temp tree mirroring the expected relative path (the probe walks the FS)."""
    import tempfile

    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        target = root / "runtime" / "molt-backend" / "src" / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(rust)
        return [
            f
            for f in SA.probe_semantic_fallthroughs(root)
            if f.title.startswith("hand-classified")
        ]


def _rust_impl(name: str, span: int) -> str:
    body = "\n".join("    // region body" for _ in range(span - 2))
    return f"impl {name} {{\n{body}\n}}\n"


def _rust_fn(name: str, span: int) -> str:
    body = "\n".join("    // region body" for _ in range(span - 2))
    return f"fn {name}() {{\n{body}\n}}\n"


def _python_function(name: str, span: int) -> str:
    body = "\n".join("    value = 1" for _ in range(span - 1))
    return f"def {name}():\n{body}\n"


def _python_class(name: str, span: int) -> str:
    body = "\n".join("    value = 1" for _ in range(span - 1))
    return f"class {name}:\n{body}\n"


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
