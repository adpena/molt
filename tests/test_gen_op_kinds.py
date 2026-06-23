"""Sync + coverage guards for the op-kind single-source-of-truth registry.

The registry (``runtime/molt-backend/src/tir/op_kinds.toml``) is the ONE table
the cross-component op-"kind"-string vocabulary lives in; ``tools/gen_op_kinds.py``
renders it into the backend Rust tables and the frontend Python constants. These
tests turn any drift into a test failure (the ``tests/test_gen_intrinsics.py``
pattern):

  1. The checked-in generated files are byte-identical to a fresh in-memory
     render (a forgotten regeneration fails here, not at runtime).
  2. The table MIRRORS the current Rust reality the audit tool extracts from
     source — the mapper arms, the three classifier sets, the per-OpCode effect
     oracle — so the table can never silently diverge from the code it generates.
  3. Every kind the FRONTEND can emit that maps to a first-class opcode is in the
     table's mapper (a new first-class frontend kind without a table row = red).

See ``docs/design/foundation/25_op_kind_registry.md`` and
``tools/audit_op_kinds.py``.
"""

from __future__ import annotations

import ast
import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace


ROOT = Path(__file__).resolve().parents[1]
GEN = ROOT / "tools" / "gen_op_kinds.py"
AUDIT = ROOT / "tools" / "audit_op_kinds.py"
OUT_RS = ROOT / "runtime/molt-backend/src/tir/op_kinds_generated.rs"
OUT_PY = ROOT / "src/molt/frontend/lowering/op_kinds_generated.py"


def _load(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # Register before exec so `@dataclass` / typing introspection can resolve the
    # module via `sys.modules[cls.__module__]` (audit_op_kinds defines dataclasses).
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _gen():
    return _load(GEN, "molt_test_gen_op_kinds")


def _audit():
    return _load(AUDIT, "molt_test_audit_op_kinds")


def _generated_arm_region(rendered: str, marker: str, next_prefix: str) -> str:
    start = rendered.index(marker)
    next_start = rendered.find(next_prefix, start + len(marker))
    if next_start == -1:
        return rendered[start:]
    return rendered[start:next_start]


# ---------------------------------------------------------------------------
# 1. Freshness: the checked-in generated files match a fresh render.
# ---------------------------------------------------------------------------


def test_generated_rs_is_in_sync() -> None:
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    checked_in = OUT_RS.read_bytes()
    assert checked_in == rendered.encode("utf-8"), (
        f"{OUT_RS.relative_to(ROOT)} is stale — run "
        "`python3 tools/gen_op_kinds.py` to regenerate from op_kinds.toml."
    )


def test_render_rs_rustfmt_uses_shared_memory_guard(monkeypatch) -> None:
    gen = _gen()
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        path = Path(cmd[-1])
        path.write_text("fn main() {}\n", encoding="utf-8", newline="\n")
        calls.append({"cmd": list(cmd), "path": path, **kwargs})
        return SimpleNamespace(
            returncode=0,
            stdout="",
            stderr="",
            check_returncode=lambda: None,
        )

    monkeypatch.setattr(
        gen.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    formatted = gen._rustfmt_rust_source("fn main(){}\n")

    assert formatted == "fn main() {}\n"
    assert len(calls) == 1
    call = calls[0]
    cmd = call["cmd"]
    assert isinstance(cmd, list)
    assert cmd[:3] == ["rustfmt", "--edition", "2024"]
    temp_path = call["path"]
    assert isinstance(temp_path, Path)
    assert temp_path.parent == ROOT / "tmp" / "gen_op_kinds"
    assert temp_path.suffix == ".rs"
    assert not temp_path.exists()
    assert call["prefix"] == "MOLT_GENERATOR"
    assert call["cwd"] == ROOT
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["timeout"] == 60.0


def test_generated_py_is_in_sync() -> None:
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_py(data)
    checked_in = OUT_PY.read_bytes()
    assert checked_in == rendered.encode("utf-8"), (
        f"{OUT_PY.relative_to(ROOT)} is stale — run "
        "`python3 tools/gen_op_kinds.py` to regenerate from op_kinds.toml."
    )


# ---------------------------------------------------------------------------
# 2. The table mirrors the Rust reality the audit extracts from source.
# ---------------------------------------------------------------------------


def test_generated_mapper_matches_table() -> None:
    """The GENERATED Rust mapper (`kind_to_opcode_table`) must recognize exactly
    the table's mapper spellings — pinning the registry⇄generated-Rust direction
    by parsing the generated file directly (the audit's Rust `match` parser)."""
    gen = _gen()
    audit = _audit()
    data = gen.load_table()

    table_spellings: set[str] = set()
    for row in data["kind"]:
        table_spellings.add(row["canonical"])
        table_spellings.update(row.get("aliases", []))

    gen_spellings = set(
        audit.extract_match_arms(OUT_RS, "kind_to_opcode_table", "match kind {")
    )
    assert gen_spellings == table_spellings, (
        "kind_to_opcode_table in the generated Rust drifted from op_kinds.toml: "
        f"gen-only={sorted(gen_spellings - table_spellings)} "
        f"table-only={sorted(table_spellings - gen_spellings)}"
    )


def test_generated_classifier_matches_table() -> None:
    """The GENERATED Rust classifier tables (`*_table` matches!) must contain
    exactly the table's flat classifier sets — parsed from the generated file."""
    gen = _gen()
    audit = _audit()
    data = gen.load_table()

    gen_fresh = set(
        audit.extract_matches_macro(OUT_RS, "copy_kind_mints_fresh_owned_ref_table")
    )
    gen_inert = set(
        audit.extract_matches_macro(OUT_RS, "copy_kind_is_inert_marker_table")
    )
    gen_transparent_alias = set(
        audit.extract_matches_macro(
            OUT_RS, "copy_kind_is_explicit_transparent_alias_table"
        )
    )
    gen_no_heap = set(
        audit.extract_matches_macro(OUT_RS, "copy_kind_is_explicit_no_heap_move_table")
    )
    assert gen_fresh == set(data["classifier_fresh_value"]), (
        "generated fresh-value table drifted from classifier_fresh_value"
    )
    assert gen_inert == set(data["classifier_inert_marker"]), (
        "generated inert-marker table drifted from classifier_inert_marker"
    )
    assert gen_transparent_alias == set(data["classifier_transparent_alias"]), (
        "generated transparent-alias table drifted from classifier_transparent_alias"
    )
    assert gen_no_heap == set(data["classifier_no_heap_move"]), (
        "generated no-heap-move table drifted from classifier_no_heap_move"
    )


def test_removed_absorbing_constructor_helper_stays_removed() -> None:
    """Result absorption is owned by the generated result-ownership tables.

    The old per-spelling ``copy_kind_absorbs_elements_table`` had no table source
    data and no consumer; keeping it around reintroduced a compiler warning and a
    second apparent authority for finalizer-sensitive container ownership. If
    this assertion fails, wire the new fact through the live
    ``kind_result_absorbs_operand_ownership_table`` authority instead.
    """
    generated = OUT_RS.read_text(encoding="utf-8")
    assert "copy_kind_absorbs_elements_table" not in generated


def test_selected_operand_result_contract_covers_python_boolops() -> None:
    """Python `and`/`or` return one operand, not a fresh value. Backends must
    retain the selected borrowed operand whenever that selected value is bound as
    an owned boxed result."""
    gen = _gen()
    data = gen.load_table()
    selected_owner = {
        row["name"]
        for row in data["opcode"]
        if row.get("result_mints_owned_selected_operand", False)
    }
    assert selected_owner == {"And", "Or"}


def test_explicit_release_operand_contract_covers_python_release_ops() -> None:
    """Python lifetime release boundaries live in the generated op-kind table."""
    gen = _gen()
    data = gen.load_table()
    release_rows = {
        row["opcode"]: row["operand"]
        for row in data.get("explicit_release_operand", [])
    }
    assert release_rows == {"DecRef": "all", "DeleteVar": 1}

    rendered = gen.render_rs(data)
    selected_block = rendered.split(
        "fn opcode_result_mints_owned_selected_operand_table"
    )[1].split("fn kind_result_mints_owned_selected_operand_table")[0]
    assert "OpCode::And" in selected_block
    assert "OpCode::Or" in selected_block
    assert "kind_to_opcode_table(kind)" in rendered


def test_explicit_release_operand_rejects_out_of_range_numeric_operand() -> None:
    gen = _gen()
    data = gen.load_table()
    opcode_rows = {row["name"]: row for row in data["opcode"]}

    mutated = json.loads(json.dumps(data))
    for row in mutated["explicit_release_operand"]:
        if row["opcode"] == "DeleteVar":
            row["operand"] = 2
            break

    try:
        gen._validate_explicit_release_operands(mutated, opcode_rows)
    except gen.OpKindTableError as exc:
        assert "out of range" in str(exc)
    else:  # pragma: no cover - explicit fail branch for pytest output clarity
        raise AssertionError("out-of-range explicit_release_operand row was accepted")


def test_explicit_release_operand_numeric_requires_fixed_opcode_arity() -> None:
    gen = _gen()
    data = gen.load_table()
    mutated = json.loads(json.dumps(data))
    for row in mutated["opcode"]:
        if row["name"] == "DeleteVar":
            row["operand_ownership"] = "all_borrowed"
            break

    opcode_rows = {row["name"]: row for row in mutated["opcode"]}
    try:
        gen._validate_explicit_release_operands(mutated, opcode_rows)
    except gen.OpKindTableError as exc:
        assert "fixed per-position operand_ownership list" in str(exc)
    else:  # pragma: no cover - explicit fail branch for pytest output clarity
        raise AssertionError("numeric explicit_release_operand without fixed arity was accepted")


def test_audit_sources_backend_vocab_from_registry() -> None:
    """The audit tool must source the backend mapper + classifier vocabularies
    from the registry (post phase-2), so its drift matrix compares the FRONTEND
    emitter against the registry. Verify the audit's extracted sets equal the
    table's (the audit reads op_kinds.toml, not the now-delegating Rust)."""
    gen = _gen()
    audit = _audit()
    data = gen.load_table()
    res = audit.run_audit()

    table_spellings: set[str] = set()
    for row in data["kind"]:
        table_spellings.add(row["canonical"])
        table_spellings.update(row.get("aliases", []))

    assert res.mapper_kinds == table_spellings
    assert res.fresh_value == set(data["classifier_fresh_value"])
    assert res.inert_marker == set(data["classifier_inert_marker"])
    assert res.transparent_alias == set(data["classifier_transparent_alias"])
    assert res.no_heap_move == set(data["classifier_no_heap_move"])
    assert set(res.fresh_value_prefixes) == set(data["classifier_fresh_value_prefixes"])


def test_guarded_field_init_has_one_wire_spelling() -> None:
    """`GUARDED_SETATTR_INIT` has one cross-component wire spelling.

    `guarded_field_set_init` was a dead registry spelling while the frontend and
    SimpleIR backends used `guarded_field_init`. Pin the chosen spelling across
    the registry, generated mapper tables, frontend audit, LLVM StoreAttr
    lowering text, native/WASM arm extraction, and the dangerous-cell baseline.
    """
    gen = _gen()
    audit = _audit()
    data = gen.load_table()
    current = "guarded_field_init"
    rejected = "guarded_field_set_init"

    store_attr = next(row for row in data["kind"] if row["canonical"] == "set_attr")
    aliases = set(store_attr.get("aliases", []))
    assert current in aliases
    assert rejected not in aliases

    generated_rs = OUT_RS.read_text(encoding="utf-8")
    generated_py = OUT_PY.read_text(encoding="utf-8")
    assert f'"{current}"' in generated_rs
    assert f'"{rejected}"' not in generated_rs
    assert f'"{current}"' in generated_py
    assert f'"{rejected}"' not in generated_py

    res = audit.run_audit()
    assert current in res.frontend.all
    assert rejected not in res.frontend.all
    assert current in res.mapper_kinds
    assert rejected not in res.mapper_kinds
    assert res.rows[current].frontend_emits
    assert res.rows[current].mapper_maps
    assert res.rows[current].native_arm
    assert res.rows[current].wasm_arm

    llvm_lowering = (
        ROOT / "runtime/molt-backend/src/llvm_backend/lowering.rs"
    ).read_text(encoding="utf-8")
    assert f'Some("{current}")' in llvm_lowering
    assert f'Some("{rejected}")' not in llvm_lowering

    current_guarded_danger = {
        kind
        for kinds in res.dangerous().values()
        for kind in kinds
        if kind.startswith("guarded_field")
    }
    baseline = json.loads((ROOT / "tools/op_kinds_baseline.json").read_text(encoding="utf-8"))
    baseline_guarded_danger = {
        kind
        for kinds in baseline["dangerous"].values()
        for kind in kinds
        if kind.startswith("guarded_field")
    }
    assert current_guarded_danger == set()
    assert baseline_guarded_danger == set()


def test_effects_rs_delegates_to_generated_tables() -> None:
    """The effect oracle in effects.rs must DELEGATE to the generated tables (no
    hand-maintained `matches!` of opcodes), and the generated tables must embed a
    correct exhaustive arm per opcode for may_throw / side_effecting / purity.
    This is the structural kill for the matches!-default-false trap: the source of
    truth is the table, and a new opcode cannot compile without a row."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    effects = (
        ROOT / "runtime/molt-backend/src/tir/passes/effects.rs"
    ).read_text(encoding="utf-8")

    # effects.rs delegates rather than hand-lists.
    for fn, table in (
        ("opcode_may_throw", "opcode_may_throw_table"),
        ("opcode_is_side_effecting", "opcode_is_side_effecting_table"),
        ("opcode_effects", "opcode_purity_table"),
    ):
        assert f"op_kinds_generated::{table}" in effects, (
            f"effects.rs {fn} must delegate to the generated {table}"
        )

    # The generated may_throw / side_effecting tables embed the table's booleans.
    may_block = rendered.split("fn opcode_may_throw_table")[1].split(
        "fn opcode_is_side_effecting_table"
    )[0]
    side_block = rendered.split("fn opcode_is_side_effecting_table")[1].split(
        "fn opcode_purity_table"
    )[0]
    purity_block = rendered.split("fn opcode_purity_table")[1]
    purity_variant = {
        "pure": "OpcodePurity::Pure",
        "pure_may_throw": "OpcodePurity::PureMayThrow",
        "impure": "OpcodePurity::Impure",
    }
    for row in data["opcode"]:
        name = row["name"]
        mt = "true" if row["may_throw"] else "false"
        se = "true" if row["side_effecting"] else "false"
        assert f"OpCode::{name} => {mt}," in may_block, (
            f"opcode_may_throw arm for {name} missing/incorrect"
        )
        assert f"OpCode::{name} => {se}," in side_block, (
            f"opcode_is_side_effecting arm for {name} missing/incorrect"
        )
        assert f"OpCode::{name} => {purity_variant[row['purity']]}," in purity_block, (
            f"opcode_purity arm for {name} missing/incorrect"
        )


def test_alias_barrier_predicates_delegate_to_generated_tables() -> None:
    """Alias-analysis opcode-only barrier facts belong in the generated registry.

    The consumer may layer operand/root checks on top, but the RC and arbitrary
    heap opcode sets must not live as hand-maintained `matches!` lists in
    alias_analysis.rs.
    """
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    alias = (
        ROOT / "runtime/molt-backend/src/tir/passes/alias_analysis.rs"
    ).read_text(encoding="utf-8")

    expected_rc = {
        "Call",
        "CallBuiltin",
        "CallMethod",
        "ChanRecvYield",
        "ChanSendYield",
        "CheckException",
        "ClosureLoad",
        "ClosureStore",
        "Raise",
        "StateSwitch",
        "StateTransition",
        "StateYield",
        "StoreAttr",
        "StoreIndex",
        "TryStart",
    }
    expected_heap = {
        "Call",
        "CallBuiltin",
        "CallMethod",
        "ChanRecvYield",
        "ChanSendYield",
        "ClosureStore",
        "DelAttr",
        "DelIndex",
        "Free",
        "ModuleCacheDel",
        "ModuleCacheSet",
        "ModuleDelGlobal",
        "ModuleDelGlobalIfPresent",
        "ModuleSetAttr",
        "Raise",
        "StateSwitch",
        "StateTransition",
        "StateYield",
        "StoreAttr",
        "StoreIndex",
        "Yield",
        "YieldFrom",
    }
    assert set(data["alias_rc_barrier_opcodes"]) == expected_rc
    assert set(data["alias_heap_barrier_opcodes"]) == expected_heap

    rc_block = rendered.split("fn opcode_is_alias_rc_barrier_table")[1].split(
        "fn opcode_is_alias_heap_barrier_table"
    )[0]
    heap_block = rendered.split("fn opcode_is_alias_heap_barrier_table")[1].split(
        "enum CanonicalizeCommutativeDomain"
    )[0]
    for opcode in expected_rc:
        assert f"OpCode::{opcode} => true," in rc_block
    assert "OpCode::Add => false," in rc_block
    for opcode in expected_heap:
        assert f"OpCode::{opcode} => true," in heap_block
    assert "OpCode::ClosureLoad => false," in heap_block

    for fn_name, table_name in (
        ("fn opcode_is_rc_barrier(", "opcode_is_alias_rc_barrier_table"),
        ("fn opcode_is_heap_barrier(", "opcode_is_alias_heap_barrier_table"),
    ):
        start = alias.index(fn_name)
        brace = alias.index("{", start)
        depth = 0
        end = brace
        for i in range(brace, len(alias)):
            if alias[i] == "{":
                depth += 1
            elif alias[i] == "}":
                depth -= 1
                if depth == 0:
                    end = i + 1
                    break
        body = alias[start:end]
        assert table_name in body
        assert "matches!" not in body


def test_deforestation_fusion_barriers_delegate_to_generated_table() -> None:
    """Iterator-chain fusion barriers belong to the op-kind registry.

    Deforestation may wrap the generated predicate for readability, but it must
    not grow a second handwritten opcode list that can drift from
    `fusion_barrier_opcodes`.
    """
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    deforestation = (
        ROOT / "runtime/molt-backend/src/tir/passes/deforestation.rs"
    ).read_text(encoding="utf-8")

    expected = {
        "Call",
        "CallBuiltin",
        "CallMethod",
        "ChanRecvYield",
        "ChanSendYield",
        "ClosureLoad",
        "ClosureStore",
        "DelAttr",
        "DelIndex",
        "Import",
        "ImportFrom",
        "Raise",
        "StateSwitch",
        "StateTransition",
        "StateYield",
        "StoreAttr",
        "StoreIndex",
        "Yield",
        "YieldFrom",
    }
    assert set(data["fusion_barrier_opcodes"]) == expected

    table_block = rendered.split("fn opcode_is_fusion_barrier_table")[1].split(
        "enum AliasTypedSlotRole"
    )[0]
    for opcode in expected:
        assert f"OpCode::{opcode} => true," in table_block
    for opcode in {"BuildList", "Index", "LoadAttr", "ObjectNewBound"}:
        assert f"OpCode::{opcode} => false," in table_block

    assert (
        "use crate::tir::op_kinds_generated::opcode_is_fusion_barrier_table;"
        in deforestation
    )
    start = deforestation.index("fn is_fusion_barrier(")
    brace = deforestation.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(deforestation)):
        if deforestation[i] == "{":
            depth += 1
        elif deforestation[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = deforestation[start:end]
    assert "opcode_is_fusion_barrier_table(opcode)" in body
    assert "matches!" not in body
    assert "OpCode::" not in body


def test_i64_zero_divisor_guards_delegate_to_generated_table() -> None:
    """Raw-i64 zero-divisor proof requirements have one opcode authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    lower_to_lir = (ROOT / "runtime/molt-backend/src/tir/lower_to_lir.rs").read_text(
        encoding="utf-8"
    )
    check_exception = (
        ROOT / "runtime/molt-backend/src/tir/passes/check_exception_elim.rs"
    ).read_text(encoding="utf-8")

    expected = {"Div", "FloorDiv", "Mod"}
    assert set(data["i64_zero_divisor_guard_opcodes"]) == expected

    table_block = rendered.split(
        "fn opcode_requires_i64_zero_divisor_guard_table"
    )[1].split("enum AliasTypedSlotRole")[0]
    for opcode in expected:
        assert f"OpCode::{opcode} => true," in table_block
    for opcode in {"Add", "Mul", "Pow"}:
        assert f"OpCode::{opcode} => false," in table_block

    table_name = "opcode_requires_i64_zero_divisor_guard_table"
    assert table_name in lower_to_lir
    assert table_name in check_exception

    for source, fn_name in (
        (lower_to_lir, "fn lower_op("),
        (check_exception, "fn op_may_raise("),
    ):
        start = source.index(fn_name)
        brace = source.index("{", start)
        depth = 0
        end = brace
        for i in range(brace, len(source)):
            if source[i] == "{":
                depth += 1
            elif source[i] == "}":
                depth -= 1
                if depth == 0:
                    end = i + 1
                    break
        body = source[start:end]
        assert table_name in body
        assert "OpCode::Div | OpCode::FloorDiv | OpCode::Mod" not in body


def test_alias_slot_observation_delegates_to_generated_table() -> None:
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    alias = (
        ROOT / "runtime/molt-backend/src/tir/passes/alias_analysis.rs"
    ).read_text(encoding="utf-8")

    expected_direct = {
        "AllocTask",
        "BuildDict",
        "BuildList",
        "BuildSet",
        "BuildSlice",
        "BuildTuple",
        "Call",
        "CallBuiltin",
        "CallMethod",
        "Index",
        "LoadAttr",
        "Raise",
        "StoreIndex",
        "Yield",
        "YieldFrom",
    }
    expected_typed_store = {"StoreAttr"}
    expected_transparent = {"Copy", "TypeGuard"}
    expected_never = {"CheckException", "DecRef", "IncRef"}
    assert set(data["alias_slot_direct_observer_opcodes"]) == expected_direct
    assert set(data["alias_slot_typed_store_opcodes"]) == expected_typed_store
    assert set(data["alias_transparent_copy_opcodes"]) == {"Copy"}
    assert set(data["alias_transparent_type_guard_opcodes"]) == {"TypeGuard"}
    assert set(data["alias_slot_never_observer_opcodes"]) == expected_never

    block = rendered.split("fn opcode_alias_slot_observation_table")[1].split(
        "enum CanonicalizeCommutativeDomain"
    )[0]
    for opcode in expected_direct:
        assert f"OpCode::{opcode} => AliasSlotObservation::DirectObserver," in block
    assert "OpCode::StoreAttr => AliasSlotObservation::TypedSlotStore," in block
    for opcode in expected_transparent:
        assert f"OpCode::{opcode} => AliasSlotObservation::TransparentAlias," in block
    for opcode in expected_never:
        assert f"OpCode::{opcode} => AliasSlotObservation::NeverObserver," in block
    assert "OpCode::Add => AliasSlotObservation::ConservativeObserver," in block

    start = alias.index("pub fn may_observe_slot(")
    brace = alias.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(alias)):
        if alias[i] == "{":
            depth += 1
        elif alias[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = alias[start:end]
    assert "aliasing_op_may_observe_slot" in body
    assert "OpCode::LoadAttr" not in body
    assert "OpCode::StoreAttr" not in body
    assert "match op.opcode" not in body


def test_alias_memory_region_delegates_to_generated_table() -> None:
    """MemRegion opcode classification is a generated opcode fact.

    The table owns the opcode-to-region class. Alias analysis may still refine
    typed slots and Copy with live operands/attrs, but it must not carry a
    private opcode dispatch beside the registry.
    """
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    alias = (
        ROOT / "runtime/molt-backend/src/tir/passes/alias_analysis.rs"
    ).read_text(encoding="utf-8")

    expected_scalar = {
        "Add",
        "And",
        "BitAnd",
        "BitNot",
        "BitOr",
        "BitXor",
        "Bool",
        "BoxVal",
        "BuildSlice",
        "CheckException",
        "CheckedAdd",
        "ConstBigInt",
        "ConstBool",
        "ConstBytes",
        "ConstFloat",
        "ConstInt",
        "ConstNone",
        "ConstStr",
        "Div",
        "Eq",
        "ExceptionPending",
        "FloorDiv",
        "FunctionDefaultsVersion",
        "Ge",
        "Gt",
        "InplaceAdd",
        "InplaceMul",
        "InplaceSub",
        "Is",
        "IsNot",
        "Le",
        "Lt",
        "Mod",
        "Mul",
        "Ne",
        "Neg",
        "Not",
        "Or",
        "Pos",
        "Pow",
        "Shl",
        "Shr",
        "Sub",
        "TypeGuard",
        "UnboxVal",
    }
    expected_typed_slot_load = {"LoadAttr"}
    expected_typed_slot_store = {"StoreAttr"}
    expected_typed_slot = expected_typed_slot_load | expected_typed_slot_store
    expected_copy = {"Copy"}
    expected_container = {"DelIndex", "Index", "StoreIndex"}
    expected_module = {
        "ModuleCacheDel",
        "ModuleCacheGet",
        "ModuleCacheSet",
        "ModuleDelGlobal",
        "ModuleDelGlobalIfPresent",
        "ModuleGetAttr",
        "ModuleGetGlobal",
        "ModuleGetName",
        "ModuleImportFrom",
        "ModuleSetAttr",
    }
    assert set(data["alias_memory_inert_opcodes"]) == expected_scalar
    assert set(data["alias_typed_slot_load_opcodes"]) == expected_typed_slot_load
    assert set(data["alias_typed_slot_store_opcodes"]) == expected_typed_slot_store
    assert set(data["alias_region_copy_refinement_opcodes"]) == expected_copy
    assert set(data["alias_region_container_element_opcodes"]) == expected_container
    assert set(data["alias_region_module_dict_opcodes"]) == expected_module

    typed_slot_block = rendered.split("fn opcode_alias_typed_slot_role_table")[1].split(
        "enum AliasTransparentAliasRole"
    )[0]
    assert "OpCode::LoadAttr => AliasTypedSlotRole::Load," in typed_slot_block
    assert "OpCode::StoreAttr => AliasTypedSlotRole::Store," in typed_slot_block
    assert "OpCode::Copy => AliasTypedSlotRole::NotTypedSlot," in typed_slot_block

    transparent_block = rendered.split(
        "fn opcode_alias_transparent_alias_role_table"
    )[1].split("enum AliasMemoryRegionClass")[0]
    assert (
        "OpCode::TypeGuard => AliasTransparentAliasRole::TypeGuard,"
        in transparent_block
    )
    assert "OpCode::Copy => AliasTransparentAliasRole::Copy," in transparent_block
    assert (
        "OpCode::LoadAttr => AliasTransparentAliasRole::NotTransparentAlias,"
        in transparent_block
    )

    block = rendered.split("fn opcode_alias_memory_region_table")[1].split(
        "fn opcode_alias_slot_observation_table"
    )[0]
    for opcode in expected_scalar:
        assert f"OpCode::{opcode} => AliasMemoryRegionClass::ScalarRegister," in block
    for opcode in expected_typed_slot:
        assert f"OpCode::{opcode} => AliasMemoryRegionClass::TypedSlotAttr," in block
    assert "OpCode::Copy => AliasMemoryRegionClass::CopyRefinement," in block
    for opcode in expected_container:
        assert f"OpCode::{opcode} => AliasMemoryRegionClass::ContainerElement," in block
    for opcode in expected_module:
        assert f"OpCode::{opcode} => AliasMemoryRegionClass::ModuleDict," in block
    for opcode in {"Alloc", "Call", "Yield"}:
        assert f"OpCode::{opcode} => AliasMemoryRegionClass::GenericHeap," in block

    start = alias.index("pub fn region_of(")
    brace = alias.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(alias)):
        if alias[i] == "{":
            depth += 1
        elif alias[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = alias[start:end]
    assert "opcode_alias_memory_region_table" in body
    assert "match op.opcode" not in body
    assert "OpCode::" not in body

    transparent_start = alias.index("fn transparent_alias_root(")
    transparent_end = alias.index("// Typed-slot store helpers", transparent_start)
    transparent_body = alias[transparent_start:transparent_end]
    assert "opcode_alias_transparent_alias_role_table" in transparent_body
    assert "match op.opcode" not in transparent_body
    assert "OpCode::" not in transparent_body

    typed_start = alias.index("fn typed_slot_field_kind(")
    typed_end = alias.index("fn typed_slot_obj_offset(", typed_start)
    typed_body = alias[typed_start:typed_end]
    assert "opcode_alias_typed_slot_role_table" in typed_body
    assert "match op.opcode" not in typed_body
    assert "OpCode::" not in typed_body


def test_opcode_fact_set_validation_rejects_unknown_opcode() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}
    mutated = json.loads(json.dumps(data))
    mutated["alias_heap_barrier_opcodes"].append("StoreIndx")
    try:
        gen._validate_opcode_fact_set(mutated, "alias_heap_barrier_opcodes", opcodes)
    except gen.OpKindTableError as e:
        assert "StoreIndx" in str(e)
    else:
        raise AssertionError("unknown opcode fact-set member was accepted")

    typed_role_overlap = json.loads(json.dumps(data))
    typed_role_overlap["alias_typed_slot_store_opcodes"].append("LoadAttr")
    try:
        gen._validate_alias_opcode_role_sets(
            typed_role_overlap,
            gen._ALIAS_TYPED_SLOT_ROLE_SETS,
            "alias typed-slot role",
        )
    except gen.OpKindTableError as e:
        assert "LoadAttr" in str(e)
    else:
        raise AssertionError("overlapping alias typed-slot role was accepted")

    transparent_role_overlap = json.loads(json.dumps(data))
    transparent_role_overlap["alias_transparent_copy_opcodes"].append("TypeGuard")
    try:
        gen._validate_alias_opcode_role_sets(
            transparent_role_overlap,
            gen._ALIAS_TRANSPARENT_ALIAS_ROLE_SETS,
            "alias transparent-alias role",
        )
    except gen.OpKindTableError as e:
        assert "TypeGuard" in str(e)
    else:
        raise AssertionError("overlapping alias transparent-alias role was accepted")

    region_overlap = json.loads(json.dumps(data))
    region_overlap["alias_region_module_dict_opcodes"].append("LoadAttr")
    try:
        gen._validate_alias_memory_region_sets(region_overlap)
    except gen.OpKindTableError as e:
        assert "LoadAttr" in str(e)
    else:
        raise AssertionError("overlapping alias memory-region class was accepted")

    overlap = json.loads(json.dumps(data))
    overlap["alias_slot_never_observer_opcodes"].append("LoadAttr")
    try:
        gen._validate_alias_slot_observation_sets(overlap)
    except gen.OpKindTableError as e:
        assert "LoadAttr" in str(e)
    else:
        raise AssertionError("overlapping alias slot observation class was accepted")


def test_canonicalize_delegates_opcode_facts_to_generated_tables() -> None:
    """Canonicalize must not carry private OpCode lists beside the registry.

    The generated table owns opcode-level algebraic facts; canonicalize.rs only
    applies the live operand-type safety predicate and performs the rewrite.
    """
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    canonicalize = (
        ROOT / "runtime/molt-backend/src/tir/passes/canonicalize.rs"
    ).read_text(encoding="utf-8")
    check_exception = (
        ROOT / "runtime/molt-backend/src/tir/passes/check_exception_elim.rs"
    ).read_text(encoding="utf-8")

    assert "fn is_commutative" not in canonicalize
    assert "fn swap_comparison" not in canonicalize
    assert "opcode_canonicalize_commutative_domain_table" in canonicalize
    assert "opcode_literal_payload_kind_table" in canonicalize
    assert "opcode_literal_payload_kind_table" in check_exception
    assert "opcode_swapped_comparison_for_canonicalize_table" in canonicalize
    assert "opcode_canonicalize_binary_rules_table" in canonicalize
    assert "OpCode::Add | OpCode::InplaceAdd" not in canonicalize
    assert "OpCode::ConstInt =>" not in canonicalize
    assert "OpCode::ConstInt =>" not in check_exception
    assert "OpCode::And if" not in canonicalize

    expected_literals = {
        "ConstInt": "int",
        "ConstBool": "bool",
    }
    expected_domains = {
        "Add": "numeric",
        "Mul": "numeric",
        "BitAnd": "i64",
        "BitOr": "i64",
        "BitXor": "i64",
        "Eq": "unboxed_scalar",
        "Ne": "unboxed_scalar",
    }
    expected_swaps = {"Lt": "Gt", "Gt": "Lt", "Le": "Ge", "Ge": "Le"}
    assert {
        row["opcode"]: row["domain"]
        for row in data["canonicalize_commutative_reorder"]
    } == expected_domains
    assert {
        row["opcode"]: row["swapped"]
        for row in data["canonicalize_swapped_comparison"]
    } == expected_swaps
    assert {
        row["opcode"]: row["literal"]
        for row in data["literal_payload_opcodes"]
    } == expected_literals
    assert len(data["canonicalize_binary_rules"]) == 28
    assert data["canonicalize_binary_rules"][0] == {
        "opcode": "Add",
        "predicate": "rhs_int",
        "value": 0,
        "type_guard": "lhs_i64",
        "action": "copy_lhs",
    }
    assert data["canonicalize_binary_rules"][-1] == {
        "opcode": "Or",
        "predicate": "lhs_bool",
        "value": True,
        "type_guard": "none",
        "action": "const_bool",
        "result": True,
    }

    literal_block = rendered.split(
        "fn opcode_literal_payload_kind_table"
    )[1].split("fn opcode_canonicalize_commutative_domain_table")[0]
    literal_variant = {
        "int": "LiteralPayloadKind::Int",
        "bool": "LiteralPayloadKind::Bool",
    }
    for opcode, literal in expected_literals.items():
        assert (
            f"OpCode::{opcode} => Some({literal_variant[literal]}),"
            in literal_block
        )
    assert "OpCode::ConstNone => None," in literal_block

    variant = {
        "numeric": "CanonicalizeCommutativeDomain::Numeric",
        "i64": "CanonicalizeCommutativeDomain::I64",
        "unboxed_scalar": "CanonicalizeCommutativeDomain::UnboxedScalar",
    }
    domain_block = rendered.split(
        "fn opcode_canonicalize_commutative_domain_table"
    )[1].split("fn opcode_swapped_comparison_for_canonicalize_table")[0]
    for opcode, domain in expected_domains.items():
        assert f"OpCode::{opcode} => Some({variant[domain]})," in domain_block
    assert "OpCode::Sub => None," in domain_block

    swap_block = rendered.split(
        "fn opcode_swapped_comparison_for_canonicalize_table"
    )[1].split("enum OperandOwnership")[0]
    for opcode, swapped in expected_swaps.items():
        assert f"OpCode::{opcode} => Some(OpCode::{swapped})," in swap_block
    assert "OpCode::Eq => None," in swap_block

    binary_block = rendered.split("fn opcode_canonicalize_binary_rules_table")[1].split(
        "enum OperandOwnership"
    )[0]
    assert "CanonicalizeBinaryPredicate::IntConst" in rendered
    assert "CanonicalizeBinaryAction::ConstBool(true)" in rendered
    assert "OpCode::Add => CANONICALIZE_BINARY_RULES_ADD," in binary_block
    assert "OpCode::And => CANONICALIZE_BINARY_RULES_AND," in binary_block
    assert "OpCode::Eq => &[]," in binary_block


def test_literal_payload_fact_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_literal = json.loads(json.dumps(data))
    bad_literal["literal_payload_opcodes"][0]["literal"] = "float"
    try:
        gen._validate_literal_payload_facts(bad_literal, opcodes)
    except gen.OpKindTableError as e:
        assert "literal" in str(e)
    else:
        raise AssertionError("bad literal payload kind was accepted")


def test_canonicalize_fact_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_domain = json.loads(json.dumps(data))
    bad_domain["canonicalize_commutative_reorder"][0]["domain"] = "boxed"
    try:
        gen._validate_canonicalize_facts(bad_domain, opcodes)
    except gen.OpKindTableError as e:
        assert "domain" in str(e)
    else:
        raise AssertionError("bad canonicalize commutative domain was accepted")

    asymmetric_swap = json.loads(json.dumps(data))
    asymmetric_swap["canonicalize_swapped_comparison"] = [
        {"opcode": "Lt", "swapped": "Gt"},
    ]
    try:
        gen._validate_canonicalize_facts(asymmetric_swap, opcodes)
    except gen.OpKindTableError as e:
        assert "symmetric" in str(e)
    else:
        raise AssertionError("asymmetric canonicalize comparison swap was accepted")

    bad_binary_predicate = json.loads(json.dumps(data))
    bad_binary_predicate["canonicalize_binary_rules"][0]["predicate"] = "rhs_float"
    try:
        gen._validate_canonicalize_facts(bad_binary_predicate, opcodes)
    except gen.OpKindTableError as e:
        assert "predicate" in str(e)
    else:
        raise AssertionError("bad canonicalize binary predicate was accepted")

    bad_binary_result = json.loads(json.dumps(data))
    bad_binary_result["canonicalize_binary_rules"][-1]["result"] = 1
    try:
        gen._validate_canonicalize_facts(bad_binary_result, opcodes)
    except gen.OpKindTableError as e:
        assert "result" in str(e)
    else:
        raise AssertionError("bad canonicalize binary result type was accepted")


def test_opcode_effects_exhaustive_over_enum() -> None:
    """The effect table must cover EVERY OpCode variant — the exhaustiveness that
    kills the matches!-default-false trap. Cross-check the table's opcode names
    against the OpCode enum declared in ops.rs."""
    import re

    gen = _gen()
    data = gen.load_table()
    table_names = [row["name"] for row in data["opcode"]]
    assert len(table_names) == len(set(table_names)), "duplicate opcode rows"

    src = (ROOT / "runtime/molt-backend/src/tir/ops.rs").read_text(
        encoding="utf-8"
    )
    m = re.search(r"pub enum OpCode \{(.*?)\n\}", src, re.S)
    assert m is not None
    enum_variants = []
    for line in m.group(1).splitlines():
        s = line.strip()
        if not s or s.startswith(("//", "/*", "*", "#[")):
            continue
        vm = re.match(r"([A-Z]\w*)\s*,?\s*$", s)
        if vm:
            enum_variants.append(vm.group(1))

    assert set(table_names) == set(enum_variants), (
        "op_kinds.toml [[opcode]] rows must be EXACTLY the OpCode enum variants; "
        f"table-only={sorted(set(table_names) - set(enum_variants))} "
        f"enum-only={sorted(set(enum_variants) - set(table_names))}"
    )


# ---------------------------------------------------------------------------
# 2b. The frontend op.kind tables (RAISING / CHECK_EXCEPTION skip / binary-op),
#     absorbed from src/molt/frontend/__init__.py (task #44, F2a). The generated
#     Python constants must equal the table, and the table's cross-checks against
#     the may_throw oracle + ast.operator exhaustiveness must hold.
# ---------------------------------------------------------------------------


def _load_generated_py():
    return _load(OUT_PY, "molt_test_op_kinds_generated_py")


def test_frontend_raising_kinds_match_table() -> None:
    """RAISING_KIND_NAMES in the generated Python equals the table's
    [[frontend_raising_kind]] rows, and every opcode-mapped row maps to a
    may_throw OpCode (the frontend⇄backend dual-oracle drift kill)."""
    gen = _gen()
    data = gen.load_table()
    py = _load_generated_py()

    table_kinds = {row["kind"] for row in data["frontend_raising_kind"]}
    assert py.RAISING_KIND_NAMES == table_kinds, (
        "RAISING_KIND_NAMES drifted from [[frontend_raising_kind]]: "
        f"gen-only={sorted(py.RAISING_KIND_NAMES - table_kinds)} "
        f"table-only={sorted(table_kinds - py.RAISING_KIND_NAMES)}"
    )

    may_throw_ops = {r["name"] for r in data["opcode"] if r["may_throw"]}
    for row in data["frontend_raising_kind"]:
        # Exactly one of opcode / reason (the generator enforces this; assert the
        # opcode-mapped rows are genuinely may_throw — the canonical oracle).
        assert ("opcode" in row) != ("reason" in row), row
        if "opcode" in row:
            assert row["opcode"] in may_throw_ops, (
                f"frontend_raising_kind {row['kind']} maps to non-may_throw opcode "
                f"{row['opcode']}"
            )


def test_frontend_check_exception_skip_kinds_match_table() -> None:
    """CHECK_EXCEPTION_SKIP_KINDS equals the table's
    [[frontend_check_exception_skip]] rows, and every opcode-backed row is either
    may_throw=false OR justified control_flow=true (skipping a may_throw op's
    CHECK_EXCEPTION without justification would drop the exception edge)."""
    gen = _gen()
    data = gen.load_table()
    py = _load_generated_py()

    table_kinds = {row["kind"] for row in data["frontend_check_exception_skip"]}
    assert py.CHECK_EXCEPTION_SKIP_KINDS == table_kinds, (
        "CHECK_EXCEPTION_SKIP_KINDS drifted from [[frontend_check_exception_skip]]: "
        f"gen-only={sorted(py.CHECK_EXCEPTION_SKIP_KINDS - table_kinds)} "
        f"table-only={sorted(table_kinds - py.CHECK_EXCEPTION_SKIP_KINDS)}"
    )

    may_throw_ops = {r["name"] for r in data["opcode"] if r["may_throw"]}
    for row in data["frontend_check_exception_skip"]:
        if "opcode" in row:
            if row.get("control_flow", False):
                assert row["opcode"] in may_throw_ops, (
                    f"{row['kind']}: control_flow=true but opcode {row['opcode']} is "
                    "not may_throw (spurious flag)"
                )
            else:
                assert row["opcode"] not in may_throw_ops, (
                    f"{row['kind']}: opcode {row['opcode']} is may_throw but not "
                    "flagged control_flow — skipping it would drop the exception edge"
                )


def test_binary_op_maps_match_table_and_are_exhaustive() -> None:
    """BINOP_OP_KIND / AUGASSIGN_OP_KIND equal the table's [[binary_op]] rows,
    keyed by ast.operator subclass __name__, and the table is EXHAUSTIVE over
    ast.operator (a missing operator is the task-#27 silent-gap class)."""
    gen = _gen()
    data = gen.load_table()
    py = _load_generated_py()

    table_binop = {row["ast_op"]: row["binop_kind"] for row in data["binary_op"]}
    table_aug = {row["ast_op"]: row["augassign_kind"] for row in data["binary_op"]}
    assert py.BINOP_OP_KIND == table_binop
    assert py.AUGASSIGN_OP_KIND == table_aug

    ast_operator_names = {cls.__name__ for cls in ast.operator.__subclasses__()}
    assert set(py.BINOP_OP_KIND) == ast_operator_names, (
        "BINOP_OP_KIND is not exhaustive over ast.operator: "
        f"missing={sorted(ast_operator_names - set(py.BINOP_OP_KIND))} "
        f"extra={sorted(set(py.BINOP_OP_KIND) - ast_operator_names)}"
    )
    assert set(py.AUGASSIGN_OP_KIND) == ast_operator_names


def test_frontend_raising_kinds_match_frontend_consumer() -> None:
    """The generated RAISING_KIND_NAMES / CHECK_EXCEPTION_SKIP_KINDS /
    *_OP_KIND constants the frontend imports are the SAME objects the generator
    renders — the frontend emit()/visit_BinOp/visit_AugAssign now have no private
    copy. Importing the frontend lowering module must expose them identically."""
    py = _load_generated_py()
    consumer = _load(
        ROOT / "src/molt/frontend/lowering/op_kinds_generated.py",
        "molt_test_op_kinds_consumer",
    )
    assert consumer.RAISING_KIND_NAMES == py.RAISING_KIND_NAMES
    assert consumer.CHECK_EXCEPTION_SKIP_KINDS == py.CHECK_EXCEPTION_SKIP_KINDS
    assert consumer.BINOP_OP_KIND == py.BINOP_OP_KIND
    assert consumer.AUGASSIGN_OP_KIND == py.AUGASSIGN_OP_KIND


def test_render_detects_frontend_table_mutation() -> None:
    """A change to a frontend table must change the rendered Python (so the
    freshness test catches it). Mutate copies and assert the render differs."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_py(data)

    mutated = json.loads(json.dumps(data))
    mutated["frontend_raising_kind"].append({"kind": "ZZZ_SYNTH", "reason": "test"})
    assert gen.render_py(mutated) != rendered, (
        "appending a frontend_raising_kind row did not change the Python render"
    )

    mutated2 = json.loads(json.dumps(data))
    for row in mutated2["binary_op"]:
        if row["ast_op"] == "Add":
            row["augassign_kind"] = "INPLACE_SYNTH"
    assert gen.render_py(mutated2) != rendered


# ---------------------------------------------------------------------------
# 3. Frontend coverage: a new first-class frontend kind needs a table row.
# ---------------------------------------------------------------------------


def test_frontend_emitter_fully_resolved_and_no_new_drift() -> None:
    """The audit must fully understand the frontend emitter (no UNRESOLVED computed
    kind-emission sites — an unresolved site means a new emit pattern the extractor
    can't prove, itself a drift hazard) and its self-validation (the floordiv /
    matmul / classifier anchors) must hold. A new first-class frontend kind that
    drifts dangerously is caught by `audit_op_kinds.py --check` against the
    baseline (the CI gate); this test pins the extractor's completeness, which
    that gate depends on."""
    audit = _audit()
    res = audit.run_audit()
    assert not res.frontend.unresolved, (
        "frontend emitter has UNRESOLVED computed kind sites the extractor cannot "
        f"prove: {res.frontend.unresolved}"
    )
    fails = audit.self_validate(res)
    assert not fails, f"audit self-validation failed: {fails}"


def test_dangerous_cell_baseline_matches_current_audit() -> None:
    """The dangerous-cell baseline is exact, not an allowlist superset.

    Baseline-only stale entries mask future regressions: if a kind is removed
    from the live dangerous sets, it must leave the committed baseline in the
    same change.
    """
    audit = _audit()
    res = audit.run_audit()
    baseline = json.loads((ROOT / "tools/op_kinds_baseline.json").read_text(encoding="utf-8"))

    assert baseline.get("dangerous", {}) == res.dangerous()


# ---------------------------------------------------------------------------
# Drift-detection mutation guards (negative tests): mutating either side reds.
# ---------------------------------------------------------------------------


def test_render_detects_table_mutation() -> None:
    """A change to the table must change the rendered output (so the freshness
    test can catch it). Mutate a copy of the parsed table and assert the render
    differs."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)

    mutated = json.loads(json.dumps(data))  # deep copy of the dict
    # Flip Add's may_throw.
    for row in mutated["opcode"]:
        if row["name"] == "Add":
            row["may_throw"] = not row["may_throw"]
    assert gen.render_rs(mutated) != rendered, (
        "mutating a table effect bit did not change the render — the freshness "
        "guard would be blind to it"
    )


def test_render_detects_classifier_mutation() -> None:
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    mutated = json.loads(json.dumps(data))
    mutated["classifier_fresh_value"].append("zzz_synthetic_kind")
    assert gen.render_rs(mutated) != rendered


# ---------------------------------------------------------------------------
# 4. Operand-ownership tables (design 27 §2.1/§2.3, the Perceus rung-2 seed of
#    the #58 Owned/Borrowed/Raw/Consumed lattice). The per-OpCode `Borrowed`
#    default is EXHAUSTIVE over the enum; the per-spelling consume override
#    ([[consuming_kind]]) replaces the old drop_insertion.rs hand consume list
#    behind the ownership-module `op_consumed_operand_root`. These tests pin the
#    render + the fail-loud classification
#    discipline + the byte-identical CallArgs consume semantics.
# ---------------------------------------------------------------------------


def _re_search(src: str, fn_sig: str) -> str:
    """Return the body text of a `match` block inside the named generated fn."""
    assert fn_sig in src, f"generated fn {fn_sig!r} not found"
    return src.split(fn_sig, 1)[1]


def _rust_tokens(src: str) -> str:
    """Collapse generated Rust layout without weakening token-order checks."""
    return " ".join(src.split())


def test_operand_ownership_table_renders_exhaustive_and_borrowed() -> None:
    """Every OpCode gets an `operand_ownership` arm in
    `opcode_operand_ownership_table` (EXHAUSTIVE over the enum — the kill for a
    new opcode silently inheriting an unstated borrow/consume assumption). The
    seed is uniformly `Borrowed` (molt's callee-borrows-args ABI, design 20 §1.2)
    EXCEPT the two interior-borrowing reads `LoadAttr`/`Index` and the
    storage value operands' container-absorb finalizer boundary."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)

    # The table region for opcode_operand_ownership_table, bounded by the next fn.
    region = _re_search(rendered, "fn opcode_operand_ownership_table").split(
        "fn opcode_borrows_source_operand"
    )[0]
    region_tokens = _rust_tokens(region)
    # The behavior-preserving seed (ladder #73): every opcode is `all_borrowed`
    # EXCEPT the two interior-borrowing reads and the explicit DecRef consume.
    # `LoadAttr` interior-borrows its single operand; `Index` interior-borrows
    # operand 0 (the container) and merely borrows operand 1 (the key).
    interior = {
        "LoadAttr": ["interior_borrow_keepalive"],
        "Index": ["interior_borrow_keepalive", "borrowed"],
    }
    container_absorb = {
        "StoreIndex": ["borrowed", "borrowed", "container_absorb"],
        "ModuleSetAttr": ["borrowed", "borrowed", "container_absorb"],
    }
    consumed = {"DecRef"}
    expected_arm = {
        "LoadAttr": "OpCode::LoadAttr => OperandOwnership::InteriorBorrowKeepAlive,",
        "Index": (
            "OpCode::Index => match operand_idx { "
            "0 => OperandOwnership::InteriorBorrowKeepAlive, "
            "_ => OperandOwnership::Borrowed, },"
        ),
        "StoreIndex": (
            "OpCode::StoreIndex => match operand_idx { "
            "0 => OperandOwnership::Borrowed, "
            "1 => OperandOwnership::Borrowed, "
            "_ => OperandOwnership::ContainerAbsorb, },"
        ),
        "ModuleSetAttr": (
            "OpCode::ModuleSetAttr => match operand_idx { "
            "0 => OperandOwnership::Borrowed, "
            "1 => OperandOwnership::Borrowed, "
            "_ => OperandOwnership::ContainerAbsorb, },"
        ),
        "DeleteVar": (
            "OpCode::DeleteVar => OperandOwnership::Borrowed,"
        ),
    }
    for row in data["opcode"]:
        name = row["name"]
        if name in interior:
            assert row["operand_ownership"] == interior[name], (
                f"{name} interior-borrow seed drifted: {row['operand_ownership']!r} "
                f"!= {interior[name]!r} (ladder #73 must stay byte-identical to the "
                "op_borrow_source LoadAttr|Index→operand-0 fact)"
            )
            assert _rust_tokens(expected_arm[name]) in region_tokens, (
                f"opcode_operand_ownership_table missing/incorrect {name} arm"
            )
        elif name in container_absorb:
            assert row["operand_ownership"] == container_absorb[name], (
                f"{name} container-absorb seed drifted: {row['operand_ownership']!r} "
                f"!= {container_absorb[name]!r}"
            )
            assert _rust_tokens(expected_arm[name]) in region_tokens, (
                f"opcode_operand_ownership_table missing/incorrect {name} arm"
            )
        elif name in consumed:
            assert row["operand_ownership"] == "all_consumed"
            assert f"OpCode::{name} => OperandOwnership::Consumed," in region, (
                f"opcode_operand_ownership_table missing/incorrect consume arm for {name}"
            )
        elif name == "DeleteVar":
            assert row["operand_ownership"] == ["borrowed", "borrowed"]
            assert _rust_tokens(expected_arm[name]) in region_tokens, (
                f"opcode_operand_ownership_table missing/incorrect {name} arm"
            )
        else:
            assert row["operand_ownership"] == "all_borrowed", (
                f"seed expectation: every non-interior-borrow opcode is all_borrowed; "
                f"{name} is {row['operand_ownership']!r} — a real consume must add a "
                "[[consuming_kind]] row (per-spelling) or be a deliberate per-OpCode "
                "classification with the migration re-verified byte-identical"
            )
            assert f"OpCode::{name} => OperandOwnership::Borrowed," in region, (
                f"opcode_operand_ownership_table missing/incorrect arm for {name}"
            )
    # The enum carries the FULL operand-ownership domain (schema-ready for the
    # #58 ownership-boundary lattice — adding a fact is a table edit, not enum
    # surgery). Pin every variant so the schema can't silently regress.
    assert (
        "pub(crate) enum OperandOwnership {\n"
        "    Borrowed,\n"
        "    Consumed,\n"
        "    Transferred,\n"
        "    InteriorBorrowKeepAlive,\n"
        "    ContainerAbsorb,\n"
        "    ConditionalValidOnlyOnEdge,\n"
        "    NoOperand,\n"
        "}"
    ) in rendered


def test_consuming_kind_table_renders_callargs_consume() -> None:
    """The `[[consuming_kind]]` rows render into `kind_consumed_operand_table`
    with the EXACT `op_consumed_operand_root` semantics: `call_bind` /
    `call_indirect` consume the LAST operand (`arity.checked_sub(1)`), every
    other spelling consumes none (`_ => None`)."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    region = _re_search(rendered, "fn kind_consumed_operand_table")

    consuming = {row["kind"]: row["consumed_operand"] for row in data["consuming_kind"]}
    # The migration's behavior-preserving seed: exactly the two CallArgs forms.
    assert consuming == {"call_bind": "last", "call_indirect": "last"}, (
        "consuming_kind drifted from the op_consumed_operand_root seed "
        f"(call_bind/call_indirect → last): {consuming}"
    )
    for kind, sel in consuming.items():
        assert sel == "last"
        assert f'"{kind}" => arity.checked_sub(1),' in region, (
            f"kind_consumed_operand_table missing the {kind} → last arm"
        )
    assert "_ => None," in region


def test_consuming_kinds_are_known_mapper_spellings() -> None:
    """Every `[[consuming_kind]]` spelling must be a real mapper spelling
    (canonical or alias of a [[kind]] row). A consume override on an unknown
    spelling silently never fires — the exact C6 double-free it must retire."""
    gen = _gen()
    data = gen.load_table()
    spellings: set[str] = set()
    for row in data["kind"]:
        spellings.add(row["canonical"])
        spellings.update(row.get("aliases", []))
    for row in data["consuming_kind"]:
        assert row["kind"] in spellings, (
            f"consuming_kind {row['kind']!r} is not a [[kind]] mapper spelling"
        )


def test_container_absorbing_kind_table_renders_storage_boundaries() -> None:
    """Preserved SimpleIR mutators that retain a value in an existing container
    get a generated per-spelling table, parallel to consuming-kind overrides but
    not consuming the operand."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    region = _re_search(rendered, "fn kind_container_absorbed_operand_table")

    absorbing = {
        row["kind"]: row["absorbed_operand"] for row in data["absorbing_operand_kind"]
    }
    assert absorbing == {
        "list_append": 1,
        "set_attr_generic_obj": 1,
        "set_attr_generic_ptr": 1,
    }
    for kind in absorbing:
        assert f'"{kind}" => Some(1),' in region
    assert "_ => None," in region


def test_result_finalizer_source_kind_table_renders_list_pop_boundary() -> None:
    """Extraction results such as list_pop carry a fresh owned ref whose
    finalizer sensitivity derives from a source container operand."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    region = _re_search(rendered, "fn kind_result_finalizer_source_operand_table")

    result_sources = {
        row["kind"]: row["source_operand"]
        for row in data["result_finalizer_source_kind"]
    }
    assert result_sources == {"list_pop": 0}
    assert "list_pop" in set(data["classifier_fresh_value"])
    assert '"list_pop" => Some(0),' in region
    assert "_ => None," in region


def test_result_absorption_tables_render_container_authority() -> None:
    """Result absorption is generated for first-class Build* opcodes and
    Copy-preserved constructor/class-definition spellings."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    opcode_region = _re_search(
        rendered, "fn opcode_result_absorbs_operand_ownership_table"
    ).split("fn kind_result_absorbs_operand_ownership_table")[0]
    kind_region = _re_search(rendered, "fn kind_result_absorbs_operand_ownership_table")

    truthy = {row["name"] for row in data["opcode"] if row["result_absorbs_operands"]}
    assert truthy == {"BuildList", "BuildDict", "BuildTuple", "BuildSet"}
    for name in truthy:
        assert f"OpCode::{name} => true," in opcode_region
    for row in data["opcode"]:
        if row["name"] not in truthy:
            assert f"OpCode::{row['name']} => false," in opcode_region

    absorbing = {row["kind"] for row in data["absorbing_kind"]}
    assert absorbing == {
        "class_def",
        "dict_new",
        "frozenset_new",
        "list_new",
        "set_new",
        "tuple_new",
    }
    for kind in absorbing:
        assert f'"{kind}"' in kind_region


def test_result_validity_table_renders_iter_next_unboxed_value_out() -> None:
    """Path-sensitive result validity is generated from op_kinds.toml.

    `IterNextUnboxed` result 0 is only initialized on the not-done edge; it must
    never be edge-dropped or retained from the exhaustion edge. The fact belongs
    in the generated op semantics table, not a drop_insertion hand list.
    """
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    region = _re_search(rendered, "fn opcode_result_validity_table").split(
        "fn opcode_result_is_conditionally_valid_only_on_edge"
    )[0]

    validity = {
        (row["opcode"], row["result"]): row["validity"]
        for row in data["result_validity"]
    }
    assert validity == {
        ("IterNextUnboxed", 0): "conditional_valid_only_on_edge"
    }

    assert (
        "pub(crate) enum ResultValidity {\n"
        "    AlwaysValid,\n"
        "    ConditionalValidOnlyOnEdge,\n"
        "}"
    ) in rendered
    arm = _generated_arm_region(region, "OpCode::IterNextUnboxed", "        OpCode::")
    assert "match result_idx" in arm
    assert "0 => ResultValidity::ConditionalValidOnlyOnEdge" in arm
    assert "_ => ResultValidity::AlwaysValid" in arm
    for row in data["opcode"]:
        if row["name"] != "IterNextUnboxed":
            assert f"OpCode::{row['name']} => ResultValidity::AlwaysValid," in region

    predicate = _re_search(
        rendered, "fn opcode_result_is_conditionally_valid_only_on_edge"
    )
    assert "opcode_result_validity_table(opcode, result_idx)" in predicate
    assert "ResultValidity::ConditionalValidOnlyOnEdge" in predicate


def test_result_validity_rejects_bad_rows() -> None:
    gen = _gen()
    data = gen.load_table()

    bad_rows = [
        {"opcode": "Bogus", "result": 0, "validity": "conditional_valid_only_on_edge"},
        {"opcode": "IterNextUnboxed", "result": -1, "validity": "conditional_valid_only_on_edge"},
        {"opcode": "IterNextUnboxed", "result": 0, "validity": "bogus"},
    ]
    opcodes = {row["name"] for row in data["opcode"]}
    for row in bad_rows:
        mutated = json.loads(json.dumps(data))
        mutated["result_validity"] = [row]
        try:
            gen._validate_result_validity(mutated, opcodes)
        except gen.OpKindTableError:
            pass
        else:  # pragma: no cover - explicit fail branch for pytest output clarity
            raise AssertionError(f"bad result_validity row was accepted: {row!r}")

    mutated = json.loads(json.dumps(data))
    mutated["result_validity"] = [
        {
            "opcode": "IterNextUnboxed",
            "result": 0,
            "validity": "conditional_valid_only_on_edge",
        },
        {
            "opcode": "IterNextUnboxed",
            "result": 0,
            "validity": "conditional_valid_only_on_edge",
        },
    ]
    try:
        gen._validate_result_validity(mutated, opcodes)
    except gen.OpKindTableError:
        pass
    else:  # pragma: no cover - explicit fail branch for pytest output clarity
        raise AssertionError("duplicate result_validity row was accepted")


def test_absorbing_kinds_remain_copy_fresh_spellings_not_aliases() -> None:
    """The preserved `*_new` spellings must not become [[kind]] aliases. They
    are fresh Copy spellings with a separate generated absorption fact."""
    gen = _gen()
    data = gen.load_table()
    mapper_spellings: set[str] = set()
    for row in data["kind"]:
        mapper_spellings.add(row["canonical"])
        mapper_spellings.update(row.get("aliases", []))
    fresh = set(data["classifier_fresh_value"])

    for row in data["absorbing_kind"]:
        kind = row["kind"]
        assert kind in fresh
        assert kind not in mapper_spellings


def test_ownership_lattice_uses_generated_result_absorption_tables() -> None:
    """The production lattice must delegate container absorption to generated
    tables, not a hand-maintained `BuildList | BuildTuple | ...` match."""
    source = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")
    marker = "fn op_result_absorbs_operand_ownership("
    assert marker in source
    start = source.index(marker)
    brace = source.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(source)):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = source[start:end]
    assert "opcode_result_absorbs_operand_ownership_table" in body
    assert "kind_result_absorbs_operand_ownership_table" in body
    for stale in (
        "OpCode::BuildList",
        "OpCode::BuildTuple",
        "OpCode::BuildDict",
        "OpCode::BuildSet",
    ):
        assert stale not in body


def test_ownership_lattice_delegates_conditional_result_validity_to_generated_table() -> None:
    """Conditional result validity must stay sourced from generated op-kind facts."""
    ownership = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")
    assert "op_kinds_generated::" in ownership, (
        "ownership_lattice_min.rs must reference generated op-kind tables"
    )
    marker = "fn conditionally_valid_result_roots("
    assert marker in ownership, "conditionally_valid_result_roots not found"
    start = ownership.index(marker)
    brace = ownership.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(ownership)):
        if ownership[i] == "{":
            depth += 1
        elif ownership[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = ownership[start:end]
    assert "opcode_result_is_conditionally_valid_only_on_edge" in body
    assert "aliases.root(result)" in body
    assert "OpCode::IterNextUnboxed" not in body, (
        "conditional result-validity must live in generated tables, not the "
        "ownership lattice source"
    )


def test_drop_insertion_delegates_consume_to_generated_table() -> None:
    """Consumed-operand ownership must live in the ownership lattice module.

    DropInsertion may ask for the consumed root, but it must not own generated
    table reads or a hand-maintained CallArgs-builder spelling list."""
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    ownership = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")

    assert "op_consumed_operand_root" in drop, (
        "drop_insertion.rs must import the ownership-module consume query"
    )
    assert "op_result_absorbs_operand_ownership" in drop, (
        "drop_insertion.rs must import the ownership-module absorption query"
    )
    assert "fn op_consumed_operand_root(" not in drop, (
        "drop_insertion.rs must not define its own consumed-operand helper"
    )
    for forbidden in (
        "kind_consumed_operand_table",
        "opcode_operand_ownership_table",
        "OperandOwnership",
    ):
        assert forbidden not in drop, (
            f"drop_insertion.rs must not own the generated consume authority: {forbidden}"
        )
    # Extract the `fn op_consumed_operand_root(...) { ... }` body by brace-matching
    # from the signature to its closing brace.
    marker = "fn op_consumed_operand_root("
    assert marker in ownership, "op_consumed_operand_root not found"
    start = ownership.index(marker)
    brace = ownership.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(ownership)):
        if ownership[i] == "{":
            depth += 1
        elif ownership[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = ownership[start:end]
    # The duplicate consume hand list must be gone from the function (the only
    # authority is now the generated table the body delegates to).
    assert '"call_bind"' not in body and '"call_indirect"' not in body, (
        "the hand-coded call_bind/call_indirect consume literals must be deleted "
        "from op_consumed_operand_root (now sourced from generated ownership facts)"
    )
    assert "kind_consumed_operand_table" in body, (
        "op_consumed_operand_root's body must call kind_consumed_operand_table"
    )
    # Both generated authorities must be wired (the per-OpCode floor is the
    # council's primary `opcode_operand_ownership_table` deliverable — it must be
    # load-bearing, not dead code).
    assert "opcode_operand_ownership_table" in body, (
        "op_consumed_operand_root must also consult the per-OpCode floor "
        "opcode_operand_ownership_table (the unified operand-ownership query)"
    )


def test_drop_insertion_delegates_conditional_result_validity_to_ownership_lattice() -> None:
    """drop_insertion.rs must consume conditional result-validity through
    OwnershipLattice root facts, not own a second generated-table/root scan."""
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    drop_prod = drop.split("mod tests", 1)[0]
    assert "fn op_result_is_conditionally_valid_only_on_edge(" not in drop
    assert "opcode_result_is_conditionally_valid_only_on_edge" not in drop
    assert "OwnershipRootFacts::compute(func, &aliases)" in drop
    assert "OwnershipLattice::compute_with_root_facts(" in drop
    assert "ownership_lattice.is_conditionally_valid_result_root(canon(v))" in drop
    assert ".conditionally_valid_result_values()" not in drop
    assert ".conditionally_valid_result_roots()" not in drop
    assert "OpCode::IterNextUnboxed" not in drop_prod.replace("`IterNextUnboxed`", ""), (
        "DropInsertion production code must not own an IterNextUnboxed "
        "result-validity hand list"
    )


def test_drop_insertion_consumes_finalizer_sensitive_roots_from_ownership_lattice() -> None:
    """DropInsertion must consume FinalizerSensitive as a root-space lattice fact.

    The lattice owns alias-root folding for finalizer-sensitive values and
    return-boundary deferral; statement-release boundary composition is checked
    separately through `StatementReleasePlan`.
    """
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    assert ".finalizer_sensitive_values()" not in drop
    assert ".finalizer_sensitive_roots()" in drop
    assert "let root = boundary.root;" not in drop
    assert "boundary.value" not in drop
    marker = "let sensitive_roots: HashSet<ValueId> = lattice"
    assert marker in drop, "sensitive_roots lattice consumer not found"
    region = drop[drop.index(marker) : drop.index("let has_suspension", drop.index(marker))]
    assert ".finalizer_sensitive_roots()" in region
    assert ".map(|&v| canon(v))" not in region


def test_drop_insertion_consumes_non_owning_copy_roots_from_ownership_lattice() -> None:
    """DropInsertion must consume C5 non-owning Copy roots as lattice facts.

    The pass owns placement, not copy-result ownership classification. The
    no-heap alias classifier is consumed through a separate ownership helper for
    CFG remapping; droppability must read the OwnershipRootFacts root set.
    """
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    lattice = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")
    assert "non_owning_copy_results" not in drop
    assert "copy_kind_mints_fresh_owned_ref" not in drop
    assert "let mints_fresh =" not in drop
    assert "OwnershipRootFacts::compute(func, &aliases)" in drop
    assert "DropEligibility::new(" in drop
    assert "drop_eligibility.is_droppable(" in drop
    assert "ownership_root_facts.is_drop_owned_root_candidate(" not in drop
    assert "fn non_owning_copy_result_roots(" in lattice
    assert "pub(crate) fn is_non_owning_copy_result_root(" in lattice
    assert "pub(crate) struct DropEligibility" in lattice
    assert "pub(crate) fn is_droppable(" in lattice
    assert "classify_copy_kind(kind)" in lattice
    assert "copy_kind_is_explicit_no_heap_move(kind)" in lattice


def test_drop_insertion_consumes_no_heap_copy_aliases_from_ownership_lattice() -> None:
    """Exception-pop CFG splitting may remap no-heap copy aliases.

    DropInsertion owns the split placement, but the `_original_kind` classifier
    read belongs to the ownership fact module so the pass does not grow another
    copy-spelling authority.
    """
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    drop_prod = drop.split("mod tests", 1)[0]
    lattice = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")

    assert "copy_transparent_alias" in drop_prod
    assert "copy_kind_is_explicit_no_heap_move" not in drop_prod
    assert "fn original_kind(" not in drop_prod

    assert "pub(crate) struct NoHeapCopyAlias" in lattice
    assert "pub(crate) fn copy_transparent_alias(" in lattice
    assert "copy_kind_is_explicit_no_heap_move(original_kind(op))" in lattice
    assert "source: op.operands[0]" in lattice
    assert "result: op.results[0]" in lattice


def test_drop_insertion_consumes_parameter_and_stack_roots_from_ownership_lattice() -> None:
    """Parameter/stack no-drop facts belong to OwnershipRootFacts, not the pass."""
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    lattice = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")
    assert "let param_ids" not in drop
    assert "let param_roots" not in drop
    assert "let stack_values" not in drop
    assert "let stack_roots" not in drop
    assert "fn produces_stack_value(" not in drop
    assert "drop_eligibility.is_droppable(" in drop
    assert "ownership_root_facts.is_drop_owned_root_candidate(" not in drop
    assert "fn parameter_roots(" in lattice
    assert "fn stack_value_roots(" in lattice
    assert "pub(crate) fn is_drop_owned_root_candidate(" in lattice


def test_drop_insertion_delegates_droppable_predicate_to_drop_eligibility() -> None:
    """DropInsertion owns placement, not the composed root/raw droppability test."""
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    lattice = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")
    assert "let droppable =" not in drop
    assert "raw_scalars.contains" not in drop
    assert "live.is_raw_scalar(v)" not in drop
    assert "DropEligibility::new(" in drop
    assert "&live.raw_scalars" in drop
    assert "drop_eligibility.is_raw_scalar_root(canon(v))" in drop
    assert "drop_eligibility.is_droppable(" in drop
    assert "pub(crate) struct DropEligibility" in lattice
    assert "pub(crate) fn is_raw_scalar_root(" in lattice
    assert "pub(crate) fn is_droppable(" in lattice


def test_drop_insertion_consumes_python_lifetime_facts_from_ownership_lattice() -> None:
    """DropInsertion consumes Python lifetime roots instead of re-scanning them."""
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    lattice = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")
    assert "PythonLifetimeFacts::compute(" in drop
    assert "let python_boundary_roots" not in drop
    assert "let explicit_release_roots" not in drop
    assert "let mut bound_roots" not in drop
    assert 'k == "store_var" || k == "load_var"' not in drop
    assert ".local_store_roots()" not in drop
    assert "python_lifetime_facts.is_local_store_root(" not in drop
    assert "python_lifetime_facts.is_bound_local_root(" not in drop
    assert "python_lifetime_facts.is_named_slot_root(" not in drop
    assert (
        "python_lifetime_facts.boundary_release_roots(&drop_eligibility, &ownership_lattice)"
        in drop
    )
    assert (
        "python_lifetime_facts.is_statement_release_boundary_root(" not in drop
    )
    assert (
        "python_lifetime_facts.is_return_boundary_deferred_root(r, &drop_eligibility)"
        in drop
    )
    assert "python_lifetime_facts.has_explicit_release_boundary(v)" in drop
    assert "pub(crate) struct PythonLifetimeFacts" in lattice
    assert "pub(crate) fn compute(func: &TirFunction, aliases: &AliasUnionFind)" in lattice
    assert "pub(crate) fn local_store_roots(" not in lattice
    assert "pub(crate) fn is_local_store_root(" not in lattice
    assert "pub(crate) fn is_bound_local_root(" not in lattice
    assert "pub(crate) fn is_named_slot_root(" not in lattice
    assert "pub(crate) fn is_explicit_release_root(" not in lattice
    assert "pub(crate) fn boundary_release_roots(" in lattice
    assert "drop_eligibility.is_droppable(*root)" in lattice
    assert "ownership_lattice.is_finalizer_sensitive_root(*root)" in lattice
    assert "!self.has_explicit_release_boundary(*root)" in lattice
    assert "pub(crate) fn is_statement_release_boundary_root(" in lattice
    assert "drop_eligibility.is_droppable(root)" in lattice
    assert "!self.local_store_roots.contains(&root)" in lattice
    assert "!self.has_explicit_release_boundary(root)" in lattice
    assert "pub(crate) fn is_return_boundary_deferred_root(" in lattice
    return_boundary_region = lattice[
        lattice.index("pub(crate) fn is_return_boundary_deferred_root(") :
        lattice.index("pub(crate) fn has_explicit_release_boundary(")
    ]
    assert "self.bound_local_roots.contains(&root)" in return_boundary_region
    assert "!self.named_slot_roots.contains(&root)" in return_boundary_region
    assert (
        "!drop_eligibility.is_conditionally_valid_result_root(root)"
        in return_boundary_region
    )
    assert "pub(crate) fn has_explicit_release_boundary(" in lattice


def test_drop_insertion_consumes_statement_release_plan_from_ownership_lattice() -> None:
    """Statement-release boundary composition belongs to the ownership module.

    DropInsertion may materialize the DecRefs, but it must not own the local
    maps that combine FinalizerSensitive storage boundaries, Python lifetime
    exclusions, drop eligibility, sorting, and deduplication.
    """
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    lattice = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")

    assert "StatementReleasePlan::compute(" in drop
    assert "statement_release_plan.contains_released_root(" in drop
    assert "statement_release_plan.after_op().get(&bid)" in drop
    for forbidden in (
        "let mut statement_release_after_op",
        "let mut statement_released_roots",
        "statement_release_finalizer_boundaries()",
        "python_lifetime_facts.is_statement_release_boundary_root(",
    ):
        assert forbidden not in drop, (
            "drop_insertion.rs must not rebuild statement-release authority: "
            f"{forbidden}"
        )

    assert "pub(crate) struct StatementReleasePlan" in lattice
    assert "pub(crate) fn compute(" in lattice
    assert "lattice.statement_release_finalizer_boundaries()" in lattice
    assert (
        "python_lifetime_facts.is_statement_release_boundary_root(root, drop_eligibility)"
        in lattice
    )
    assert "plan.after_op" in lattice
    assert "plan.released_roots.insert(root)" in lattice
    assert "roots.sort_unstable_by_key(" in lattice
    assert "roots.dedup()" in lattice


def test_drop_insertion_consumes_exception_creation_facts_from_ownership_lattice() -> None:
    """CreationRef classification belongs to the ownership module, not placement."""
    drop = (
        ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs"
    ).read_text(encoding="utf-8")
    lattice = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")
    assert "exception_creation_ref_values" in drop
    assert "fn exception_creation_ref_values(" not in drop
    assert "copy_kind_is_exception_creation_ref" not in drop
    assert "pub(crate) fn exception_creation_ref_values(" in lattice
    assert "copy_kind_is_exception_creation_ref" in lattice
    assert "op.opcode != OpCode::Copy" in lattice


def test_operand_ownership_mandatory_fail_loud() -> None:
    """A `[[opcode]]` row WITHOUT `operand_ownership` is a hard generation error
    (mirroring the may_throw/side_effecting/purity exhaustive discipline) — a new
    opcode cannot render until its operand ownership is stated. No silent
    borrow-default that could leak, nor a consume-default that could double-free."""
    gen = _gen()
    data = gen.load_table()
    mutated = json.loads(json.dumps(data))
    # Drop the field from one opcode row.
    del mutated["opcode"][0]["operand_ownership"]
    try:
        gen._validate_operand_ownership(
            mutated["opcode"][0]["name"], mutated["opcode"][0].get("operand_ownership")
        )
    except gen.OpKindTableError as e:
        assert "operand_ownership" in str(e)
    else:
        raise AssertionError(
            "a missing operand_ownership must raise OpKindTableError (fail-loud)"
        )


def test_operand_ownership_rejects_bad_value() -> None:
    """A malformed `operand_ownership` (bad string / bad list leaf) is a hard
    error — a typo must never silently degrade to a borrow/consume assumption, or
    a dropped keepalive (the round-6 interior-borrow UAF). The borrow-of leaf
    `interior_borrow_keepalive` is LIST-ONLY: it is NOT a valid uniform shorthand
    (an op that interior-borrows one operand still borrows the rest)."""
    gen = _gen()
    for bad in (
        "borrowed",
        "all_owned",
        "consume",
        ["borrowed", "moved"],
        # interior_borrow_keepalive is reachable only via a per-position list;
        # neither the bare leaf nor an `all_*` form of it is a valid shorthand.
        "interior_borrow_keepalive",
        "all_interior_borrow_keepalive",
        7,
    ):
        try:
            gen._validate_operand_ownership("SynthOp", bad)
        except gen.OpKindTableError:
            pass
        else:
            raise AssertionError(
                f"operand_ownership={bad!r} must raise OpKindTableError"
            )
    # The valid shapes must pass, including a per-position list that carries the
    # interior-borrow leaf alongside a plain borrowed operand (the `Index` shape).
    for good in (
        "all_borrowed",
        "all_consumed",
        ["borrowed", "consumed"],
        ["interior_borrow_keepalive"],
        ["interior_borrow_keepalive", "borrowed"],
    ):
        gen._validate_operand_ownership("SynthOp", good)


def test_consuming_kind_rejects_unknown_spelling_fail_loud() -> None:
    """A `[[consuming_kind]]` naming a spelling absent from the mapper is a hard
    generation error (the structural kill for a typo'd consume override that
    would silently never fire)."""
    gen = _gen()
    data = gen.load_table()
    mutated = json.loads(json.dumps(data))
    mutated["consuming_kind"].append(
        {"kind": "zzz_not_a_real_kind", "consumed_operand": "last"}
    )
    # Rebuild the valid-spellings map exactly as load_table does.
    owner: dict[str, str] = {}
    for row in mutated["kind"]:
        owner[row["canonical"]] = row["canonical"]
        for a in row.get("aliases", []):
            owner[a] = row["canonical"]
    try:
        gen._validate_consuming_kinds(mutated, owner)
    except gen.OpKindTableError as e:
        assert "zzz_not_a_real_kind" in str(e)
    else:
        raise AssertionError(
            "an unknown consuming_kind spelling must raise OpKindTableError"
        )


# --- Interior-borrow keepalive (design 27 §1.5 borrow-of edge; ladder #73) -----
# The `interior_borrow_keepalive` operand-ownership leaf is the borrow-of fact: a
# per-position operand whose backing store the op's result interior-borrows (the
# `LoadAttr`/`Index` source — the round-6 `Counter._handle` UAF). It renders into
# `opcode_borrows_source_operand`, the single declarative authority `op_borrow_source`
# (alias_analysis.rs) reads, REPLACING the hand-coded `LoadAttr | Index` match.
# These tests pin the seed (byte-identical), the render, and the consumer migration.


def test_borrows_source_operand_renders_loadattr_and_index() -> None:
    """The `interior_borrow_keepalive` rows render into
    `opcode_borrows_source_operand` with the design-27 §1.5 borrow-of seed:
    `LoadAttr`/`Index` borrow into operand 0, every other op into none. This is
    the behavior-preserving migration of `op_borrow_source` (the prior hardcoded
    `LoadAttr | Index => operands.first()`) — and the first construction of
    `OperandOwnership::InteriorBorrowKeepAlive` by a generated TABLE (not just
    `from_str`), the ladder-#73 deliverable."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    region = _re_search(rendered, "fn opcode_borrows_source_operand").split(
        "fn opcode_result_absorbs_operand_ownership_table"
    )[0]

    # The toml seed: exactly LoadAttr/Index carry interior_borrow_keepalive at
    # position 0 (byte-identical to op_borrow_source's LoadAttr|Index→operand-0).
    borrows = {
        row["name"]: gen._borrows_source_operand_index(row["operand_ownership"])
        for row in data["opcode"]
    }
    interior = {k: v for k, v in borrows.items() if v is not None}
    assert interior == {"LoadAttr": 0, "Index": 0}, (
        "opcode_borrows_source_operand drifted from the op_borrow_source seed "
        f"(LoadAttr/Index → operand 0): {interior}"
    )
    assert "OpCode::LoadAttr => Some(0)," in region
    assert "OpCode::Index => Some(0)," in region
    # Exhaustive fall-through: every non-interior-borrow op → None.
    assert "_ => None," in region
    # `OrdAt` is a fused i64 read (a scalar copied out, NOT a reference into the
    # container) — it must NOT be in the table (the round-6 explicit exclusion).
    assert "OpCode::OrdAt =>" not in region, (
        "OrdAt produces an i64 code point, not an interior borrow — it owes no "
        "keepalive and must stay off the borrows-source table"
    )
    # `InteriorBorrowKeepAlive` is constructed by the generated operand-ownership
    # table now — GENUINELY LIVE, not a `from_str`-only forward-compat variant.
    own_region = _re_search(rendered, "fn opcode_operand_ownership_table").split(
        "fn opcode_borrows_source_operand"
    )[0]
    assert "OperandOwnership::InteriorBorrowKeepAlive" in own_region, (
        "opcode_operand_ownership_table must construct InteriorBorrowKeepAlive "
        "(the ladder-#73 deliverable: the first real InteriorBorrowKeepAlive consumer)"
    )


def test_op_borrow_source_delegates_to_generated_table() -> None:
    """alias_analysis.rs's `op_borrow_source` must DELEGATE to the generated
    `opcode_borrows_source_operand` (no hand-maintained `OpCode::LoadAttr |
    OpCode::Index` match in its body). This is the council's 'migrate one consumer
    + delete one duplicate fact' proof for the interior-borrow keepalive (ladder
    #73): the borrow-of fact lives in op_kinds.toml, read by the single authority.

    Scoped to the FUNCTION BODY (not the whole file) so the legitimate LoadAttr /
    Index references elsewhere in alias_analysis.rs (load-purity classification,
    the borrow-provenance unit-test fixtures) are not mistaken for the deleted
    hand-coded match."""
    alias = (ROOT / "runtime/molt-backend/src/tir/passes/alias_analysis.rs").read_text(encoding="utf-8")
    marker = "fn op_borrow_source("
    assert marker in alias, "op_borrow_source not found"
    start = alias.index(marker)
    brace = alias.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(alias)):
        if alias[i] == "{":
            depth += 1
        elif alias[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = alias[start:end]
    # The duplicate borrow-of hand list must be gone from the function (the only
    # authority is now the generated table the body delegates to).
    assert "OpCode::LoadAttr" not in body and "OpCode::Index" not in body, (
        "the hand-coded LoadAttr|Index borrow-of match must be deleted from "
        "op_borrow_source (now sourced from opcode_borrows_source_operand)"
    )
    assert "opcode_borrows_source_operand" in body, (
        "op_borrow_source's body must call the generated opcode_borrows_source_operand"
    )


def test_render_detects_operand_ownership_mutation() -> None:
    """Mutating an operand-ownership classification must change the render (so the
    freshness guard catches a forgotten regeneration)."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)

    mutated = json.loads(json.dumps(data))
    mutated["opcode"][0]["operand_ownership"] = "all_consumed"
    assert gen.render_rs(mutated) != rendered, (
        "flipping an opcode's operand_ownership did not change the render"
    )

    mutated2 = json.loads(json.dumps(data))
    mutated2["consuming_kind"].append({"kind": "call", "consumed_operand": 0})
    assert gen.render_rs(mutated2) != rendered, (
        "adding a consuming_kind row did not change the render"
    )

    # Flipping the interior-borrow seed (the borrow-of fact) must change the
    # render too — the freshness guard protects the round-6 keepalive.
    mutated3 = json.loads(json.dumps(data))
    for row in mutated3["opcode"]:
        if row["name"] == "LoadAttr":
            row["operand_ownership"] = "all_borrowed"
    assert gen.render_rs(mutated3) != rendered, (
        "dropping LoadAttr's interior_borrow_keepalive did not change the render "
        "(would silently re-open the round-6 interior-borrow UAF)"
    )

    mutated4 = json.loads(json.dumps(data))
    for row in mutated4["opcode"]:
        if row["name"] == "BuildList":
            row["result_absorbs_operands"] = False
    assert gen.render_rs(mutated4) != rendered, (
        "dropping BuildList's result_absorbs_operands fact did not change the render"
    )

    mutated5 = json.loads(json.dumps(data))
    mutated5["absorbing_kind"] = [
        row for row in mutated5["absorbing_kind"] if row["kind"] != "list_new"
    ]
    assert gen.render_rs(mutated5) != rendered, (
        "dropping list_new from absorbing_kind did not change the render"
    )

    mutated6 = json.loads(json.dumps(data))
    mutated6["result_validity"] = []
    assert gen.render_rs(mutated6) != rendered, (
        "dropping IterNextUnboxed result_validity did not change the render"
    )


# ---------------------------------------------------------------------------
# 5. Per-terminator operand ownership (design 27 §2.4, the ownership-moves-out /
#    transfer axis; ladder #72). A `Terminator` is NOT an `OpCode` (it is the
#    `Terminator` enum in blocks.rs), so its operand ownership is a DISTINCT
#    generated table keyed on `TerminatorKind` + `OperandCategory`. The
#    `[[terminator]]` section seeds the FIRST real `Transferred` consumer
#    (`Return` value + branch-arg into a successor phi), and
#    `terminator_operand_is_transferred` REPLACES the hand-coded transfer
#    carve-out behind ownership-module `terminator_branch_args` and
#    `terminator_uses_root`. These tests pin the render, the enum
#    exhaustiveness, and the consumer migration.
# ---------------------------------------------------------------------------


def test_terminator_table_renders_transferred_and_borrowed() -> None:
    """The `[[terminator]]` rows render into `terminator_operand_ownership_table`
    with the design-27 §2.4 transfer set: `Return` value + every branch-arg are
    `Transferred`; the `CondBranch`/`Switch` predicate is `Borrowed`;
    `StateDispatch` has no direct SSA predicate; absent categories are
    `NoOperand`. This is the behavior-preserving seed of
    the migrated transfer carve-out — and the first construction of the
    `Transferred` variant by a generated table (not just `from_str`)."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    region = _re_search(rendered, "fn terminator_operand_ownership_table").split(
        "fn terminator_operand_is_transferred"
    )[0]
    region_tokens = _rust_tokens(region)

    variant = {
        "borrowed": "OperandOwnership::Borrowed",
        "transferred": "OperandOwnership::Transferred",
        "none": "OperandOwnership::NoOperand",
    }
    # The behavior-preserving seed (matches the prior hand-coded carve-out
    # exactly): branch-arg forwarders transfer; Return value transfers; the
    # cond/switch predicate is borrowed; StateDispatch reads frame state outside
    # SSA but forwards args like Switch; Branch/Return/Unreachable have an absent
    # category each.
    expected = {
        "Branch": {"direct": "none", "branch_arg": "transferred"},
        "CondBranch": {"direct": "borrowed", "branch_arg": "transferred"},
        "Switch": {"direct": "borrowed", "branch_arg": "transferred"},
        "StateDispatch": {"direct": "none", "branch_arg": "transferred"},
        "Return": {"direct": "transferred", "branch_arg": "none"},
        "Unreachable": {"direct": "none", "branch_arg": "none"},
    }
    table = {row["name"]: row for row in data["terminator"]}
    assert {
        k: {"direct": v["direct"], "branch_arg": v["branch_arg"]}
        for k, v in table.items()
    } == expected, (
        "[[terminator]] drifted from the design-27 §2.4 transfer-site seed "
        "(the migrated terminator_branch_args + terminator_uses_root carve-out)"
    )
    for name, cats in expected.items():
        direct_expr = _rust_tokens(
            f"(TerminatorKind::{name}, OperandCategory::Direct) => {variant[cats['direct']]},"
        )
        direct_block = _rust_tokens(
            f"(TerminatorKind::{name}, OperandCategory::Direct) => {{ {variant[cats['direct']]} }}"
        )
        assert direct_expr in region_tokens or direct_block in region_tokens, (
            f"terminator_operand_ownership_table missing Direct arm for {name}"
        )
        branch_expr = _rust_tokens(
            f"(TerminatorKind::{name}, OperandCategory::BranchArg) => {variant[cats['branch_arg']]},"
        )
        branch_block = _rust_tokens(
            f"(TerminatorKind::{name}, OperandCategory::BranchArg) => {{ {variant[cats['branch_arg']]} }}"
        )
        assert branch_expr in region_tokens or branch_block in region_tokens, (
            f"terminator_operand_ownership_table missing BranchArg arm for {name}"
        )

    # `Transferred` is constructed by the generated table — it is GENUINELY LIVE
    # now, not a `from_str`-only forward-compat variant.
    assert "OperandOwnership::Transferred" in region, (
        "the terminator table must construct OperandOwnership::Transferred "
        "(the ladder-#72 deliverable: the first real Transferred consumer)"
    )
    # The derived transfer predicate is rendered and reads the table.
    derived = _re_search(rendered, "fn terminator_operand_is_transferred")
    assert (
        "terminator_operand_ownership_table(kind, category)" in derived
        and "OperandOwnership::Transferred" in derived
    ), "terminator_operand_is_transferred must derive from the table + Transferred"


def test_terminator_section_exhaustive_over_enum() -> None:
    """The `[[terminator]]` section must cover EXACTLY the `Terminator` enum
    variants declared in blocks.rs — the exhaustiveness that kills an
    unclassified terminator silently inheriting a borrow/transfer assumption in
    the drop pass (mirroring the OpCode-effect exhaustiveness)."""
    import re

    gen = _gen()
    data = gen.load_table()
    table_names = [row["name"] for row in data["terminator"]]
    assert len(table_names) == len(set(table_names)), "duplicate terminator rows"

    src = (ROOT / "runtime/molt-backend/src/tir/blocks.rs").read_text(
        encoding="utf-8"
    )
    m = re.search(r"pub enum Terminator \{(.*?)\n\}", src, re.S)
    assert m is not None, "Terminator enum not found in blocks.rs"
    enum_variants = []
    for line in m.group(1).splitlines():
        s = line.strip()
        if not s or s.startswith(("//", "/*", "*", "#[")):
            continue
        vm = re.match(r"([A-Z]\w*)\s*[\{,]", s)
        if vm:
            enum_variants.append(vm.group(1))

    assert set(table_names) == set(enum_variants), (
        "op_kinds.toml [[terminator]] rows must be EXACTLY the Terminator enum "
        f"variants; table-only={sorted(set(table_names) - set(enum_variants))} "
        f"enum-only={sorted(set(enum_variants) - set(table_names))}"
    )
    # The generator's declarative expectation (_TERMINATOR_VARIANTS) must also
    # equal the enum — so the two cannot drift behind the section's back.
    assert set(gen._TERMINATOR_VARIANTS) == set(enum_variants), (
        "gen_op_kinds._TERMINATOR_VARIANTS drifted from the Terminator enum: "
        f"gen-only={sorted(set(gen._TERMINATOR_VARIANTS) - set(enum_variants))} "
        f"enum-only={sorted(set(enum_variants) - set(gen._TERMINATOR_VARIANTS))}"
    )


def test_terminator_validation_fail_loud() -> None:
    """A `[[terminator]]` row with a bad/missing leaf, or a non-exhaustive
    section, is a hard generation error (no silent transfer/borrow default)."""
    gen = _gen()
    data = gen.load_table()

    # Bad leaf value (consumed is N/A for a terminator; a typo must not degrade).
    for bad in ("consumed", "all_borrowed", "moved", 7, None):
        mutated = json.loads(json.dumps(data))
        mutated["terminator"][0]["direct"] = bad
        try:
            gen._validate_terminators(mutated)
        except gen.OpKindTableError:
            pass
        else:
            raise AssertionError(
                f"terminator direct={bad!r} must raise OpKindTableError"
            )

    # Non-exhaustive (drop a variant row) is a hard error.
    mutated = json.loads(json.dumps(data))
    mutated["terminator"] = [r for r in mutated["terminator"] if r["name"] != "Return"]
    try:
        gen._validate_terminators(mutated)
    except gen.OpKindTableError as e:
        assert "EXHAUSTIVE" in str(e) or "Return" in str(e)
    else:
        raise AssertionError("a missing Terminator variant must raise (exhaustiveness)")

    # The valid leaves pass.
    for good in ("borrowed", "transferred", "none"):
        m2 = json.loads(json.dumps(data))
        m2["terminator"][0]["direct"] = good
        m2["terminator"][0]["branch_arg"] = good
        gen._validate_terminators(m2)


def test_render_detects_terminator_mutation() -> None:
    """Mutating a terminator classification must change the render (so the
    freshness guard catches a forgotten regeneration)."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)

    mutated = json.loads(json.dumps(data))
    for row in mutated["terminator"]:
        if row["name"] == "Return":
            row["direct"] = "borrowed"  # flip the Return transfer off
    assert gen.render_rs(mutated) != rendered, (
        "flipping Return's direct ownership did not change the render"
    )


def test_drop_insertion_delegates_transfer_to_generated_authority() -> None:
    """Terminator transfer ownership must live in the ownership module.

    Scoped to the two transfer-helper FUNCTION BODIES so the structural shape
    match (which fields carry args — legitimately in the pass) is not mistaken for
    a hand-coded transfer fact."""
    drop = (ROOT / "runtime/molt-backend/src/tir/passes/drop_insertion.rs").read_text(encoding="utf-8")
    ownership = (
        ROOT / "runtime/molt-backend/src/tir/passes/ownership_lattice_min.rs"
    ).read_text(encoding="utf-8")

    def _fn_body(src: str, marker: str, label: str) -> str:
        assert marker in src, f"{marker} not found in {label}"
        start = src.index(marker)
        brace = src.index("{", start)
        depth = 0
        for i in range(brace, len(src)):
            if src[i] == "{":
                depth += 1
            elif src[i] == "}":
                depth -= 1
                if depth == 0:
                    return src[start : i + 1]
        raise AssertionError(f"unbalanced braces after {marker}")

    assert "terminator_branch_args" in drop
    assert "terminator_uses_root" in drop
    assert "fn terminator_branch_args(" not in drop
    assert "fn terminator_uses_root(" not in drop
    for forbidden in (
        "terminator_operand_is_transferred",
        "OperandCategory",
        "TerminatorKind",
    ):
        assert forbidden not in drop, (
            f"drop_insertion.rs must not own generated terminator ownership: {forbidden}"
        )

    branch_args = _fn_body(
        ownership, "fn terminator_branch_args(", "ownership_lattice_min.rs"
    )
    uses_root = _fn_body(
        ownership, "fn terminator_uses_root(", "ownership_lattice_min.rs"
    )

    assert "terminator_operand_is_transferred" in branch_args, (
        "terminator_branch_args must gate the forwarded args on the generated "
        "BranchArg transfer fact (not treat them as transfers unconditionally)"
    )
    assert "OperandCategory::BranchArg" in branch_args
    assert "terminator_operand_is_transferred" in uses_root, (
        "terminator_uses_root's Return arm must read the generated Direct transfer "
        "fact (the migrated hand-coded carve-out)"
    )
    assert "OperandCategory::Direct" in uses_root
