use super::purity::{
    SimpleIrScalarPurityFacts, simple_ir_op_is_provably_nonthrowing_with_facts,
    simple_ir_op_needs_scalar_plan_for_nonthrowing,
};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::tir::simple_def_use::{visit_simple_ir_reads, visit_simple_ir_result_names};
use crate::tir::target_info::TargetInfo;
use crate::{OpIR, SimpleIR};
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use std::collections::HashSet;

fn simple_ir_unused_result_is_removable(
    facts: Option<&SimpleIrScalarPurityFacts<'_>>,
    op: &OpIR,
) -> bool {
    if !simple_ir_op_is_provably_nonthrowing_with_facts(facts, op) {
        return false;
    }

    matches!(
        op.kind,
        molt_ir::OpKind::Const
            | molt_ir::OpKind::ConstInt
            | molt_ir::OpKind::ConstFloat
            | molt_ir::OpKind::ConstStr
            | molt_ir::OpKind::ConstBool
            | molt_ir::OpKind::ConstNone
            | molt_ir::OpKind::ConstBytes
            | molt_ir::OpKind::ConstBigint
            | molt_ir::OpKind::ConstEllipsis
            | molt_ir::OpKind::Missing
            | molt_ir::OpKind::Copy
            | molt_ir::OpKind::CopyVar
            | molt_ir::OpKind::LoadVar
            | molt_ir::OpKind::Add
            | molt_ir::OpKind::Sub
            | molt_ir::OpKind::Mul
            | molt_ir::OpKind::InplaceAdd
            | molt_ir::OpKind::InplaceSub
            | molt_ir::OpKind::InplaceMul
            | molt_ir::OpKind::Neg
            | molt_ir::OpKind::Pos
            | molt_ir::OpKind::BitAnd
            | molt_ir::OpKind::BitOr
            | molt_ir::OpKind::BitXor
            | molt_ir::OpKind::BitNot
            | molt_ir::OpKind::Shl
            | molt_ir::OpKind::Shr
            | molt_ir::OpKind::Lshift
            | molt_ir::OpKind::Rshift
            | molt_ir::OpKind::InplaceBitAnd
            | molt_ir::OpKind::InplaceBitOr
            | molt_ir::OpKind::InplaceBitXor
            | molt_ir::OpKind::InplaceLshift
            | molt_ir::OpKind::InplaceRshift
            | molt_ir::OpKind::Lt
            | molt_ir::OpKind::Le
            | molt_ir::OpKind::Gt
            | molt_ir::OpKind::Ge
            | molt_ir::OpKind::Eq
            | molt_ir::OpKind::Ne
            | molt_ir::OpKind::Is
            | molt_ir::OpKind::IsNot
            | molt_ir::OpKind::ModuleCacheGet
            | molt_ir::OpKind::ModuleGetAttr
            | molt_ir::OpKind::Box
            | molt_ir::OpKind::Unbox
            | molt_ir::OpKind::Cast
            | molt_ir::OpKind::Widen
            | molt_ir::OpKind::IdentityAlias
            | molt_ir::OpKind::BindingAlias
            | molt_ir::OpKind::BuildList
            | molt_ir::OpKind::BuildTuple
            | molt_ir::OpKind::BuildDict
            | molt_ir::OpKind::BuildSet
            | molt_ir::OpKind::BuildSlice
            | molt_ir::OpKind::TypeGuard
    )
}

pub fn eliminate_dead_ops(ir: &mut SimpleIR, target_info: &TargetInfo) {
    if backend_env::var(BackendEnv::MOLT_DISABLE_DEAD_OP_ELIM).is_ok() {
        return;
    }

    let trace = backend_env::var(BackendEnv::MOLT_DEBUG_DEAD_OP_ELIM).is_ok();
    let mut total_removed = 0usize;

    for func in &mut ir.functions {
        // Representation planning lowers and optimizes the complete function.
        // A DCE round only deletes unused pure operations, so facts for every
        // retained definition remain valid. Build lazily on the first round
        // that needs scalar proof and reuse it through the fixpoint instead of
        // repeating the full target pipeline up to five times.
        let mut scalar_plan = None;
        for _round in 0..5 {
            // Build a set of all consumed names from the canonical field-role
            // visitor. In particular, unpack_sequence trailing args are
            // definitions, not self-consumption that pins dead unpack ops.
            let mut consumed: HashSet<String> = HashSet::new();

            for op in &func.ops {
                visit_simple_ir_reads(op, |source| {
                    consumed.insert(source.name.to_string());
                });
                // s_value can reference function names or variable names
                // in certain ops — conservatively keep anything it references.
                if let Some(sv) = &op.s_value {
                    // Only count as consumed if this is a load/copy-like op
                    // that reads the value. Calls reference functions, not locals.
                    if matches!(
                        op.kind,
                        molt_ir::OpKind::CopyVar
                            | molt_ir::OpKind::LoadVar
                            | molt_ir::OpKind::StoreVar
                    ) {
                        consumed.insert(sv.clone());
                    }
                }
            }

            let before = func.ops.len();
            let needs_scalar_plan = func.ops.iter().any(|op| {
                if !simple_ir_op_needs_scalar_plan_for_nonthrowing(op) {
                    return false;
                }
                let mut has_result = false;
                let mut any_consumed = false;
                visit_simple_ir_result_names(op, |name| {
                    has_result = true;
                    any_consumed |= consumed.contains(name);
                });
                has_result && !any_consumed
            });
            if needs_scalar_plan && scalar_plan.is_none() {
                scalar_plan = Some(ScalarRepresentationPlan::for_function_ir_for_target(
                    func,
                    target_info,
                ));
            }
            let scalar_facts = needs_scalar_plan
                .then(|| SimpleIrScalarPurityFacts::for_function(func, scalar_plan.as_ref()));

            func.ops.retain(|op| {
                // Keep all ops whose execution is observable, including
                // potential exceptions and user-code dispatch.
                if !simple_ir_unused_result_is_removable(scalar_facts.as_ref(), op) {
                    return true;
                }

                // Keep nops (they're just markers, trivial to keep).
                if op.kind == molt_ir::OpKind::Nop {
                    return true;
                }

                let mut has_result = false;
                let mut any_consumed = false;
                visit_simple_ir_result_names(op, |name| {
                    has_result = true;
                    any_consumed |= consumed.contains(name);
                });
                if has_result {
                    return any_consumed;
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
