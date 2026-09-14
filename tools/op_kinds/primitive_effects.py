"""One declarative primitive operator effect matrix for Python and Rust."""

from __future__ import annotations

from dataclasses import dataclass
from itertools import product

from .errors import OpKindTableError


PRIMITIVE_FRONTEND_TYPES = {
    "I64": "int",
    "BigInt": "int",
    "F64": "float",
    "Bool": "bool",
    "Str": "str",
    "Bytes": "bytes",
    "None": "None",
}


def comparison_warning_pairs(data: dict) -> tuple[tuple[str, str], ...]:
    rows = data.get("comparison_warning_pairs")
    if not isinstance(rows, list):
        raise OpKindTableError("comparison_warning_pairs must be an array")
    pairs: set[tuple[str, str]] = set()
    for row in rows:
        if (
            not isinstance(row, list)
            or len(row) != 2
            or any(
                not isinstance(ty, str) or ty not in PRIMITIVE_FRONTEND_TYPES
                for ty in row
            )
        ):
            raise OpKindTableError(
                "comparison warning pair requires two exact scalar types"
            )
        pair = tuple(sorted(row))
        if pair in pairs:
            raise OpKindTableError("duplicate comparison warning pair")
        pairs.add(pair)
    return tuple(sorted(pairs))


@dataclass(frozen=True)
class PrimitiveEffectCase:
    opcodes: tuple[str, ...]
    operands: tuple[tuple[str, ...], ...]
    purity: str


def primitive_effect_cases(data: dict) -> tuple[PrimitiveEffectCase, ...]:
    rows = data.get("primitive_operator_effect_cases")
    if not isinstance(rows, list) or not rows:
        raise OpKindTableError(
            "primitive_operator_effect_cases must be a nonempty list"
        )
    opcodes = {row["name"]: row for row in data["opcode"]}
    seen: set[tuple[str, tuple[str, ...]]] = set()
    cases: list[PrimitiveEffectCase] = []
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"opcodes", "operands", "purity"}:
            raise OpKindTableError(
                "primitive operator effect case requires opcodes, operands, purity"
            )
        names, operands, purity = row["opcodes"], row["operands"], row["purity"]
        if (
            not isinstance(names, list)
            or not names
            or any(not isinstance(n, str) or n not in opcodes for n in names)
        ):
            raise OpKindTableError(
                "primitive operator effect case has unknown or missing opcode"
            )
        if len(set(names)) != len(names):
            raise OpKindTableError(
                "primitive operator effect case has duplicate opcode"
            )
        if not isinstance(operands, list) or len(operands) not in (1, 2):
            raise OpKindTableError(
                "primitive operator effect case requires unary or binary operands"
            )
        for group in operands:
            if (
                not isinstance(group, list)
                or not group
                or any(
                    not isinstance(ty, str) or ty not in PRIMITIVE_FRONTEND_TYPES
                    for ty in group
                )
            ):
                raise OpKindTableError(
                    "primitive operator effect case has unknown or empty operand domain"
                )
            if len(set(group)) != len(group):
                raise OpKindTableError(
                    "primitive operator effect case has duplicate operand type"
                )
        if purity not in ("pure", "pure_may_throw"):
            raise OpKindTableError("primitive operator effect case has invalid purity")
        for name in names:
            opcode = opcodes[name]
            if not (
                opcode["may_throw"]
                and opcode["side_effecting"]
                and opcode["purity"] == "impure"
            ):
                raise OpKindTableError(
                    f"primitive operator {name} must retain conservative coarse effects"
                )
            if type(opcode.get("operand_arity")) is not int or opcode[
                "operand_arity"
            ] != len(operands):
                raise OpKindTableError(
                    f"primitive operator {name} operands must match canonical fixed operand_arity"
                )
            for types in product(*operands):
                key = name, types
                if key in seen:
                    raise OpKindTableError(
                        f"duplicate primitive operator effect case: {key}"
                    )
                seen.add(key)
        cases.append(
            PrimitiveEffectCase(tuple(names), tuple(tuple(g) for g in operands), purity)
        )
    return tuple(cases)


def frontend_operator_map(data: dict) -> dict[str, str]:
    governed = {name for case in primitive_effect_cases(data) for name in case.opcodes}
    mapping = {
        spelling.upper(): row["mapper_opcode"]
        for row in data.get("kind", [])
        if row.get("mapper_opcode") in governed
        for spelling in (row["canonical"], *row.get("aliases", []))
    }
    for row in data.get("frontend_effect_kind", []):
        name = row.get("effects_like_opcode")
        if name is None:
            continue
        if (
            not isinstance(name, str)
            or name not in governed
            or row["effect"] != "writes_heap"
        ):
            raise OpKindTableError(
                f"frontend operator {row['kind']} has invalid effect projection"
            )
        if row["kind"] in mapping and mapping[row["kind"]] != name:
            raise OpKindTableError(
                f"frontend operator {row['kind']} overrides its mapped effect authority"
            )
        mapping[row["kind"]] = name
    return dict(sorted(mapping.items()))


def render_primitive_effects_rs(data: dict) -> str:
    cases = primitive_effect_cases(data)
    governed = sorted({name for case in cases for name in case.opcodes})
    lines = [
        "/// Exact primitive effects; annotations must not supply operand facts.\n",
        "pub fn opcode_primitive_effects_table(opcode: OpCode, operands: &[&TirType]) -> Option<OpcodeEffects> {\n",
        "    let left = operands.first().map(|ty| ty.semantic_type()).unwrap_or(&TirType::DynBox);\n",
        "    let right = operands.get(1).map(|ty| ty.semantic_type()).unwrap_or(&TirType::DynBox);\n",
        "    match (opcode, operands.len(), left, right) {\n",
    ]
    for case in cases:
        names = " | ".join(f"OpCode::{n}" for n in case.opcodes)
        groups = [" | ".join(f"TirType::{ty}" for ty in g) for g in case.operands]
        if len(groups) == 1:
            groups.append("_")
        effect = (
            "OPCODE_EFFECTS_PURE"
            if case.purity == "pure"
            else "OPCODE_EFFECTS_PURE_MAY_THROW"
        )
        lines.append(
            f"        ({names}, {len(case.operands)}, {groups[0]}, {groups[1]}) => Some({effect}),\n"
        )
    names = " | ".join(f"OpCode::{name}" for name in governed)
    lines.extend(
        [
            f"        ({names}, _, _, _) => Some(OPCODE_EFFECTS_IMPURE),\n",
            "        _ => None,\n    }\n}\n\n",
        ]
    )
    return "".join(lines)


def render_primitive_effects_py(data: dict) -> str:
    matrices: dict[str, dict[tuple[str, ...], tuple[str, bool]]] = {}
    for case in primitive_effect_cases(data):
        for name in case.opcodes:
            matrix = matrices.setdefault(name, {})
            for types in product(*case.operands):
                key = tuple(PRIMITIVE_FRONTEND_TYPES[ty] for ty in types)
                facts = ("pure", case.purity == "pure")
                if key in matrix and matrix[key] != facts:
                    raise OpKindTableError(
                        f"frontend primitive effect projection conflict: {name} {key}"
                    )
                matrix[key] = facts
    lines = [
        "# Exact primitive effects share the Rust operation matrix.\n",
        "FRONTEND_OPERATOR_OPCODE: dict[str, str] = {\n",
    ]
    for kind, name in frontend_operator_map(data).items():
        lines.append(f"    {kind!r}: {name!r},\n")
    lines.append(
        "}\n\n_FRONTEND_OPERATOR_EFFECTS: dict[str, dict[tuple[str, ...], tuple[str, bool]]] = {\n"
    )
    for name, matrix in sorted(matrices.items()):
        lines.append(f"    {name!r}: {{\n")
        for types, facts in sorted(matrix.items()):
            lines.append(f"        {types!r}: {facts!r},\n")
        lines.append("    },\n")
    lines.extend(
        [
            "}\n\n",
            "def frontend_operator_facts(kind: str, *operands: str | None) -> tuple[str, bool] | None:\n",
            "    opcode = FRONTEND_OPERATOR_OPCODE.get(kind)\n",
            "    if opcode is None:\n        return None\n",
            "    if any(ty is None for ty in operands):\n        return 'writes_heap', False\n",
            "    key = tuple(ty for ty in operands if ty is not None)\n",
            "    return _FRONTEND_OPERATOR_EFFECTS[opcode].get(key, ('writes_heap', False))\n\n",
        ]
    )
    return "".join(lines)
