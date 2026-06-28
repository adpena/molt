use super::call_site_abi::WasmCallSiteAbi;
use super::constant_ops::{ConstantOpContext, emit_constant_op};
use super::context::CompileFuncContext;
use super::control_flow::ControlKind;
use super::function_frame::WasmFunctionFrame;
use super::multi_return_layout::WasmMultiReturnLayout;
use super::*;

mod builder_ops;
mod call_ops;
mod control_ops;
mod core_runtime_ops;
mod local_state_ops;
mod numeric_ops;
mod object_attr_ops;
mod result_sink;
mod runtime_service_ops;

use call_ops::{CallOpContext, CallOpEmission, emit_call_op};
use control_ops::{ControlOpContext, emit_control_op};
use core_runtime_ops::emit_core_runtime_op;
use local_state_ops::emit_local_state_op;
use numeric_ops::emit_numeric_op;
use object_attr_ops::emit_object_attr_op;
use runtime_service_ops::{RuntimeServiceOpContext, emit_runtime_service_op};

pub(super) struct WasmFunctionEmitContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) func_ir: &'a FunctionIR,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) call_site_abi: &'a WasmCallSiteAbi<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) exception_handler_region_indices: &'a BTreeSet<usize>,
    pub(super) frame: &'a WasmFunctionFrame,
    pub(super) multi_return_candidates: &'a BTreeMap<String, usize>,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
    pub(super) tail_call_count: &'a Cell<usize>,
}

impl<'a, 'ctx> WasmFunctionEmitContext<'a, 'ctx> {
    pub(super) fn locals(&self) -> &WasmFrameLocals {
        self.frame.locals()
    }

    pub(super) fn const_cache(&self) -> &ConstantCache {
        self.frame.const_cache()
    }

    pub(super) fn scalar_plan(&self) -> &ScalarRepresentationPlan {
        self.frame.scalar_plan()
    }

    pub(super) fn arena_local(&self) -> Option<u32> {
        self.frame.arena_local()
    }

    pub(super) fn emit_ops(
        &mut self,
        func: &mut Function,
        ops: &[OpIR],
        control_stack: &mut Vec<ControlKind>,
        try_stack: &mut Vec<usize>,
        label_stack: &mut Vec<i64>,
        label_depths: &mut BTreeMap<i64, usize>,
        base_idx: usize,
    ) {
        let backend = &mut self.backend;
        let func_ir = self.func_ir;
        let ctx = self.ctx;
        let call_site_abi = self.call_site_abi;
        let import_ids = self.import_ids;
        let exception_handler_region_indices = self.exception_handler_region_indices;
        let frame = self.frame;
        let runtime_lookup_only_vars = frame.runtime_lookup_only_vars();
        let seeded_runtime_const_op_indices = frame.seeded_runtime_const_op_indices();
        let locals = frame.locals();
        let const_cache = frame.const_cache();
        let scalar_plan = frame.scalar_plan();
        let multi_return_candidates = self.multi_return_candidates;
        let multi_return = frame.multi_return();
        let func_index = self.func_index;
        let reloc_enabled = self.reloc_enabled;
        let native_eh_enabled = self.native_eh_enabled;
        let tail_call_eligible = frame.tail_call_eligible();
        let arena_local = frame.arena_local();
        let tail_call_count = self.tail_call_count;

        let last_use_local: BTreeMap<String, usize> = {
            let mut lu = BTreeMap::new();
            for (i, op) in ops.iter().enumerate() {
                if let Some(var) = &op.var
                    && var != "none"
                {
                    lu.insert(var.clone(), i);
                }
                if let Some(args) = &op.args {
                    for name in args {
                        if name != "none" {
                            lu.insert(name.clone(), i);
                        }
                    }
                }
            }
            lu
        };
        let (rc_skip_inc, rc_skip_dec) =
            crate::passes::compute_rc_coalesce_skips(ops, &last_use_local);
        let mut known_raw_ints: BTreeMap<u32, i64> = BTreeMap::new();
        let mut skip_next = false;

        for (rel_idx, op) in ops.iter().enumerate() {
            let op_idx = base_idx + rel_idx;

            if seeded_runtime_const_op_indices.contains(&op_idx) {
                continue;
            }

            if skip_next {
                skip_next = false;
                continue;
            }

            if emit_numeric_op(
                func,
                op,
                import_ids,
                locals,
                const_cache,
                scalar_plan,
                reloc_enabled,
                &known_raw_ints,
            ) {
                continue;
            }
            if emit_core_runtime_op(
                func,
                op,
                func_ir,
                import_ids,
                locals,
                scalar_plan,
                multi_return,
                reloc_enabled,
                arena_local,
                ops,
                op_idx,
            ) {
                continue;
            }
            if emit_object_attr_op(
                backend,
                func,
                op,
                func_ir,
                ctx,
                import_ids,
                locals,
                func_index,
                reloc_enabled,
                op_idx,
            ) {
                continue;
            }
            if emit_local_state_op(
                backend,
                func,
                op,
                import_ids,
                locals,
                const_cache,
                func_index,
                reloc_enabled,
            ) {
                continue;
            }

            let mut call_op_context = CallOpContext {
                func_ir,
                call_site_abi,
                import_ids,
                runtime_lookup_only_vars,
                locals,
                const_cache,
                multi_return_candidates,
                multi_return,
                reloc_enabled,
                tail_call_eligible,
                arena_local,
                tail_call_count,
                ops,
                last_use_local: &last_use_local,
                rc_skip_inc: &rc_skip_inc,
                rc_skip_dec: &rc_skip_dec,
                rel_idx,
                op_idx,
                try_stack_is_empty: try_stack.is_empty(),
            };
            match emit_call_op(&mut call_op_context, func, op) {
                CallOpEmission::Handled => continue,
                CallOpEmission::HandledAndSkipNext => {
                    skip_next = true;
                    continue;
                }
                CallOpEmission::NotHandled => {}
            }

            if emit_runtime_service_op(
                RuntimeServiceOpContext {
                    call_site_abi,
                    import_ids,
                    locals,
                    const_cache,
                    reloc_enabled,
                    native_eh_enabled,
                },
                func,
                op,
            ) {
                continue;
            }

            if emit_constant_op(
                ConstantOpContext {
                    backend,
                    ctx,
                    import_ids,
                    locals,
                    const_cache,
                    func_index,
                    reloc_enabled,
                },
                func,
                op,
                &mut known_raw_ints,
            ) {
                continue;
            }

            emit_control_op(
                ControlOpContext {
                    func_ir,
                    import_ids,
                    locals,
                    const_cache,
                    scalar_plan,
                    multi_return,
                    exception_handler_region_indices,
                    control_stack,
                    try_stack,
                    label_stack,
                    label_depths,
                    reloc_enabled,
                    native_eh_enabled,
                    arena_local,
                    op_idx,
                },
                func,
                op,
            );

            match op.kind.as_str() {
                "if"
                | "else"
                | "end_if"
                | "loop_start"
                | "loop_index_start"
                | "loop_break"
                | "loop_break_if_true"
                | "loop_break_if_false"
                | "loop_continue"
                | "label"
                | "br_if"
                | "jump"
                | "state_switch"
                | "state_transition"
                | "state_yield"
                | "chan_send_yield"
                | "chan_recv_yield"
                | "try_start"
                | "try_end"
                | "check_exception"
                | "loop_end"
                | "ret"
                | "ret_void" => {
                    known_raw_ints.clear();
                }
                "const" => {}
                _ => {
                    if let Some(ref out) = op.out
                        && let Some(&out_idx) = locals.get(out.as_str())
                    {
                        known_raw_ints.remove(&out_idx);
                    }
                }
            }
        }
    }
}
