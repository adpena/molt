use super::context::CompileFuncContext;
use super::local_layout::WasmLocalLayout;
use super::op_loop::{ControlKind, WasmFunctionEmitContext};
use super::*;
use crate::wasm_plan::is_production_lir_wasm_fast_path_name;

fn emit_seeded_runtime_const_op(
    this: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &BTreeMap<String, u32>,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    const_str_scratch_segment: DataSegmentRef,
) {
    match op.kind.as_str() {
        "const_not_implemented" => {
            emit_call(func, reloc_enabled, import_ids["not_implemented"]);
            let local_idx = locals[op.out.as_ref().expect("const_not_implemented out")];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_ellipsis" => {
            emit_call(func, reloc_enabled, import_ids["ellipsis"]);
            let local_idx = locals[op.out.as_ref().expect("const_ellipsis out")];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_str" => {
            let out_name = op.out.as_ref().expect("const_str out");
            let bytes = op
                .bytes
                .as_deref()
                .unwrap_or_else(|| op.s_value.as_ref().expect("const_str bytes").as_bytes());
            let data = this.add_data_segment(reloc_enabled, bytes);
            let ptr_local = locals[&format!("{out_name}_ptr")];
            let len_local = locals[&format!("{out_name}_len")];
            this.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::LocalSet(ptr_local));
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalSet(len_local));
            func.instruction(&Instruction::LocalGet(ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len_local));
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            emit_call(func, reloc_enabled, import_ids["string_from_bytes"]);
            func.instruction(&Instruction::Drop);
            let out_local = locals[out_name];
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(out_local));
        }
        "const_bigint" => {
            let s = op.s_value.as_ref().expect("const_bigint string");
            let out_name = op.out.as_ref().expect("const_bigint out");
            let bytes = s.as_bytes();
            let data = this.add_data_segment(reloc_enabled, bytes);
            let ptr_local = locals[&format!("{out_name}_ptr")];
            let len_local = locals[&format!("{out_name}_len")];
            this.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::LocalSet(ptr_local));
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalSet(len_local));
            func.instruction(&Instruction::LocalGet(ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len_local));
            emit_call(func, reloc_enabled, import_ids["bigint_from_str"]);
            let out_local = locals[out_name];
            func.instruction(&Instruction::LocalSet(out_local));
        }
        "const_bytes" => {
            let bytes = op.bytes.as_ref().expect("const_bytes bytes");
            let out_name = op.out.as_ref().expect("const_bytes out");
            let data = this.add_data_segment(reloc_enabled, bytes);
            let ptr_local = locals[&format!("{out_name}_ptr")];
            let len_local = locals[&format!("{out_name}_len")];
            this.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::LocalSet(ptr_local));
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalSet(len_local));
            func.instruction(&Instruction::LocalGet(ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len_local));
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            emit_call(func, reloc_enabled, import_ids["bytes_from_bytes"]);
            func.instruction(&Instruction::Drop);
            let out_local = locals[out_name];
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(out_local));
        }
        _ => panic!("unsupported seeded runtime const op {}", op.kind),
    }
}

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
            let mut named_calls = lir_output.runtime_calls.iter();
            for instruction in &lir_output.instructions {
                if matches!(
                    instruction,
                    Instruction::Call(crate::lower_to_wasm::NAMED_RUNTIME_CALL_PLACEHOLDER)
                ) {
                    let name = named_calls.next().unwrap_or_else(|| {
                        panic!(
                            "LIR fast output for '{}' has more named-call placeholders than runtime_calls entries",
                            func_ir.name
                        )
                    });
                    let import_index = ctx.import_ids[name];
                    assert!(
                        import_index != u32::MAX,
                        "LIR fast output for '{}' calls runtime import '{name}' which was skipped/pruned from the import set",
                        func_ir.name
                    );
                    func.instruction(&Instruction::Call(import_index));
                    continue;
                }
                func.instruction(instruction);
            }
            assert!(
                named_calls.next().is_none(),
                "LIR fast output for '{}' has unconsumed runtime_calls entries",
                func_ir.name
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
            is_multi_return_callee,
            multi_ret_locals,
            multi_ret_tuple_vars,
            multi_ret_call_locals,
            multi_ret_call_vars,
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

        let dispatch_blocks = if stateful || jumpful {
            let (block_starts, block_for_op) = build_dispatch_blocks(&func_ir.ops);
            let block_map_bytes = build_dispatch_block_map(&block_for_op);
            let block_map_segment = self.add_data_segment(reloc_enabled, &block_map_bytes);
            Some((block_starts, block_map_segment))
        } else {
            None
        };
        let dispatch_control_maps = if stateful || jumpful {
            Some(build_dispatch_control_maps(
                &func_ir.ops,
                stateful,
                &func_ir.name,
            ))
        } else {
            None
        };
        let state_resume_maps = if stateful {
            let (state_map, const_ints) = build_state_resume_maps(&func_ir.ops);
            let state_remap_table = build_dense_state_remap_table(&state_map).map(|remap_bytes| {
                let remap_entries = (remap_bytes.len() / std::mem::size_of::<i64>()) as i64;
                let remap_segment = self.add_data_segment(reloc_enabled, &remap_bytes);
                (remap_entries, remap_segment)
            });
            Some((state_map, const_ints, state_remap_table))
        } else {
            None
        };
        if let Some((_, block_map_segment)) = dispatch_blocks.as_ref() {
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for dispatch");
            self.emit_data_ptr(reloc_enabled, func_index, &mut func, *block_map_segment);
            func.instruction(&Instruction::LocalSet(block_map_base_local));
        }
        if let Some((_, _, Some((_, remap_segment)))) = state_resume_maps.as_ref() {
            let remap_base_local =
                state_remap_base_local.expect("state remap base local missing for stateful wasm");
            self.emit_data_ptr(reloc_enabled, func_index, &mut func, *remap_segment);
            func.instruction(&Instruction::LocalSet(remap_base_local));
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

        let exception_handler_region_indices: BTreeSet<usize> = {
            let mut label_to_op_index: BTreeMap<i64, usize> = BTreeMap::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if matches!(op.kind.as_str(), "label" | "state_label")
                    && let Some(label_id) = op.value
                {
                    label_to_op_index.insert(label_id, idx);
                }
            }

            let mut regions = BTreeSet::new();
            let handler_labels: Vec<i64> = func_ir
                .ops
                .iter()
                .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                .collect();

            for label in handler_labels {
                let Some(&start_idx) = label_to_op_index.get(&label) else {
                    continue;
                };
                let mut nested_pushes = 0usize;
                for handler_idx in start_idx..func_ir.ops.len() {
                    let handler_op = &func_ir.ops[handler_idx];
                    regions.insert(handler_idx);
                    match handler_op.kind.as_str() {
                        "exception_push" => nested_pushes += 1,
                        "exception_pop" => {
                            if nested_pushes == 0 {
                                break;
                            }
                            nested_pushes -= 1;
                        }
                        "ret" | "ret_void" => break,
                        _ => {}
                    }
                }
            }
            regions
        };

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
            is_multi_return_callee,
            multi_ret_locals: &multi_ret_locals,
            multi_ret_tuple_vars: &multi_ret_tuple_vars,
            multi_ret_call_locals: &multi_ret_call_locals,
            multi_ret_call_vars: &multi_ret_call_vars,
            func_index,
            reloc_enabled,
            native_eh_enabled,
            tail_call_eligible,
            arena_local,
            tail_call_count: &tail_call_count,
        };

        if stateful {
            let func = &mut func;
            let state_local = state_local.expect("state local missing for stateful wasm");
            let self_ptr_local = self_ptr_local.expect("self ptr local missing for stateful wasm");
            let self_param = *locals
                .get("self_param")
                .expect("self_param missing for stateful wasm");
            let self_local = *locals
                .get("self")
                .expect("self local missing for stateful wasm");
            let op_count = func_ir.ops.len();
            let (block_starts, _) = dispatch_blocks
                .as_ref()
                .expect("dispatch blocks missing for stateful wasm");
            let block_count = block_starts.len();
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for stateful wasm");
            let dispatch_control_maps = dispatch_control_maps
                .as_ref()
                .expect("dispatch control maps missing for stateful wasm");
            let label_to_index = &dispatch_control_maps.label_to_index;
            let else_for_if = &dispatch_control_maps.else_for_if;
            let end_for_if = &dispatch_control_maps.end_for_if;
            let end_for_else = &dispatch_control_maps.end_for_else;
            let loop_continue_target = &dispatch_control_maps.loop_continue_target;
            let loop_break_target = &dispatch_control_maps.loop_break_target;
            let exception_handler_region_indices: std::collections::BTreeSet<usize> = {
                let mut regions = std::collections::BTreeSet::new();
                let handler_labels: Vec<i64> = func_ir
                    .ops
                    .iter()
                    .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                    .collect();
                for label in handler_labels {
                    let Some(&start_idx) = label_to_index.get(&label) else {
                        continue;
                    };
                    let mut nested_pushes = 0usize;
                    for handler_idx in start_idx..op_count {
                        let handler_op = &func_ir.ops[handler_idx];
                        regions.insert(handler_idx);
                        match handler_op.kind.as_str() {
                            "exception_push" => nested_pushes += 1,
                            "exception_pop" => {
                                if nested_pushes == 0 {
                                    break;
                                }
                                nested_pushes -= 1;
                            }
                            "ret" | "ret_void" => break,
                            _ => {}
                        }
                    }
                }
                regions
            };
            let (state_map, const_ints, state_remap_table) = state_resume_maps
                .as_ref()
                .expect("state resume maps missing for stateful wasm");
            let state_remap_table_entries = state_remap_table.as_ref().map(|(entries, _)| *entries);
            let sparse_state_remap_entries = state_remap_table_entries
                .is_none()
                .then(|| build_sparse_state_remap_entries(state_map));

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::LocalSet(self_ptr_local));

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Or);
            func.instruction(&Instruction::LocalSet(self_local));

            func.instruction(&Instruction::LocalGet(self_ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            emit_call(func, reloc_enabled, import_ids["obj_get_state"]);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64LtS);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(-1));
            func.instruction(&Instruction::I64Xor);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            if let Some(remap_entries) = state_remap_table_entries {
                let remap_base_local = state_remap_base_local
                    .expect("state remap base local missing for stateful wasm");
                let remap_value_local = state_remap_value_local
                    .expect("state remap value local missing for stateful wasm");
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(remap_entries));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(remap_base_local));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::I32Const(8));
                func.instruction(&Instruction::I32Mul);
                func.instruction(&Instruction::I32Add);
                func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::LocalSet(remap_value_local));
                func.instruction(&Instruction::LocalGet(remap_value_local));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64GeS);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(remap_value_local));
                func.instruction(&Instruction::LocalSet(state_local));
                func.instruction(&Instruction::End);
                func.instruction(&Instruction::End);
            } else {
                emit_sparse_state_remap_lookup(
                    func,
                    state_local,
                    sparse_state_remap_entries
                        .as_deref()
                        .expect("sparse state remap entries missing for stateful wasm"),
                );
            }
            func.instruction(&Instruction::End);

            let dispatch_depths: Vec<u32> = (0..block_count)
                .map(|idx| (block_count - 1 - idx) as u32)
                .collect();

            let return_local = return_local.expect("stateful/jumpful missing return local");
            func.instruction(&Instruction::Block(BlockType::Empty));
            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..block_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(op_count as i64));
            func.instruction(&Instruction::I64GeU);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::I64Const(block_count as i64));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(block_map_base_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(4));
            func.instruction(&Instruction::I32Mul);
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                align: 2,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..block_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), block_count as u32));
            func.instruction(&Instruction::End);

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();

            for (block_idx, start) in block_starts.iter().enumerate() {
                let end = block_starts.get(block_idx + 1).copied().unwrap_or(op_count);
                let depth = dispatch_depths[block_idx];
                let mut block_terminated = false;

                for idx in *start..end {
                    let op = &func_ir.ops[idx];
                    match op.kind.as_str() {
                        "state_switch" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "aiter" => {
                            let args = op.args.as_ref().unwrap();
                            let iter = locals[&args[0]];
                            func.instruction(&Instruction::LocalGet(iter));
                            emit_call(func, reloc_enabled, import_ids["aiter"]);
                            func.instruction(&Instruction::LocalSet(
                                locals[op.out.as_ref().unwrap()],
                            ));
                        }
                        "state_transition" => {
                            let args = op.args.as_ref().unwrap();
                            let future = locals[&args[0]];
                            let (slot_bits, pending_state) = if args.len() == 2 {
                                (None, locals[&args[1]])
                            } else {
                                (Some(locals[&args[1]]), locals[&args[2]])
                            };
                            let pending_state_name =
                                if args.len() == 2 { &args[1] } else { &args[2] };
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let out = locals[op.out.as_ref().unwrap()];
                            let next_block = idx + 1;
                            let return_depth = depth + 2;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(future));
                            emit_call(func, reloc_enabled, import_ids["future_poll"]);
                            func.instruction(&Instruction::LocalSet(out));
                            // Store pending return value before the
                            // conditional so the If block does not
                            // leave values on the stack.
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::LocalSet(return_local));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(future));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["sleep_register"]);
                            func.instruction(&Instruction::Drop);
                            func.instruction(&Instruction::Br(return_depth));
                            func.instruction(&Instruction::End);
                            if let Some(slot) = slot_bits {
                                func.instruction(&Instruction::LocalGet(self_ptr_local));
                                func.instruction(&Instruction::I32WrapI64);
                                func.instruction(&Instruction::LocalGet(slot));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                                func.instruction(&Instruction::LocalGet(out));
                                emit_call(func, reloc_enabled, import_ids["closure_store"]);
                                func.instruction(&Instruction::Drop);
                            }
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "state_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let pair = locals[&args[0]];
                            let resume_state_id = op.value.unwrap();
                            let resume_encoded = state_map
                                .get(&resume_state_id)
                                .copied()
                                .map(|idx| !(idx as i64));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(encoded) = resume_encoded {
                                func.instruction(&Instruction::I64Const(encoded));
                            } else {
                                func.instruction(&Instruction::I64Const(resume_state_id));
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(pair));
                            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            func.instruction(&Instruction::LocalGet(pair));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "chan_send_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let chan = locals[&args[0]];
                            let val = locals[&args[1]];
                            let pending_state = locals[&args[2]];
                            let pending_state_name = &args[2];
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(chan));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["chan_send"]);
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::Return);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "chan_recv_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let chan = locals[&args[0]];
                            let pending_state = locals[&args[1]];
                            let pending_state_name = &args[1];
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(chan));
                            emit_call(func, reloc_enabled, import_ids["chan_recv"]);
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::Return);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let else_idx = else_for_if.get(&idx).copied();
                            let end_idx = end_for_if.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "if without end_if")
                            });
                            let false_target = if let Some(else_pos) = else_idx {
                                else_pos + 1
                            } else {
                                end_idx + 1
                            };
                            let true_block = idx + 1;
                            let false_block = false_target;
                            let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(
                                &scalar_plan,
                                &args[0],
                            ) {
                                "is_truthy_int"
                            } else {
                                "is_truthy"
                            };
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids[truthy_import],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(true_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(false_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "else" => {
                            let end_idx = end_for_else.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "else without end_if")
                            });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "end_if" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_start" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_index_start" => {
                            let args = op.args.as_ref().unwrap();
                            let start = locals[&args[0]];
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(start));
                            func.instruction(&Instruction::LocalSet(out));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_break_if_true" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_true without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_exception" => {
                            // Value-less exception-flag break in the jumpful
                            // state-machine lowering.  Mirrors `loop_break_if_true`
                            // but reads the sacrosanct `exception_pending` flag
                            // (`!= 0`) instead of an is_truthy(cond) value: TRUE
                            // (pending) -> jump to the loop-end state, FALSE ->
                            // fall through to the next state.
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_exception without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_false" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_false without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            // Break when the condition is *falsy*: invert truthiness.
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break" => {
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_continue" => {
                            let start_idx =
                                loop_continue_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_continue without loop",
                                    )
                                });
                            let start_block = start_idx + 1;
                            func.instruction(&Instruction::I64Const(start_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_end" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "jump" => {
                            let target_label = op.value.expect("jump missing label");
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown jump label {target_label}"),
                                    )
                                });
                            let target_block = target_idx;
                            func.instruction(&Instruction::I64Const(target_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "br_if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let target_label = op.value.unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "br_if missing label")
                            });
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown br_if label {target_label}"),
                                    )
                                });
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(target_idx as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                        }
                        "try_start" | "try_end" | "label" | "state_label" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "check_exception" => {
                            if native_eh_enabled {
                                // Native EH: skip polling; fall through to next state.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else if exception_handler_region_indices.contains(&idx) {
                                // Exception-handler regions operate on the currently
                                // pending exception. Re-polling here would immediately
                                // re-branch back into the same handler before
                                // exception_clear/print/cleanup can run.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else {
                                let target_label = op.value.unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "check_exception missing label",
                                    )
                                });
                                let target_idx = label_to_index
                                    .get(&target_label)
                                    .copied()
                                    .unwrap_or_else(|| {
                                        dispatch_control_panic(
                                            &func_ir.name,
                                            idx,
                                            format_args!(
                                                "unknown check_exception label {target_label}"
                                            ),
                                        )
                                    });
                                let target_block = target_idx;
                                let next_block = idx + 1;
                                emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                                func.instruction(&Instruction::I64Const(0));
                                func.instruction(&Instruction::I64Ne);
                                func.instruction(&Instruction::If(BlockType::Empty));
                                func.instruction(&Instruction::I64Const(target_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::Else);
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::End);
                                block_terminated = true;
                            }
                        }
                        "ret" => {
                            let ret_local =
                                op.var.as_ref().and_then(|name| locals.get(name).copied());
                            if let Some(local_idx) = ret_local {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            } else {
                                dispatch_control_panic(
                                    &func_ir.name,
                                    idx,
                                    format_args!("ret target local {:?} is not present", op.var),
                                );
                            }
                            // Defensive arena teardown: state-machine functions
                            // do not currently produce arena-eligible allocs
                            // (StateYield forces GlobalEscape), but symmetry
                            // matters if escape analysis ever loosens.
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "ret_void" => {
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        _ => {
                            op_emitter.emit_ops(
                                func,
                                std::slice::from_ref(op),
                                &mut scratch_control,
                                &mut scratch_try,
                                &mut label_stack,
                                &mut label_depths,
                                idx,
                            );
                        }
                    }
                    if block_terminated {
                        break;
                    }
                }

                let next_state = end;
                if !block_terminated {
                    func.instruction(&Instruction::I64Const(next_state as i64));
                    func.instruction(&Instruction::LocalSet(state_local));
                }
                func.instruction(&Instruction::Br(depth));

                if block_idx + 1 < block_count {
                    func.instruction(&Instruction::End);
                }
            }

            func.instruction(&Instruction::Br(0));
            func.instruction(&Instruction::End);
            const_cache.emit_none(func);
            func.instruction(&Instruction::LocalSet(return_local));
            func.instruction(&Instruction::End);
            // Defensive arena teardown for the stateful trailing return.
            if let Some(arena_idx) = arena_local {
                func.instruction(&Instruction::LocalGet(arena_idx));
                emit_call(func, reloc_enabled, import_ids["arena_free"]);
            }
            func.instruction(&Instruction::LocalGet(return_local));
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        } else if jumpful {
            let func = &mut func;
            let state_local = state_local.expect("state local missing for jumpful wasm");
            let op_count = func_ir.ops.len();
            let (block_starts, _) = dispatch_blocks
                .as_ref()
                .expect("dispatch blocks missing for jumpful wasm");
            let block_count = block_starts.len();
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for jumpful wasm");
            let dispatch_control_maps = dispatch_control_maps
                .as_ref()
                .expect("dispatch control maps missing for jumpful wasm");
            let label_to_index = &dispatch_control_maps.label_to_index;
            let else_for_if = &dispatch_control_maps.else_for_if;
            let end_for_if = &dispatch_control_maps.end_for_if;
            let end_for_else = &dispatch_control_maps.end_for_else;
            let loop_continue_target = &dispatch_control_maps.loop_continue_target;
            let loop_break_target = &dispatch_control_maps.loop_break_target;
            let exception_handler_region_indices: std::collections::BTreeSet<usize> = {
                let mut regions = std::collections::BTreeSet::new();
                let handler_labels: Vec<i64> = func_ir
                    .ops
                    .iter()
                    .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                    .collect();
                for label in handler_labels {
                    let Some(&start_idx) = label_to_index.get(&label) else {
                        continue;
                    };
                    let mut nested_pushes = 0usize;
                    for handler_idx in start_idx..op_count {
                        let handler_op = &func_ir.ops[handler_idx];
                        regions.insert(handler_idx);
                        match handler_op.kind.as_str() {
                            "exception_push" => nested_pushes += 1,
                            "exception_pop" => {
                                if nested_pushes == 0 {
                                    break;
                                }
                                nested_pushes -= 1;
                            }
                            "ret" | "ret_void" => break,
                            _ => {}
                        }
                    }
                }
                regions
            };

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();
            let mut label_stack: Vec<i64> = Vec::new();
            let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

            let dispatch_depths: Vec<u32> = (0..block_count)
                .map(|idx| (block_count - 1 - idx) as u32)
                .collect();

            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::LocalSet(state_local));

            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..block_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(op_count as i64));
            func.instruction(&Instruction::I64GeU);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::I64Const(block_count as i64));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(block_map_base_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(4));
            func.instruction(&Instruction::I32Mul);
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                align: 2,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..block_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), block_count as u32));
            func.instruction(&Instruction::End);

            for (block_idx, start) in block_starts.iter().enumerate() {
                let end = block_starts.get(block_idx + 1).copied().unwrap_or(op_count);
                let depth = dispatch_depths[block_idx];
                let mut block_terminated = false;

                for idx in *start..end {
                    let op = &func_ir.ops[idx];
                    match op.kind.as_str() {
                        "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                        | "chan_recv_yield" => {
                            dispatch_control_panic(
                                &func_ir.name,
                                idx,
                                format_args!("jumpful path hit stateful op {}", op.kind),
                            );
                        }
                        "if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let else_idx = else_for_if.get(&idx).copied();
                            let end_idx = end_for_if.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "if without end_if")
                            });
                            let false_target = if let Some(else_pos) = else_idx {
                                else_pos + 1
                            } else {
                                end_idx + 1
                            };
                            let true_block = idx + 1;
                            let false_block = false_target;
                            let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(
                                &scalar_plan,
                                &args[0],
                            ) {
                                "is_truthy_int"
                            } else {
                                "is_truthy"
                            };
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids[truthy_import],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(true_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(false_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "else" => {
                            let end_idx = end_for_else.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "else without end_if")
                            });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "end_if" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_start" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_index_start" => {
                            let args = op.args.as_ref().unwrap();
                            let start = locals[&args[0]];
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(start));
                            func.instruction(&Instruction::LocalSet(out));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_break_if_true" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_true without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_exception" => {
                            // Value-less exception-flag break in the jumpful
                            // state-machine lowering.  Mirrors `loop_break_if_true`
                            // but reads the sacrosanct `exception_pending` flag
                            // (`!= 0`) instead of an is_truthy(cond) value: TRUE
                            // (pending) -> jump to the loop-end state, FALSE ->
                            // fall through to the next state.
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_exception without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_false" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_false without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            // Break when the condition is *falsy*: invert truthiness.
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break" => {
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_continue" => {
                            let start_idx =
                                loop_continue_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_continue without loop",
                                    )
                                });
                            let start_block = start_idx + 1;
                            func.instruction(&Instruction::I64Const(start_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_end" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "jump" => {
                            let target_label = op.value.unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "jump missing label")
                            });
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown jump label {target_label}"),
                                    )
                                });
                            let target_block = target_idx;
                            func.instruction(&Instruction::I64Const(target_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "br_if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let target_label = op.value.unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "br_if missing label")
                            });
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown br_if label {target_label}"),
                                    )
                                });
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(target_idx as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                        }
                        "try_start" | "try_end" | "label" | "state_label" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "check_exception" => {
                            if native_eh_enabled {
                                // Native EH: skip polling; fall through to next state.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else if exception_handler_region_indices.contains(&idx) {
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else {
                                let target_label = op.value.unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "check_exception missing label",
                                    )
                                });
                                let target_idx = label_to_index
                                    .get(&target_label)
                                    .copied()
                                    .unwrap_or_else(|| {
                                        dispatch_control_panic(
                                            &func_ir.name,
                                            idx,
                                            format_args!(
                                                "unknown check_exception label {target_label}"
                                            ),
                                        )
                                    });
                                let target_block = target_idx;
                                let next_block = idx + 1;
                                emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                                func.instruction(&Instruction::I64Const(0));
                                func.instruction(&Instruction::I64Ne);
                                func.instruction(&Instruction::If(BlockType::Empty));
                                func.instruction(&Instruction::I64Const(target_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::Else);
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::End);
                                block_terminated = true;
                            }
                        }
                        "ret" => {
                            let ret_local =
                                op.var.as_ref().and_then(|name| locals.get(name).copied());
                            if let Some(local_idx) = ret_local {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            } else {
                                dispatch_control_panic(
                                    &func_ir.name,
                                    idx,
                                    format_args!("ret target local {:?} is not present", op.var),
                                );
                            }
                            // Defensive arena teardown: state-machine functions
                            // do not currently produce arena-eligible allocs
                            // (StateYield forces GlobalEscape), but symmetry
                            // matters if escape analysis ever loosens.
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "ret_void" => {
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        _ => {
                            op_emitter.emit_ops(
                                func,
                                std::slice::from_ref(op),
                                &mut scratch_control,
                                &mut scratch_try,
                                &mut label_stack,
                                &mut label_depths,
                                idx,
                            );
                        }
                    }
                    if block_terminated {
                        break;
                    }
                }

                let next_state = end;
                if !block_terminated {
                    func.instruction(&Instruction::I64Const(next_state as i64));
                    func.instruction(&Instruction::LocalSet(state_local));
                }
                func.instruction(&Instruction::Br(depth));

                if block_idx + 1 < block_count {
                    func.instruction(&Instruction::End);
                }
            }
            func.instruction(&Instruction::Br(0));
            func.instruction(&Instruction::End);
            // Defensive arena teardown for the stateful trailing return.
            if let Some(arena_idx) = arena_local {
                func.instruction(&Instruction::LocalGet(arena_idx));
                emit_call(func, reloc_enabled, import_ids["arena_free"]);
            }
            const_cache.emit_none(func);
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
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
