use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_binary::emit_simple_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::Function;

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
    let arg_locals: Vec<u32> = args_names.iter().map(|n| locals[n]).collect();
    let out = locals[op.out.as_ref().unwrap()];
    emit_simple_call(
        func,
        reloc_enabled,
        import_ids[selection.import],
        &arg_locals,
        out,
    );
}
