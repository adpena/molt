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

    _validate_frontend_tables(data, opcodes)

    return data


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
            raise OpKindTableError(f"[[frontend_raising_kind]] row missing 'kind': {row}")
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
        raise OpKindTableError(
            "table has no [[frontend_check_exception_skip]] rows"
        )
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
// (alias_analysis.rs), and the per-OpCode effect oracle (effects.rs). A drift
// between this file and op_kinds.toml is caught by tests/test_gen_op_kinds.py;
// a new op kind that the frontend can emit but that is absent here is caught by
// tools/audit_op_kinds.py --check.

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
    out.append("    }\n}\n")

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
    out.append("# Frontend `op.kind`s that can raise at runtime — emit() attaches the\n")
    out.append("# expression-level col_offset for traceback caret annotations. Each row\n")
    out.append("# is either an opcode-mapped may_throw kind (cross-checked against the\n")
    out.append("# [[opcode]] oracle at generation) or a documented frontend-specific kind.\n")
    out.append("RAISING_KIND_NAMES: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for row in raising:
        out.append(f'        "{row["kind"]}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    skip = data.get("frontend_check_exception_skip", [])
    out.append("# Frontend `op.kind`s after which emit() does NOT auto-insert a\n")
    out.append("# CHECK_EXCEPTION (control-flow / structural kinds, plus the two may_throw\n")
    out.append("# kinds whose exceptional edge is handled structurally — RAISE,\n")
    out.append("# STATE_TRANSITION). NOT the complement of may_throw; see op_kinds.toml.\n")
    out.append("CHECK_EXCEPTION_SKIP_KINDS: frozenset[str] = frozenset(\n")
    out.append("    {\n")
    for row in skip:
        out.append(f'        "{row["kind"]}",\n')
    out.append("    }\n")
    out.append(")\n\n")

    binary = data.get("binary_op", [])
    out.append("# `ast.operator` subclass __name__ -> the binary-form frontend op.kind\n")
    out.append("# (visit_BinOp). EXHAUSTIVE over ast.operator (generation-time checked).\n")
    out.append("BINOP_OP_KIND: dict[str, str] = {\n")
    for row in binary:
        out.append(f'    "{row["ast_op"]}": "{row["binop_kind"]}",\n')
    out.append("}\n\n")

    out.append("# `ast.operator` subclass __name__ -> the augmented-assignment op.kind\n")
    out.append("# (visit_AugAssign). The in-place kind routes through the in-place dunder\n")
    out.append("# (__iadd__/__ifloordiv__/...) before the binary fallback, matching CPython.\n")
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
