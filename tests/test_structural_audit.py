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

import ast
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


def test_iter_source_files_prunes_excluded_directories_before_descent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    runtime = tmp_path / "runtime"
    runtime.mkdir()
    (runtime / "live.rs").write_text("fn live() {}\n", encoding="utf-8")
    venv = runtime / ".venv"
    venv.mkdir()
    (venv / "ignored.rs").write_text("fn ignored() {}\n", encoding="utf-8")

    real_iterdir = Path.iterdir

    def guarded_iterdir(path: Path):
        if path == venv:
            raise AssertionError("structural_audit descended into excluded .venv")
        return real_iterdir(path)

    monkeypatch.setattr(Path, "iterdir", guarded_iterdir)

    files = [
        path.relative_to(tmp_path).as_posix()
        for path in SA._iter_source_files(tmp_path, (".rs",))
    ]

    assert files == ["runtime/live.rs"]


@pytest.mark.slow
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


def test_update_baseline_and_write_board_share_one_cli_scan(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
):
    (tmp_path / "tools").mkdir(parents=True)
    (tmp_path / "docs" / "design" / "foundation").mkdir(parents=True)
    (tmp_path / "runtime" / "molt-runtime" / "src").mkdir(parents=True)
    (tmp_path / "runtime" / "molt-runtime" / "src" / "lib.rs").write_text(
        'pub fn missing() { todo!("tracked implementation"); }\n',
        encoding="utf-8",
    )

    assert (
        SA.main(
            [
                "--root",
                str(tmp_path),
                "--update-baseline",
                "--write-board",
            ]
        )
        == 0
    )

    baseline = json.loads(
        (tmp_path / "tools" / "structural_audit_baseline.json").read_text(
            encoding="utf-8"
        )
    )
    board = (
        tmp_path / "docs" / "design" / "foundation" / "STRUCTURAL_AUDIT_BOARD.md"
    ).read_text(encoding="utf-8")
    output = capsys.readouterr().out

    assert baseline["debt_markers_total"] == 1
    assert "| debt_markers_total | 1 |" in board
    assert "runtime/molt-runtime/src/lib.rs:1" in board
    assert "baseline updated:" in output
    assert "board written:" in output


def test_path_scope_json_limits_findings_and_metrics(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
):
    pkg = tmp_path / "src" / "molt"
    pkg.mkdir(parents=True)
    (pkg / "selected.py").write_text(
        "# TODO(compiler): selected path debt marker\n", encoding="utf-8"
    )
    (pkg / "unselected.py").write_text(
        "# FIXME(compiler): unselected path debt marker\n", encoding="utf-8"
    )

    assert (
        SA.main(
            [
                "--root",
                str(tmp_path),
                "--path",
                "src/molt/selected.py",
                "--json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)

    assert payload["path_scope"] == ["src/molt/selected.py"]
    assert payload["metrics"]["debt_markers_total"] == 1
    assert [finding["location"] for finding in payload["findings"]] == [
        "src/molt/selected.py:1"
    ]


def test_path_scope_is_diagnostic_only_not_a_ratchet_check(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
):
    pkg = tmp_path / "src" / "molt"
    pkg.mkdir(parents=True)
    (pkg / "selected.py").write_text(
        "# TODO(compiler): selected path debt marker\n", encoding="utf-8"
    )

    rc = SA.main(
        [
            "--root",
            str(tmp_path),
            "--path",
            "src/molt/selected.py",
            "--check",
        ]
    )

    captured = capsys.readouterr()
    assert rc == 2
    assert "--path is diagnostic-only" in captured.err


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
        'pub fn missing() { todo!("real implementation"); }\n',
        encoding="utf-8",
    )

    findings = SA.probe_debt_markers(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert metrics["debt_markers_total"] == 2
    assert {finding.location for finding in findings} == {
        "runtime/molt-runtime/src/lib.rs:2",
        "src/molt/feature.py:1",
    }


def test_rust_debt_consumer_preserves_exact_hits_with_one_lexical_scan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import rust_source_scan

    source = (
        'const TEXT: &str = r#"// TODO hidden; todo!()"#;\r\n'
        "// TODO real\r\n"
        "/* HACK outer /* FIXME nested */ tail */\r\n"
        'fn missing() { todo!("FIXME literal"); }\r\n'
    )
    original_scan = rust_source_scan._non_code_spans
    scans = 0

    def counted_scan(text: str):
        nonlocal scans
        scans += 1
        return original_scan(text)

    # Instrument the shared lexical authority, not the projection API: a
    # regression to two single projections must fail this consumer contract.
    monkeypatch.setattr(rust_source_scan, "_non_code_spans", counted_scan)
    hits = SA._debt_marker_hits(Path("runtime/sample.rs"), source)
    assert [(hit.line, hit.marker) for hit in hits] == [
        (2, "TODO"),
        (3, "FIXME"),
        (3, "HACK"),
        (4, "todo!"),
    ]
    assert scans == 1


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


def test_python_stub_surface_probe_counts_stubs_and_notimplemented(
    tmp_path: Path,
):
    stdlib = tmp_path / "src" / "molt" / "stdlib"
    stdlib.mkdir(parents=True)
    (stdlib / "gap.py").write_text(
        '"""Intrinsic-first stdlib module stub for `gap`."""\n'
        "def loads(payload):\n"
        "    raise NotImplementedError('real parser missing')\n"
        "def message():\n"
        "    return 'NotImplementedError in a string is not a raise'\n",
        encoding="utf-8",
    )

    findings = SA.probe_python_stub_surfaces(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert metrics["python_stub_surfaces_total"] == 2
    assert findings[0].location == "src/molt/stdlib/gap.py:1"
    assert findings[0].detail == (
        "L1:intrinsic-first stub, L3:raise NotImplementedError"
    )


def test_python_stub_surface_probe_ignores_detector_pattern_literals(
    tmp_path: Path,
):
    tools = tmp_path / "tools"
    tools.mkdir(parents=True)
    (tools / "structural_audit.py").write_text(
        "import re\n"
        "_INTRINSIC_FIRST_STUB_RE = re.compile(\n"
        '    r"not fully lowered yet; only an intrinsic-first stub is available|"\n'
        '    r"intrinsic-first (?:top-level )?stdlib .*stub|"\n'
        '    r"stub-only for now",\n'
        "    re.IGNORECASE,\n"
        ")\n",
        encoding="utf-8",
    )

    findings = SA.probe_python_stub_surfaces(tmp_path)

    assert findings == []


def test_rust_stub_surface_probe_counts_live_stubs_not_tests(tmp_path: Path):
    rust = tmp_path / "runtime" / "molt-backend-rust" / "src" / "rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        'fn emit(out: &mut String) { out.push_str("/* MOLT_STUB: op */"); }\n'
        'fn todo_live() { todo!("wire real lowering"); }\n',
        encoding="utf-8",
    )
    (rust / "checked_backend.rs").write_text(
        "#[cfg(test)]\n"
        "mod tests {\n"
        '    fn fixture() { assert!(!source.contains("MOLT_STUB")); }\n'
        "}\n",
        encoding="utf-8",
    )
    tests = tmp_path / "runtime" / "molt-backend-rust" / "tests"
    tests.mkdir(parents=True)
    (tests.parent / "Cargo.toml").write_text(
        '[package]\nname = "fixture"\nversion = "0.1.0"\n', encoding="utf-8"
    )
    (tests / "stub_fixture.rs").write_text(
        'fn fixture() { unimplemented!("fixture only"); }\n',
        encoding="utf-8",
    )
    runtime = tmp_path / "runtime" / "molt-runtime" / "src"
    runtime.mkdir(parents=True)
    (runtime / "memoryview.rs").write_text(
        "fn gap() {\n"
        "    raise_exception::<u64>(\n"
        "        _py,\n"
        '        "NotImplementedError",\n'
        '        "missing",\n'
        "    );\n"
        "}\n"
        "fn fallback_probe() {\n"
        '    if clear_pending_if_kind(_py, &["NotImplementedError"]) {\n'
        "        return;\n"
        "    }\n"
        "}\n"
        "fn exception_hierarchy(name: &str) -> bool {\n"
        '    matches!(name, "NotImplementedError")\n'
        "}\n",
        encoding="utf-8",
    )

    findings = SA.probe_rust_stub_surfaces(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert metrics["rust_stub_surfaces_total"] == 3
    assert {finding.location for finding in findings} == {
        "runtime/molt-backend-rust/src/rust/op_emitter.rs:1",
        "runtime/molt-runtime/src/memoryview.rs:4",
    }


def test_rust_backend_lowering_gap_probe_counts_unsupported_op_groups(
    tmp_path: Path,
):
    rust = tmp_path / "runtime" / "molt-backend-rust" / "src" / "rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '            "branch"\n'
        '            | "block_on"\n'
        '            | "bridge_unavailable" => {\n'
        "                self.emit_unsupported_op(\n"
        "                    op,\n"
        '                    format!("semantic op `{}` has no Rust backend lowering", op.kind),\n'
        "                );\n"
        "            }\n"
        "            other => {\n"
        '                self.emit_unsupported_op(op, format!("unsupported {other}"));\n'
        "            }\n",
        encoding="utf-8",
    )

    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    metrics = SA.ratchet_metrics(findings)

    assert metrics["rust_backend_lowering_gaps_total"] == 4
    assert findings[0].location == (
        "runtime/molt-backend-rust/src/rust/op_emitter.rs:4"
    )
    assert findings[0].detail == "L4:branch, block_on, bridge_unavailable"
    assert findings[1].detail == "L10:catch-all Rust backend op"


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


@pytest.mark.parametrize("parameters", ["<'a>", "<Value>", "<Value: Into<(u64, u64)>>"])
def test_enum_variant_extraction_handles_generic_headers(parameters: str):
    source = f"pub enum Domain{parameters} {{ Unit, Tuple(Value), Record {{ value: Value }} }}"
    assert SA._count_enum_variants(source, "Domain") == {"Unit", "Tuple", "Record"}
    assert SA._count_enum_variants(f'const TEXT: &str = "{source}";', "Domain") == set()


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
        f"pub fn production() {{}}\n#[cfg(test)]\nmod publication_race_tests {{\n{tests_body}\n}}\n",
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


def test_generated_marker_inside_string_literal_is_not_generated(tmp_path: Path):
    src = tmp_path / "runtime" / "molt-backend-rust" / "src" / "rust"
    src.mkdir(parents=True)
    path = src / "prelude.rs"
    path.write_text(
        "fn emit_header(output: &mut String) {\n"
        "    output.push_str(concat!(\n"
        '        "// Auto-generated - do not edit\\n",\n'
        '        "#![allow(dead_code)]\\n",\n'
        "    ));\n"
        "}\n",
        encoding="utf-8",
    )

    assert not SA._is_generated(path, tmp_path)


def test_duplicate_authority_probe_ignores_split_rust_test_modules(tmp_path: Path):
    passes = tmp_path / "runtime" / "molt-passes" / "src" / "tir" / "passes"
    tests = passes / "gvn"
    tests.mkdir(parents=True)
    (passes / "gvn.rs").write_text(
        '#[cfg(test)]\n#[path = "gvn/tests.rs"]\nmod tests;\n', encoding="utf-8"
    )
    (passes / "effects.rs").write_text(
        "fn opcode_is_side_effecting(opcode: OpCode) -> bool {\n"
        "    matches!(opcode, OpCode::Call)\n"
        "}\n",
        encoding="utf-8",
    )
    (tests / "tests.rs").write_text(
        "fn side_effecting_ops_preserved() {\n"
        "    assert!(matches!(opcode, OpCode::Call));\n"
        "}\n",
        encoding="utf-8",
    )

    assert SA.probe_duplicate_authorities(tmp_path) == []


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
        target.write_text(rust, encoding="utf-8")
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


def test_lowering_gap_pattern_does_not_absorb_completed_neighboring_arms(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"enumerate" => self.emit_enumerate(op),\n'
        '"zip" | "sorted" => self.emit_sorted(op),\n'
        '"module_import"\n'
        '| "module_import_from"\n'
        '| "module_import_star" => {\n'
        '    self.emit_unsupported_op(op, "requires import protocol");\n'
        "}\n",
        encoding="utf-8",
    )
    (rust / "synthetic_supported_helpers.rs").write_text(
        "fn emit_enumerate(&mut self, op: &OpIR) {}\nfn emit_sorted(&mut self, op: &OpIR) {}\n",
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1
    assert findings[0].metric == 3
    assert (
        findings[0].detail == "L6:module_import, module_import_from, module_import_star"
    )
    assert SA.ratchet_metrics(findings)["rust_backend_lowering_gaps_total"] == 3


def test_lowering_gap_ignores_comment_arrows_and_commented_calls(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '// self.emit_unsupported_op(op, "comment-only");\n'
        '"module_import"\n'
        '| "module_import_from" // => protocol note\n'
        '| "module_import_star" => {\n'
        '    self.emit_unsupported_op(op, "requires import protocol");\n'
        "}\n",
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1
    assert findings[0].metric == 3
    assert (
        findings[0].detail == "L5:module_import, module_import_from, module_import_star"
    )


@pytest.mark.parametrize(
    "literal", ['"https://x"', 'r##"// /* fake */"##', "' / '".replace(" ", "")]
)
def test_lowering_gap_preserves_call_after_literals(tmp_path, literal):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" => {\n'
        f'let note = {literal}; self.emit_unsupported_op(op, "required");\n'
        "}\n",
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1 and findings[0].metric == 1
    assert findings[0].detail == "L2:module_import"


def test_lowering_gap_ignores_nested_comments_and_literal_fake_calls(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        'let fake = r#"self.emit_unsupported_op(op, "fake");"#;\n'
        '/* outer /* nested */ self.emit_unsupported_op(op, "fake"); */\n'
        '"module_import"\n'
        '| "module_import_from" /* => nested /* => */ */\n'
        '| "module_import_star" => {\n'
        'self.emit_unsupported_op(op, "required");\n'
        "}\n",
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1 and findings[0].metric == 3
    assert (
        findings[0].detail == "L6:module_import, module_import_from, module_import_star"
    )


@pytest.mark.parametrize(
    "separator", ["\x0b", "\x0c", "\x1c", "\x85", "\u2028", "\u2029"]
)
def test_lowering_gap_paired_views_keep_rust_newline_coordinates(tmp_path, separator):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        f'let note = "a{separator}b{separator}c";\n'
        '"zip" => {\nzip_impl();\n}\n'
        '"module_import" | "module_import_from" | "module_import_star" => {\n'
        'self.emit_unsupported_op(op, "required");\n}\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1 and findings[0].metric == 3
    assert (
        findings[0].detail == "L6:module_import, module_import_from, module_import_star"
    )


def test_lowering_gap_multiline_pattern_survives_comment_only_lines(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"zip" => self.emit_zip(op),\n'
        '"module_import"\n// keep this family together\n'
        '| "module_import_from"\n/* nested /* => */ comment */\n'
        '| "module_import_star" => {\n'
        'self.emit_unsupported_op(op, "required");\n}\n',
        encoding="utf-8",
    )
    (rust / "synthetic_supported_helpers.rs").write_text(
        "fn emit_zip(&mut self, op: &OpIR) {}\n", encoding="utf-8"
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1 and findings[0].metric == 3
    assert (
        findings[0].detail == "L7:module_import, module_import_from, module_import_star"
    )


def test_lowering_gap_follows_rejection_only_sibling_and_forwarders(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    (rust / "op_emitter").mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" | "module_import_from" | "module_import_star" => self.emit_import(op),\n',
        encoding="utf-8",
    )
    (rust / "op_emitter/modules.rs").write_text(
        "fn emit_import(&mut self, op: &OpIR) { self.import_rejection(op); }\n"
        "fn import_rejection(&mut self, op: &OpIR) {\n"
        'self.emit_unsupported_op(op, "requires Python import protocol");\n}\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1 and findings[0].metric == 3
    assert (
        findings[0].detail == "L1:module_import, module_import_from, module_import_star"
    )


def test_lowering_gap_keeps_mixed_validation_guard_visible_without_global_claim(
    tmp_path,
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    (rust / "op_emitter").mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"unpack_sequence" => self.emit_unpack(op),\n', encoding="utf-8"
    )
    (rust / "op_emitter/gaps.rs").write_text(
        "fn emit_unpack(&mut self, op: &OpIR) {\n"
        'if bad_arity(op) { self.emit_unsupported_op(op, "invalid arity"); return; }\n'
        'self.emit_line("actual unpacking");\n}\n',
        encoding="utf-8",
    )
    (rust / "synthetic_supported_helpers.rs").write_text(
        "fn emit_line(&mut self, text: &str) {}\n", encoding="utf-8"
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1
    assert findings[0].probe == "rust_backend_rejection_applicability"
    assert "emit_unpack" in findings[0].detail and findings[0].metric == 0


def test_lowering_gap_does_not_accept_ambiguous_delegated_method_identity(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    (rust / "op_emitter").mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" => self.emit_import(op),\n', encoding="utf-8"
    )
    (rust / "op_emitter/first.rs").write_text(
        'fn emit_import(&mut self, op: &OpIR) { self.emit_unsupported_op(op, "missing"); }\n',
        encoding="utf-8",
    )
    (rust / "op_emitter/second.rs").write_text(
        'fn emit_import(&mut self, op: &OpIR) { self.emit_line("supported"); }\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert any(
        "Ambiguous" in f.title
        and f.metric == 0
        and f.probe == "rust_backend_rejection_applicability"
        for f in findings
    )
    metrics = SA.ratchet_metrics(findings)
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] > 0


def test_lowering_gap_literal_cfg_test_does_not_hide_real_rejection(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    (rust / "op_emitter").mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" => self.emit_import(op),\n', encoding="utf-8"
    )
    (rust / "op_emitter/modules.rs").write_text(
        'const NOTE: &str = r#"\n#[cfg(test)]\nmod tests {\n"#;\n'
        'fn emit_import(&mut self, op: &OpIR) { self.emit_unsupported_op(op, "missing"); }\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert any(f.metric == 1 and "module_import" in f.detail for f in findings)


@pytest.mark.parametrize("guard", ["let note = 1;", "if bad_arity(op) { return; }"])
def test_lowering_gap_transitive_mixed_helper_remains_applicability_debt(
    tmp_path, guard
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" => self.emit_import(op),\n', encoding="utf-8"
    )
    (rust / "modules.rs").write_text(
        f"fn emit_import(&mut self, op: &OpIR) {{ {guard} self.forward_import(op); }}\n"
        "fn forward_import(&mut self, op: &OpIR) { let note = 2; self.reject_import(op); }\n"
        'fn reject_import(&mut self, op: &OpIR) { self . emit_unsupported_op(op, "missing"); }\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert not any(f.metric for f in findings)
    obligations = [
        f for f in findings if f.probe == "rust_backend_rejection_applicability"
    ]
    assert len(obligations) == 2
    assert all(f.metric == 0 for f in obligations)
    assert any("emit_import:" in f.detail for f in obligations)
    assert any("forward_import:" in f.detail for f in obligations)


@pytest.mark.parametrize(
    "callee", ["self . emit_import(op)", "self\n    . emit_import(op)"]
)
def test_lowering_gap_shared_call_grammar_covers_spaced_multiline_dispatch(
    tmp_path, callee
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        f'"module_import" | "module_import_from" | "module_import_star" => {callee},\n',
        encoding="utf-8",
    )
    (rust / "modules.rs").write_text(
        'fn emit_import(&mut self, op: &OpIR) { self . emit_unsupported_op(op, "missing"); }\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1 and findings[0].metric == 3
    assert "module_import, module_import_from, module_import_star" in findings[0].detail


@pytest.mark.parametrize(
    "reference",
    [
        "Self::emit_unsupported_op",
        "Self\n :: emit_unsupported_op",
        "self . emit_unsupported_op",
    ],
)
def test_lowering_gap_known_non_call_rejection_reference_is_unresolved_debt(
    tmp_path, reference
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        f'"module_import" => {{ let reject = {reference}; reject(self, op, "missing"); }},\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert len(findings) == 1
    assert findings[0].probe == "rust_backend_rejection_applicability"
    assert findings[0].metric == 0
    assert "emit_unsupported_op" in findings[0].detail


@pytest.mark.parametrize(
    "signature",
    [
        "fn emit_import(&mut self, op: &OpIR)",
        "fn emit_import<'a>(&mut self, op: &'a OpIR)",
    ],
)
@pytest.mark.parametrize("site", ["rust/modules.rs", "rust.rs"])
def test_lowering_gap_generic_and_root_module_helper_inventory(
    tmp_path, signature, site
):
    source = tmp_path / "runtime/molt-backend-rust/src"
    (source / "rust").mkdir(parents=True)
    (source / "rust/op_emitter.rs").write_text(
        '"module_import" => self.emit_import(op),\n', encoding="utf-8"
    )
    (source / site).write_text(
        signature + ' { self.emit_unsupported_op(op, "missing"); }\n', encoding="utf-8"
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert SA.ratchet_metrics(findings)["rust_backend_lowering_gaps_total"] == 1


def test_lowering_gap_missing_or_macro_helper_is_fail_closed_proof_debt(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" => self.emit_import(op),\n', encoding="utf-8"
    )
    (rust / "modules.rs").write_text(
        'macro_rules! reject { ($n:ident) => { fn $n(&mut self, op: &OpIR) { self.emit_unsupported_op(op, "missing"); } }; }\nimpl RustBackend { reject!(emit_import); }\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert any("Unresolved" in f.title and "emit_import" in f.detail for f in findings)
    assert (
        SA.ratchet_metrics(findings)["rust_backend_rejection_applicability_total"] >= 1
    )


@pytest.mark.parametrize(
    "prefix", ["#[cfg(test)]\nmod tests;\n", "\x0c\n#[cfg(test)]\nfn helper() {}\n"]
)
def test_lowering_gap_cfg_test_declaration_cannot_hide_later_production(
    tmp_path, prefix
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" => self.emit_import(op),\n', encoding="utf-8"
    )
    (rust / "modules.rs").write_text(
        prefix
        + 'fn emit_import(&mut self, op: &OpIR) { self.emit_unsupported_op(op, "missing"); }\n',
        encoding="utf-8",
    )
    assert (
        SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))[
            "rust_backend_lowering_gaps_total"
        ]
        == 1
    )


def test_rust_stub_cfg_scope_cannot_be_spoofed_by_literal_or_out_of_line_module():
    text = 'const T: &str = "\n#[cfg(test)]\nmod tests {\n";\nfn live() { todo!("gap"); }\n'
    assert any(hit.marker == "todo!" for hit in SA._rust_stub_surface_hits(text))
    assert any(
        hit.marker == "todo!"
        for hit in SA._rust_stub_surface_hits(
            '#[cfg(test)]\nmod tests;\nfn live() { todo!("gap"); }\n'
        )
    )
    assert not SA._rust_stub_surface_hits(
        '#[cfg(test)]\nfn test_only() { todo!("fixture"); }\n'
    )


@pytest.mark.parametrize("prior_reject", [False, True])
@pytest.mark.parametrize(
    "callee", ["self . emit_import(op)", "self\n . emit_import(op)"]
)
def test_lowering_gap_multiline_anchor_uses_own_arm_and_preserves_dedup(
    tmp_path, prior_reject, callee
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    prior = 'self.emit_unsupported_op(op, "zip");' if prior_reject else "zip_impl();"
    (rust / "op_emitter.rs").write_text(
        f'"zip" => {{\n{prior}\n}}\n"module_import"\n| "module_import_from"\n| "module_import_star" => {callee},\n',
        encoding="utf-8",
    )
    (rust / "modules.rs").write_text(
        'fn emit_import(&mut self, op: &OpIR) { self.emit_unsupported_op(op, "missing"); }\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert (
        SA.ratchet_metrics(findings)["rust_backend_lowering_gaps_total"]
        == 3 + prior_reject
    )
    assert any(
        "module_import, module_import_from, module_import_star" in f.detail
        for f in findings
    )


def test_mixed_rejection_obligations_are_independently_fail_closed_ratchet():
    finding = SA.Finding(
        probe="rust_backend_rejection_applicability",
        severity="medium",
        title="unproven",
        location="fixture",
        detail="mixed",
        suggested_action="prove",
        class_retired="rust-backend-rejection-applicability",
        metric=0,
    )
    metrics = SA.ratchet_metrics([finding])
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] == 1
    assert (
        metrics["rust_backend_rejection_applicability_total"]
        > SA.ratchet_metrics([])["rust_backend_rejection_applicability_total"]
    )


@pytest.mark.parametrize(
    "source",
    [
        "impl A { fn width(&self) -> usize { self.width } }\nimpl B { fn width(&self) -> usize { self.width } }\n",
        "impl A { fn new() -> Self { A } }\nimpl B { fn new() -> Self { B } fn make() -> Self { Self::new() } }\n",
    ],
)
def test_lowering_gap_unrelated_field_and_associated_references_are_not_rejection_debt(
    tmp_path, source
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text("", encoding="utf-8")
    (rust / "types.rs").write_text(source, encoding="utf-8")
    metrics = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] == 0


def test_lowering_gap_associated_call_is_proof_debt_never_definite(tmp_path):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        '"module_import" => Self::emit_import(self, op),\n', encoding="utf-8"
    )
    (rust / "modules.rs").write_text(
        'fn emit_import(&mut self, op: &OpIR) { Self::emit_unsupported_op(self, op, "missing"); }\n',
        encoding="utf-8",
    )
    metrics = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] >= 1


@pytest.mark.parametrize(
    "callee", ["self\u200e.\u200femit_import(op)", "self.r#emit_import(op)"]
)
def test_lowering_gap_rust_whitespace_and_raw_identifiers_share_call_grammar(
    tmp_path, callee
):
    rust = tmp_path / "runtime/molt-backend-rust/src/rust"
    rust.mkdir(parents=True)
    (rust / "op_emitter.rs").write_text(
        f'"zip" => {{ zip_impl(); }}\n"module_import" | "module_import_star" => {callee},\n',
        encoding="utf-8",
    )
    (rust / "modules.rs").write_text(
        'fn r#emit_import(&mut self, op: &OpIR) { self.r#emit_unsupported_op(op, "missing"); }\n',
        encoding="utf-8",
    )
    assert (
        SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))[
            "rust_backend_lowering_gaps_total"
        ]
        == 2
    )


@pytest.mark.parametrize(
    "test_member", ["hits: u32,", "Variant(u32),", "hits: (u32, u32),"]
)
def test_rust_cfg_test_member_comma_does_not_hide_next_production_impl(test_member):
    source = f'struct S {{\n#[cfg(test)]\n{test_member}\n}}\nimpl S {{ fn live(&self) {{ todo!("gap"); }} }}\n'
    assert any(hit.marker == "todo!" for hit in SA._rust_stub_surface_hits(source))


def test_rust_cfg_multiline_test_function_parameters_remain_excluded():
    source = '#[cfg(test)]\nfn test_only(a: u32,\n b: u32) { todo!("test fixture"); }\nfn live() { todo!("gap"); }\n'
    assert [(h.line, h.marker) for h in SA._rust_stub_surface_hits(source)] == [
        (4, "todo!")
    ]


@pytest.mark.parametrize(
    "variant", ["renamed", "relocated", "free", "direct", "missing"]
)
def test_rejection_recording_mechanism_survives_refactoring(tmp_path, variant):
    crate = tmp_path / "runtime/molt-backend-rust"
    family = crate / "src/rust"
    family.mkdir(parents=True)
    (crate / "Cargo.toml").write_text("[package]\nname='fixture'\n", encoding="utf-8")
    (crate / "src/rust.rs").write_text(
        'struct RustBackend {\nunsupported_ops: Vec<String>,\n}\nfn emit_source(&mut self, op: &OpIR) { self.unsupported_ops.clear(); self.emit(op); String::new() }\nfn compile_checked(&mut self, op: &OpIR) { let source = self.emit_source(op); if !self.unsupported_ops.is_empty() { return Err(format!("unsupported: {:?}", self.unsupported_ops)); } Ok(source) }',
        encoding="utf-8",
    )
    name = "record_refusal" if variant == "renamed" else "emit_unsupported_op"
    recorder = f'fn {name}(&mut self, op: &OpIR, reason: impl Into<String>) {{ let reason = reason.into(); self.unsupported_ops.push(format!("rejected")); }}\n'
    invocation = f'self.{name}(op, "missing")'
    if variant == "free":
        invocation = f'{name}(self, op, "missing")'
    elif variant == "direct":
        invocation = 'self.unsupported_ops.push(format!("rejected"))'
    path = family / "op_emitter.rs"
    if variant == "relocated":
        path = family / "op_emitter/mod.rs"
        path.parent.mkdir()
    if variant != "missing":
        path.write_text(
            recorder
            + f'fn emit(&mut self, op: &OpIR) {{ match op.kind {{ "module_import" => {{ {invocation}; }} }} }}',
            encoding="utf-8",
        )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    metrics = SA.ratchet_metrics(findings)
    if variant in {"renamed", "relocated"}:
        assert metrics["rust_backend_lowering_gaps_total"] == 1
    else:
        assert metrics["rust_backend_lowering_gaps_total"] == 0
        assert metrics["rust_backend_rejection_applicability_total"] > 0
        if variant in {"free", "direct"}:
            assert any("emit:" in finding.detail for finding in findings)
        else:
            assert any(
                "dispatch source authority" in finding.title for finding in findings
            )


@pytest.mark.parametrize(
    "relative",
    ["runtime/molt-runtime/src/abi.rs", "runtime/molt-backend-rust/src/rust/abi.rs"],
)
def test_cfg_test_member_cannot_hide_production_abi_stub_sibling(tmp_path, relative):
    path = tmp_path / relative
    path.parent.mkdir(parents=True)
    path.write_text(
        'struct State {\n#[cfg(test)]\nhits: u32,\n}\nimpl State { fn export(&self) { todo!("production ABI gap"); } }\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_stub_surfaces(tmp_path)
    assert len(findings) == 1
    assert findings[0].metric == 1
    assert findings[0].location == f"{relative}:5"


@pytest.mark.parametrize(
    "refactor", ["rename", "relocate", "raw_reference", "rust_whitespace"]
)
def test_actual_rejection_protocol_refactor_preserves_debt(tmp_path, refactor):
    import shutil

    relative = Path("runtime/molt-backend-rust")
    original = ROOT / relative
    destination = tmp_path / relative
    shutil.copytree(original / "src/rust", destination / "src/rust")
    shutil.copyfile(original / "src/rust.rs", destination / "src/rust.rs")
    shutil.copyfile(original / "Cargo.toml", destination / "Cargo.toml")
    before = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert before["rust_backend_lowering_gaps_total"] > 0
    if refactor == "rename":
        for path in (destination / "src").rglob("*.rs"):
            source = path.read_text(encoding="utf-8")
            path.write_text(
                source.replace("emit_unsupported_op", "record_refusal"),
                encoding="utf-8",
            )
    elif refactor in {"raw_reference", "rust_whitespace"}:
        for path in (destination / "src").rglob("*.rs"):
            source = path.read_text(encoding="utf-8")
            if refactor == "raw_reference":
                source = source.replace(
                    ".emit_unsupported_op(", ".r#emit_unsupported_op("
                )
            else:
                source = source.replace(
                    ".emit_unsupported_op(", ".\u200e emit_unsupported_op("
                )
            path.write_text(source, encoding="utf-8")
    else:
        old = destination / "src/rust/op_emitter.rs"
        new = destination / "src/rust/op_emitter/mod.rs"
        new.parent.mkdir(exist_ok=True)
        old.rename(new)
    after = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert (
        after["rust_backend_lowering_gaps_total"]
        == before["rust_backend_lowering_gaps_total"]
    )
    assert (
        after["rust_backend_rejection_applicability_total"]
        == before["rust_backend_rejection_applicability_total"]
    )


@pytest.mark.parametrize(
    "body",
    [
        'self.unsupported_ops.push(format!("missing")); self.unsupported_ops.clear();',
        "self.unsupported_ops.extend(Vec::<String>::new());",
        'let deferred = || self.unsupported_ops.push(format!("missing"));',
    ],
)
def test_recording_mutation_is_not_definite_refusal_without_terminal_push(
    tmp_path, body
):
    import shutil

    relative = Path("runtime/molt-backend-rust")
    original = ROOT / relative
    destination = tmp_path / relative
    shutil.copytree(original / "src/rust", destination / "src/rust")
    shutil.copyfile(original / "src/rust.rs", destination / "src/rust.rs")
    shutil.copyfile(original / "Cargo.toml", destination / "Cargo.toml")
    path = destination / "src/rust/op_emitter.rs"
    source = path.read_text(encoding="utf-8")
    code = SA.mask_rust_comments_and_strings(source)
    method = code.index("pub(super) fn emit_unsupported_op")
    opening = code.index("{", method)
    end, _ = SA._balanced_block(code, opening)
    path.write_text(source[: opening + 1] + body + source[end - 1 :], encoding="utf-8")
    metrics = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] > 0


@pytest.mark.parametrize(
    "recovery", ["dispatcher_clear", "consumer_clear", "missing_err", "aliased_clear"]
)
def test_actual_rejection_protocol_recovery_requires_proof_not_definite_gap(
    tmp_path, recovery
):
    import shutil

    relative = Path("runtime/molt-backend-rust")
    original = ROOT / relative
    destination = tmp_path / relative
    shutil.copytree(original / "src/rust", destination / "src/rust")
    shutil.copyfile(original / "src/rust.rs", destination / "src/rust.rs")
    shutil.copyfile(original / "Cargo.toml", destination / "Cargo.toml")
    if recovery in {"dispatcher_clear", "aliased_clear"}:
        path = destination / "src/rust/op_emitter.rs"
        source = path.read_text(encoding="utf-8").replace(
            "pub(super) fn emit_op(&mut self, op: &OpIR) {",
            (
                "pub(super) fn emit_op(&mut self, op: &OpIR) { self.unsupported_ops.clear();"
                if recovery == "dispatcher_clear"
                else "pub(super) fn emit_op(&mut self, op: &OpIR) { { let alias = &mut *self; alias.unsupported_ops.clear(); }"
            ),
        )
    else:
        path = destination / "src/rust.rs"
        source = path.read_text(encoding="utf-8")
        if recovery == "consumer_clear":
            source = source.replace(
                "if !self.unsupported_ops.is_empty() {",
                "self.unsupported_ops.clear(); if !self.unsupported_ops.is_empty() {",
            )
        else:
            source = source.replace(
                "return Err(format!(",
                "let _ignored: Result<String, String> = Err(format!(",
            )
    path.write_text(source, encoding="utf-8")
    metrics = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] > 0


@pytest.mark.parametrize("retain_recorder", [True, False])
def test_healthy_uncalled_refusal_protocol_has_no_permanent_debt_floor(
    tmp_path, retain_recorder
):
    crate = tmp_path / "runtime/molt-backend-rust"
    family = crate / "src/rust"
    family.mkdir(parents=True)
    (crate / "Cargo.toml").write_text("[package]\nname='fixture'\n", encoding="utf-8")
    (crate / "src/rust.rs").write_text(
        "struct RustBackend {\nunsupported_ops: Vec<String>,\n}\nfn emit_source(&mut self) { self.unsupported_ops.clear(); String::new() }\n"
        'fn compile_checked(&mut self) { let source = self.emit_source(); if !self.unsupported_ops.is_empty() { return Err(format!("unsupported: {:?}", self.unsupported_ops)); } Ok(source) }\n',
        encoding="utf-8",
    )
    (family / "op_emitter.rs").write_text(
        (
            'fn record(&mut self) { self.unsupported_ops.push(format!("missing")); }\n'
            if retain_recorder
            else ""
        )
        + 'fn emit_line(&mut self) { self.output.push("x"); }',
        encoding="utf-8",
    )
    metrics = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] == 0


@pytest.mark.parametrize("bad_value", [float("nan"), float("inf"), -1, True, "0"])
def test_cli_ratchet_rejects_invalid_baseline_numbers(tmp_path, capsys, bad_value):
    path = tmp_path / "tools/structural_audit_baseline.json"
    path.parent.mkdir()
    metrics = SA.ratchet_metrics([])
    metrics["rust_backend_rejection_applicability_total"] = bad_value
    path.write_text(json.dumps(metrics), encoding="utf-8")
    assert SA.main(["--root", str(tmp_path), "--check"]) == 2
    import math

    expected = (
        "non-finite JSON number"
        if isinstance(bad_value, float) and not math.isfinite(bad_value)
        else "finite non-negative numbers"
    )
    assert expected in capsys.readouterr().err


@pytest.mark.parametrize("json_output", [False, True])
@pytest.mark.parametrize("regression", [False, True])
def test_cli_check_verdict_is_independent_of_output_format(
    tmp_path, capsys, json_output, regression
):
    baseline = tmp_path / "tools/structural_audit_baseline.json"
    baseline.parent.mkdir()
    baseline.write_text(json.dumps(SA.ratchet_metrics([])), encoding="utf-8")
    source = tmp_path / "src/molt/stdlib/fixture.py"
    source.parent.mkdir(parents=True)
    source.write_text(
        "def operation():\n    raise NotImplementedError\n"
        if regression
        else "def operation():\n    return 1\n",
        encoding="utf-8",
    )
    args = ["--root", str(tmp_path), "--check"]
    if json_output:
        args.append("--json")
    assert SA.main(args) == int(regression)
    output = capsys.readouterr()
    if json_output:
        assert json.loads(output.out)["metrics"]["python_stub_surfaces_total"] == int(
            regression
        )


@pytest.mark.parametrize(
    "call", ["self.emit_unsupported_op(op)", "emit_unsupported_op(self, op)"]
)
def test_retired_primitive_with_remaining_calls_is_unresolved_proof_debt(
    tmp_path, call
):
    crate = tmp_path / "runtime/molt-backend-rust"
    family = crate / "src/rust"
    family.mkdir(parents=True)
    (crate / "Cargo.toml").write_text("[package]\nname='fixture'\n", encoding="utf-8")
    (crate / "src/rust.rs").write_text(
        f"fn emit_source(&mut self) {{ self.unsupported_ops.clear(); {call}; String::new() }}\n"
        'fn compile_checked(&mut self) { let source = self.emit_source(); if !self.unsupported_ops.is_empty() { return Err(format!("unsupported: {:?}", self.unsupported_ops)); } Ok(source) }\n',
        encoding="utf-8",
    )
    (family / "op_emitter.rs").write_text("", encoding="utf-8")
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert (
        SA.ratchet_metrics(findings)["rust_backend_rejection_applicability_total"] > 0
    )
    assert any("emit_unsupported_op" in finding.detail for finding in findings)


@pytest.mark.parametrize(
    "reason", ["missing", "module_import_star", "other", "string=>arrow"]
)
def test_lowering_gap_body_literals_do_not_count_as_pattern_ops(tmp_path, reason):
    path = tmp_path / "runtime/molt-backend-rust/src/rust/op_emitter.rs"
    path.parent.mkdir(parents=True)
    path.write_text(
        f'"module_import" => self.emit_unsupported_op(op, "{reason}"),\n',
        encoding="utf-8",
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert SA.ratchet_metrics(findings)["rust_backend_lowering_gaps_total"] == 1
    assert findings[0].detail == "L1:module_import"


@pytest.mark.parametrize(
    "source",
    [
        '#[cfg(test)] let t = (); todo!("production");',
        '#[cfg(test)] fn test_only() {} fn live() { todo!("production"); }',
        '#[cfg(test)]\nfn test_only() {\n} fn live() { todo!("production"); }',
    ],
)
def test_cfg_test_partial_line_cannot_hide_following_production(source):
    assert any(hit.marker == "todo!" for hit in SA._rust_stub_surface_hits(source))


@pytest.mark.parametrize(
    "escape",
    [
        "recursive_entry",
        "raw_field",
        "destructure",
        "take_self",
        "assign_self",
        "alias_take",
        "foreign_backend",
        "unknown_signature",
        "unchecked_compile",
        "pre_emission_self",
        "late_entry_reset",
        "reset_after_emit",
        "early_return",
        "bad_ok_tail",
        "async_recorder",
        "unknown_macro",
        "unicode_macro",
        "recursive_checked",
        "public_lowerer",
        "public_extern_lowerer",
        "public_unknown_signature",
        "public_ambiguous_lowerer",
        "unknown_absolute_take",
        "paren_assign_default",
        "typed_alias_default",
        "imported_take",
        "qualified_default_reference",
        "foreign_qualified",
        "unknown_reset_without_field",
        "ambiguous_reset",
    ],
)
def test_actual_protocol_escape_is_explicit_unmet_evidence(tmp_path, escape):
    import shutil

    relative = Path("runtime/molt-backend-rust")
    original = ROOT / relative
    destination = tmp_path / relative
    shutil.copytree(original / "src/rust", destination / "src/rust")
    shutil.copyfile(original / "src/rust.rs", destination / "src/rust.rs")
    shutil.copyfile(original / "Cargo.toml", destination / "Cargo.toml")
    emitter = destination / "src/rust/op_emitter.rs"
    root = destination / "src/rust.rs"
    source = emitter.read_text(encoding="utf-8")
    entry = "pub(super) fn emit_op(&mut self, op: &OpIR) {"
    injected = {
        "recursive_checked": "let _ = self.compile_checked(&crate::SimpleIR { functions: Vec::new(), profile: None });",
        "paren_assign_default": "*(self) = Default::default();",
        "typed_alias_default": "let this: &mut Self = self; *this = Default::default();",
        "imported_take": "use std::mem::take; let _old = take(self);",
        "qualified_default_reference": "let constructor = <Self as Default>::default; *self = constructor();",
        "unknown_reset_without_field": "Self::reset_all(self, [0]);",
        "unknown_absolute_take": "Self::reset_all(self, [0]);",
        "ambiguous_reset": "*self = Self::new();",
        "recursive_entry": "self.emit_source(&crate::SimpleIR { functions: Vec::new(), profile: None }, None);",
        "raw_field": "self.r#unsupported_ops.clear();",
        "destructure": "{ let Self { unsupported_ops, .. } = &mut *self; unsupported_ops.clear(); }",
        "take_self": "let _old = std::mem::take(self);",
        "assign_self": "*self = Self::new();",
        "alias_take": "let alias = &mut *self; let _old = std::mem::take(alias);",
        "unknown_signature": "Self::reset_refusals(self, [0]);",
    }
    if escape in injected:
        source = source.replace(entry, entry + injected[escape])
        if escape == "unknown_signature":
            source += "\nimpl RustBackend { fn reset_refusals(&mut self, _s: [u8; 1]) { self.unsupported_ops.clear(); } }\n"
        elif escape == "unknown_reset_without_field":
            source += "\nimpl RustBackend { fn reset_all(&mut self, _s: [u8; 1]) { *self = Self::new(); } }\n"
        elif escape == "unknown_absolute_take":
            source += "\nimpl RustBackend { fn reset_all(&mut self, _s: [u8; 1]) { let _old = ::std::mem::take(self); } }\n"
        elif escape == "ambiguous_reset":
            source += "\nstruct Shadow; impl Shadow { fn emit_op(&self) {} }\n"
        emitter.write_text(source, encoding="utf-8")
    elif escape == "async_recorder":
        emitter.write_text(
            source.replace(
                "pub(super) fn emit_unsupported_op",
                "pub(super) async fn emit_unsupported_op",
            ),
            encoding="utf-8",
        )
    elif escape in {"unknown_macro", "unicode_macro"}:
        invocation = "bail!()" if escape == "unknown_macro" else "bail\u200e!()"
        source = source.replace(
            '.push(format!("`{}` (rust backend): {reason}", op.kind));',
            f".push({invocation});",
        )
        assert f".push({invocation});" in source
        emitter.write_text(
            "macro_rules! bail { () => { return }; }\n" + source, encoding="utf-8"
        )
    else:
        source = root.read_text(encoding="utf-8")
        if escape in {
            "public_lowerer",
            "public_extern_lowerer",
            "public_unknown_signature",
            "public_ambiguous_lowerer",
        }:
            visibility = (
                'pub extern "C"' if escape == "public_extern_lowerer" else "pub"
            )
            padding = ", _pad: [u8; 1]" if escape == "public_unknown_signature" else ""
            source += f"\nimpl RustBackend {{ {visibility} fn compile_function(&mut self, func: &FunctionIR{padding}) -> String {{ self.emit_function(func, None); std::mem::take(&mut self.output) }} }}\n"
            if escape == "public_ambiguous_lowerer":
                source += (
                    "\nstruct Shadow; impl Shadow { fn compile_function(&self) {} }\n"
                )
        elif escape == "foreign_qualified":
            source = source.replace(
                "self.emit_op(&ops[i]);",
                "{ let mut trial: Self = Default::default(); Self::emit_op(&mut trial, &ops[i]); self.output.push_str(&trial.output); }",
            )
        elif escape == "foreign_backend":
            source = source.replace(
                "self.emit_op(&ops[i]);",
                "{ let mut trial = RustBackend::new(); trial.emit_op(&ops[i]); self.output.push_str(&trial.output); }",
            )
        elif escape == "unchecked_compile":
            source = source.replace("#[cfg(test)]\n    fn compile(", "fn compile(")
        elif escape == "pre_emission_self":
            source = source.replace(
                "let source = self.emit_source",
                "self.emit_op(&ir.functions[0].ops[0]); let source = self.emit_source",
            )
        elif escape == "late_entry_reset":
            source = source.replace(
                "// Entry point", "self.unsupported_ops.clear(); // Entry point"
            )
        elif escape == "reset_after_emit":
            source = source.replace("self.unsupported_ops.clear();", "", 1).replace(
                "// Entry point", "self.unsupported_ops.clear(); // Entry point"
            )
        elif escape == "early_return":
            source = source.replace(
                "let source = self.emit_source",
                "return Ok(String::new()); let source = self.emit_source",
            )
        elif escape == "bad_ok_tail":
            source = source.replace("Ok(source)", "Ok(String::new())")
        root.write_text(source, encoding="utf-8")
    metrics = SA.ratchet_metrics(SA.probe_rust_backend_lowering_gaps(tmp_path))
    assert metrics["rust_backend_lowering_gaps_total"] == 0
    assert metrics["rust_backend_rejection_applicability_total"] > 0


def test_refusal_field_inference_excludes_unrelated_empty_checks():
    source = 'if !self.output.is_empty() { let present = 1; } if !self.unsupported_ops.is_empty() { return Err(format!("unsupported: {:?}", self.unsupported_ops)); }'
    assert [match[1] for match in SA._rust_checked_refusal_guards(source)] == [
        "unsupported_ops"
    ]


@pytest.mark.parametrize("corruption", ["duplicate", "extra", "missing"])
def test_cli_baseline_json_integrity_matches_receipt_authority(
    tmp_path, capsys, corruption
):
    path = tmp_path / "tools/structural_audit_baseline.json"
    path.parent.mkdir()
    baseline = SA.ratchet_metrics([])
    if corruption == "duplicate":
        text = json.dumps(baseline)
        text = text[:-1] + ', "rust_backend_lowering_gaps_total": 999}'
    elif corruption == "extra":
        baseline["unrecognized_metric"] = 0
        text = json.dumps(baseline)
    else:
        del baseline["rust_backend_lowering_gaps_total"]
        text = json.dumps(baseline)
    path.write_text(text, encoding="utf-8")
    assert SA.main(["--root", str(tmp_path), "--check"]) == 2
    error = capsys.readouterr().err
    assert "invalid baseline" in error or "metric keys differ" in error


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))


def test_guarded_wildcard_does_not_hide_fail_closed_default():
    source = """fn classify(op: &TirOp) {
        match op.opcode {
            OpCode::A | OpCode::B | OpCode::C => Ok(None),
            _ if bookkeeping(op) => Ok(None),
            _ => Err(()),
        }
    }
    """
    assert _scan_rust_string(source, "tir/passes/effects.rs") == []
    assert (
        len(
            _scan_rust_string(
                source.replace("_ => Err(())", "_ => None"), "tir/passes/effects.rs"
            )
        )
        == 1
    )


def test_rust_regions_use_lexical_offsets_and_exact_test_scope():
    source = 'const BAIT: &str = r#"{\nfn fake() {}\n"#;\n'
    source += "#[cfg(test)]\nmod spec { fn oracle() {} }\n"
    source += 'fn live() { let s = "}"; }\n'
    source += "struct Stats {\n#[cfg(test)] scanned: Map<u32, u32>,\nlive: usize,\n}\n"
    source += "fn sibling() {}\n"
    regions = SA._rust_top_level_regions(source)
    assert [
        (region.name, region.start_line, region.end_line) for region in regions
    ] == [("live", 6, 6), ("Stats", 7, 10), ("sibling", 11, 11)]
    masked = SA.mask_rust_test_items(source)
    assert "scanned" not in masked
    assert "live: usize" in masked


def test_declared_external_oracle_is_not_production_debt(tmp_path):
    source = tmp_path / "runtime/sample/src"
    source.mkdir(parents=True)
    (source / "main.rs").write_text(
        '#[cfg(test)] #[path = "specifications.rs"] mod assertions;\n', encoding="utf-8"
    )
    (source / "specifications.rs").write_text(
        "fn oracle(opcode: OpCode) -> bool {\n"
        "matches!(opcode, OpCode::A | OpCode::B | OpCode::C)\n"
        '}\nfn fixture() { panic!("MOLT_STUB"); }\n',
        encoding="utf-8",
    )
    assert SA.probe_semantic_fallthroughs(tmp_path) == []
    assert SA.probe_rust_stub_surfaces(tmp_path) == []
    (source / "main.rs").write_text("mod specifications;\n", encoding="utf-8")
    assert len(SA.probe_semantic_fallthroughs(tmp_path)) == 1
    assert len(SA.probe_rust_stub_surfaces(tmp_path)) == 1


def test_shared_rust_admission_proof_requires_dominating_validation(tmp_path):
    import shutil
    from tools.structural_audit_rust_admission import _body, proven_rejected_kinds

    source = (ROOT / "runtime/molt-backend-rust/src/rust.rs").read_text(
        encoding="utf-8"
    )
    consumer = _body(source, "compile_checked")
    assert consumer is not None
    denied = proven_rejected_kinds(ROOT, consumer)
    assert denied is not None
    assert {"class_new", "module_import", "const_bytes", "int"} <= denied
    assert "const_str" not in denied
    assert (
        proven_rejected_kinds(
            ROOT, consumer.replace("let admitted =", "let bypassed =", 1)
        )
        is None
    )
    for relative in (
        "runtime/molt-tir/src/target_admission.rs",
        "runtime/molt-tir/src/target_admission/runtime.rs",
        "runtime/molt-tir/src/target_admission/numeric.rs",
        "runtime/molt-ir/src/tir/target_info.rs",
        "runtime/molt-ir/src/ir.rs",
        "runtime/molt-ir/src/tir/op_kinds_generated.rs",
        "runtime/molt-ir/src/tir/op_kinds.toml",
        "src/molt/frontend/lowering/op_kinds_generated.py",
        "runtime/molt-backend-rust/src/rust.rs",
    ):
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / relative, destination)
    shutil.copytree(
        ROOT / "runtime/molt-backend-rust/src/rust",
        tmp_path / "runtime/molt-backend-rust/src/rust",
    )
    assert proven_rejected_kinds(tmp_path, consumer) == denied
    runtime = tmp_path / "runtime/molt-tir/src/target_admission/runtime.rs"
    runtime.write_text(
        runtime.read_text(encoding="utf-8").replace(
            "requirements.difference(supported_requirements)",
            "supported_requirements",
            1,
        ),
        encoding="utf-8",
    )
    assert proven_rejected_kinds(tmp_path, consumer) is None


def test_dispatch_admission_projection_preserves_unknown_and_guarded_arms():
    from tools.structural_audit_rust_admission import literal_dispatch_arm_ranges

    source = 'match op.kind.as_str() { "denied" | "other" => self.refuse(op), "conditional" if allowed(op) => self.refuse(op), _ => self.refuse(op), }'
    arms = literal_dispatch_arm_ranges(source)
    assert [kinds for _, _, kinds in arms] == [frozenset({"denied", "other"})]


def _copy_live_rust_admission_sources(destination):
    """Copy the actual source authority, never a handwritten support fixture."""
    import shutil

    for relative in (
        "runtime/molt-tir/src/target_admission.rs",
        "runtime/molt-tir/src/target_admission/runtime.rs",
        "runtime/molt-tir/src/target_admission/numeric.rs",
        "runtime/molt-ir/src/tir/target_info.rs",
        "runtime/molt-ir/src/ir.rs",
        "runtime/molt-ir/src/ir_schema.rs",
        "runtime/molt-ir/src/literal_payload.rs",
        "runtime/molt-ir/src/tir/simple_def_use.rs",
        "runtime/molt-ir/src/tir/op_kinds_generated.rs",
        "runtime/molt-ir/src/tir/op_kinds.toml",
        "src/molt/frontend/lowering/op_kinds_generated.py",
        "runtime/molt-backend-rust/Cargo.toml",
        "runtime/molt-backend-rust/src/rust.rs",
    ):
        path = destination / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / relative, path)
    shutil.copytree(
        ROOT / "runtime/molt-backend-rust/src/rust",
        destination / "runtime/molt-backend-rust/src/rust",
    )


def test_live_rust_admitted_domains_close_refusal_paths(tmp_path):
    from tools.structural_audit_rust_admission import _body, proven_admitted_wire_domain

    _copy_live_rust_admission_sources(tmp_path)
    source = (tmp_path / "runtime/molt-backend-rust/src/rust.rs").read_text(
        encoding="utf-8"
    )
    domain = proven_admitted_wire_domain(tmp_path, _body(source, "compile_checked"))
    assert domain is not None
    assert {"const_int", "load_const", "const_bigint"} <= domain.denied
    assert {"inplace_floordiv", "inplace_mod", "mod_", "binop_pow"} <= domain.denied
    assert "store_local" not in domain.registered
    assert "const_float" in domain.possible
    assert SA.probe_rust_backend_lowering_gaps(tmp_path) == []


def test_live_rust_operand_projection_uses_registered_current_validators():
    from tools.structural_audit_rust_domains import _operand_shapes

    schema = (ROOT / "runtime/molt-ir/src/ir_schema.rs").read_text(encoding="utf-8")
    generated = (ROOT / "runtime/molt-ir/src/tir/op_kinds_generated.rs").read_text(
        encoding="utf-8"
    )
    shapes = _operand_shapes(schema, generated)
    assert shapes is not None
    # Independent wire expectations: these emitters consume exactly one value;
    # an invented wire spelling must not acquire a shape from the recognizer.
    assert shapes["warn_stderr"] == 1
    assert shapes["type_guard"] == 1
    assert "__unregistered_wire" not in shapes


@pytest.mark.parametrize(
    ("owner", "before", "after"),
    [
        (
            "validate_registered_op_kind",
            "if !simpleir_kind_is_registered(kind)",
            "if simpleir_kind_is_registered(kind)",
        ),
        (
            "validate_registered_op_kind",
            "validate_op_not_retired(kind)?;",
            "return Ok(());",
        ),
        (
            "validate_simple_op_shape",
            "validate_registered_op_kind(&op.kind)?;",
            "let _ = validate_registered_op_kind(&op.kind);",
        ),
        (
            "validate_simple_op_shape",
            "validate_registered_op_kind(&op.kind)?;",
            'validate_registered_op_kind("warn_stderr")?;',
        ),
        (
            "validate_simple_op_shape",
            "op.args.as_ref().map(Vec::len)",
            "Some(1)",
        ),
        (
            "validate_simple_op_shape",
            "op.var.is_some()",
            "op.var.is_none()",
        ),
        (
            "validate_op_shape",
            "operands.unwrap_or(0) != shape.operands",
            "operands.unwrap_or(1) != shape.operands",
        ),
        (
            "validate_op_shape",
            "kind: shape.kind.into(),",
            'kind: "different-operation".into(),',
        ),
        (
            "validate_simple_op_shape",
            "kind: shape.kind.into(),",
            'kind: "different-operation".into(),',
        ),
    ],
)
def test_rust_operand_projection_rejects_changed_admission_or_operands(
    owner, before, after
):
    from tools.structural_audit_rust_admission import _body
    from tools.structural_audit_rust_domains import _operand_shapes

    schema = (ROOT / "runtime/molt-ir/src/ir_schema.rs").read_text(encoding="utf-8")
    generated = (ROOT / "runtime/molt-ir/src/tir/op_kinds_generated.rs").read_text(
        encoding="utf-8"
    )
    assert _operand_shapes(schema, generated) is not None
    body = _body(schema, owner)
    assert body is not None and schema.count(body) == 1 and body.count(before) == 1
    changed = schema.replace(body, body.replace(before, after, 1), 1)
    assert _operand_shapes(changed, generated) is None


def test_rust_operand_projection_requires_generated_registration_membership():
    from tools.structural_audit_rust_admission import _body
    from tools.structural_audit_rust_domains import _operand_shapes

    schema = (ROOT / "runtime/molt-ir/src/ir_schema.rs").read_text(encoding="utf-8")
    generated = (ROOT / "runtime/molt-ir/src/tir/op_kinds_generated.rs").read_text(
        encoding="utf-8"
    )
    assert _operand_shapes(schema, generated) is not None
    body = _body(generated, "simpleir_kind_is_registered")
    assert body is not None and generated.count(body) == 1
    assert body.count('"warn_stderr"') == 1
    for changed_body in (
        body.replace('"warn_stderr"', '"__unregistered_wire"'),
        "true",
    ):
        changed = generated.replace(body, changed_body, 1)
        assert _operand_shapes(schema, changed) is None


@pytest.mark.parametrize(
    ("capability", "kinds"),
    [
        ("cpython_float_divmod", {"inplace_floordiv", "inplace_mod", "mod_"}),
        ("cpython_power", {"pow", "binop_pow"}),
    ],
)
def test_rust_admitted_domain_numeric_total_rejection_requires_each_branch(
    tmp_path, capability, kinds
):
    from tools.structural_audit_rust_admission import _body, proven_admitted_wire_domain

    _copy_live_rust_admission_sources(tmp_path)
    consumer = _body(
        (tmp_path / "runtime/molt-backend-rust/src/rust.rs").read_text(
            encoding="utf-8"
        ),
        "compile_checked",
    )
    baseline = proven_admitted_wire_domain(tmp_path, consumer)
    assert baseline is not None and kinds <= baseline.denied
    numeric = tmp_path / "runtime/molt-tir/src/target_admission/numeric.rs"
    source = numeric.read_text(encoding="utf-8")
    before = f"(!capabilities.{capability}).then_some("
    assert source.count(before) == 1
    numeric.write_text(
        source.replace(before, f"(capabilities.{capability}).then_some(", 1),
        encoding="utf-8",
    )
    # The changed float branch now succeeds with the same false capability.
    # Neither role nor wire spelling may retain its unconditional exclusion.
    changed = proven_admitted_wire_domain(tmp_path, consumer)
    assert changed is not None
    assert kinds.isdisjoint(changed.denied)


@pytest.mark.parametrize(
    ("relative", "before", "after", "expected_probe"),
    [
        (
            "runtime/molt-ir/src/ir.rs",
            "validate_simple_ir_transport_contract(ir)?;",
            "",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-ir/src/ir_schema.rs",
            "validate_value_transport(op)?;",
            "",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-ir/src/ir_schema.rs",
            "if !simpleir_kind_is_registered(kind)",
            "if simpleir_kind_is_registered(kind)",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-ir/src/ir_schema.rs",
            "validate_registered_op_kind(&op.kind)?;",
            "let _ = validate_registered_op_kind(&op.kind);",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-ir/src/ir_schema.rs",
            "operands.unwrap_or(0) != shape.operands",
            "operands.unwrap_or(0) > shape.operands",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-ir/src/ir_schema.rs",
            "if actual != expected",
            "if actual > expected",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-ir/src/ir_schema.rs",
            "simpleir_op_shape(&op.kind).map_or(1, |shape| shape.operands)",
            "simpleir_op_shape(&op.kind).map_or(0, |shape| shape.operands)",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-ir/src/ir_schema.rs",
            'name.is_empty() || name == "none"',
            'name.is_empty() || name == "n one"',
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-tir/src/target_admission/runtime.rs",
            "requirements.difference(supported_requirements)",
            "supported_requirements",
            "rust_backend_lowering_gap",
        ),
        (
            "runtime/molt-ir/src/tir/target_info.rs",
            "Self::runtime_semantics_for(TargetKind::Rust)",
            "Self::runtime_semantics_for(TargetKind::NativeCranelift)",
            "rust_backend_lowering_gap",
        ),
        (
            "runtime/molt-ir/src/tir/op_kinds_generated.rs",
            '"build_tuple" | "tuple_new" => Some(SimpleIrRuntimeRequirements(2))',
            '"build_tuple" | "tuple_new" => Some(SimpleIrRuntimeRequirements(0))',
            "rust_backend_lowering_gap",
        ),
        (
            "runtime/molt-tir/src/target_admission/numeric.rs",
            "if capabilities.arbitrary_precision_integers\n",
            "if capabilities.arbitrary_precision_integers || true\n",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-backend-rust/src/rust/op_emitter.rs",
            '"warn_stderr" => self.emit_op_warn_stderr(op),',
            "",
            "rust_backend_lowering_gap",
        ),
        (
            "runtime/molt-ir/src/tir/op_kinds_generated.rs",
            'kind: "type_guard",\n        family: "value_transport",\n        operands: 1,',
            'kind: "type_guard",\n        family: "value_transport",\n        operands: 2,',
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-backend-rust/src/rust/op_emitter.rs",
            "if self.emit_op_literal(op)",
            "if false && self.emit_op_literal(op)",
            "rust_backend_lowering_gap",
        ),
        (
            "runtime/molt-backend-rust/src/rust/op_emitter/values.rs",
            "SimpleLiteral::Float(value) => Ok(format!(",
            "SimpleLiteral::Float(value) => Err(format!(",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-backend-rust/src/rust/op_emitter/values.rs",
            "let source = source.name;",
            'self.emit_unsupported_op(op, "new valid-input rejection"); let source = source.name;',
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-backend-rust/src/rust/op_emitter/gaps.rs",
            'if out != "_" && out != "none" && !out.is_empty()',
            "if true",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-backend-rust/src/rust.rs",
            "let is_main = func.name",
            "self.emit_op_local_copy(&OpIR::default()); let is_main = func.name",
            "rust_backend_rejection_applicability",
        ),
        (
            "runtime/molt-backend-rust/src/rust.rs",
            "func.ops.clone()",
            "Vec::new()",
            "rust_backend_lowering_gap",
        ),
    ],
)
def test_rust_admitted_domain_mutations_restore_obligations(
    tmp_path, relative, before, after, expected_probe
):
    _copy_live_rust_admission_sources(tmp_path)
    # A failed baseline projection cannot serve as a negative mutation oracle.
    assert SA.probe_rust_backend_lowering_gaps(tmp_path) == []
    path = tmp_path / relative
    source = path.read_text(encoding="utf-8")
    assert before in source
    path.write_text(source.replace(before, after, 1), encoding="utf-8")
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert any(finding.probe == expected_probe for finding in findings), findings


def test_rust_branch_projection_preserves_semantic_string_tokens():
    from tools.structural_audit_rust_domains import _same, match_arms

    assert not _same('name == "none"', 'name == "n one"')
    arms = match_arms(
        'match op.kind.as_str() { "live" => { self.ok(op); } _ => self.fail(op), }',
        "op.kind.as_str()",
    )
    assert arms is not None and len(arms) == 2
    assert arms[0].pattern.strip() == '"live"'
    assert arms[1].pattern.strip() == "_"


def test_rust_branch_projection_sibling_parsers_keep_exact_literal_ranges():
    from tools.structural_audit_rust_admission import literal_dispatch_arm_ranges
    from tools.structural_audit_rust_domains import match_arms

    source = (
        'match op.kind.as_str() { /* before */ "first" | "_alias" => self.one(op), '
        '"guarded" if predicate("=>") => self.two(op), '
        '"last" => { self.three(op); } _ => self.fail(op), }'
    )
    arms = match_arms(source, "op.kind.as_str()")
    assert arms is not None and len(arms) == 4
    ranges = literal_dispatch_arm_ranges(source)
    assert [names for _, _, names in ranges] == [
        frozenset({"first", "_alias"}),
        frozenset({"last"}),
    ]
    assert [start for start, _, _ in ranges] == [
        source.index('"first"'),
        source.index('"last"'),
    ]
    assert "self.one(op)" in source[ranges[0][0] : ranges[0][1]]
    assert "self.fail(op)" not in source[ranges[1][0] : ranges[1][1]]
    assert arms[-1].start == source.index("_ =>")


def test_generated_literal_table_preserves_wire_literal_content():
    from tools.structural_audit_rust_admission import _generated_literal_table

    source = 'match kind { "nop" => Some(Mask(0)), /* skip */ _ => None, }'
    result = r"Some\(Mask\((?P<value>\d+)\)\)"
    assert _generated_literal_table(source, result, "_=>None,") == {"nop": "0"}
    assert (
        _generated_literal_table(source.replace('"nop"', '"n op"'), result, "_=>None,")
        is None
    )
    assert (
        _generated_literal_table(
            source.replace('"nop"', 'r#"nop"#'), result, "_=>None,"
        )
        is None
    )


def test_rust_admitted_domain_unknown_wire_requires_fail_closed_admission(tmp_path):
    _copy_live_rust_admission_sources(tmp_path)
    dispatcher = tmp_path / "runtime/molt-backend-rust/src/rust/op_emitter.rs"
    source = dispatcher.read_text(encoding="utf-8")
    assert "_ => self.emit_op_other(op)," in source
    dispatcher.write_text(
        source.replace(
            "_ => self.emit_op_other(op),",
            '"__unregistered_wire" => self.emit_op_other(op),\n_ => self.emit_op_other(op),',
            1,
        ),
        encoding="utf-8",
    )
    # An unknown spelling does not become an admitted operation merely because
    # a dispatcher contains an explicit refusal for it.
    assert SA.probe_rust_backend_lowering_gaps(tmp_path) == []
    runtime = tmp_path / "runtime/molt-tir/src/target_admission/runtime.rs"
    source = runtime.read_text(encoding="utf-8")
    before = "let Some(requirements) = op.runtime_requirements() else {"
    assert before in source
    runtime.write_text(
        source.replace(before, before + " continue;", 1), encoding="utf-8"
    )
    findings = SA.probe_rust_backend_lowering_gaps(tmp_path)
    assert any(finding.probe == "rust_backend_lowering_gap" for finding in findings)


def test_compatibility_protocol_inventory_fails_closed(tmp_path):
    protocol = SA.compatibility_errors
    # The same byte-exact projection used by the gate must match the committed
    # consumer after the normal repository formatting/generation convergence.
    assert protocol.projection_errors(ROOT) == []
    receipt = tmp_path / protocol.SOURCE_RECEIPTS
    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.write_text(
        (ROOT / protocol.SOURCE_RECEIPTS).read_text(encoding="utf-8"), encoding="utf-8"
    )
    for name, source in protocol.projections().items():
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")
    for facts in protocol.OUTCOMES.values():
        path = tmp_path / facts.witness
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
    assert protocol.projection_errors(tmp_path) == []
    source = "from _compatibility_errors import counter_fromkeys_error as error\nraise error()\n"
    assert protocol.python_inventory(source, True)[0].proved
    assert not protocol.python_inventory(
        source.replace("error()", "error(context)"), True
    )[0].proved
    assert not protocol.python_inventory(
        source.replace("counter_fromkeys_error", "unknown_error"), True
    )[0].proved
    assert not protocol.python_inventory(source + "error = object()\n", True)[0].proved
    assert not protocol.python_inventory(source, False)[0].proved
    rust = "use crate::builtins::compatibility_error::CompatibilityError; CompatibilityError::MemoryviewLookup { rank: 2 }.raise(py);"
    assert protocol.rust_inventory(rust, SA.mask_rust_comments_and_strings(rust), True)[
        0
    ].proved
    unknown = rust.replace("MemoryviewLookup", "Unknown")
    assert not protocol.rust_inventory(unknown, unknown, True)[0].proved
    emitter = tmp_path / protocol.RUST_PATH
    canonical = emitter.read_text(encoding="utf-8")
    emitter.write_bytes(canonical.replace("\n", "\r\n").encode("utf-8"))
    assert protocol.projection_errors(tmp_path) == []
    # Checkout newline policy is harmless; diagnostic spacing and predicates
    # are semantic and cannot be erased by token/whitespace normalization.
    emitter.write_text(
        canonical.replace(
            "multi-dimensional sub-views", "multi-dimensional  sub-views"
        ),
        encoding="utf-8",
    )
    assert protocol.projection_errors(tmp_path)
    emitter.write_text(canonical.replace("rank > 1", "rank > 0"), encoding="utf-8")
    assert protocol.projection_errors(tmp_path)
    findings = SA.probe_rust_stub_surfaces(tmp_path)
    assert any(item.probe == "rust_stub_surface" for item in findings)


def test_compatibility_protocol_generation_requires_receipts_and_witnesses(
    tmp_path, monkeypatch
):
    protocol = SA.compatibility_errors
    assert protocol.main(["--check"]) == 0
    assert set(protocol.generated_outputs()) == {
        protocol.ROOT / name for name in protocol.projections()
    }
    # Receipts and witnesses are generation inputs: without them nothing is
    # published, so neither --check nor --write can bless an unpinned projection.
    monkeypatch.setattr(protocol, "ROOT", tmp_path)
    with pytest.raises(ValueError, match="source receipts are missing"):
        protocol.main(["--write"])
    assert not (tmp_path / protocol.RUST_PATH).exists()
    receipt = tmp_path / protocol.SOURCE_RECEIPTS
    receipt.parent.mkdir(parents=True)
    receipt.write_text(
        (ROOT / protocol.SOURCE_RECEIPTS).read_text(encoding="utf-8"), encoding="utf-8"
    )
    with pytest.raises(ValueError, match="missing compatibility witness"):
        protocol.main(["--check"])
    for facts in protocol.OUTCOMES.values():
        path = tmp_path / facts.witness
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
    assert protocol.main(["--check"]) == 1
    assert protocol.main(["--write"]) == 0
    assert protocol.main(["--check"]) == 0
    assert protocol.projection_errors(tmp_path) == []


def test_compatibility_classification_does_not_exempt_raw_raises(tmp_path):
    source = 'raise NotImplementedError("Counter.fromkeys() is undefined.  Use Counter(iterable) instead.")'
    hits = SA._python_stub_surface_hits(tmp_path / "unrelated.py", source)
    assert len(hits) == 1
    rust = 'fn unrelated() { raise_exception(py, "NotImplementedError", "multi-dimensional sub-views are not implemented"); }'
    assert len(SA._rust_stub_surface_hits(rust)) == 1


def _operation_fixture(root):
    (root / "src").mkdir(parents=True)
    (root / "tools").mkdir()
    for name in ("a.py", "b.py"):
        (root / "src" / name).write_text("# TODO repair\n", encoding="utf-8")
    (root / "tools/generator_manifest.toml").write_text("", encoding="utf-8")
    return root


def test_audit_nested_root_scope_and_exception_restore(tmp_path):
    left = _operation_fixture(tmp_path / "left")
    right = _operation_fixture(tmp_path / "right")
    with SA.audit_operation(left, frozenset({"src/a.py"})):
        assert [f.location for f in SA.probe_debt_markers(left)] == ["src/a.py:1"]
        with pytest.raises(ValueError, match="nested"):
            with SA.audit_operation(right, frozenset({"src/b.py"})):
                assert [f.location for f in SA.probe_debt_markers(right)] == [
                    "src/b.py:1"
                ]
                raise ValueError("nested")
        assert [f.location for f in SA.probe_debt_markers(left)] == ["src/a.py:1"]
    assert {f.location for f in SA.probe_debt_markers(right)} == {
        "src/a.py:1",
        "src/b.py:1",
    }


def test_audit_manifest_and_source_refresh_between_operations(tmp_path):
    root = _operation_fixture(tmp_path)
    manifest = root / "tools/generator_manifest.toml"
    source = root / "src/a.py"
    with SA.audit_operation(root):
        assert not SA._is_generated(source, root)
        assert SA._source_text(source) == "# TODO repair\n"
        source.write_text("# finished\n", encoding="utf-8")
        manifest.write_text('[[generator]]\noutputs=["src/a.py"]\n', encoding="utf-8")
        assert not SA._is_generated(source, root)
        assert SA._source_text(source) == "# TODO repair\n"
        with SA.audit_operation(root):
            assert SA._is_generated(source, root)
            assert SA._source_text(source) == "# finished\n"
        assert not SA._is_generated(source, root)
    with SA.audit_operation(root):
        assert SA._is_generated(source, root)
        assert SA._source_text(source) == "# finished\n"
    manifest.write_text("", encoding="utf-8")
    assert not SA._is_generated(source, root)
    assert [f.location for f in SA.probe_debt_markers(root)] == ["src/b.py:1"]


def test_audit_concurrent_run_scopes_do_not_leak_or_resurrect(tmp_path, monkeypatch):
    from threading import Event, Thread

    left = _operation_fixture(tmp_path / "left")
    right = _operation_fixture(tmp_path / "right")
    entered_a, entered_b, exited_a = Event(), Event(), Event()
    results, errors = {}, []

    def probe(root):
        initial = SA.probe_debt_markers(root)
        if root == left:
            entered_a.set()
            assert entered_b.wait(5)
        else:
            assert entered_a.wait(5)
            entered_b.set()
            assert exited_a.wait(5)
        assert SA.probe_debt_markers(root) == initial
        return initial

    monkeypatch.setattr(SA, "PROBES", (probe,))

    def run(root, name):
        try:
            results[name] = SA.run_all(root, frozenset({f"src/{name}.py"}))
        except BaseException as exc:
            errors.append(exc)
        finally:
            if name == "a":
                exited_a.set()
            else:
                entered_b.set()

    threads = [
        Thread(target=run, args=(left, "a")),
        Thread(target=run, args=(right, "b")),
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(12)
    assert not any(thread.is_alive() for thread in threads)
    assert not errors
    assert [f.location for f in results["a"]] == ["src/a.py:1"]
    assert [f.location for f in results["b"]] == ["src/b.py:1"]
    (right / "src/b.py").write_text("# finished\n", encoding="utf-8")
    assert [f.location for f in SA.probe_debt_markers(right)] == ["src/a.py:1"]


def _process_wide_patch_count(tmp_path: Path, body: str) -> int:
    tests = tmp_path / "tests"
    tests.mkdir(parents=True, exist_ok=True)
    (tests / "test_fixture.py").write_text(body, encoding="utf-8")
    return sum(int(f.metric) for f in SA.probe_process_wide_test_patches(tmp_path))


def test_process_wide_patch_probe_counts_rebinding_a_shared_module(tmp_path: Path):
    body = (
        "import subprocess\n"
        "def test_a(monkeypatch):\n"
        "    monkeypatch.setattr(mod.os, 'getpid', lambda: 1)\n"
        "    monkeypatch.setattr(\n"
        "        mod.subprocess,\n"
        "        'run',\n"
        "        fake,\n"
        "    )\n"
        "    monkeypatch.setattr(subprocess, 'Popen', fake)\n"
        "    monkeypatch.setattr('pkg.mod.time.monotonic', fake)\n"
    )
    assert _process_wide_patch_count(tmp_path, body) == 4


def test_process_wide_patch_probe_accepts_patches_of_an_installed_view(
    tmp_path: Path,
):
    body = (
        "def test_a(monkeypatch):\n"
        "    install_module_view(monkeypatch, 'os', os, mod, getpid=fake)\n"
        "    monkeypatch.setattr(mod.os, 'kill', fake)\n"
        "    install_module_os_view(monkeypatch, other, name='nt')\n"
        "    monkeypatch.setattr(other.os, 'getpid', fake)\n"
    )
    assert _process_wide_patch_count(tmp_path, body) == 0


def test_process_wide_patch_probe_does_not_extend_a_view_to_other_modules(
    tmp_path: Path,
):
    body = (
        "def test_a(monkeypatch):\n"
        "    install_module_view(monkeypatch, 'os', os, mod, getpid=fake)\n"
        "    monkeypatch.setattr(helper.os, 'getpid', fake)\n"
        "def test_b(monkeypatch):\n"
        "    monkeypatch.setattr(mod.os, 'kill', fake)\n"
    )
    assert _process_wide_patch_count(tmp_path, body) == 2


def test_process_wide_patch_probe_ignores_process_wide_by_nature(tmp_path: Path):
    body = (
        "def test_a(monkeypatch):\n"
        "    monkeypatch.setattr(mod.sys, 'argv', ['molt'])\n"
        "    monkeypatch.setattr(sys, 'path', [])\n"
        "    monkeypatch.setattr(mod.os, 'environ', {})\n"
        "    monkeypatch.setattr(mod, 'helper', fake)\n"
    )
    assert _process_wide_patch_count(tmp_path, body) == 0


def _raw_intrinsic_binding_count(tmp_path: Path, relative: str, body: str) -> int:
    path = tmp_path / "src" / "molt" / "stdlib" / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    return sum(int(f.metric) for f in SA.probe_stdlib_raw_intrinsic_names(tmp_path))


def test_raw_intrinsic_probe_counts_module_scope_bindings(tmp_path: Path):
    body = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        "molt_spawn = _require_intrinsic('molt_spawn')\n"
        "_MOLT_PRIVATE = _require_intrinsic('molt_private')\n"
        "_require_intrinsic('molt_injected', globals())\n"
        "from asyncio import molt_block_on\n"
        "try:\n"
        "    molt_guarded = _require_intrinsic('molt_guarded')\n"
        "except RuntimeError:\n"
        "    pass\n"
        "if TYPE_CHECKING:\n"
        "    def molt_declared() -> None: ...\n"
        "def helper():\n"
        "    molt_local = _require_intrinsic('molt_local')\n"
        "    return molt_local\n"
    )
    # molt_spawn, the injected name, the import and the guarded binding leak;
    # a private name, a type-checking declaration and a local do not.
    assert _raw_intrinsic_binding_count(tmp_path, "mod.py", body) == 4


def test_raw_intrinsic_probe_ignores_code_outside_the_stdlib(tmp_path: Path):
    body = "molt_spawn = object()\n"
    stdlib_count = _raw_intrinsic_binding_count(tmp_path, "mod.py", body)
    outside = tmp_path / "src" / "molt" / "cli.py"
    outside.write_text(body, encoding="utf-8")
    assert (
        sum(int(f.metric) for f in SA.probe_stdlib_raw_intrinsic_names(tmp_path))
        == stdlib_count
        == 1
    )


def _build_failure_skip_count(tmp_path: Path, body: str) -> int:
    path = tmp_path / "tests" / "test_sample.py"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    return sum(int(f.metric) for f in SA.probe_build_failure_test_skips(tmp_path))


def test_build_failure_skip_probe_counts_skips_that_hide_a_failed_build(
    tmp_path: Path,
):
    body = (
        "import pytest\n"
        "def test_a(result):\n"
        "    if result.returncode:\n"
        "        pytest.skip(f'Compilation failed: {result.stderr[:300]}')\n"
        "    pytest.skip('Build/run error: x')\n"
        "    pytest.skip('one or both builds failed')\n"
        "    pytest.skip('Backend killed during compilation (stale daemon)')\n"
    )
    assert _build_failure_skip_count(tmp_path, body) == 4


def test_build_failure_skip_probe_keeps_capability_skips(tmp_path: Path):
    body = (
        "import pytest\n"
        "def test_a():\n"
        "    pytest.skip('cargo is required for backend compilation.')\n"
        "    pytest.skip('clang is required for target C data-model compilation')\n"
        "    pytest.skip(reason)\n"
        "    pytest.fail('Compilation failed')\n"
    )
    assert _build_failure_skip_count(tmp_path, body) == 0


def test_raise_collector_finds_every_raise_in_every_statement_block():
    source = (
        "def f(x):\n"
        "    if x:\n"
        "        raise A\n"
        "    elif x:\n"
        "        raise B\n"
        "    else:\n"
        "        raise C\n"
        "    for _ in x:\n"
        "        raise D\n"
        "    else:\n"
        "        raise E\n"
        "    while x:\n"
        "        raise F\n"
        "    try:\n"
        "        raise G\n"
        "    except H:\n"
        "        raise I\n"
        "    else:\n"
        "        raise J\n"
        "    finally:\n"
        "        raise K\n"
        "    with x:\n"
        "        raise L\n"
        "    match x:\n"
        "        case 1:\n"
        "            raise M\n"
        "    class N:\n"
        "        def g(self):\n"
        "            raise O\n"
        "    try:\n"
        "        pass\n"
        "    except* P:\n"
        "        raise Q\n"
        "    return lambda: x\n"
    )
    tree = ast.parse(source)
    expected = sorted(
        node.lineno for node in ast.walk(tree) if isinstance(node, ast.Raise)
    )
    assert len(expected) == 14
    assert sorted(node.lineno for node in SA._python_raise_nodes(tree, source)) == (
        expected
    )
