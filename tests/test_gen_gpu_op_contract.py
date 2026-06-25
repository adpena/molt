"""Sync + drift-catch guards for the tinygrad GPU op-contract FACT.

``runtime/molt-gpu/op_contract.toml`` is the generated, checked registry mapping
every molt ``PrimitiveOp`` to its disposition against the pinned upstream tinygrad
**0.13.0** op set (``tools/gen_gpu_op_contract.py``, doc 67 Phase 1, fact family
``gpu_op_contract``). These tests turn any drift into a test failure (the
``tests/test_gen_op_kinds.py`` / ``tests/test_gen_stringprep_tables.py`` pattern):

  1. The checked-in ``op_contract.toml`` is byte-identical to a fresh in-memory
     render (a forgotten regeneration fails here, not silently in production), and
     ``--check`` is idempotent.
  2. The reconciliation table EXACTLY covers ``PrimitiveOp::ALL`` from ops.rs.
  3. **The gate CATCHES drift**: feeding a mutated upstream op set / changed
     C-pattern / new ALU op makes ``reconcile()`` FAIL naming the drifted op — the
     core thesis of doc 67 (invisible drift becomes a RED gate). This is the
     synthetic-drift proof the deliverable requires.
  4. The §1.2.1 keystone divergences are reconciled with the correct disposition.

See ``docs/design/foundation/67_compat_tinygrad_dflash.md`` §1.2.1 and §3.1.
"""

from __future__ import annotations

import importlib.util
import sys
import tomllib
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
GEN = ROOT / "tools" / "gen_gpu_op_contract.py"
OUT = ROOT / "runtime/molt-gpu/op_contract.toml"


def _load_generator():
    spec = importlib.util.spec_from_file_location("molt_test_gen_gpu_op_contract", GEN)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_gen_gpu_op_contract"] = module
    spec.loader.exec_module(module)
    return module


# ---------------------------------------------------------------------------
# (1) Freshness / idempotence
# ---------------------------------------------------------------------------


def test_op_contract_is_in_sync() -> None:
    gen = _load_generator()
    model = gen.reconcile()
    rendered = gen.render_toml(model)
    checked_in = OUT.read_text(encoding="utf-8")
    assert checked_in == rendered, (
        f"{OUT.relative_to(ROOT)} is stale; run `python3 tools/gen_gpu_op_contract.py`."
    )


def test_check_mode_is_green() -> None:
    gen = _load_generator()
    assert gen.main(["--check"]) == 0


# ---------------------------------------------------------------------------
# (2) The reconciliation table covers PrimitiveOp::ALL exactly
# ---------------------------------------------------------------------------


def test_disposition_table_covers_primitive_op_all() -> None:
    gen = _load_generator()
    molt_ops = set(gen.parse_molt_primitive_ops())
    table_ops = set(gen.MOLT_OP_DISPOSITIONS)
    assert molt_ops == table_ops, (
        "disposition table must EXACTLY cover ops.rs::PrimitiveOp::ALL; "
        f"missing={sorted(molt_ops - table_ops)} extra={sorted(table_ops - molt_ops)}"
    )


# ---------------------------------------------------------------------------
# (3) DRIFT-CATCH PROOF — the deliverable's core requirement.
# Each case mutates a parsed-source input and asserts reconcile() FAILS naming
# the drifted op. Each test loads a fresh generator module (no shared state) and
# overrides the relevant parse_* function via monkeypatch.
# ---------------------------------------------------------------------------


def test_drift_caught_when_max_gains_code_for_op(monkeypatch) -> None:
    """If upstream gives MAX a code_for_op entry, the `rewrite` disposition for
    molt `Max` is now WRONG — the gate must catch it (reclassify as mapped)."""
    gen = _load_generator()
    real_code_for_op = gen.parse_code_for_op()
    drifted = dict(real_code_for_op)
    drifted["MAX"] = "max({a},{b})"  # upstream grew a renderer entry
    monkeypatch.setattr(gen, "parse_code_for_op", lambda: drifted)
    with pytest.raises(gen.OpContractError) as exc:
        gen.reconcile()
    msg = str(exc.value)
    assert "Max" in msg and "MAX" in msg and "code_for_op" in msg


def test_drift_caught_when_cmod_cpattern_changes(monkeypatch) -> None:
    """If upstream changes the CMOD C-pattern, the rendered contract changes; the
    byte-exact --check would go stale. Here we prove the *pattern* is sourced from
    upstream (not hardcoded) by mutating it and observing the rendered TOML move."""
    gen = _load_generator()
    real = gen.parse_code_for_op()
    drifted = dict(real)
    drifted["CMOD"] = "MOD_CHANGED({a},{b})"
    monkeypatch.setattr(gen, "parse_code_for_op", lambda: drifted)
    model = gen.reconcile()
    rendered = gen.render_toml(model)
    # The drifted pattern must surface in the regenerated contract (so --check
    # against the checked-in file would be RED). The checked-in file does NOT
    # contain it.
    assert "MOD_CHANGED({a},{b})" in rendered
    assert "MOD_CHANGED" not in OUT.read_text(encoding="utf-8")


def test_drift_caught_when_new_upstream_alu_op_appears(monkeypatch) -> None:
    """A NEW upstream GroupOp.ALU member that molt neither maps nor records a
    disposition for must FAIL as `unclassified` (the FDIV/POW-class drift). This
    is the check that would have flagged the §1.2.1 ops on day one."""
    gen = _load_generator()
    real_sets = gen.parse_group_op_sets()
    real_members = gen.parse_ops_enum()
    drifted_sets = dict(real_sets)
    # Inject a brand-new ALU member upstream that molt has no disposition for.
    drifted_sets["ALU"] = set(real_sets["ALU"]) | {"NEWALU"}
    monkeypatch.setattr(gen, "parse_group_op_sets", lambda: drifted_sets)
    monkeypatch.setattr(gen, "parse_ops_enum", lambda: real_members + ["NEWALU"])
    with pytest.raises(gen.OpContractError) as exc:
        gen.reconcile()
    msg = str(exc.value)
    assert "UNCLASSIFIED" in msg and "NEWALU" in msg


def test_drift_caught_when_mapped_upstream_op_disappears(monkeypatch) -> None:
    """If a molt op's claimed upstream Ops member vanishes from the pinned enum
    (an upstream rename/removal), the gate must catch the dangling claim."""
    gen = _load_generator()
    real_members = gen.parse_ops_enum()
    # Drop CDIV (which molt `Idiv` maps to) from the enum.
    drifted = [m for m in real_members if m != "CDIV"]
    monkeypatch.setattr(gen, "parse_ops_enum", lambda: drifted)
    with pytest.raises(gen.OpContractError) as exc:
        gen.reconcile()
    msg = str(exc.value)
    assert "CDIV" in msg and "NOT a member" in msg


def test_drift_caught_when_molt_op_added_without_disposition(monkeypatch) -> None:
    """A new molt PrimitiveOp without a reconciliation row must FAIL (symmetry:
    molt cannot grow an op the contract does not classify against upstream)."""
    gen = _load_generator()
    real = gen.parse_molt_primitive_ops()
    monkeypatch.setattr(gen, "parse_molt_primitive_ops", lambda: real + ["NewMoltOp"])
    with pytest.raises(gen.OpContractError) as exc:
        gen.reconcile()
    msg = str(exc.value)
    assert "NewMoltOp" in msg and "disposition table" in msg


def test_real_run_is_green_after_drift_tests() -> None:
    """Sanity: with no patches, reconcile() succeeds and matches the artifact.

    Proves the drift tests above did not leave global state mutated (the real
    run is GREEN). Ordering-independent because each loader gives a fresh module.
    """
    gen = _load_generator()
    model = gen.reconcile()
    assert gen.render_toml(model) == OUT.read_text(encoding="utf-8")


# ---------------------------------------------------------------------------
# (4) §1.2.1 keystone divergences are reconciled with the correct disposition
# ---------------------------------------------------------------------------


def test_keystone_1_2_1_divergences_reconciled() -> None:
    data = tomllib.loads(OUT.read_text(encoding="utf-8"))
    prims = {p["molt_op"]: p for p in data["primitive"]}
    uonly = {u["upstream_op"]: u for u in data["upstream_only_alu"]}
    nonp = {u["upstream_op"]: u for u in data["upstream_non_primitive"]}

    # CMOD/CDIV naming: molt Mod/Idiv map to upstream CMOD/CDIV (spelling drift).
    assert prims["Idiv"]["upstream_op"] == "CDIV"
    assert prims["Idiv"]["renderer_c_pattern"] == "({a}/{b})"
    assert prims["Mod"]["upstream_op"] == "CMOD"
    assert prims["Mod"]["renderer_c_pattern"] == "({a}%{b})"

    # MAX has no code_for_op entry — it is a REWRITE, not a renderer primitive.
    assert prims["Max"]["disposition"] == "rewrite"
    assert prims["Max"]["renderer_c_pattern"] == ""
    assert prims["Max"]["lowers_to"] == "WHERE(CMPLT(a, b), b, a)"

    # The new upstream ALU ops are classified (not silently absent).
    for op in ("FDIV", "POW", "FLOORDIV", "FLOORMOD", "MULACC"):
        assert uonly[op]["disposition"] == "composed", op
        assert uonly[op]["lowers_to"]
    assert uonly["THREEFRY"]["disposition"] == "not_yet_supported"
    assert uonly["THREEFRY"]["reason"]

    # WMMA is a classified non-primitive (tensor-core), not a silent omission.
    assert nonp["WMMA"]["scope"] == "tensor-core"


def test_contract_meta_pins_0_13_0() -> None:
    data = tomllib.loads(OUT.read_text(encoding="utf-8"))
    assert data["meta"]["pinned_tinygrad_version"] == "0.13.0"
    assert data["meta"]["molt_primitive_op_count"] == 26
