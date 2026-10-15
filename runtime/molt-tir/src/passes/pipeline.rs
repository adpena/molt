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
    pub introduces: &'static [&'static str],
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
                Some(required.union(simpleir_runtime_requirements_table(kind)?))
            })
    }
}

/// The function stage, in order.
pub const SIMPLE_IR_FUNCTION_PASSES: &[SimpleIrPass] = &[
    SimpleIrPass {
        name: "rewrite_stateful_loops",
        introduces: &[
            "end_if",
            "if",
            "jump",
            "label",
            "loop_break",
            "loop_break_if_exception",
            "loop_break_if_false",
            "loop_break_if_true",
            "loop_continue",
            "loop_end",
            "loop_index_start",
            "loop_start",
            "not",
            "state_label",
            "state_switch",
            "state_transition",
            "state_yield",
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
        introduces: &["const", "const_bool"],
        lowering: None,
        run: |function| super::fold_constants(&mut function.ops),
    },
    SimpleIrPass {
        name: "fold_constants_cross_block",
        introduces: &["const", "const_bool"],
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
    ir.functions
        .par_iter_mut()
        .filter(|function| !function.is_extern)
        .for_each(|function| {
            for run in &passes {
                run(function);
            }
        });
}
