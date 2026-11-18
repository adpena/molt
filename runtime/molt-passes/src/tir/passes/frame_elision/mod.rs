//! A body no observer can reach links no frame record (call_anatomy.md 4.3,
//! "Which functions need no record at all"; design 78, section 5.5).
//!
//! A synchronous Python function links a frame record on entry and pops it at
//! every exit (`trace_enter_slot`, `trace_exit`), keeps its locals in the
//! record's homes (`frame_home_store`) and publishes its source location
//! (`line`, `frame_set_location`). Only an observer reads the record:
//!
//! * a traceback, built while an exception passes through the frame;
//! * frame introspection (`sys._getframe`, `f_back`, `inspect`, `locals()`),
//!   reachable only from code the body runs: a call, a finalizer a release
//!   runs, or a signal handler an asynchronous-work poll runs;
//! * a suspension, which keeps the frame alive.
//!
//! A body whose every operation cannot raise and calls no Python code, with no
//! poll, handler region or suspension, has none of these, so its record is
//! never read: this pass deletes the record, the homes and the locations, and
//! the body leaves the frame protocol (`ExecutionContextPolicy::None`). Its
//! `CheckException` operations can never see a pending exception and are
//! deleted with it. The recursion charge stays with the call site, as the
//! native backend already omits it for a callee that makes no call.
//!
//! A home holds a reference to its binding until the frame clears. A body
//! whose homes hold only exact scalars releases nothing a finalizer could
//! observe, so dropping the homes moves no Python-visible release; any other
//! home keeps the record.

use crate::tir::blocks::BlockId;
use crate::tir::dominators::{CfgEdgePolicy, reachable_blocks_with};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    SimpleIrRuntimeRequirements, simpleir_runtime_requirements_table,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::{ExecutionContextPolicy, OpKind};

/// What one frame operation becomes when the record goes.
enum FrameOp {
    /// The record's link, its pop, and source-location publications.
    Drop,
    /// A home store: its result is the stored value.
    Home,
}

fn frame_op(op: &TirOp) -> Option<FrameOp> {
    if op.opcode == OpCode::FrameSetLocation {
        return Some(FrameOp::Drop);
    }
    if op.opcode != OpCode::Copy {
        return None;
    }
    match op.origin? {
        OpKind::TraceEnterSlot | OpKind::TraceExit | OpKind::Line => Some(FrameOp::Drop),
        OpKind::FrameHomeStore => Some(FrameOp::Home),
        _ => None,
    }
}

/// Why a body keeps its frame record, or `None` when no observer can reach
/// it. `exact` holds the body's exact scalar facts.
fn observer(
    func: &TirFunction,
    reachable: &HashSet<BlockId>,
    exact: &HashMap<ValueId, TirType>,
) -> Option<&'static str> {
    if func.execution_context != ExecutionContextPolicy::Local {
        return Some("the body links no record of its own");
    }
    if func.has_state_machine() {
        return Some("a suspension keeps the frame alive");
    }
    if func.has_exception_handlers() {
        return Some("a handler region");
    }
    if func.is_codegen_partition() {
        return Some("a codegen partition lends its frame");
    }
    let mut blocks: Vec<BlockId> = reachable.iter().copied().collect();
    blocks.sort_unstable();
    for block in blocks {
        let body = &func.blocks[&block];
        for op in &body.ops {
            match frame_op(op) {
                Some(FrameOp::Drop) => continue,
                Some(FrameOp::Home) => {
                    if op.operands.len() != 1
                        || op.results.len() != 1
                        || !exact.contains_key(&op.operands[0])
                    {
                        return Some("a home holds a value a finalizer could observe");
                    }
                    continue;
                }
                None => {}
            }
            if let Some(reason) = op_observer(op, exact) {
                return Some(reason);
            }
        }
        if let Some(reason) = terminator_observer(&body.terminator, exact) {
            return Some(reason);
        }
    }
    None
}

/// Why an observer of the frame can run at `op`, or `None` when none can:
/// the per-operation half of [`frame_observer`]. `exact` holds the body's
/// exact scalar facts. A non-poll `CheckException` observes nothing itself:
/// the operation that raised was the observation point. Frame home and
/// location operations are not classified here; their callers own them.
pub fn op_observer(op: &TirOp, exact: &HashMap<ValueId, TirType>) -> Option<&'static str> {
    if op.opcode == OpCode::CheckException {
        return op
            .is_async_work_poll()
            .then_some("an asynchronous-work poll can run a handler");
    }
    if op.origin.is_some_and(|kind| {
        simpleir_runtime_requirements_table(kind).is_none_or(|requirements| {
            requirements.contains(SimpleIrRuntimeRequirements::EXECUTION_FRAME)
                || requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION)
        })
    }) {
        return Some("an operation reads the frame");
    }
    let effects = super::effects::op_effects_with_types(op, exact);
    if !effects.nothrow {
        return Some("an operation may raise");
    }
    if effects.may_call_python {
        return Some("an operation may run Python code");
    }
    None
}

/// Why an observer of the frame can run at a block's terminator: a branch
/// that tests a value's truth through Python.
pub fn terminator_observer(
    terminator: &crate::tir::blocks::Terminator,
    exact: &HashMap<ValueId, TirType>,
) -> Option<&'static str> {
    let effects = crate::tir::op_semantics::terminator_effects(terminator, exact);
    (!effects.nothrow || effects.may_call_python)
        .then_some("a branch tests a value's truth through Python")
}

/// The one authority for "no observer can reach this body's frame": why
/// `func` keeps its frame record, or `None` when its frame is unobservable.
/// Frame elision, and any transform that removes or merges a frame (for
/// example generator fusion), asks this predicate; none keeps its own copy.
pub fn frame_observer(func: &TirFunction) -> Option<&'static str> {
    let reachable = reachable_blocks_with(func, CfgEdgePolicy::TerminatorOnly);
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    observer(func, &reachable, &exact)
}

/// Delete `func`'s frame record when no observer can reach it; return
/// whether it did.
pub fn run(func: &mut TirFunction) -> bool {
    if frame_observer(func).is_some() {
        return false;
    }
    for block in func.blocks.values_mut() {
        block.ops.retain_mut(|op| {
            if op.opcode == OpCode::CheckException {
                return false;
            }
            match frame_op(op) {
                None => true,
                Some(FrameOp::Drop) => false,
                Some(FrameOp::Home) => {
                    *op = TirOp {
                        source_span: op.source_span,
                        ..TirOp::new(OpCode::Copy, op.operands.clone(), op.results.clone())
                    };
                    true
                }
            }
        });
    }
    // Blocks only an exception edge reached lose their last predecessor; the
    // re-optimization that follows every module transform deletes them.
    func.execution_context = ExecutionContextPolicy::None;
    func.has_exception_handling = false;
    true
}

/// Delete the frame record of every body in `module` no observer can reach,
/// and re-optimize each; return their ids in row order.
pub fn run_module(
    module: &mut crate::tir::function::TirModule,
    tti: &crate::tir::target_info::TargetInfo,
) -> Vec<molt_ir::FunctionId> {
    let mut elided = Vec::new();
    for (func, &id) in module.functions.iter_mut().zip(&module.ids) {
        if run(func) {
            let _ = super::reoptimize(func, tti);
            elided.push(id);
        }
    }
    elided.sort_unstable();
    elided
}

#[cfg(test)]
pub(crate) mod tests;
