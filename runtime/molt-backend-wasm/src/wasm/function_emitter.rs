use super::WasmBackend;
use super::context::CompileFuncContext;
use super::control_flow::ControlKind;
use super::function_frame::{WasmFrameControlMode, WasmFunctionFramePlan};
use super::op_loop::WasmFunctionEmitContext;
use super::state_dispatch::{
    NonLinearDispatchPlan, emit_jumpful_dispatch, emit_stateful_dispatch,
    exception_handler_region_indices,
};
use crate::FunctionIR;
use crate::wasm::lir_fast::try_emit_planned_lir_fast_body;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{BlockType, ExportKind, Instruction};

impl WasmBackend {
    pub(super) fn compile_func(
        &mut self,
        func_ir: &FunctionIR,
        type_idx: u32,
        ctx: &CompileFuncContext<'_>,
    ) {
        let func_index = self.func_count;
        let reloc_enabled = ctx.reloc_enabled;
        if std::env::var("MOLT_DEBUG_WASM_SIG_FUNC").ok().as_deref() == Some(func_ir.name.as_str())
        {
            eprintln!(
                "WASM_SIG_FUNC name={} type_idx={} params={:?} param_types={:?}",
                func_ir.name, type_idx, func_ir.params, func_ir.param_types
            );
        }
        self.funcs.function(type_idx);
        if reloc_enabled && func_ir.name == "molt_main" {
            self.molt_main_index = Some(func_index);
        } else {
            self.exports
                .export(&func_ir.name, ExportKind::Func, self.func_count);
        }
        self.func_count += 1;
        if try_emit_planned_lir_fast_body(self, func_ir, func_index, reloc_enabled, ctx) {
            return;
        }
        let call_site_abi = &ctx.call_site_abi;
        let import_ids = ctx.import_ids;
        let frame_plan = WasmFunctionFramePlan::for_function(func_ir, ctx);
        let (mut func, frame) = frame_plan.into_function_and_frame();
        let multi_return_candidates = ctx.multi_return_candidates;
        frame.emit_debug_local_map(func_ir);
        let mut control_stack: Vec<ControlKind> = Vec::new();
        let mut try_stack: Vec<usize> = Vec::new();
        let mut label_stack: Vec<i64> = Vec::new();
        let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

        let dispatch_plan =
            NonLinearDispatchPlan::build(self, func_ir, reloc_enabled, frame.control_mode());
        let dispatch_locals = frame.dispatch_locals();
        if let (Some(plan), Some(locals)) = (dispatch_plan.as_ref(), dispatch_locals) {
            plan.emit_table_bases(self, func_index, &mut func, reloc_enabled, locals);
        }
        frame.emit_dispatch_seed_initializers(
            self,
            &mut func,
            func_index,
            reloc_enabled,
            import_ids,
            ctx.const_str_scratch_segment,
        );
        frame.emit_entry_initializers(&mut func, reloc_enabled, import_ids);

        // Capture native_eh_enabled before the closure to avoid borrowing self.
        // Native EH requires non-relocatable output (wasm-ld doesn't support EH relocations)
        let native_eh_enabled = self.options.native_eh_enabled && !self.options.reloc_enabled;

        // Tail call optimization counter (WASM tail calls proposal §3.5).
        // Uses Cell so the closure can mutate it while also being borrowed
        // by multiple call sites (stateful dispatch emits ops one-at-a-time).
        let tail_call_count: Cell<usize> = Cell::new(0);

        let exception_handler_region_indices = exception_handler_region_indices(&func_ir.ops);

        let mut op_emitter = WasmFunctionEmitContext {
            backend: self,
            func_ir,
            ctx,
            call_site_abi,
            import_ids,
            exception_handler_region_indices: &exception_handler_region_indices,
            frame: &frame,
            multi_return_candidates,
            func_index,
            reloc_enabled,
            native_eh_enabled,
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
                let func = &mut func;
                let mut jump_labels: BTreeSet<i64> = BTreeSet::new();
                let mut label_order: Vec<i64> = Vec::new();
                for op in &func_ir.ops {
                    match op.kind.as_str() {
                        "jump" => {
                            if let Some(label_id) = op.value {
                                jump_labels.insert(label_id);
                            }
                        }
                        "label" => {
                            if let Some(label_id) = op.value {
                                label_order.push(label_id);
                            }
                        }
                        _ => {}
                    }
                }
                let label_ids: Vec<i64> = label_order
                    .into_iter()
                    .filter(|label_id| jump_labels.contains(label_id))
                    .collect();
                if !label_ids.is_empty() {
                    for label_id in label_ids.iter().rev() {
                        func.instruction(&Instruction::Block(BlockType::Empty));
                        control_stack.push(ControlKind::Block);
                        label_depths.insert(*label_id, control_stack.len() - 1);
                        label_stack.push(*label_id);
                    }
                }
                op_emitter.emit_ops(
                    func,
                    &func_ir.ops,
                    &mut control_stack,
                    &mut try_stack,
                    &mut label_stack,
                    &mut label_depths,
                    0,
                );
                while !label_stack.is_empty() {
                    label_stack.pop();
                    func.instruction(&Instruction::End);
                    control_stack.pop();
                }
                // Plain functions can legally rely on Python's implicit `None`
                // return. Match the stateful/jumpful lowering paths instead of
                // falling off the end of an i64-returning WASM function.
                // Free the per-function ScopeArena before falling off the end —
                // explicit `ret` ops free their own arena, but implicit-`None`
                // fallthrough still needs the symmetric teardown.
                frame.emit_implicit_return(func, reloc_enabled, import_ids);
            }
        }

        drop(op_emitter);

        // Accumulate tail call count from this function into the backend total.
        self.tail_calls_emitted += tail_call_count.get();

        self.codes.function(&func);
    }
}
