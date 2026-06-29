from __future__ import annotations

from .schema import *  # noqa: F403
from .render_rust_common import (
    _opcode_const_suffix,
    _render_opcode_bool_arms,
    _rs_bool,
    _rs_string,
)

_OPERAND_INDEPENDENT_RESULT_TYPE_VARIANTS = {
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


def _render_operand_independent_result_type(opcodes: list[dict]) -> str:
    lines = [
        "/// Operand-independent result type facts for opcodes whose single result\n",
        "/// type is intrinsic to the opcode. Operand-, attr-, and builtin-dependent\n",
        "/// opcodes deliberately return None and are inferred by type_refine.rs.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum OperandIndependentResultType {\n",
        "    I64,\n",
        "    F64,\n",
        "    Bool,\n",
        "    Str,\n",
        "    None,\n",
        "    Bytes,\n",
        "    DynBox,\n",
        "    ListDynBox,\n",
        "    DictDynBoxDynBox,\n",
        "    SetDynBox,\n",
        "}\n\n",
        "impl OperandIndependentResultType {\n",
        "    #[inline]\n",
        "    pub fn to_tir_type(self) -> TirType {\n",
        "        match self {\n",
        "            OperandIndependentResultType::I64 => TirType::I64,\n",
        "            OperandIndependentResultType::F64 => TirType::F64,\n",
        "            OperandIndependentResultType::Bool => TirType::Bool,\n",
        "            OperandIndependentResultType::Str => TirType::Str,\n",
        "            OperandIndependentResultType::None => TirType::None,\n",
        "            OperandIndependentResultType::Bytes => TirType::Bytes,\n",
        "            OperandIndependentResultType::DynBox => TirType::DynBox,\n",
        "            OperandIndependentResultType::ListDynBox => {\n",
        "                TirType::List(Box::new(TirType::DynBox))\n",
        "            }\n",
        "            OperandIndependentResultType::DictDynBoxDynBox => {\n",
        "                TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox))\n",
        "            }\n",
        "            OperandIndependentResultType::SetDynBox => {\n",
        "                TirType::Set(Box::new(TirType::DynBox))\n",
        "            }\n",
        "        }\n",
        "    }\n",
        "}\n\n",
        "/// Operand-independent result type by opcode. EXHAUSTIVE over OpCode so new\n",
        "/// opcodes cannot silently inherit or miss an intrinsic result-type fact.\n",
        "#[inline]\n",
        "pub fn opcode_operand_independent_result_type_table(\n",
        "    opcode: OpCode,\n",
        ") -> Option<OperandIndependentResultType> {\n",
        "    match opcode {\n",
    ]
    for row in opcodes:
        name = row["name"]
        type_name = row.get("operand_independent_result_type")
        variant = (
            f"Some({_OPERAND_INDEPENDENT_RESULT_TYPE_VARIANTS[type_name]})"
            if type_name is not None
            else "None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.extend(
        [
            "    }\n",
            "}\n\n",
            "#[inline]\n",
            "pub fn opcode_operand_independent_result_tir_type(\n",
            "    opcode: OpCode,\n",
            ") -> Option<TirType> {\n",
            "    opcode_operand_independent_result_type_table(opcode)\n",
            "        .map(OperandIndependentResultType::to_tir_type)\n",
            "}\n",
        ]
    )
    return "".join(lines)


def _render_type_refine_attr_result_type_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("type_refine_attr_result_type_rules", [])
    }
    lines = [
        "/// Type-refine attr-derived result-type rule by opcode. This table owns\n",
        "/// the opcode membership for result types determined by op attrs rather\n",
        "/// than operands; type_refine.rs owns the semantics of each rule.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum TypeRefineAttrResultTypeRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_TYPE_REFINE_ATTR_RESULT_TYPE_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Attr-result-type rule by opcode. EXHAUSTIVE over OpCode so new\n",
            "/// attr-keyed producers cannot silently inherit a pass-local default.\n",
            "#[inline]\n",
            "pub fn opcode_type_refine_attr_result_type_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> TypeRefineAttrResultTypeRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"TypeRefineAttrResultTypeRule::{_TYPE_REFINE_ATTR_RESULT_TYPE_RULES[rule]}"
            if rule is not None
            else "TypeRefineAttrResultTypeRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_type_refine_operand_type_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("type_refine_operand_type_rules", [])
    }
    lines = [
        "/// Type-refine operand-dependent result-type rule by opcode. This table\n",
        "/// owns opcode membership for inference that depends on operand types;\n",
        "/// type_refine.rs owns the semantics of each rule.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum TypeRefineOperandTypeRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_TYPE_REFINE_OPERAND_TYPE_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Operand-dependent result-type rule by opcode. EXHAUSTIVE over OpCode so\n",
            "/// new opcodes cannot silently bypass type-refine's generated rule lattice.\n",
            "#[inline]\n",
            "pub fn opcode_type_refine_operand_type_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> TypeRefineOperandTypeRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"TypeRefineOperandTypeRule::{_TYPE_REFINE_OPERAND_TYPE_RULES[rule]}"
            if rule is not None
            else "TypeRefineOperandTypeRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_sccp_constant_seed_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"] for row in data.get("sccp_constant_seed_rules", [])
    }
    lines = [
        "/// SCCP constant-seed rule by opcode. This table owns opcode\n",
        "/// membership for constants SCCP can put directly into its lattice;\n",
        "/// sccp.rs owns the attr parsing for each rule.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum SccpConstantSeedRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_SCCP_CONSTANT_SEED_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Constant-seed rule by opcode. EXHAUSTIVE over OpCode so new\n",
            "/// constant constructors cannot silently inherit a pass-local default.\n",
            "#[inline]\n",
            "pub fn opcode_sccp_constant_seed_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> SccpConstantSeedRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"SccpConstantSeedRule::{_SCCP_CONSTANT_SEED_RULES[rule]}"
            if rule is not None
            else "SccpConstantSeedRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_sccp_constant_eval_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"] for row in data.get("sccp_constant_eval_rules", [])
    }
    lines = [
        "/// SCCP constant-evaluation rule by opcode. This table owns opcode\n",
        "/// membership for foldable op families; sccp.rs owns each rule's\n",
        "/// CPython-compatible constant evaluation semantics.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum SccpConstantEvalRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_SCCP_CONSTANT_EVAL_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Constant-evaluation rule by opcode. EXHAUSTIVE over OpCode so new\n",
            "/// foldable ops cannot hide behind SCCP's non-foldable default.\n",
            "#[inline]\n",
            "pub fn opcode_sccp_constant_eval_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> SccpConstantEvalRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"SccpConstantEvalRule::{_SCCP_CONSTANT_EVAL_RULES[rule]}"
            if rule is not None
            else "SccpConstantEvalRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_value_range_transfer_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"] for row in data.get("value_range_transfer_rules", [])
    }
    lines = [
        "/// Value-range transfer rule by opcode. This table owns opcode\n",
        "/// membership for modeled integer transfer functions; value_range.rs\n",
        "/// owns each rule's interval arithmetic semantics.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum ValueRangeTransferRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_VALUE_RANGE_TRANSFER_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Integer range transfer rule by opcode. EXHAUSTIVE over OpCode so new\n",
            "/// modeled arithmetic ops cannot hide behind value-range's default.\n",
            "#[inline]\n",
            "pub fn opcode_value_range_transfer_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> ValueRangeTransferRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"ValueRangeTransferRule::{_VALUE_RANGE_TRANSFER_RULES[rule]}"
            if rule is not None
            else "ValueRangeTransferRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_value_range_const_fold_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("value_range_const_fold_rules", [])
    }
    lines = [
        "/// Value-range integer constant-fold rule by opcode. This table owns\n",
        "/// membership for foldable integer expressions used by value_range.rs's\n",
        "/// const/length collection; the pass owns checked arithmetic semantics.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum ValueRangeConstFoldRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_VALUE_RANGE_CONST_FOLD_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Integer constant-fold rule by opcode. EXHAUSTIVE over OpCode so\n",
            "/// constant-mask/range derivation cannot drift from a private opcode list.\n",
            "#[inline]\n",
            "pub fn opcode_value_range_const_fold_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> ValueRangeConstFoldRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"ValueRangeConstFoldRule::{_VALUE_RANGE_CONST_FOLD_RULES[rule]}"
            if rule is not None
            else "ValueRangeConstFoldRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_value_range_cond_narrow_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("value_range_cond_narrow_rules", [])
    }
    lines = [
        "/// Value-range conditional guard-narrowing rule by opcode. This table\n",
        "/// owns opcode membership for guard-true upper-bound facts;\n",
        "/// value_range.rs owns CFG polarity, operand resolution, and narrowing math.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum ValueRangeCondNarrowRule {\n",
        "    None,\n",
    ]
    for variant in _VALUE_RANGE_COND_NARROW_RULES.values():
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Conditional guard-narrowing rule by opcode. EXHAUSTIVE over OpCode so\n",
            "/// edge-sensitive value-range facts cannot drift behind a private Lt/Le list.\n",
            "#[inline]\n",
            "pub fn opcode_value_range_cond_narrow_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> ValueRangeCondNarrowRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"ValueRangeCondNarrowRule::{_VALUE_RANGE_COND_NARROW_RULES[rule]}"
            if rule is not None
            else "ValueRangeCondNarrowRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_value_range_container_length_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("value_range_container_length_rules", [])
    }
    lines = [
        "/// Value-range container-length rule by opcode. This table owns opcode\n",
        "/// membership for length fact producers; value_range.rs owns operand counts,\n",
        "/// builtin-name validation, copy resolution, and length formulas.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum ValueRangeContainerLengthRule {\n",
        "    None,\n",
    ]
    for variant in _VALUE_RANGE_CONTAINER_LENGTH_RULES.values():
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Container-length rule by opcode. EXHAUSTIVE over OpCode so list/tuple\n",
            "/// builders, repeat candidates, and len calls cannot drift behind a private\n",
            "/// value_range.rs opcode list.\n",
            "#[inline]\n",
            "pub fn opcode_value_range_container_length_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> ValueRangeContainerLengthRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"ValueRangeContainerLengthRule::{_VALUE_RANGE_CONTAINER_LENGTH_RULES[rule]}"
            if rule is not None
            else "ValueRangeContainerLengthRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_range_devirt_role(opcodes: list[dict], data: dict) -> str:
    role_by_opcode = {
        row["opcode"]: row["role"] for row in data.get("range_devirt_roles", [])
    }
    lines = [
        "/// Range-loop devirtualization scanner role by opcode. This table owns\n",
        "/// only opcode membership for the CallBuiltin/GetIter/IterNextUnboxed\n",
        "/// pattern roles; range_devirt.rs owns name, shape, loop, and CFG checks.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum RangeDevirtRole {\n",
    ]
    for variant in _RANGE_DEVIRT_ROLES.values():
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Opcode role used by range_devirt.rs. EXHAUSTIVE over OpCode so new\n",
            "/// iterator/range opcodes cannot inherit a pass-local silent default.\n",
            "#[inline]\n",
            "pub fn opcode_range_devirt_role_table(opcode: OpCode) -> RangeDevirtRole {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        role = role_by_opcode.get(name, "none")
        variant = _RANGE_DEVIRT_ROLES[role]
        lines.append(f"        OpCode::{name} => RangeDevirtRole::{variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_vectorize_opcode_facts(opcodes: list[dict], data: dict) -> str:
    fact_by_opcode = {
        row["opcode"]: row for row in data.get("vectorize_opcode_facts", [])
    }
    lines = [
        "/// Vectorization body decision by opcode. This table owns opcode-level\n",
        "/// eligibility facts; vectorize.rs owns CFG, attrs, lane typing, and\n",
        "/// accumulator proof semantics.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum VectorizeBodyAction {\n",
    ]
    for variant in _VECTORIZE_BODY_ACTIONS.values():
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Vectorization reduction rule by opcode, folded into VectorizeOpcodeFacts.\n",
            "/// The table owns opcode-to-family membership; vectorize.rs owns proof that\n",
            "/// a matching op is actually using the loop accumulator.\n",
            "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            "pub enum VectorReductionRule {\n",
            "    None,\n",
        ]
    )
    for variant in _VECTOR_REDUCTION_RULES.values():
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// All opcode-level facts consumed by vectorize.rs.\n",
            "///\n",
            "/// This match is EXHAUSTIVE over OpCode so vectorization cannot grow\n",
            "/// pass-local opcode taxonomies or silent defaults when new opcodes land.\n",
            "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            "pub struct VectorizeOpcodeFacts {\n",
            "    pub body_action: VectorizeBodyAction,\n",
            "    pub reduction_rule: VectorReductionRule,\n",
            "    pub annotation_target: bool,\n",
            "}\n\n",
            "#[inline]\n",
            "pub fn opcode_vectorize_facts_table(opcode: OpCode) -> VectorizeOpcodeFacts {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        fact = fact_by_opcode.get(name, {})
        body = _VECTORIZE_BODY_ACTIONS[fact.get("body", "reject")]
        reduction = fact.get("reduction")
        reduction_variant = (
            f"VectorReductionRule::{_VECTOR_REDUCTION_RULES[reduction]}"
            if reduction is not None
            else "VectorReductionRule::None"
        )
        annotation = "true" if fact.get("annotation_target", False) else "false"
        lines.append(
            f"        OpCode::{name} => VectorizeOpcodeFacts {{ "
            f"body_action: VectorizeBodyAction::{body}, "
            f"reduction_rule: {reduction_variant}, "
            f"annotation_target: {annotation} }},\n"
        )
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_lir_verify_rule(opcodes: list[dict], data: dict) -> str:
    rule_by_opcode = {
        row["opcode"]: row["rule"] for row in data.get("lir_verify_rules", [])
    }
    lines = [
        "/// Representation-aware LIR verifier hook by opcode. This table owns\n",
        "/// verifier dispatch membership; verify_lir.rs owns each hook's invariant\n",
        "/// implementation and diagnostics.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum LirVerifyRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_LIR_VERIFY_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// LIR verifier hook by opcode. EXHAUSTIVE over OpCode so new opcodes\n",
            "/// cannot silently miss representation-aware verification dispatch.\n",
            "#[inline]\n",
            "pub fn opcode_lir_verify_rule_table(opcode: OpCode) -> LirVerifyRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"LirVerifyRule::{_LIR_VERIFY_RULES[rule]}"
            if rule is not None
            else "LirVerifyRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_repr_projectable_result_rules(opcodes: list[dict], data: dict) -> str:
    raw_rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("repr_raw_i64_full_deopt_seed_rules", [])
    }
    bool_rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("repr_projectable_bool_result_rules", [])
    }
    float_rule_by_opcode = {
        row["opcode"]: row["rule"]
        for row in data.get("repr_projectable_float_result_rules", [])
    }
    lines = [
        "/// Representation-plan raw-i64 full-deopt seed role by opcode. The table\n",
        "/// owns opcode membership; value_repr.rs owns result-index, type, and\n",
        "/// inline-safe exclusion checks for each role.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum ReprRawI64FullDeoptSeedRule {\n",
        "    None,\n",
    ]
    for variant in sorted(_REPR_RAW_I64_FULL_DEOPT_SEED_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Raw-i64 full-deopt seed role by opcode. EXHAUSTIVE over OpCode so new\n",
            "/// checked/full-range producers cannot silently miss carrier projection or\n",
            "/// inherit it from a pass-local wildcard.\n",
            "#[inline]\n",
            "pub fn opcode_repr_raw_i64_full_deopt_seed_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> ReprRawI64FullDeoptSeedRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = raw_rule_by_opcode.get(name)
        variant = (
            f"ReprRawI64FullDeoptSeedRule::{_REPR_RAW_I64_FULL_DEOPT_SEED_RULES[rule]}"
            if rule is not None
            else "ReprRawI64FullDeoptSeedRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.extend(
        [
            "    }\n",
            "}\n\n",
        ]
    )
    lines.extend(
        [
            "/// Representation-plan bool projection role by opcode. The table owns\n",
            "/// opcode membership; value_repr.rs owns live result-index, operand-carrier,\n",
            "/// and Copy-source checks for each role.\n",
            "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            "pub enum ReprProjectableBoolResultRule {\n",
            "    None,\n",
        ]
    )
    for variant in sorted(_REPR_PROJECTABLE_BOOL_RESULT_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Bool projection role by opcode. EXHAUSTIVE over OpCode so new bool\n",
            "/// producers cannot silently miss scalar-carrier projection or inherit it\n",
            "/// from a pass-local wildcard.\n",
            "#[inline]\n",
            "pub fn opcode_repr_projectable_bool_result_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> ReprProjectableBoolResultRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = bool_rule_by_opcode.get(name)
        variant = (
            "ReprProjectableBoolResultRule::"
            f"{_REPR_PROJECTABLE_BOOL_RESULT_RULES[rule]}"
            if rule is not None
            else "ReprProjectableBoolResultRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.extend(
        [
            "    }\n",
            "}\n\n",
            "/// Representation-plan float projection role by opcode. The table owns\n",
            "/// opcode membership; value_repr.rs owns live operand-carrier and Copy-source\n",
            "/// checks for each role.\n",
            "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            "pub enum ReprProjectableFloatResultRule {\n",
            "    None,\n",
        ]
    )
    for variant in sorted(_REPR_PROJECTABLE_FLOAT_RESULT_RULES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Float projection role by opcode. EXHAUSTIVE over OpCode so new float\n",
            "/// producers cannot silently miss scalar-carrier projection or inherit it\n",
            "/// from a pass-local wildcard.\n",
            "#[inline]\n",
            "pub fn opcode_repr_projectable_float_result_rule_table(\n",
            "    opcode: OpCode,\n",
            ") -> ReprProjectableFloatResultRule {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = float_rule_by_opcode.get(name)
        variant = (
            "ReprProjectableFloatResultRule::"
            f"{_REPR_PROJECTABLE_FLOAT_RESULT_RULES[rule]}"
            if rule is not None
            else "ReprProjectableFloatResultRule::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_counted_loop_comparison_roles(opcodes: list[dict], data: dict) -> str:
    role_by_opcode = {
        row["opcode"]: row["role"]
        for row in data.get("counted_loop_comparison_roles", [])
    }
    inverse_by_opcode = {
        row["opcode"]: row["inverse"]
        for row in data.get("counted_loop_comparison_roles", [])
    }
    lines = [
        "/// Counted-loop ordered-comparison role by opcode. The registry owns\n",
        "/// Lt/Le/Gt/Ge membership, loop-direction polarity, and logical inverse;\n",
        "/// counted_loop.rs owns SSA shape, constants, and trip-count arithmetic.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum CountedLoopComparisonRole {\n",
        "    None,\n",
    ]
    for variant in sorted(_COUNTED_LOOP_COMPARISON_ROLES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "impl CountedLoopComparisonRole {\n",
            "    #[inline]\n",
            "    pub fn is_ordered(self) -> bool {\n",
            "        !matches!(self, CountedLoopComparisonRole::None)\n",
            "    }\n\n",
            "    #[inline]\n",
            "    pub fn requires_positive_step(self) -> bool {\n",
            "        matches!(\n",
            "            self,\n",
            "            CountedLoopComparisonRole::IncreasingExclusive\n",
            "                | CountedLoopComparisonRole::IncreasingInclusive\n",
            "        )\n",
            "    }\n\n",
            "    #[inline]\n",
            "    pub fn is_inclusive(self) -> bool {\n",
            "        matches!(\n",
            "            self,\n",
            "            CountedLoopComparisonRole::IncreasingInclusive\n",
            "                | CountedLoopComparisonRole::DecreasingInclusive\n",
            "        )\n",
            "    }\n",
            "}\n\n",
            "/// Counted-loop comparison role by opcode. EXHAUSTIVE over OpCode so new\n",
            "/// comparisons cannot silently enter or miss counted-loop recognition through\n",
            "/// pass-local wildcard/default logic.\n",
            "#[inline]\n",
            "pub fn opcode_counted_loop_comparison_role_table(\n",
            "    opcode: OpCode,\n",
            ") -> CountedLoopComparisonRole {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        role = role_by_opcode.get(name)
        variant = (
            f"CountedLoopComparisonRole::{_COUNTED_LOOP_COMPARISON_ROLES[role]}"
            if role is not None
            else "CountedLoopComparisonRole::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.extend(
        [
            "    }\n",
            "}\n\n",
            "/// Logical inverse for counted-loop ordered comparisons under branch-polarity\n",
            "/// inversion. EXHAUSTIVE over OpCode; non-counted-loop comparisons map None.\n",
            "#[inline]\n",
            "pub fn opcode_counted_loop_inverted_comparison_table(\n",
            "    opcode: OpCode,\n",
            ") -> Option<OpCode> {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        inverse = inverse_by_opcode.get(name)
        variant = f"Some(OpCode::{inverse})" if inverse is not None else "None"
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _module_concurrency_marker_attrs_const(opcode: str) -> str:
    return f"MODULE_CONCURRENCY_MARKER_ATTRS_{_opcode_const_suffix(opcode)}"


def _render_module_slot_promotion_roles(opcodes: list[dict], data: dict) -> str:
    concurrency_rows = list(data.get("module_concurrency_marker_source_roles", []))
    concurrency_by_opcode = {row["opcode"]: row for row in concurrency_rows}
    access_by_opcode = {
        row["opcode"]: row["role"] for row in data.get("module_slot_access_roles", [])
    }
    lines = [
        "/// Module-slot promotion concurrency evidence role. The registry owns\n",
        "/// opcode membership and attribute payload names; module_slot_promotion.rs\n",
        "/// owns the string policy for threading modules and direct thread intrinsics.\n",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
        "pub enum ModuleConcurrencyMarkerSourceRole {\n",
    ]
    for variant in sorted(_MODULE_CONCURRENCY_MARKER_SOURCE_ROLES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            "pub struct ModuleConcurrencyMarkerSourceFacts {\n",
            "    pub role: ModuleConcurrencyMarkerSourceRole,\n",
            "    pub attrs: &'static [&'static str],\n",
            "}\n\n",
            "const MODULE_CONCURRENCY_MARKER_SOURCE_NONE: ModuleConcurrencyMarkerSourceFacts =\n",
            "    ModuleConcurrencyMarkerSourceFacts {\n",
            "        role: ModuleConcurrencyMarkerSourceRole::None,\n",
            "        attrs: &[],\n",
            "    };\n",
        ]
    )
    for row in concurrency_rows:
        const_name = _module_concurrency_marker_attrs_const(row["opcode"])
        attrs = ", ".join(_rs_string(attr) for attr in row.get("attrs", []))
        lines.append(f"const {const_name}: &[&str] = &[{attrs}];\n")
    lines.extend(
        [
            "\n",
            "/// Module-slot promotion concurrency marker facts by opcode. EXHAUSTIVE over\n",
            "/// OpCode so new import/call carriers cannot silently bypass the module-wide\n",
            "/// concurrency refusal scan through pass-local wildcard/default logic.\n",
            "#[inline]\n",
            "pub fn opcode_module_concurrency_marker_source_facts_table(\n",
            "    opcode: OpCode,\n",
            ") -> ModuleConcurrencyMarkerSourceFacts {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        facts = concurrency_by_opcode.get(name)
        if facts is None:
            lines.append(
                f"        OpCode::{name} => MODULE_CONCURRENCY_MARKER_SOURCE_NONE,\n"
            )
            continue
        variant = _MODULE_CONCURRENCY_MARKER_SOURCE_ROLES[facts["role"]]
        const_name = _module_concurrency_marker_attrs_const(name)
        lines.append(
            "        "
            f"OpCode::{name} => ModuleConcurrencyMarkerSourceFacts {{ "
            f"role: ModuleConcurrencyMarkerSourceRole::{variant}, "
            f"attrs: {const_name} "
            "},\n"
        )
    lines.extend(
        [
            "    }\n",
            "}\n\n",
            "/// Module-slot promotion module-dict access role. `KeyedAttr` ops are safe\n",
            "/// only after module-root and const-name proof in the pass; wildcard ops\n",
            "/// disqualify promotion for the whole function.\n",
            "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            "pub enum ModuleSlotAccessRole {\n",
        ]
    )
    for variant in sorted(_MODULE_SLOT_ACCESS_ROLES.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "/// Module-slot promotion access role by opcode. EXHAUSTIVE over OpCode so\n",
            "/// module ATTR/dict membership cannot drift across prefilters, preheader\n",
            "/// scans, and in-loop legality scans.\n",
            "#[inline]\n",
            "pub fn opcode_module_slot_access_role_table(opcode: OpCode) -> ModuleSlotAccessRole {\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        role = access_by_opcode.get(name)
        variant = (
            f"ModuleSlotAccessRole::{_MODULE_SLOT_ACCESS_ROLES[role]}"
            if role is not None
            else "ModuleSlotAccessRole::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_simple_opcode_rule_table(
    opcodes: list[dict],
    data: dict,
    *,
    table_key: str,
    enum_name: str,
    fn_name: str,
    variants: dict[str, str],
    docs: list[str],
) -> str:
    rule_by_opcode = {row["opcode"]: row["rule"] for row in data.get(table_key, [])}
    lines: list[str] = []
    for doc in docs:
        lines.append(f"/// {doc}\n")
    lines.extend(
        [
            "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n",
            f"pub enum {enum_name} {{\n",
            "    None,\n",
        ]
    )
    for variant in sorted(variants.values()):
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            f"/// {enum_name} by opcode. EXHAUSTIVE over OpCode so a new opcode\n",
            "/// cannot silently enter or miss this consumer through pass-local\n",
            "/// wildcard/default logic.\n",
            "#[inline]\n",
            f"pub fn {fn_name}(opcode: OpCode) -> {enum_name} {{\n",
            "    match opcode {\n",
        ]
    )
    for row in opcodes:
        name = row["name"]
        rule = rule_by_opcode.get(name)
        variant = (
            f"{enum_name}::{variants[rule]}"
            if rule is not None
            else f"{enum_name}::None"
        )
        lines.append(f"        OpCode::{name} => {variant},\n")
    lines.append("    }\n}\n")
    return "".join(lines)


def _render_residual_tir_semantic_roles(opcodes: list[dict], data: dict) -> str:
    parts = [
        _render_simple_opcode_rule_table(
            opcodes,
            data,
            table_key="tir_verify_attr_rules",
            enum_name="TirVerifyAttrRule",
            fn_name="opcode_tir_verify_attr_rule_table",
            variants=_TIR_VERIFY_ATTR_RULES,
            docs=[
                "Required op-level attr/operand validation role for verify.rs.",
                "Opcode membership lives in op_kinds.toml; verify.rs owns",
                "diagnostic text and payload value checks.",
            ],
        ),
        "\n",
        _render_simple_opcode_rule_table(
            opcodes,
            data,
            table_key="sroa_const_immediate_rules",
            enum_name="SroaConstImmediateRule",
            fn_name="opcode_sroa_const_immediate_rule_table",
            variants=_SROA_CONST_IMMEDIATE_RULES,
            docs=[
                "SROA constant-immediate recognition role. Opcode membership",
                "lives in op_kinds.toml; sroa.rs owns range proof for",
                "inline integer immediates.",
            ],
        ),
        "\n",
        _render_simple_opcode_rule_table(
            opcodes,
            data,
            table_key="strength_reduction_rules",
            enum_name="StrengthReductionRule",
            fn_name="opcode_strength_reduction_rule_table",
            variants=_STRENGTH_REDUCTION_RULES,
            docs=[
                "Strength-reduction rewrite role. Opcode membership lives in",
                "op_kinds.toml; strength_reduction.rs owns constant/type proof",
                "and replacement construction.",
            ],
        ),
        "\n",
        _render_simple_opcode_rule_table(
            opcodes,
            data,
            table_key="scev_expr_rules",
            enum_name="ScevExprRule",
            fn_name="opcode_scev_expr_rule_table",
            variants=_SCEV_EXPR_RULES,
            docs=[
                "SCEV affine-expression construction role. Opcode membership",
                "lives in op_kinds.toml; scev.rs owns arity checks and lattice",
                "folding.",
            ],
        ),
    ]
    return "".join(parts)
