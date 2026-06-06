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

    return data


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
    out.append(")\n\n\n")

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
