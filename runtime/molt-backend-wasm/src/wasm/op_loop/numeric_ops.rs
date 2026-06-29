use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, wasm_numeric_runtime_selection};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::Function;

#[path = "numeric_ops/additive_ops.rs"]
mod additive_ops;
#[path = "numeric_ops/bitwise_ops.rs"]
mod bitwise_ops;
#[path = "numeric_ops/common.rs"]
mod common;
#[path = "numeric_ops/comparison_ops.rs"]
mod comparison_ops;
#[path = "numeric_ops/division_ops.rs"]
mod division_ops;
#[path = "numeric_ops/vector_reduction_ops.rs"]
mod vector_reduction_ops;

#[allow(unused_variables)]
pub(super) fn emit_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    let Some(selection) = wasm_numeric_runtime_selection(op.kind.as_str()) else {
        return false;
    };
    match selection.op_loop_kind {
        WasmNumericOpLoopKind::Add | WasmNumericOpLoopKind::Sub | WasmNumericOpLoopKind::Mul => {
            additive_ops::emit_additive_numeric_op(
                func,
                op,
                selection,
                import_ids,
                locals,
                const_cache,
                scalar_plan,
                reloc_enabled,
                known_raw_ints,
            );
        }
        WasmNumericOpLoopKind::TrueDiv
        | WasmNumericOpLoopKind::FloorDiv
        | WasmNumericOpLoopKind::Mod
        | WasmNumericOpLoopKind::Matmul
        | WasmNumericOpLoopKind::Pow
        | WasmNumericOpLoopKind::PowMod
        | WasmNumericOpLoopKind::Round
        | WasmNumericOpLoopKind::Trunc => {
            division_ops::emit_division_numeric_op(
                func,
                op,
                selection,
                import_ids,
                locals,
                const_cache,
                scalar_plan,
                reloc_enabled,
                known_raw_ints,
            );
        }
        WasmNumericOpLoopKind::BitAnd
        | WasmNumericOpLoopKind::BitOr
        | WasmNumericOpLoopKind::BitXor
        | WasmNumericOpLoopKind::Invert
        | WasmNumericOpLoopKind::Neg
        | WasmNumericOpLoopKind::Pos
        | WasmNumericOpLoopKind::LShift
        | WasmNumericOpLoopKind::RShift => {
            bitwise_ops::emit_bitwise_numeric_op(
                func,
                op,
                selection,
                import_ids,
                locals,
                const_cache,
                scalar_plan,
                reloc_enabled,
                known_raw_ints,
            );
        }
        WasmNumericOpLoopKind::Lt
        | WasmNumericOpLoopKind::Le
        | WasmNumericOpLoopKind::Gt
        | WasmNumericOpLoopKind::Ge
        | WasmNumericOpLoopKind::Eq
        | WasmNumericOpLoopKind::Ne
        | WasmNumericOpLoopKind::StringEq => {
            comparison_ops::emit_comparison_numeric_op(
                func,
                op,
                selection,
                import_ids,
                locals,
                const_cache,
                scalar_plan,
                reloc_enabled,
                known_raw_ints,
            );
        }
        WasmNumericOpLoopKind::VectorReduction => {
            vector_reduction_ops::emit_vector_reduction_numeric_op(
                func,
                op,
                selection,
                import_ids,
                locals,
                const_cache,
                scalar_plan,
                reloc_enabled,
                known_raw_ints,
            );
        }
    }
    true
}
