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
    fact_graph = tmp_path / "runtime" / "molt-passes" / "src" / "tir"
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


def test_debt_probe_ignores_domain_temporary_and_tool_regex_strings(tmp_path: Path):
    pkg = tmp_path / "src" / "molt"
    pkg.mkdir(parents=True)
    (pkg / "tempfile.py").write_text(
        '"""Return a temporary file name."""\n'
        "TODO_RE = r'TODO(owner): parser contract, not a live debt marker'\n"
        "# Default prefix for temporary file/directory names.\n",
        encoding="utf-8",
    )

    findings = SA.probe_debt_markers(tmp_path)

    assert findings == []


def test_debt_probe_counts_comments_and_rust_macros(tmp_path: Path):
    pkg = tmp_path / "src" / "molt"
    pkg.mkdir(parents=True)
    (pkg / "feature.py").write_text(
        "# TODO(compiler): route through generated facts\n"
        "TEXT = 'TODO in a user-facing string is not a marker'\n",
        encoding="utf-8",
    )
    rust = tmp_path / "runtime" / "molt-runtime" / "src"
    rust.mkdir(parents=True)
    (rust / "lib.rs").write_text(
        'const TEXT: &str = "todo!(not code)";\n'
        "pub fn missing() { todo!(\"real implementation\"); }\n",
        encoding="utf-8",
    )

    findings = SA.probe_debt_markers(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert metrics["debt_markers_total"] == 2
    assert {finding.location for finding in findings} == {
        "runtime/molt-runtime/src/lib.rs:2",
        "src/molt/feature.py:1",
    }


def test_debt_probe_ignores_bare_upstream_stdlib_xxx_not_owned_debt(tmp_path: Path):
    stdlib = tmp_path / "src" / "molt" / "stdlib"
    stdlib.mkdir(parents=True)
    (stdlib / "_pyio.py").write_text(
        "# XXX Should this return the number of bytes written???\n"
        "# XXX: this is a bit of a hack; keep counting owned debt words\n"
        "# FIXME: replace with generated parser facts\n",
        encoding="utf-8",
    )

    findings = SA.probe_debt_markers(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert metrics["debt_markers_total"] == 2
    assert findings[0].location == "src/molt/stdlib/_pyio.py:2"
    assert findings[0].detail == "L2:hack, L3:FIXME"


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


def test_generated_opcode_table_role_match_is_not_flagged():
    """Generated-role consumers are not hand-maintained opcode authorities."""
    rust = (
        "fn scan(op: &TirOp) {\n"
        "    match opcode_module_slot_access_role_table(op.opcode) {\n"
        "        ModuleSlotAccessRole::KeyedAttr => {\n"
        "            let is_set = op.opcode == OpCode::ModuleSetAttr;\n"
        "            if is_set { record_set(); }\n"
        "        }\n"
        "        _ if op.opcode == OpCode::CheckException => {}\n"
        "        _ => {}\n"
        "    }\n"
        "}\n"
    )
    findings = _scan_rust_string(rust, "tir/passes/module_slot_promotion.rs")
    assert findings == [], f"generated role match wrongly flagged: {findings}"


def test_type_to_opcode_constructor_match_is_not_flagged():
    """A non-opcode scrutinee that constructs opcodes is not an opcode classifier."""
    rust = (
        "fn placeholder(ty: &TirType) -> OpCode {\n"
        "    match ty {\n"
        "        TirType::I64 => OpCode::ConstInt,\n"
        "        TirType::Bool => OpCode::ConstBool,\n"
        "        TirType::F64 => OpCode::ConstFloat,\n"
        "        _ => OpCode::ConstNone,\n"
        "    }\n"
        "}\n"
    )
    findings = _scan_rust_string(rust, "tir/ops.rs")
    assert findings == [], f"type-to-opcode constructor wrongly flagged: {findings}"


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


def test_large_single_cohesive_region_is_not_kitchen_sink_file(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    src.mkdir(parents=True)
    (src / "cohesive.rs").write_text(_rust_impl("Cohesive", 420), encoding="utf-8")

    findings = SA.probe_kitchen_sink_files(tmp_path, ceiling=100)

    assert findings == []


def test_large_multi_region_rust_file_is_kitchen_sink_file(tmp_path: Path):
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

    findings = SA.probe_kitchen_sink_files(tmp_path, ceiling=100)

    assert len(findings) == 1
    assert findings[0].probe == "kitchen_sink_file"
    assert findings[0].location.endswith("mixed.rs")
    assert findings[0].metric >= 60
    assert "3 large top-level regions" in findings[0].title

    undecomposed = SA.probe_undecomposed_god_files(tmp_path, ceiling=100)
    assert len(undecomposed) == 1
    assert undecomposed[0].probe == "undecomposed_god_file"
    assert undecomposed[0].location.endswith("mixed.rs")


def test_cfg_test_module_does_not_create_kitchen_sink_file(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    src.mkdir(parents=True)
    tests_body = "\n".join("    // fixture line" for _ in range(420))
    (src / "fixtures.rs").write_text(
        f"pub fn production() {{}}\n#[cfg(test)]\nmod tests {{\n{tests_body}\n}}\n",
        encoding="utf-8",
    )

    findings = SA.probe_kitchen_sink_files(tmp_path, ceiling=100)

    assert findings == []


def test_python_module_regions_drive_kitchen_sink_file(tmp_path: Path):
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

    findings = SA.probe_kitchen_sink_files(
        tmp_path,
        ceiling=100,
        py_ceiling=100,
    )

    assert len(findings) == 1
    assert findings[0].location.endswith("mixed.py")
    assert findings[0].metric == 60
    assert "3 large top-level regions" in findings[0].title


def test_generated_large_file_is_not_kitchen_sink_file(tmp_path: Path):
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

    findings = SA.probe_kitchen_sink_files(tmp_path, ceiling=100)

    assert findings == []


def test_kitchen_sink_metrics_are_ratchet_metrics():
    findings = [
        SA.Finding(
            probe="kitchen_sink_file",
            severity="medium",
            title="4 large top-level regions (900 excess lines)",
            location="runtime/example.rs",
            detail="",
            suggested_action="",
            metric=900,
        )
    ]

    metrics = SA.ratchet_metrics(findings)

    assert metrics["kitchen_sink_files"] == 1
    assert metrics["max_kitchen_sink_structural_score"] == 900
    assert metrics["kitchen_sink_large_regions"] == 4


def test_cohesive_sibling_package_is_credited_not_ratcheted(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src" / "lowering"
    src.mkdir(parents=True)
    for idx in range(4):
        (src / f"family_{idx}.rs").write_text(
            _rust_fn(f"family_{idx}", 120),
            encoding="utf-8",
        )

    large = SA.probe_large_source_files(tmp_path, ceiling=100)
    kitchen = SA.probe_kitchen_sink_files(tmp_path, ceiling=100)
    undecomposed = SA.probe_undecomposed_god_files(tmp_path, ceiling=100)
    metrics = SA.ratchet_metrics(large + kitchen + undecomposed)

    assert len(large) == 4
    assert all("sibling-rich package" in finding.detail for finding in large)
    assert kitchen == []
    assert undecomposed == []
    assert metrics["kitchen_sink_files"] == 0
    assert metrics["undecomposed_god_files"] == 0
    assert metrics["max_undecomposed_file_lines"] == 0


def test_residual_with_decomposition_directory_is_reported_not_max_ratcheted(
    tmp_path: Path,
):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    family = src / "lowering"
    family.mkdir(parents=True)
    (src / "lowering.rs").write_text(_rust_impl("Residual", 120), encoding="utf-8")
    for idx in range(4):
        (family / f"part_{idx}.rs").write_text(
            _rust_fn(f"part_{idx}", 40),
            encoding="utf-8",
        )

    large = SA.probe_large_source_files(tmp_path, ceiling=100)
    undecomposed = SA.probe_undecomposed_god_files(tmp_path, ceiling=100)
    metrics = SA.ratchet_metrics(large + undecomposed)

    assert [finding.location for finding in large] == [
        "runtime/molt-backend/src/lowering.rs"
    ]
    assert "decomposition directory `lowering/`" in large[0].detail
    assert undecomposed == []
    assert metrics["undecomposed_god_files"] == 0
    assert metrics["max_undecomposed_file_lines"] == 0


def test_honest_debt_union_covers_lone_large_files(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend" / "src"
    src.mkdir(parents=True)
    (src / "cohesive.rs").write_text(_rust_impl("Cohesive", 120), encoding="utf-8")
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

    large = SA.probe_large_source_files(tmp_path, ceiling=100)
    kitchen = SA.probe_kitchen_sink_files(tmp_path, ceiling=100)
    undecomposed = SA.probe_undecomposed_god_files(tmp_path, ceiling=100)

    raw_lone = {
        finding.location
        for finding in large
        if "no decomposition context detected" in finding.detail
    }
    honest_debt = {finding.location for finding in kitchen + undecomposed}
    assert raw_lone <= honest_debt


def test_native_scalar_plan_authority_ratchets_side_set_clones(tmp_path: Path):
    target = (
        tmp_path
        / "runtime"
        / "molt-backend-native"
        / "src"
        / "native_backend"
        / "function_compiler"
        / "fc"
        / "arith.rs"
    )
    target.parent.mkdir(parents=True)
    target.write_text(
        """
fn lowered(representation_plan: &ScalarRepresentationPlan) {
    let bool_primary_vars = representation_plan.primary_name_sets().bool_;
    let float_primary_vars = representation_plan.primary_name_sets().float;
    let int_carriers_plan = representation_plan;
    drop((bool_primary_vars, float_primary_vars, int_carriers_plan));
}
""",
        encoding="utf-8",
    )

    findings = SA.probe_native_scalar_plan_authority(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert len(findings) == 4
    assert {
        "raw-bool membership cloned out of ScalarRepresentationPlan",
        "raw-f64 membership cloned out of ScalarRepresentationPlan",
        "legacy plan alias beside ScalarRepresentationPlan",
        "native backend cloned primary-name sets instead of plan predicates",
    } == {finding.detail for finding in findings}
    assert metrics["native_scalar_plan_authority_violations"] == 8


def test_native_scalar_plan_authority_allows_direct_plan_predicates(tmp_path: Path):
    target = (
        tmp_path
        / "runtime"
        / "molt-backend-native"
        / "src"
        / "native_backend"
        / "function_compiler"
        / "scalar_carriers.rs"
    )
    target.parent.mkdir(parents=True)
    target.write_text(
        """
fn lowered(representation_plan: &ScalarRepresentationPlan, name: &str) -> bool {
    representation_plan.is_raw_int_carrier_name(name)
        || representation_plan.is_bool_unboxed(name)
        || representation_plan.is_float_unboxed(name)
}
""",
        encoding="utf-8",
    )

    findings = SA.probe_native_scalar_plan_authority(tmp_path)

    assert findings == []


def test_repr_name_scalar_authority_ratchets_bool_float_side_stores(tmp_path: Path):
    target = tmp_path / "runtime" / "molt-tir" / "src" / "representation_plan.rs"
    target.parent.mkdir(parents=True)
    target.write_text(
        """
struct ScalarRepresentationPlan {
    repr_by_name: PlanHashMap<String, Repr>,
    bool_primary_names: PlanHashSet<String>,
    float_primary_names: PlanHashSet<String>,
}
""",
        encoding="utf-8",
    )

    findings = SA.probe_repr_name_scalar_authority(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert len(findings) == 2
    assert {
        "raw-bool membership stored beside repr_by_name",
        "raw-f64 membership stored beside repr_by_name",
    } == {finding.detail for finding in findings}
    assert metrics["repr_name_scalar_authority_violations"] == 2


def test_repr_name_scalar_authority_allows_map_views_and_computation(tmp_path: Path):
    target = tmp_path / "runtime" / "molt-tir" / "src" / "representation_plan.rs"
    target.parent.mkdir(parents=True)
    target.write_text(
        """
struct ScalarRepresentationPlan {
    repr_by_name: PlanHashMap<String, Repr>,
}

impl ScalarRepresentationPlan {
    fn compute_bool_primary_names(&self) {}
    fn compute_float_primary_names(&self) {}
    fn is_bool_unboxed(&self, name: &str) -> bool {
        self.repr_by_name.get(name).is_some_and(|repr| repr.is_bool_carrier())
    }
    fn is_float_unboxed(&self, name: &str) -> bool {
        self.repr_by_name.get(name).is_some_and(|repr| repr.is_float_unboxed())
    }
}
""",
        encoding="utf-8",
    )

    findings = SA.probe_repr_name_scalar_authority(tmp_path)

    assert findings == []


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
