use super::super::super::result_sink::store_runtime_result;
use crate::wasm::method_ic_select::selected_method_ic_runtime;
use crate::wasm::op_loop::call_ops::custody::{call_adopts_arguments, push_spilled_arguments};
use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{box_int, stable_ic_site_id};
use crate::{FunctionIR, OpIR};
use wasm_encoder::{Function, Instruction};

/// The owned method-IC entry for an instruction that adopted its receiver (or
/// `self`) and its arguments: the call's leading operands are already pushed,
/// then the spilled arguments from operand `first_arg` on. The runtime moves
/// them into the resolved method's frame or releases them; this frame never
/// sees them again.
#[allow(clippy::too_many_arguments)]
fn emit_owned_method_ic_tail(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    spill_base: u32,
    first_arg: usize,
    import: WasmRuntimeImport,
) {
    let args_names = op.args.as_ref().unwrap();
    push_spilled_arguments(func, locals, spill_base, &args_names[first_arg..]);
    emit_call(func, reloc_enabled, import_ids[import]);
    store_runtime_result(func, op, locals, import_ids, reloc_enabled, import);
}

pub(super) fn emit_method_inline_cache_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
    spill_base: u32,
) -> bool {
    match op.kind.as_str() {
        "call_method_ic" => {
            // Fused instance-method dispatch: LOAD_METHOD/CALL_METHOD without
            // bound-method or callargs allocation on the IC fast path.
            let args_names = op.args.as_ref().unwrap();
            let recv = locals[&args_names[0]];
            let method_name = op
                .s_value
                .as_ref()
                .expect("call_method_ic missing method name");
            let bytes = method_name.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                "call_method_ic",
            ));
            if call_adopts_arguments(op) {
                // The instruction adopted its receiver and arguments.
                func.instruction(&Instruction::I64Const(site_bits));
                func.instruction(&Instruction::LocalGet(recv));
                backend.emit_data_ptr(reloc_enabled, func_index, func, data);
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::I64Const(bytes.len() as i64));
                emit_owned_method_ic_tail(
                    func,
                    op,
                    import_ids,
                    locals,
                    reloc_enabled,
                    spill_base,
                    1,
                    WasmRuntimeImport::CallMethodIcOwned,
                );
                return true;
            }
            let selected =
                selected_method_ic_runtime(op).expect("call_method_ic selector must exist");
            let extra = &args_names[selected.extra_arg_start..];

            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(recv));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            for name in extra {
                func.instruction(&Instruction::LocalGet(locals[name]));
            }
            emit_call(func, reloc_enabled, import_ids[selected.import]);
            store_runtime_result(func, op, locals, import_ids, reloc_enabled, selected.import);
        }
        "call_super_method_ic" => {
            // Fused super().method() dispatch without super, bound-method, or
            // callargs allocation on the fast path.
            let args_names = op.args.as_ref().unwrap();
            let class = locals[&args_names[0]];
            let self_local = locals[&args_names[1]];
            let method_name = op
                .s_value
                .as_ref()
                .expect("call_super_method_ic missing method name");
            let bytes = method_name.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                "call_super_method_ic",
            ));
            if call_adopts_arguments(op) {
                // The instruction adopted `self` and its arguments; the class
                // stays borrowed.
                func.instruction(&Instruction::I64Const(site_bits));
                func.instruction(&Instruction::LocalGet(class));
                func.instruction(&Instruction::LocalGet(self_local));
                backend.emit_data_ptr(reloc_enabled, func_index, func, data);
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::I64Const(bytes.len() as i64));
                emit_owned_method_ic_tail(
                    func,
                    op,
                    import_ids,
                    locals,
                    reloc_enabled,
                    spill_base,
                    2,
                    WasmRuntimeImport::CallSuperMethodIcOwned,
                );
                return true;
            }
            let selected =
                selected_method_ic_runtime(op).expect("call_super_method_ic selector must exist");
            let extra = &args_names[selected.extra_arg_start..];

            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(class));
            func.instruction(&Instruction::LocalGet(self_local));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            for name in extra {
                func.instruction(&Instruction::LocalGet(locals[name]));
            }
            emit_call(func, reloc_enabled, import_ids[selected.import]);
            store_runtime_result(func, op, locals, import_ids, reloc_enabled, selected.import);
        }
        _ => return false,
    }
    true
}
