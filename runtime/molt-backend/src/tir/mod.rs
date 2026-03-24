pub mod cache;
pub mod types;
pub mod values;
pub mod ops;
pub mod blocks;
pub mod cfg;
pub mod function;
pub mod ssa;
pub mod lower_from_simple;
pub mod printer;
pub mod verify;
pub mod lower_to_simple;
pub mod lower_to_wasm;
pub mod type_refine;
pub mod passes;
pub mod parallel;
pub mod gpu;
pub mod gpu_msl;
pub mod gpu_wgsl;
pub mod gpu_cuda;
pub mod gpu_hip;
pub mod mlir_compat;
pub mod deopt;
pub mod gpu_runtime;
pub mod gpu_metal;
pub mod gpu_webgpu;
pub mod gpu_dispatch;
pub mod gpu_cuda_runtime;
pub mod serialize;
pub mod wasm_component;
pub mod wasm_split;
pub mod wasm_streaming;
pub mod mlir_bridge;
pub mod gpu_mlx;
pub mod bolt;
pub mod tests_roundtrip;

/// Returns true for SimpleIR ops that are purely structural control-flow
/// markers (if/else/end_if/loop_start/loop_end/label/state_label) and should
/// be skipped during SSA conversion and type hint correlation.
///
/// Shared between `ssa.rs` and `lower_from_simple.rs` to ensure identical
/// classification — divergence would silently misalign SSA ops with original ops.
pub(crate) fn is_structural(kind: &str) -> bool {
    matches!(
        kind,
        "label" | "state_label" | "if" | "else" | "end_if"
            | "loop_start" | "loop_end" | "loop_break" | "loop_continue"
            | "jump" | "goto" | "br_if"
            | "loop_break_if_true" | "loop_break_if_false"
            | "ret" | "ret_void" | "return"
            | "nop"
    )
}

// Re-export primary types for convenience.
pub use self::blocks::{BlockId, TirBlock, Terminator};
pub use self::function::{TirFunction, TirModule};
pub use self::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
pub use self::types::{FuncSignature, TirType};
pub use self::values::{TirValue, ValueId};
