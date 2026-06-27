pub mod lir;
pub mod lir_printer;
pub mod lower_to_lir;
pub mod mlir_compat;
pub mod pipeline_cache;
pub mod tests_roundtrip;
pub mod verify_lir;
pub mod verify_lir_repr;
pub use molt_ir::tir::{
    blocks, call_targets, cfg, dominators, effect_proof, function, op_kinds_generated, ops,
    printer, serialize, ssa, types, values, verify,
};
pub use molt_passes::tir::{
    analysis, bolt, cache, call_facts, call_graph, drop_phase, exception_regions, fact_graph,
    lower_from_simple, lower_to_simple, module_phase, numeric_facts, parallel, pass_delta,
    pass_manager, passes, simple_value_names, target_info, type_refine,
};

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
pub use self::fact_graph::{
    FACT_GRAPH_KIND, FACT_GRAPH_SCHEMA_VERSION, FactConsumer, FactEdge, FactEntry, FactGraph,
    FactGraphSummary, FactProducer, FactSourceSite, ValueFactNode,
};
pub use self::function::{TirFunction, TirModule};
pub use self::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
pub use self::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub use self::target_info::{BuildProfile, ProfileData, SimdCaps, TargetInfo, TargetKind};
pub use self::types::{FuncSignature, TirType};
pub use self::values::{TirValue, ValueId};
pub use module_phase::{ModuleAnalysis, run_module_pipeline};
