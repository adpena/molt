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


# ---------------------------------------------------------------------------
# 1. Freshness: the checked-in generated files match a fresh render.
# ---------------------------------------------------------------------------


def test_generated_rs_is_in_sync() -> None:
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    checked_in = OUT_RS.read_text()
    assert checked_in == rendered, (
        f"{OUT_RS.relative_to(ROOT)} is stale — run "
        "`python3 tools/gen_op_kinds.py` to regenerate from op_kinds.toml."
    )


def test_generated_py_is_in_sync() -> None:
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_py(data)
    checked_in = OUT_PY.read_text()
    assert checked_in == rendered, (
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
    gen_no_heap = set(
        audit.extract_matches_macro(OUT_RS, "copy_kind_is_explicit_no_heap_move_table")
    )
    assert gen_fresh == set(data["classifier_fresh_value"]), (
        "generated fresh-value table drifted from classifier_fresh_value"
    )
    assert gen_inert == set(data["classifier_inert_marker"]), (
        "generated inert-marker table drifted from classifier_inert_marker"
    )
    assert gen_no_heap == set(data["classifier_no_heap_move"]), (
        "generated no-heap-move table drifted from classifier_no_heap_move"
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
    assert res.inert_marker == set(data["classifier_inert_marker"])
    assert res.no_heap_move == set(data["classifier_no_heap_move"])
    assert set(res.fresh_value_prefixes) == set(data["classifier_fresh_value_prefixes"])


def test_effects_rs_delegates_to_generated_tables() -> None:
    """The effect oracle in effects.rs must DELEGATE to the generated tables (no
    hand-maintained `matches!` of opcodes), and the generated tables must embed a
    correct exhaustive arm per opcode for may_throw / side_effecting / purity.
    This is the structural kill for the matches!-default-false trap: the source of
    truth is the table, and a new opcode cannot compile without a row."""
    gen = _gen()
    data = gen.load_table()
    rendered = gen.render_rs(data)
    effects = (ROOT / "runtime/molt-backend/src/tir/passes/effects.rs").read_text()

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


def test_opcode_effects_exhaustive_over_enum() -> None:
    """The effect table must cover EVERY OpCode variant — the exhaustiveness that
    kills the matches!-default-false trap. Cross-check the table's opcode names
    against the OpCode enum declared in ops.rs."""
    import re

    gen = _gen()
    data = gen.load_table()
    table_names = [row["name"] for row in data["opcode"]]
    assert len(table_names) == len(set(table_names)), "duplicate opcode rows"

    src = (ROOT / "runtime/molt-backend/src/tir/ops.rs").read_text()
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
