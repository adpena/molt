//! TIR invariant checker.
//!
//! Verifies that a [`TirFunction`] is well-formed SSA. Call
//! [`verify_function`] to get a list of [`VerifyError`]s; an empty list
//! means the function is valid.

use std::collections::{HashMap, HashSet};

use crate::native_callable_abi::{NATIVE_CALLABLE_ABI_CHOICES, parse_native_callable_abi};

use super::blocks::{BlockId, Terminator};
use super::dominators::{self, ProgramPointDominance};
use super::function::TirFunction;
use super::op_kinds_generated::{
    TirVerifyAttrRule, opcode_accepts_operand_count, opcode_accepts_result_count,
    opcode_canonical_kind_table, opcode_fixed_result_count_table,
    opcode_tir_verify_attr_rule_table,
};
use super::ops::AttrValue;
use super::values::ValueId;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single verification error with location context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// Block where the error was detected (if applicable).
    pub block: Option<BlockId>,
    /// Op index within the block (if applicable).
    pub op_index: Option<usize>,
    /// Human-readable description.
    pub message: String,
}

impl VerifyError {
    fn func(msg: impl Into<String>) -> Self {
        Self {
            block: None,
            op_index: None,
            message: msg.into(),
        }
    }

    fn block(bid: BlockId, msg: impl Into<String>) -> Self {
        Self {
            block: Some(bid),
            op_index: None,
            message: msg.into(),
        }
    }

    fn op(bid: BlockId, op_idx: usize, msg: impl Into<String>) -> Self {
        Self {
            block: Some(bid),
            op_index: Some(op_idx),
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.block, self.op_index) {
            (None, _) => write!(f, "[func] {}", self.message),
            (Some(bid), None) => write!(f, "[^{}] {}", bid, self.message),
            (Some(bid), Some(idx)) => write!(f, "[^{} op#{}] {}", bid, idx, self.message),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Verify that `func` satisfies all TIR well-formedness invariants.
///
/// Returns `Ok(())` if the function is valid, or `Err(errors)` with a
/// non-empty list of all violations found.
pub fn verify_function(func: &TirFunction) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();
    if let Err(shape_errors) = verify_operation_shapes(func) {
        errors.extend(shape_errors);
    }
    verify_entry_block(func, &mut errors);
    verify_no_duplicate_values(func, &mut errors);
    verify_op_attributes(func, &mut errors);
    verify_terminators(func, &mut errors);
    verify_block_args(func, &mut errors);
    verify_ssa(func, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Check 1: entry block exists
// ---------------------------------------------------------------------------

fn verify_entry_block(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    if !func.blocks.contains_key(&func.entry_block) {
        errors.push(VerifyError::func(format!(
            "entry block ^{} does not exist in blocks map",
            func.entry_block
        )));
    }
}

// ---------------------------------------------------------------------------
// Check 2: no duplicate ValueIds
// ---------------------------------------------------------------------------

fn verify_no_duplicate_values(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    let mut defined: HashSet<ValueId> = HashSet::new();

    for (bid, block) in &func.blocks {
        // Block arguments count as definitions.
        for arg in &block.args {
            if !defined.insert(arg.id) {
                errors.push(VerifyError::block(
                    *bid,
                    format!("duplicate definition of {}", arg.id),
                ));
            }
        }
        // Op results count as definitions.
        for (op_idx, op) in block.ops.iter().enumerate() {
            for result in &op.results {
                if !defined.insert(*result) {
                    errors.push(VerifyError::op(
                        *bid,
                        op_idx,
                        format!("duplicate definition of {}", result),
                    ));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Check 3: op-level attribute and operand validation
// ---------------------------------------------------------------------------

/// Shared generated shape admission, also usable before a target mutates its
/// output module. No whole-function SSA or program-closure assumptions apply.
pub fn verify_operation_shapes(func: &TirFunction) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();
    for (bid, block) in &func.blocks {
        for (op_index, op) in block.ops.iter().enumerate() {
            let original_kind = match op.attrs.get("_original_kind") {
                Some(AttrValue::Str(kind)) => Some(kind.as_str()),
                _ => None,
            };
            let kind = if op.opcode == super::ops::OpCode::Copy {
                original_kind.unwrap_or_else(|| opcode_canonical_kind_table(op.opcode))
            } else {
                opcode_canonical_kind_table(op.opcode)
            };
            // A typed carrier does not erase a retired origin. Live aliases
            // still use the carrier's own shape rather than being reinterpreted.
            if let Some(original_kind) = original_kind.filter(|original| *original != kind)
                && let Err(error) = crate::ir_schema::validate_op_not_retired(original_kind)
            {
                errors.push(VerifyError::op(*bid, op_index, error.to_string()));
                continue;
            }
            let value = match op.attrs.get("value") {
                Some(AttrValue::Int(value)) => Some(*value),
                _ => None,
            };
            if let Err(error) =
                crate::ir_schema::validate_op_shape(kind, Some(op.operands.len()), value)
            {
                errors.push(VerifyError::op(*bid, op_index, error.to_string()));
                continue;
            }
            // Retired origins and malformed shapes own diagnostic precedence;
            // do not diagnose the payload of a carrier already rejected above.
            if let Err(error) = crate::literal_payload::validate_tir_literal(op) {
                errors.push(VerifyError::op(*bid, op_index, error));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn verify_op_attributes(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    for (bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            if !opcode_accepts_operand_count(op.opcode, op.operands.len()) {
                errors.push(VerifyError::op(
                    *bid,
                    op_idx,
                    format!(
                        "{:?} has invalid operand count {}",
                        op.opcode,
                        op.operands.len()
                    ),
                ));
            }
            // Check required attributes per opcode.
            // String, bytes and bigint payloads are admitted by the shared
            // literal authority in verify_operation_shapes. Numeric constant
            // attribute handling remains separate from this generated table.
            match opcode_tir_verify_attr_rule_table(op.opcode) {
                TirVerifyAttrRule::CallCallee if !op_has_call_callee(op) => {
                    // Direct calls carry a symbol; opaque calls carry a
                    // callable operand. Native direct symbols may have no
                    // operands, and builtins retain their own dispatch form.
                    errors.push(VerifyError::op(
                        *bid,
                        op_idx,
                        format!("{:?} op has no callee for its call target role", op.opcode),
                    ));
                }
                TirVerifyAttrRule::CallMethod
                    if !op.attrs.contains_key("method")
                        && !op.attrs.contains_key("callee")
                        && !op.attrs.contains_key("s_value")
                        && op.operands.is_empty() =>
                {
                    errors.push(VerifyError::op(
                        *bid,
                        op_idx,
                        "CallMethod op has no method (attr or operand)",
                    ));
                }
                TirVerifyAttrRule::UnpackSequenceShape => {
                    let expected = match op.attrs.get("value") {
                        Some(AttrValue::Int(value)) => usize::try_from(*value).ok(),
                        _ => None,
                    };
                    if op.operands.len() != 1 || expected != Some(op.results.len()) {
                        errors.push(VerifyError::op(
                            *bid,
                            op_idx,
                            format!(
                                "UnpackSequence requires one operand and value == result count (operands={}, results={}, value={expected:?})",
                                op.operands.len(),
                                op.results.len(),
                            ),
                        ));
                    }
                }
                _ => {}
            }
            if op.is_async_work_poll() && !op.can_carry_async_work_poll() {
                errors.push(VerifyError::op(
                    *bid,
                    op_idx,
                    format!("{:?} op cannot carry the async_work_poll marker", op.opcode),
                ));
            }
            verify_native_callable_attrs(*bid, op_idx, op, errors);

            // Semantic arity remains fixed for inference; binding admission also
            // accounts for the registry's explicitly discardable result family.
            let expected_results = opcode_fixed_result_count_table(op.opcode);

            if let Some(expected) = expected_results
                && !opcode_accepts_result_count(op.opcode, op.results.len())
            {
                errors.push(VerifyError::op(
                    *bid,
                    op_idx,
                    format!(
                        "{:?} op has {} results but expected {}",
                        op.opcode,
                        op.results.len(),
                        expected
                    ),
                ));
            }
        }
    }
}

fn op_has_call_callee(op: &super::ops::TirOp) -> bool {
    use super::op_kinds_generated::{SimpleIrCallTargetRole, simpleir_call_target_role};

    // Admit exactly the builtin identity and argument convention consumed by
    // lowering and analysis; attribute or operand presence is not authority.
    if op.opcode == super::ops::OpCode::CallBuiltin {
        return op.builtin_call().is_some();
    }
    if super::call_targets::direct_call_symbol_for_op(op).is_some() {
        return true;
    }
    let kind = match op.attrs.get("_original_kind") {
        None => "call",
        Some(AttrValue::Str(kind)) => kind.as_str(),
        Some(_) => return false,
    };
    if simpleir_call_target_role(kind) != Some(SimpleIrCallTargetRole::Opaque) {
        return false;
    }
    // The native ABI validator below checks the complete metadata and arity.
    // Only invoke_ffi can replace its callable operand with a native symbol.
    !op.operands.is_empty()
        || (kind == "invoke_ffi"
            && attr_str(op, "native_callable_binding") == Some("direct_symbol")
            && attr_str(op, "native_callable_symbol").is_some_and(|symbol| !symbol.is_empty()))
}

fn attr_str<'a>(op: &'a super::ops::TirOp, key: &str) -> Option<&'a str> {
    match op.attrs.get(key) {
        Some(AttrValue::Str(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn has_native_callable_attr(op: &super::ops::TirOp) -> bool {
    op.attrs.contains_key("native_callable_export")
        || op.attrs.contains_key("native_callable_binding")
        || op.attrs.contains_key("native_callable_symbol")
        || op.attrs.contains_key("native_callable_abi")
}

fn push_native_callable_error(
    errors: &mut Vec<VerifyError>,
    bid: BlockId,
    op_idx: usize,
    message: impl Into<String>,
) {
    errors.push(VerifyError::op(bid, op_idx, message));
}

fn verify_native_callable_attrs(
    bid: BlockId,
    op_idx: usize,
    op: &super::ops::TirOp,
    errors: &mut Vec<VerifyError>,
) {
    if !has_native_callable_attr(op) {
        return;
    }

    if op.opcode != super::ops::OpCode::Call || attr_str(op, "_original_kind") != Some("invoke_ffi")
    {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            "native callable metadata belongs only on invoke_ffi call ops",
        );
    }

    let Some(export_name) = attr_str(op, "native_callable_export") else {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            "invoke_ffi native callable export requires native_callable_export",
        );
        return;
    };
    if export_name.trim().is_empty() || export_name.chars().any(char::is_control) {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            "invoke_ffi native_callable_export must be nonempty and printable",
        );
    }

    let Some(binding) = attr_str(op, "native_callable_binding") else {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            format!(
                "invoke_ffi native callable export `{export_name}` requires native_callable_binding"
            ),
        );
        return;
    };
    if !matches!(binding, "module_attr" | "direct_symbol") {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            format!(
                "invoke_ffi native callable export `{export_name}` has unsupported binding `{binding}`"
            ),
        );
        return;
    }

    let Some(abi) = attr_str(op, "native_callable_abi") else {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            format!(
                "invoke_ffi native callable export `{export_name}` requires native_callable_abi"
            ),
        );
        return;
    };
    let Some(parsed_abi) = parse_native_callable_abi(abi) else {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            format!(
                "invoke_ffi native callable export `{export_name}` has unknown native_callable_abi `{abi}`; expected one of: {NATIVE_CALLABLE_ABI_CHOICES}"
            ),
        );
        return;
    };

    if binding == "module_attr" && parsed_abi.requires_direct_symbol_binding() {
        push_native_callable_error(
            errors,
            bid,
            op_idx,
            format!(
                "invoke_ffi native callable export `{export_name}` uses module_attr direct-symbol ABI `{abi}`"
            ),
        );
    }

    if binding == "direct_symbol" {
        let Some(symbol) = attr_str(op, "native_callable_symbol") else {
            push_native_callable_error(
                errors,
                bid,
                op_idx,
                format!(
                    "invoke_ffi native callable export `{export_name}` direct_symbol requires native_callable_symbol"
                ),
            );
            return;
        };
        if symbol.trim().is_empty() || symbol.chars().any(char::is_control) {
            push_native_callable_error(
                errors,
                bid,
                op_idx,
                "invoke_ffi native_callable_symbol must be nonempty and printable",
            );
        }
    }

    if let Some(fixed_payload_arity) = parsed_abi.fixed_arity() {
        let expected = fixed_payload_arity + usize::from(binding == "module_attr");
        if op.operands.len() != expected {
            push_native_callable_error(
                errors,
                bid,
                op_idx,
                format!(
                    "invoke_ffi native callable export `{export_name}` with ABI `{abi}` has {} operand(s), expected {expected}",
                    op.operands.len()
                ),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Check 4: every block has a well-formed terminator (block exists in function)
// ---------------------------------------------------------------------------

fn verify_terminators(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    for (bid, block) in &func.blocks {
        match &block.terminator {
            Terminator::Branch { target, .. } => {
                if !func.blocks.contains_key(target) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("branch target ^{} does not exist", target),
                    ));
                }
            }
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                if !func.blocks.contains_key(then_block) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("cond_branch then_block ^{} does not exist", then_block),
                    ));
                }
                if !func.blocks.contains_key(else_block) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("cond_branch else_block ^{} does not exist", else_block),
                    ));
                }
            }
            Terminator::Switch { cases, default, .. }
            | Terminator::StateDispatch { cases, default, .. } => {
                if !func.blocks.contains_key(default) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("switch default block ^{} does not exist", default),
                    ));
                }
                for (case_val, target, _) in cases {
                    if !func.blocks.contains_key(target) {
                        errors.push(VerifyError::block(
                            *bid,
                            format!("switch case {} target ^{} does not exist", case_val, target),
                        ));
                    }
                }
            }
            Terminator::Return { values } => {
                if !func.return_abi.returns_value() && !values.is_empty() {
                    errors.push(VerifyError::block(
                        *bid,
                        "value return through a void function ABI",
                    ));
                }
                if values.len() > 1 {
                    errors.push(VerifyError::block(
                        *bid,
                        format!(
                            "Python function return carries at most one object, found {} values",
                            values.len()
                        ),
                    ));
                }
            }
            Terminator::Unreachable => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Check 4: branch arg counts match target block param counts
// ---------------------------------------------------------------------------

fn verify_block_args(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    let arg_count = |bid: &BlockId| -> Option<usize> { func.blocks.get(bid).map(|b| b.args.len()) };
    let exception_targets = dominators::exception_label_to_block(func);

    for (bid, block) in &func.blocks {
        for (op_index, op) in block.ops.iter().enumerate() {
            if !dominators::is_exception_transfer_edge(op.opcode) {
                continue;
            }
            let Some(AttrValue::Int(label)) = op.attrs.get("value") else {
                continue;
            };
            let Some(target) = exception_targets.get(label) else {
                errors.push(VerifyError::op(
                    *bid,
                    op_index,
                    format!("implicit exception edge label {label} has no unique target block"),
                ));
                continue;
            };
            let Some(expected) = arg_count(target) else {
                errors.push(VerifyError::op(
                    *bid,
                    op_index,
                    format!("implicit exception edge label {label} targets block ^{target} which does not exist"),
                ));
                continue;
            };
            if op.operands.len() != expected {
                errors.push(VerifyError::op(
                    *bid,
                    op_index,
                    format!(
                        "implicit exception edge to ^{} passes {} args but block expects {}",
                        target,
                        op.operands.len(),
                        expected
                    ),
                ));
            }
        }
        match &block.terminator {
            Terminator::Branch { target, args } => {
                if let Some(expected) = arg_count(target)
                    && args.len() != expected
                {
                    errors.push(VerifyError::block(
                        *bid,
                        format!(
                            "branch to ^{} passes {} args but block expects {}",
                            target,
                            args.len(),
                            expected
                        ),
                    ));
                }
            }
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if let Some(expected) = arg_count(then_block)
                    && then_args.len() != expected
                {
                    errors.push(VerifyError::block(
                        *bid,
                        format!(
                            "cond_branch to ^{} passes {} then_args but block expects {}",
                            then_block,
                            then_args.len(),
                            expected
                        ),
                    ));
                }
                if let Some(expected) = arg_count(else_block)
                    && else_args.len() != expected
                {
                    errors.push(VerifyError::block(
                        *bid,
                        format!(
                            "cond_branch to ^{} passes {} else_args but block expects {}",
                            else_block,
                            else_args.len(),
                            expected
                        ),
                    ));
                }
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            }
            | Terminator::StateDispatch {
                cases,
                default,
                default_args,
                ..
            } => {
                if let Some(expected) = arg_count(default)
                    && default_args.len() != expected
                {
                    errors.push(VerifyError::block(
                        *bid,
                        format!(
                            "switch default ^{} passed {} args but block expects {}",
                            default,
                            default_args.len(),
                            expected
                        ),
                    ));
                }
                for (case_val, target, args) in cases {
                    if let Some(expected) = arg_count(target)
                        && args.len() != expected
                    {
                        errors.push(VerifyError::block(
                            *bid,
                            format!(
                                "switch case {} to ^{} passes {} args but block expects {}",
                                case_val,
                                target,
                                args.len(),
                                expected
                            ),
                        ));
                    }
                }
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Check 5: SSA dominance — every use must be dominated by its definition
// ---------------------------------------------------------------------------

fn verify_ssa(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    // Include every executable exceptional entry at its observation position.
    // Retained, unreachable region labels remain structurally verified above,
    // but do not manufacture execution paths or SSA dominance obligations.
    let dom = ProgramPointDominance::compute_executable(func);
    let mut definitions = HashMap::new();

    for (bid, block) in &func.blocks {
        for arg in &block.args {
            definitions.insert(arg.id, (*bid, None));
        }
        for (op_idx, op) in block.ops.iter().enumerate() {
            for result in &op.results {
                definitions.insert(*result, (*bid, Some(op_idx)));
            }
        }
    }

    // Check every operand use.
    let check_use =
        |bid: BlockId, op_idx: Option<usize>, used: ValueId, errors: &mut Vec<VerifyError>| {
            match definitions.get(&used) {
                None => {
                    let msg = format!("{} used but never defined", used);
                    match op_idx {
                        Some(i) => errors.push(VerifyError::op(bid, i, msg)),
                        None => errors.push(VerifyError::block(bid, msg)),
                    }
                }
                Some(&(def_bid, def_op)) => {
                    let position = op_idx.unwrap_or(usize::MAX);
                    if !dom.definition_available(def_bid, def_op, bid, position) {
                        let msg = if def_bid == bid && def_op.is_some_and(|index| index >= position)
                        {
                            format!(
                                "{} used at op#{} but defined later at op#{}",
                                used,
                                position,
                                def_op.unwrap()
                            )
                        } else {
                            format!(
                                "{} defined in ^{} does not dominate use in ^{}",
                                used, def_bid, bid
                            )
                        };
                        match op_idx {
                            Some(i) => errors.push(VerifyError::op(bid, i, msg)),
                            None => errors.push(VerifyError::block(bid, msg)),
                        }
                    }
                }
            }
        };

    for (bid, block) in &func.blocks {
        // Skip unreachable blocks — their ops may reference values whose
        // definitions no longer dominate them after optimization passes
        // changed the CFG (e.g., SCCP branch folding).
        if !dom.is_reachable(*bid) {
            continue;
        }
        for (op_idx, op) in block.ops.iter().enumerate() {
            for operand in &op.operands {
                check_use(*bid, Some(op_idx), *operand, errors);
            }
        }
        // Check terminator operands.
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for v in args {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                check_use(*bid, None, *cond, errors);
                for v in then_args {
                    check_use(*bid, None, *v, errors);
                }
                for v in else_args {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                check_use(*bid, None, *value, errors);
                for (_, _, args) in cases {
                    for v in args {
                        check_use(*bid, None, *v, errors);
                    }
                }
                for v in default_args {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                for (_, _, args) in cases {
                    for v in args {
                        check_use(*bid, None, *v, errors);
                    }
                }
                for v in default_args {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::Return { values } => {
                for v in values {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::Unreachable => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The verifier must admit the identity that lowering actually transports.
    /// Operand presence cannot turn a direct call into a dynamic one, and a
    /// symbol on an opaque spelling cannot replace its callable operand.
    #[test]
    fn call_target_roles_require_transportable_identity() {
        use super::super::ops::{AttrDict, Dialect, OpCode, TirOp};
        use super::super::types::TirType;

        let check = |opcode,
                     original: Option<AttrValue>,
                     target: Option<(&str, AttrValue)>,
                     operands,
                     valid| {
            let mut func = TirFunction::new(
                "call_role".into(),
                vec![TirType::DynBox],
                TirType::None,
                crate::FunctionReturnAbi::Void,
            );
            let mut attrs = AttrDict::new();
            if let Some(kind) = original {
                attrs.insert("_original_kind".into(), kind);
            }
            if let Some((key, value)) = target {
                attrs.insert(key.into(), value);
            }
            let block = func.blocks.get_mut(&func.entry_block).unwrap();
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands: if operands { vec![ValueId(0)] } else { vec![] },
                results: vec![],
                attrs,
                source_span: None,
            });
            block.terminator = Terminator::Return { values: vec![] };
            let result = verify_function(&func);
            if valid {
                assert!(result.is_ok(), "{func:?}: {result:?}");
            } else {
                let errors = result.expect_err("untransportable call identity must fail");
                assert!(
                    errors
                        .iter()
                        .any(|error| error.message.contains("has no callee")),
                    "{errors:?}"
                );
            }
        };
        for kind in [None, Some("call"), Some("call_internal")] {
            for operands in [false, true] {
                let original = kind.map(|kind| AttrValue::Str(kind.into()));
                check(
                    OpCode::Call,
                    original.clone(),
                    Some(("s_value", AttrValue::Str("fixture_target".into()))),
                    operands,
                    true,
                );
                for target in [
                    None,
                    Some(("callee", AttrValue::Str("fixture_target".into()))),
                    Some(("s_value", AttrValue::Str(String::new()))),
                    Some(("s_value", AttrValue::Int(1))),
                ] {
                    check(OpCode::Call, original.clone(), target, operands, false);
                }
            }
        }
        for kind in [
            "call_func",
            "call_function",
            "call_indirect",
            "call_bind",
            "call_guarded",
            "invoke_ffi",
        ] {
            for target in [
                None,
                Some(("s_value", AttrValue::Str("incidental".into()))),
                Some(("callee", AttrValue::Str("incidental".into()))),
            ] {
                for operands in [false, true] {
                    check(
                        OpCode::Call,
                        Some(AttrValue::Str(kind.into())),
                        target.clone(),
                        operands,
                        operands,
                    );
                }
            }
        }
        for original in [AttrValue::Str("unknown_call".into()), AttrValue::Bool(true)] {
            check(
                OpCode::Call,
                Some(original),
                Some(("s_value", AttrValue::Str("fixture_target".into()))),
                true,
                false,
            );
        }
        for kind in ["gpu_thread_id", "gpu_barrier"] {
            let symbol =
                super::super::call_targets::gpu_runtime_symbol_for_simple_kind(kind).unwrap();
            check(
                OpCode::Call,
                Some(AttrValue::Str(kind.into())),
                Some(("s_value", AttrValue::Str(symbol.into()))),
                false,
                true,
            );
            check(
                OpCode::Call,
                Some(AttrValue::Str(kind.into())),
                Some(("s_value", AttrValue::Str("wrong_symbol".into()))),
                true,
                false,
            );
        }
        // Dedicated builtin dispatch retains its separate admission contract.
        check(OpCode::CallBuiltin, None, None, true, true);
        check(
            OpCode::CallBuiltin,
            None,
            Some(("s_value", AttrValue::Str("len".into()))),
            false,
            true,
        );
        check(OpCode::CallBuiltin, None, None, false, false);
    }

    /// Admission follows the executable builtin convention, including named
    /// zero-argument calls and the specialized range/print wire spellings.
    #[test]
    fn builtin_call_admission_uses_canonical_dispatch_contract() {
        use super::super::ops::{AttrDict, Dialect, OpCode, TirOp};
        use super::super::types::TirType;

        let check = |attrs: AttrDict, operands: usize, results: usize, valid: bool| {
            let mut func = TirFunction::new(
                "builtin_admission".into(),
                vec![TirType::DynBox; operands],
                TirType::None,
                crate::FunctionReturnAbi::Void,
            );
            let results = (0..results).map(|_| func.fresh_value()).collect();
            let block = func.blocks.get_mut(&func.entry_block).unwrap();
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CallBuiltin,
                operands: (0..operands).map(|index| ValueId(index as u32)).collect(),
                results,
                attrs,
                source_span: None,
            });
            block.terminator = Terminator::Return { values: vec![] };
            let result = verify_function(&func);
            if valid {
                assert!(result.is_ok(), "{func:?}: {result:?}");
            } else {
                let errors = result.expect_err("malformed builtin dispatch must fail admission");
                assert!(
                    errors
                        .iter()
                        .any(|error| error.message.contains("CallBuiltin op has no callee")),
                    "{errors:?}"
                );
            }
        };
        for operands in [0, 1, 3] {
            for results in [0, 1] {
                for key in ["name", "s_value"] {
                    check(
                        AttrDict::from([(key.into(), AttrValue::Str("len".into()))]),
                        operands,
                        results,
                        true,
                    );
                }
                check(
                    AttrDict::from([
                        ("name".into(), AttrValue::Str("len".into())),
                        ("s_value".into(), AttrValue::Str("len".into())),
                    ]),
                    operands,
                    results,
                    true,
                );
                check(AttrDict::new(), operands, results, operands != 0);
                for kind in ["print", "builtin_print"] {
                    check(
                        AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
                        operands,
                        results,
                        true,
                    );
                }
            }
        }
        for operands in 0..=4 {
            check(
                AttrDict::from([("_original_kind".into(), AttrValue::Str("range_new".into()))]),
                operands,
                1,
                operands == 3,
            );
        }
        for operands in [0, 1] {
            for key in ["name", "s_value", "_original_kind"] {
                for value in [AttrValue::Str(String::new()), AttrValue::Int(1)] {
                    check(AttrDict::from([(key.into(), value)]), operands, 1, false);
                }
            }
            for key in [
                "callee",
                "runtime_symbol",
                "native_callable_symbol",
                "native_callable_export",
            ] {
                let mut attrs = AttrDict::from([(key.into(), AttrValue::Str("len".into()))]);
                check(attrs.clone(), operands, 1, false);
                attrs.insert("name".into(), AttrValue::Str("len".into()));
                check(attrs, operands, 1, false);
            }
        }
        for attrs in [
            AttrDict::from([
                ("name".into(), AttrValue::Str("len".into())),
                ("s_value".into(), AttrValue::Str("abs".into())),
            ]),
            AttrDict::from([
                ("name".into(), AttrValue::Str("len".into())),
                ("_original_kind".into(), AttrValue::Str("print".into())),
            ]),
            AttrDict::from([
                ("name".into(), AttrValue::Str("len".into())),
                ("_original_kind".into(), AttrValue::Str("range_new".into())),
            ]),
            AttrDict::from([(
                "_original_kind".into(),
                AttrValue::Str("unknown_builtin".into()),
            )]),
        ] {
            check(attrs, 3, 1, false);
        }
        check(
            AttrDict::from([("name".into(), AttrValue::Str("len".into()))]),
            1,
            2,
            false,
        );
    }

    #[test]
    fn generated_preserved_shapes_fail_before_target_lowering() {
        use super::super::op_kinds_generated::{SIMPLEIR_OP_SHAPES, SimpleIrOpValueRule};
        use super::super::ops::{AttrDict, Dialect, OpCode, TirOp};
        use super::super::types::TirType;
        for shape in SIMPLEIR_OP_SHAPES {
            let mut func = TirFunction::new(
                "preserved_shape".into(),
                vec![],
                TirType::None,
                crate::FunctionReturnAbi::Void,
            );
            let mut attrs =
                AttrDict::from([("_original_kind".into(), AttrValue::Str(shape.kind.into()))]);
            if shape.value_rule == SimpleIrOpValueRule::NonNegative {
                attrs.insert("value".into(), AttrValue::Int(0));
            }
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .push(TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands: (0..shape.operands).map(|i| ValueId(i as u32)).collect(),
                    results: vec![],
                    attrs,
                    source_span: None,
                });
            assert!(verify_operation_shapes(&func).is_ok(), "{}", shape.kind);
            func.blocks.get_mut(&func.entry_block).unwrap().ops[0]
                .operands
                .push(ValueId(101));
            let errors = verify_operation_shapes(&func).unwrap_err();
            assert_eq!(errors.len(), 1);
            assert!(errors[0].message.contains(shape.kind));
            assert!(errors[0].message.contains("args"));
            let op = &mut func.blocks.get_mut(&func.entry_block).unwrap().ops[0];
            op.operands.pop();
            if shape.value_rule == SimpleIrOpValueRule::NonNegative {
                op.attrs.insert("value".into(), AttrValue::Str("0".into()));
                assert!(
                    verify_operation_shapes(&func).unwrap_err()[0]
                        .message
                        .contains("explicit nonnegative")
                );
            }
        }
    }

    #[test]
    fn retired_preserved_operations_share_wire_admission() {
        for (kind, operands) in [
            ("store_init", 2),
            ("guarded_field_init", 2),
            ("object_new_bound_stack", 1),
            ("list_repeat_range", 2),
            ("list_repeat_range", 4),
        ] {
            let expected = crate::ir_schema::validate_op_shape(kind, Some(operands as usize), None)
                .unwrap_err()
                .to_string();
            for &opcode in super::super::op_kinds_generated::ALL_OPCODES {
                let mut func = TirFunction::new(
                    "retired_operation".into(),
                    vec![],
                    TirType::None,
                    crate::FunctionReturnAbi::Void,
                );
                func.blocks
                    .get_mut(&func.entry_block)
                    .unwrap()
                    .ops
                    .push(TirOp {
                        dialect: Dialect::Molt,
                        opcode,
                        operands: (0..operands).map(ValueId).collect(),
                        results: vec![ValueId(100)],
                        attrs: AttrDict::from([(
                            "_original_kind".into(),
                            AttrValue::Str(kind.into()),
                        )]),
                        source_span: None,
                    });
                let errors = verify_operation_shapes(&func).unwrap_err();
                assert_eq!(errors.len(), 1, "{opcode:?}/{kind}");
                assert_eq!(errors[0].message, expected, "{opcode:?}/{kind}");
            }
        }
    }
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    /// Build a minimal valid function: add(i64, i64) -> i64.
    fn valid_add_function() -> TirFunction {
        let mut func = TirFunction::new(
            "add".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
            crate::FunctionReturnAbi::Value,
        );
        let result = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        func
    }

    #[test]
    fn valid_function_passes_verification() {
        let func = valid_add_function();
        assert!(
            verify_function(&func).is_ok(),
            "valid add function should pass: {:?}",
            verify_function(&func).err()
        );
    }

    #[test]
    fn async_work_marker_rejects_unrelated_tir_opcodes() {
        let mut func = valid_add_function();
        func.blocks.get_mut(&func.entry_block).unwrap().ops[0]
            .attrs
            .insert(
                super::super::ops::ASYNC_WORK_POLL_ATTR.into(),
                AttrValue::Bool(true),
            );
        let errors = verify_function(&func).expect_err("Add cannot service async work");
        assert!(errors.iter().any(|error| {
            error
                .message
                .contains("cannot carry the async_work_poll marker")
        }));
    }

    #[test]
    fn direct_symbol_pyinit_with_module_name_verifies() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            crate::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let mut attrs = AttrDict::new();
        attrs.insert(
            "native_callable_binding".into(),
            AttrValue::Str("direct_symbol".into()),
        );
        attrs.insert(
            "native_callable_symbol".into(),
            AttrValue::Str("PyInit__native".into()),
        );
        attrs.insert(
            "native_callable_export".into(),
            AttrValue::Str("__molt_static_pyinit__.nativepkg._native".into()),
        );
        attrs.insert(
            "native_callable_abi".into(),
            AttrValue::Str("molt.pyinit_module_v1".into()),
        );
        attrs.insert("_original_kind".into(), AttrValue::Str("invoke_ffi".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![ValueId(0)],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        assert!(
            verify_function(&func).is_ok(),
            "direct-symbol native call should verify: {:?}",
            verify_function(&func).err()
        );
    }

    #[test]
    fn module_attr_object_callargs_native_callable_verifies() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            crate::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let mut attrs = AttrDict::new();
        attrs.insert(
            "native_callable_binding".into(),
            AttrValue::Str("module_attr".into()),
        );
        attrs.insert(
            "native_callable_export".into(),
            AttrValue::Str("scipy.ndimage.gaussian_filter".into()),
        );
        attrs.insert(
            "native_callable_abi".into(),
            AttrValue::Str("molt.object_callargs_v1".into()),
        );
        attrs.insert("_original_kind".into(), AttrValue::Str("invoke_ffi".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        assert!(
            verify_function(&func).is_ok(),
            "module_attr object-callargs native call should verify: {:?}",
            verify_function(&func).err()
        );
    }

    #[test]
    fn module_attr_direct_symbol_abi_fails_closed() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            crate::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let mut attrs = AttrDict::new();
        attrs.insert(
            "native_callable_binding".into(),
            AttrValue::Str("module_attr".into()),
        );
        attrs.insert(
            "native_callable_export".into(),
            AttrValue::Str("scipy.ndimage.distance_transform_edt".into()),
        );
        attrs.insert(
            "native_callable_abi".into(),
            AttrValue::Str("molt.forward_f32_v1".into()),
        );
        attrs.insert("_original_kind".into(), AttrValue::Str("invoke_ffi".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("module_attr direct-symbol ABI")),
            "expected module_attr ABI-shape error, got: {:?}",
            errors
        );
    }

    #[test]
    fn module_attr_native_callable_without_operand_still_fails() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::DynBox,
            crate::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let mut attrs = AttrDict::new();
        attrs.insert(
            "native_callable_binding".into(),
            AttrValue::Str("module_attr".into()),
        );
        attrs.insert(
            "native_callable_symbol".into(),
            AttrValue::Str("scipy.ndimage.distance_transform_edt".into()),
        );

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("has no callee")),
            "expected missing callee error, got: {:?}",
            errors
        );
    }

    fn unpack_function(operand_count: usize, result_count: usize, expected: i64) -> TirFunction {
        let mut func = TirFunction::new(
            "unpack".into(),
            vec![TirType::DynBox; operand_count],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let results: Vec<ValueId> = (0..result_count).map(|_| func.fresh_value()).collect();
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(expected));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::UnpackSequence,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
            results,
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };
        func
    }

    #[test]
    fn unpack_sequence_shape_is_verified() {
        assert!(verify_function(&unpack_function(1, 2, 2)).is_ok());
        assert!(verify_function(&unpack_function(1, 0, 0)).is_ok());
        for malformed in [
            unpack_function(0, 0, 0),
            unpack_function(2, 2, 2),
            unpack_function(1, 2, 1),
            unpack_function(1, 1, 2),
            unpack_function(1, 0, -1),
        ] {
            let errors = verify_function(&malformed).expect_err("malformed unpack must fail");
            assert!(
                errors.iter().any(|error| error
                    .message
                    .contains("UnpackSequence requires one operand")),
                "unexpected errors: {errors:?}"
            );
        }
    }

    #[test]
    fn missing_entry_block_fails() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        // Set entry_block to a non-existent block id.
        func.entry_block = BlockId(99);
        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("entry block")),
            "expected entry block error, got: {:?}",
            errors
        );
    }

    #[test]
    fn branch_to_nonexistent_block_fails() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        // Point the entry block terminator to a block that doesn't exist.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: BlockId(99),
            args: vec![],
        };
        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("does not exist")),
            "expected 'does not exist' error, got: {:?}",
            errors
        );
    }

    #[test]
    fn wrong_branch_arg_count_fails() {
        // Entry branches to bb1 but passes 1 arg; bb1 expects 0.
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );

        // Add a const so we have ValueId(0) defined.
        let v0 = func.fresh_value();
        let bb1 = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v0],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![v0], // passing 1 arg
        };

        // bb1 expects no args.
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![], // expects 0
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("expects")),
            "expected arg-count error, got: {:?}",
            errors
        );
    }

    #[test]
    fn implicit_exception_edges_reject_missing_or_ambiguous_targets_without_panicking() {
        for opcode in [OpCode::CheckException, OpCode::TryStart] {
            for labels in [vec![], vec![(99, 77)], vec![(0, 77), (99, 77)]] {
                for reverse in [false, true] {
                    let mut func = TirFunction::new(
                        "invalid_handler".into(),
                        vec![],
                        TirType::None,
                        crate::FunctionReturnAbi::Void,
                    );
                    let mut labels = labels.clone();
                    if reverse {
                        labels.reverse();
                    }
                    func.label_id_map.extend(labels);
                    let mut attrs = AttrDict::new();
                    attrs.insert("value".into(), AttrValue::Int(77));
                    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
                    entry.ops.push(TirOp {
                        dialect: Dialect::Molt,
                        opcode,
                        operands: vec![],
                        results: vec![],
                        attrs,
                        source_span: None,
                    });
                    entry.terminator = Terminator::Return { values: vec![] };
                    let errors =
                        verify_function(&func).expect_err("invalid handler must be diagnosed");
                    assert_eq!(errors.len(), 1, "{opcode:?}: {errors:?}");
                    assert_eq!(errors[0].block, Some(func.entry_block));
                    assert_eq!(errors[0].op_index, Some(0));
                    assert!(
                        errors[0]
                            .message
                            .contains("implicit exception edge label 77")
                    );
                    assert_eq!(
                        dominators::executable_reverse_postorder(&func),
                        vec![func.entry_block]
                    );
                    assert_eq!(dominators::build_pred_map(&func).len(), 1);
                }
            }
        }
    }

    #[test]
    fn implicit_exception_edge_arg_count_is_verified_for_the_full_opcode_family() {
        for opcode in [OpCode::CheckException, OpCode::TryStart] {
            let mut func = TirFunction::new(
                "exception_edge".into(),
                vec![TirType::DynBox],
                TirType::None,
                crate::FunctionReturnAbi::Void,
            );
            let handler = func.fresh_block();
            let handler_arg = func.fresh_value();
            func.value_types.insert(handler_arg, TirType::DynBox);
            func.label_id_map.insert(handler.0, 77);
            func.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![TirValue {
                        id: handler_arg,
                        ty: TirType::DynBox,
                    }],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(77));
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .push(TirOp {
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![],
                    results: vec![],
                    attrs,
                    source_span: None,
                });
            func.blocks.get_mut(&func.entry_block).unwrap().terminator =
                Terminator::Return { values: vec![] };

            let errors = verify_function(&func).expect_err("missing exception payload must fail");
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("implicit exception edge")),
                "unexpected errors for {opcode:?}: {errors:?}"
            );

            func.blocks.get_mut(&func.entry_block).unwrap().ops[0].operands = vec![ValueId(0)];
            assert!(
                verify_function(&func).is_ok(),
                "one payload per handler arg must verify for {opcode:?}: {:?}",
                verify_function(&func).err()
            );
        }
    }

    #[test]
    fn duplicate_value_definition_fails() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let v0 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // Define v0 twice.
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v0],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v0], // duplicate!
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("duplicate")),
            "expected duplicate error, got: {:?}",
            errors
        );
    }

    #[test]
    fn multiple_python_return_values_fail_before_target_lowering() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
            crate::FunctionReturnAbi::Value,
        );
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0), ValueId(1)],
        };

        let errors = verify_function(&func).expect_err("multi-value Python return must fail");
        assert!(errors.iter().any(|error| {
            error
                .message
                .contains("Python function return carries at most one object")
        }));
    }

    #[test]
    fn use_of_undefined_value_fails() {
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let undefined = ValueId(999);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Neg,
            operands: vec![undefined], // never defined
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("never defined")),
            "expected undefined value error, got: {:?}",
            errors
        );
    }

    #[test]
    fn valid_multi_block_function_passes() {
        // Build: func @branch(bool) -> i64
        //   ^bb0(%0: bool):
        //     cond_br %0, ^bb1, ^bb2
        //   ^bb1:
        //     %2 = const_int {value: 1}
        //     return %2
        //   ^bb2:
        //     %3 = const_int {value: 0}
        //     return %3
        let mut func = TirFunction::new(
            "branch".into(),
            vec![TirType::Bool],
            TirType::I64,
            crate::FunctionReturnAbi::Value,
        );

        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();

        let v1 = func.fresh_value();
        let v2 = func.fresh_value();

        // Entry.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: bb1,
            then_args: vec![],
            else_block: bb2,
            else_args: vec![],
        };

        // bb1.
        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), crate::tir::ops::AttrValue::Int(1));
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v1],
                    attrs: attrs1,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v1] },
            },
        );

        // bb2.
        let mut attrs2 = AttrDict::new();
        attrs2.insert("value".into(), crate::tir::ops::AttrValue::Int(0));
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v2],
                    attrs: attrs2,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v2] },
            },
        );

        assert!(
            verify_function(&func).is_ok(),
            "multi-block branch function should pass: {:?}",
            verify_function(&func).err()
        );
    }

    fn capture_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn capture_transfer(opcode: OpCode, label: i64, operands: Vec<ValueId>) -> TirOp {
        let mut op = capture_op(opcode, operands, vec![]);
        op.attrs.insert("value".into(), AttrValue::Int(label));
        op
    }

    #[test]
    fn exceptional_captures_require_definition_before_every_observation() {
        for early_observation in [false, true] {
            let mut func = TirFunction::new(
                "exceptional_capture".into(),
                vec![],
                TirType::None,
                crate::FunctionReturnAbi::Void,
            );
            let handler = func.fresh_block();
            let continuation = func.fresh_block();
            let owner = func.fresh_value();
            let handler_owner = func.fresh_value();
            for value in [owner, handler_owner] {
                func.value_types.insert(value, TirType::DynBox);
            }
            func.label_id_map.insert(handler.0, 17);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            // Registration precedes allocation but does not execute the handler.
            entry
                .ops
                .push(capture_transfer(OpCode::TryStart, 17, vec![]));
            if early_observation {
                entry
                    .ops
                    .push(capture_transfer(OpCode::CheckException, 17, vec![]));
            }
            entry
                .ops
                .push(capture_op(OpCode::BuildList, vec![], vec![owner]));
            entry
                .ops
                .push(capture_transfer(OpCode::CheckException, 17, vec![]));
            entry.terminator = Terminator::Return { values: vec![] };
            func.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![
                        capture_op(OpCode::DecRef, vec![owner], vec![]),
                        capture_op(OpCode::BuildList, vec![], vec![handler_owner]),
                    ],
                    terminator: Terminator::Branch {
                        target: continuation,
                        args: vec![],
                    },
                },
            );
            func.blocks.insert(
                continuation,
                TirBlock {
                    id: continuation,
                    args: vec![],
                    ops: vec![capture_op(OpCode::DecRef, vec![handler_owner], vec![])],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            let result = verify_function(&func);
            if early_observation {
                let errors = result.expect_err("early exceptional entry skips the captured owner");
                assert!(
                    errors.iter().any(|error| error.block == Some(handler)
                        && error.op_index == Some(0)
                        && error.message.contains("does not dominate")),
                    "missing captured-owner rejection: {errors:?}"
                );
            } else {
                result.expect("protected and handler-local definitions reach every executable use");
            }
        }
    }

    #[test]
    fn shared_exceptional_tail_checks_all_arms_and_explicit_payloads() {
        for capture_arm_local in [false, true] {
            let mut func = TirFunction::new(
                "shared_capture".into(),
                vec![TirType::Bool],
                TirType::None,
                crate::FunctionReturnAbi::Void,
            );
            let left = func.fresh_block();
            let right = func.fresh_block();
            let cleanup = func.fresh_block();
            let common = func.fresh_value();
            let left_owner = func.fresh_value();
            let right_owner = func.fresh_value();
            let payload = func.fresh_value();
            for value in [common, left_owner, right_owner, payload] {
                func.value_types.insert(value, TirType::DynBox);
            }
            func.label_id_map.insert(cleanup.0, 19);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry
                .ops
                .push(capture_op(OpCode::BuildList, vec![], vec![common]));
            entry.terminator = Terminator::CondBranch {
                cond: ValueId(0),
                then_block: left,
                then_args: vec![],
                else_block: right,
                else_args: vec![],
            };
            for (id, owner) in [(left, left_owner), (right, right_owner)] {
                func.blocks.insert(
                    id,
                    TirBlock {
                        id,
                        args: vec![],
                        ops: vec![
                            capture_op(OpCode::BuildList, vec![], vec![owner]),
                            capture_transfer(OpCode::CheckException, 19, vec![owner]),
                        ],
                        terminator: Terminator::Return { values: vec![] },
                    },
                );
            }
            func.blocks.insert(
                cleanup,
                TirBlock {
                    id: cleanup,
                    args: vec![TirValue {
                        id: payload,
                        ty: TirType::DynBox,
                    }],
                    ops: vec![
                        capture_op(
                            OpCode::DecRef,
                            vec![if capture_arm_local {
                                left_owner
                            } else {
                                common
                            }],
                            vec![],
                        ),
                        capture_op(OpCode::DecRef, vec![payload], vec![]),
                    ],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            let result = verify_function(&func);
            if capture_arm_local {
                let errors = result.expect_err("right entry does not define left's captured owner");
                assert!(
                    errors.iter().any(|error| error.block == Some(cleanup)
                        && error.op_index == Some(0)
                        && error.message.contains("does not dominate")),
                    "missing cross-arm capture rejection: {errors:?}"
                );
            } else {
                result.expect("common capture and arm-specific explicit payload are available");
            }
        }
    }

    #[test]
    fn dominator_metadata_handles_reachable_and_unreachable_blocks() {
        let mut func = TirFunction::new(
            "dom_meta".into(),
            vec![TirType::Bool, TirType::I64],
            TirType::I64,
            crate::FunctionReturnAbi::Value,
        );
        let bb_then = func.fresh_block();
        let bb_else = func.fresh_block();
        let bb_join = func.fresh_block();
        let bb_dead = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: bb_then,
            then_args: vec![],
            else_block: bb_else,
            else_args: vec![],
        };

        func.blocks.insert(
            bb_then,
            TirBlock {
                id: bb_then,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb_join,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb_else,
            TirBlock {
                id: bb_else,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb_join,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            bb_join,
            TirBlock {
                id: bb_join,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![ValueId(1)],
                },
            },
        );
        func.blocks.insert(
            bb_dead,
            TirBlock {
                id: bb_dead,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );

        let dom_tree = ProgramPointDominance::compute_executable(&func);
        assert!(dom_tree.definition_available(func.entry_block, None, bb_then, 0));
        assert!(dom_tree.definition_available(func.entry_block, None, bb_else, 0));
        assert!(dom_tree.definition_available(func.entry_block, None, bb_join, 0));
        assert!(!dom_tree.definition_available(bb_then, None, bb_join, 0));
        assert!(!dom_tree.definition_available(bb_else, None, bb_join, 0));
        assert!(!dom_tree.definition_available(bb_dead, None, bb_dead, 0));
        assert!(!dom_tree.definition_available(BlockId(99), None, BlockId(99), 0));
        assert!(!dom_tree.definition_available(func.entry_block, None, bb_dead, 0));
    }

    #[test]
    fn dominator_metadata_matches_idom_chain_reference() {
        let mut func = TirFunction::new(
            "dom_ref".into(),
            vec![TirType::Bool],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let entry = func.entry_block;

        let mut blocks = Vec::new();
        for _ in 0..12 {
            blocks.push(func.fresh_block());
        }
        let unreachable = func.fresh_block();

        let entry_block = func.blocks.get_mut(&entry).unwrap();
        entry_block.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: blocks[0],
            then_args: vec![],
            else_block: blocks[1],
            else_args: vec![],
        };

        for (idx, bid) in blocks.iter().enumerate() {
            let terminator = if idx == blocks.len() - 1 {
                Terminator::Return { values: vec![] }
            } else if idx % 3 == 0 {
                Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: blocks[idx + 1],
                    then_args: vec![],
                    else_block: blocks[(idx + 2).min(blocks.len() - 1)],
                    else_args: vec![],
                }
            } else {
                Terminator::Branch {
                    target: blocks[idx + 1],
                    args: vec![],
                }
            };
            func.blocks.insert(
                *bid,
                TirBlock {
                    id: *bid,
                    args: vec![],
                    ops: vec![],
                    terminator,
                },
            );
        }
        func.blocks.insert(
            unreachable,
            TirBlock {
                id: unreachable,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );

        let dom_tree = ProgramPointDominance::compute_executable(&func);
        let policy = dominators::CfgEdgePolicy::TerminatorOnly;
        let predecessors = dominators::build_pred_map_with(&func, policy);
        let idom = dominators::compute_idoms_with(&func, &predecessors, policy);
        let mut all_blocks = vec![entry];
        all_blocks.extend(blocks.iter().copied());
        all_blocks.push(unreachable);

        for &a in &all_blocks {
            for &b in &all_blocks {
                assert_eq!(
                    dom_tree.definition_available(a, None, b, 0),
                    dominators::dominates(a, b, &idom),
                    "dominance mismatch: {} -> {}",
                    a,
                    b
                );
            }
        }
    }
}
