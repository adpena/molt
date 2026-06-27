pub mod blocks;
pub mod call_targets;
pub mod cfg;
pub mod dominators;
pub mod effect_proof;
pub mod function;
pub mod op_kinds_generated;
pub mod ops;
pub mod printer;
pub mod serialize;
pub mod ssa;
pub mod types;
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
pub use self::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub use self::types::{FuncSignature, TirType};
pub use self::values::{TirValue, ValueId};
