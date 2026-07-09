pub mod blocks;
pub mod call_targets;
pub mod cfg;
pub mod dominators;
pub mod effect_proof;
pub mod function;
pub mod numeric_facts;
pub mod op_kinds_generated;
pub mod ops;
pub mod printer;
pub mod serialize;
pub mod ssa;
pub mod target_info;
pub mod types;
pub mod value_range;
pub mod values;
pub mod verify;

/// Returns true for SimpleIR ops that are purely structural control-flow
/// markers and should be skipped during SSA conversion and type hint
/// correlation.
pub(crate) fn is_structural(kind: &str) -> bool {
    op_kinds_generated::simpleir_kind_is_structural(kind)
}

// Re-export primary types for convenience.
pub use self::blocks::{BlockId, Terminator, TirBlock};
pub use self::function::{TirFunction, TirModule};
pub use self::numeric_facts::{IntRange, ScevExpr, TripCount};
pub use self::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub use self::target_info::{BuildProfile, ProfileData, SimdCaps, TargetInfo, TargetKind};
pub use self::types::{FuncSignature, TirType};
pub use self::value_range::ValueRangeResult;
pub use self::values::{TirValue, ValueId};
