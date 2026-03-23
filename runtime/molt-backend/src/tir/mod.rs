pub mod types;
pub mod values;
pub mod ops;
pub mod blocks;
pub mod cfg;
pub mod function;

// Re-export primary types for convenience.
pub use self::blocks::{BlockId, TirBlock, Terminator};
pub use self::function::{TirFunction, TirModule};
pub use self::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub use self::types::{FuncSignature, TirType};
pub use self::values::{TirValue, ValueId};
