from __future__ import annotations

import ast

from .errors import OpKindTableError
from .schema import _FRONTEND_EFFECT_VALUES


def _frontend_wire_spelling_to_op_kind(spelling: str) -> str:
    """Map a wire-kind spelling to the frontend's pre-serialization op.kind."""

    return spelling.upper()


def _frontend_effect_from_opcode(row: dict) -> str:
    """Return the frontend optimizer's memory-effect class for an OpCode row.

    This is the alias/CSE axis only. Raising capability is a separate DCE
    barrier rendered from [[frontend_raising_kind]] as RAISING_KIND_NAMES.
    """

    if row["side_effecting"]:
        return "writes_heap"
    if row["purity"] == "impure":
        return "reads_heap"
    return "pure"


def _frontend_raising_nothrow_on_primitives(data: dict) -> set[str]:
    """Return raising kinds whose raise is disproved by primitive constants."""

    out: set[str] = set()
    for row in data.get("frontend_raising_kind", []):
        if row.get("nothrow_on_primitives", False):
            out.add(row["kind"])
    return out


def _simpleir_registered_runtime_kinds(data: dict) -> set[str]:
    """Derive every wire spelling covered by runtime-semantic admission."""

    registered: set[str] = set()
    for row in data.get("kind", []):
        registered.add(row["canonical"])
        registered.update(row.get("aliases", []))
    for table in ("simpleir_control_kind", "frontend_effect_kind"):
        registered.update(row["kind"] for row in data.get(table, []))
    for row in data.get("simpleir_runtime_requirement_roles", []):
        registered.update(data.get(row["table"], []))
    registered.update(data.get("simpleir_runtime_neutral_semantics_kinds", []))
    return registered


def _frontend_effect_class_map(data: dict) -> dict[str, str]:
    """Build the generated frontend pre-serialization effect oracle."""

    opcodes_by_name = {row["name"]: row for row in data.get("opcode", [])}
    effects: dict[str, str] = {}

    for row in data.get("kind", []):
        opcode_name = row.get("mapper_opcode")
        if not opcode_name:
            continue
        opcode = opcodes_by_name[opcode_name]
        effect = _frontend_effect_from_opcode(opcode)
        for spelling in [row["canonical"], *row.get("aliases", [])]:
            effects[_frontend_wire_spelling_to_op_kind(spelling)] = effect

    # [[frontend_raising_kind]] is the may-raise axis, not the memory axis.
    # Memory classes come from opcode facts above or explicit
    # [[frontend_effect_kind]] overrides below.

    for row in data.get("simpleir_control_kind", []):
        if any(
            row.get(flag, False)
            for flag in (
                "structural",
                "terminator",
                "suspend",
                "repoll",
                "block_leader",
                "block_ender",
                "conditional_branch",
            )
        ):
            effects[_frontend_wire_spelling_to_op_kind(row["kind"])] = "control"

    for row in data.get("frontend_check_exception_skip", []):
        kind = row["kind"]
        if row.get("control_flow", False) or kind.startswith(("EXCEPTION_", "STATE_")):
            effects[kind] = "control"

    for row in data.get("frontend_effect_kind", []):
        effects[row["kind"]] = row["effect"]

    return effects


def _validate_frontend_tables(data: dict, opcodes: list[dict]) -> None:
    """Structurally validate the frontend `op.kind` tables.

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
        if "nothrow_on_primitives" in row and not isinstance(
            row["nothrow_on_primitives"], bool
        ):
            raise OpKindTableError(
                f"frontend_raising_kind {kind}: 'nothrow_on_primitives' must be a bool"
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

    # -- [[frontend_effect_kind]] ------------------------------------------
    frontend_effect_rows = data.get("frontend_effect_kind", [])
    if not isinstance(frontend_effect_rows, list) or not frontend_effect_rows:
        raise OpKindTableError("table has no [[frontend_effect_kind]] rows")
    seen_effect: set[str] = set()
    for row in frontend_effect_rows:
        kind = row.get("kind")
        if not isinstance(kind, str) or not kind:
            raise OpKindTableError(
                f"[[frontend_effect_kind]] row missing 'kind': {row}"
            )
        if kind in seen_effect:
            raise OpKindTableError(f"duplicate frontend_effect_kind: {kind}")
        seen_effect.add(kind)
        effect = row.get("effect")
        if effect not in _FRONTEND_EFFECT_VALUES:
            raise OpKindTableError(
                f"frontend_effect_kind {kind}: effect must be one of "
                f"{sorted(_FRONTEND_EFFECT_VALUES)}, got {effect!r}"
            )
        if not isinstance(row.get("reason"), str) or not row["reason"]:
            raise OpKindTableError(
                f"frontend_effect_kind {kind}: 'reason' must be a non-empty string"
            )

    effect_map = _frontend_effect_class_map(data)
    raising_kinds = {row["kind"] for row in raising}

    required_effects = {
        "ADD": "pure",
        "SUB": "pure",
        "MUL": "pure",
        "EQ": "pure",
        "NE": "pure",
        "LT": "pure",
        "LE": "pure",
        "GT": "pure",
        "GE": "pure",
        "NEG": "pure",
        "POS": "pure",
        "INVERT": "pure",
        "ABS": "pure",
        "CONST_STR": "pure",
        "INDEX": "reads_heap",
        "GET_ATTR": "reads_heap",
        "MODULE_GET_ATTR": "reads_heap",
        "GETATTR_GENERIC_OBJ": "reads_heap",
        "GUARDED_GETATTR": "reads_heap",
        "LOAD_VAR": "reads_heap",
        "STORE_VAR": "writes_heap",
        "SETATTR_GENERIC_OBJ": "writes_heap",
        "CHECK_EXCEPTION": "control",
        "STATE_TRANSITION": "control",
        "EXCEPTION_MATCH_BUILTIN": "reads_heap",
    }
    for kind, expected in required_effects.items():
        actual = effect_map.get(kind)
        if actual != expected:
            raise OpKindTableError(
                f"frontend memory-effect invariant {kind}: expected {expected}, "
                f"got {actual}"
            )

    required_raising = {
        "ADD": True,
        "EQ": True,
        "NEG": True,
        "ABS": True,
        "INVERT": True,
        "GET_ATTR": True,
        "INDEX": True,
        "MODULE_GET_ATTR": True,
        "SETATTR_GENERIC_OBJ": True,
        "PHI": False,
        "CONST_STR": False,
        "LOAD_VAR": False,
        "STORE_VAR": False,
    }
    for kind, should_raise in required_raising.items():
        actual = kind in raising_kinds
        if actual != should_raise:
            raise OpKindTableError(
                f"frontend raising-axis invariant {kind}: expected {should_raise}, "
                f"got {actual}"
            )

    nothrow_on_primitives = _frontend_raising_nothrow_on_primitives(data)
    for kind in ("ADD", "SUB", "MUL", "EQ", "NEG", "ABS", "INVERT"):
        if kind not in nothrow_on_primitives:
            raise OpKindTableError(
                f"frontend primitive-nothrow invariant {kind}: missing opt-in"
            )
    for kind in ("DIV", "FLOORDIV", "MOD", "POW", "LSHIFT", "RSHIFT", "IN", "NOT_IN"):
        if kind in nothrow_on_primitives:
            raise OpKindTableError(
                f"frontend primitive-nothrow invariant {kind}: unsafe opt-in"
            )
