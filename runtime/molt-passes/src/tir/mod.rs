pub mod analysis;
pub mod bolt;
pub mod cache;
pub mod call_facts;
pub mod call_graph;
pub mod deopt;
pub mod drop_phase;
pub mod exception_regions;
pub mod fact_graph;
pub mod lower_from_simple;
pub mod lower_to_simple;
pub mod module_phase;
pub mod numeric_facts;
pub mod parallel;
pub mod pass_delta;
pub mod pass_manager;
pub mod passes;
pub mod simple_value_names;
pub mod target_info;
pub mod type_refine;

pub use molt_ir::tir::{
    blocks, call_targets, cfg, dominators, effect_proof, function, op_kinds_generated, ops,
    printer, serialize, ssa, types, values, verify,
};

/// Pass-layer access to the canonical SimpleIR structural classifier.
pub(crate) fn is_structural(kind: &str) -> bool {
    op_kinds_generated::simpleir_kind_is_structural(kind)
}

// Re-export primary IR and pass-layer types for convenience.
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
pub use self::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub use self::target_info::{BuildProfile, ProfileData, SimdCaps, TargetInfo, TargetKind};
pub use self::types::{FuncSignature, TirType};
pub use self::values::{TirValue, ValueId};
