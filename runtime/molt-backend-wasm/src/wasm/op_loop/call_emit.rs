use super::result_sink::{store_non_none_result_or_drop, store_result_or_drop};
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{
    OpLoopRuntimeArgSpec, OpLoopRuntimeCallSpec, OpLoopRuntimeSinkSpec,
};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
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

    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[call.import_name],
    );
    emit_op_loop_runtime_sink(context, func, op, call.sink);
}

pub(super) fn emit_op_loop_local_prefix_call_id(
    context: &OpLoopRuntimeCallContext<'_>,
    func: &mut Function,
    op: &OpIR,
    import_id: u32,
    arg_count: usize,
    sink: OpLoopRuntimeSinkSpec,
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
    emit_call(func, context.reloc_enabled, import_id);
    emit_op_loop_runtime_sink(context, func, op, sink);
}

pub(super) fn emit_op_loop_runtime_sink(
    context: &OpLoopRuntimeCallContext<'_>,
    func: &mut Function,
    op: &OpIR,
    sink: OpLoopRuntimeSinkSpec,
) {
    match sink {
        OpLoopRuntimeSinkSpec::ResultOrDrop => store_result_or_drop(func, op, context.locals),
        OpLoopRuntimeSinkSpec::NonNoneResultOrDrop => {
            store_non_none_result_or_drop(func, op, context.locals)
        }
        OpLoopRuntimeSinkSpec::Drop => {
            func.instruction(&Instruction::Drop);
        }
        OpLoopRuntimeSinkSpec::None => {}
    }
}
