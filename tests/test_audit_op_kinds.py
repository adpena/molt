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

  1. is EMPTY on the current healthy tree (no false positives — in particular the
     no-result statement ops ``del_boundary`` / ``try_end`` are exempt),
  2. CATCHES a synthetic unrouted result-producing kind and NAMES it,
  3. correctly classifies result-producing vs no-result kinds from the
     ``lower_to_simple.rs`` ``fn lower_op`` ``out:`` dispositions.
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


def test_no_result_statement_kinds_are_exempt() -> None:
    """``del_boundary`` / ``try_end`` reach the native catch-all but are ignored
    there (``op.out.is_some()`` is false), so they must be classified non-result
    and excluded from D8 — otherwise D8 false-positives on the healthy tree."""
    res = AUDIT.run_audit()
    for kind in ("del_boundary", "try_end"):
        row = res.rows[kind]
        assert row.frontend_emits, kind
        assert row.mapper_maps, kind  # they DO map to an OpCode (DelBoundary/TryEnd)
        assert not row.produces_result, (
            f"{kind} must be non-result (lower_op emits it with no `out`)"
        )
        # No routing slice — yet exempt from D8 by virtue of being non-result.
        assert not row.native_routing_slice, kind


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


def test_d8_does_not_flag_unrouted_no_result_kind() -> None:
    """Removing a no-result statement kind from the native slices must NOT trip
    D8 — the catch-all ignores it (no result), so it needs no slice."""
    res = AUDIT.run_audit()
    live_slice = {k for k, r in res.rows.items() if r.native_routing_slice}
    # del_boundary/try_end have no slice anyway; force-remove to assert the
    # non-result exemption holds even when absent from every routing slice.
    broken = _synthetic_broken_result(live_slice - {"try_end", "del_boundary"})
    assert broken.dangerous()["native_codegen_gap"] == []
