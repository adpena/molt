use super::constant_ops::emit_seeded_runtime_const_op;
use super::context::CompileFuncContext;
use super::local_layout::WasmLocalLayout;
use super::op_loop::{ControlKind, WasmFunctionEmitContext};
use super::state_dispatch::{
    NonLinearDispatchLocals, NonLinearDispatchPlan, emit_jumpful_dispatch, emit_stateful_dispatch,
    exception_handler_region_indices,
};
use super::*;
use crate::wasm_lir_fast_output::emit_lir_fast_output_body;
use crate::wasm_plan::is_production_lir_wasm_fast_path_name;

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
        if is_production_lir_wasm_fast_path_name(&func_ir.name)
            && !ctx.escaped_callable_targets.contains(&func_ir.name)
            && let Some(lir_output) = ctx.lir_fast_outputs.get(&func_ir.name)
        {
            if std::env::var("MOLT_DEBUG_WASM_SIG_FUNC").ok().as_deref()
                == Some(func_ir.name.as_str())
            {
                eprintln!(
                    "WASM_SIG_FUNC fast_path name={} lir_param_types={:?} lir_result_types={:?}",
                    func_ir.name, lir_output.param_types, lir_output.result_types
                );
            }
            let mut func = Function::new_with_locals_types(lir_output.locals.clone());
            // Resolve NAMED runtime calls: the k-th placeholder pairs with
            // runtime_calls[k] (positional — instruction indexes shift under
            // the LIR peephole pass, so the pairing is by order, not index).
            emit_lir_fast_output_body(
                &func_ir.name,
                lir_output,
                |name| ctx.import_ids[name],
                &mut func,
            );
            self.codes.function(&func);
            return;
        }
        let func_map = ctx.func_map;
        let func_indices = ctx.func_indices;
        let trampoline_map = ctx.trampoline_map;
        let table_base = ctx.table_base;
        let import_ids = ctx.import_ids;
        let closure_functions = ctx.closure_functions;
        let local_layout = WasmLocalLayout::for_function(func_ir, ctx);
        let WasmLocalLayout {
            locals,
            local_types,
            runtime_lookup_only_vars,
            scalar_plan,
            stateful,
            jumpful,
            tail_call_eligible,
            arena_local,
            self_ptr_local,
            state_local,
            block_map_base_local,
            return_local,
            state_remap_base_local,
            state_remap_value_local,
            const_cache,
            const_seed_locals,
            seeded_runtime_const_ops,
            seeded_runtime_const_op_indices,
            multi_return,
        } = local_layout;
        let multi_return_candidates = ctx.multi_return_candidates;
        let mut func = Function::new_with_locals_types(local_types);
        if std::env::var("MOLT_DEBUG_WASM_LOCALS_FUNC").ok().as_deref()
            == Some(func_ir.name.as_str())
        {
            eprintln!("WASM_DEBUG_FUNC {}", func_ir.name);
            for (idx, op) in func_ir.ops.iter().enumerate() {
                let mut mentioned: Vec<String> = Vec::new();
                if let Some(args) = &op.args {
                    mentioned.extend(args.iter().cloned());
                }
                if let Some(var) = &op.var {
                    mentioned.push(var.clone());
                }
                if let Some(out) = &op.out {
                    mentioned.push(out.clone());
                }
                mentioned.sort();
                mentioned.dedup();
                let mapped: Vec<String> = mentioned
                    .into_iter()
                    .filter_map(|name| locals.get(&name).map(|slot| format!("{name}->{slot}")))
                    .collect();
                eprintln!(
                    "WASM_DEBUG_OP {} kind={} var={:?} out={:?} args={:?} locals={:?}",
                    idx, op.kind, op.var, op.out, op.args, mapped
                );
            }
        }
        let mut control_stack: Vec<ControlKind> = Vec::new();
        let mut try_stack: Vec<usize> = Vec::new();
        let mut label_stack: Vec<i64> = Vec::new();
        let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

        let dispatch_plan =
            NonLinearDispatchPlan::build(self, func_ir, reloc_enabled, stateful, jumpful);
        let dispatch_locals = if stateful || jumpful {
            Some(NonLinearDispatchLocals {
                state_local: state_local.expect("state local missing for dispatch wasm"),
                block_map_base_local: block_map_base_local
                    .expect("block map base local missing for dispatch wasm"),
                return_local: return_local.expect("stateful/jumpful missing return local"),
                self_ptr_local,
                state_remap_base_local,
                state_remap_value_local,
            })
        } else {
            None
        };
        if let (Some(plan), Some(locals)) = (dispatch_plan.as_ref(), dispatch_locals) {
            plan.emit_table_bases(self, func_index, &mut func, reloc_enabled, locals);
        }
        if stateful || jumpful {
            for (_, op) in &seeded_runtime_const_ops {
                emit_seeded_runtime_const_op(
                    self,
                    &mut func,
                    op,
                    &locals,
                    func_index,
                    reloc_enabled,
                    import_ids,
                    ctx.const_str_scratch_segment,
                );
            }
            // Seed dispatch locals from their first literal assignment so control-flow
            // edge threading cannot observe a raw wasm zero (0.0 bits) for an
            // otherwise integer/none local before its defining block executes.
            for (local_idx, bits) in const_seed_locals.iter().copied() {
                func.instruction(&Instruction::I64Const(bits));
                func.instruction(&Instruction::LocalSet(local_idx));
            }
        }

        // Initialize constant materialization cache (once per function entry).
        const_cache.emit_init(&mut func);

        // Scope arena setup: invoke `molt_arena_new` once at function entry
        // and stash the handle in the reserved local. Mirrors the native
        // backend's MLKit-style region lifecycle so NoEscape allocations
        // bypass the global allocator and the entire arena is freed in O(1)
        // before each return.
        if let Some(idx) = arena_local {
            emit_call(&mut func, reloc_enabled, import_ids["arena_new"]);
            func.instruction(&Instruction::LocalSet(idx));
        }

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
            func_map,
            func_indices,
            trampoline_map,
            table_base,
            import_ids,
            closure_functions,
            runtime_lookup_only_vars: &runtime_lookup_only_vars,
            seeded_runtime_const_op_indices: &seeded_runtime_const_op_indices,
            exception_handler_region_indices: &exception_handler_region_indices,
            locals: &locals,
            const_cache: &const_cache,
            scalar_plan: &scalar_plan,
            multi_return_candidates,
            multi_return: &multi_return,
            func_index,
            reloc_enabled,
            native_eh_enabled,
            tail_call_eligible,
            arena_local,
            tail_call_count: &tail_call_count,
        };

        if stateful {
            let plan = dispatch_plan
                .as_ref()
                .expect("dispatch plan missing for stateful wasm");
            emit_stateful_dispatch(
                &mut func,
                &mut op_emitter,
                plan,
                dispatch_locals.expect("dispatch locals missing for stateful wasm"),
            );
        } else if jumpful {
            let plan = dispatch_plan
                .as_ref()
                .expect("dispatch plan missing for jumpful wasm");
            emit_jumpful_dispatch(
                &mut func,
                &mut op_emitter,
                plan,
                dispatch_locals.expect("dispatch locals missing for jumpful wasm"),
            );
        } else {
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
            if let Some(arena_idx) = arena_local {
                func.instruction(&Instruction::LocalGet(arena_idx));
                emit_call(func, reloc_enabled, import_ids["arena_free"]);
            }
            const_cache.emit_none(func);
            func.instruction(&Instruction::End);
        }

        drop(op_emitter);

        // Accumulate tail call count from this function into the backend total.
        self.tail_calls_emitted += tail_call_count.get();

        self.codes.function(&func);
    }
}
