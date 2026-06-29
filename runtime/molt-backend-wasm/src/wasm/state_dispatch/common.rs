use super::super::control_flow::dispatch_control_panic;
use super::super::op_loop::WasmFunctionEmitContext;
use super::DispatchMode;
use super::plan::{NonLinearDispatchLocals, NonLinearDispatchPlan};
use super::state_remap::{build_sparse_state_remap_entries, emit_sparse_state_remap_lookup};
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_plan::wasm_scalar_truthiness_fast_path_for_name;
use crate::wasm_values::{INT_MASK, POINTER_MASK, emit_branch_truthiness_i32};
use crate::{FunctionIR, OpIR};
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{BlockType, Function, Instruction};

pub(in crate::wasm) fn exception_handler_region_indices(ops: &[OpIR]) -> BTreeSet<usize> {
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

pub(super) fn exception_handler_region_indices_from_label_map(
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

pub(super) fn emit_stateful_resume_prelude(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
) {
    let self_ptr_local = locals
        .self_ptr_local
        .expect("self ptr local missing for stateful wasm");
    let self_param = *op_emitter
        .locals()
        .get(WasmFrameLocals::SELF_PARAM_NAME)
        .expect("self_param missing for stateful wasm");
    let self_local = *op_emitter
        .locals()
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
    op_emitter.const_cache().emit_qnan_tag_ptr(func);
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

pub(super) fn emit_dispatch_if(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    plan: &NonLinearDispatchPlan,
    locals: NonLinearDispatchLocals,
    op: &OpIR,
    idx: usize,
    depth: u32,
) {
    let args = op.args.as_ref().unwrap();
    let cond = op_emitter.locals()[&args[0]];
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
        if wasm_scalar_truthiness_fast_path_for_name(op_emitter.scalar_plan(), &args[0]) {
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

pub(super) fn emit_dispatch_loop_break_cond(
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
    let cond = op_emitter.locals()[&args[0]];
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

pub(super) fn emit_dispatch_check_exception(
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

pub(super) fn emit_conditional_state_branch(
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

pub(super) fn emit_set_state_and_br(
    func: &mut Function,
    state_local: u32,
    state: usize,
    depth: u32,
) {
    func.instruction(&Instruction::I64Const(state as i64));
    func.instruction(&Instruction::LocalSet(state_local));
    func.instruction(&Instruction::Br(depth));
}

pub(super) fn emit_obj_set_state_arg(func: &mut Function, locals: NonLinearDispatchLocals) {
    func.instruction(&Instruction::LocalGet(
        locals.self_ptr_local.expect("stateful self ptr missing"),
    ));
    func.instruction(&Instruction::I32WrapI64);
}

pub(super) fn emit_pending_state_value(
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

pub(super) fn loop_break_target(
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

pub(super) fn label_target(
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

pub(super) fn require_stateful(mode: DispatchMode, func_ir: &FunctionIR, idx: usize, op: &OpIR) {
    if mode == DispatchMode::Stateful {
        return;
    }
    dispatch_control_panic(
        &func_ir.name,
        idx,
        format_args!("jumpful path hit stateful op {}", op.kind),
    );
}

pub(super) fn emit_arena_free(func: &mut Function, op_emitter: &WasmFunctionEmitContext<'_, '_>) {
    if let Some(arena_idx) = op_emitter.arena_local() {
        func.instruction(&Instruction::LocalGet(arena_idx));
        emit_call(
            func,
            op_emitter.reloc_enabled,
            op_emitter.import_ids["arena_free"],
        );
    }
}

pub(super) fn emit_dispatch_trailing_return(
    func: &mut Function,
    op_emitter: &WasmFunctionEmitContext<'_, '_>,
    locals: NonLinearDispatchLocals,
    mode: DispatchMode,
) {
    func.instruction(&Instruction::Br(0));
    func.instruction(&Instruction::End);
    if mode == DispatchMode::Stateful {
        op_emitter.const_cache().emit_none(func);
        func.instruction(&Instruction::LocalSet(locals.return_local));
        func.instruction(&Instruction::End);
        emit_arena_free(func, op_emitter);
        func.instruction(&Instruction::LocalGet(locals.return_local));
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    } else {
        emit_arena_free(func, op_emitter);
        op_emitter.const_cache().emit_none(func);
        func.instruction(&Instruction::Return);
        func.instruction(&Instruction::End);
    }
}
