use super::plain;
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use crate::wasm::context::CompileFuncContext;
use crate::wasm::function_frame::{WasmFrameControlMode, WasmFunctionFramePlan};
use crate::wasm::op_loop::WasmFunctionEmitContext;
use crate::wasm::state_dispatch::{
    NonLinearDispatchPlan, emit_jumpful_dispatch, emit_stateful_dispatch,
    exception_handler_region_indices,
};
use std::cell::Cell;

pub(super) fn emit_fallback_function_body(
    backend: &mut WasmBackend,
    func_ir: &FunctionIR,
    func_index: u32,
    ctx: &CompileFuncContext<'_>,
) {
    let reloc_enabled = ctx.reloc_enabled;
    let call_site_abi = &ctx.call_site_abi;
    let import_ids = ctx.import_ids;
    let frame_plan = WasmFunctionFramePlan::for_function(func_ir);
    let (mut func, frame) = frame_plan.into_function_and_frame();
    frame.emit_debug_local_map(func_ir);

    let dispatch_plan =
        NonLinearDispatchPlan::build(backend, func_ir, reloc_enabled, frame.control_mode());
    let dispatch_locals = frame.dispatch_locals();
    if let (Some(plan), Some(locals)) = (dispatch_plan.as_ref(), dispatch_locals) {
        plan.emit_table_bases(backend, func_index, &mut func, reloc_enabled, locals);
    }
    frame.emit_dispatch_seed_initializers(
        backend,
        &mut func,
        func_index,
        reloc_enabled,
        import_ids,
        ctx.const_str_scratch_segment,
    );
    frame.emit_entry_initializers(&mut func, reloc_enabled, import_ids);

    // Capture native_eh_enabled before the closure to avoid borrowing backend.
    // Native EH requires non-relocatable output because wasm-ld does not
    // support EH relocations.
    let native_eh_enabled = backend.options.native_eh_enabled && !backend.options.reloc_enabled;
    let tail_call_enabled = backend.options.tail_call_enabled;

    // Uses Cell so stateful dispatch can emit ops one at a time while sharing
    // the same tail-call counter.
    let tail_call_count: Cell<usize> = Cell::new(0);

    let exception_handler_region_indices = exception_handler_region_indices(&func_ir.ops);

    {
        let mut op_emitter = WasmFunctionEmitContext {
            backend,
            func_ir,
            ctx,
            call_site_abi,
            import_ids,
            exception_handler_region_indices: &exception_handler_region_indices,
            frame: &frame,
            func_index,
            reloc_enabled,
            native_eh_enabled,
            tail_call_enabled,
            tail_call_count: &tail_call_count,
        };

        match frame.control_mode() {
            WasmFrameControlMode::Stateful => {
                let plan = dispatch_plan
                    .as_ref()
                    .expect("dispatch plan missing for stateful wasm");
                emit_stateful_dispatch(
                    &mut func,
                    &mut op_emitter,
                    plan,
                    dispatch_locals.expect("dispatch locals missing for stateful wasm"),
                );
            }
            WasmFrameControlMode::Jumpful => {
                let plan = dispatch_plan
                    .as_ref()
                    .expect("dispatch plan missing for jumpful wasm");
                emit_jumpful_dispatch(
                    &mut func,
                    &mut op_emitter,
                    plan,
                    dispatch_locals.expect("dispatch locals missing for jumpful wasm"),
                );
            }
            WasmFrameControlMode::Plain => {
                plain::emit_plain_function_body(func_ir, &mut func, &mut op_emitter);
            }
        }
    }

    backend.tail_calls_emitted += tail_call_count.get();
    backend.codes.function(&func);
}
