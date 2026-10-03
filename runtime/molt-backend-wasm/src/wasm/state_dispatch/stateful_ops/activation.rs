//! Non-terminating scheduler primitives. Shared TIR owns branches and returns.

use super::super::common::emit_obj_set_state_arg;
use super::super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use crate::OpIR;
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_values::{box_pending, emit_box_bool_from_i32};
use wasm_encoder::{Function, Instruction};

pub(in crate::wasm::state_dispatch) fn emit_activation_op(
    func: &mut Function,
    context: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
) {
    match op.kind.as_str() {
        "state_set" => {
            let state = op.value.expect("state_set requires its resume state");
            let encoded = plan
                .state_resume
                .as_ref()
                .and_then(|resume| resume.state_map.get(&state))
                .map_or(state, |&target| !(target as i64));
            emit_obj_set_state_arg(func, locals);
            func.instruction(&Instruction::I64Const(encoded));
            emit_call(
                func,
                context.reloc_enabled,
                context.import_ids[WasmRuntimeImport::ObjSetState],
            );
        }
        "is_pending" => {
            let args = op.args.as_ref().expect("is_pending requires an operand");
            func.instruction(&Instruction::LocalGet(context.locals()[&args[0]]));
            func.instruction(&Instruction::I64Const(box_pending()));
            func.instruction(&Instruction::I64Eq);
            emit_box_bool_from_i32(func);
            func.instruction(&Instruction::LocalSet(
                context.locals()[op.out.as_ref().unwrap()],
            ));
        }
        "task_wait" => {
            let args = op.args.as_ref().expect("task_wait requires a future");
            func.instruction(&Instruction::LocalGet(
                locals
                    .self_ptr_local
                    .expect("task_wait requires an activation frame"),
            ));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(context.locals()[&args[0]]));
            emit_call(
                func,
                context.reloc_enabled,
                context.import_ids[WasmRuntimeImport::HandleResolve],
            );
            emit_call(
                func,
                context.reloc_enabled,
                context.import_ids[WasmRuntimeImport::SleepRegister],
            );
            func.instruction(&Instruction::Drop);
        }
        _ => unreachable!("non-activation operation"),
    }
}
