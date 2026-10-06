//! Call-instruction custody on the WASM lane (`argument_custody`).
//!
//! A source call instruction owns one reference to each operand it adopted,
//! on both of its continuations. The runtime's owned entries move those
//! references into an adopting frame or release them as CPython's CALL
//! cleanup does; the helpers here cover the paths where compiled code itself
//! must hand them back: after a borrowing builtin returns, after a direct leg
//! returns (the callable), and on a failure before invocation.

use super::site::spill_call_args;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

/// Whether the call instruction adopted any operand. It then owns one
/// reference to each adopted operand on both of its continuations, and its
/// direct leg may enter only an adopting entry.
pub(in crate::wasm::op_loop) fn call_adopts_arguments(op: &OpIR) -> bool {
    op.argument_custody
        .as_deref()
        .is_some_and(|custody| custody.contains(&molt_ir::ParameterCustody::Transferred))
}

/// Whether the instruction adopted operand `index`.
pub(in crate::wasm::op_loop) fn operand_adopted(op: &OpIR, index: usize) -> bool {
    op.argument_custody
        .as_deref()
        .is_some_and(|custody| custody.get(index) == Some(&molt_ir::ParameterCustody::Transferred))
}

/// Spill `names` to the shared call spill region and push `(args_ptr, nargs)`
/// for an owned runtime entry. The stores leave the operand stack unchanged,
/// so earlier call operands may already be pushed. Every owned entry copies
/// the region before it can run Python code that reuses it.
pub(in crate::wasm::op_loop) fn push_spilled_arguments(
    func: &mut Function,
    locals: &WasmFrameLocals,
    spill_base: u32,
    names: &[String],
) {
    spill_call_args(func, locals, spill_base, names);
    func.instruction(&Instruction::I64Const(spill_base as i64));
    func.instruction(&Instruction::I64Const(names.len() as i64));
}

/// Release a call instruction's adopted inputs that no callee took over: once
/// a borrowing callee returns, or on a failure before invocation. The runtime
/// owns CALL's release order (`call_inputs_release`, CPython's
/// `DECREF_INPUTS`): the adopted arguments from operand `first_arg` on, in the
/// target version's order, then the adopted callable (operand 0) when the
/// instruction has one.
#[allow(clippy::too_many_arguments)]
pub(in crate::wasm::op_loop) fn release_adopted_call_inputs(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    locals: &WasmFrameLocals,
    spill_base: u32,
    op: &OpIR,
    has_callable: bool,
    first_arg: usize,
) {
    if !call_adopts_arguments(op) {
        return;
    }
    let args = op.args.as_deref().unwrap_or(&[]);
    let adopted: Vec<String> = args
        .iter()
        .enumerate()
        .skip(first_arg)
        .filter(|&(index, _)| operand_adopted(op, index))
        .map(|(_, name)| name.clone())
        .collect();
    if has_callable && operand_adopted(op, 0) {
        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
    } else {
        func.instruction(&Instruction::I64Const(0));
    }
    push_spilled_arguments(func, locals, spill_base, &adopted);
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::CallInputsRelease],
    );
}

/// Release an adopted callable once its direct leg has returned: the callee
/// frame has already ended its parameters, and CALL releases its callable
/// last, as CPython's frame clear releases the function after the locals.
pub(in crate::wasm::op_loop) fn release_adopted_callable(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    locals: &WasmFrameLocals,
    op: &OpIR,
) {
    if !operand_adopted(op, 0) {
        return;
    }
    let args = op.args.as_deref().unwrap_or(&[]);
    func.instruction(&Instruction::LocalGet(locals[&args[0]]));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::DecRefObj],
    );
}
