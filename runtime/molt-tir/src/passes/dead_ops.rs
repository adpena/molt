use super::def_use::{simple_ir_defined_names, simple_ir_var_field_is_read};
use super::purity::{
    SimpleIrScalarPurityFacts, simple_ir_op_is_provably_nonthrowing_with_facts,
    simple_ir_op_needs_scalar_plan_for_nonthrowing,
};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::{OpIR, SimpleIR};
use std::collections::HashSet;

fn simple_ir_unused_result_is_removable(
    facts: Option<&SimpleIrScalarPurityFacts<'_>>,
    op: &OpIR,
) -> bool {
    if !simple_ir_op_is_provably_nonthrowing_with_facts(facts, op) {
        return false;
    }

    matches!(
        op.kind.as_str(),
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
            | "copy"
            | "copy_var"
            | "load_var"
            | "load_fast"
            | "load_var_slot"
            | "load_closure"
            | "add"
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
            | "shl"
            | "shr"
            | "lshift"
            | "rshift"
            | "inplace_bit_and"
            | "inplace_bit_or"
            | "inplace_bit_xor"
            | "inplace_lshift"
            | "inplace_rshift"
            | "lt"
            | "le"
            | "gt"
            | "ge"
            | "eq"
            | "ne"
            | "is"
            | "is_not"
            | "module_cache_get"
            | "module_get_attr"
            | "box"
            | "unbox"
            | "cast"
            | "widen"
            | "identity_alias"
            | "binding_alias"
            | "build_list"
            | "build_tuple"
            | "build_dict"
            | "build_set"
            | "build_slice"
            | "type_guard"
    )
}

pub fn eliminate_dead_ops(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_DEAD_OP_ELIM").is_ok() {
        return;
    }

    let trace = std::env::var("MOLT_DEBUG_DEAD_OP_ELIM").is_ok();
    let mut total_removed = 0usize;

    for func in &mut ir.functions {
        for _round in 0..5 {
            // Build a set of all consumed names. `args` are always data
            // inputs; `var` is input for read/copy/return-like ops but is a
            // target definition for store_var and iter_next_unboxed.
            let mut consumed: HashSet<String> = HashSet::new();

            for op in &func.ops {
                if let Some(args) = &op.args {
                    for arg in args {
                        consumed.insert(arg.clone());
                    }
                }
                if let Some(v) = &op.var
                    && simple_ir_var_field_is_read(op)
                {
                    consumed.insert(v.clone());
                }
                // s_value can reference function names or variable names
                // in certain ops — conservatively keep anything it references.
                if let Some(sv) = &op.s_value {
                    // Only count as consumed if this is a load/copy-like op
                    // that reads the value. Calls reference functions, not locals.
                    if matches!(
                        op.kind.as_str(),
                        "copy_var" | "load_var" | "load_fast" | "store_var"
                    ) {
                        consumed.insert(sv.clone());
                    }
                }
            }

            let before = func.ops.len();
            let needs_scalar_plan = func.ops.iter().any(|op| {
                simple_ir_op_needs_scalar_plan_for_nonthrowing(op)
                    && !simple_ir_defined_names(op).is_empty()
                    && simple_ir_defined_names(op)
                        .iter()
                        .all(|name| !consumed.contains(*name))
            });
            let scalar_plan =
                needs_scalar_plan.then(|| ScalarRepresentationPlan::for_function_ir(func));
            let scalar_facts = needs_scalar_plan
                .then(|| SimpleIrScalarPurityFacts::for_function(func, scalar_plan.as_ref()));

            func.ops.retain(|op| {
                // Keep all ops whose execution is observable, including
                // potential exceptions and user-code dispatch.
                if !simple_ir_unused_result_is_removable(scalar_facts.as_ref(), op) {
                    return true;
                }

                // Keep nops (they're just markers, trivial to keep).
                if op.kind == "nop" {
                    return true;
                }

                let defined = simple_ir_defined_names(op);
                if !defined.is_empty() {
                    return defined.iter().any(|name| consumed.contains(*name));
                }

                // Ops without a result variable but with no side effects
                // are dead (e.g., a bare `build_list` with no assignment).
                // Conservatively keep them — they might be consumed by
                // stack-based implicit references we can't see.
                true
            });

            let removed = before - func.ops.len();
            total_removed += removed;

            if removed == 0 {
                break; // fixpoint
            }
        }
    }

    if trace && total_removed > 0 {
        eprintln!("dead-op-elim: removed {total_removed} dead ops across all functions");
    }
}
