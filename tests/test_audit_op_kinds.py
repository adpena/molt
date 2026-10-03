"""Regression protection for tools/audit_op_kinds.py — specifically the D8
``native_codegen_gap`` enforcement that closes the dispatch ``handler-arm ⊄
HANDLED_KINDS`` hole (the ``copy`` P0 instance).

The native dispatch routes op-kinds purely via each ``fc/*`` handler's
``HANDLED_KINDS`` slice (``op_family::native_op_family``). A result-producing kind
that the frontend emits but that NO slice claims is dead at its handler ``match``
arm and hits the dispatch's loud catch-all panic at codegen — exactly how
``copy`` shipped broken (matched in ``value_transfer.rs`` but absent from
``value_transfer::HANDLED_KINDS``). These tests prove the audit's
``native_codegen_gap`` cell:

  1. is EMPTY on the current healthy tree,
  2. CATCHES synthetic unrouted result-producing and no-result kinds and NAMES them,
  3. correctly classifies result-producing vs no-result kinds from the
     ``lower_to_simple.rs`` ``fn lower_op`` ``out:`` dispositions, and
  4. treats handler ``match`` arm / ``HANDLED_KINDS`` drift as a dangerous cell.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from dataclasses import replace
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "tools" / "audit_op_kinds.py"


def _load_tool():
    spec = importlib.util.spec_from_file_location("molt_test_audit_op_kinds", TOOL)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_audit_op_kinds"] = module
    spec.loader.exec_module(module)
    return module


AUDIT = _load_tool()


def test_llvm_generic_eligibility_uses_generated_boxed_contracts() -> None:
    from tools.llvm_runtime_abi_audit import runtime_boxed_abi_facts

    actual = AUDIT.extract_llvm_boxed_runtime_abis()
    assert actual == {fact.symbol: fact for fact in runtime_boxed_abi_facts().values()}
    assert actual["molt_spawn"].return_abi == "I64"
    assert actual["molt_print_newline"].return_abi == "Void"
    assert actual["molt_cell_new"].arity == 1
    for symbol in (
        "molt_int_from_i64",
        "molt_int_as_i64",
        "molt_obj_get_state",
        "molt_function_closure_bits",
    ):
        assert symbol not in actual
    assert AUDIT.llvm_boxed_runtime_abi_mismatches() == []


def test_boxed_abi_drift_is_dangerous_for_value_and_void_calls() -> None:
    result = AUDIT.run_audit()
    broken = replace(
        result,
        llvm_boxed_runtime_abi_mismatch=[
            "return-mismatch:molt_cell_new/1",
            "arity-mismatch:molt_spawn/1",
        ],
    )
    assert broken.dangerous()["llvm_boxed_runtime_abi_mismatch"] == [
        "arity-mismatch:molt_spawn/1",
        "return-mismatch:molt_cell_new/1",
    ]
    assert "llvm_void_runtime_abi_mismatch" not in broken.dangerous()


def test_frontend_direct_lowered_kind_expression_uses_its_guard(
    tmp_path: Path, monkeypatch
) -> None:
    path = tmp_path / AUDIT.SERIALIZATION_PY.relative_to(ROOT)
    path.parent.mkdir(parents=True)
    path.write_text(
        """
def emit(op):
    if op.kind in {"FUNC_NEW", "FUNC_NEW_CLOSURE"}:
        return {"kind": op.kind.lower()}
def unguarded(op):
    return {"kind": op.kind.lower()}
""",
        encoding="utf-8",
    )
    monkeypatch.setattr(AUDIT, "SERIALIZATION_MODULES", (AUDIT.SERIALIZATION_PY,))
    result = AUDIT.extract_frontend_kinds(root=tmp_path)
    assert result.all == {"func_new", "func_new_closure"}
    assert len(result.unresolved) == 1


def test_extracted_serialization_function_kinds_are_resolved() -> None:
    result = AUDIT.extract_frontend_kinds()
    assert {"func_new", "func_new_closure"} <= result.all
    assert not result.unresolved


def test_self_validation_passes_on_current_tree() -> None:
    """The audit's own ground-truth anchors (including the D8 anchors) must pass."""
    res = AUDIT.run_audit()
    fails = AUDIT.self_validate(res)
    assert fails == [], "\n".join(fails)


def test_native_codegen_gap_empty_on_healthy_tree() -> None:
    """No result-producing emitted kind may lack a native routing slice today."""
    res = AUDIT.run_audit()
    assert res.dangerous()["native_codegen_gap"] == []


def test_copy_is_result_producing_and_natively_routed() -> None:
    """The 2026-06-24 instance: ``copy`` is frontend-emitted, result-producing,
    and now claimed by ``value_transfer::HANDLED_KINDS``. If any of these
    regressed, D8 would either miss the bug (not result-producing) or fire
    (not routed)."""
    res = AUDIT.run_audit()
    row = res.rows["copy"]
    assert row.frontend_emits
    assert row.produces_result
    assert row.native_arm


def test_no_result_statement_kinds_are_natively_routed() -> None:
    """No-result statement ops are still side-effect/control-flow facts. They
    must be classified non-result for reporting, but D8 must require an explicit
    native routing slice so the catch-all cannot silently skip them."""
    res = AUDIT.run_audit()
    for kind in ("del_boundary", "try_start", "try_end"):
        row = res.rows[kind]
        assert row.frontend_emits, kind
        assert row.mapper_maps, kind
        assert not row.produces_result, (
            f"{kind} must be non-result (lower_op emits it with no `out`)"
        )
        assert row.native_routing_slice, kind


def test_lower_op_nonresult_extractor_ground_truth() -> None:
    """The ``lower_to_simple.rs`` ``fn lower_op`` ``out:`` extractor must agree
    with hand-verified ground truth: the no-result statement ops are present, and
    result-producing ops (and passthrough ops not in ``lower_op``) are absent."""
    nonresult = AUDIT.extract_native_lower_nonresult_kinds()
    # No-result statement ops emitted by lower_op with `..OpIR::default()` (no out).
    for kind in ("del_boundary", "try_end", "try_start"):
        assert kind in nonresult, kind
    # Result-producing op in lower_op (`out: out_var`) must NOT be exempt.
    assert "alloc" not in nonresult
    # Passthrough kinds (lowered via lower_preserved_op, NOT lower_op) must NOT be
    # exempt — the safe direction that keeps `copy`/`add` checked by D8.
    assert "copy" not in nonresult
    assert "add" not in nonresult


def _synthetic_broken_result(routing_slice: set[str]) -> "object":
    """Build an AuditResult identical to the live audit EXCEPT the native ROUTING
    SLICE membership is the given (possibly broken) set. D8 keys on
    ``native_routing_slice`` (the exact HANDLED_KINDS/INLINE/NO_CODEGEN authority),
    so this faithfully reproduces an arm⊄HANDLED_KINDS regression. Reuses every
    other extractor so the synthetic state matches the real pipeline."""
    res = AUDIT.run_audit()
    nonresult = AUDIT.extract_native_lower_nonresult_kinds()
    new_rows = {
        kind: replace(
            row,
            native_routing_slice=kind in routing_slice,
            produces_result=kind not in nonresult,
        )
        for kind, row in res.rows.items()
    }
    return replace(res, rows=new_rows)


def test_d8_catches_unrouted_copy() -> None:
    """Reproduce the exact pre-fix bug: remove ``copy`` from the native routing
    SLICE and confirm D8 fires and NAMES ``copy``."""
    res = AUDIT.run_audit()
    assert res.rows["copy"].native_routing_slice, (
        "precondition: copy is in a routing slice on this tree"
    )
    live_slice = {k for k, r in res.rows.items() if r.native_routing_slice}
    broken = _synthetic_broken_result(live_slice - {"copy"})
    gap = broken.dangerous()["native_codegen_gap"]
    assert "copy" in gap, gap
    # The healthy tree must remain green when copy is present in the slice.
    healthy = _synthetic_broken_result(live_slice)
    assert healthy.dangerous()["native_codegen_gap"] == []


def test_d8_keys_on_exact_slice_not_advisory_native_arm() -> None:
    """The textual ``native_arm`` over-counts (it picks up ``"copy" =>`` arms in
    unrelated pre-analysis helpers in function_compiler.rs). D8 MUST key on the
    exact routing slice, or that over-count masks the copy bug. Assert the two
    authorities actually differ for ``copy`` is NOT required, but D8's behaviour
    must follow the slice: with copy out of the slice (but still in native_arm),
    D8 still fires."""
    advisory = AUDIT.extract_native_simpleir_arm_kinds()
    slices = AUDIT.extract_native_routing_slice_kinds()
    # copy is in BOTH today; the slice is the authority. Simulate the real bug:
    # copy present in advisory (textual helper arm) but absent from the slice.
    assert "copy" in advisory and "copy" in slices
    broken = _synthetic_broken_result(slices - {"copy"})
    # Even though copy would still be in the advisory native_arm, D8 fires:
    assert broken.rows["copy"].native_arm  # advisory untouched by the helper
    assert not broken.rows["copy"].native_routing_slice
    assert "copy" in broken.dangerous()["native_codegen_gap"]


def test_d8_catches_unrouted_no_result_kind() -> None:
    """Removing a no-result statement kind from the native slices must trip D8:
    no-result side effects and control metadata are not allowed to disappear
    through the catch-all."""
    res = AUDIT.run_audit()
    live_slice = {k for k, r in res.rows.items() if r.native_routing_slice}
    broken = _synthetic_broken_result(live_slice - {"try_end", "del_boundary"})
    gap = broken.dangerous()["native_codegen_gap"]
    assert "try_end" in gap
    assert "del_boundary" in gap


def test_d9_treats_handler_arm_slice_drift_as_dangerous() -> None:
    res = AUDIT.run_audit()
    assert res.dangerous()["native_handler_routing_drift"] == []
    broken = replace(
        res,
        native_handler_routing_drift=[
            "runtime/molt-backend-native/src/native_backend/function_compiler/fc/value_transfer.rs:"
            "handle_value_transfer_op:copy:arm-not-in-HANDLED_KINDS"
        ],
    )
    assert broken.dangerous()["native_handler_routing_drift"] == [
        "runtime/molt-backend-native/src/native_backend/function_compiler/fc/value_transfer.rs:"
        "handle_value_transfer_op:copy:arm-not-in-HANDLED_KINDS"
    ]


def test_d9_uses_routed_slice_union_for_delegated_handlers() -> None:
    """`handle_arith_op` owns both arith and delegated vec-reduction slices."""
    dispatch_slices = AUDIT.extract_native_family_dispatch_slices()
    assert dispatch_slices["Arith"] == [
        ("arith", "HANDLED_KINDS"),
        ("vec_reductions", "HANDLED_KINDS"),
    ]
    handlers = AUDIT.extract_native_family_handlers()
    assert handlers["Arith"] == ("arith", "handle_arith_op")
    assert AUDIT.extract_native_handler_routing_drifts() == []


def test_d9_routes_bitwise_and_matrix_to_dedicated_families() -> None:
    """Bitwise/shift and matrix operators must not collapse back into arith."""
    dispatch_slices = AUDIT.extract_native_family_dispatch_slices()
    assert dispatch_slices["BitwiseShift"] == [("bitwise_shift", "HANDLED_KINDS")]
    assert dispatch_slices["MatrixOps"] == [("matrix_ops", "HANDLED_KINDS")]

    handlers = AUDIT.extract_native_family_handlers()
    assert handlers["BitwiseShift"] == (
        "bitwise_shift",
        "handle_bitwise_shift_op",
    )
    assert handlers["MatrixOps"] == ("matrix_ops", "handle_matrix_op")
    assert AUDIT.extract_native_handler_routing_drifts() == []


def _native_projection_fixture(tmp_path: Path, monkeypatch, opcode: str = "ConstInt"):
    data = AUDIT._load_op_kinds_toml()
    row = next(row for row in data["kind"] if row.get("mapper_opcode") == opcode)
    spellings = {row["canonical"], *row.get("aliases", [])}
    path = tmp_path / "fixture.rs"
    routes = "\n".join(f"    {json.dumps(kind)}," for kind in sorted(spellings))
    path.write_text(
        f"""
const HANDLED_KINDS: &[&str] = &[
{routes}
];
fn project_wire(wire: &str) -> &str {{
    if crate::tir::op_kinds_generated::kind_to_opcode_table(wire)
        == Some(crate::tir::ops::OpCode::{opcode})
    {{
        {json.dumps(row["canonical"])}
    }} else {{
        wire
    }}
}}
fn handle_fixture(op: &OpIR) {{
    match project_wire(&op.kind) {{
        {json.dumps(row["canonical"])} => {{}},
        _ => panic!("unsupported"),
    }}
}}
""",
        encoding="utf-8",
    )
    monkeypatch.setattr(AUDIT, "ROOT", tmp_path)
    monkeypatch.setattr(AUDIT, "NATIVE_FC_DIR", tmp_path)
    monkeypatch.setattr(
        AUDIT,
        "extract_native_family_dispatch_slices",
        lambda: {"Fixture": [("fixture", "HANDLED_KINDS")]},
    )
    monkeypatch.setattr(
        AUDIT,
        "extract_native_family_handlers",
        lambda: {"Fixture": ("fixture", "handle_fixture")},
    )
    return path, row, data


@pytest.mark.parametrize("opcode", ["ConstInt", "Bool"])
def test_d9_registry_projection_expands_raw_alias_preimage(
    tmp_path: Path, monkeypatch, opcode: str
) -> None:
    path, row, _ = _native_projection_fixture(tmp_path, monkeypatch, opcode)
    spellings = {row["canonical"], *row["aliases"]}
    assert (
        AUDIT.extract_native_handler_arm_kinds(path, "handle_fixture", spellings)
        == spellings
    )
    assert AUDIT.extract_native_handler_routing_drifts() == []


def test_d9_live_literal_projection_uses_registry_aliases() -> None:
    path = AUDIT.NATIVE_FC_DIR / "const_literals.rs"
    routes = AUDIT.extract_rust_str_slice_consts(path)["HANDLED_KINDS"]
    assert (
        AUDIT.extract_native_handler_arm_kinds(path, "handle_const_literal_op", routes)
        == routes
    )


def test_d9_missing_alias_route_is_not_erased_by_canonicalization(
    tmp_path: Path, monkeypatch
) -> None:
    path, row, _ = _native_projection_fixture(tmp_path, monkeypatch)
    missing = row["aliases"][0]
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            f"    {json.dumps(missing)},\n", "", 1
        ),
        encoding="utf-8",
    )
    assert AUDIT.extract_native_handler_routing_drifts() == [
        f"fixture.rs:handle_fixture:{missing}:arm-not-in-fixture::HANDLED_KINDS"
    ]


def test_d9_new_registry_alias_requires_its_own_raw_route(
    tmp_path: Path, monkeypatch
) -> None:
    _, row, data = _native_projection_fixture(tmp_path, monkeypatch)
    row["aliases"].append("future_literal_alias")
    monkeypatch.setattr(AUDIT, "_load_op_kinds_toml", lambda: data)
    assert AUDIT.extract_native_handler_routing_drifts() == [
        "fixture.rs:handle_fixture:future_literal_alias:arm-not-in-fixture::HANDLED_KINDS"
    ]


def test_d9_projected_alias_arm_is_unreachable_not_coverage(
    tmp_path: Path, monkeypatch
) -> None:
    path, row, _ = _native_projection_fixture(tmp_path, monkeypatch)
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            f"{json.dumps(row['canonical'])} =>", f"{json.dumps(row['aliases'][0])} =>"
        ),
        encoding="utf-8",
    )
    with pytest.raises(
        AUDIT.RustMatchParseError, match="unreachable projected alias arms"
    ):
        AUDIT.extract_native_handler_routing_drifts()


def test_d9_missing_canonical_arm_does_not_claim_any_aliases(
    tmp_path: Path, monkeypatch
) -> None:
    path, row, _ = _native_projection_fixture(tmp_path, monkeypatch)
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            f"        {json.dumps(row['canonical'])} => {{}},\n", ""
        ),
        encoding="utf-8",
    )
    assert AUDIT.extract_native_handler_routing_drifts() == [
        f"fixture.rs:handle_fixture:{kind}:fixture::HANDLED_KINDS-not-in-arm"
        for kind in sorted({row["canonical"], *row["aliases"]})
    ]


@pytest.mark.parametrize(
    "before,after",
    [
        ("== Some", "!= Some"),
        ("kind_to_opcode_table(wire)", "untrusted_transform(wire)"),
        ("if crate::", "let extra = wire; if crate::"),
        ("    } else {\n        wire", '    } else {\n        "const"'),
        ('        "const"\n', '        "const_float"\n'),
        ("OpCode::ConstInt", "OpCode::UnknownOpcode"),
    ],
)
def test_d9_projection_requires_the_complete_generated_contract(
    tmp_path: Path, monkeypatch, before: str, after: str
) -> None:
    path, _, _ = _native_projection_fixture(tmp_path, monkeypatch)
    source = path.read_text(encoding="utf-8")
    assert before in source
    path.write_text(source.replace(before, after, 1), encoding="utf-8")
    with pytest.raises(AUDIT.RustMatchParseError):
        AUDIT.extract_native_handler_routing_drifts()


def test_d9_opaque_dispatch_cannot_borrow_a_nested_raw_match(
    tmp_path: Path, monkeypatch
) -> None:
    path, _, _ = _native_projection_fixture(tmp_path, monkeypatch)
    path.write_text(
        """
fn handle_fixture(op: &OpIR) {
    match opaque(op.kind.as_str()) {
        _ => match op.kind.as_str() { "const" => (), _ => () },
    }
}
""",
        encoding="utf-8",
    )
    with pytest.raises(AUDIT.RustMatchParseError, match="without a supported"):
        AUDIT.extract_native_handler_arm_kinds(path, "handle_fixture", {"const"})


def test_native_match_parser_stays_inside_real_function_boundaries(
    tmp_path: Path,
) -> None:
    path = tmp_path / "scoped.rs"
    path.write_text(
        """
// fn target() { match op.kind.as_str() { "comment" => () } }
fn target(op: &OpIR) {
    let text = "match op.kind.as_str() { fake }";
}
fn unrelated(op: &OpIR) {
    match op.kind.as_str() { "neighbor" => (), _ => () }
}
""",
        encoding="utf-8",
    )
    with pytest.raises(AUDIT.RustMatchParseError, match="not found in fn target"):
        AUDIT.extract_match_arms(path, "target", "match op.kind.as_str()")
    with pytest.raises(AUDIT.RustMatchParseError, match="without a supported"):
        AUDIT.extract_native_handler_arm_kinds(path, "target", {"const", "const_int"})


def test_llvm_preserved_coverage_comes_from_handler_slices() -> None:
    """LLVM preserved-op coverage must follow handler-owned slices, not the root
    dispatcher text. Direct, callable, and container handlers each own their
    route set beside their lowering match."""
    res = AUDIT.run_audit()

    assert "call_async" in res.llvm_arms
    assert "floordiv" in res.llvm_arms
    assert "func_new" in res.llvm_arms
    assert "dict_new" in res.llvm_arms
    assert AUDIT.extract_llvm_preserved_handler_routing_drifts() == []
    assert res.dangerous()["llvm_preserved_handler_routing_drift"] == []


def test_llvm_preserved_handler_drift_is_dangerous() -> None:
    res = AUDIT.run_audit()
    broken = replace(
        res,
        llvm_preserved_handler_routing_drift=[
            "runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops/direct_ops.rs:"
            "lower_preserved_direct_op:floordiv:arm-not-in-HANDLED_KINDS"
        ],
    )
    assert broken.dangerous()["llvm_preserved_handler_routing_drift"] == [
        "runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops/direct_ops.rs:"
        "lower_preserved_direct_op:floordiv:arm-not-in-HANDLED_KINDS"
    ]


@pytest.fixture(scope="module")
def d10_audit_result():
    return AUDIT.run_audit()


def test_frontend_dormancy_retains_typed_activation_operations(
    d10_audit_result,
) -> None:
    res = d10_audit_result
    for kind in ("is_pending", "state_set", "task_wait"):
        row = res.rows[kind]
        assert row.mapper_maps
        assert not row.frontend_emits
        # D5 observes the frontend only. A typed pass producer is neither an
        # exemption nor evidence that the frontend emitted this spelling.
        assert kind in res.dangerous()["mapped_never_emitted"]


@pytest.mark.parametrize(
    "category, kind, updates, expected_guidance",
    [
        (
            "mapped_never_emitted",
            "state_set",
            {},
            "This category does not prove dead operations or missing mappings",
        ),
        (
            "ownedvalue_never_emitted",
            "vec_sum",
            {"frontend_emits": False},
            "Frontend absence does not prove an unused ownership row",
        ),
        (
            "owned_result_transparent_alias",
            "abs",
            {"classifier_class": "TransparentAlias"},
            "This count is not a count of proven leaks",
        ),
    ],
)
def test_finding_guidance_preserves_categories_baseline_and_verdict(
    d10_audit_result,
    tmp_path: Path,
    monkeypatch,
    capsys,
    category: str,
    kind: str,
    updates: dict,
    expected_guidance: str,
) -> None:
    res = replace(
        d10_audit_result,
        rows={kind: replace(d10_audit_result.rows[kind], **updates)},
    )
    before = AUDIT.to_baseline(res)
    assert {key: rows for key, rows in before["dangerous"].items() if rows} == {
        category: [kind]
    }
    AUDIT.print_report(res)
    report = capsys.readouterr()
    assert f"-- {category} (1) --" in report.out
    assert expected_guidance in report.out
    assert report.err == ""
    assert AUDIT.to_baseline(res) == before

    empty = replace(res, rows={})
    empty_baseline = AUDIT.to_baseline(empty)
    baseline_path = tmp_path / "baseline.json"
    monkeypatch.setattr(AUDIT, "BASELINE_PATH", baseline_path)
    for label, current, baseline, expected_rc in (
        ("exact", res, before, 0),
        ("NEW", res, empty_baseline, 1),
        ("STALE", empty, before, 1),
    ):
        baseline_path.write_text(json.dumps(baseline), encoding="utf-8")
        baseline_bytes = baseline_path.read_bytes()
        current_before = AUDIT.to_baseline(current)
        assert AUDIT.check_against_baseline(current) == expected_rc
        output = capsys.readouterr()
        assert baseline_path.read_bytes() == baseline_bytes
        assert AUDIT.to_baseline(current) == current_before
        if expected_rc:
            assert f"{label} dangerous-cell" in output.err
            assert f"'{category}'" in output.err
            assert kind in output.err
            assert expected_guidance in output.err
            assert "Add a mapper arm" not in output.err
            assert "and leaks its result" not in output.err
        else:
            assert output.err == ""
            assert "baseline is exact" in output.out


def test_d10_owned_results_are_classified_by_their_return_contract(
    d10_audit_result,
) -> None:
    """An owned heap result needs independent custody, even when the boxed
    ABI carrier matches operand 0. These fused split/count kernels return a
    fresh tuple and are classified by that contract. D10 alone does not prove
    that every other owned boxed return has a heap representation."""
    res = d10_audit_result
    owned = res.owned_result_runtime_kinds
    for kind in (
        "string_split_ws_dict_inc",
        "string_split_sep_dict_inc",
        "dict_str_int_inc",
        "operator_index",
    ):
        assert kind in owned
        assert res.rows[kind].produces_result
        assert res.rows[kind].classifier_class == "OwnedValue"
        assert kind not in res.dangerous()["owned_result_transparent_alias"]
    row = res.rows["string_split_ws_dict_inc"]
    leaking = replace(
        res,
        rows={
            **res.rows,
            "string_split_ws_dict_inc": replace(
                row, classifier_class="TransparentAlias"
            ),
        },
    )
    assert (
        "string_split_ws_dict_inc"
        in leaking.dangerous()["owned_result_transparent_alias"]
    )
    # A declared borrowed return is not an owned result.
    assert "dict_set" not in owned


def test_d10_runtime_services_need_an_actual_copy_operation(d10_audit_result) -> None:
    res = d10_audit_result
    kind = "platform_system"
    row = res.rows[kind]
    assert row.produces_result and row.llvm_runtime_fallback_eligible
    assert kind in res.owned_result_runtime_kinds
    assert row.classifier_class == "TransparentAlias"
    assert not row.frontend_emits
    assert not row.mapper_maps
    assert not row.native_routing_slice
    assert not row.llvm_dedicated_arm
    assert kind not in res.dangerous()["owned_result_transparent_alias"]

    # An advisory text match does not turn a runtime callable into an operation.
    advisory = replace(res, rows={kind: replace(row, native_arm=True, wasm_arm=True)})
    assert advisory.dangerous()["owned_result_transparent_alias"] == []

    # Each executable source of operation vocabulary independently establishes
    # scope. A newly emitted, unclassified Copy must be caught before a backend
    # grows a dedicated route; internal preserved routes are also in scope.
    for route in (
        "frontend_emits",
        "native_routing_slice",
        "llvm_dedicated_arm",
    ):
        broken = replace(res, rows={kind: replace(row, **{route: True})})
        assert broken.dangerous()["owned_result_transparent_alias"] == [kind], route

    emitted = replace(row, frontend_emits=True)
    nonresult = replace(res, rows={kind: replace(emitted, produces_result=False)})
    assert nonresult.dangerous()["owned_result_transparent_alias"] == []
    borrowed = replace(
        res,
        rows={kind: emitted},
        owned_result_runtime_kinds=res.owned_result_runtime_kinds - {kind},
    )
    assert borrowed.dangerous()["owned_result_transparent_alias"] == []
    no_heap_move = replace(
        res, rows={kind: emitted}, no_heap_move=res.no_heap_move | {kind}
    )
    assert no_heap_move.dangerous()["owned_result_transparent_alias"] == []


@pytest.mark.parametrize("kind", ["vec_sum", "vec_prod", "vec_min", "vec_max"])
def test_d10_preserved_internal_results_keep_their_ownership_gate(
    d10_audit_result,
    kind: str,
) -> None:
    res = d10_audit_result
    row = res.rows[kind]
    assert row.llvm_runtime_fallback_eligible
    assert not row.llvm_dedicated_arm
    assert row.native_routing_slice
    assert row.produces_result
    assert not row.mapper_maps
    assert kind in res.owned_result_runtime_kinds
    assert row.classifier_class == "OwnedValue"
    assert kind not in res.dangerous()["owned_result_transparent_alias"]
    # Removing frontend visibility must not hide an internally preserved
    # reduction: its native route and normalized boxed contract still own it.
    broken = replace(
        res,
        rows={
            kind: replace(
                row, frontend_emits=False, classifier_class="TransparentAlias"
            )
        },
    )
    assert broken.dangerous()["owned_result_transparent_alias"] == [kind]


def test_d10_copy_scope_uses_the_canonical_mapper(d10_audit_result) -> None:
    res = d10_audit_result
    registry = {
        "kind": [
            {
                "canonical": "internal_copy",
                "aliases": ["internal_copy_alias"],
                "mapper_opcode": "Copy",
            },
            {"canonical": "internal_add", "aliases": [], "mapper_opcode": "Add"},
        ]
    }
    mapped = AUDIT.mapper_kinds_from_registry(registry)
    copies = AUDIT.mapper_kinds_from_registry(registry, opcode="Copy")
    assert mapped == {"internal_copy", "internal_copy_alias", "internal_add"}
    assert copies == {"internal_copy", "internal_copy_alias"}
    service = res.rows["platform_system"]
    rows = {
        kind: replace(service, kind=kind, frontend_emits=True, mapper_maps=True)
        for kind in mapped
    }
    broken = replace(
        res,
        rows=rows,
        mapper_kinds=mapped,
        copy_mapper_kinds=copies,
        owned_result_runtime_kinds=mapped,
    )
    assert broken.dangerous()["owned_result_transparent_alias"] == sorted(copies)
    assert "copy" in res.copy_mapper_kinds
    assert "add" not in res.copy_mapper_kinds


def test_d9_single_kind_handler_can_match_representation_without_kind_dispatch(
    tmp_path,
):
    path = tmp_path / "single.rs"
    path.write_text(
        "fn handle(op: &OpIR) { match op.args.as_deref() { Some(args) => (), None => () } }",
        encoding="utf-8",
    )
    assert AUDIT.extract_native_handler_arm_kinds(path, "handle", {"index"}) == {
        "index"
    }
    with pytest.raises(AUDIT.RustMatchParseError):
        AUDIT.extract_native_handler_arm_kinds(path, "handle", {"index", "store_index"})


def test_d9_single_kind_handler_cannot_infer_an_opaque_wire_kind_domain(tmp_path):
    path = tmp_path / "single.rs"
    path.write_text(
        "fn handle(op: &OpIR) { if opaque(op.kind.as_str()) { invoke(op); } }",
        encoding="utf-8",
    )
    with pytest.raises(AUDIT.RustMatchParseError):
        AUDIT.extract_native_handler_arm_kinds(path, "handle", {"index"})



def test_d10_projects_full_runtime_return_authority_and_actual_aliases(d10_audit_result):
    from wasm_abi_gen.manifest import load_manifest, runtime_operation_return_specs

    contracts = runtime_operation_return_specs(load_manifest())
    for kind in (
        "abs", "json_parse", "msgpack_parse", "cbor_parse",
        "gen_send", "gen_throw", "gen_close", "asyncgen_new",
        "builtin_type", "exception_push", "exception_pop", "type_of",
        "class_layout_version", "class_set_layout_version", "list_int_new",
    ):
        assert contracts[kind] == "owned_object", kind
        row = d10_audit_result.rows[kind]
        assert row.classifier_class == "OwnedValue", kind
        broken = replace(d10_audit_result, rows={kind: replace(row, classifier_class="TransparentAlias")})
        assert broken.dangerous()["owned_result_transparent_alias"] == [kind]
    for kind in (
        "dict_set", "dict_update_missing", "frame_home_load",
        "function_closure_bits", "guard_tag", "guard_type",
        "const_ellipsis", "const_not_implemented",
        "trace_enter_slot", "trace_exit", "trace_set_line", "frame_invocation_exit",
    ):
        assert contracts[kind] == "borrowed_object", kind
        assert kind not in d10_audit_result.owned_result_runtime_kinds
    assert contracts["alloc_class"] == "unpublished_object"
    assert contracts["int_as_i64"] == "raw_bits"
    assert contracts["print_newline"] == "void"
    assert d10_audit_result.dangerous()["owned_result_transparent_alias"] == []


def test_runtime_return_projection_rejects_conflicting_operation_selectors():
    from wasm_abi_gen.manifest import (
        WasmAbiManifestError, load_manifest, runtime_operation_return_specs,
    )

    data = load_manifest()
    data["const_op_policy"].append({"kind": "abs", "materializer_import": "ellipsis"})
    with pytest.raises(WasmAbiManifestError, match="conflicting runtime return contracts"):
        runtime_operation_return_specs(data)
