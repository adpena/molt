//! The one SimpleIR optimization pipeline every target runs before the TIR lift.
//!
//! The order is data in [`SIMPLE_IR_FUNCTION_PASSES`]; no backend lists passes.
//! A pass runs on a target when the target supports the generated runtime
//! requirements of every op kind the pass can introduce and implements the
//! lowering feature the pass relies on, if any. Function passes read and write
//! only their own [`FunctionIR`], so functions run in parallel and the result
//! does not depend on scheduling. See
//! `docs/design/foundation/76_simple_ir_pass_pipeline.md`.

use rayon::prelude::*;

use crate::tir::op_kinds_generated::{
    SimpleIrRuntimeRequirements, simpleir_runtime_requirements_table,
};
use crate::tir::target_info::TargetInfo;
use crate::{FunctionIR, SimpleIR};

/// A backend lowering feature a pass's output relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleIrLowering {
    /// The backend scalarizes tuples escape analysis marks `stack_eligible`.
    NonEscapingTupleScalarization,
}

/// One function-level SimpleIR pass and what it needs from a target.
#[derive(Debug, Clone, Copy)]
pub struct SimpleIrPass {
    /// Stable pass name, used by diagnostics and tests.
    pub name: &'static str,
    /// Every op kind the pass can introduce.
    pub introduces: &'static [molt_ir::OpKind],
    /// The lowering feature the pass's output relies on, if any.
    pub lowering: Option<SimpleIrLowering>,
    /// The pass itself.
    pub run: fn(&mut FunctionIR),
}

impl SimpleIrPass {
    /// Whether the pass runs for `target`.
    pub fn applies_to(&self, target: &TargetInfo) -> bool {
        let lowered = match self.lowering {
            None => true,
            Some(SimpleIrLowering::NonEscapingTupleScalarization) => {
                target.lowering.non_escaping_tuple_scalarization
            }
        };
        lowered
            && self.introduced_requirements().is_some_and(|required| {
                required
                    .difference(target.supported_runtime_semantics)
                    .is_empty()
            })
    }

    /// The generated runtime requirements of every op kind the pass can
    /// introduce; `None` when a kind is unclassified, so the pass never runs.
    pub fn introduced_requirements(&self) -> Option<SimpleIrRuntimeRequirements> {
        self.introduces
            .iter()
            .try_fold(SimpleIrRuntimeRequirements::NONE, |required, kind| {
                Some(required.union(simpleir_runtime_requirements_table(*kind)?))
            })
    }
}

/// The function stage, in order.
pub const SIMPLE_IR_FUNCTION_PASSES: &[SimpleIrPass] = &[
    SimpleIrPass {
        name: "rewrite_stateful_loops",
        introduces: &[
            molt_ir::OpKind::EndIf,
            molt_ir::OpKind::If,
            molt_ir::OpKind::Jump,
            molt_ir::OpKind::Label,
            molt_ir::OpKind::LoopBreak,
            molt_ir::OpKind::LoopBreakIfException,
            molt_ir::OpKind::LoopBreakIfFalse,
            molt_ir::OpKind::LoopBreakIfTrue,
            molt_ir::OpKind::LoopContinue,
            molt_ir::OpKind::LoopEnd,
            molt_ir::OpKind::LoopIndexStart,
            molt_ir::OpKind::LoopStart,
            molt_ir::OpKind::Not,
            molt_ir::OpKind::StateLabel,
            molt_ir::OpKind::StateSwitch,
            molt_ir::OpKind::StateTransition,
            molt_ir::OpKind::StateYield,
        ],
        lowering: None,
        run: super::rewrite_stateful_loops,
    },
    SimpleIrPass {
        name: "eliminate_unbound_local_checks",
        introduces: &[],
        lowering: None,
        run: super::eliminate_unbound_local_checks,
    },
    SimpleIrPass {
        name: "eliminate_redundant_guard_tags",
        introduces: &[],
        lowering: None,
        run: super::eliminate_redundant_guard_tags,
    },
    SimpleIrPass {
        name: "elide_dead_struct_allocs",
        introduces: &[],
        lowering: None,
        run: super::elide_dead_struct_allocs,
    },
    SimpleIrPass {
        name: "escape_analysis",
        introduces: &[],
        lowering: Some(SimpleIrLowering::NonEscapingTupleScalarization),
        run: super::escape_analysis,
    },
    SimpleIrPass {
        name: "rc_coalescing",
        introduces: &[],
        lowering: None,
        run: super::rc_coalescing,
    },
    SimpleIrPass {
        name: "fold_constants",
        introduces: &[molt_ir::OpKind::Const, molt_ir::OpKind::ConstBool],
        lowering: None,
        run: |function| super::fold_constants(&mut function.ops),
    },
    SimpleIrPass {
        name: "fold_constants_cross_block",
        introduces: &[molt_ir::OpKind::Const, molt_ir::OpKind::ConstBool],
        lowering: None,
        run: |function| super::fold_constants_cross_block(&mut function.ops),
    },
    SimpleIrPass {
        name: "elide_safe_exception_checks",
        introduces: &[],
        lowering: None,
        run: super::elide_safe_exception_checks,
    },
];

/// Run the module stage, then every applicable function pass over each
/// non-extern function in parallel.
pub fn run_simple_ir_pipeline(ir: &mut SimpleIR, target: &TargetInfo) {
    super::apply_profile_order(ir);
    let passes: Vec<fn(&mut FunctionIR)> = SIMPLE_IR_FUNCTION_PASSES
        .iter()
        .filter(|pass| pass.applies_to(target))
        .map(|pass| pass.run)
        .collect();
    // The request's pool honors its CPU limit and keeps concurrent compiles
    // within one shared set of threads.
    let request = molt_ir::backend_environment::carry();
    crate::tir::pipeline_cache::request_thread_pool().install(|| {
        ir.functions
            .par_iter_mut()
            .filter(|function| !function.is_extern)
            .for_each(|function| {
                request.enter(|| {
                    for run in &passes {
                        run(function);
                    }
                })
            })
    });
}
