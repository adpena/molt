#!/usr/bin/env python3
"""Generate the op-kind registry artifacts from the canonical table.

Single source of truth: ``runtime/molt-backend/src/tir/op_kinds.toml``.

Cross-component op-"kind"-string drift is molt's most prolific silent-miscompile
bug class (see ``docs/design/foundation/25_op_kind_registry.md`` and
``tools/audit_op_kinds.py``). Five components independently keyed on the JSON wire
"kind" vocabulary, each with its own private table. This generator renders that
ONE table into every consumer so the tables can never drift:

  - ``runtime/molt-backend/src/tir/op_kinds_generated.rs`` — the data tables the
    backend's ``kind_to_opcode`` mapper, the ``CopyLowering`` classifier
    (``copy_kind_mints_fresh_owned_ref`` / ``classify_copy_kind`` /
    ``copy_kind_is_explicit_no_heap_move``), and the per-OpCode effect oracle
    (``opcode_may_throw`` / ``opcode_is_side_effecting`` / ``opcode_effects``)
    consume. The effect oracle is rendered as an EXHAUSTIVE match over the
    ``OpCode`` enum (no wildcard arm), so a newly added opcode fails to compile
    until it is given an explicit effect classification in the table — the
    structural kill for the ``matches!``-default-false trap.
  - ``src/molt/frontend/lowering/op_kinds_generated.py`` — the canonical wire
    spellings the frontend emitter (``map_ops_to_json``) uses, so the producer
    and the backend mapper share one spelling.

``tests/test_gen_op_kinds.py`` re-renders both files in memory and asserts byte
equality with the checked-in copies, turning any forgotten regeneration into a
test failure (the ``tests/test_gen_intrinsics.py`` pattern).

Usage::

    python3 tools/gen_op_kinds.py            # (re)write the generated files
    python3 tools/gen_op_kinds.py --check    # exit 1 if a generated file is stale
"""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover - fallback for <3.11
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]

TABLE = ROOT / "runtime/molt-backend/src/tir/op_kinds.toml"
OUT_RS = ROOT / "runtime/molt-backend/src/tir/op_kinds_generated.rs"
OUT_PY = ROOT / "src/molt/frontend/lowering/op_kinds_generated.py"

# Valid enum values for the table's constrained columns. A value outside these
# sets is a hard error (a typo in the table must not silently degrade to a
# fallback classification).
_PURITY_VALUES = {"pure", "pure_may_throw", "impure"}

# Operand-ownership: the per-operand borrowed|consumed axis (design 27 §2.1).
# A uniform shorthand ("all_borrowed" / "all_consumed") or a per-position list of
# these two leaf values. molt's "callee borrows all args" ABI (design 20 §1.2)
# makes "all_borrowed" the universal default; "consumed" is the rare op-frees-it
# case (the CallArgs builder, the C6 double-free class). A value outside this set
# is a hard error (a typo must never silently degrade to a borrow assumption that
# leaks, or a consume assumption that double-frees).
_OPERAND_OWNERSHIP_LEAVES = {"borrowed", "consumed"}
_OPERAND_OWNERSHIP_UNIFORM = {"all_borrowed", "all_consumed"}

# Per-TERMINATOR operand-category leaves (design 27 §2.4, the ownership-moves-out
# axis). A `Terminator` is NOT an `OpCode` — its operand ownership is a distinct
# table — so it admits the `transferred` move-out leaf (a `Return` value / a
# branch-arg into a successor phi) and the `none` sentinel (a category with no
# operand on that variant). `borrowed` is the still-live-but-not-moved predicate
# (`CondBranch`/`Switch` discriminant). `consumed` is NOT meaningful for a
# terminator (nothing frees a terminator operand internally), so it is excluded.
_TERMINATOR_OWNERSHIP_LEAVES = {"borrowed", "transferred", "none"}

# The `Terminator` enum variants (blocks.rs). The [[terminator]] section MUST be
# EXHAUSTIVE over this set (a new variant fails to render until classified —
# mirroring the [[opcode]] exhaustiveness discipline). Kept here (not parsed from
# Rust) as the single declarative expectation; tests/test_gen_op_kinds.py
# cross-checks it against the enum declared in blocks.rs so the two cannot drift.
_TERMINATOR_VARIANTS = ("Branch", "CondBranch", "Switch", "Return", "Unreachable")

# The three flat classifier sets (mirroring the flat `matches!` arms in
# alias_analysis.rs). Kept distinct from the mapper's alias grouping because the
# classifier groups per-individual-kind, not per-OpCode-equivalence.
_CLASSIFIER_SETS = (
    "classifier_fresh_value",
    "classifier_inert_marker",
    "classifier_no_heap_move",
)


# Rust `bool` literal helper.
def _rs_bool(value: bool) -> str:
    return "true" if value else "false"


# ---------------------------------------------------------------------------
# Table loading + validation
# ---------------------------------------------------------------------------


class OpKindTableError(RuntimeError):
    pass


def load_table() -> dict:
    """Load and structurally validate ``op_kinds.toml``.

    Validation is fail-loud: a malformed/ambiguous table must never render a
    silently-degraded generated file.
    """
    if not TABLE.exists():
        raise OpKindTableError(f"op-kind table missing: {TABLE}")
    data = tomllib.loads(TABLE.read_text())

    opcodes = data.get("opcode", [])
    if not opcodes:
        raise OpKindTableError("table has no [[opcode]] rows")
    seen_opcodes: set[str] = set()
    for row in opcodes:
        name = row.get("name")
        if not isinstance(name, str) or not name:
            raise OpKindTableError(f"[[opcode]] row missing 'name': {row}")
        if name in seen_opcodes:
            raise OpKindTableError(f"duplicate [[opcode]] name: {name}")
        seen_opcodes.add(name)
        if not isinstance(row.get("may_throw"), bool):
            raise OpKindTableError(f"opcode {name}: 'may_throw' must be a bool")
        if not isinstance(row.get("side_effecting"), bool):
            raise OpKindTableError(f"opcode {name}: 'side_effecting' must be a bool")
        purity = row.get("purity")
        if purity not in _PURITY_VALUES:
            raise OpKindTableError(
                f"opcode {name}: 'purity' must be one of {sorted(_PURITY_VALUES)}, "
                f"got {purity!r}"
            )
        # Cross-axis invariant: the `purity` class and `may_throw` bit are two
        # views of the same throw property and MUST agree. `OpEffects::PURE` has
        # `nothrow = true`, so a `pure` opcode cannot also be `may_throw`; a
        # `pure_may_throw` opcode is precisely the throwing-but-deterministic
        # class (`Div`/`FloorDiv`/`Mod`/`Pow`/`Shl`/`Shr`), so it MUST be
        # `may_throw`. `impure` is unconstrained (a call both throws and mutates).
        # This is the structural kill for the drift that classified `Pow` as
        # `pure_may_throw` yet `may_throw = false` (and `Shl`/`Shr` as fully
        # `pure`), which let DCE silently drop a dead `1 << -1` / `0 ** -1`.
        if purity == "pure" and row["may_throw"]:
            raise OpKindTableError(
                f"opcode {name}: purity 'pure' requires may_throw = false "
                "(a pure op is nothrow); use purity 'pure_may_throw' if it raises"
            )
        if purity == "pure_may_throw" and not row["may_throw"]:
            raise OpKindTableError(
                f"opcode {name}: purity 'pure_may_throw' requires may_throw = true "
                "(it raises for some inputs); use purity 'pure' if it never raises"
            )
        # Operand ownership is MANDATORY and explicit on every opcode (mirroring
        # the may_throw/side_effecting/purity exhaustive-classification
        # discipline): a new OpCode cannot render until it states whether each
        # operand is borrowed or consumed. Fail-loud — no silent borrow default.
        _validate_operand_ownership(name, row.get("operand_ownership"))

    prefixes = data.get("classifier_fresh_value_prefixes", [])
    if not isinstance(prefixes, list) or not all(isinstance(p, str) for p in prefixes):
        raise OpKindTableError(
            "classifier_fresh_value_prefixes must be a list of strings"
        )

    for key in _CLASSIFIER_SETS:
        members = data.get(key, [])
        if not isinstance(members, list) or not all(
            isinstance(x, str) for x in members
        ):
            raise OpKindTableError(f"{key} must be a list of strings")
        if len(set(members)) != len(members):
            raise OpKindTableError(f"{key} has duplicate members")

    kinds = data.get("kind", [])
    # Every mapper spelling (canonical or alias) must be globally unique within
    # the mapper — a kind string maps to exactly one OpCode; two rows owning it
    # is the exact drift this registry kills.
    owner: dict[str, str] = {}
    seen_canon: set[str] = set()
    for row in kinds:
        canon = row.get("canonical")
        if not isinstance(canon, str) or not canon:
            raise OpKindTableError(f"[[kind]] row missing 'canonical': {row}")
        if canon in seen_canon:
            raise OpKindTableError(f"duplicate canonical kind: {canon}")
        seen_canon.add(canon)
        aliases = row.get("aliases", [])
        if not isinstance(aliases, list) or not all(
            isinstance(a, str) for a in aliases
        ):
            raise OpKindTableError(f"kind {canon}: 'aliases' must be a list of strings")
        mapper = row.get("mapper_opcode")
        if not isinstance(mapper, str) or mapper not in seen_opcodes:
            raise OpKindTableError(
                f"kind {canon}: mapper_opcode {mapper!r} is not a known OpCode"
            )
        for spelling in [canon, *aliases]:
            if spelling in owner:
                raise OpKindTableError(
                    f"mapper spelling {spelling!r} owned by both "
                    f"{owner[spelling]!r} and {canon!r}"
                )
            owner[spelling] = canon

    # -- [[consuming_kind]] operand-ownership overrides per wire-kind spelling --
    # Each row names a wire-kind SPELLING (canonical OR alias of a [[kind]] row)
    # that consumes a specific operand. `owner` is exactly the set of valid
    # mapper spellings; a row naming an unknown spelling is a hard error (the
    # structural kill for a typo'd consume override silently doing nothing — the
    # very C6 double-free this column retires).
    _validate_consuming_kinds(data, owner)

    _validate_terminators(data)

    _validate_frontend_tables(data, opcodes)

    return data


def _validate_operand_ownership(name: str, value: object) -> None:
    """Validate one opcode's ``operand_ownership`` (fail-loud).

    Accepts a uniform shorthand (``"all_borrowed"`` / ``"all_consumed"``) or a
    per-position list of the leaf values (``"borrowed"`` / ``"consumed"``). Any
    other shape is a hard error — a missing/typo'd classification must never
    silently degrade to a borrow assumption (leak) or a consume assumption
    (double-free).
    """
    if value is None:
        raise OpKindTableError(
            f"opcode {name}: 'operand_ownership' is mandatory — classify every "
            "operand as borrowed|consumed (use \"all_borrowed\" for the common "
            "callee-borrows-args case; design 20 §1.2 / design 27 §2.1)"
        )
    if isinstance(value, str):
        if value not in _OPERAND_OWNERSHIP_UNIFORM:
            raise OpKindTableError(
                f"opcode {name}: 'operand_ownership' string must be one of "
                f"{sorted(_OPERAND_OWNERSHIP_UNIFORM)}, got {value!r} (or use a "
                "per-position list of borrowed|consumed)"
            )
        return
    if isinstance(value, list):
        if not value:
            raise OpKindTableError(
                f"opcode {name}: 'operand_ownership' list must be non-empty (use "
                'the "all_borrowed" shorthand for a uniform op)'
            )
        for i, leaf in enumerate(value):
            if leaf not in _OPERAND_OWNERSHIP_LEAVES:
                raise OpKindTableError(
                    f"opcode {name}: 'operand_ownership'[{i}] must be one of "
                    f"{sorted(_OPERAND_OWNERSHIP_LEAVES)}, got {leaf!r}"
                )
        return
    raise OpKindTableError(
        f"opcode {name}: 'operand_ownership' must be a string shorthand or a list, "
        f"got {type(value).__name__}"
    )


def _validate_consuming_kinds(data: dict, valid_spellings: dict[str, str]) -> None:
    """Structurally validate the ``[[consuming_kind]]`` operand-ownership
    overrides (fail-loud). Each row pins one wire-kind SPELLING to a consumed
    operand position; the spelling must be a known mapper spelling and the
    consumed-operand selector must be ``"last"`` or a non-negative integer."""
    rows = data.get("consuming_kind", [])
    if not isinstance(rows, list):
        raise OpKindTableError("[[consuming_kind]] must be an array of tables")
    seen: set[str] = set()
    for row in rows:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(f"[[consuming_kind]] row missing 'kind': {row}")
        if kind in seen:
            raise OpKindTableError(f"duplicate consuming_kind: {kind}")
        seen.add(kind)
        if kind not in valid_spellings:
            raise OpKindTableError(
                f"consuming_kind {kind!r} is not a known [[kind]] mapper spelling "
                "(canonical or alias) — a consume override on an unknown spelling "
                "would silently never fire (the C6 double-free it must retire)"
            )
        sel = row.get("consumed_operand")
        if sel == "last":
            continue
        if isinstance(sel, bool) or not isinstance(sel, int) or sel < 0:
            raise OpKindTableError(
                f"consuming_kind {kind}: 'consumed_operand' must be \"last\" or a "
                f"non-negative operand index, got {sel!r}"
            )


def _validate_terminators(data: dict) -> None:
    """Structurally validate the ``[[terminator]]`` per-terminator operand
    ownership (design 27 §2.4, fail-loud). Each row classifies one ``Terminator``
    enum variant's two operand categories (``direct`` / ``branch_arg``) as a
    ``_TERMINATOR_OWNERSHIP_LEAVES`` value. The section MUST be EXHAUSTIVE over
    the ``Terminator`` enum (a new variant unclassified is a generation-time
    failure — the kill for a terminator silently inheriting a transfer/borrow
    assumption, mirroring the [[opcode]] exhaustiveness discipline)."""
    rows = data.get("terminator", [])
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError("table has no [[terminator]] rows")
    seen: set[str] = set()
    for row in rows:
        name = row.get("name")
        if not isinstance(name, str) or not name:
            raise OpKindTableError(f"[[terminator]] row missing 'name': {row}")
        if name in seen:
            raise OpKindTableError(f"duplicate [[terminator]] name: {name}")
        seen.add(name)
        for col in ("direct", "branch_arg"):
            leaf = row.get(col)
            if leaf not in _TERMINATOR_OWNERSHIP_LEAVES:
                raise OpKindTableError(
                    f"terminator {name}: {col!r} must be one of "
                    f"{sorted(_TERMINATOR_OWNERSHIP_LEAVES)}, got {leaf!r}"
                )
    if seen != set(_TERMINATOR_VARIANTS):
        raise OpKindTableError(
            "[[terminator]] must be EXHAUSTIVE over the Terminator enum "
            "(an unclassified variant would silently inherit a transfer/borrow "
            "assumption in drop_insertion's transfer carve-out): "
            f"table-only={sorted(seen - set(_TERMINATOR_VARIANTS))} "
            f"enum-only={sorted(set(_TERMINATOR_VARIANTS) - seen)}"
        )


# ---------------------------------------------------------------------------
# Frontend op.kind table validation (molt task #44, F2a)
# ---------------------------------------------------------------------------


def _validate_frontend_tables(data: dict, opcodes: list[dict]) -> None:
    """Structurally validate the three frontend `op.kind` tables.

    These describe the FRONTEND's UPPERCASE pre-serialization `op.kind`
    vocabulary (distinct from the wire `[[kind]]` spellings). The validation is
    the structural kill for the frontend⇄backend dual raising-oracle drift:

      * Every `[[frontend_raising_kind]]` row carrying `opcode = X` is
        cross-checked X.may_throw == true (flipping an opcode to nothrow while
        leaving it raising here is a generation-time FAILURE).
      * Every `[[frontend_check_exception_skip]]` row carrying `opcode = X` is
        cross-checked X.may_throw == false UNLESS it sets `control_flow = true`
        (a may_throw skip member must justify itself as structurally handled).
      * `[[binary_op]]` is cross-checked EXHAUSTIVE over `ast.operator` — a
        missing operator subclass is a generation-time FAILURE (the task-#27
        lesson that the hand augassign map silently omitted 7 inplace kinds).
    """
    may_throw_ops = {r["name"] for r in opcodes if r["may_throw"]}
    opcode_names = {r["name"] for r in opcodes}

    # -- [[frontend_raising_kind]] ------------------------------------------
    raising = data.get("frontend_raising_kind", [])
    if not isinstance(raising, list) or not raising:
        raise OpKindTableError("table has no [[frontend_raising_kind]] rows")
    seen_raising: set[str] = set()
    for row in raising:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[frontend_raising_kind]] row missing 'kind': {row}"
            )
        if kind in seen_raising:
            raise OpKindTableError(f"duplicate frontend_raising_kind: {kind}")
        seen_raising.add(kind)
        has_opcode = "opcode" in row
        has_reason = "reason" in row
        if has_opcode == has_reason:
            raise OpKindTableError(
                f"frontend_raising_kind {kind}: exactly one of 'opcode' / 'reason' "
                "required (opcode = a may_throw OpCode it maps to; reason = a "
                "documented frontend-specific justification)"
            )
        if has_opcode:
            op = row["opcode"]
            if op not in opcode_names:
                raise OpKindTableError(
                    f"frontend_raising_kind {kind}: opcode {op!r} is not a known OpCode"
                )
            if op not in may_throw_ops:
                raise OpKindTableError(
                    f"frontend_raising_kind {kind}: opcode {op!r} is NOT may_throw — "
                    "a raising frontend kind must map to a may_throw OpCode (or use "
                    "'reason' for a frontend-specific pre-specialization/preserved kind)"
                )
        else:
            if not isinstance(row["reason"], str) or not row["reason"]:
                raise OpKindTableError(
                    f"frontend_raising_kind {kind}: 'reason' must be a non-empty string"
                )

    # -- [[frontend_check_exception_skip]] ----------------------------------
    skip = data.get("frontend_check_exception_skip", [])
    if not isinstance(skip, list) or not skip:
        raise OpKindTableError("table has no [[frontend_check_exception_skip]] rows")
    seen_skip: set[str] = set()
    for row in skip:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[frontend_check_exception_skip]] row missing 'kind': {row}"
            )
        if kind in seen_skip:
            raise OpKindTableError(f"duplicate frontend_check_exception_skip: {kind}")
        seen_skip.add(kind)
        has_opcode = "opcode" in row
        has_reason = "reason" in row
        if has_opcode == has_reason:
            raise OpKindTableError(
                f"frontend_check_exception_skip {kind}: exactly one of 'opcode' / "
                "'reason' required"
            )
        if has_opcode:
            op = row["opcode"]
            if op not in opcode_names:
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: opcode {op!r} is not a "
                    "known OpCode"
                )
            control_flow = row.get("control_flow", False)
            if not isinstance(control_flow, bool):
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: 'control_flow' must be a bool"
                )
            if control_flow:
                # A may_throw opcode is skip-listed because its exceptional edge
                # is handled structurally; the flag must be justified by an
                # actually-throwing opcode.
                if op not in may_throw_ops:
                    raise OpKindTableError(
                        f"frontend_check_exception_skip {kind}: control_flow = true but "
                        f"opcode {op!r} is NOT may_throw (the flag is spurious — a "
                        "nothrow opcode needs no control_flow exception)"
                    )
            else:
                if op in may_throw_ops:
                    raise OpKindTableError(
                        f"frontend_check_exception_skip {kind}: opcode {op!r} is "
                        "may_throw but not flagged control_flow — skipping its "
                        "CHECK_EXCEPTION would drop the exception edge. Set "
                        "control_flow = true (with justification) or remove the row."
                    )
        else:
            if "control_flow" in row:
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: 'control_flow' only applies "
                    "to opcode-backed rows (a frontend-only structural kind needs none)"
                )
            if not isinstance(row["reason"], str) or not row["reason"]:
                raise OpKindTableError(
                    f"frontend_check_exception_skip {kind}: 'reason' must be a "
                    "non-empty string"
                )

    # -- [[binary_op]] (EXHAUSTIVE over ast.operator) -----------------------
    binary = data.get("binary_op", [])
    if not isinstance(binary, list) or not binary:
        raise OpKindTableError("table has no [[binary_op]] rows")
    seen_binary: set[str] = set()
    for row in binary:
        ast_op = row.get("ast_op")
        if not isinstance(ast_op, str) or not ast_op:
            raise OpKindTableError(f"[[binary_op]] row missing 'ast_op': {row}")
        if ast_op in seen_binary:
            raise OpKindTableError(f"duplicate binary_op ast_op: {ast_op}")
        seen_binary.add(ast_op)
        for col in ("binop_kind", "augassign_kind"):
            if not isinstance(row.get(col), str) or not row[col]:
                raise OpKindTableError(
                    f"binary_op {ast_op}: {col!r} must be a non-empty string"
                )
    ast_operator_names = {cls.__name__ for cls in ast.operator.__subclasses__()}
    if seen_binary != ast_operator_names:
        raise OpKindTableError(
            "[[binary_op]] must be EXHAUSTIVE over ast.operator subclasses "
            "(every binary/augmented operator must have a row, or visit_BinOp / "
            "visit_AugAssign would silently miss it — the task-#27 inplace-kind gap):"
            f" table-only={sorted(seen_binary - ast_operator_names)} "
            f"ast-only={sorted(ast_operator_names - seen_binary)}"
        )


# ---------------------------------------------------------------------------
# Rust rendering
# ---------------------------------------------------------------------------

_RS_HEADER = """\
// @generated by tools/gen_op_kinds.py from
// runtime/molt-backend/src/tir/op_kinds.toml. DO NOT EDIT.
//
// The single source of truth for the cross-component op-"kind"-string vocabulary
// (docs/design/foundation/25_op_kind_registry.md). These tables back the
// `kind_to_opcode` mapper (ssa.rs), the `CopyLowering` classifier
// (alias_analysis.rs), the per-OpCode effect oracle (effects.rs), and the
// operand-ownership tables (design 27 §2.1/§2.3, consumed by drop_insertion.rs's
// `op_consumed_operand_root`). A drift between this file and op_kinds.toml is
// caught by tests/test_gen_op_kinds.py; a new op kind that the frontend can emit
// but that is absent here is caught by tools/audit_op_kinds.py --check.

use crate::tir::ops::OpCode;

"""


def render_rs(data: dict) -> str:
    opcodes = data["opcode"]
    kinds = data.get("kind", [])
    prefixes = data.get("classifier_fresh_value_prefixes", [])

    out: list[str] = [_RS_HEADER]

    # -- kind_to_opcode table ------------------------------------------------
    out.append(
        "/// Map a SimpleIR `kind` string to its first-class TIR `OpCode`, or\n"
        "/// `None` when the kind has no first-class opcode (the caller lifts it to\n"
        "/// `OpCode::Copy{_original_kind}`). Mirrors the `|`-grouped arms in the\n"
        "/// table; the round-trip / legacy spellings live in each row's aliases.\n"
        "#[inline]\n"
        "pub(crate) fn kind_to_opcode_table(kind: &str) -> Option<OpCode> {\n"
        "    match kind {\n"
    )
    for row in kinds:
        opcode = row.get("mapper_opcode")
        if opcode is None:
            continue
        if row.get("group") == "gpu":
            out.append(
                "        // GPU offload primitives lower through the call machinery.\n"
            )
        spellings = [row["canonical"], *row.get("aliases", [])]
        pat = " | ".join(f'"{s}"' for s in spellings)
        out.append(f"        {pat} => Some(OpCode::{opcode}),\n")
    out.append("        _ => None,\n")
    out.append("    }\n}\n\n")

    # -- fresh-value classifier exact set ------------------------------------
    fresh = list(data.get("classifier_fresh_value", []))
    out.append(
        "/// EXACT-match arm of `copy_kind_mints_fresh_owned_ref`: kinds whose\n"
        "/// runtime mints a fresh +1 owned reference. The `vec_*` prefix rule is\n"
        "/// applied separately by the caller (see `fresh_value_prefixes`).\n"
        "#[inline]\n"
        "pub(crate) fn copy_kind_mints_fresh_owned_ref_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(fresh))
    out.append("    )\n}\n\n")

    # -- fresh-value prefix rule ---------------------------------------------
    out.append(
        "/// Prefix rules for `copy_kind_mints_fresh_owned_ref`: a kind starting\n"
        "/// with any of these mints a fresh owned reference (e.g. the `vec_*`\n"
        "/// vectorized-reduction family, each calling a dedicated `molt_vec_*`).\n"
        "pub(crate) const FRESH_VALUE_PREFIXES: &[&str] = &[\n"
    )
    for p in prefixes:
        out.append(f'    "{p}",\n')
    out.append("];\n\n")

    # -- inert-marker classifier exact set -----------------------------------
    inert = list(data.get("classifier_inert_marker", []))
    out.append(
        "/// EXACT-match arm of `classify_copy_kind`'s inert bucket: kinds with a\n"
        "/// dedicated RC-inert backend lowering and no surviving heap reference to\n"
        "/// own (`line`/`trace_*`/`missing`/`nop`, the read-only repr/layout guards).\n"
        "#[inline]\n"
        "pub(crate) fn copy_kind_is_inert_marker_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(inert))
    out.append("    )\n}\n\n")

    # -- explicit no-heap-move classifier exact set --------------------------
    no_heap = list(data.get("classifier_no_heap_move", []))
    out.append(
        "/// EXACT-match arm of `copy_kind_is_explicit_no_heap_move`: kinds that are\n"
        "/// a provable no-incref pure move of operand 0 (bare `copy`, the named SSA/\n"
        "/// var moves, the validate-and-pass-through guards). A bare `Copy` with no\n"
        "/// `_original_kind` is handled by the caller (it is also a no-heap move).\n"
        "#[inline]\n"
        "pub(crate) fn copy_kind_is_explicit_no_heap_move_table(kind: &str) -> bool {\n"
        "    matches!(\n"
        "        kind,\n"
    )
    out.append(_render_matches_arm(no_heap))
    out.append("    )\n}\n\n")

    # -- effect oracle: exhaustive over OpCode -------------------------------
    may_throw = [r["name"] for r in opcodes if r["may_throw"]]
    side = [r["name"] for r in opcodes if r["side_effecting"]]
    out.append(
        "/// Whether an `OpCode` may raise an exception (DCE must preserve it even\n"
        "/// when its result is dead). EXHAUSTIVE over the enum — a new variant fails\n"
        "/// to compile until it is classified in op_kinds.toml.\n"
        "#[inline]\n"
        "pub(crate) fn opcode_may_throw_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, may_throw))
    out.append("    }\n}\n\n")

    out.append(
        "/// Whether an `OpCode` has an observable side effect. EXHAUSTIVE over the\n"
        "/// enum — a new variant fails to compile until it is classified.\n"
        "#[inline]\n"
        "pub(crate) fn opcode_is_side_effecting_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_bool_arms(opcodes, side))
    out.append("    }\n}\n\n")

    out.append(
        "/// Purity class for the LICM/GVN purity core (`opcode_effects`). The\n"
        "/// consumer (effects.rs) maps each variant to its `OpEffects` triple,\n"
        "/// keeping that triple's canonical definition on the consumer side:\n"
        "///   `Pure`         => (consistent, effect_free, nothrow) = (T, T, T)\n"
        "///   `PureMayThrow` => (T, T, F)\n"
        "///   `Impure`       => (F, F, F)\n"
        "#[derive(Clone, Copy, PartialEq, Eq)]\n"
        "pub(crate) enum OpcodePurity {\n"
        "    Pure,\n"
        "    PureMayThrow,\n"
        "    Impure,\n"
        "}\n\n"
        "/// Per-OpCode purity class. EXHAUSTIVE over the enum — a new variant fails\n"
        "/// to compile until classified in op_kinds.toml.\n"
        "#[inline]\n"
        "pub(crate) fn opcode_purity_table(opcode: OpCode) -> OpcodePurity {\n"
        "    match opcode {\n"
    )
    out.append(_render_opcode_purity_arms(opcodes))
    out.append("    }\n}\n\n")

    # -- operand ownership: per-OpCode default + per-spelling consume override --
    out.append(_render_operand_ownership(opcodes, data.get("consuming_kind", [])))

    # -- per-terminator operand ownership (the ownership-moves-out / transfer axis) --
    out.append("\n")
    out.append(_render_terminator_ownership(data.get("terminator", [])))

    return "".join(out)


def _render_matches_arm(spellings: list[str]) -> str:
    """Render the body of a `matches!(kind, ...)` as one `|`-joined OR-pattern,
    one spelling per line, in the order given. Empty set renders a never-match
    arm so the function is still well-formed."""
    if not spellings:
        # An empty exact set means "only the prefix/None paths apply". Render a
        # single impossible literal pattern (a NUL-prefixed kind never occurs as
        # a wire spelling) so the `matches!` stays well-formed and always false.
        return '        "\\0__never__"\n'
    lines = []
    for i, s in enumerate(spellings):
        sep = "" if i == len(spellings) - 1 else " |"
        lines.append(f'        "{s}"{sep}\n')
    return "".join(lines)


def _render_opcode_bool_arms(opcodes: list[dict], truthy: list[str]) -> str:
    """Render exhaustive `OpCode::X => bool` arms in table order."""
    truthy_set = set(truthy)
    lines = []
    for row in opcodes:
        name = row["name"]
        lines.append(f"        OpCode::{name} => {_rs_bool(name in truthy_set)},\n")
    return "".join(lines)


_PURITY_VARIANT = {
    "pure": "OpcodePurity::Pure",
    "pure_may_throw": "OpcodePurity::PureMayThrow",
    "impure": "OpcodePurity::Impure",
}


def _render_opcode_purity_arms(opcodes: list[dict]) -> str:
    lines = []
    for row in opcodes:
        name = row["name"]
        variant = _PURITY_VARIANT[row["purity"]]
        lines.append(f"        OpCode::{name} => {variant},\n")
    return "".join(lines)


_OPERAND_OWNERSHIP_VARIANT = {
    "borrowed": "OperandOwnership::Borrowed",
    "consumed": "OperandOwnership::Consumed",
    # Move-out leaves used by the per-TERMINATOR table (design 27 §2.4). The
    # opcode `operand_ownership` validator restricts opcodes to borrowed|consumed;
    # these are reachable only via the terminator categories.
    "transferred": "OperandOwnership::Transferred",
    "none": "OperandOwnership::NoOperandOwnership",
}


def _render_operand_ownership(opcodes: list[dict], consuming: list[dict]) -> str:
    """Render the operand-ownership tables (design 27 §2.1/§2.3):

      * ``OperandOwnership`` — the per-operand borrowed|consumed leaf.
      * ``opcode_operand_ownership_table(opcode, operand_idx)`` — the per-OpCode
        DEFAULT, EXHAUSTIVE over the enum (a new variant fails to compile until
        classified). Honors the per-position list form (a list opcode dispatches
        on ``operand_idx``); a uniform opcode ignores the index.
      * ``kind_consumed_operand_table(kind, arity)`` — the per-SPELLING consume
        override keyed on the ``_original_kind`` attr. Returns the 0-based index
        of the consumed operand, resolving ``"last"`` against the op's ``arity``.
        This is the table ``op_consumed_operand_root`` reads (replacing the
        hand-coded ``matches!(_original_kind, "call_bind" | "call_indirect")``).
    """
    out: list[str] = []
    # `operand_idx` is referenced by the match body ONLY when some opcode carries
    # a per-position list (which renders a `match operand_idx { … }` arm). When
    # every opcode is uniform (`all_borrowed`/`all_consumed`), the index is
    # genuinely unused — emit the idiomatic `_operand_idx` so the generated file
    # stays warning-free (rather than an `#[allow]` blanket). The PUBLIC contract
    # is still "indexed by operand position"; the name flips to `operand_idx` the
    # moment a per-position classification lands.
    any_per_position = any(
        isinstance(row["operand_ownership"], list)
        and len(set(row["operand_ownership"])) > 1
        for row in opcodes
    )
    idx_param = "operand_idx" if any_per_position else "_operand_idx"
    out.append(
        "/// Operand-ownership leaf (design 27 §2.1): does an op release this\n"
        "/// operand internally (`Consumed` — the holder must NOT also drop it, a\n"
        "/// double-free otherwise) or merely borrow it (`Borrowed` — the holder\n"
        "/// keeps its obligation and drops at the value's true last use)? molt's\n"
        "/// `callee borrows all args` ABI (design 20 §1.2) makes `Borrowed` the\n"
        "/// universal default; `Consumed` is the CallArgs-builder / move-into class.\n"
        "/// The result-side lattice (Owned/Borrowed/Raw/MaybeUninit) is the\n"
        "/// classifier_* tables — a SEPARATE axis from this operand-side leaf.\n"
        "///\n"
        "/// The variant set models molt's FULL operand-ownership domain so the\n"
        "/// design-27 ownership-boundary lattice (#58) and the next consumer\n"
        "/// migrations are TABLE edits, not enum surgery. `Borrowed`/`Consumed`\n"
        "/// seed the per-OpCode + per-spelling tables; `Transferred` seeds the\n"
        "/// per-TERMINATOR table (design 27 §2.4 transfer sites — ladder #72). The\n"
        "/// remaining two name EXISTING molt facts whose hand-lists migrate into\n"
        "/// ownership rows in follow-up tranches:\n"
        "///   * `Transferred` — ownership moves OUT of the function/block: a\n"
        "///     `Return` value or a branch-arg passed into a successor block arg.\n"
        "///     LIVE: constructed by `terminator_operand_ownership_table` and read\n"
        "///     by drop_insertion's `terminator_uses_root` / `terminator_branch_args`.\n"
        "///   * `InteriorBorrowKeepAlive` — the round-6 interior-borrow keepalive:\n"
        "///     the operand must stay live because the result holds an INTERIOR\n"
        "///     reference into it (drop deferred to the interior ref's last use).\n"
        "///   * `ConditionalValidOnlyOnEdge` — the §2.8 `IterNextUnboxed` value-out:\n"
        "///     valid only on the not-exhausted edge, NEVER unconditionally\n"
        "///     droppable (stale stack garbage on the exhaustion edge).\n"
        "///   * `NoOperandOwnership` — no ref-bearing operand in that category (a\n"
        "///     raw lane; a terminator category absent on a variant — `Branch` has\n"
        "///     no direct operand, `Return` forwards no branch arg).\n"
        "// `InteriorBorrowKeepAlive`/`ConditionalValidOnlyOnEdge` are seeded as\n"
        "// their consumer hand-lists migrate (the interior-borrow / iter-cond\n"
        "// tranches). The schema is kept ALIVE (not ornamental) by `ALL` +\n"
        "// `from_str`/`as_str` below: every variant is constructed and round-\n"
        "// tripped, so a dropped or renamed variant is a compile/test failure.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub(crate) enum OperandOwnership {\n"
        "    Borrowed,\n"
        "    Consumed,\n"
        "    Transferred,\n"
        "    InteriorBorrowKeepAlive,\n"
        "    ConditionalValidOnlyOnEdge,\n"
        "    NoOperandOwnership,\n"
        "}\n\n"
        "// Parse/render path for the operand-ownership vocabulary. `Transferred`\n"
        "// is now LIVE through `terminator_operand_ownership_table` (ladder #72);\n"
        "// `from_str` remains the toml-ingest path the REMAINING migrations\n"
        "// (InteriorBorrowKeepAlive / ConditionalValidOnlyOnEdge rows) read and is\n"
        "// not yet wired to a runtime caller, so `from_str`/`as_str`/`ALL` keep\n"
        "// allow(dead_code) — SCOPED to this forward-compat parse API, never the\n"
        "// enum (every variant is constructed) nor the file. `ALL` + the round-\n"
        "// trip test keep every variant constructed and live today.\n"
        "#[allow(dead_code)]\n"
        "impl OperandOwnership {\n"
        "    pub(crate) const ALL: [OperandOwnership; 6] = [\n"
        "        OperandOwnership::Borrowed,\n"
        "        OperandOwnership::Consumed,\n"
        "        OperandOwnership::Transferred,\n"
        "        OperandOwnership::InteriorBorrowKeepAlive,\n"
        "        OperandOwnership::ConditionalValidOnlyOnEdge,\n"
        "        OperandOwnership::NoOperandOwnership,\n"
        "    ];\n"
        "    pub(crate) fn as_str(self) -> &'static str {\n"
        "        match self {\n"
        "            OperandOwnership::Borrowed => \"borrowed\",\n"
        "            OperandOwnership::Consumed => \"consumed\",\n"
        "            OperandOwnership::Transferred => \"transferred\",\n"
        "            OperandOwnership::InteriorBorrowKeepAlive => \"interior_borrow_keepalive\",\n"
        "            OperandOwnership::ConditionalValidOnlyOnEdge => \"conditional_valid_only_on_edge\",\n"
        "            OperandOwnership::NoOperandOwnership => \"no_operand_ownership\",\n"
        "        }\n"
        "    }\n"
        "    pub(crate) fn from_str(s: &str) -> Option<OperandOwnership> {\n"
        "        match s {\n"
        "            \"borrowed\" => Some(OperandOwnership::Borrowed),\n"
        "            \"consumed\" => Some(OperandOwnership::Consumed),\n"
        "            \"transferred\" => Some(OperandOwnership::Transferred),\n"
        "            \"interior_borrow_keepalive\" => Some(OperandOwnership::InteriorBorrowKeepAlive),\n"
        "            \"conditional_valid_only_on_edge\" => Some(OperandOwnership::ConditionalValidOnlyOnEdge),\n"
        "            \"no_operand_ownership\" => Some(OperandOwnership::NoOperandOwnership),\n"
        "            _ => None,\n"
        "        }\n"
        "    }\n"
        "}\n\n"
        "#[cfg(test)]\n"
        "mod operand_ownership_schema_tests {\n"
        "    use super::OperandOwnership;\n"
        "    #[test]\n"
        "    fn every_variant_round_trips() {\n"
        "        // The schema is alive: every declared variant parses + renders +\n"
        "        // round-trips. Dropping or renaming a variant breaks this test.\n"
        "        for v in OperandOwnership::ALL {\n"
        "            assert_eq!(OperandOwnership::from_str(v.as_str()), Some(v));\n"
        "        }\n"
        "        assert_eq!(OperandOwnership::from_str(\"bogus\"), None);\n"
        "    }\n"
        "}\n\n"
    )

    out.append(
        "/// Per-OpCode operand-ownership DEFAULT: how `OpCode` treats the operand\n"
        "/// at `operand_idx`. EXHAUSTIVE over the enum — a new variant fails to\n"
        "/// compile until it is given an `operand_ownership` row in op_kinds.toml.\n"
        "/// A uniform opcode (`all_borrowed`/`all_consumed`) ignores the index; a\n"
        "/// per-position opcode dispatches on it (positions past the listed arity\n"
        "/// fall back to the LAST listed leaf — variadic tails inherit the final\n"
        "/// position's treatment). This is the per-OpCode floor; a finer\n"
        "/// per-`_original_kind` consume is `kind_consumed_operand_table`.\n"
        "#[inline]\n"
        "pub(crate) fn opcode_operand_ownership_table(\n"
        "    opcode: OpCode,\n"
        f"    {idx_param}: usize,\n"
        ") -> OperandOwnership {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        spec = row["operand_ownership"]
        out.append(f"        OpCode::{name} => {_operand_ownership_arm(spec)},\n")
    out.append("    }\n}\n\n")

    out.append(
        "/// Per-SPELLING consume override (design 27 §2.3): for a `Copy`-lifted op\n"
        "/// carrying `_original_kind = kind`, the 0-based index of the operand the\n"
        "/// op CONSUMES (frees internally), or `None` if it consumes none. `arity`\n"
        "/// is the op's operand count, used to resolve a `\"last\"` selector. The\n"
        "/// drop pass treats a value whose last use is the consumed-operand\n"
        "/// position exactly like a `Return` transfer — no trailing `DecRef`.\n"
        "/// Replaces the hand-coded `op_consumed_operand_root` match.\n"
        "#[inline]\n"
        "pub(crate) fn kind_consumed_operand_table(kind: &str, arity: usize) -> Option<usize> {\n"
        "    match kind {\n"
    )
    if consuming:
        for row in consuming:
            kind = row["kind"]
            sel = row["consumed_operand"]
            if sel == "last":
                out.append(
                    f'        "{kind}" => arity.checked_sub(1),\n'
                )
            else:
                out.append(f'        "{kind}" => Some({int(sel)}),\n')
    out.append("        _ => None,\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_terminator_ownership(terminators: list[dict]) -> str:
    """Render the per-TERMINATOR operand-ownership authority (design 27 §2.4):

      * ``TerminatorKind`` — a zero-cost discriminant of the ``Terminator`` enum
        (blocks.rs) the table is keyed on (the drop pass maps ``&Terminator`` ->
        ``TerminatorKind`` with one structural match). EXHAUSTIVE over the enum.
      * ``OperandCategory`` — ``Direct`` (the terminator's own operands: a
        ``Return`` value, a ``CondBranch``/``Switch`` predicate) vs ``BranchArg``
        (a value forwarded into a successor's phi). The two categories have
        different ownership, so they are classified independently.
      * ``terminator_operand_ownership_table(kind, category)`` — the per-(variant,
        category) ``OperandOwnership`` leaf, EXHAUSTIVE over both axes.
      * ``terminator_operand_is_transferred(kind, category)`` — the derived
        predicate drop_insertion reads: ``true`` iff the leaf is ``Transferred``
        (ownership moves OUT — no trailing ``DecRef`` at the transfer point). This
        is the generated authority that REPLACES the hand-coded transfer carve-out
        in ``terminator_branch_args`` + the ``Return`` arm of ``terminator_uses_root``.
    """
    out: list[str] = []
    out.append(
        "/// Zero-cost discriminant of the `Terminator` enum (blocks.rs) the\n"
        "/// per-terminator operand-ownership table is keyed on. EXHAUSTIVE over the\n"
        "/// enum — a new `Terminator` variant fails to render until it is given a\n"
        "/// [[terminator]] row in op_kinds.toml (the transfer-carve-out kill: an\n"
        "/// unclassified terminator can't silently inherit a borrow/transfer\n"
        "/// assumption). The drop pass maps `&Terminator` -> `TerminatorKind` with\n"
        "/// one structural match; this keeps the ownership FACT declarative while\n"
        "/// the structural shape (which fields carry args) stays in the pass.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub(crate) enum TerminatorKind {\n"
    )
    for row in terminators:
        out.append(f"    {row['name']},\n")
    out.append("}\n\n")

    out.append(
        "/// Which operand CATEGORY of a terminator a query is about: the\n"
        "/// terminator's own `Direct` operands (a `Return` value, a `CondBranch`/\n"
        "/// `Switch` predicate) versus a `BranchArg` forwarded into a successor's\n"
        "/// block-arg (phi). The two have different ownership (a `Return` value\n"
        "/// transfers to the caller; a predicate is borrowed; a branch-arg transfers\n"
        "/// into the phi) so they are classified on separate axes.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub(crate) enum OperandCategory {\n"
        "    Direct,\n"
        "    BranchArg,\n"
        "}\n\n"
    )

    out.append(
        "/// Per-(terminator variant, operand category) ownership leaf (design 27\n"
        "/// §2.4). EXHAUSTIVE over both axes — a new `Terminator` variant fails to\n"
        "/// compile until classified. `Transferred` = ownership moves OUT (a\n"
        "/// `Return` value to the caller; a branch-arg into a successor phi);\n"
        "/// `Borrowed` = the predicate is read but not moved (drop relocated to the\n"
        "/// dying edge); `NoOperandOwnership` = the variant has no operand in that\n"
        "/// category. The consume axis is N/A for a terminator (nothing frees a\n"
        "/// terminator operand internally), so `Consumed` never appears here.\n"
        "#[inline]\n"
        "pub(crate) fn terminator_operand_ownership_table(\n"
        "    kind: TerminatorKind,\n"
        "    category: OperandCategory,\n"
        ") -> OperandOwnership {\n"
        "    match (kind, category) {\n"
    )
    for row in terminators:
        name = row["name"]
        direct = _OPERAND_OWNERSHIP_VARIANT[row["direct"]]
        branch = _OPERAND_OWNERSHIP_VARIANT[row["branch_arg"]]
        out.append(
            f"        (TerminatorKind::{name}, OperandCategory::Direct) => {direct},\n"
        )
        out.append(
            f"        (TerminatorKind::{name}, OperandCategory::BranchArg) => {branch},\n"
        )
    out.append("    }\n}\n\n")

    out.append(
        "/// Derived transfer predicate drop_insertion reads (design 27 §2.4): does\n"
        "/// the terminator TRANSFER ownership of an operand in `category`? `true`\n"
        "/// iff the leaf is `Transferred` — the drop pass must NOT emit a trailing\n"
        "/// `DecRef` at the transfer point (the caller / successor phi owns it).\n"
        "/// This single declarative authority REPLACES the hand-coded transfer\n"
        "/// carve-out (the `Return` arm of `terminator_uses_root` + the\n"
        "/// `terminator_branch_args` membership). A future terminator transfer fact\n"
        "/// is a [[terminator]] row edit, never a drop-pass edit.\n"
        "#[inline]\n"
        "pub(crate) fn terminator_operand_is_transferred(\n"
        "    kind: TerminatorKind,\n"
        "    category: OperandCategory,\n"
        ") -> bool {\n"
        "    matches!(\n"
        "        terminator_operand_ownership_table(kind, category),\n"
        "        OperandOwnership::Transferred\n"
        "    )\n"
        "}\n"
    )
    return "".join(out)


def _operand_ownership_arm(spec: object) -> str:
    """Render the RHS of one `opcode_operand_ownership_table` match arm.

    A uniform spec collapses to a constant variant; a per-position list renders a
    nested `match operand_idx` whose final listed position also serves every
    higher index (the variadic-tail rule), keeping the function total."""
    if spec == "all_borrowed":
        return "OperandOwnership::Borrowed"
    if spec == "all_consumed":
        return "OperandOwnership::Consumed"
    assert isinstance(spec, list)
    leaves = [_OPERAND_OWNERSHIP_VARIANT[x] for x in spec]
    if len(set(leaves)) == 1:
        # A homogeneous list is just the uniform case (e.g. ["borrowed"]).
        return leaves[0]
    arms = []
    for i, leaf in enumerate(leaves[:-1]):
        arms.append(f"{i} => {leaf}")
    # The final listed position is the catch-all (covers its index AND any
    # higher variadic-tail index).
    arms.append(f"_ => {leaves[-1]}")
    return "match operand_idx { " + ", ".join(arms) + " }"


# ---------------------------------------------------------------------------
# Python rendering (frontend canonical spellings)
# ---------------------------------------------------------------------------

_PY_HEADER = """\
# @generated by tools/gen_op_kinds.py from
# runtime/molt-backend/src/tir/op_kinds.toml. DO NOT EDIT.
#
# The canonical JSON wire "kind" spellings the frontend emitter (map_ops_to_json
# in serialization.py) must use, so the producer and the backend `kind_to_opcode`
# mapper share ONE spelling. Sourced from op_kinds.toml (the cross-component
# single source of truth, docs/design/foundation/25_op_kind_registry.md).
#
# `CANONICAL_KIND` maps every alias spelling to its canonical wire kind; the
# emitter routes its spelling through it so a `floordiv`/`floor_div`-style schism
# can never re-open. `tests/test_gen_op_kinds.py` pins this file in sync.
#
# This file ALSO carries the frontend's four pre-serialization `op.kind` tables
# (molt task #44, F2a), absorbed from the hand-kept structures that previously
# lived in src/molt/frontend/__init__.py:
#   RAISING_KIND_NAMES         — op.kinds that can raise (emit() attaches the
#                                caret col_offset), from [[frontend_raising_kind]].
#   CHECK_EXCEPTION_SKIP_KINDS — op.kinds after which emit() skips the auto
#                                CHECK_EXCEPTION, from [[frontend_check_exception_skip]].
#   BINOP_OP_KIND / AUGASSIGN_OP_KIND — ast.operator subclass __name__ -> the
#                                binary / augmented-assignment op.kind, from
#                                [[binary_op]] (EXHAUSTIVE over ast.operator).

from __future__ import annotations

"""


def render_py(data: dict) -> str:
    kinds = data.get("kind", [])
    out: list[str] = [_PY_HEADER]

    # The canonical-spelling map: every spelling (canonical or alias) -> canonical.
    out.append("CANONICAL_KIND: dict[str, str] = {\n")
    for row in kinds:
        canon = row["canonical"]
        for spelling in [canon, *row.get("aliases", [])]:
            out.append(f'    "{spelling}": "{canon}",\n')
    out.append("}\n\n")

    # The set of canonical wire kinds (the emitter's allowed output vocabulary
    # for kinds that have a first-class mapper opcode).
    mapper_canon = [r["canonical"] for r in kinds if r.get("mapper_opcode") is not None]
    out.append("MAPPER_CANONICAL_KINDS: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for canon in mapper_canon:
        out.append(f'        "{canon}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    # -- frontend op.kind tables (F2a) --------------------------------------
    raising = data.get("frontend_raising_kind", [])
    out.append(
        "# Frontend `op.kind`s that can raise at runtime — emit() attaches the\n"
    )
    out.append(
        "# expression-level col_offset for traceback caret annotations. Each row\n"
    )
    out.append(
        "# is either an opcode-mapped may_throw kind (cross-checked against the\n"
    )
    out.append(
        "# [[opcode]] oracle at generation) or a documented frontend-specific kind.\n"
    )
    out.append("RAISING_KIND_NAMES: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for row in raising:
        out.append(f'        "{row["kind"]}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    skip = data.get("frontend_check_exception_skip", [])
    out.append("# Frontend `op.kind`s after which emit() does NOT auto-insert a\n")
    out.append(
        "# CHECK_EXCEPTION (control-flow / structural kinds, plus the two may_throw\n"
    )
    out.append("# kinds whose exceptional edge is handled structurally — RAISE,\n")
    out.append(
        "# STATE_TRANSITION). NOT the complement of may_throw; see op_kinds.toml.\n"
    )
    out.append("CHECK_EXCEPTION_SKIP_KINDS: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for row in skip:
        out.append(f'        "{row["kind"]}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    binary = data.get("binary_op", [])
    out.append(
        "# `ast.operator` subclass __name__ -> the binary-form frontend op.kind\n"
    )
    out.append(
        "# (visit_BinOp). EXHAUSTIVE over ast.operator (generation-time checked).\n"
    )
    out.append("BINOP_OP_KIND: dict[str, str] = {\n")
    for row in binary:
        out.append(f'    "{row["ast_op"]}": "{row["binop_kind"]}",\n')
    out.append("}\n\n")

    out.append(
        "# `ast.operator` subclass __name__ -> the augmented-assignment op.kind\n"
    )
    out.append(
        "# (visit_AugAssign). The in-place kind routes through the in-place dunder\n"
    )
    out.append(
        "# (__iadd__/__ifloordiv__/...) before the binary fallback, matching CPython.\n"
    )
    out.append("AUGASSIGN_OP_KIND: dict[str, str] = {\n")
    for row in binary:
        out.append(f'    "{row["ast_op"]}": "{row["augassign_kind"]}",\n')
    out.append("}\n\n\n")

    out.append("def canonical_kind(kind: str) -> str:\n")
    out.append('    """Return the canonical wire spelling for *kind*.\n\n')
    out.append(
        "    Identity for any kind not in the registry (the overwhelming common\n"
    )
    out.append(
        "    case: the kind is already canonical). The registry only records the\n"
    )
    out.append('    alias collapses that exist today (e.g. the floordiv family)."""\n')
    out.append("    return CANONICAL_KIND.get(kind, kind)\n")

    return "".join(out)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def _check(path: Path, rendered: str) -> bool:
    """Return True if *path* is in sync with *rendered* (prints a diff hint)."""
    if not path.exists():
        print(f"MISSING generated file: {path}", file=sys.stderr)
        return False
    current = path.read_text()
    if current != rendered:
        print(
            f"STALE generated file: {path}\n"
            f"  run `python3 tools/gen_op_kinds.py` to regenerate from "
            f"{TABLE.relative_to(ROOT)}",
            file=sys.stderr,
        )
        return False
    return True


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if a generated file is stale (CI mode); do not write",
    )
    args = ap.parse_args(argv)

    data = load_table()
    rs = render_rs(data)
    py = render_py(data)

    if args.check:
        ok = _check(OUT_RS, rs)
        ok = _check(OUT_PY, py) and ok
        if ok:
            print("op-kind generated files: in sync")
        return 0 if ok else 1

    OUT_RS.write_text(rs)
    OUT_PY.write_text(py)
    print(f"wrote {OUT_RS.relative_to(ROOT)}")
    print(f"wrote {OUT_PY.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
