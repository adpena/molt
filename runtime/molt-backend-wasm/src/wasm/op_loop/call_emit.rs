use super::result_sink::{discard_runtime_result, store_runtime_result};
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{OpLoopRuntimeArgSpec, OpLoopRuntimeCallSpec, WasmRuntimeImport};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::{TrackedImportIds, selected_import_id};
use wasm_encoder::{Function, Instruction};

#[derive(Clone, Copy)]
pub(super) struct OpLoopRuntimeCallContext<'a> {
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) reloc_enabled: bool,
}

pub(super) fn emit_op_loop_runtime_call(
    context: &OpLoopRuntimeCallContext<'_>,
    func: &mut Function,
    op: &OpIR,
    call: OpLoopRuntimeCallSpec,
) {
    for arg in call.args {
        match *arg {
            OpLoopRuntimeArgSpec::Local(index) => {
                let args = op
                    .args
                    .as_ref()
                    .unwrap_or_else(|| panic!("{} missing op-loop runtime args", op.kind));
                let name = args.get(index).unwrap_or_else(|| {
                    panic!(
                        "{} missing op-loop runtime arg {index}; only {} args present",
                        op.kind,
                        args.len()
                    )
                });
                func.instruction(&Instruction::LocalGet(context.locals[name]));
            }
            OpLoopRuntimeArgSpec::OpValueI64(message) => {
                func.instruction(&Instruction::I64Const(op.value.expect(message)));
            }
        }
    }

    emit_call(func, context.reloc_enabled, context.import_ids[call.import]);
    if call.discard_result {
        discard_runtime_result(func, context.import_ids, context.reloc_enabled, call.import);
    } else {
        store_runtime_result(
            func,
            op,
            context.locals,
            context.import_ids,
            context.reloc_enabled,
            call.import,
        );
    }
}

pub(super) fn emit_op_loop_local_prefix_call(
    context: &OpLoopRuntimeCallContext<'_>,
    func: &mut Function,
    op: &OpIR,
    import: WasmRuntimeImport,
    arg_count: usize,
    function_name: &str,
) {
    let args = op.args.as_ref().unwrap_or_else(|| {
        panic!(
            "wasm runtime op '{}' expected {arg_count} args, got none",
            op.kind
        )
    });
    assert!(
        args.len() >= arg_count,
        "wasm runtime op '{}' expected at least {arg_count} args, got {}",
        op.kind,
        args.len()
    );
    for arg in &args[..arg_count] {
        func.instruction(&Instruction::LocalGet(context.locals[arg]));
    }
    let import_id = selected_import_id(context.import_ids, import, function_name, &op.kind);
    emit_call(func, context.reloc_enabled, import_id);
    store_runtime_result(
        func,
        op,
        context.locals,
        context.import_ids,
        context.reloc_enabled,
        import,
    );
}
