use super::op_loop::{ControlKind, WasmFunctionEmitContext};
use super::*;
use crate::wasm_dispatch::{
    DispatchControlMaps, build_dense_state_remap_table, build_dispatch_block_map,
    build_dispatch_blocks, build_dispatch_control_maps, build_sparse_state_remap_entries,
    build_state_resume_maps, dispatch_control_panic, emit_sparse_state_remap_lookup,
};

pub(super) struct NonLinearDispatchPlan {
    block_starts: Vec<usize>,
    block_map_segment: DataSegmentRef,
    control_maps: DispatchControlMaps,
    state_resume: Option<StateResumePlan>,
}

struct StateResumePlan {
    state_map: BTreeMap<i64, usize>,
    const_ints: BTreeMap<String, i64>,
    remap_table: Option<(i64, DataSegmentRef)>,
}

#[derive(Clone, Copy)]
pub(super) struct NonLinearDispatchLocals {
    pub(super) state_local: u32,
    pub(super) block_map_base_local: u32,
    pub(super) return_local: u32,
    pub(super) self_ptr_local: Option<u32>,
    pub(super) state_remap_base_local: Option<u32>,
    pub(super) state_remap_value_local: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DispatchMode {
    Stateful,
    Jumpful,
}

impl NonLinearDispatchPlan {
    pub(super) fn build(
        backend: &mut WasmBackend,
        func_ir: &FunctionIR,
        reloc_enabled: bool,
        stateful: bool,
        jumpful: bool,
    ) -> Option<Self> {
        if !stateful && !jumpful {
            return None;
        }

        let (block_starts, block_for_op) = build_dispatch_blocks(&func_ir.ops);
        let block_map_bytes = build_dispatch_block_map(&block_for_op);
        let block_map_segment = backend.add_data_segment(reloc_enabled, &block_map_bytes);
        let control_maps = build_dispatch_control_maps(&func_ir.ops, stateful, &func_ir.name);
        let state_resume = stateful.then(|| {
            let (state_map, const_ints) = build_state_resume_maps(&func_ir.ops);
            let remap_table = build_dense_state_remap_table(&state_map).map(|remap_bytes| {
                let remap_entries = (remap_bytes.len() / std::mem::size_of::<i64>()) as i64;
                let remap_segment = backend.add_data_segment(reloc_enabled, &remap_bytes);
                (remap_entries, remap_segment)
            });
            StateResumePlan {
                state_map,
                const_ints,
                remap_table,
            }
        });

        Some(Self {
            block_starts,
            block_map_segment,
            control_maps,
            state_resume,
        })
    }

    pub(super) fn emit_table_bases(
        &self,
        backend: &mut WasmBackend,
        func_index: u32,
        func: &mut Function,
        reloc_enabled: bool,
        locals: NonLinearDispatchLocals,
    ) {
        backend.emit_data_ptr(reloc_enabled, func_index, func, self.block_map_segment);
        func.instruction(&Instruction::LocalSet(locals.block_map_base_local));
        if let Some((_, remap_segment)) = self
            .state_resume
            .as_ref()
            .and_then(|resume| resume.remap_table.as_ref())
        {
            let remap_base_local = locals
                .state_remap_base_local
                .expect("state remap base local missing for stateful wasm");
            backend.emit_data_ptr(reloc_enabled, func_index, func, *remap_segment);
            func.instruction(&Instruction::LocalSet(remap_base_local));
        }
    }
}

pub(super) fn exception_handler_region_indices(ops: &[OpIR]) -> BTreeSet<usize> {
    let mut label_to_op_index: BTreeMap<i64, usize> = BTreeMap::new();
    for (idx, op) in ops.iter().enumerate() {
        if matches!(op.kind.as_str(), "label" | "state_label")
            && let Some(label_id) = op.value
        {
            label_to_op_index.insert(label_id, idx);
        }
    }
    exception_handler_region_indices_from_label_map(ops, &label_to_op_index)
}

pub(super) fn emit_stateful_dispatch(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
) {
    emit_non_linear_dispatch(func, op_emitter, plan, locals, DispatchMode::Stateful);
}

pub(super) fn emit_jumpful_dispatch(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
) {
    emit_non_linear_dispatch(func, op_emitter, plan, locals, DispatchMode::Jumpful);
}

fn emit_non_linear_dispatch(
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    mode: DispatchMode,
) {
    let func_ir = op_emitter.func_ir;
    let op_count = func_ir.ops.len();
    let block_count = plan.block_starts.len();
    let dispatch_depths: Vec<u32> = (0..block_count)
        .map(|idx| (block_count - 1 - idx) as u32)
        .collect();
    let exception_regions = exception_handler_region_indices_from_label_map(
        &func_ir.ops,
        &plan.control_maps.label_to_index,
    );

    match mode {
        DispatchMode::Stateful => emit_stateful_resume_prelude(func, op_emitter, plan, locals),
        DispatchMode::Jumpful => {
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::LocalSet(locals.state_local));
        }
    }

    if mode == DispatchMode::Stateful {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }
    func.instruction(&Instruction::Loop(BlockType::Empty));
    for _ in (0..block_count).rev() {
        func.instruction(&Instruction::Block(BlockType::Empty));
    }

    emit_dispatch_block_lookup(func, op_count, block_count, locals);

    let mut scratch_control: Vec<ControlKind> = Vec::new();
    let mut scratch_try: Vec<usize> = Vec::new();
    let mut label_stack: Vec<i64> = Vec::new();
    let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

    for (block_idx, start) in plan.block_starts.iter().enumerate() {
        let end = plan
            .block_starts
            .get(block_idx + 1)
            .copied()
            .unwrap_or(op_count);
        let depth = dispatch_depths[block_idx];
        let mut block_terminated = false;

        for idx in *start..end {
            let op = &func_ir.ops[idx];
            match op.kind.as_str() {
                "state_switch" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
                    block_terminated = true;
                }
                "aiter" if mode == DispatchMode::Stateful => {
                    let args = op.args.as_ref().unwrap();
                    let iter = op_emitter.locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(iter));
                    emit_call(
                        func,
                        op_emitter.reloc_enabled,
                        op_emitter.import_ids["aiter"],
                    );
                    func.instruction(&Instruction::LocalSet(
                        op_emitter.locals[op.out.as_ref().unwrap()],
                    ));
                }
                "state_transition" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_state_transition(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
                }
                "state_yield" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_state_yield(func, op_emitter, plan, locals, op, idx);
                    block_terminated = true;
                }
                "chan_send_yield" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_chan_send_yield(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
                }
                "chan_recv_yield" => {
                    require_stateful(mode, func_ir, idx, op);
                    emit_chan_recv_yield(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
                }
                "if" => {
                    emit_dispatch_if(func, op_emitter, plan, locals, op, idx, depth);
                    block_terminated = true;
                }
                "else" => {
                    let end_idx = plan
                        .control_maps
                        .end_for_else
                        .get(&idx)
                        .copied()
                        .unwrap_or_else(|| {
                            dispatch_control_panic(&func_ir.name, idx, "else without end_if")
                        });
                    emit_set_state_and_br(func, locals.state_local, end_idx + 1, depth);
                    block_terminated = true;
                }
                "end_if" | "loop_start" | "loop_end" | "try_start" | "try_end" | "label"
                | "state_label" => {
                    emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
                    block_terminated = true;
                }
                "loop_index_start" => {
                    let args = op.args.as_ref().unwrap();
                    let start = op_emitter.locals[&args[0]];
                    let out = op_emitter.locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalGet(start));
                    func.instruction(&Instruction::LocalSet(out));
                    emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
                    block_terminated = true;
                }
                "loop_break_if_true" => {
                    emit_dispatch_loop_break_cond(
                        func, op_emitter, plan, locals, op, idx, depth, false,
                    );
                    block_terminated = true;
                }
                "loop_break_if_false" => {
                    emit_dispatch_loop_break_cond(
                        func, op_emitter, plan, locals, op, idx, depth, true,
                    );
                    block_terminated = true;
                }
                "loop_break_if_exception" => {
                    let end_idx = loop_break_target(plan, func_ir, idx, "loop_break_if_exception");
                    let end_block = end_idx + 1;
                    let next_block = idx + 1;
                    emit_call(
                        func,
                        op_emitter.reloc_enabled,
                        op_emitter.import_ids["exception_pending"],
                    );
                    func.instruction(&Instruction::I64Const(0));
                    func.instruction(&Instruction::I64Ne);
                    emit_conditional_state_branch(
                        func,
                        locals.state_local,
                        end_block,
                        next_block,
                        depth + 1,
                    );
                    block_terminated = true;
                }
                "loop_break" => {
                    let end_idx = loop_break_target(plan, func_ir, idx, "loop_break");
                    emit_set_state_and_br(func, locals.state_local, end_idx + 1, depth);
                    block_terminated = true;
                }
                "loop_continue" => {
                    let start_idx = plan
                        .control_maps
                        .loop_continue_target
                        .get(&idx)
                        .copied()
                        .unwrap_or_else(|| {
                            dispatch_control_panic(&func_ir.name, idx, "loop_continue without loop")
                        });
                    emit_set_state_and_br(func, locals.state_local, start_idx + 1, depth);
                    block_terminated = true;
                }
                "jump" => {
                    let target_label = op.value.unwrap_or_else(|| {
                        dispatch_control_panic(&func_ir.name, idx, "jump missing label")
                    });
                    let target_idx = label_target(plan, func_ir, idx, target_label, "jump");
                    emit_set_state_and_br(func, locals.state_local, target_idx, depth);
                    block_terminated = true;
                }
                "br_if" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = op_emitter.locals[&args[0]];
                    let target_label = op.value.unwrap_or_else(|| {
                        dispatch_control_panic(&func_ir.name, idx, "br_if missing label")
                    });
                    let target_idx = label_target(plan, func_ir, idx, target_label, "br_if");
                    emit_branch_truthiness_i32(
                        func,
                        cond,
                        op_emitter.import_ids["is_truthy"],
                        op_emitter.reloc_enabled,
                    );
                    func.instruction(&Instruction::If(BlockType::Empty));
                    emit_set_state_and_br(func, locals.state_local, target_idx, depth + 1);
                    func.instruction(&Instruction::End);
                }
                "check_exception" => {
                    emit_dispatch_check_exception(
                        func,
                        op_emitter,
                        plan,
                        locals,
                        op,
                        idx,
                        depth,
                        &exception_regions,
                    );
                    block_terminated = true;
                }
                "ret" => {
                    let ret_local = op
                        .var
                        .as_ref()
                        .and_then(|name| op_emitter.locals.get(name).copied());
                    if let Some(local_idx) = ret_local {
                        func.instruction(&Instruction::LocalGet(local_idx));
                    } else {
                        dispatch_control_panic(
                            &func_ir.name,
                            idx,
                            format_args!("ret target local {:?} is not present", op.var),
                        );
                    }
                    emit_arena_free(func, op_emitter);
                    func.instruction(&Instruction::Return);
                    block_terminated = true;
                }
                "ret_void" => {
                    emit_arena_free(func, op_emitter);
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

        if !block_terminated {
            func.instruction(&Instruction::I64Const(end as i64));
            func.instruction(&Instruction::LocalSet(locals.state_local));
        }
        func.instruction(&Instruction::Br(depth));

        if block_idx + 1 < block_count {
            func.instruction(&Instruction::End);
        }
    }

    emit_dispatch_trailing_return(func, op_emitter, locals, mode);
}

fn exception_handler_region_indices_from_label_map(
    ops: &[OpIR],
    label_to_index: &BTreeMap<i64, usize>,
) -> BTreeSet<usize> {
    let mut regions = BTreeSet::new();
    let handler_labels: Vec<i64> = ops
        .iter()
        .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
        .collect();
    for label in handler_labels {
        let Some(&start_idx) = label_to_index.get(&label) else {
            continue;
        };
        let mut nested_pushes = 0usize;
        for handler_idx in start_idx..ops.len() {
            let handler_op = &ops[handler_idx];
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
}

fn emit_stateful_resume_prelude(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
) {
    let self_ptr_local = locals
        .self_ptr_local
        .expect("self ptr local missing for stateful wasm");
    let self_param = *op_emitter
        .locals
        .get("self_param")
        .expect("self_param missing for stateful wasm");
    let self_local = *op_emitter
        .locals
        .get("self")
        .expect("self local missing for stateful wasm");
    let resume = plan
        .state_resume
        .as_ref()
        .expect("state resume maps missing for stateful wasm");
    let state_remap_table_entries = resume.remap_table.as_ref().map(|(entries, _)| *entries);
    let sparse_state_remap_entries = state_remap_table_entries
        .is_none()
        .then(|| build_sparse_state_remap_entries(&resume.state_map));

    func.instruction(&Instruction::LocalGet(self_param));
    func.instruction(&Instruction::LocalSet(self_ptr_local));

    func.instruction(&Instruction::LocalGet(self_param));
    func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
    func.instruction(&Instruction::I64And);
    op_emitter.const_cache.emit_qnan_tag_ptr(func);
    func.instruction(&Instruction::I64Or);
    func.instruction(&Instruction::LocalSet(self_local));

    func.instruction(&Instruction::LocalGet(self_ptr_local));
    func.instruction(&Instruction::I32WrapI64);
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_get_state"],
    );
    func.instruction(&Instruction::LocalSet(locals.state_local));
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64LtS);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I64Const(-1));
    func.instruction(&Instruction::I64Xor);
    func.instruction(&Instruction::LocalSet(locals.state_local));
    func.instruction(&Instruction::Else);
    if let Some(remap_entries) = state_remap_table_entries {
        let remap_base_local = locals
            .state_remap_base_local
            .expect("state remap base local missing for stateful wasm");
        let remap_value_local = locals
            .state_remap_value_local
            .expect("state remap value local missing for stateful wasm");
        func.instruction(&Instruction::LocalGet(locals.state_local));
        func.instruction(&Instruction::I64Const(remap_entries));
        func.instruction(&Instruction::I64LtU);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(remap_base_local));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalGet(locals.state_local));
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
        func.instruction(&Instruction::LocalSet(locals.state_local));
        func.instruction(&Instruction::End);
        func.instruction(&Instruction::End);
    } else {
        emit_sparse_state_remap_lookup(
            func,
            locals.state_local,
            sparse_state_remap_entries
                .as_deref()
                .expect("sparse state remap entries missing for stateful wasm"),
        );
    }
    func.instruction(&Instruction::End);
}

fn emit_dispatch_block_lookup(
    func: &mut Function,
    op_count: usize,
    block_count: usize,
    locals: NonLinearDispatchLocals,
) {
    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I64Const(op_count as i64));
    func.instruction(&Instruction::I64GeU);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::I64Const(block_count as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::LocalGet(locals.block_map_base_local));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(locals.state_local));
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
    func.instruction(&Instruction::LocalSet(locals.state_local));
    func.instruction(&Instruction::End);

    func.instruction(&Instruction::LocalGet(locals.state_local));
    func.instruction(&Instruction::I32WrapI64);
    let targets: Vec<u32> = (0..block_count).map(|idx| idx as u32).collect();
    func.instruction(&Instruction::BrTable(targets.into(), block_count as u32));
    func.instruction(&Instruction::End);
}

fn emit_state_transition(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let future = op_emitter.locals[&args[0]];
    let (slot_bits, pending_state) = if args.len() == 2 {
        (None, op_emitter.locals[&args[1]])
    } else {
        (
            Some(op_emitter.locals[&args[1]]),
            op_emitter.locals[&args[2]],
        )
    };
    let pending_state_name = if args.len() == 2 { &args[1] } else { &args[2] };
    let pending_target_idx = pending_encoded_target(plan, pending_state_name);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals[op.out.as_ref().unwrap()];
    let next_block = idx + 1;
    let return_depth = depth + 2;

    func.instruction(&Instruction::I64Const(next_block as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    emit_obj_set_state_arg(func, locals);
    emit_pending_state_value(func, pending_state, pending_target_idx);
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::LocalGet(future));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["future_poll"],
    );
    func.instruction(&Instruction::LocalSet(out));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::LocalSet(locals.return_local));
    func.instruction(&Instruction::LocalGet(out));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::LocalGet(
        locals.self_ptr_local.expect("stateful self ptr missing"),
    ));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(future));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["handle_resolve"],
    );
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["sleep_register"],
    );
    func.instruction(&Instruction::Drop);
    func.instruction(&Instruction::Br(return_depth));
    func.instruction(&Instruction::End);
    if let Some(slot) = slot_bits {
        emit_obj_set_state_arg(func, locals);
        func.instruction(&Instruction::LocalGet(slot));
        func.instruction(&Instruction::I64Const(INT_MASK as i64));
        func.instruction(&Instruction::I64And);
        func.instruction(&Instruction::LocalGet(out));
        emit_call(
            func,
            op_emitter.reloc_enabled,
            op_emitter.import_ids["closure_store"],
        );
        func.instruction(&Instruction::Drop);
    }
    emit_obj_set_state_arg(func, locals);
    func.instruction(&Instruction::I64Const(next_state_id));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::Br(depth));
}

fn emit_state_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
) {
    let args = op.args.as_ref().unwrap();
    let pair = op_emitter.locals[&args[0]];
    let resume_state_id = op.value.unwrap();
    let resume_encoded = plan
        .state_resume
        .as_ref()
        .and_then(|resume| resume.state_map.get(&resume_state_id).copied())
        .map(|target_idx| !(target_idx as i64));
    func.instruction(&Instruction::I64Const((idx + 1) as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    emit_obj_set_state_arg(func, locals);
    if let Some(encoded) = resume_encoded {
        func.instruction(&Instruction::I64Const(encoded));
    } else {
        func.instruction(&Instruction::I64Const(resume_state_id));
    }
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::LocalGet(pair));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["inc_ref_obj"],
    );
    func.instruction(&Instruction::LocalGet(pair));
    func.instruction(&Instruction::Return);
}

fn emit_chan_send_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let chan = op_emitter.locals[&args[0]];
    let val = op_emitter.locals[&args[1]];
    let pending_state = op_emitter.locals[&args[2]];
    let pending_target_idx = pending_encoded_target(plan, &args[2]);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals[op.out.as_ref().unwrap()];
    emit_prepare_pending_yield(
        func,
        op_emitter,
        locals,
        idx,
        pending_state,
        pending_target_idx,
    );
    func.instruction(&Instruction::LocalGet(chan));
    func.instruction(&Instruction::LocalGet(val));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["chan_send"],
    );
    emit_finish_channel_yield(func, op_emitter, locals, out, next_state_id, depth);
}

fn emit_chan_recv_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let chan = op_emitter.locals[&args[0]];
    let pending_state = op_emitter.locals[&args[1]];
    let pending_target_idx = pending_encoded_target(plan, &args[1]);
    let next_state_id = op.value.unwrap();
    let out = op_emitter.locals[op.out.as_ref().unwrap()];
    emit_prepare_pending_yield(
        func,
        op_emitter,
        locals,
        idx,
        pending_state,
        pending_target_idx,
    );
    func.instruction(&Instruction::LocalGet(chan));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["chan_recv"],
    );
    emit_finish_channel_yield(func, op_emitter, locals, out, next_state_id, depth);
}

fn emit_prepare_pending_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    locals: NonLinearDispatchLocals,
    idx: usize,
    pending_state: u32,
    pending_target_idx: Option<i64>,
) {
    func.instruction(&Instruction::I64Const((idx + 1) as i64));
    func.instruction(&Instruction::LocalSet(locals.state_local));
    emit_obj_set_state_arg(func, locals);
    emit_pending_state_value(func, pending_state, pending_target_idx);
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
}

fn emit_finish_channel_yield(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    locals: NonLinearDispatchLocals,
    out: u32,
    next_state_id: i64,
    depth: u32,
) {
    func.instruction(&Instruction::LocalSet(out));
    func.instruction(&Instruction::LocalGet(out));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::I64Eq);
    func.instruction(&Instruction::If(BlockType::Empty));
    func.instruction(&Instruction::I64Const(box_pending()));
    func.instruction(&Instruction::Return);
    func.instruction(&Instruction::End);
    emit_obj_set_state_arg(func, locals);
    func.instruction(&Instruction::I64Const(next_state_id));
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["obj_set_state"],
    );
    func.instruction(&Instruction::Br(depth));
}

fn emit_dispatch_if(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let cond = op_emitter.locals[&args[0]];
    let else_idx = plan.control_maps.else_for_if.get(&idx).copied();
    let end_idx = plan
        .control_maps
        .end_for_if
        .get(&idx)
        .copied()
        .unwrap_or_else(|| {
            dispatch_control_panic(&op_emitter.func_ir.name, idx, "if without end_if")
        });
    let false_target = if let Some(else_pos) = else_idx {
        else_pos + 1
    } else {
        end_idx + 1
    };
    let truthy_import =
        if wasm_scalar_truthiness_fast_path_for_name(op_emitter.scalar_plan, &args[0]) {
            "is_truthy_int"
        } else {
            "is_truthy"
        };
    emit_branch_truthiness_i32(
        func,
        cond,
        op_emitter.import_ids[truthy_import],
        op_emitter.reloc_enabled,
    );
    emit_conditional_state_branch(func, locals.state_local, idx + 1, false_target, depth + 1);
}

fn emit_dispatch_loop_break_cond(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
    invert: bool,
) {
    let args = op.args.as_ref().unwrap();
    let cond = op_emitter.locals[&args[0]];
    let end_idx = loop_break_target(plan, op_emitter.func_ir, idx, op.kind.as_str());
    let end_block = end_idx + 1;
    let next_block = idx + 1;
    emit_branch_truthiness_i32(
        func,
        cond,
        op_emitter.import_ids["is_truthy"],
        op_emitter.reloc_enabled,
    );
    if invert {
        func.instruction(&Instruction::I32Eqz);
    }
    emit_conditional_state_branch(func, locals.state_local, end_block, next_block, depth + 1);
}

fn emit_dispatch_check_exception(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
    exception_regions: &BTreeSet<usize>,
) {
    if op_emitter.native_eh_enabled || exception_regions.contains(&idx) {
        emit_set_state_and_br(func, locals.state_local, idx + 1, depth);
        return;
    }
    let target_label = op.value.unwrap_or_else(|| {
        dispatch_control_panic(
            &op_emitter.func_ir.name,
            idx,
            "check_exception missing label",
        )
    });
    let target_idx = label_target(
        plan,
        op_emitter.func_ir,
        idx,
        target_label,
        "check_exception",
    );
    emit_call(
        func,
        op_emitter.reloc_enabled,
        op_emitter.import_ids["exception_pending"],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_conditional_state_branch(func, locals.state_local, target_idx, idx + 1, depth + 1);
}

fn emit_conditional_state_branch(
    func: &mut Function,
    state_local: u32,
    true_state: usize,
    false_state: usize,
    branch_depth: u32,
) {
    func.instruction(&Instruction::If(BlockType::Empty));
    emit_set_state_and_br(func, state_local, true_state, branch_depth);
    func.instruction(&Instruction::Else);
    emit_set_state_and_br(func, state_local, false_state, branch_depth);
    func.instruction(&Instruction::End);
}

fn emit_set_state_and_br(func: &mut Function, state_local: u32, state: usize, depth: u32) {
    func.instruction(&Instruction::I64Const(state as i64));
    func.instruction(&Instruction::LocalSet(state_local));
    func.instruction(&Instruction::Br(depth));
}

fn emit_obj_set_state_arg(func: &mut Function, locals: NonLinearDispatchLocals) {
    func.instruction(&Instruction::LocalGet(
        locals.self_ptr_local.expect("stateful self ptr missing"),
    ));
    func.instruction(&Instruction::I32WrapI64);
}

fn emit_pending_state_value(
    func: &mut Function,
    pending_state: u32,
    pending_target_idx: Option<i64>,
) {
    if let Some(pending_encoded) = pending_target_idx {
        func.instruction(&Instruction::I64Const(pending_encoded));
    } else {
        func.instruction(&Instruction::LocalGet(pending_state));
        func.instruction(&Instruction::I64Const(INT_MASK as i64));
        func.instruction(&Instruction::I64And);
    }
}

fn pending_encoded_target(plan: &NonLinearDispatchPlan, pending_state_name: &str) -> Option<i64> {
    let resume = plan
        .state_resume
        .as_ref()
        .expect("state resume maps missing for stateful wasm");
    resume
        .const_ints
        .get(pending_state_name)
        .and_then(|state_id| resume.state_map.get(state_id).copied())
        .map(|idx| !(idx as i64))
}

fn loop_break_target(
    plan: &NonLinearDispatchPlan,
    func_ir: &FunctionIR,
    idx: usize,
    kind: &str,
) -> usize {
    plan.control_maps
        .loop_break_target
        .get(&idx)
        .copied()
        .unwrap_or_else(|| {
            dispatch_control_panic(&func_ir.name, idx, format_args!("{kind} without loop"))
        })
}

fn label_target(
    plan: &NonLinearDispatchPlan,
    func_ir: &FunctionIR,
    idx: usize,
    label: i64,
    kind: &str,
) -> usize {
    plan.control_maps
        .label_to_index
        .get(&label)
        .copied()
        .unwrap_or_else(|| {
            dispatch_control_panic(
                &func_ir.name,
                idx,
                format_args!("unknown {kind} label {label}"),
            )
        })
}

fn require_stateful(mode: DispatchMode, func_ir: &FunctionIR, idx: usize, op: &OpIR) {
    if mode == DispatchMode::Stateful {
        return;
    }
    dispatch_control_panic(
        &func_ir.name,
        idx,
        format_args!("jumpful path hit stateful op {}", op.kind),
    );
}

fn emit_arena_free(func: &mut Function, op_emitter: &WasmFunctionEmitContext<'_, '_>) {
    if let Some(arena_idx) = op_emitter.arena_local {
        func.instruction(&Instruction::LocalGet(arena_idx));
        emit_call(
            func,
            op_emitter.reloc_enabled,
            op_emitter.import_ids["arena_free"],
        );
    }
}

fn emit_dispatch_trailing_return(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    locals: NonLinearDispatchLocals,
    mode: DispatchMode,
) {
    func.instruction(&Instruction::Br(0));
    func.instruction(&Instruction::End);
    if mode == DispatchMode::Stateful {
        op_emitter.const_cache.emit_none(func);
        func.instruction(&Instruction::LocalSet(locals.return_local));
        func.instruction(&Instruction::End);
        emit_arena_free(func, op_emitter);
        func.instruction(&Instruction::LocalGet(locals.return_local));
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    } else {
        emit_arena_free(func, op_emitter);
        op_emitter.const_cache.emit_none(func);
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    }
}
