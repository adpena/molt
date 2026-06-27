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
import sys
from dataclasses import replace
from pathlib import Path

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
