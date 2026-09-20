use super::super::result_sink::store_runtime_result;
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::{Function, Instruction};

#[allow(unused_variables)]
pub(super) fn emit_vector_reduction_numeric_op(
    func: &mut Function,
    op: &OpIR,
    selection: WasmNumericRuntimeSelection,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    debug_assert_eq!(
        selection.op_loop_kind,
        WasmNumericOpLoopKind::VectorReduction
    );
    let args_names = op.args.as_ref().unwrap();
    for name in args_names {
        func.instruction(&Instruction::LocalGet(locals[name]));
    }
    emit_call(func, reloc_enabled, import_ids[selection.import]);
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        selection.import,
    );
}
