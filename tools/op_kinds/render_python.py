from __future__ import annotations

from .validate import _frontend_effect_class_map

def _canonical_kinds_for_opcodes(data: dict, opcodes: set[str]) -> list[str]:
    out: set[str] = set()
    for row in data.get("kind", []):
        if row.get("mapper_opcode") in opcodes:
            out.add(row["canonical"])
    return sorted(out)


def _render_py_frozenset(name: str, values: list[str]) -> str:
    out: list[str] = [f"{name}: frozenset[str] = frozenset(\n", "    {\n"]
    for value in sorted(values):
        out.append(f'        "{value}",\n')
    out.extend(["    }\n", ")\n\n"])
    return "".join(out)


def _render_py_binary_image_fact_sets(data: dict) -> str:
    heap_roots = set(
        _canonical_kinds_for_opcodes(
            data, set(data.get("escape_alloc_site_opcodes", []))
        )
    )
    heap_roots.update(data.get("classifier_fresh_value", []))
    heap_roots.update(data.get("classifier_exception_creation_ref", []))
    heap_roots.update(row["kind"] for row in data.get("absorbing_kind", []))

    stack_roots = set(
        _canonical_kinds_for_opcodes(data, {"StackAlloc", "ObjectNewBoundStack"})
    )
    ref_retain = set(
        _canonical_kinds_for_opcodes(
            data, set(data.get("refcount_balance_inc_opcodes", []))
        )
    )
    ref_retain.update(data.get("classifier_owned_alias", []))
    ref_retain.update(
        _canonical_kinds_for_opcodes(
            data,
            {
                row["name"]
                for row in data.get("opcode", [])
                if row.get("result_mints_owned_selected_operand", False)
            },
        )
    )

    ref_release = set(
        _canonical_kinds_for_opcodes(
            data, set(data.get("refcount_balance_dec_opcodes", []))
        )
    )
    ref_release.update(
        _canonical_kinds_for_opcodes(
            data,
            {row["opcode"] for row in data.get("explicit_release_operand", [])},
        )
    )
    ref_release.update(_canonical_kinds_for_opcodes(data, {"Free"}))

    heap_exposure = set(
        _canonical_kinds_for_opcodes(
            data, set(data.get("refcount_heap_exposure_opcodes", []))
        )
    )
    heap_exposure.update(row["kind"] for row in data.get("absorbing_kind", []))
    heap_exposure.update(row["kind"] for row in data.get("absorbing_operand_kind", []))

    out: list[str] = []
    out.append("# Binary-image allocation/ownership analysis categories. These are\n")
    out.append(
        "# generated from the same opcode and preserved-Copy ownership facts that\n"
    )
    out.append(
        "# TIR, escape analysis, drop insertion, and refcount analysis consume.\n"
    )
    out.append(
        "# The analyzer canonicalizes first-class aliases before checking these\n"
    )
    out.append("# sets; preserved Copy spellings stay explicit registry facts.\n")
    out.append(
        _render_py_frozenset("BINARY_IMAGE_HEAP_ALLOC_ROOT_KINDS", sorted(heap_roots))
    )
    out.append(
        _render_py_frozenset("BINARY_IMAGE_STACK_ALLOC_ROOT_KINDS", sorted(stack_roots))
    )
    out.append(
        _render_py_frozenset("BINARY_IMAGE_REF_RETAIN_KINDS", sorted(ref_retain))
    )
    out.append(
        _render_py_frozenset("BINARY_IMAGE_REF_RELEASE_KINDS", sorted(ref_release))
    )
    out.append(
        _render_py_frozenset("BINARY_IMAGE_HEAP_EXPOSURE_KINDS", sorted(heap_exposure))
    )
    return "".join(out)


# ---------------------------------------------------------------------------
# Python rendering (frontend canonical spellings)
# ---------------------------------------------------------------------------

def _render_py_frontend_effect_sets(data: dict) -> str:
    effects = _frontend_effect_class_map(data)
    out: list[str] = []
    out.append(
        "# Frontend pre-serialization optimizer effect classes. Mapper opcode\n"
    )
    out.append(
        "# spellings derive from [[kind]] + [[opcode]], may-raise frontend spellings\n"
    )
    out.append(
        "# derive from [[frontend_raising_kind]], CFG/state facts derive from\n"
    )
    out.append(
        "# [[simpleir_control_kind]], and frontend-only overrides come from\n"
    )
    out.append("# [[frontend_effect_kind]].\n")
    out.append("FRONTEND_EFFECT_CLASS: dict[str, str] = {\n")
    for kind in sorted(effects):
        out.append(f'    "{kind}": "{effects[kind]}",\n')
    out.append("}\n\n")

    for effect, const_name in (
        ("pure", "FRONTEND_EFFECT_PURE_KINDS"),
        ("reads_heap", "FRONTEND_EFFECT_READS_HEAP_KINDS"),
        ("writes_heap", "FRONTEND_EFFECT_WRITES_HEAP_KINDS"),
        ("control", "FRONTEND_EFFECT_CONTROL_KINDS"),
    ):
        out.append(f"{const_name}: frozenset[str] = frozenset(\n")
        out.append("    {\n")
        for kind in sorted(
            kind for kind, row_effect in effects.items() if row_effect == effect
        ):
            out.append(f'        "{kind}",\n')
        out.append("    }\n")
        out.append(")\n\n")
    return "".join(out)


_PY_HEADER = """\
# @generated by tools/gen_op_kinds.py from
# runtime/molt-ir/src/tir/op_kinds.toml. DO NOT EDIT.
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
# This file ALSO carries the frontend's pre-serialization `op.kind` authorities
# (molt task #44, F2a), absorbed from hand-kept frontend structures:
#   RAISING_KIND_NAMES         — op.kinds that can raise (emit() attaches the
#                                caret col_offset), from [[frontend_raising_kind]].
#   CHECK_EXCEPTION_SKIP_KINDS — op.kinds after which emit() skips the auto
#                                CHECK_EXCEPTION, from [[frontend_check_exception_skip]].
#   FRONTEND_EFFECT_CLASS      — pre-serialization optimizer effect classes,
#                                derived from mapper opcodes, control tables,
#                                and [[frontend_effect_kind]] overrides.
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

    out.append(_render_py_frontend_effect_sets(data))

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

    out.append(_render_py_binary_image_fact_sets(data))

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

__all__ = [
    name
    for name in globals()
    if name.startswith('_') and not name.startswith('__')
    or name == 'render_py'
]
