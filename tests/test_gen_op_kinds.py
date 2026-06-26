"""Sync + coverage guards for the op-kind single-source-of-truth registry.

The registry (``runtime/molt-tir/src/tir/op_kinds.toml``) is the ONE table
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
import importlib
import importlib.util
import json
import re
import sys
from pathlib import Path
from types import SimpleNamespace


ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "tools" / "audit_op_kinds.py"
OUT_RS = ROOT / "runtime/molt-tir/src/tir/op_kinds_generated.rs"
OUT_PY = ROOT / "src/molt/frontend/lowering/op_kinds_generated.py"


def _read_rs_module_cluster(root_file: Path) -> str:
    parts: list[str] = []
    module_dir = root_file.with_suffix("")
    if module_dir.is_dir():
        for child in sorted(module_dir.rglob("*.rs")):
            if "tests" in child.relative_to(module_dir).parts:
                continue
            parts.append(child.read_text(encoding="utf-8"))
    parts.append(root_file.read_text(encoding="utf-8"))
    return "\n".join(parts)


def _rust_pub_decl(src: str, kind: str, name: str) -> bool:
    return (
        re.search(rf"\bpub(?:\(crate\))?\s+{kind}\s+{re.escape(name)}\b", src)
        is not None
    )


def _rust_pub_fn(src: str, name: str) -> bool:
    return (
        re.search(rf"\bpub(?:\(crate\))?\s+fn\s+{re.escape(name)}\s*\(", src)
        is not None
    )


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
    return importlib.import_module("tools.op_kinds")


def _render_rust():
    return importlib.import_module("tools.op_kinds.render_rust")


def _audit():
    return _load(AUDIT, "molt_test_audit_op_kinds")


def _generated_arm_region(rendered: str, marker: str, next_prefix: str) -> str:
    start = rendered.index(marker)
    next_start = rendered.find(next_prefix, start + len(marker))
    if next_start == -1:
        return rendered[start:]
    return rendered[start:next_start]


def _rust_fn_body(src: str, marker: str) -> str:
    start = src.index(marker)
    brace = src.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    return src[start:end]


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
    render_rust = _render_rust()
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
        render_rust.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    formatted = render_rust._rustfmt_rust_source("fn main(){}\n")

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


def test_simpleir_control_kinds_delegate_to_generated_tables() -> None:
    """SimpleIR CFG/SSA/pre-SSA control facts live in the registry."""
    gen = _gen()
    audit_mod = _audit()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    mod_rs = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/mod.rs")
    cfg_rs = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/cfg.rs")
    lower_from_simple = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/lower_from_simple.rs")
    audit = (ROOT / "tools/audit_op_kinds.py").read_text(encoding="utf-8")

    expected = {
        "label": {"structural", "block_leader"},
        "state_label": {"structural", "block_leader"},
        "if": {"structural", "block_leader", "conditional_branch"},
        "else": {"structural", "block_leader"},
        "end_if": {"structural", "block_leader"},
        "loop_start": {"structural", "block_leader", "block_ender"},
        "loop_end": {"structural", "block_leader", "block_ender"},
        "loop_break": {"structural", "terminator"},
        "loop_continue": {"structural", "terminator"},
        "jump": {"structural", "terminator"},
        "goto": {"structural", "terminator"},
        "br_if": {"structural", "conditional_branch"},
        "loop_break_if_true": {"structural", "conditional_branch"},
        "loop_break_if_false": {"structural", "conditional_branch"},
        "loop_break_if_exception": {"structural", "conditional_branch"},
        "ret": {"structural", "terminator"},
        "ret_void": {"structural", "terminator"},
        "return": {"structural", "terminator"},
        "nop": {"structural"},
        "state_switch": {"structural", "block_ender"},
        "state_yield": {"suspend", "block_ender"},
        "state_transition": {"suspend", "repoll", "block_leader", "block_ender"},
        "chan_send_yield": {"suspend", "repoll", "block_leader", "block_ender"},
        "chan_recv_yield": {"suspend", "repoll", "block_leader", "block_ender"},
        "loop_index_start": {"pre_ssa_rewritten"},
        "loop_index_next": {"pre_ssa_rewritten"},
        "phi": {"ssa_only"},
    }
    fields = gen._SIMPLEIR_CONTROL_FACT_FIELDS
    actual = {
        row["kind"]: {field for field in fields if row[field]}
        for row in data["simpleir_control_kind"]
    }
    assert actual == expected

    for field in fields:
        fn_name = f"simpleir_kind_is_{field}"
        assert _rust_pub_fn(rendered, fn_name)
        body = _rust_fn_body(rendered, f"pub fn {fn_name}(")
        for kind, facts in expected.items():
            assert (f'"{kind}"' in body) == (field in facts)

    consumed_body = _rust_fn_body(
        rendered, "pub fn simpleir_kind_is_cfg_or_ssa_consumed("
    )
    audit_exempt = audit_mod.structural_kinds_from_registry(data)
    assert audit_exempt == {
        kind
        for kind, facts in expected.items()
        if facts & {"structural", "pre_ssa_rewritten", "ssa_only"}
    }
    for kind in audit_exempt:
        assert f'"{kind}"' in consumed_body
    for mapped_suspend in {
        "state_yield",
        "state_transition",
        "chan_send_yield",
        "chan_recv_yield",
    }:
        assert f'"{mapped_suspend}"' not in consumed_body
        assert mapped_suspend not in audit_exempt

    assert "op_kinds_generated::simpleir_kind_is_structural(kind)" in _rust_fn_body(
        mod_rs, "pub(crate) fn is_structural("
    )
    for marker, table in (
        ("fn is_terminator(", "simpleir_kind_is_terminator(kind)"),
        ("fn is_block_leader(", "simpleir_kind_is_block_leader(kind)"),
        ("fn is_block_ender(", "simpleir_kind_is_block_ender(kind)"),
        ("fn is_conditional_branch(", "simpleir_kind_is_conditional_branch(kind)"),
    ):
        body = _rust_fn_body(cfg_rs, marker)
        assert table in body
        assert "matches!" not in body
        assert "loop_start" not in body

    pre_ssa_body = _rust_fn_body(lower_from_simple, "fn is_pre_ssa_rewritten_kind(")
    assert "simpleir_kind_is_pre_ssa_rewritten(kind)" in pre_ssa_body
    assert "PRE_SSA_REWRITTEN_KINDS" not in lower_from_simple

    assert "structural_kinds_from_registry(registry)" in audit
    assert "_STRUCTURAL_CLASSIFIER_FNS" not in audit
    structural_fn = audit.split("def structural_kinds_from_registry", maxsplit=1)[
        1
    ].split("def extract_vec_reduction_ops", maxsplit=1)[0]
    assert "extract_rust_str_slice_const" not in structural_fn


def test_simpleir_control_kind_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()

    duplicate = json.loads(json.dumps(data))
    duplicate["simpleir_control_kind"].append(duplicate["simpleir_control_kind"][0])
    try:
        gen._validate_simpleir_control_kinds(duplicate)
    except gen.OpKindTableError as e:
        assert "duplicate simpleir_control_kind" in str(e)
    else:
        raise AssertionError("duplicate simpleir_control_kind was accepted")

    invalid = json.loads(json.dumps(data))
    invalid["simpleir_control_kind"][0]["repoll"] = True
    try:
        gen._validate_simpleir_control_kinds(invalid)
    except gen.OpKindTableError as e:
        assert "repoll requires suspend and block_leader" in str(e)
    else:
        raise AssertionError("repoll without suspend/block_leader was accepted")

    ssa_overlap = json.loads(json.dumps(data))
    ssa_overlap["simpleir_control_kind"][-1]["structural"] = True
    try:
        gen._validate_simpleir_control_kinds(ssa_overlap)
    except gen.OpKindTableError as e:
        assert "ssa_only cannot overlap runtime facts" in str(e)
    else:
        raise AssertionError("ssa_only/runtime overlap was accepted")


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


def test_simpleir_control_kind_tables_match_registry_and_consumers() -> None:
    """CFG/pre-SSA/SSA-only SimpleIR control facts are registry-owned."""
    gen = _gen()
    audit = _audit()
    data = gen.load_table()

    rows = data["simpleir_control_kind"]
    for field in gen._SIMPLEIR_CONTROL_FACT_FIELDS:
        expected = {row["kind"] for row in rows if row[field]}
        generated = set(
            audit.extract_matches_macro(OUT_RS, f"simpleir_kind_is_{field}")
        )
        assert generated == expected, (
            f"simpleir_kind_is_{field} drifted from [[simpleir_control_kind]]"
        )

    consumed_expected = {
        row["kind"]
        for row in rows
        if row["structural"] or row["pre_ssa_rewritten"] or row["ssa_only"]
    }
    consumed_generated = set(
        audit.extract_matches_macro(OUT_RS, "simpleir_kind_is_cfg_or_ssa_consumed")
    )
    assert consumed_generated == consumed_expected

    tir_mod = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/mod.rs")
    cfg = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/cfg.rs")
    lower_from_simple = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/lower_from_simple.rs")
    audit_source = (ROOT / "tools/audit_op_kinds.py").read_text(encoding="utf-8")

    assert "op_kinds_generated::simpleir_kind_is_structural(kind)" in tir_mod
    assert "simpleir_kind_is_terminator(kind)" in cfg
    assert "simpleir_kind_is_block_leader(kind)" in cfg
    assert "simpleir_kind_is_block_ender(kind)" in cfg
    assert "simpleir_kind_is_conditional_branch(kind)" in cfg
    assert "fn is_suspend_op" not in cfg
    assert "fn is_repoll_op" not in cfg
    assert "simpleir_kind_is_suspend" in cfg
    assert "simpleir_kind_is_repoll" in cfg
    assert "simpleir_kind_is_pre_ssa_rewritten(kind)" in lower_from_simple
    assert "PRE_SSA_REWRITTEN_KINDS" not in lower_from_simple
    assert "structural_kinds_from_registry(registry)" in audit_source
    assert "derive_structural_kinds" not in audit_source
    assert "_STRUCTURAL_CLASSIFIER_FNS" not in audit_source


def test_generated_classifier_matches_table() -> None:
    """The GENERATED Rust classifier tables (`*_table` matches!) must contain
    exactly the table's flat classifier sets — parsed from the generated file."""
    gen = _gen()
    audit = _audit()
    data = gen.load_table()

    gen_fresh = set(
        audit.extract_matches_macro(OUT_RS, "copy_kind_mints_fresh_owned_ref_table")
    )
    gen_owned_alias = set(
        audit.extract_matches_macro(OUT_RS, "copy_kind_mints_owned_alias_ref_table")
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
    assert gen_owned_alias == set(data["classifier_owned_alias"]), (
        "generated owned-alias table drifted from classifier_owned_alias"
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
    assert release_rows == {"DecRef": "all", "DeleteVar": 1, "DelBoundary": "all"}

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
        raise AssertionError(
            "numeric explicit_release_operand without fixed arity was accepted"
        )


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
    assert res.owned_alias == set(data["classifier_owned_alias"])
    assert res.inert_marker == set(data["classifier_inert_marker"])
    assert res.transparent_alias == set(data["classifier_transparent_alias"])
    assert res.no_heap_move == set(data["classifier_no_heap_move"])
    assert set(res.fresh_value_prefixes) == set(data["classifier_fresh_value_prefixes"])


def test_audit_native_arms_include_extracted_op_family_authority() -> None:
    """Native dispatch coverage follows fc/* HANDLED_KINDS after decomposition."""
    audit = _audit()
    native_arms = audit.extract_native_simpleir_arm_kinds()
    memory_consts = audit.extract_rust_str_slice_consts(
        ROOT / "runtime/molt-backend/src/native_backend/function_compiler/fc/memory.rs",
        {"HANDLED_KINDS"},
    )

    assert "guarded_field_init" in memory_consts["HANDLED_KINDS"]
    assert "guarded_field_init" in native_arms
    assert "guarded_field_set" in native_arms
    assert "const" in native_arms


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

    llvm_lowering = _read_rs_module_cluster(ROOT / "runtime/molt-backend/src/llvm_backend/lowering.rs")
    assert f'Some("{current}")' in llvm_lowering
    assert f'Some("{rejected}")' not in llvm_lowering

    current_guarded_danger = {
        kind
        for kinds in res.dangerous().values()
        for kind in kinds
        if kind.startswith("guarded_field")
    }
    baseline = json.loads(
        (ROOT / "tools/op_kinds_baseline.json").read_text(encoding="utf-8")
    )
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
    correct exhaustive arm per opcode for may_throw / side_effecting / effects.
    This is the structural kill for the matches!-default-false trap: the source of
    truth is the table, and a new opcode cannot compile without a row."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    effects = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/effects.rs")

    # effects.rs delegates rather than hand-lists.
    for fn, table in (
        ("opcode_may_throw", "opcode_may_throw_table"),
        ("opcode_is_side_effecting", "opcode_is_side_effecting_table"),
        ("opcode_effects", "opcode_effects_table"),
    ):
        assert f"op_kinds_generated::{table}" in effects, (
            f"effects.rs {fn} must delegate to the generated {table}"
        )
    for stale in (
        "EXPECTED_MOVABLE",
        "EXPECTED_GVN_",
        "assert_opcode_is_listed",
        "OpcodePurity",
        "opcode_purity_table",
    ):
        assert stale not in effects, (
            f"effects.rs still carries stale effect lane {stale}"
        )

    # The generated may_throw / side_effecting tables embed the table's booleans.
    all_opcodes_block = rendered.split("pub const ALL_OPCODES")[1].split(
        "fn opcode_may_throw_table"
    )[0]
    may_block = rendered.split("fn opcode_may_throw_table")[1].split(
        "fn opcode_is_side_effecting_table"
    )[0]
    side_block = rendered.split("fn opcode_is_side_effecting_table")[1].split(
        "fn opcode_effects_table"
    )[0]
    effects_block = rendered.split("fn opcode_effects_table")[1]
    effect_const = {
        "pure": "OPCODE_EFFECTS_PURE",
        "pure_may_throw": "OPCODE_EFFECTS_PURE_MAY_THROW",
        "impure": "OPCODE_EFFECTS_IMPURE",
    }
    for row in data["opcode"]:
        name = row["name"]
        mt = "true" if row["may_throw"] else "false"
        se = "true" if row["side_effecting"] else "false"
        assert f"OpCode::{name}," in all_opcodes_block, (
            f"ALL_OPCODES entry for {name} missing"
        )
        assert f"OpCode::{name} => {mt}," in may_block, (
            f"opcode_may_throw arm for {name} missing/incorrect"
        )
        assert f"OpCode::{name} => {se}," in side_block, (
            f"opcode_is_side_effecting arm for {name} missing/incorrect"
        )
        assert f"OpCode::{name} => {effect_const[row['purity']]}," in effects_block, (
            f"opcode_effects arm for {name} missing/incorrect"
        )


def test_verify_result_arity_delegates_to_generated_table() -> None:
    """Verifier result-count policy belongs to the op-kind registry."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    verify = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/verify.rs")

    table = {row["name"]: row["result_arity"] for row in data["opcode"]}
    variable = {opcode for opcode, arity in table.items() if arity == "variable"}
    assert variable == {
        "Call",
        "CallMethod",
        "CallMethodIc",
        "CallSuperMethodIc",
        "CallBuiltin",
        "CheckException",
        "Copy",
        "ScfIf",
        "ScfFor",
        "ScfWhile",
        "ScfYield",
    }

    expected_fixed_or_variable = {
        "ConstInt": "one",
        "Add": "one",
        "InplaceAdd": "one",
        "CheckedAdd": "two",
        "CheckedMul": "two",
        "IterNextUnboxed": "two",
        "ForIter": "one",
        "DeleteVar": "one",
        "AllocTask": "one",
        "StateTransition": "one",
        "ChanSendYield": "one",
        "ChanRecvYield": "one",
        "ClosureStore": "zero",
        "Yield": "zero",
        "YieldFrom": "zero",
        "Free": "zero",
        "DelBoundary": "zero",
        "ExceptionPending": "one",
        "FunctionDefaultsVersion": "one",
        "TryStart": "zero",
        "TryEnd": "zero",
        "StateBlockStart": "zero",
        "StateBlockEnd": "zero",
        "ModuleCacheGet": "one",
        "ModuleCacheSet": "zero",
        "ModuleCacheDel": "zero",
        "ModuleGetAttr": "one",
        "ModuleImportFrom": "one",
        "ModuleGetGlobal": "one",
        "ModuleGetName": "one",
        "ModuleSetAttr": "zero",
        "ModuleDelGlobal": "zero",
        "ModuleDelGlobalIfPresent": "zero",
        "WarnStderr": "zero",
        "Deopt": "zero",
        "Call": "variable",
        "CallMethod": "variable",
        "CallMethodIc": "variable",
        "CallSuperMethodIc": "variable",
        "CallBuiltin": "variable",
        "CheckException": "variable",
    }
    for opcode, arity in expected_fixed_or_variable.items():
        assert table[opcode] == arity

    arm = {
        "zero": "Some(0)",
        "one": "Some(1)",
        "two": "Some(2)",
        "variable": "None",
    }
    table_block = rendered.split("fn opcode_fixed_result_count_table")[1].split(
        "fn opcode_is_alias_rc_barrier_table"
    )[0]
    for opcode, arity in expected_fixed_or_variable.items():
        assert f"OpCode::{opcode} => {arm[arity]}," in table_block

    assert "opcode_fixed_result_count_table(op.opcode)" in verify
    assert "let expected_results = match op.opcode" not in verify
    assert "OpCode::ConstInt\n                | OpCode::ConstBigInt" not in verify


def test_call_roles_delegate_to_generated_tables() -> None:
    """Call graph and CallFacts opcode/kind membership is registry-owned."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    call_graph = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/call_graph.rs")
    call_facts = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/call_facts.rs")

    expected_roles = {
        "Call": "user_call",
        "CallMethod": "dynamic_method",
        "CallMethodIc": "dynamic_method",
        "CallSuperMethodIc": "dynamic_method",
        "CallBuiltin": "runtime_builtin",
        "Copy": "copy_original_kind",
    }
    assert {row["opcode"]: row["role"] for row in data["call_opcode_roles"]} == (
        expected_roles
    )
    expected_user_call_kinds = [
        "call",
        "call_func",
        "call_internal",
        "call_indirect",
        "call_bind",
        "call_function",
        "call_guarded",
        "call_method",
        "invoke_ffi",
    ]
    assert data["call_graph_user_call_kinds"] == expected_user_call_kinds

    role_block = rendered.split("fn opcode_call_role_table")[1].split(
        "fn simpleir_kind_is_call_graph_user_call"
    )[0]
    role_variant = {
        "user_call": "UserCall",
        "dynamic_method": "DynamicMethod",
        "runtime_builtin": "RuntimeBuiltin",
        "copy_original_kind": "CopyOriginalKind",
    }
    for opcode, role in expected_roles.items():
        assert (
            f"OpCode::{opcode} => CallOpcodeRole::{role_variant[role]}," in role_block
        )
    assert "OpCode::AllocTask => CallOpcodeRole::NotCall," in role_block

    kind_block = rendered.split("fn simpleir_kind_is_call_graph_user_call")[1].split(
        "fn opcode_fixed_result_count_table"
    )[0]
    for kind in expected_user_call_kinds:
        assert f'"{kind}"' in kind_block
    for excluded in ("gpu_thread_id", "gpu_barrier", "call_builtin", "range_new"):
        assert f'"{excluded}"' not in kind_block

    assert "opcode_call_role_table" in call_graph
    assert "simpleir_kind_is_call_graph_user_call" in call_graph
    assert "opcode_call_role_table" in call_facts
    assert "fn is_call_kind" not in call_graph
    assert "fn is_call_op" not in call_facts
    assert "OpCode::Call | OpCode::CallMethod | OpCode::CallBuiltin" not in call_facts


def test_call_role_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_role = json.loads(json.dumps(data))
    bad_role["call_opcode_roles"].append(
        {"opcode": "AllocTask", "role": "copy_original_kind"}
    )
    try:
        gen._validate_call_opcode_roles(bad_role, opcodes)
    except gen.OpKindTableError as exc:
        assert "copy_original_kind is reserved for OpCode::Copy" in str(exc)
    else:
        raise AssertionError("bad call opcode role was accepted")

    mapper_opcode_by_spelling = {}
    for row in data["kind"]:
        for spelling in [row["canonical"], *row.get("aliases", [])]:
            mapper_opcode_by_spelling[spelling] = row["mapper_opcode"]

    bad_kind = json.loads(json.dumps(data))
    bad_kind["call_graph_user_call_kinds"].append("call_builtin")
    try:
        gen._validate_call_graph_user_call_kinds(bad_kind, mapper_opcode_by_spelling)
    except gen.OpKindTableError as exc:
        assert "maps to OpCode::CallBuiltin" in str(exc)
    else:
        raise AssertionError("call_builtin was accepted as a user-call kind")


def test_ssa_attr_transport_delegates_to_generated_tables() -> None:
    """SSA attr transport owns live values; op_kinds.toml owns membership."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    ssa = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/ssa.rs")

    expected_attrs = {
        "Import": "module",
        "ImportFrom": "name",
        "LoadAttr": "name",
        "StoreAttr": "name",
        "DelAttr": "name",
        "CallBuiltin": "name",
        "CallMethod": "method",
        "CallMethodIc": "method",
        "CallSuperMethodIc": "method",
    }
    assert {
        row["opcode"]: row["attr"] for row in data["ssa_s_value_attr_keys"]
    } == expected_attrs

    expected_original_kind = {
        "store_var",
        "call_func",
        "call_internal",
        "call_indirect",
        "call_bind",
        "call_function",
        "call_guarded",
        "invoke_ffi",
        "gpu_thread_id",
        "gpu_block_id",
        "gpu_block_dim",
        "gpu_grid_dim",
        "gpu_barrier",
        "builtin_print",
        "print",
        "range_new",
        "get_attr_generic_ptr",
        "get_attr_generic_obj",
        "get_attr_name",
        "guarded_field_get",
        "load",
        "load_attr",
        "store_attr",
        "set_attr_name",
        "set_attr_generic_ptr",
        "set_attr_generic_obj",
        "guarded_field_set",
        "guarded_field_init",
        "store",
        "store_init",
        "del_attr_name",
        "del_attr_generic_ptr",
        "del_attr_generic_obj",
        "index_set",
    }
    assert set(data["ssa_original_kind_preserving_kinds"]) == expected_original_kind
    copy_row = next(row for row in data["kind"] if row["canonical"] == "copy")
    assert set(copy_row["aliases"]) == {"store_var", "load_var", "copy_var"}
    assert "copy_var" not in data["ssa_original_kind_preserving_kinds"]
    assert "load_var" not in data["ssa_original_kind_preserving_kinds"]

    attr_block = rendered.split("fn opcode_ssa_s_value_attr_key_table", maxsplit=1)[
        1
    ].split("fn simpleir_kind_preserves_original_kind_for_ssa", maxsplit=1)[0]
    for opcode, attr in expected_attrs.items():
        assert f'OpCode::{opcode} => Some("{attr}"),' in attr_block
    for opcode in ("Call", "Copy", "Index", "StoreIndex"):
        assert f"OpCode::{opcode} => None," in attr_block

    preserve_block = rendered.split(
        "fn simpleir_kind_preserves_original_kind_for_ssa", maxsplit=1
    )[1].split("fn copy_kind_mints_fresh_owned_ref_table", maxsplit=1)[0]
    for kind in expected_original_kind:
        assert f'"{kind}"' in preserve_block
    for kind in (
        "copy",
        "load_var",
        "copy_var",
        "call",
        "call_builtin",
        "call_method_ic",
        "call_super_method_ic",
    ):
        assert f'"{kind}"' not in preserve_block

    mapper_block = rendered.split("fn kind_to_opcode_table", maxsplit=1)[1].split(
        "fn opcode_ssa_s_value_attr_key_table", maxsplit=1
    )[0]
    assert (
        '"copy" | "store_var" | "load_var" | "copy_var" => Some(OpCode::Copy)'
        in mapper_block
    )

    production = ssa.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_ssa_s_value_attr_key_table(opcode)" in production
    assert (
        "simpleir_kind_preserves_original_kind_for_ssa(op.kind.as_str())" in production
    )
    assert "kind_to_opcode_table(op.kind.as_str()).is_some()" in production
    for stale in (
        "match opcode {\n                OpCode::Import",
        "OpCode::ImportFrom\n                | OpCode::LoadAttr",
        'OpCode::Call && op.kind != "call"',
        "OpCode::CallBuiltin && !matches!",
        "OpCode::LoadAttr\n                | OpCode::StoreAttr",
        '"get_attr" | "set_attr" | "index"',
        '!matches!(op.kind.as_str(), "copy" | "load_var" | "copy_var")',
    ):
        assert stale not in production


def test_ssa_attr_transport_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}
    mapper_opcode_by_spelling = {
        spelling: row["mapper_opcode"]
        for row in data["kind"]
        for spelling in [row["canonical"], *row.get("aliases", [])]
    }

    bad_attr = json.loads(json.dumps(data))
    bad_attr["ssa_s_value_attr_keys"][0]["attr"] = "s_value"
    try:
        gen._validate_ssa_attr_transport(bad_attr, opcodes, mapper_opcode_by_spelling)
    except gen.OpKindTableError as exc:
        assert "attr must be one of" in str(exc)
    else:
        raise AssertionError("bad SSA s_value attr key was accepted")

    duplicate = json.loads(json.dumps(data))
    duplicate["ssa_s_value_attr_keys"].append(
        json.loads(json.dumps(duplicate["ssa_s_value_attr_keys"][0]))
    )
    try:
        gen._validate_ssa_attr_transport(duplicate, opcodes, mapper_opcode_by_spelling)
    except gen.OpKindTableError as exc:
        assert "duplicate ssa_s_value_attr_keys opcode" in str(exc)
    else:
        raise AssertionError("duplicate SSA s_value attr row was accepted")

    unknown_kind = json.loads(json.dumps(data))
    unknown_kind["ssa_original_kind_preserving_kinds"].append("not_a_kind")
    try:
        gen._validate_ssa_attr_transport(
            unknown_kind, opcodes, mapper_opcode_by_spelling
        )
    except gen.OpKindTableError as exc:
        assert "not a known mapper spelling" in str(exc)
    else:
        raise AssertionError("unknown SSA original-kind spelling was accepted")

    forbidden_copy = json.loads(json.dumps(data))
    forbidden_copy["ssa_original_kind_preserving_kinds"].append("load_var")
    try:
        gen._validate_ssa_attr_transport(
            forbidden_copy, opcodes, mapper_opcode_by_spelling
        )
    except gen.OpKindTableError as exc:
        assert "only store_var may preserve for OpCode::Copy" in str(exc)
    else:
        raise AssertionError("load_var original-kind preservation was accepted")

    wrong_opcode = json.loads(json.dumps(data))
    wrong_opcode["ssa_original_kind_preserving_kinds"].append("add")
    try:
        gen._validate_ssa_attr_transport(
            wrong_opcode, opcodes, mapper_opcode_by_spelling
        )
    except gen.OpKindTableError as exc:
        assert "not an SSA original-kind transport opcode" in str(exc)
    else:
        raise AssertionError("non-transport opcode preservation was accepted")


def test_fuzz_tir_passes_uses_generated_opcode_shapes() -> None:
    """The TIR pass fuzzer's opcode/input-generation shape is registry-owned."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    fuzz = _read_rs_module_cluster(ROOT / "runtime/molt-backend/fuzz/fuzz_targets/fuzz_tir_passes.rs")

    expected = {
        "Add": {"operands": 2, "attr_payload": "none"},
        "Sub": {"operands": 2, "attr_payload": "none"},
        "Mul": {"operands": 2, "attr_payload": "none"},
        "Neg": {"operands": 1, "attr_payload": "none"},
        "Not": {"operands": 1, "attr_payload": "none"},
        "Eq": {"operands": 2, "attr_payload": "none"},
        "Lt": {"operands": 2, "attr_payload": "none"},
        "BitAnd": {"operands": 2, "attr_payload": "none"},
        "BitOr": {"operands": 2, "attr_payload": "none"},
        "Shl": {"operands": 2, "attr_payload": "none"},
        "ConstInt": {"operands": 0, "attr_payload": "i64_value"},
        "ConstFloat": {"operands": 0, "attr_payload": "f64_value"},
        "ConstBool": {"operands": 0, "attr_payload": "bool_value"},
        "ConstNone": {"operands": 0, "attr_payload": "none"},
        "IncRef": {"operands": 1, "attr_payload": "none"},
        "DecRef": {"operands": 1, "attr_payload": "none"},
        "BoxVal": {"operands": 1, "attr_payload": "none"},
        "UnboxVal": {"operands": 1, "attr_payload": "none"},
    }
    table = {
        row["opcode"]: {
            "operands": row["operands"],
            "attr_payload": row.get("attr_payload", "none"),
        }
        for row in data["fuzz_tir_opcode_shapes"]
    }
    assert table == expected

    shapes_region = rendered.split("pub const FUZZ_TIR_OPCODE_SHAPES")[1].split(
        "pub fn opcode_fuzz_tir_operand_count_table"
    )[0]
    operand_region = rendered.split("fn opcode_fuzz_tir_operand_count_table")[1].split(
        "fn opcode_fuzz_tir_attr_payload_rule_table"
    )[0]
    attr_region = rendered.split("fn opcode_fuzz_tir_attr_payload_rule_table")[1].split(
        "pub enum OperandIndependentResultType"
    )[0]
    payload_variant = {
        "none": "None",
        "i64_value": "I64Value",
        "f64_value": "F64Value",
        "bool_value": "BoolValue",
    }
    for opcode, shape in expected.items():
        operands = shape["operands"]
        variant = payload_variant[shape["attr_payload"]]
        assert (
            "FuzzTirOpcodeShape {\n"
            f"        opcode: OpCode::{opcode},\n"
            f"        operands: {operands},\n"
            f"        attr_payload: FuzzTirAttrPayloadRule::{variant},\n"
            "    },"
        ) in shapes_region
        assert f"OpCode::{opcode} => Some({operands})," in operand_region
        assert f"OpCode::{opcode} => FuzzTirAttrPayloadRule::{variant}," in attr_region
    assert "OpCode::Copy => None," in operand_region
    assert "OpCode::Copy => FuzzTirAttrPayloadRule::None," in attr_region

    assert "FUZZ_TIR_OPCODE_SHAPES" in fuzz
    assert "FuzzTirAttrPayloadRule" in fuzz
    assert "opcode_fixed_result_count_table(opcode)" in fuzz
    assert "const OPCODES" not in fuzz
    assert "let num_operands = match opcode" not in fuzz
    assert "let has_result = !matches!" not in fuzz
    assert "match opcode" not in fuzz
    assert "OpCode::Copy" not in fuzz


def test_fuzz_tir_opcode_shape_validation_rejects_variable_result_opcodes() -> None:
    gen = _gen()
    data = gen.load_table()
    bad = json.loads(json.dumps(data))
    bad["fuzz_tir_opcode_shapes"].append({"opcode": "Copy", "operands": 1})
    opcodes = {row["name"]: row for row in bad["opcode"]}

    try:
        gen._validate_fuzz_tir_opcode_shapes(bad, opcodes)
    except gen.OpKindTableError as exc:
        assert "fixed zero/one-result" in str(exc)
    else:
        raise AssertionError("variable-result fuzz TIR opcode shape was accepted")


def test_fuzz_tir_opcode_shape_validation_rejects_attr_payload_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    bad = json.loads(json.dumps(data))
    for row in bad["fuzz_tir_opcode_shapes"]:
        if row["opcode"] == "ConstFloat":
            row["attr_payload"] = "i64_value"
            break
    opcodes = {row["name"]: row for row in bad["opcode"]}

    try:
        gen._validate_fuzz_tir_opcode_shapes(bad, opcodes)
    except gen.OpKindTableError as exc:
        assert "attr_payload must be 'f64_value'" in str(exc)
    else:
        raise AssertionError("bad fuzz TIR attr payload rule was accepted")


def test_operand_independent_result_types_delegate_to_generated_table() -> None:
    """Intrinsic result-type facts live in op_kinds.toml and generated Rust.

    Operand-dependent producers must stay absent: type_refine.rs can prove them
    only after it sees operand facts, and block_versioning.rs must not resurrect
    its old opcode-only arithmetic proof list.
    """
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    block_versioning = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/block_versioning.rs")
    type_refine = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/type_refine.rs")
    branchless = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/branchless_count.rs")
    fast_math = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/fast_math.rs")
    gvn = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/gvn.rs")
    strength_reduction = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/strength_reduction.rs")

    expected = {
        "ConstInt": "i64",
        "ConstBigInt": "dynbox",
        "ConstFloat": "f64",
        "ConstStr": "str",
        "ConstBool": "bool",
        "ConstNone": "none",
        "ConstBytes": "bytes",
        "Eq": "bool",
        "Ne": "bool",
        "Lt": "bool",
        "Le": "bool",
        "Gt": "bool",
        "Ge": "bool",
        "Is": "bool",
        "IsNot": "bool",
        "In": "bool",
        "NotIn": "bool",
        "Not": "bool",
        "Bool": "bool",
        "OrdAt": "i64",
        "BuildList": "list_dynbox",
        "BuildDict": "dict_dynbox_dynbox",
        "BuildSet": "set_dynbox",
        "ModuleCacheGet": "dynbox",
        "ModuleGetAttr": "dynbox",
        "ModuleImportFrom": "dynbox",
        "ModuleGetGlobal": "dynbox",
        "ModuleGetName": "dynbox",
    }
    table = {
        row["name"]: row["operand_independent_result_type"]
        for row in data["opcode"]
        if "operand_independent_result_type" in row
    }
    assert table == expected

    variant = {
        "i64": "OperandIndependentResultType::I64",
        "f64": "OperandIndependentResultType::F64",
        "bool": "OperandIndependentResultType::Bool",
        "str": "OperandIndependentResultType::Str",
        "none": "OperandIndependentResultType::None",
        "bytes": "OperandIndependentResultType::Bytes",
        "dynbox": "OperandIndependentResultType::DynBox",
        "list_dynbox": "OperandIndependentResultType::ListDynBox",
        "dict_dynbox_dynbox": "OperandIndependentResultType::DictDynBoxDynBox",
        "set_dynbox": "OperandIndependentResultType::SetDynBox",
    }
    table_block = rendered.split("fn opcode_operand_independent_result_type_table")[
        1
    ].split("fn opcode_operand_independent_result_tir_type")[0]
    for opcode, ty in expected.items():
        assert f"OpCode::{opcode} => Some({variant[ty]})," in table_block

    unsafe_opcode_only_facts = {
        "Add",
        "Sub",
        "Mul",
        "Div",
        "FloorDiv",
        "Mod",
        "Pow",
        "Neg",
        "Pos",
        "BitAnd",
        "BitOr",
        "BitXor",
        "BitNot",
        "Shl",
        "Shr",
        "And",
        "Or",
        "BuildTuple",
        "GetIter",
        "Index",
        "Copy",
        "TypeGuard",
        "CallBuiltin",
        "CheckedAdd",
        "CheckedMul",
        "IterNextUnboxed",
    }
    assert unsafe_opcode_only_facts.isdisjoint(table)
    for opcode in unsafe_opcode_only_facts:
        assert f"OpCode::{opcode} => None," in table_block

    table_name = "opcode_operand_independent_result_tir_type"
    value_proves_body = block_versioning.split("fn value_proves_type", 1)[1].split(
        "fn clone_block_with_fresh_values", 1
    )[0]
    assert table_name in value_proves_body
    for stale in ("OpCode::Div", "OpCode::Shl", "OpCode::Shr", "OpCode::And"):
        assert stale not in value_proves_body

    infer_body = type_refine.split("fn infer_single_result_type_with_attrs", 1)[
        1
    ].split("fn fresh_value_kind_result_type", 1)[0]
    assert table_name in infer_body
    assert "OpCode::ConstInt => Some(TirType::I64)" not in infer_body
    assert "OpCode::BuildList => Some(TirType::List" not in infer_body
    assert "OpCode::ModuleCacheGet" not in infer_body
    for source in (branchless, fast_math, gvn, strength_reduction):
        production = source.split("#[cfg(test)]", maxsplit=1)[0]
        assert table_name in production

    branchless_production = branchless.split("#[cfg(test)]", maxsplit=1)[0]
    assert "OpCode::Eq\n                | OpCode::Ne" not in branchless_production
    assert "OpCode::ConstFloat =>" not in branchless_production

    gvn_production = gvn.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_gvn_numbering_role_table(op.opcode)" in gvn_production
    assert "OpCode::ConstInt\n            | OpCode::ConstBool" not in gvn_production

    fast_math_production = fast_math.split("#[cfg(test)]", maxsplit=1)[0]
    assert "match op.opcode" not in fast_math_production
    assert "OpCode::ConstFloat =>" not in fast_math_production
    assert "OpCode::ConstInt =>" not in fast_math_production
    assert "OpCode::ConstBool =>" not in fast_math_production

    strength_reduction_production = strength_reduction.split(
        "#[cfg(test)]", maxsplit=1
    )[0]
    assert (
        "match op.opcode"
        not in strength_reduction_production.split(
            "// Phase 2: Scan all blocks and rewrite eligible ops.", maxsplit=1
        )[0]
    )
    assert "OpCode::ConstFloat =>" not in strength_reduction_production
    assert "OpCode::ConstBool =>" not in strength_reduction_production


def test_type_refine_result_type_rules_delegate_to_generated_tables() -> None:
    """type_refine owns rule semantics; op_kinds.toml owns opcode membership."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    type_refine = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/type_refine.rs")

    expected_attr_rows = {
        "ObjectNewBound": "object_type_hint",
        "ObjectNewBoundStack": "object_type_hint",
        "Call": "call_return_type",
        "CallMethod": "call_return_type",
        "CallMethodIc": "call_return_type",
        "CallSuperMethodIc": "call_return_type",
        "CallBuiltin": "call_builtin_return_type",
        "TypeGuard": "type_guard",
        "Copy": "copy_original_kind",
    }
    expected_operand_rows = {
        "Add": "add",
        "InplaceAdd": "add",
        "Mul": "mul",
        "InplaceMul": "mul",
        "Sub": "numeric_arithmetic",
        "InplaceSub": "numeric_arithmetic",
        "Mod": "numeric_arithmetic",
        "Pow": "numeric_arithmetic",
        "FloorDiv": "numeric_arithmetic",
        "Div": "true_division",
        "Neg": "unary_numeric",
        "Pos": "unary_numeric",
        "And": "bool_select",
        "Or": "bool_select",
        "BitAnd": "bitwise_i64",
        "BitOr": "bitwise_i64",
        "BitXor": "bitwise_i64",
        "BitNot": "bit_not_i64",
        "BuildTuple": "build_tuple",
        "GetIter": "get_iter",
        "ForIter": "iter_next",
        "IterNext": "iter_next",
        "Index": "index",
        "Copy": "copy",
        "BoxVal": "box_val",
        "UnboxVal": "unbox_val",
    }
    assert {
        row["opcode"]: row["rule"] for row in data["type_refine_attr_result_type_rules"]
    } == expected_attr_rows
    assert {
        row["opcode"]: row["rule"] for row in data["type_refine_operand_type_rules"]
    } == expected_operand_rows

    attr_variant = {
        "object_type_hint": "ObjectTypeHint",
        "call_return_type": "CallReturnType",
        "call_builtin_return_type": "CallBuiltinReturnType",
        "type_guard": "TypeGuard",
        "copy_original_kind": "CopyOriginalKind",
    }
    operand_variant = {
        "add": "Add",
        "mul": "Mul",
        "numeric_arithmetic": "NumericArithmetic",
        "true_division": "TrueDivision",
        "unary_numeric": "UnaryNumeric",
        "bool_select": "BoolSelect",
        "bitwise_i64": "BitwiseI64",
        "bit_not_i64": "BitNotI64",
        "build_tuple": "BuildTuple",
        "get_iter": "GetIter",
        "iter_next": "IterNext",
        "index": "Index",
        "copy": "Copy",
        "box_val": "BoxVal",
        "unbox_val": "UnboxVal",
    }
    assert "pub enum TypeRefineAttrResultTypeRule" in rendered
    assert "pub enum TypeRefineOperandTypeRule" in rendered
    attr_block = rendered.split(
        "fn opcode_type_refine_attr_result_type_rule_table", maxsplit=1
    )[1].split("pub enum TypeRefineOperandTypeRule", maxsplit=1)[0]
    operand_block = rendered.split(
        "fn opcode_type_refine_operand_type_rule_table", maxsplit=1
    )[1].split("pub enum GvnNumberingRole", maxsplit=1)[0]
    for opcode, rule in expected_attr_rows.items():
        expected = f"TypeRefineAttrResultTypeRule::{attr_variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in attr_block
    for opcode, rule in expected_operand_rows.items():
        expected = f"TypeRefineOperandTypeRule::{operand_variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in operand_block
    for opcode in ("Shl", "Shr", "ConstInt", "BuildList"):
        assert f"OpCode::{opcode} => TypeRefineOperandTypeRule::None," in operand_block

    production = type_refine.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_type_refine_attr_result_type_rule_table(opcode)" in production
    assert "opcode_type_refine_operand_type_rule_table(opcode)" in production
    assert "attr_result_type_override(op.opcode, &op.attrs)" in production
    infer_body = production.split("fn infer_single_result_type_with_attrs", maxsplit=1)[
        1
    ].split("fn fresh_value_kind_result_type", maxsplit=1)[0]
    assert "match opcode {" not in infer_body
    assert "OpCode::Add | OpCode::InplaceAdd" not in infer_body
    assert "OpCode::Copy =>" not in infer_body
    assert (
        "TypeRefineOperandTypeRule::Copy => operand_types.first().cloned()"
        in infer_body
    )


def test_type_refine_result_type_rule_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_operand = json.loads(json.dumps(data))
    bad_operand["type_refine_operand_type_rules"][0]["rule"] = "opcode_guess"
    try:
        gen._validate_opcode_rule_rows(
            bad_operand,
            "type_refine_operand_type_rules",
            opcodes,
            gen._TYPE_REFINE_OPERAND_TYPE_RULES,
            "type-refine operand type rule",
        )
    except gen.OpKindTableError as exc:
        assert "type-refine operand type rule must be one of" in str(exc)
    else:
        raise AssertionError("bad type-refine operand rule was accepted")

    duplicate_attr = json.loads(json.dumps(data))
    duplicate_attr["type_refine_attr_result_type_rules"].append(
        json.loads(json.dumps(duplicate_attr["type_refine_attr_result_type_rules"][0]))
    )
    try:
        gen._validate_opcode_rule_rows(
            duplicate_attr,
            "type_refine_attr_result_type_rules",
            opcodes,
            gen._TYPE_REFINE_ATTR_RESULT_TYPE_RULES,
            "type-refine attr result-type rule",
        )
    except gen.OpKindTableError as exc:
        assert "duplicate type_refine_attr_result_type_rules opcode" in str(exc)
    else:
        raise AssertionError("duplicate type-refine attr rule was accepted")

    unknown_attr_opcode = json.loads(json.dumps(data))
    unknown_attr_opcode["type_refine_attr_result_type_rules"][0]["opcode"] = (
        "AlmostCall"
    )
    try:
        gen._validate_opcode_rule_rows(
            unknown_attr_opcode,
            "type_refine_attr_result_type_rules",
            opcodes,
            gen._TYPE_REFINE_ATTR_RESULT_TYPE_RULES,
            "type-refine attr result-type rule",
        )
    except gen.OpKindTableError as exc:
        assert "opcode 'AlmostCall' is not a known OpCode" in str(exc)
    else:
        raise AssertionError("unknown type-refine attr opcode was accepted")


def test_sccp_constant_rules_delegate_to_generated_tables() -> None:
    """SCCP owns fold semantics; op_kinds.toml owns foldable opcode membership."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    sccp = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/sccp.rs")

    expected_seed_rows = {
        "ConstInt": "int_attr",
        "ConstFloat": "float_attr",
        "ConstBool": "bool_attr",
        "ConstStr": "str_attr",
        "ConstNone": "none_singleton",
    }
    expected_eval_rows = {
        "Add": "add",
        "Sub": "sub",
        "Mul": "mul",
        "Div": "div",
        "FloorDiv": "floordiv",
        "Mod": "mod",
        "Pow": "pow",
        "Eq": "eq",
        "Ne": "ne",
        "Lt": "lt",
        "Le": "le",
        "Gt": "gt",
        "Ge": "ge",
        "Neg": "neg",
        "Not": "not",
        "BuildList": "build_list",
        "BuildDict": "build_dict",
        "BuildTuple": "build_tuple_as_list",
    }
    assert {
        row["opcode"]: row["rule"] for row in data["sccp_constant_seed_rules"]
    } == expected_seed_rows
    assert {
        row["opcode"]: row["rule"] for row in data["sccp_constant_eval_rules"]
    } == expected_eval_rows

    seed_variant = {
        "int_attr": "IntAttr",
        "float_attr": "FloatAttr",
        "bool_attr": "BoolAttr",
        "str_attr": "StrAttr",
        "none_singleton": "NoneSingleton",
    }
    eval_variant = {
        "add": "Add",
        "sub": "Sub",
        "mul": "Mul",
        "div": "Div",
        "floordiv": "FloorDiv",
        "mod": "Mod",
        "pow": "Pow",
        "eq": "Eq",
        "ne": "Ne",
        "lt": "Lt",
        "le": "Le",
        "gt": "Gt",
        "ge": "Ge",
        "neg": "Neg",
        "not": "Not",
        "build_list": "BuildList",
        "build_dict": "BuildDict",
        "build_tuple_as_list": "BuildTupleAsList",
    }
    assert "pub enum SccpConstantSeedRule" in rendered
    assert "pub enum SccpConstantEvalRule" in rendered
    seed_block = rendered.split("fn opcode_sccp_constant_seed_rule_table", maxsplit=1)[
        1
    ].split("pub enum SccpConstantEvalRule", maxsplit=1)[0]
    eval_block = rendered.split("fn opcode_sccp_constant_eval_rule_table", maxsplit=1)[
        1
    ].split("pub enum GvnNumberingRole", maxsplit=1)[0]
    for opcode, rule in expected_seed_rows.items():
        expected = f"SccpConstantSeedRule::{seed_variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in seed_block
    for opcode in ("ConstBytes", "ConstBigInt", "Add"):
        assert f"OpCode::{opcode} => SccpConstantSeedRule::None," in seed_block
    for opcode, rule in expected_eval_rows.items():
        expected = f"SccpConstantEvalRule::{eval_variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in eval_block
    for opcode in ("CallBuiltin", "CallMethod", "Shl", "ConstInt"):
        assert f"OpCode::{opcode} => SccpConstantEvalRule::None," in eval_block

    production = sccp.split("#[cfg(test)]", maxsplit=1)[0]
    assert "seed_constant_lattice_value(op.opcode, &op.attrs)" in production
    assert "opcode_sccp_constant_seed_rule_table(op.opcode)" in production
    assert "opcode_sccp_constant_eval_rule_table(opcode)" in production
    seed_helper = production.split("fn seed_constant_lattice_value", maxsplit=1)[
        1
    ].split("/// Try to evaluate", maxsplit=1)[0]
    assert "match opcode {" not in seed_helper
    assert "OpCode::ConstInt =>" not in seed_helper
    eval_body = production.split("fn evaluate_op", maxsplit=1)[1].split(
        "/// Fold string concatenation", maxsplit=1
    )[0]
    assert "match opcode {" not in eval_body
    assert "OpCode::Add =>" not in eval_body
    assert "SccpConstantEvalRule::BuildTupleAsList => eval_build_list" in eval_body


def test_sccp_constant_rule_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_eval = json.loads(json.dumps(data))
    bad_eval["sccp_constant_eval_rules"][0]["rule"] = "fold_everything"
    try:
        gen._validate_opcode_rule_rows(
            bad_eval,
            "sccp_constant_eval_rules",
            opcodes,
            gen._SCCP_CONSTANT_EVAL_RULES,
            "SCCP constant eval rule",
        )
    except gen.OpKindTableError as exc:
        assert "SCCP constant eval rule must be one of" in str(exc)
    else:
        raise AssertionError("bad SCCP constant eval rule was accepted")

    duplicate_seed = json.loads(json.dumps(data))
    duplicate_seed["sccp_constant_seed_rules"].append(
        json.loads(json.dumps(duplicate_seed["sccp_constant_seed_rules"][0]))
    )
    try:
        gen._validate_opcode_rule_rows(
            duplicate_seed,
            "sccp_constant_seed_rules",
            opcodes,
            gen._SCCP_CONSTANT_SEED_RULES,
            "SCCP constant seed rule",
        )
    except gen.OpKindTableError as exc:
        assert "duplicate sccp_constant_seed_rules opcode" in str(exc)
    else:
        raise AssertionError("duplicate SCCP constant seed rule was accepted")

    unknown_seed_opcode = json.loads(json.dumps(data))
    unknown_seed_opcode["sccp_constant_seed_rules"][0]["opcode"] = "ConstMaybe"
    try:
        gen._validate_opcode_rule_rows(
            unknown_seed_opcode,
            "sccp_constant_seed_rules",
            opcodes,
            gen._SCCP_CONSTANT_SEED_RULES,
            "SCCP constant seed rule",
        )
    except gen.OpKindTableError as exc:
        assert "opcode 'ConstMaybe' is not a known OpCode" in str(exc)
    else:
        raise AssertionError("unknown SCCP constant seed opcode was accepted")


def test_value_range_rules_delegate_to_generated_tables() -> None:
    """Value-range owns interval formulas; op_kinds.toml owns opcode membership."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    value_range = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/value_range.rs")

    expected_transfer_rows = {
        "Add": "add",
        "Sub": "sub",
        "Mul": "mul",
        "Neg": "neg",
        "BitAnd": "bit_and",
        "BitOr": "bit_or",
        "BitXor": "bit_xor",
        "Mod": "mod",
        "Shr": "shr",
        "Shl": "shl",
    }
    expected_const_fold_rows = {
        "Add": "add",
        "Sub": "sub",
        "Mul": "mul",
        "Shl": "shl",
        "Shr": "shr",
        "BitAnd": "bit_and",
        "BitOr": "bit_or",
        "BitXor": "bit_xor",
    }
    expected_cond_narrow_rows = {
        "Lt": "lt_upper_exclusive",
        "Le": "le_upper_inclusive",
    }
    expected_container_length_rows = {
        "BuildList": "fixed_literal",
        "BuildTuple": "fixed_literal",
        "Mul": "list_repeat",
        "CallBuiltin": "len_call",
    }
    assert {
        row["opcode"]: row["rule"] for row in data["value_range_transfer_rules"]
    } == expected_transfer_rows
    assert {
        row["opcode"]: row["rule"] for row in data["value_range_const_fold_rules"]
    } == expected_const_fold_rows
    assert {
        row["opcode"]: row["rule"] for row in data["value_range_cond_narrow_rules"]
    } == expected_cond_narrow_rows
    assert {
        row["opcode"]: row["rule"] for row in data["value_range_container_length_rules"]
    } == expected_container_length_rows

    variant = {
        "add": "Add",
        "sub": "Sub",
        "mul": "Mul",
        "neg": "Neg",
        "bit_and": "BitAnd",
        "bit_or": "BitOr",
        "bit_xor": "BitXor",
        "mod": "Mod",
        "shr": "Shr",
        "shl": "Shl",
    }
    narrow_variant = {
        "lt_upper_exclusive": "LtUpperExclusive",
        "le_upper_inclusive": "LeUpperInclusive",
    }
    length_variant = {
        "fixed_literal": "FixedLiteral",
        "list_repeat": "ListRepeat",
        "len_call": "LenCall",
    }
    assert "pub enum ValueRangeTransferRule" in rendered
    assert "pub enum ValueRangeConstFoldRule" in rendered
    assert "pub enum ValueRangeCondNarrowRule" in rendered
    assert "pub enum ValueRangeContainerLengthRule" in rendered
    transfer_block = rendered.split(
        "fn opcode_value_range_transfer_rule_table", maxsplit=1
    )[1].split("pub enum ValueRangeConstFoldRule", maxsplit=1)[0]
    fold_block = rendered.split(
        "fn opcode_value_range_const_fold_rule_table", maxsplit=1
    )[1].split("pub enum ValueRangeCondNarrowRule", maxsplit=1)[0]
    narrow_block = rendered.split(
        "fn opcode_value_range_cond_narrow_rule_table", maxsplit=1
    )[1].split("pub enum ValueRangeContainerLengthRule", maxsplit=1)[0]
    length_block = rendered.split(
        "fn opcode_value_range_container_length_rule_table", maxsplit=1
    )[1].split("pub enum RangeDevirtRole", maxsplit=1)[0]
    for opcode, rule in expected_transfer_rows.items():
        expected = f"ValueRangeTransferRule::{variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in transfer_block
    for opcode, rule in expected_const_fold_rows.items():
        expected = f"ValueRangeConstFoldRule::{variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in fold_block
    for opcode, rule in expected_cond_narrow_rows.items():
        expected = f"ValueRangeCondNarrowRule::{narrow_variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in narrow_block
    for opcode, rule in expected_container_length_rows.items():
        expected = f"ValueRangeContainerLengthRule::{length_variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in length_block
    for opcode in ("Div", "FloorDiv", "ConstInt", "BuildList"):
        assert f"OpCode::{opcode} => ValueRangeTransferRule::None," in transfer_block
        assert f"OpCode::{opcode} => ValueRangeConstFoldRule::None," in fold_block
        assert f"OpCode::{opcode} => ValueRangeCondNarrowRule::None," in narrow_block
    for opcode in ("Div", "FloorDiv", "ConstInt", "Lt"):
        assert (
            f"OpCode::{opcode} => ValueRangeContainerLengthRule::None," in length_block
        )

    production = value_range.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_value_range_transfer_rule_table(op.opcode)" in production
    assert "opcode_value_range_const_fold_rule_table(op.opcode)" in production
    assert "opcode_value_range_cond_narrow_rule_table(*opcode)" in production
    assert "opcode_value_range_container_length_rule_table(op.opcode)" in production
    transfer_body = production.split("fn transfer_op_range", maxsplit=1)[1].split(
        "/// Collect `ConstInt` values", maxsplit=1
    )[0]
    assert "match op.opcode {" not in transfer_body
    assert "OpCode::Add if op.operands.len() == 2" not in transfer_body
    fold_loop = production.split("let mut changed = true;", maxsplit=1)[1].split(
        "// Second pass: container lengths", maxsplit=1
    )[0]
    assert "matches!(" not in fold_loop
    assert "match op.opcode {" not in fold_loop
    assert "ValueRangeConstFoldRule::BitAnd => Some(a & b)" in fold_loop
    guard_body = production.split("fn narrow_from_header_guards", maxsplit=1)[1].split(
        "/// Meet `range`", maxsplit=1
    )[0]
    assert "match opcode" not in guard_body
    assert "OpCode::Lt" not in guard_body
    assert "OpCode::Le" not in guard_body
    assert "ValueRangeCondNarrowRule::LtUpperExclusive" in guard_body
    assert "ValueRangeCondNarrowRule::LeUpperInclusive" in guard_body
    length_body = production.split("// Second pass: container lengths", maxsplit=1)[
        1
    ].split("/// Build header", maxsplit=1)[0]
    assert "match op.opcode" not in length_body
    assert "OpCode::BuildList" not in length_body
    assert "OpCode::BuildTuple" not in length_body
    assert "OpCode::Mul" not in length_body
    assert "OpCode::CallBuiltin" not in length_body
    assert "ValueRangeContainerLengthRule::FixedLiteral" in length_body
    assert "ValueRangeContainerLengthRule::ListRepeat" in length_body
    assert "ValueRangeContainerLengthRule::LenCall" in length_body


def test_range_devirt_roles_delegate_to_generated_table() -> None:
    """Range devirt owns range/CFG proof; op_kinds.toml owns opcode roles."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    range_devirt = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/range_devirt.rs")

    expected_rows = [
        {"opcode": "CallBuiltin", "role": "range_call_candidate"},
        {"opcode": "GetIter", "role": "iterator_candidate"},
        {"opcode": "IterNextUnboxed", "role": "next_unboxed_candidate"},
    ]
    assert data["range_devirt_roles"] == expected_rows

    role_block = rendered.split("pub enum RangeDevirtRole", maxsplit=1)[1].split(
        "pub enum VectorizeBodyAction", maxsplit=1
    )[0]
    assert "pub fn opcode_range_devirt_role_table" in role_block
    assert "OpCode::CallBuiltin => RangeDevirtRole::RangeCallCandidate," in role_block
    assert "OpCode::GetIter => RangeDevirtRole::IteratorCandidate," in role_block
    assert (
        "OpCode::IterNextUnboxed => RangeDevirtRole::NextUnboxedCandidate,"
        in role_block
    )
    assert "OpCode::Add => RangeDevirtRole::None," in role_block

    production = range_devirt.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_range_devirt_role_table(op.opcode)" in production
    assert "RangeDevirtRole::RangeCallCandidate" in production
    assert "RangeDevirtRole::IteratorCandidate" in production
    assert "RangeDevirtRole::NextUnboxedCandidate" in production
    assert "match op.opcode" not in production
    assert "OpCode::CallBuiltin =>" not in production
    assert "OpCode::GetIter if" not in production
    assert "OpCode::IterNextUnboxed if" not in production


def test_vectorize_opcode_facts_delegate_to_generated_table() -> None:
    """Vectorize owns loop proof; op_kinds.toml owns opcode-level facts."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    vectorize = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/vectorize.rs")

    expected_rows = {
        "Add": {"body": "scalar_arithmetic", "reduction": "sum"},
        "Sub": {"body": "scalar_arithmetic"},
        "Mul": {"body": "scalar_arithmetic", "reduction": "product"},
        "Div": {"body": "scalar_arithmetic"},
        "FloorDiv": {"body": "scalar_arithmetic"},
        "Mod": {"body": "scalar_arithmetic"},
        "Pow": {"body": "scalar_arithmetic"},
        "Neg": {"body": "scalar_arithmetic"},
        "Pos": {"body": "scalar_arithmetic"},
        "BitAnd": {"body": "scalar_arithmetic", "reduction": "and"},
        "BitOr": {"body": "scalar_arithmetic", "reduction": "or"},
        "BitXor": {"body": "scalar_arithmetic"},
        "BitNot": {"body": "scalar_arithmetic"},
        "Shl": {"body": "scalar_arithmetic"},
        "Shr": {"body": "scalar_arithmetic"},
        "Eq": {"body": "scalar_arithmetic"},
        "Ne": {"body": "scalar_arithmetic"},
        "Lt": {"body": "scalar_arithmetic", "reduction": "min"},
        "Le": {"body": "scalar_arithmetic", "reduction": "min"},
        "Gt": {"body": "scalar_arithmetic", "reduction": "max"},
        "Ge": {"body": "scalar_arithmetic", "reduction": "max"},
        "ConstInt": {"body": "scalar_arithmetic"},
        "ConstFloat": {"body": "scalar_arithmetic"},
        "ConstBool": {"body": "scalar_arithmetic"},
        "UnboxVal": {"body": "scalar_arithmetic"},
        "BoxVal": {"body": "scalar_arithmetic"},
        "Copy": {"body": "copy_if_plain"},
        "TypeGuard": {"body": "non_escaping_guard"},
        "GetIter": {"body": "iteration_control", "annotation_target": True},
        "IterNext": {"body": "iteration_control"},
        "IterNextUnboxed": {"body": "iteration_control"},
        "ForIter": {
            "body": "iteration_control",
            "loop_header": True,
            "annotation_target": True,
        },
        "ScfFor": {
            "body": "iteration_control",
            "loop_header": True,
            "annotation_target": True,
        },
    }
    rows = {
        row["opcode"]: {
            "body": row["body"],
            "reduction": row.get("reduction"),
            "loop_header": row.get("loop_header", False),
            "annotation_target": row.get("annotation_target", False),
        }
        for row in data["vectorize_opcode_facts"]
    }
    normalized_expected_rows = {
        opcode: {
            "body": facts["body"],
            "reduction": facts.get("reduction"),
            "loop_header": facts.get("loop_header", False),
            "annotation_target": facts.get("annotation_target", False),
        }
        for opcode, facts in expected_rows.items()
    }
    assert "vector_reduction_rules" not in data
    assert rows == normalized_expected_rows

    action_variant = {
        "scalar_arithmetic": "ScalarArithmetic",
        "copy_if_plain": "CopyIfPlain",
        "iteration_control": "IterationControl",
        "non_escaping_guard": "NonEscapingGuard",
    }
    reduction_variant = {
        "sum": "Sum",
        "product": "Product",
        "and": "And",
        "or": "Or",
        "min": "Min",
        "max": "Max",
    }
    assert "pub enum VectorizeBodyAction" in rendered
    assert "pub enum VectorReductionRule" in rendered
    assert "pub struct VectorizeOpcodeFacts" in rendered
    table_block = rendered.split("fn opcode_vectorize_facts_table", maxsplit=1)[
        1
    ].split("pub enum LirVerifyRule", maxsplit=1)[0]

    def vectorize_arm(opcode: str) -> str:
        match = re.search(
            rf"OpCode::{opcode}\s*=>\s*VectorizeOpcodeFacts\s*\{{(?P<body>.*?)\}},",
            table_block,
            flags=re.S,
        )
        assert match is not None, f"missing vectorize facts arm for {opcode}"
        return match.group("body")

    for opcode, row in rows.items():
        arm = vectorize_arm(opcode)
        body = action_variant[row["body"]]
        rule = row.get("reduction")
        expected_rule = (
            f"VectorReductionRule::{reduction_variant[rule]}"
            if rule is not None
            else "VectorReductionRule::None"
        )
        expected_loop_header = "true" if row["loop_header"] else "false"
        expected_annotation = "true" if row["annotation_target"] else "false"
        assert f"body_action: VectorizeBodyAction::{body}" in arm
        assert f"reduction_rule: {expected_rule}" in arm
        assert f"loop_header_marker: {expected_loop_header}" in arm
        assert f"annotation_target: {expected_annotation}" in arm
    call_arm = vectorize_arm("Call")
    assert "body_action: VectorizeBodyAction::Reject" in call_arm
    assert "reduction_rule: VectorReductionRule::None" in call_arm
    assert "loop_header_marker: false" in call_arm
    assert "annotation_target: false" in call_arm

    production = vectorize.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_vectorize_facts_table(op.opcode)" in production
    assert "opcode_vector_reduction_rule_table" not in production
    assert "fn vectorize_body_decision(" in production
    assert "VectorizeBodyAction::CopyIfPlain if op.is_plain_value_copy()" in production
    helper = production.split("fn reduction_op_for_rule", maxsplit=1)[1].split(
        "// ---------------------------------------------------------------------------",
        maxsplit=1,
    )[0]
    assert "OpCode::" not in helper
    assert "VectorReductionRule::Min => Some(ReductionOp::Min)" in helper
    analysis = production.split("fn analyse_loop", maxsplit=1)[1].split(
        "// Resolve the lane element type",
        maxsplit=1,
    )[0]
    assert "let opcode_facts = opcode_vectorize_facts_table(op.opcode)" in analysis
    assert "vectorize_body_decision(op, opcode_facts.body_action)" in analysis
    assert "reduction = reduction_op_for_rule(opcode_facts.reduction_rule)" in analysis
    for stale in (
        "fn is_impure_call",
        "fn is_memory_store",
        "fn is_disqualifying",
        "fn is_scalar_arithmetic",
        "OpCode::Call | OpCode::CallMethod | OpCode::CallBuiltin",
        "OpCode::ForIter | OpCode::ScfFor | OpCode::GetIter",
    ):
        assert stale not in production
    assert "reduction = match op.opcode" not in analysis


def test_vectorize_opcode_facts_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_rule = json.loads(json.dumps(data))
    bad_rule["vectorize_opcode_facts"][0]["reduction"] = "maybe_sum"
    try:
        gen._validate_vectorize_opcode_facts(bad_rule, opcodes)
    except gen.OpKindTableError as exc:
        assert "reduction must be one of" in str(exc)
    else:
        raise AssertionError("bad vectorize reduction rule was accepted")

    duplicate_rule = json.loads(json.dumps(data))
    duplicate_rule["vectorize_opcode_facts"].append(
        json.loads(json.dumps(duplicate_rule["vectorize_opcode_facts"][0]))
    )
    try:
        gen._validate_vectorize_opcode_facts(duplicate_rule, opcodes)
    except gen.OpKindTableError as exc:
        assert "duplicate vectorize_opcode_facts opcode" in str(exc)
    else:
        raise AssertionError("duplicate vectorize opcode fact was accepted")

    unknown_opcode = json.loads(json.dumps(data))
    unknown_opcode["vectorize_opcode_facts"][0]["opcode"] = "ReduceMaybe"
    try:
        gen._validate_vectorize_opcode_facts(unknown_opcode, opcodes)
    except gen.OpKindTableError as exc:
        assert "opcode 'ReduceMaybe' is not a known OpCode" in str(exc)
    else:
        raise AssertionError("unknown vectorize opcode fact was accepted")

    bad_body = json.loads(json.dumps(data))
    bad_body["vectorize_opcode_facts"][0]["body"] = "maybe_vector"
    try:
        gen._validate_vectorize_opcode_facts(bad_body, opcodes)
    except gen.OpKindTableError as exc:
        assert "body must be one of" in str(exc)
    else:
        raise AssertionError("bad vectorize body action was accepted")

    bad_flag = json.loads(json.dumps(data))
    bad_flag["vectorize_opcode_facts"][0]["loop_header"] = "yes"
    try:
        gen._validate_vectorize_opcode_facts(bad_flag, opcodes)
    except gen.OpKindTableError as exc:
        assert "loop_header must be bool" in str(exc)
    else:
        raise AssertionError("bad vectorize bool flag was accepted")

    bad_loop_header_owner = json.loads(json.dumps(data))
    bad_loop_header_owner["vectorize_opcode_facts"][0]["loop_header"] = True
    try:
        gen._validate_vectorize_opcode_facts(bad_loop_header_owner, opcodes)
    except gen.OpKindTableError as exc:
        assert "loop_header requires body='iteration_control'" in str(exc)
    else:
        raise AssertionError("non-iteration loop-header marker was accepted")

    bad_annotation_owner = json.loads(json.dumps(data))
    bad_annotation_owner["vectorize_opcode_facts"][0]["annotation_target"] = True
    try:
        gen._validate_vectorize_opcode_facts(bad_annotation_owner, opcodes)
    except gen.OpKindTableError as exc:
        assert "annotation_target requires body='iteration_control'" in str(exc)
    else:
        raise AssertionError("non-iteration annotation target was accepted")

    bad_reduction_owner = json.loads(json.dumps(data))
    bad_reduction_owner["vectorize_opcode_facts"][0]["body"] = "iteration_control"
    try:
        gen._validate_vectorize_opcode_facts(bad_reduction_owner, opcodes)
    except gen.OpKindTableError as exc:
        assert "reduction requires body='scalar_arithmetic'" in str(exc)
    else:
        raise AssertionError("non-arithmetic vectorize reduction owner was accepted")


def test_lir_verify_rules_delegate_to_generated_table() -> None:
    """LIR verifier owns invariants; op_kinds.toml owns opcode dispatch."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    verify_lir = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/verify_lir.rs")

    expected_rows = {
        "BoxVal": "box_value",
        "UnboxVal": "unbox_value",
        "Add": "checked_i64_arithmetic",
        "Sub": "checked_i64_arithmetic",
        "Mul": "checked_i64_arithmetic",
        "CallBuiltin": "truthy_materialization",
    }
    assert {
        row["opcode"]: row["rule"] for row in data["lir_verify_rules"]
    } == expected_rows

    variant = {
        "box_value": "BoxValue",
        "unbox_value": "UnboxValue",
        "checked_i64_arithmetic": "CheckedI64Arithmetic",
        "truthy_materialization": "TruthyMaterialization",
    }
    assert "pub enum LirVerifyRule" in rendered
    table_block = rendered.split("fn opcode_lir_verify_rule_table", maxsplit=1)[
        1
    ].split("pub enum GvnNumberingRole", maxsplit=1)[0]
    for opcode, rule in expected_rows.items():
        expected = f"LirVerifyRule::{variant[rule]}"
        assert f"OpCode::{opcode} => {expected}," in table_block
    for opcode in ("Div", "ConstInt", "Call", "StoreAttr"):
        assert f"OpCode::{opcode} => LirVerifyRule::None," in table_block

    production = verify_lir.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_lir_verify_rule_table(op.tir_op.opcode)" in production
    verify_loop = production.split("fn verify_ops", maxsplit=1)[1].split(
        "fn verify_op_surface",
        maxsplit=1,
    )[0]
    assert "match op.tir_op.opcode" not in verify_loop
    assert "OpCode::Add | OpCode::Sub | OpCode::Mul" not in verify_loop
    assert (
        "LirVerifyRule::CheckedI64Arithmetic => {\n"
        "                    verify_checked_i64_arithmetic"
    ) in verify_loop


def test_lir_verify_rule_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_rule = json.loads(json.dumps(data))
    bad_rule["lir_verify_rules"][0]["rule"] = "maybe_box"
    try:
        gen._validate_opcode_rule_rows(
            bad_rule,
            "lir_verify_rules",
            opcodes,
            gen._LIR_VERIFY_RULES,
            "LIR verifier rule",
        )
    except gen.OpKindTableError as exc:
        assert "LIR verifier rule must be one of" in str(exc)
    else:
        raise AssertionError("bad LIR verifier rule was accepted")

    duplicate_rule = json.loads(json.dumps(data))
    duplicate_rule["lir_verify_rules"].append(
        json.loads(json.dumps(duplicate_rule["lir_verify_rules"][0]))
    )
    try:
        gen._validate_opcode_rule_rows(
            duplicate_rule,
            "lir_verify_rules",
            opcodes,
            gen._LIR_VERIFY_RULES,
            "LIR verifier rule",
        )
    except gen.OpKindTableError as exc:
        assert "duplicate lir_verify_rules opcode" in str(exc)
    else:
        raise AssertionError("duplicate LIR verifier rule was accepted")

    unknown_opcode = json.loads(json.dumps(data))
    unknown_opcode["lir_verify_rules"][0]["opcode"] = "VerifyMaybe"
    try:
        gen._validate_opcode_rule_rows(
            unknown_opcode,
            "lir_verify_rules",
            opcodes,
            gen._LIR_VERIFY_RULES,
            "LIR verifier rule",
        )
    except gen.OpKindTableError as exc:
        assert "opcode 'VerifyMaybe' is not a known OpCode" in str(exc)
    else:
        raise AssertionError("unknown LIR verifier opcode was accepted")


def test_value_range_rule_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_transfer = json.loads(json.dumps(data))
    bad_transfer["value_range_transfer_rules"][0]["rule"] = "hopeful_range"
    try:
        gen._validate_opcode_rule_rows(
            bad_transfer,
            "value_range_transfer_rules",
            opcodes,
            gen._VALUE_RANGE_TRANSFER_RULES,
            "value-range transfer rule",
        )
    except gen.OpKindTableError as exc:
        assert "value-range transfer rule must be one of" in str(exc)
    else:
        raise AssertionError("bad value-range transfer rule was accepted")

    duplicate_fold = json.loads(json.dumps(data))
    duplicate_fold["value_range_const_fold_rules"].append(
        json.loads(json.dumps(duplicate_fold["value_range_const_fold_rules"][0]))
    )
    try:
        gen._validate_opcode_rule_rows(
            duplicate_fold,
            "value_range_const_fold_rules",
            opcodes,
            gen._VALUE_RANGE_CONST_FOLD_RULES,
            "value-range const-fold rule",
        )
    except gen.OpKindTableError as exc:
        assert "duplicate value_range_const_fold_rules opcode" in str(exc)
    else:
        raise AssertionError("duplicate value-range const-fold rule was accepted")

    unknown_fold_opcode = json.loads(json.dumps(data))
    unknown_fold_opcode["value_range_const_fold_rules"][0]["opcode"] = "FoldMaybe"
    try:
        gen._validate_opcode_rule_rows(
            unknown_fold_opcode,
            "value_range_const_fold_rules",
            opcodes,
            gen._VALUE_RANGE_CONST_FOLD_RULES,
            "value-range const-fold rule",
        )
    except gen.OpKindTableError as exc:
        assert "opcode 'FoldMaybe' is not a known OpCode" in str(exc)
    else:
        raise AssertionError("unknown value-range const-fold opcode was accepted")

    bad_cond_narrow = json.loads(json.dumps(data))
    bad_cond_narrow["value_range_cond_narrow_rules"][0]["rule"] = "maybe_less"
    try:
        gen._validate_opcode_rule_rows(
            bad_cond_narrow,
            "value_range_cond_narrow_rules",
            opcodes,
            gen._VALUE_RANGE_COND_NARROW_RULES,
            "value-range conditional-narrow rule",
        )
    except gen.OpKindTableError as exc:
        assert "value-range conditional-narrow rule must be one of" in str(exc)
    else:
        raise AssertionError("bad value-range conditional-narrow rule was accepted")

    duplicate_cond_narrow = json.loads(json.dumps(data))
    duplicate_cond_narrow["value_range_cond_narrow_rules"].append(
        json.loads(
            json.dumps(duplicate_cond_narrow["value_range_cond_narrow_rules"][0])
        )
    )
    try:
        gen._validate_opcode_rule_rows(
            duplicate_cond_narrow,
            "value_range_cond_narrow_rules",
            opcodes,
            gen._VALUE_RANGE_COND_NARROW_RULES,
            "value-range conditional-narrow rule",
        )
    except gen.OpKindTableError as exc:
        assert "duplicate value_range_cond_narrow_rules opcode" in str(exc)
    else:
        raise AssertionError(
            "duplicate value-range conditional-narrow rule was accepted"
        )

    unknown_cond_narrow_opcode = json.loads(json.dumps(data))
    unknown_cond_narrow_opcode["value_range_cond_narrow_rules"][0]["opcode"] = (
        "NarrowMaybe"
    )
    try:
        gen._validate_opcode_rule_rows(
            unknown_cond_narrow_opcode,
            "value_range_cond_narrow_rules",
            opcodes,
            gen._VALUE_RANGE_COND_NARROW_RULES,
            "value-range conditional-narrow rule",
        )
    except gen.OpKindTableError as exc:
        assert "opcode 'NarrowMaybe' is not a known OpCode" in str(exc)
    else:
        raise AssertionError(
            "unknown value-range conditional-narrow opcode was accepted"
        )

    bad_container_length = json.loads(json.dumps(data))
    bad_container_length["value_range_container_length_rules"][0]["rule"] = "maybe_len"
    try:
        gen._validate_opcode_rule_rows(
            bad_container_length,
            "value_range_container_length_rules",
            opcodes,
            gen._VALUE_RANGE_CONTAINER_LENGTH_RULES,
            "value-range container-length rule",
        )
    except gen.OpKindTableError as exc:
        assert "value-range container-length rule must be one of" in str(exc)
    else:
        raise AssertionError("bad value-range container-length rule was accepted")

    duplicate_container_length = json.loads(json.dumps(data))
    duplicate_container_length["value_range_container_length_rules"].append(
        json.loads(
            json.dumps(
                duplicate_container_length["value_range_container_length_rules"][0]
            )
        )
    )
    try:
        gen._validate_opcode_rule_rows(
            duplicate_container_length,
            "value_range_container_length_rules",
            opcodes,
            gen._VALUE_RANGE_CONTAINER_LENGTH_RULES,
            "value-range container-length rule",
        )
    except gen.OpKindTableError as exc:
        assert "duplicate value_range_container_length_rules opcode" in str(exc)
    else:
        raise AssertionError("duplicate value-range container-length rule was accepted")

    unknown_container_length_opcode = json.loads(json.dumps(data))
    unknown_container_length_opcode["value_range_container_length_rules"][0][
        "opcode"
    ] = "LengthMaybe"
    try:
        gen._validate_opcode_rule_rows(
            unknown_container_length_opcode,
            "value_range_container_length_rules",
            opcodes,
            gen._VALUE_RANGE_CONTAINER_LENGTH_RULES,
            "value-range container-length rule",
        )
    except gen.OpKindTableError as exc:
        assert "opcode 'LengthMaybe' is not a known OpCode" in str(exc)
    else:
        raise AssertionError("unknown value-range container-length opcode was accepted")


def test_range_devirt_role_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_role = json.loads(json.dumps(data))
    bad_role["range_devirt_roles"][0]["role"] = "iterator_candidate"
    try:
        gen._validate_range_devirt_roles(bad_role, opcodes)
    except gen.OpKindTableError as exc:
        assert "reserved for OpCode::GetIter" in str(exc)
    else:
        raise AssertionError("CallBuiltin was accepted with the GetIter role")

    duplicate_role = json.loads(json.dumps(data))
    duplicate_role["range_devirt_roles"].append(
        json.loads(json.dumps(duplicate_role["range_devirt_roles"][0]))
    )
    try:
        gen._validate_range_devirt_roles(duplicate_role, opcodes)
    except gen.OpKindTableError as exc:
        assert "duplicate range_devirt_roles opcode: CallBuiltin" in str(exc)
    else:
        raise AssertionError("duplicate range-devirt opcode role was accepted")


def test_operand_independent_result_type_validation_rejects_drift(tmp_path) -> None:
    gen = _gen()
    table = ROOT / "runtime/molt-tir/src/tir/op_kinds.toml"
    original = table.read_text(encoding="utf-8")

    bad_type = original.replace(
        'name = "ConstInt"\n'
        "may_throw = false\n"
        "side_effecting = false\n"
        'purity = "pure"\n'
        'result_arity = "one"\n'
        'operand_independent_result_type = "i64"',
        'name = "ConstInt"\n'
        "may_throw = false\n"
        "side_effecting = false\n"
        'purity = "pure"\n'
        'result_arity = "one"\n'
        'operand_independent_result_type = "bigint_maybe"',
        1,
    )
    tmp_table = tmp_path / "op_kinds_bad_type.toml"
    tmp_table.write_text(bad_type, encoding="utf-8", newline="\n")
    try:
        gen.load_table(tmp_table)
    except gen.OpKindTableError as exc:
        assert "operand_independent_result_type must be one of" in str(exc)
    else:  # pragma: no cover - explicit fail branch for pytest output clarity
        raise AssertionError("invalid operand_independent_result_type was accepted")

    bad_arity = original.replace(
        'name = "ConstBool"\n'
        "may_throw = false\n"
        "side_effecting = false\n"
        'purity = "pure"\n'
        'result_arity = "one"\n'
        'operand_independent_result_type = "bool"',
        'name = "ConstBool"\n'
        "may_throw = false\n"
        "side_effecting = false\n"
        'purity = "pure"\n'
        'result_arity = "zero"\n'
        'operand_independent_result_type = "bool"',
        1,
    )
    tmp_table = tmp_path / "op_kinds_bad_arity.toml"
    tmp_table.write_text(bad_arity, encoding="utf-8", newline="\n")
    try:
        gen.load_table(tmp_table)
    except gen.OpKindTableError as exc:
        assert "operand_independent_result_type requires result_arity = 'one'" in str(
            exc
        )
    else:  # pragma: no cover - explicit fail branch for pytest output clarity
        raise AssertionError("multi-result intrinsic result-type fact was accepted")


def test_result_arity_rejects_unreviewed_variable_opcode(tmp_path) -> None:
    """`variable` is an audited escape hatch, not a default for uncertain ops."""
    gen = _gen()
    table = ROOT / "runtime/molt-tir/src/tir/op_kinds.toml"
    mutated = table.read_text(encoding="utf-8").replace(
        'name = "Add"\n'
        "may_throw = false\n"
        "side_effecting = false\n"
        'purity = "pure"\n'
        'result_arity = "one"',
        'name = "Add"\n'
        "may_throw = false\n"
        "side_effecting = false\n"
        'purity = "pure"\n'
        'result_arity = "variable"',
        1,
    )
    tmp_table = tmp_path / "op_kinds.toml"
    tmp_table.write_text(mutated, encoding="utf-8", newline="\n")

    try:
        gen.load_table(tmp_table)
    except gen.OpKindTableError as exc:
        assert "result_arity = 'variable' is reserved" in str(exc)
    else:  # pragma: no cover - explicit fail branch for pytest output clarity
        raise AssertionError("unreviewed variable result_arity opcode was accepted")


def test_gvn_numbering_roles_delegate_to_generated_table() -> None:
    """GVN numbering policy and proven-type seeds are generated distinct facts."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    effects = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/effects.rs")
    type_refine = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/type_refine.rs")
    gvn = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/gvn.rs")

    expected_always = {
        "BoxVal",
        "UnboxVal",
    }
    expected_type_gated = {
        "Add",
        "Sub",
        "Mul",
        "InplaceAdd",
        "InplaceSub",
        "InplaceMul",
        "Div",
        "FloorDiv",
        "Mod",
        "Pow",
        "Neg",
        "Pos",
        "Eq",
        "Ne",
        "Lt",
        "Le",
        "Gt",
        "Ge",
        "Is",
        "IsNot",
        "BitAnd",
        "BitOr",
        "BitXor",
        "BitNot",
        "Shl",
        "Shr",
        "And",
        "Or",
        "Not",
        "Bool",
        "TypeGuard",
    }
    expected_value_keyed_rows = {
        "ConstInt": {"opcode": "ConstInt", "key": "i64_attr", "attrs": ["value"]},
        "ConstBigInt": {
            "opcode": "ConstBigInt",
            "key": "str_attr",
            "attrs": ["s_value"],
        },
        "ConstBool": {"opcode": "ConstBool", "key": "bool_attr", "attrs": ["value"]},
        "ConstNone": {"opcode": "ConstNone", "key": "none_singleton"},
        "ConstFloat": {
            "opcode": "ConstFloat",
            "key": "f64_bits_attr",
            "attrs": ["f_value", "value"],
        },
        "ConstStr": {
            "opcode": "ConstStr",
            "key": "str_attr",
            "attrs": ["s_value", "value"],
        },
        "ConstBytes": {
            "opcode": "ConstBytes",
            "key": "bytes_attr",
            "attrs": ["bytes", "value"],
        },
    }
    expected_attr_key_rows = {
        "TypeGuard": {
            "opcode": "TypeGuard",
            "key": "str_attr",
            "attrs": ["expected_type", "ty"],
        },
    }
    expected_value_keyed = set(expected_value_keyed_rows)
    expected_proven_seeds = expected_value_keyed - {"ConstBigInt"}
    assert set(data["gvn_always_numberable_opcodes"]) == expected_always
    assert set(data["gvn_type_gated_numberable_opcodes"]) == expected_type_gated
    assert {
        row["opcode"]: row for row in data["gvn_value_keyed_constant_opcodes"]
    } == expected_value_keyed_rows
    assert {
        row["opcode"]: row for row in data["gvn_numberable_attr_key_opcodes"]
    } == expected_attr_key_rows
    assert set(data["proven_result_type_seed_opcodes"]) == expected_proven_seeds

    gvn_block = rendered.split("fn opcode_gvn_numbering_role_table")[1].split(
        "enum GvnValueKeyKind"
    )[0]
    key_spec_block = rendered.split("fn opcode_gvn_value_key_spec_table")[1].split(
        "fn opcode_is_proven_result_type_seed_table"
    )[0]
    proven_block = rendered.split("fn opcode_is_proven_result_type_seed_table")[
        1
    ].split("fn opcode_is_alias_rc_barrier_table")[0]
    for row in data["opcode"]:
        name = row["name"]
        if name in expected_always:
            expected_role = "GvnNumberingRole::Always"
        elif name in expected_type_gated:
            expected_role = "GvnNumberingRole::TypeGated"
        elif name in expected_value_keyed:
            expected_role = "GvnNumberingRole::ValueKeyedConstant"
        else:
            expected_role = "GvnNumberingRole::Never"
        expected_bool = "true" if name in expected_proven_seeds else "false"
        assert f"OpCode::{name} => {expected_role}," in gvn_block
        assert f"OpCode::{name} => {expected_bool}," in proven_block
    assert "OpCode::ConstBigInt => GvnNumberingRole::ValueKeyedConstant," in gvn_block
    assert "OpCode::ConstBigInt => false," in proven_block
    assert "pub enum GvnValueKeyKind" in rendered
    assert "pub struct GvnValueKeySpec" in rendered
    expected_key_variants = {
        "ConstInt": "I64Attr",
        "ConstBigInt": "StrAttr",
        "ConstBool": "BoolAttr",
        "ConstNone": "NoneSingleton",
        "ConstFloat": "F64BitsAttr",
        "ConstStr": "StrAttr",
        "ConstBytes": "BytesAttr",
        "TypeGuard": "StrAttr",
    }
    expected_attr_consts = {
        "ConstInt": 'const GVN_VALUE_KEY_ATTRS_CONST_INT: &[&str] = &["value"];',
        "ConstBigInt": 'const GVN_VALUE_KEY_ATTRS_CONST_BIG_INT: &[&str] = &["s_value"];',
        "ConstBool": 'const GVN_VALUE_KEY_ATTRS_CONST_BOOL: &[&str] = &["value"];',
        "ConstNone": "const GVN_VALUE_KEY_ATTRS_CONST_NONE: &[&str] = &[];",
        "ConstFloat": (
            'const GVN_VALUE_KEY_ATTRS_CONST_FLOAT: &[&str] = &["f_value", "value"];'
        ),
        "ConstStr": (
            'const GVN_VALUE_KEY_ATTRS_CONST_STR: &[&str] = &["s_value", "value"];'
        ),
        "ConstBytes": (
            'const GVN_VALUE_KEY_ATTRS_CONST_BYTES: &[&str] = &["bytes", "value"];'
        ),
        "TypeGuard": (
            'const GVN_VALUE_KEY_ATTRS_TYPE_GUARD: &[&str] = &["expected_type", "ty"];'
        ),
    }
    for opcode, variant in expected_key_variants.items():
        const_decl = expected_attr_consts[opcode]
        const_name = const_decl.split(":", maxsplit=1)[0].removeprefix("const ")
        assert const_decl in rendered
        arm = key_spec_block.split(
            f"OpCode::{opcode} => Some(GvnValueKeySpec {{", maxsplit=1
        )[1].split("}),", maxsplit=1)[0]
        assert f"kind: GvnValueKeyKind::{variant}," in arm
        assert f"attrs: {const_name}," in arm
    assert "OpCode::Add => None," in key_spec_block

    type_refine_production = type_refine.split("#[cfg(test)]", maxsplit=1)[0]
    assert (
        "opcode_is_proven_result_type_seed_table(op.opcode)" in type_refine_production
    )
    extract_proven_map = type_refine_production.split(
        "pub fn extract_proven_map", maxsplit=1
    )[1].split("fn parse_return_type_str", maxsplit=1)[0]
    assert "match op.opcode" not in extract_proven_map
    gvn_production = gvn.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_gvn_numbering_role_table(op.opcode)" in gvn_production
    assert "opcode_gvn_value_key_spec_table(op.opcode)" in gvn_production
    assert "GvnNumberingRole::Always" in gvn_production
    assert "GvnNumberingRole::TypeGated" in gvn_production
    assert "GvnNumberingRole::ValueKeyedConstant" in gvn_production
    assert "opcode_is_gvn_value_keyed_constant_table" not in gvn_production
    assert "opcode_gvn_constant_key_spec_table" not in gvn_production
    assert "fn is_always_numberable" not in gvn_production
    assert "fn is_typed_numberable" not in gvn_production
    assert "fn is_const_opcode" not in gvn_production
    assert "fn const_keys" not in gvn_production
    assert "const_int_key" not in gvn_production
    assert "const_str_key" not in gvn_production
    assert "const_bytes_key" not in gvn_production
    for opcode in expected_value_keyed:
        assert f"OpCode::{opcode} =>" not in gvn_production

    effects_production = effects.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_is_type_gated_numberable" not in effects_production


def test_gvn_value_keyed_constant_fact_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"]: row for row in data["opcode"]}

    bad_key = json.loads(json.dumps(data))
    bad_key["gvn_value_keyed_constant_opcodes"][0]["key"] = "opaque_default"
    try:
        gen._validate_gvn_value_keyed_constant_facts(bad_key, opcodes)
    except gen.OpKindTableError as e:
        assert "key must be one of" in str(e)
    else:
        raise AssertionError("bad GVN constant key kind was accepted")

    missing_attrs = json.loads(json.dumps(data))
    missing_attrs["gvn_value_keyed_constant_opcodes"][0].pop("attrs")
    try:
        gen._validate_gvn_value_keyed_constant_facts(missing_attrs, opcodes)
    except gen.OpKindTableError as e:
        assert "requires a non-empty attrs list" in str(e)
    else:
        raise AssertionError("GVN constant attr-key row without attrs was accepted")

    duplicate = json.loads(json.dumps(data))
    duplicate["gvn_value_keyed_constant_opcodes"].append(
        json.loads(json.dumps(duplicate["gvn_value_keyed_constant_opcodes"][0]))
    )
    try:
        gen._validate_gvn_value_keyed_constant_facts(duplicate, opcodes)
    except gen.OpKindTableError as e:
        assert "duplicate gvn_value_keyed_constant_opcodes opcode" in str(e)
    else:
        raise AssertionError("duplicate GVN constant key row was accepted")

    attr_bad_opcode = json.loads(json.dumps(data))
    attr_bad_opcode["gvn_numberable_attr_key_opcodes"].append(
        {"opcode": "Call", "key": "str_attr", "attrs": ["name"]}
    )
    try:
        gen._validate_gvn_numberable_attr_key_facts(attr_bad_opcode, opcodes)
    except gen.OpKindTableError as e:
        assert "opcode must be in gvn_always_numberable_opcodes" in str(e)
    else:
        raise AssertionError("GVN attr key for non-numbered opcode was accepted")

    attr_none_key = json.loads(json.dumps(data))
    attr_none_key["gvn_numberable_attr_key_opcodes"][0]["key"] = "none_singleton"
    try:
        gen._validate_gvn_numberable_attr_key_facts(attr_none_key, opcodes)
    except gen.OpKindTableError as e:
        assert "key must be one of" in str(e)
    else:
        raise AssertionError("GVN attr key row accepted none_singleton")

    attr_missing_attrs = json.loads(json.dumps(data))
    attr_missing_attrs["gvn_numberable_attr_key_opcodes"][0]["attrs"] = []
    try:
        gen._validate_gvn_numberable_attr_key_facts(attr_missing_attrs, opcodes)
    except gen.OpKindTableError as e:
        assert "requires a non-empty attrs list" in str(e)
    else:
        raise AssertionError("GVN attr key row without attrs was accepted")


def test_alias_barrier_predicates_delegate_to_generated_tables() -> None:
    """Alias-analysis opcode-only barrier facts belong in the generated registry.

    The consumer may layer operand/root checks on top, but the RC and arbitrary
    heap opcode sets must not live as hand-maintained `matches!` lists in
    alias_analysis.rs.
    """
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    alias = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/alias_analysis.rs")

    expected_rc = {
        "Call",
        "CallBuiltin",
        "CallMethod",
        "CallMethodIc",
        "CallSuperMethodIc",
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
        "CallMethodIc",
        "CallSuperMethodIc",
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
    deforestation = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/deforestation.rs")

    expected = {
        "Call",
        "CallBuiltin",
        "CallMethod",
        "CallMethodIc",
        "CallSuperMethodIc",
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
        "enum GeneratorFusionPollRole"
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


def test_polyhedral_opcodes_delegate_to_generated_tables() -> None:
    """Polyhedral loop classification belongs to generated opcode facts."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    polyhedral = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/polyhedral.rs")

    loop_headers = {"ForIter", "ScfFor"}
    affine_body = {
        "Add",
        "Sub",
        "Mul",
        "Div",
        "FloorDiv",
        "Mod",
        "Index",
        "StoreIndex",
        "ConstInt",
        "ConstFloat",
        "ConstBool",
        "ConstNone",
        "Lt",
        "Le",
        "Gt",
        "Ge",
        "Eq",
        "Ne",
        "ForIter",
        "ScfFor",
        "ScfYield",
        "GetIter",
        "IterNext",
    }
    assert set(data["polyhedral_loop_header_opcodes"]) == loop_headers
    assert set(data["polyhedral_affine_body_opcodes"]) == affine_body
    assert "Copy" not in affine_body

    header_block = rendered.split("fn opcode_is_polyhedral_loop_header_table")[1].split(
        "fn opcode_is_polyhedral_affine_body_table"
    )[0]
    affine_block = rendered.split("fn opcode_is_polyhedral_affine_body_table")[1].split(
        "fn opcode_is_refcount_heap_exposure_table"
    )[0]
    for opcode in loop_headers:
        assert f"OpCode::{opcode} => true," in header_block
    assert "OpCode::Call => false," in header_block
    for opcode in affine_body:
        assert f"OpCode::{opcode} => true," in affine_block
    for opcode in {"Call", "BuildList", "Copy", "StateYield"}:
        assert f"OpCode::{opcode} => false," in affine_block

    production = polyhedral.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_is_polyhedral_loop_header_table" in production
    assert "opcode_is_polyhedral_affine_body_table" in production
    assert "op.is_plain_value_copy()" in production
    assert "matches!" not in production
    assert "OpCode::" not in production


def test_generator_fusion_poll_roles_delegate_to_generated_table() -> None:
    """Generator poll-body eligibility has one opcode-role authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    generator_fusion = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/generator_fusion.rs")

    required = {"StateYield"}
    reject = {
        "AllocTask",
        "ChanRecvYield",
        "ChanSendYield",
        "StateBlockEnd",
        "StateBlockStart",
        "StateTransition",
        "Yield",
        "YieldFrom",
    }
    assert set(data["generator_fusion_poll_required_yield_opcodes"]) == required
    assert set(data["generator_fusion_poll_reject_opcodes"]) == reject
    assert required.isdisjoint(reject)
    assert "StateSwitch" not in required | reject
    iter_use_roles = [
        {"opcode": "IterNext", "role": "next_use"},
        {"opcode": "Is", "role": "none_guard"},
    ]
    assert data["generator_fusion_iter_use_roles"] == iter_use_roles

    assert "pub enum GeneratorFusionPollRole" in rendered
    assert "pub fn opcode_generator_fusion_poll_role_table" in rendered
    table_block = rendered.split("fn opcode_generator_fusion_poll_role_table")[1].split(
        "pub enum GeneratorFusionIterUseRole"
    )[0]
    for row in data["opcode"]:
        if row["name"] in required:
            role = "GeneratorFusionPollRole::RequiredYield"
        elif row["name"] in reject:
            role = "GeneratorFusionPollRole::Reject"
        else:
            role = "GeneratorFusionPollRole::Neutral"
        assert f"OpCode::{row['name']} => {role}," in table_block
    iter_use_block = rendered.split("fn opcode_generator_fusion_iter_use_role_table")[
        1
    ].split("fn opcode_is_state_machine_table")[0]
    assert "OpCode::IterNext => GeneratorFusionIterUseRole::NextUse," in iter_use_block
    assert "OpCode::Is => GeneratorFusionIterUseRole::NoneGuard," in iter_use_block
    assert "OpCode::Add => GeneratorFusionIterUseRole::None," in iter_use_block

    production = generator_fusion.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_generator_fusion_poll_role_table" in production
    body = production.split("fn is_poll_fusable", maxsplit=1)[1].split(
        "fn entry_has_predecessor", maxsplit=1
    )[0]
    assert "opcode_generator_fusion_poll_role_table(op.opcode)" in body
    assert "rejects_fusion()" in body
    assert "is_required_yield()" in body
    assert "match op.opcode" not in body
    assert "OpCode::StateYield" not in body
    assert "OpCode::YieldFrom" not in body
    assert "OpCode::AllocTask" not in body
    assert ".filter(|op| op.opcode == OpCode::StateYield)" not in production
    iter_body = production.split(
        "fn iter_uses_are_next_and_optional_none_guard", maxsplit=1
    )[1].split("fn terminator_uses", maxsplit=1)[0]
    assert "opcode_generator_fusion_iter_use_role_table(op.opcode)" in iter_body
    assert "GeneratorFusionIterUseRole::NextUse" in iter_body
    assert "GeneratorFusionIterUseRole::NoneGuard" in iter_body
    assert "match op.opcode" not in iter_body
    assert "OpCode::IterNext if" not in iter_body
    assert "OpCode::Is =>" not in iter_body


def test_generator_fusion_iter_use_role_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_role = json.loads(json.dumps(data))
    bad_role["generator_fusion_iter_use_roles"][0]["role"] = "none_guard"
    try:
        gen._validate_generator_fusion_iter_use_roles(bad_role, opcodes)
    except gen.OpKindTableError as exc:
        assert "reserved for OpCode::Is" in str(exc)
    else:
        raise AssertionError("IterNext was accepted with the Is guard role")

    duplicate_role = json.loads(json.dumps(data))
    duplicate_role["generator_fusion_iter_use_roles"].append(
        json.loads(json.dumps(duplicate_role["generator_fusion_iter_use_roles"][0]))
    )
    try:
        gen._validate_generator_fusion_iter_use_roles(duplicate_role, opcodes)
    except gen.OpKindTableError as exc:
        assert "duplicate generator_fusion_iter_use_roles opcode: IterNext" in str(exc)
    else:
        raise AssertionError("duplicate generator-fusion iter role was accepted")


def test_lowered_state_machine_body_opcodes_delegate_to_generated_table() -> None:
    """Lowered coroutine body detection belongs to the op-kind registry."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    function = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/function.rs")

    expected = {
        "AllocTask",
        "ChanRecvYield",
        "ChanSendYield",
        "StateSwitch",
        "StateTransition",
        "StateYield",
    }
    assert set(data["lowered_state_machine_body_opcodes"]) == expected
    assert expected < set(data["state_machine_opcodes"])
    assert {"StateBlockStart", "StateBlockEnd", "Yield", "YieldFrom"}.isdisjoint(
        expected
    )

    table_block = rendered.split("fn opcode_is_lowered_state_machine_body_table")[
        1
    ].split("fn opcode_is_drop_insertion_suspension_point_table")[0]
    for row in data["opcode"]:
        expected_bool = "true" if row["name"] in expected else "false"
        assert f"OpCode::{row['name']} => {expected_bool}," in table_block

    table_name = "opcode_is_lowered_state_machine_body_table"
    assert table_name in function
    marker = "pub fn has_state_machine("
    start = function.index(marker)
    brace = function.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(function)):
        if function[i] == "{":
            depth += 1
        elif function[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = function[start:end]
    assert "Terminator::StateDispatch" in body
    assert f"{table_name}(op.opcode)" in body
    assert "OpCode::" not in body


def test_drop_insertion_suspension_points_delegate_to_generated_table() -> None:
    """Drop insertion's suspension retain points have one opcode authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    drop_insertion = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")

    expected = {
        "ChanRecvYield",
        "ChanSendYield",
        "StateYield",
        "Yield",
        "YieldFrom",
    }
    assert set(data["drop_insertion_suspension_point_opcodes"]) == expected
    assert expected < set(data["state_machine_opcodes"])
    assert {"AllocTask", "StateSwitch", "StateTransition"}.isdisjoint(expected)

    table_block = rendered.split("fn opcode_is_drop_insertion_suspension_point_table")[
        1
    ].split("fn opcode_is_drop_insertion_return_deferral_barrier_table")[0]
    for row in data["opcode"]:
        expected_bool = "true" if row["name"] in expected else "false"
        assert f"OpCode::{row['name']} => {expected_bool}," in table_block

    table_name = "opcode_is_drop_insertion_suspension_point_table"
    assert table_name in drop_insertion
    start = drop_insertion.index("fn is_suspension_point(")
    brace = drop_insertion.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(drop_insertion)):
        if drop_insertion[i] == "{":
            depth += 1
        elif drop_insertion[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = drop_insertion[start:end]
    assert f"{table_name}(opcode)" in body
    assert "matches!" not in body
    assert "OpCode::" not in body


def test_drop_insertion_return_deferral_barriers_delegate_to_generated_table() -> None:
    """Return-boundary deferral barriers have one opcode authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    drop_insertion = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")

    expected = {"DecRef", "Free", "IncRef"}
    assert set(data["drop_insertion_return_deferral_barrier_opcodes"]) == expected
    assert "DelBoundary" not in expected
    assert expected.isdisjoint(data["drop_insertion_suspension_point_opcodes"])

    table_block = rendered.split(
        "fn opcode_is_drop_insertion_return_deferral_barrier_table"
    )[1].split("fn opcode_is_fusion_barrier_table")[0]
    for row in data["opcode"]:
        expected_bool = "true" if row["name"] in expected else "false"
        assert f"OpCode::{row['name']} => {expected_bool}," in table_block

    table_name = "opcode_is_drop_insertion_return_deferral_barrier_table"
    assert table_name in drop_insertion
    start = drop_insertion.index("fn is_return_deferral_barrier(")
    brace = drop_insertion.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(drop_insertion)):
        if drop_insertion[i] == "{":
            depth += 1
        elif drop_insertion[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = drop_insertion[start:end]
    assert f"{table_name}(opcode)" in body
    assert "matches!" not in body
    assert "OpCode::" not in body

    scan = drop_insertion.split("let mut disqualified: HashSet<ValueId>", maxsplit=1)[
        1
    ].split("Gate (c) transfer rail", maxsplit=1)[0]
    assert "is_return_deferral_barrier(op.opcode)" in scan
    assert "OpCode::IncRef | OpCode::DecRef | OpCode::Free" not in scan
    assert "matches!(op.opcode" not in scan


def test_state_machine_opcodes_delegate_to_generated_table() -> None:
    """Linear transform state-machine exclusions belong to the op-kind registry."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    inliner = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/inliner.rs")
    promotion = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/module_slot_promotion.rs")

    expected = {
        "AllocTask",
        "ChanRecvYield",
        "ChanSendYield",
        "StateBlockEnd",
        "StateBlockStart",
        "StateSwitch",
        "StateTransition",
        "StateYield",
        "Yield",
        "YieldFrom",
    }
    assert set(data["state_machine_opcodes"]) == expected

    table_block = rendered.split("fn opcode_is_state_machine_table")[1].split(
        "fn opcode_module_concurrency_marker_source_facts_table"
    )[0]
    for row in data["opcode"]:
        expected_bool = "true" if row["name"] in expected else "false"
        assert f"OpCode::{row['name']} => {expected_bool}," in table_block

    table_name = "opcode_is_state_machine_table"
    for source, helper in (
        (inliner, "fn is_generator_or_async_op("),
        (promotion, "fn is_state_machine_op("),
    ):
        assert table_name in source
        start = source.index(helper)
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
        assert f"{table_name}(opcode)" in body
        assert "matches!" not in body
        assert "OpCode::" not in body


def test_module_slot_promotion_roles_delegate_to_generated_tables() -> None:
    """Module-slot promotion opcode roles belong to the op-kind registry."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    promotion = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/module_slot_promotion.rs")
    production = promotion.split("#[cfg(test)]", maxsplit=1)[0]

    assert data["module_concurrency_marker_source_roles"] == [
        {
            "opcode": "Import",
            "role": "module_name",
            "attrs": ["s_value", "name"],
        },
        {
            "opcode": "ImportFrom",
            "role": "module_name",
            "attrs": ["s_value", "name"],
        },
        {
            "opcode": "Call",
            "role": "thread_intrinsic_callee",
            "attrs": ["s_value", "name"],
        },
        {
            "opcode": "CallBuiltin",
            "role": "thread_intrinsic_callee",
            "attrs": ["s_value", "name"],
        },
    ]
    assert data["module_slot_access_roles"] == [
        {"opcode": "ModuleGetAttr", "role": "keyed_attr"},
        {"opcode": "ModuleSetAttr", "role": "keyed_attr"},
        {"opcode": "ModuleGetGlobal", "role": "wildcard_module_dict"},
        {"opcode": "ModuleDelGlobal", "role": "wildcard_module_dict"},
        {"opcode": "ModuleDelGlobalIfPresent", "role": "wildcard_module_dict"},
    ]

    assert _rust_pub_decl(rendered, "enum", "ModuleConcurrencyMarkerSourceRole")
    assert _rust_pub_decl(rendered, "struct", "ModuleConcurrencyMarkerSourceFacts")
    assert _rust_pub_fn(
        rendered, "opcode_module_concurrency_marker_source_facts_table"
    )
    assert _rust_pub_decl(rendered, "enum", "ModuleSlotAccessRole")
    assert _rust_pub_fn(rendered, "opcode_module_slot_access_role_table")

    concurrency_table = rendered.split(
        "fn opcode_module_concurrency_marker_source_facts_table"
    )[1].split("pub enum ModuleSlotAccessRole")[0]
    assert "OpCode::Import => ModuleConcurrencyMarkerSourceFacts" in concurrency_table
    assert "role: ModuleConcurrencyMarkerSourceRole::ModuleName" in concurrency_table
    assert "attrs: MODULE_CONCURRENCY_MARKER_ATTRS_IMPORT" in concurrency_table
    assert "OpCode::Call => ModuleConcurrencyMarkerSourceFacts" in concurrency_table
    assert (
        "role: ModuleConcurrencyMarkerSourceRole::ThreadIntrinsicCallee"
        in concurrency_table
    )
    assert "attrs: MODULE_CONCURRENCY_MARKER_ATTRS_CALL" in concurrency_table
    assert "OpCode::ModuleCacheGet => MODULE_CONCURRENCY_MARKER_SOURCE_NONE" in (
        concurrency_table
    )

    access_table = rendered.split("fn opcode_module_slot_access_role_table")[1].split(
        "fn opcode_sets_exception_handling_table"
    )[0]
    assert "OpCode::ModuleGetAttr => ModuleSlotAccessRole::KeyedAttr" in access_table
    assert "OpCode::ModuleSetAttr => ModuleSlotAccessRole::KeyedAttr" in access_table
    assert (
        "OpCode::ModuleGetGlobal => ModuleSlotAccessRole::WildcardModuleDict"
        in access_table
    )
    assert "OpCode::ModuleCacheGet => ModuleSlotAccessRole::None" in access_table

    assert "opcode_module_concurrency_marker_source_facts_table" in production
    assert "opcode_module_slot_access_role_table" in production
    for helper in (
        "fn module_has_concurrency_markers(",
        "fn single_module_root(",
        "fn is_wildcard_module_op(",
    ):
        body = _rust_fn_body(production, helper)
        assert "OpCode::" not in body
        assert "matches!" not in body

    assert "OpCode::Import | OpCode::ImportFrom" not in production
    assert "OpCode::Call | OpCode::CallBuiltin" not in production
    assert "OpCode::ModuleGetAttr | OpCode::ModuleSetAttr" not in production
    assert (
        "OpCode::ModuleGetGlobal | OpCode::ModuleDelGlobal | "
        "OpCode::ModuleDelGlobalIfPresent"
        not in production
    )
    assert "matches!(op.opcode, OpCode::ModuleGetAttr" not in production


def test_residual_tir_semantic_roles_delegate_to_generated_tables() -> None:
    """Residual TIR semantic opcode roles belong to the op-kind registry."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)

    overflow_peel = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/overflow_peel.rs")
    verify = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/verify.rs")
    sroa = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/sroa.rs")
    strength = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/strength_reduction.rs")
    scev = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/scev.rs")

    assert data["tir_verify_attr_rules"] == [
        {"opcode": "Call", "rule": "call_callee"},
        {"opcode": "CallBuiltin", "rule": "call_callee"},
        {"opcode": "CallMethod", "rule": "call_method"},
        {"opcode": "CallMethodIc", "rule": "call_method"},
        {"opcode": "CallSuperMethodIc", "rule": "call_method"},
        {"opcode": "ObjectNewBoundStack", "rule": "positive_payload_bytes"},
    ]
    assert data["sroa_const_immediate_rules"] == [
        {"opcode": "ConstNone", "rule": "always_immediate"},
        {"opcode": "ConstBool", "rule": "always_immediate"},
        {"opcode": "ConstFloat", "rule": "always_immediate"},
        {"opcode": "ConstInt", "rule": "inline_int_if_range"},
    ]
    assert data["strength_reduction_rules"] == [
        {"opcode": "Mul", "rule": "mul_by_two"},
        {"opcode": "Pow", "rule": "pow_square"},
        {"opcode": "FloorDiv", "rule": "power_two_floor_div"},
        {"opcode": "Mod", "rule": "power_two_mod"},
    ]
    assert data["scev_expr_rules"] == [
        {"opcode": "Add", "rule": "add"},
        {"opcode": "Sub", "rule": "sub"},
        {"opcode": "Mul", "rule": "mul"},
    ]

    for enum_name, fn_name in (
        ("TirVerifyAttrRule", "opcode_tir_verify_attr_rule_table"),
        ("SroaConstImmediateRule", "opcode_sroa_const_immediate_rule_table"),
        ("StrengthReductionRule", "opcode_strength_reduction_rule_table"),
        ("ScevExprRule", "opcode_scev_expr_rule_table"),
    ):
        assert _rust_pub_decl(rendered, "enum", enum_name)
        assert _rust_pub_fn(rendered, fn_name)

    rendered_expectations = {
        "OpCode::Call => TirVerifyAttrRule::CallCallee": rendered,
        "OpCode::ObjectNewBoundStack => TirVerifyAttrRule::PositivePayloadBytes": rendered,
        "OpCode::ConstNone => SroaConstImmediateRule::AlwaysImmediate": rendered,
        "OpCode::ConstInt => SroaConstImmediateRule::InlineIntIfRange": rendered,
        "OpCode::FloorDiv => StrengthReductionRule::PowerTwoFloorDiv": rendered,
        "OpCode::Mod => StrengthReductionRule::PowerTwoMod": rendered,
        "OpCode::Add => ScevExprRule::Add": rendered,
        "OpCode::Sub => ScevExprRule::Sub": rendered,
        "OpCode::ModuleCacheGet => ScevExprRule::None": rendered,
    }
    for needle, haystack in rendered_expectations.items():
        assert needle in haystack

    overflow_production = overflow_peel.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_is_state_machine_table" in overflow_production
    assert "OpCode::StateSwitch" not in overflow_production
    assert "matches!(\n                op.opcode" not in overflow_production

    verify_body = _rust_fn_body(verify, "fn verify_op_attributes(")
    assert "opcode_tir_verify_attr_rule_table(op.opcode)" in verify_body
    assert "OpCode::Call | OpCode::CallBuiltin" not in verify_body
    assert "OpCode::ObjectNewBoundStack" not in verify_body

    sroa_body = _rust_fn_body(sroa, "fn collect_const_immediates(")
    assert "opcode_sroa_const_immediate_rule_table(op.opcode)" in sroa_body
    assert "OpCode::ConstNone | OpCode::ConstBool | OpCode::ConstFloat" not in sroa_body
    assert "OpCode::ConstInt if" not in sroa_body

    strength_production = strength.split("#[cfg(test)]", maxsplit=1)[0]
    assert "opcode_strength_reduction_rule_table(op.opcode)" in strength_production
    assert "match op.opcode" not in strength_production
    assert "PowerTwoFloorDiv" in strength_production
    assert "PowerTwoMod" in strength_production
    assert "Phase 3" not in strength_production

    scev_body = _rust_fn_body(scev, "fn scev_of_op(")
    assert "opcode_scev_expr_rule_table(opcode)" in scev_body
    assert "OpCode::Add if" not in scev_body
    assert "OpCode::Sub if" not in scev_body
    assert "OpCode::Mul if" not in scev_body


def test_refcount_heap_exposure_delegates_to_generated_table() -> None:
    """Deferred-RC heap exposure has one opcode authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    refcount = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/refcount_elim.rs")

    expected = {
        "AllocTask",
        "BuildDict",
        "BuildList",
        "BuildSet",
        "BuildSlice",
        "BuildTuple",
        "Call",
        "CallBuiltin",
        "CallMethod",
        "CallMethodIc",
        "CallSuperMethodIc",
        "ChanRecvYield",
        "ChanSendYield",
        "ClosureStore",
        "Import",
        "ImportFrom",
        "Raise",
        "StateYield",
        "StoreAttr",
        "StoreIndex",
        "Yield",
        "YieldFrom",
    }
    assert set(data["refcount_heap_exposure_opcodes"]) == expected

    table_block = rendered.split("fn opcode_is_refcount_heap_exposure_table")[1].split(
        "fn opcode_is_fusion_barrier_table"
    )[0]
    for opcode in expected:
        assert f"OpCode::{opcode} => true," in table_block
    for opcode in {"Free", "DelAttr", "ModuleCacheSet", "Add"}:
        assert f"OpCode::{opcode} => false," in table_block

    table_name = "opcode_is_refcount_heap_exposure_table"
    assert table_name in refcount
    start = refcount.index("fn is_heap_exposing(")
    brace = refcount.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(refcount)):
        if refcount[i] == "{":
            depth += 1
        elif refcount[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = refcount[start:end]
    assert table_name in body
    assert "matches!" not in body
    assert "OpCode::" not in body


def test_escape_alloc_sites_delegate_to_generated_table() -> None:
    """Escape-analysis allocation roots have one opcode authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    escape_analysis = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/escape_analysis.rs")

    expected = {
        "Alloc",
        "ObjectNewBound",
        "BuildList",
        "BuildDict",
        "BuildTuple",
        "BuildSet",
        "AllocTask",
    }
    assert set(data["escape_alloc_site_opcodes"]) == expected

    table_block = rendered.split("fn opcode_is_escape_alloc_site_table")[1].split(
        "fn opcode_is_refcount_heap_exposure_table"
    )[0]
    for row in data["opcode"]:
        opcode = row["name"]
        expected_bool = "true" if opcode in expected else "false"
        assert f"OpCode::{opcode} => {expected_bool}," in table_block

    table_name = "opcode_is_escape_alloc_site_table"
    assert table_name in escape_analysis
    start = escape_analysis.index("fn is_alloc_site(")
    brace = escape_analysis.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(escape_analysis)):
        if escape_analysis[i] == "{":
            depth += 1
        elif escape_analysis[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = escape_analysis[start:end]
    assert f"{table_name}(opcode)" in body
    assert "matches!" not in body
    assert "OpCode::" not in body


def test_refcount_balance_roles_delegate_to_generated_table() -> None:
    """Refcount balance accounting has one opcode-role authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    refcount = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/refcount_elim.rs")

    assert set(data["refcount_balance_inc_opcodes"]) == {"IncRef"}
    assert set(data["refcount_balance_dec_opcodes"]) == {"DecRef"}
    assert "pub enum RefcountBalanceRole" in rendered
    assert "RefcountBalanceRole::Increment => 1," in rendered
    assert "RefcountBalanceRole::Decrement => -1," in rendered

    table_block = rendered.split("fn opcode_refcount_balance_role_table")[1].split(
        "fn opcode_is_lowered_state_machine_body_table"
    )[0]
    for row in data["opcode"]:
        if row["name"] == "IncRef":
            role = "RefcountBalanceRole::Increment"
        elif row["name"] == "DecRef":
            role = "RefcountBalanceRole::Decrement"
        else:
            role = "RefcountBalanceRole::NotRefcountBalance"
        assert f"OpCode::{row['name']} => {role}," in table_block

    production = refcount.split("// Tests", maxsplit=1)[0]
    table_name = "opcode_refcount_balance_role_table"
    assert table_name in production
    assert "fn refcount_balance_role(" in production
    assert "fn is_refcount_balance_op(" in production
    assert "fn complementary_refcount_opcode(" in production
    assert (
        "op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef" not in production
    )
    assert (
        "op_i.opcode != OpCode::IncRef && op_i.opcode != OpCode::DecRef"
        not in production
    )
    assert "OpCode::IncRef =>" not in production
    assert "OpCode::DecRef =>" not in production


def test_i64_arithmetic_lowering_facts_delegate_to_generated_tables() -> None:
    """Raw-i64 arithmetic lowering policy belongs to the op-kind registry."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    lower_to_lir = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/lower_to_lir.rs")

    overflow_box_dispatch = {
        "Add",
        "Div",
        "FloorDiv",
        "InplaceAdd",
        "InplaceMul",
        "InplaceSub",
        "Mod",
        "Mul",
        "Sub",
    }
    checked_triples = {"Add", "Mul", "Sub"}
    assert set(data["i64_overflow_box_dispatch_opcodes"]) == overflow_box_dispatch
    assert set(data["i64_checked_overflow_triple_opcodes"]) == checked_triples
    assert checked_triples < overflow_box_dispatch

    overflow_table_block = rendered.split(
        "fn opcode_requires_i64_overflow_box_dispatch_table"
    )[1].split("fn opcode_supports_i64_checked_overflow_triple_table")[0]
    checked_table_block = rendered.split(
        "fn opcode_supports_i64_checked_overflow_triple_table"
    )[1].split("fn opcode_uses_boxed_runtime_inplace_dispatch_table")[0]
    for row in data["opcode"]:
        overflow_bool = "true" if row["name"] in overflow_box_dispatch else "false"
        checked_bool = "true" if row["name"] in checked_triples else "false"
        assert f"OpCode::{row['name']} => {overflow_bool}," in overflow_table_block
        assert f"OpCode::{row['name']} => {checked_bool}," in checked_table_block

    overflow_table_name = "opcode_requires_i64_overflow_box_dispatch_table"
    checked_table_name = "opcode_supports_i64_checked_overflow_triple_table"
    assert overflow_table_name in lower_to_lir
    assert checked_table_name in lower_to_lir

    start = lower_to_lir.index("fn lower_op(")
    brace = lower_to_lir.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(lower_to_lir)):
        if lower_to_lir[i] == "{":
            depth += 1
        elif lower_to_lir[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    lower_body = lower_to_lir[start:end]
    assert overflow_table_name in lower_body
    assert "OpCode::InplaceAdd" not in lower_body
    assert "OpCode::FloorDiv" not in lower_body

    start = lower_to_lir.index("fn lowers_to_checked_i64_arithmetic(")
    brace = lower_to_lir.index("{", start)
    depth = 0
    end = brace
    for i in range(brace, len(lower_to_lir)):
        if lower_to_lir[i] == "{":
            depth += 1
        elif lower_to_lir[i] == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    helper_body = lower_to_lir[start:end]
    assert checked_table_name in helper_body
    assert "OpCode::Add | OpCode::Sub | OpCode::Mul" not in helper_body


def test_llvm_boxed_runtime_inplace_dispatch_delegates_to_generated_table() -> None:
    """First-class augassign opcodes must route boxed LLVM fallback via registry."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    llvm_lowering = _read_rs_module_cluster(ROOT / "runtime/molt-backend/src/llvm_backend/lowering.rs")

    expected = {"InplaceAdd", "InplaceSub", "InplaceMul"}
    assert set(data["boxed_runtime_inplace_dispatch_opcodes"]) == expected

    table_block = rendered.split("fn opcode_uses_boxed_runtime_inplace_dispatch_table")[
        1
    ].split("fn opcode_requires_i64_zero_divisor_guard_table")[0]
    for row in data["opcode"]:
        expected_bool = "true" if row["name"] in expected else "false"
        assert f"OpCode::{row['name']} => {expected_bool}," in table_block

    table_name = "opcode_uses_boxed_runtime_inplace_dispatch_table"
    body = _rust_fn_body(llvm_lowering, "fn emit_binary_arith(")
    assert table_name in body
    assert 'k.starts_with("inplace_")' in body
    assert "matches!(\n                    op.opcode" not in body
    assert "OpCode::InplaceAdd | OpCode::InplaceSub | OpCode::InplaceMul" not in body


def test_i64_zero_divisor_guards_delegate_to_generated_table() -> None:
    """Raw-i64 zero-divisor proof requirements have one opcode authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    lower_to_lir = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/lower_to_lir.rs")
    licm = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/licm.rs")
    check_exception = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/check_exception_elim.rs")

    zero_divisor_guards = {"Div", "FloorDiv", "Mod"}
    shift_count_guards = {"Shl", "Shr"}
    assert set(data["i64_zero_divisor_guard_opcodes"]) == zero_divisor_guards
    assert set(data["i64_shift_count_guard_opcodes"]) == shift_count_guards

    zero_table_block = rendered.split(
        "fn opcode_requires_i64_zero_divisor_guard_table"
    )[1].split("fn opcode_requires_i64_shift_count_guard_table")[0]
    for opcode in zero_divisor_guards:
        assert f"OpCode::{opcode} => true," in zero_table_block
    for opcode in {"Add", "Mul", "Pow"}:
        assert f"OpCode::{opcode} => false," in zero_table_block

    shift_table_block = rendered.split(
        "fn opcode_requires_i64_shift_count_guard_table"
    )[1].split("fn opcode_has_exception_label_attr_table")[0]
    for opcode in shift_count_guards:
        assert f"OpCode::{opcode} => true," in shift_table_block
    for opcode in {"Div", "Mod", "Pow"}:
        assert f"OpCode::{opcode} => false," in shift_table_block

    zero_table_name = "opcode_requires_i64_zero_divisor_guard_table"
    shift_table_name = "opcode_requires_i64_shift_count_guard_table"
    assert zero_table_name in lower_to_lir
    assert zero_table_name in check_exception
    assert zero_table_name in licm
    assert shift_table_name in licm

    for source, fn_name in (
        (lower_to_lir, "fn lower_op("),
        (check_exception, "fn op_may_raise("),
        (licm, "fn throw_condition_disproven("),
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
        assert zero_table_name in body
        assert "OpCode::Div | OpCode::FloorDiv | OpCode::Mod" not in body
        if source == licm:
            assert shift_table_name in body
            assert "OpCode::Shl | OpCode::Shr" not in body


def test_exception_label_opcode_facts_delegate_to_generated_tables() -> None:
    """Exception metadata opcode roles have one registry authority."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    sources = {
        "inliner": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/inliner.rs"),
        "generator_fusion": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/generator_fusion.rs"),
        "lower_to_simple": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/lower_to_simple.rs"),
        "lower_from_simple": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/lower_from_simple.rs"),
        "function": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/function.rs"),
        "dominators": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/dominators.rs"),
        "dce": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/dce.rs"),
        "sccp": _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/sccp.rs"),
    }

    label_attr = {"CheckException", "TryStart", "TryEnd"}
    transfer_edge = {"CheckException", "TryStart"}
    exception_handling = {
        "CheckException",
        "TryStart",
        "TryEnd",
        "StateBlockStart",
        "StateBlockEnd",
    }
    handler_regions = {"TryStart", "TryEnd", "StateBlockStart", "StateBlockEnd"}
    structured_scf = {"ScfIf", "ScfFor", "ScfWhile", "ScfYield"}
    nesting_roles = [
        {"opcode": "TryStart", "role": "enter"},
        {"opcode": "TryEnd", "role": "exit"},
    ]
    assert set(data["exception_label_attr_opcodes"]) == label_attr
    assert set(data["exception_transfer_edge_opcodes"]) == transfer_edge
    assert set(data["exception_handling_opcodes"]) == exception_handling
    assert set(data["exception_handler_region_opcodes"]) == handler_regions
    assert set(data["structured_scf_marker_opcodes"]) == structured_scf
    assert data["exception_region_nesting_roles"] == nesting_roles

    handling_block = rendered.split("fn opcode_sets_exception_handling_table")[
        1
    ].split("fn opcode_is_exception_handler_region_table")[0]
    handler_block = rendered.split("fn opcode_is_exception_handler_region_table")[
        1
    ].split("fn opcode_is_structured_scf_marker_table")[0]
    scf_block = rendered.split("fn opcode_is_structured_scf_marker_table")[1].split(
        "fn opcode_requires_i64_overflow_box_dispatch_table"
    )[0]
    label_block = rendered.split("fn opcode_has_exception_label_attr_table")[1].split(
        "fn opcode_is_exception_transfer_edge_table"
    )[0]
    transfer_block = rendered.split("fn opcode_is_exception_transfer_edge_table")[
        1
    ].split("enum ExceptionRegionNestingRole")[0]
    nesting_block = rendered.split("enum ExceptionRegionNestingRole")[1].split(
        "enum AliasTypedSlotRole"
    )[0]
    for opcode in exception_handling:
        assert f"OpCode::{opcode} => true," in handling_block
    assert "OpCode::ExceptionPending => false," in handling_block
    for opcode in handler_regions:
        assert f"OpCode::{opcode} => true," in handler_block
    assert "OpCode::CheckException => false," in handler_block
    for opcode in structured_scf:
        assert f"OpCode::{opcode} => true," in scf_block
    assert "OpCode::ForIter => false," in scf_block
    for opcode in label_attr:
        assert f"OpCode::{opcode} => true," in label_block
    assert "OpCode::ExceptionPending => false," in label_block
    for opcode in transfer_edge:
        assert f"OpCode::{opcode} => true," in transfer_block
    assert "OpCode::TryEnd => false," in transfer_block
    assert "pub fn opcode_exception_region_nesting_role_table" in nesting_block
    assert "OpCode::TryStart => ExceptionRegionNestingRole::Enter," in nesting_block
    assert "OpCode::TryEnd => ExceptionRegionNestingRole::Exit," in nesting_block
    assert (
        "OpCode::CheckException => ExceptionRegionNestingRole::None," in nesting_block
    )

    assert "opcode_has_exception_label_attr_table" in sources["inliner"]
    assert "opcode_has_exception_label_attr_table" in sources["generator_fusion"]
    assert "opcode_has_exception_label_attr_table" in sources["lower_to_simple"]
    assert "opcode_is_exception_transfer_edge_table" in sources["dominators"]
    assert "opcode_exception_region_nesting_role_table" in sources["dce"]
    assert "ExceptionRegionNestingRole" in sources["dce"]
    assert "opcode_exception_region_nesting_role_table" in sources["sccp"]
    assert "ExceptionRegionNestingRole" in sources["sccp"]
    assert "opcode_sets_exception_handling_table" in sources["lower_from_simple"]
    assert "opcode_is_exception_handler_region_table" in sources["function"]
    assert "opcode_is_structured_scf_marker_table" in sources["lower_to_simple"]

    stale_literal = "OpCode::CheckException | OpCode::TryStart | OpCode::TryEnd"
    for name, source in sources.items():
        if name == "dce":
            source = source.split("#[cfg(test)]", maxsplit=1)[0]
        assert stale_literal not in source
    assert (
        "matches!(opcode, OpCode::CheckException | OpCode::TryStart)"
        not in sources["dominators"]
    )
    dce_production = sources["dce"].split("#[cfg(test)]", maxsplit=1)[0]
    assert "OpCode::TryStart =>" not in dce_production
    assert "OpCode::TryEnd =>" not in dce_production
    sccp_production = sources["sccp"].split("#[cfg(test)]", maxsplit=1)[0]
    assert "OpCode::TryStart =>" not in sccp_production
    assert "OpCode::TryEnd =>" not in sccp_production


def test_exception_region_nesting_role_validation_rejects_drift() -> None:
    gen = _gen()
    data = gen.load_table()
    opcodes = {row["name"] for row in data["opcode"]}

    bad_role = json.loads(json.dumps(data))
    bad_role["exception_region_nesting_roles"][0]["role"] = "exit"
    try:
        gen._validate_exception_region_nesting_roles(bad_role, opcodes)
    except gen.OpKindTableError as exc:
        assert "reserved for OpCode::TryEnd" in str(exc)
    else:
        raise AssertionError("TryStart was accepted with the TryEnd nesting role")


def test_alias_slot_observation_delegates_to_generated_table() -> None:
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    alias = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/alias_analysis.rs")

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
        "CallMethodIc",
        "CallSuperMethodIc",
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
    alias = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/alias_analysis.rs")

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
        "CheckedMul",
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

    transparent_block = rendered.split("fn opcode_alias_transparent_alias_role_table")[
        1
    ].split("enum AliasMemoryRegionClass")[0]
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


def test_simpleir_control_kind_validation_rejects_invalid_rows() -> None:
    gen = _gen()
    data = gen.load_table()

    def expect_error(mutated: dict, needle: str) -> None:
        try:
            gen._validate_simpleir_control_kinds(mutated)
        except gen.OpKindTableError as exc:
            assert needle in str(exc)
        else:
            raise AssertionError(
                f"invalid simpleir_control_kind row accepted: {needle}"
            )

    duplicate = json.loads(json.dumps(data))
    duplicate["simpleir_control_kind"].append(
        json.loads(json.dumps(duplicate["simpleir_control_kind"][0]))
    )
    expect_error(duplicate, "duplicate simpleir_control_kind")

    bad_bool = json.loads(json.dumps(data))
    bad_bool["simpleir_control_kind"][0]["structural"] = "yes"
    expect_error(bad_bool, "must be a bool")

    unknown = json.loads(json.dumps(data))
    unknown["simpleir_control_kind"][0]["ambient"] = True
    expect_error(unknown, "unknown fields")

    ssa_overlap = json.loads(json.dumps(data))
    for row in ssa_overlap["simpleir_control_kind"]:
        if row["kind"] == "phi":
            row["structural"] = True
            break
    expect_error(ssa_overlap, "ssa_only cannot overlap")

    repoll_without_suspend = json.loads(json.dumps(data))
    for row in repoll_without_suspend["simpleir_control_kind"]:
        if row["kind"] == "state_transition":
            row["suspend"] = False
            break
    expect_error(repoll_without_suspend, "repoll requires suspend")

    suspend_without_ender = json.loads(json.dumps(data))
    for row in suspend_without_ender["simpleir_control_kind"]:
        if row["kind"] == "state_yield":
            row["block_ender"] = False
            break
    expect_error(suspend_without_ender, "suspend requires block_ender")

    terminator_without_structural = json.loads(json.dumps(data))
    for row in terminator_without_structural["simpleir_control_kind"]:
        if row["kind"] == "ret":
            row["structural"] = False
            break
    expect_error(terminator_without_structural, "terminator requires structural")

    no_fact = json.loads(json.dumps(data))
    for field in gen._SIMPLEIR_CONTROL_FACT_FIELDS:
        no_fact["simpleir_control_kind"][0][field] = False
    expect_error(no_fact, "at least one fact must be true")


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
        gen._validate_disjoint_opcode_role_sets(
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
        gen._validate_disjoint_opcode_role_sets(
            transparent_role_overlap,
            gen._ALIAS_TRANSPARENT_ALIAS_ROLE_SETS,
            "alias transparent-alias role",
        )
    except gen.OpKindTableError as e:
        assert "TypeGuard" in str(e)
    else:
        raise AssertionError("overlapping alias transparent-alias role was accepted")

    gvn_role_overlap = json.loads(json.dumps(data))
    gvn_role_overlap["gvn_type_gated_numberable_opcodes"].append("BoxVal")
    try:
        gen._validate_disjoint_opcode_role_sets(
            gvn_role_overlap,
            gen._GVN_NUMBERING_ROLE_SETS,
            "GVN numbering role",
        )
    except gen.OpKindTableError as e:
        assert "BoxVal" in str(e)
    else:
        raise AssertionError("overlapping GVN numbering role was accepted")

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
    canonicalize = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/canonicalize.rs")
    check_exception = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/check_exception_elim.rs")

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
        row["opcode"]: row["domain"] for row in data["canonicalize_commutative_reorder"]
    } == expected_domains
    assert {
        row["opcode"]: row["swapped"] for row in data["canonicalize_swapped_comparison"]
    } == expected_swaps
    assert {
        row["opcode"]: row["literal"] for row in data["literal_payload_opcodes"]
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

    literal_block = rendered.split("fn opcode_literal_payload_kind_table")[1].split(
        "fn opcode_canonicalize_commutative_domain_table"
    )[0]
    literal_variant = {
        "int": "LiteralPayloadKind::Int",
        "bool": "LiteralPayloadKind::Bool",
    }
    for opcode, literal in expected_literals.items():
        assert f"OpCode::{opcode} => Some({literal_variant[literal]})," in literal_block
    assert "OpCode::ConstNone => None," in literal_block

    variant = {
        "numeric": "CanonicalizeCommutativeDomain::Numeric",
        "i64": "CanonicalizeCommutativeDomain::I64",
        "unboxed_scalar": "CanonicalizeCommutativeDomain::UnboxedScalar",
    }
    domain_block = rendered.split("fn opcode_canonicalize_commutative_domain_table")[
        1
    ].split("fn opcode_swapped_comparison_for_canonicalize_table")[0]
    for opcode, domain in expected_domains.items():
        assert f"OpCode::{opcode} => Some({variant[domain]})," in domain_block
    assert "OpCode::Sub => None," in domain_block

    swap_block = rendered.split("fn opcode_swapped_comparison_for_canonicalize_table")[
        1
    ].split("enum OperandOwnership")[0]
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

    src = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/ops.rs")
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


def test_frontend_effect_classes_match_generated_authority() -> None:
    """The frontend optimizer effect map is generated from op_kinds.toml, not
    from a hand-kept Python set in the optimizer."""
    gen = _gen()
    data = gen.load_table()
    py = _load_generated_py()

    expected = gen._frontend_effect_class_map(data)
    assert py.FRONTEND_EFFECT_CLASS == expected
    assert py.FRONTEND_EFFECT_PURE_KINDS == {
        kind for kind, effect in expected.items() if effect == "pure"
    }
    assert py.FRONTEND_EFFECT_READS_HEAP_KINDS == {
        kind for kind, effect in expected.items() if effect == "reads_heap"
    }
    assert py.FRONTEND_EFFECT_WRITES_HEAP_KINDS == {
        kind for kind, effect in expected.items() if effect == "writes_heap"
    }
    assert py.FRONTEND_EFFECT_CONTROL_KINDS == {
        kind for kind, effect in expected.items() if effect == "control"
    }


def test_frontend_effect_classes_pin_pre_specialization_barriers() -> None:
    py = _load_generated_py()

    for kind in {
        "ADD",
        "SUB",
        "MUL",
        "EQ",
        "NE",
        "LT",
        "LE",
        "GT",
        "GE",
        "INDEX",
        "GET_ATTR",
        "MODULE_GET_ATTR",
        "CONST_STR",
    }:
        assert py.FRONTEND_EFFECT_CLASS[kind] == "writes_heap"
        assert kind not in py.FRONTEND_EFFECT_PURE_KINDS
        assert kind not in py.FRONTEND_EFFECT_READS_HEAP_KINDS

    assert py.FRONTEND_EFFECT_CLASS["LOAD_VAR"] == "reads_heap"
    assert py.FRONTEND_EFFECT_CLASS["STORE_VAR"] == "writes_heap"
    assert py.FRONTEND_EFFECT_CLASS["PHI"] == "pure"
    assert py.FRONTEND_EFFECT_CLASS["EXCEPTION_MATCH_BUILTIN"] == "reads_heap"
    assert py.FRONTEND_EFFECT_CLASS["STATE_TRANSITION"] == "control"
    assert py.FRONTEND_EFFECT_CLASS["GUARD_TAG"] == "control"


def test_midend_effect_oracle_consumes_generated_authority_only() -> None:
    source = (
        ROOT / "src/molt/frontend/lowering/midend_canonicalization.py"
    ).read_text(encoding="utf-8")
    method = source.split("def _op_effect_class", 1)[1].split(
        "def _is_pure_op_for_global_cse", 1
    )[0]

    assert 'FRONTEND_EFFECT_CLASS.get(op_kind, "unknown")' in method
    assert "op_kind in {" not in method
    assert ".startswith(" not in method


def test_generated_python_canonicalizes_rc_aliases_for_analysis() -> None:
    py = _load_generated_py()

    assert py.canonical_kind("borrow") == "inc_ref"
    assert py.canonical_kind("release") == "dec_ref"
    assert "inc_ref" in py.BINARY_IMAGE_REF_RETAIN_KINDS
    assert "dec_ref" in py.BINARY_IMAGE_REF_RELEASE_KINDS
    assert "borrow" not in py.BINARY_IMAGE_REF_RETAIN_KINDS
    assert "release" not in py.BINARY_IMAGE_REF_RELEASE_KINDS
    assert py.canonical_kind("borrow") in py.BINARY_IMAGE_REF_RETAIN_KINDS
    assert py.canonical_kind("release") in py.BINARY_IMAGE_REF_RELEASE_KINDS
    assert "alloc" in py.BINARY_IMAGE_HEAP_ALLOC_ROOT_KINDS
    assert "list_new" in py.BINARY_IMAGE_HEAP_ALLOC_ROOT_KINDS
    assert "del_boundary" in py.BINARY_IMAGE_REF_RELEASE_KINDS


def test_binary_image_analysis_consumes_generated_allocation_sets() -> None:
    analyzer = (ROOT / "src/molt/compiler_analysis/backend_ir.py").read_text(
        encoding="utf-8"
    )

    for generated_name in (
        "BINARY_IMAGE_HEAP_ALLOC_ROOT_KINDS",
        "BINARY_IMAGE_STACK_ALLOC_ROOT_KINDS",
        "BINARY_IMAGE_REF_RETAIN_KINDS",
        "BINARY_IMAGE_REF_RELEASE_KINDS",
        "BINARY_IMAGE_HEAP_EXPOSURE_KINDS",
    ):
        assert f"op_kind_facts.{generated_name}" in analyzer
        private_name = f"_{generated_name.removeprefix('BINARY_IMAGE_')}"
        assert not re.search(rf"(?m)^{private_name}\s*=", analyzer)


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

    mutated_effect = json.loads(json.dumps(data))
    for row in mutated_effect["frontend_effect_kind"]:
        if row["kind"] == "LOAD_VAR":
            row["effect"] = "writes_heap"
            break
    assert gen.render_py(mutated_effect) != rendered, (
        "mutating a frontend_effect_kind row did not change the Python render"
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
    baseline = json.loads(
        (ROOT / "tools/op_kinds_baseline.json").read_text(encoding="utf-8")
    )

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
    mutated = json.loads(json.dumps(data))
    mutated["classifier_owned_alias"].append("zzz_synthetic_alias")
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
        "DeleteVar": ("OpCode::DeleteVar => OperandOwnership::Borrowed,"),
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
        "pub enum OperandOwnership {\n"
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
    assert validity == {("IterNextUnboxed", 0): "conditional_valid_only_on_edge"}

    assert (
        "pub enum ResultValidity {\n"
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
        {
            "opcode": "IterNextUnboxed",
            "result": -1,
            "validity": "conditional_valid_only_on_edge",
        },
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
    source = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")
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


def test_ownership_lattice_delegates_conditional_result_validity_to_generated_table() -> (
    None
):
    """Conditional result validity must stay sourced from generated op-kind facts."""
    ownership = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")
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
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    ownership = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")

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


def test_drop_insertion_delegates_conditional_result_validity_to_ownership_lattice() -> (
    None
):
    """drop_insertion.rs must consume conditional result-validity through
    OwnershipLattice root facts, not own a second generated-table/root scan."""
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    drop_prod = drop.split("mod tests", 1)[0]
    assert "fn op_result_is_conditionally_valid_only_on_edge(" not in drop
    assert "opcode_result_is_conditionally_valid_only_on_edge" not in drop
    assert "OwnershipRootFacts::compute(func, &aliases)" in drop
    assert "OwnershipLattice::compute_with_root_facts(" in drop
    assert "ownership_lattice.is_conditionally_valid_result_root(canon(v))" in drop
    assert ".conditionally_valid_result_values()" not in drop
    assert ".conditionally_valid_result_roots()" not in drop
    assert "OpCode::IterNextUnboxed" not in drop_prod.replace(
        "`IterNextUnboxed`", ""
    ), (
        "DropInsertion production code must not own an IterNextUnboxed "
        "result-validity hand list"
    )


def test_drop_insertion_consumes_finalizer_sensitive_roots_from_ownership_lattice() -> (
    None
):
    """DropInsertion must consume FinalizerSensitive as a root-space lattice fact.

    The lattice owns alias-root folding for finalizer-sensitive values and
    return-boundary deferral; statement-release boundary composition is checked
    separately through `StatementReleasePlan`.
    """
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    assert ".finalizer_sensitive_values()" not in drop
    assert ".finalizer_sensitive_roots()" in drop
    assert "let root = boundary.root;" not in drop
    assert "boundary.value" not in drop
    marker = "let sensitive_roots: HashSet<ValueId> = lattice"
    assert marker in drop, "sensitive_roots lattice consumer not found"
    region = drop[
        drop.index(marker) : drop.index("let has_suspension", drop.index(marker))
    ]
    assert ".finalizer_sensitive_roots()" in region
    assert ".map(|&v| canon(v))" not in region


def test_drop_insertion_consumes_non_owning_copy_roots_from_ownership_lattice() -> None:
    """DropInsertion must consume C5 non-owning Copy roots as lattice facts.

    The pass owns placement, not copy-result ownership classification. The
    no-heap alias classifier is consumed through a separate ownership helper for
    CFG remapping; droppability must read the OwnershipRootFacts root set.
    """
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    lattice = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")
    assert "non_owning_copy_results" not in drop
    assert "copy_kind_mints_fresh_owned_ref" not in drop
    assert "let mints_fresh =" not in drop
    assert "OwnershipRootFacts::compute(func, &aliases)" in drop
    assert "DropEligibility::new(" in drop
    assert "drop_eligibility.is_droppable(" in drop
    assert "ownership_root_facts.is_drop_owned_root_candidate(" not in drop
    assert "fn non_owning_copy_result_roots(" in lattice
    assert _rust_pub_fn(lattice, "is_non_owning_copy_result_root")
    assert _rust_pub_decl(lattice, "struct", "DropEligibility")
    assert _rust_pub_fn(lattice, "is_droppable")
    assert "classify_copy_kind(kind)" in lattice
    assert "copy_kind_is_explicit_no_heap_move(kind)" in lattice


def test_drop_insertion_consumes_no_heap_copy_aliases_from_ownership_lattice() -> None:
    """Exception-pop CFG splitting may remap no-heap copy aliases.

    DropInsertion owns the split placement, but the `_original_kind` classifier
    read belongs to the ownership fact module so the pass does not grow another
    copy-spelling authority.
    """
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    drop_prod = drop.split("mod tests", 1)[0]
    lattice = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")

    assert "copy_transparent_alias" in drop_prod
    assert "copy_kind_is_explicit_no_heap_move" not in drop_prod
    assert "fn original_kind(" not in drop_prod

    assert _rust_pub_decl(lattice, "struct", "NoHeapCopyAlias")
    assert _rust_pub_fn(lattice, "copy_transparent_alias")
    assert "copy_kind_is_explicit_no_heap_move(original_kind(op))" in lattice
    assert "source: op.operands[0]" in lattice
    assert "result: op.results[0]" in lattice


def test_drop_insertion_consumes_parameter_and_stack_roots_from_ownership_lattice() -> (
    None
):
    """Parameter/stack no-drop facts belong to OwnershipRootFacts, not the pass."""
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    lattice = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")
    assert "let param_ids" not in drop
    assert "let param_roots" not in drop
    assert "let stack_values" not in drop
    assert "let stack_roots" not in drop
    assert "fn produces_stack_value(" not in drop
    assert "drop_eligibility.is_droppable(" in drop
    assert "ownership_root_facts.is_drop_owned_root_candidate(" not in drop
    assert "fn parameter_roots(" in lattice
    assert "fn stack_value_roots(" in lattice
    assert _rust_pub_fn(lattice, "is_drop_owned_root_candidate")


def test_drop_insertion_delegates_droppable_predicate_to_drop_eligibility() -> None:
    """DropInsertion owns placement, not the composed root/raw droppability test."""
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    lattice = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")
    assert "let droppable =" not in drop
    assert "raw_scalars.contains" not in drop
    assert "live.is_raw_scalar(v)" not in drop
    assert "DropEligibility::new(" in drop
    assert "&live.raw_scalars" in drop
    assert "drop_eligibility.is_raw_scalar_root(canon(v))" in drop
    assert "drop_eligibility.is_droppable(" in drop
    assert _rust_pub_decl(lattice, "struct", "DropEligibility")
    assert _rust_pub_fn(lattice, "is_raw_scalar_root")
    assert _rust_pub_fn(lattice, "is_droppable")


def test_drop_insertion_consumes_python_lifetime_facts_from_ownership_lattice() -> None:
    """DropInsertion consumes Python lifetime roots instead of re-scanning them."""
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    lattice = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")
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
    assert "python_lifetime_facts.is_statement_release_boundary_root(" not in drop
    assert (
        "python_lifetime_facts.is_return_boundary_deferred_root(r, &drop_eligibility)"
        in drop
    )
    assert "python_lifetime_facts.has_explicit_release_boundary(v)" in drop
    assert _rust_pub_decl(lattice, "struct", "PythonLifetimeFacts")
    assert "fn compute(func: &TirFunction, aliases: &AliasUnionFind)" in lattice
    assert "fn local_store_roots(" not in lattice
    assert "fn is_local_store_root(" not in lattice
    assert "fn is_bound_local_root(" not in lattice
    assert "fn is_named_slot_root(" not in lattice
    assert "fn is_explicit_release_root(" not in lattice
    assert _rust_pub_fn(lattice, "boundary_release_roots")
    assert "drop_eligibility.is_droppable(*root)" in lattice
    assert "ownership_lattice.is_finalizer_sensitive_root(*root)" in lattice
    assert "!self.has_explicit_release_boundary(*root)" in lattice
    assert _rust_pub_fn(lattice, "is_statement_release_boundary_root")
    assert "drop_eligibility.is_droppable(root)" in lattice
    assert "!self.local_store_roots.contains(&root)" in lattice
    assert "!self.has_explicit_release_boundary(root)" in lattice
    assert _rust_pub_fn(lattice, "is_return_boundary_deferred_root")
    return_boundary_region = lattice[
        lattice.index("fn is_return_boundary_deferred_root(") : lattice.index(
            "fn has_explicit_release_boundary("
        )
    ]
    assert "self.bound_local_roots.contains(&root)" in return_boundary_region
    assert "!self.named_slot_roots.contains(&root)" in return_boundary_region
    assert (
        "!drop_eligibility.is_conditionally_valid_result_root(root)"
        in return_boundary_region
    )
    assert _rust_pub_fn(lattice, "has_explicit_release_boundary")


def test_drop_insertion_consumes_statement_release_plan_from_ownership_lattice() -> (
    None
):
    """Statement-release boundary composition belongs to the ownership module.

    DropInsertion may materialize the DecRefs, but it must not own the local
    maps that combine FinalizerSensitive storage boundaries, Python lifetime
    exclusions, drop eligibility, sorting, and deduplication.
    """
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    lattice = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")

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

    assert _rust_pub_decl(lattice, "struct", "StatementReleasePlan")
    assert _rust_pub_fn(lattice, "compute")
    assert "lattice.statement_release_finalizer_boundaries()" in lattice
    assert (
        "python_lifetime_facts.is_statement_release_boundary_root(root, drop_eligibility)"
        in lattice
    )
    assert "plan.after_op" in lattice
    assert "plan.released_roots.insert(root)" in lattice
    assert "roots.sort_unstable_by_key(" in lattice
    assert "roots.dedup()" in lattice


def test_drop_insertion_consumes_exception_creation_facts_from_ownership_lattice() -> (
    None
):
    """CreationRef classification belongs to the ownership module, not placement."""
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    lattice = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")
    assert "exception_creation_ref_values" in drop
    assert "fn exception_creation_ref_values(" not in drop
    assert "copy_kind_is_exception_creation_ref" not in drop
    assert _rust_pub_fn(lattice, "exception_creation_ref_values")
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
    alias = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/alias_analysis.rs")
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

    src = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/blocks.rs")
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
    drop = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/drop_insertion.rs")
    ownership = _read_rs_module_cluster(ROOT / "runtime/molt-tir/src/tir/passes/ownership_lattice_min.rs")

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
