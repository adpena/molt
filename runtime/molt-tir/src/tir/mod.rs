pub mod analysis;
pub mod blocks;
pub mod bolt;
pub mod cache;
pub mod call_facts;
pub mod call_graph;
pub mod cfg;
pub mod deopt;
pub mod dominators;
pub mod drop_phase;
pub mod effect_proof;
pub mod exception_regions;
pub mod function;
pub mod lir;
pub mod lir_printer;
pub mod lower_from_simple;
pub mod lower_to_lir;
pub mod lower_to_simple;
pub mod lower_to_wasm;
pub mod mlir_compat;
pub mod module_phase;
pub mod op_kinds_generated;
pub mod ops;
pub mod parallel;
pub mod pass_delta;
pub mod pass_manager;
pub mod passes;
pub mod printer;
pub mod serialize;
pub mod ssa;
pub mod target_info;
pub mod tests_roundtrip;
pub mod type_refine;
pub mod types;
pub mod values;
pub mod verify;
pub mod verify_lir;
pub mod verify_lir_repr;
pub mod wasm_component;
pub mod wasm_split;
pub mod wasm_streaming;

/// Returns true for SimpleIR ops that are purely structural control-flow
/// markers (if/else/end_if/loop_start/loop_end/label/state_label) and should
/// be skipped during SSA conversion and type hint correlation.
///
/// Exception-handling ops (check_exception, try_start, try_end,
/// state_block_start, state_block_end) are NOT structural — they carry
/// semantics that must be preserved as TirOps through SSA conversion and
/// the round-trip pipeline.  Classifying them as structural causes SSA to
/// silently drop them, breaking exception handling and round-trip tests.
///
/// Shared between `ssa.rs` and `lower_from_simple.rs` to ensure identical
/// classification — divergence would silently misalign SSA ops with original ops.
pub(crate) fn is_structural(kind: &str) -> bool {
    matches!(
        kind,
        "label"
            | "state_label"
            | "if"
            | "else"
            | "end_if"
            | "loop_start"
            | "loop_end"
            | "loop_break"
            | "loop_continue"
            | "jump"
            | "goto"
            | "br_if"
            | "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break_if_exception"
            | "ret"
            | "ret_void"
            | "return"
            | "nop"
            // `state_switch` is the `_poll` dispatch terminator: it ends its
            // block and lowers to a `StateDispatch` terminator (it must NOT also
            // appear as a body op, or the dispatch would be emitted twice).
            | "state_switch"
    )
}

// Re-export primary types for convenience.
pub use self::blocks::{BlockId, Terminator, TirBlock};
pub use self::call_facts::{
    CallFacts, CallFactsAnalysis, CallFactsTable, CallTargetFact, FactValue, InlineEligibility,
    InlineWhyNot,
};
pub use self::call_graph::{CallEdge, CallGraph};
pub use self::exception_regions::{
    ExceptionMatchRefFact, ExceptionOpPosition, ExceptionRegionDiagnostic,
    ExceptionRegionDiagnosticKind, ExceptionRegionFacts, ExceptionRegions,
    verify_exception_regions,
};
pub use self::function::{TirFunction, TirModule};
pub use self::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
pub use self::module_phase::{ModuleAnalysis, run_module_pipeline};
pub use self::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub use self::target_info::{BuildProfile, ProfileData, SimdCaps, TargetInfo, TargetKind};
pub use self::types::{FuncSignature, TirType};
pub use self::values::{TirValue, ValueId};
