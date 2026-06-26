use crate::repr::ScalarKind;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::tir::effect_proof::simple_ir_has_static_module_class_binding_effect_proof;
use crate::{FunctionIR, OpIR};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// SimpleIR dead op elimination (intra-function)
//
// Removes ops within each function whose results are never consumed by any
// subsequent op. This is the SimpleIR equivalent of TIR DCE — it catches
// waste from frontend codegen before TIR lifting even sees it.
//
// Safety: only removes ops that are provably pure (no side effects).
// Side-effecting ops (calls, stores, raises, imports) are always preserved.
// ---------------------------------------------------------------------------

/// Returns `true` for SimpleIR ops that provably cannot introduce a Python
/// exception. This is intentionally stricter than "has no writes": expression
/// statements still have to execute user dispatch and raise the same exceptions
/// as CPython even when their produced value is unused.
pub struct SimpleIrScalarPurityFacts<'a> {
    plan: Option<&'a ScalarRepresentationPlan>,
    literal_kinds: BTreeMap<String, ScalarKind>,
}

impl<'a> SimpleIrScalarPurityFacts<'a> {
    pub fn for_function(func: &FunctionIR, plan: Option<&'a ScalarRepresentationPlan>) -> Self {
        let literal_kinds = func
            .ops
            .iter()
            .filter_map(|op| {
                let out = op.out.as_ref()?;
                Some((out.clone(), simple_ir_literal_scalar_kind(op)?))
            })
            .collect();
        Self {
            plan,
            literal_kinds,
        }
    }

    fn name_scalar_kind(&self, name: &str) -> Option<ScalarKind> {
        self.literal_kinds
            .get(name)
            .copied()
            .or_else(|| self.plan.and_then(|plan| plan.name_scalar_kind(name)))
    }

    fn name_is_integer_family(&self, name: &str) -> bool {
        matches!(
            self.name_scalar_kind(name),
            Some(ScalarKind::Int | ScalarKind::Bool)
        ) || self
            .plan
            .is_some_and(|plan| plan.name_is_integer_family(name))
    }
}

fn simple_ir_literal_scalar_kind(op: &OpIR) -> Option<ScalarKind> {
    match op.kind.as_str() {
        "const" => op.value.map(|_| ScalarKind::Int),
        "const_bool" => Some(ScalarKind::Bool),
        "const_float" => Some(ScalarKind::Float),
        "const_str" => Some(ScalarKind::Str),
        "const_none" => Some(ScalarKind::NoneValue),
        _ => None,
    }
}

pub fn simple_ir_op_is_provably_nonthrowing_with_facts(
    facts: Option<&SimpleIrScalarPurityFacts<'_>>,
    op: &OpIR,
) -> bool {
    let kind = op.kind.as_str();

    if simple_ir_op_has_static_module_class_binding_effect_proof(op) {
        return true;
    }

    if matches!(
        kind,
        "const"
            | "const_int"
            | "const_float"
            | "const_str"
            | "const_bool"
            | "const_none"
            | "const_bytes"
            | "const_bigint"
            | "const_ellipsis"
            | "missing"
    ) {
        return true;
    }

    if matches!(
        kind,
        "load_var"
            | "store_var"
            | "load_fast"
            | "store_fast"
            | "load_var_slot"
            | "store_var_slot"
            | "load_closure"
            | "store_closure"
    ) {
        return true;
    }

    if matches!(
        kind,
        "copy"
            | "copy_var"
            | "identity_alias"
            | "binding_alias"
            | "box"
            | "unbox"
            | "cast"
            | "widen"
            | "phi"
    ) {
        return true;
    }

    if let Some(facts) = facts
        && simple_ir_scalar_op_is_provably_nonthrowing(facts, op)
    {
        return true;
    }

    if matches!(kind, "is" | "is_not") {
        return true;
    }

    if matches!(
        kind,
        "guard_tag" | "guard_layout" | "guard_int" | "guard_float" | "type_guard"
    ) {
        return true;
    }

    if matches!(kind, "store" | "load") {
        return true;
    }

    if matches!(
        kind,
        "if" | "else"
            | "end_if"
            | "loop_start"
            | "loop_end"
            | "loop_continue"
            | "loop_break"
            | "loop_break_if_false"
            | "loop_index_start"
            | "loop_index_next"
            | "jump"
            | "label"
            | "line"
    ) {
        return true;
    }

    if matches!(kind, "code_slots_init" | "code_slot_set" | "code_new") {
        return true;
    }

    if matches!(
        kind,
        "trace_enter_slot"
            | "trace_exit"
            | "exception_clear"
            | "exception_last"
            | "exception_last_pending"
            | "exception_finally_pending_observer"
            | "exception_stack_enter"
            | "exception_stack_clear"
            | "exception_stack_depth"
            | "context_depth"
            | "check_exception"
    ) {
        return true;
    }

    false
}

fn simple_ir_scalar_op_is_provably_nonthrowing(
    facts: &SimpleIrScalarPurityFacts<'_>,
    op: &OpIR,
) -> bool {
    let args = op.args.as_deref().unwrap_or(&[]);
    let arg_kind = |name: &str| facts.name_scalar_kind(name);
    let arg_is_numeric = |name: &str| {
        matches!(
            arg_kind(name),
            Some(ScalarKind::Int | ScalarKind::Bool | ScalarKind::Float)
        )
    };
    let all_args_numeric =
        || !args.is_empty() && args.iter().all(|arg| arg_is_numeric(arg.as_str()));
    let all_args_str = || {
        !args.is_empty()
            && args
                .iter()
                .all(|arg| arg_kind(arg.as_str()) == Some(ScalarKind::Str))
    };
    let all_args_scalar = || !args.is_empty() && args.iter().all(|arg| arg_kind(arg).is_some());
    let first_source_kind = || {
        op.var
            .as_deref()
            .or_else(|| args.first().map(String::as_str))
            .and_then(arg_kind)
    };

    match op.kind.as_str() {
        "add" | "inplace_add" => all_args_numeric() || all_args_str(),
        "sub" | "mul" | "inplace_sub" | "inplace_mul" => all_args_numeric(),
        "neg" | "pos" => matches!(
            first_source_kind(),
            Some(ScalarKind::Int | ScalarKind::Bool | ScalarKind::Float)
        ),
        "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "bitand" | "bitor" | "bitxor"
        | "inplace_bit_and" | "inplace_bit_or" | "inplace_bit_xor" => {
            !args.is_empty()
                && args
                    .iter()
                    .all(|arg| facts.name_is_integer_family(arg.as_str()))
        }
        "eq" | "ne" => all_args_scalar(),
        "lt" | "le" | "gt" | "ge" => all_args_numeric() || all_args_str(),
        _ => false,
    }
}

pub(super) fn simple_ir_op_needs_scalar_plan_for_nonthrowing(op: &OpIR) -> bool {
    matches!(
        op.kind.as_str(),
        "add"
            | "sub"
            | "mul"
            | "inplace_add"
            | "inplace_sub"
            | "inplace_mul"
            | "neg"
            | "pos"
            | "bit_and"
            | "bit_or"
            | "bit_xor"
            | "bit_not"
            | "bitand"
            | "bitor"
            | "bitxor"
            | "inplace_bit_and"
            | "inplace_bit_or"
            | "inplace_bit_xor"
            | "lt"
            | "le"
            | "gt"
            | "ge"
            | "eq"
            | "ne"
    )
}

/// Returns `true` when an unused-result op can be erased without dropping
/// Python-observable behaviour.
fn simple_ir_op_has_static_module_class_binding_effect_proof(op: &OpIR) -> bool {
    simple_ir_has_static_module_class_binding_effect_proof(&op.kind, op.effect_proof.as_deref())
}
