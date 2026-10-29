use crate::repr::ScalarKind;
use crate::representation_plan::ScalarRepresentationPlan;
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
    guards: super::guard_elision::RuntimeGuardFacts,
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
            guards: super::guard_elision::RuntimeGuardFacts::for_function(func),
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
    match op.kind {
        molt_ir::OpKind::Const => op.value.map(|_| ScalarKind::Int),
        molt_ir::OpKind::ConstBool => Some(ScalarKind::Bool),
        molt_ir::OpKind::ConstFloat => Some(ScalarKind::Float),
        molt_ir::OpKind::ConstStr => Some(ScalarKind::Str),
        molt_ir::OpKind::ConstNone => Some(ScalarKind::NoneValue),
        _ => None,
    }
}

pub fn simple_ir_op_is_provably_nonthrowing_with_facts(
    facts: Option<&SimpleIrScalarPurityFacts<'_>>,
    op: &OpIR,
) -> bool {
    let kind = op.kind;

    if matches!(kind, molt_ir::OpKind::GuardTag | molt_ir::OpKind::GuardType) {
        return facts.is_some_and(|facts| facts.guards.is_nonthrowing(op, facts.plan));
    }

    if op.is_async_work_poll() {
        return false;
    }

    if matches!(
        kind,
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
    ) {
        return true;
    }

    if matches!(
        kind,
        molt_ir::OpKind::LoadVar | molt_ir::OpKind::StoreVar | molt_ir::OpKind::StoreFast
    ) {
        return true;
    }

    if matches!(
        kind,
        molt_ir::OpKind::Copy
            | molt_ir::OpKind::CopyVar
            | molt_ir::OpKind::IdentityAlias
            | molt_ir::OpKind::BindingAlias
            | molt_ir::OpKind::Unbox
            | molt_ir::OpKind::Cast
            | molt_ir::OpKind::Widen
            | molt_ir::OpKind::Phi
    ) {
        return true;
    }

    if let Some(facts) = facts
        && simple_ir_scalar_op_is_provably_nonthrowing(facts, op)
    {
        return true;
    }

    if matches!(kind, molt_ir::OpKind::Is | molt_ir::OpKind::IsNot) {
        return true;
    }

    if matches!(
        kind,
        molt_ir::OpKind::GuardLayout
            | molt_ir::OpKind::GuardInt
            | molt_ir::OpKind::GuardFloat
            | molt_ir::OpKind::TypeGuard
    ) {
        return true;
    }

    if matches!(kind, molt_ir::OpKind::Store | molt_ir::OpKind::Load) {
        return true;
    }

    if matches!(
        kind,
        molt_ir::OpKind::If
            | molt_ir::OpKind::Else
            | molt_ir::OpKind::EndIf
            | molt_ir::OpKind::LoopStart
            | molt_ir::OpKind::LoopEnd
            | molt_ir::OpKind::LoopContinue
            | molt_ir::OpKind::LoopBreak
            | molt_ir::OpKind::LoopBreakIfFalse
            | molt_ir::OpKind::LoopIndexStart
            | molt_ir::OpKind::LoopIndexNext
            | molt_ir::OpKind::Jump
            | molt_ir::OpKind::Label
            | molt_ir::OpKind::Line
    ) {
        return true;
    }

    if matches!(
        kind,
        molt_ir::OpKind::CodeSlotsInit | molt_ir::OpKind::CodeSlotSet | molt_ir::OpKind::CodeNew
    ) {
        return true;
    }

    if matches!(
        kind,
        molt_ir::OpKind::TraceExit
            | molt_ir::OpKind::ExceptionClear
            | molt_ir::OpKind::ExceptionLast
            | molt_ir::OpKind::ExceptionLastPending
            | molt_ir::OpKind::ExceptionFinallyPendingObserver
            | molt_ir::OpKind::ExceptionStackEnter
            | molt_ir::OpKind::ExceptionStackClear
            | molt_ir::OpKind::ExceptionStackDepth
            | molt_ir::OpKind::CheckException
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

    match op.kind {
        molt_ir::OpKind::Add | molt_ir::OpKind::InplaceAdd => all_args_numeric() || all_args_str(),
        molt_ir::OpKind::Sub
        | molt_ir::OpKind::Mul
        | molt_ir::OpKind::InplaceSub
        | molt_ir::OpKind::InplaceMul => all_args_numeric(),
        molt_ir::OpKind::Neg | molt_ir::OpKind::Pos => matches!(
            first_source_kind(),
            Some(ScalarKind::Int | ScalarKind::Bool | ScalarKind::Float)
        ),
        molt_ir::OpKind::BitAnd
        | molt_ir::OpKind::BitOr
        | molt_ir::OpKind::BitXor
        | molt_ir::OpKind::BitNot
        | molt_ir::OpKind::InplaceBitAnd
        | molt_ir::OpKind::InplaceBitOr
        | molt_ir::OpKind::InplaceBitXor => {
            !args.is_empty()
                && args
                    .iter()
                    .all(|arg| facts.name_is_integer_family(arg.as_str()))
        }
        molt_ir::OpKind::Eq | molt_ir::OpKind::Ne => all_args_scalar(),
        molt_ir::OpKind::Lt | molt_ir::OpKind::Le | molt_ir::OpKind::Gt | molt_ir::OpKind::Ge => {
            all_args_numeric() || all_args_str()
        }
        _ => false,
    }
}

pub(super) fn simple_ir_op_needs_scalar_plan_for_nonthrowing(op: &OpIR) -> bool {
    matches!(
        op.kind,
        molt_ir::OpKind::Add
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
            | molt_ir::OpKind::InplaceBitAnd
            | molt_ir::OpKind::InplaceBitOr
            | molt_ir::OpKind::InplaceBitXor
            | molt_ir::OpKind::Lt
            | molt_ir::OpKind::Le
            | molt_ir::OpKind::Gt
            | molt_ir::OpKind::Ge
            | molt_ir::OpKind::Eq
            | molt_ir::OpKind::Ne
    )
}
