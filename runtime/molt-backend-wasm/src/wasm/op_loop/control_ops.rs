use super::super::control_flow::{
    ControlKind, dispatch_control_panic, loop_break_depth, loop_continue_depth,
};
use super::super::multi_return_layout::WasmMultiReturnLayout;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi::TAG_EXCEPTION_INDEX;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::{
    is_shared_drop_fact_marker, wasm_scalar_truthiness_fast_path_for_name,
};
use crate::wasm_values::{ConstantCache, emit_branch_truthiness_i32};
use crate::{FunctionIR, OpIR};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{BlockType, Catch, Function, Instruction, ValType};

pub(super) struct ControlOpContext<'a> {
    pub(super) func_ir: &'a FunctionIR,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) scalar_plan: &'a ScalarRepresentationPlan,
    pub(super) multi_return: &'a WasmMultiReturnLayout,
    pub(super) exception_handler_region_indices: &'a BTreeSet<usize>,
    pub(super) control_stack: &'a mut Vec<ControlKind>,
    pub(super) try_stack: &'a mut Vec<usize>,
    pub(super) label_stack: &'a mut Vec<i64>,
    pub(super) label_depths: &'a mut BTreeMap<i64, usize>,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
    pub(super) arena_local: Option<u32>,
    pub(super) op_idx: usize,
}

pub(super) fn emit_control_op(control_ctx: ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let ControlOpContext {
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
    } = control_ctx;

    match op.kind.as_str() {
        "ret" => emit_ret(
            func,
            func_ir,
            op,
            import_ids,
            locals,
            multi_return,
            reloc_enabled,
            arena_local,
            op_idx,
        ),
        "ret_void" => {
            emit_arena_free(func, import_ids, reloc_enabled, arena_local);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::Return);
        }
        "jump" => {
            let target = op.value.expect("jump missing label");
            let depth = label_depths
                .get(&target)
                .map(|idx| control_stack.len().saturating_sub(1 + idx))
                .unwrap_or_else(|| panic!("jump target {} missing label block", target));
            func.instruction(&Instruction::Br(depth as u32));
        }
        "br_if" => {
            let args = op.args.as_ref().unwrap();
            let cond = locals[&args[0]];
            let target = op.value.expect("br_if missing label");
            let depth = label_depths
                .get(&target)
                .map(|idx| control_stack.len().saturating_sub(1 + idx))
                .unwrap_or_else(|| panic!("br_if target {} missing label block", target));
            emit_branch_truthiness_i32(func, cond, import_ids["is_truthy"], reloc_enabled);
            func.instruction(&Instruction::BrIf(depth as u32));
        }
        "if" => {
            let args = op.args.as_ref().unwrap();
            let cond = locals[&args[0]];
            let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(&scalar_plan, &args[0])
            {
                "is_truthy_int"
            } else {
                "is_truthy"
            };
            emit_branch_truthiness_i32(func, cond, import_ids[truthy_import], reloc_enabled);
            func.instruction(&Instruction::If(BlockType::Empty));
            control_stack.push(ControlKind::If);
        }
        "label" => {
            if let Some(label_id) = op.value
                && let Some(top) = label_stack.last().copied()
                && top == label_id
            {
                label_stack.pop();
                label_depths.remove(&label_id);
                func.instruction(&Instruction::End);
                control_stack.pop();
            }
        }
        "else" => {
            func.instruction(&Instruction::Else);
        }
        "end_if" => {
            func.instruction(&Instruction::End);
            control_stack.pop();
        }
        "loop_start" => {
            func.instruction(&Instruction::Block(BlockType::Empty));
            func.instruction(&Instruction::Loop(BlockType::Empty));
            control_stack.push(ControlKind::Block);
            control_stack.push(ControlKind::Loop);
        }
        "loop_index_start" => {
            let args = op.args.as_ref().unwrap();
            let start = locals[&args[0]];
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalSet(out));
        }
        "loop_index_next" => {
            let args = op.args.as_ref().unwrap();
            let next_idx = locals[&args[0]];
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalGet(next_idx));
            func.instruction(&Instruction::LocalSet(out));
        }
        "loop_break_if_true" => {
            let args = op.args.as_ref().unwrap();
            let cond = locals[&args[0]];
            emit_branch_truthiness_i32(func, cond, import_ids["is_truthy"], reloc_enabled);
            emit_loop_break(func, control_stack, func_ir, op_idx, "loop_break_if_true");
        }
        "loop_break_if_false" => {
            let args = op.args.as_ref().unwrap();
            let cond = locals[&args[0]];
            emit_branch_truthiness_i32(func, cond, import_ids["is_truthy"], reloc_enabled);
            func.instruction(&Instruction::I32Eqz);
            emit_loop_break(func, control_stack, func_ir, op_idx, "loop_break_if_false");
        }
        "loop_break_if_exception" => {
            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            emit_loop_break(
                func,
                control_stack,
                func_ir,
                op_idx,
                "loop_break_if_exception",
            );
        }
        "loop_break" => {
            let depth = loop_break_depth(control_stack).unwrap_or_else(|| {
                dispatch_control_panic(&func_ir.name, op_idx, "loop_break without loop")
            });
            func.instruction(&Instruction::Br(depth));
        }
        "loop_continue" => {
            let depth = loop_continue_depth(control_stack).unwrap_or_else(|| {
                dispatch_control_panic(&func_ir.name, op_idx, "loop_continue without loop")
            });
            func.instruction(&Instruction::Br(depth));
        }
        "loop_end" => {
            func.instruction(&Instruction::End);
            func.instruction(&Instruction::End);
            control_stack.pop();
            control_stack.pop();
        }
        "try_start" => {
            if native_eh_enabled {
                func.instruction(&Instruction::Block(BlockType::Result(ValType::I64)));
                control_stack.push(ControlKind::Block);
                func.instruction(&Instruction::TryTable(
                    BlockType::Empty,
                    Cow::Borrowed(&[Catch::One {
                        tag: TAG_EXCEPTION_INDEX,
                        label: 0,
                    }]),
                ));
                control_stack.push(ControlKind::Try);
                try_stack.push(control_stack.len() - 1);
            } else {
                func.instruction(&Instruction::Block(BlockType::Empty));
                control_stack.push(ControlKind::Try);
                try_stack.push(control_stack.len() - 1);
            }
        }
        "try_end" => {
            if native_eh_enabled {
                func.instruction(&Instruction::End);
                control_stack.pop();
                try_stack.pop();
                const_cache.emit_none(func);
                func.instruction(&Instruction::End);
                control_stack.pop();
                func.instruction(&Instruction::Drop);
            } else {
                func.instruction(&Instruction::End);
                control_stack.pop();
                try_stack.pop();
            }
        }
        "check_exception" => {
            if native_eh_enabled {
            } else if exception_handler_region_indices.contains(&op_idx) {
            } else if let Some(&try_index) = try_stack.last() {
                emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                let depth = control_stack.len().saturating_sub(1 + try_index);
                func.instruction(&Instruction::BrIf(depth as u32));
            }
        }
        kind if is_shared_drop_fact_marker(kind) => {}
        _ => {
            dispatch_control_panic(
                &func_ir.name,
                op_idx,
                format_args!("unsupported op kind `{}`", op.kind),
            );
        }
    }
}

fn emit_ret(
    func: &mut Function,
    func_ir: &FunctionIR,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    multi_return: &WasmMultiReturnLayout,
    reloc_enabled: bool,
    arena_local: Option<u32>,
    op_idx: usize,
) {
    let ret_var = op.var.as_ref();
    let callee_value_locals = multi_return.callee_value_locals();
    if ret_var.is_some_and(|v| multi_return.is_callee_tuple_var(v))
        && !callee_value_locals.is_empty()
    {
        for &local_idx in callee_value_locals {
            func.instruction(&Instruction::LocalGet(local_idx));
        }
    } else {
        let ret_local = ret_var.and_then(|name| locals.get(name).copied());
        if let Some(local_idx) = ret_local {
            func.instruction(&Instruction::LocalGet(local_idx));
        } else {
            dispatch_control_panic(
                &func_ir.name,
                op_idx,
                format_args!("ret target local {:?} is not present", op.var),
            );
        }
    }
    emit_arena_free(func, import_ids, reloc_enabled, arena_local);
    func.instruction(&Instruction::Return);
}

fn emit_arena_free(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    arena_local: Option<u32>,
) {
    if let Some(arena_idx) = arena_local {
        func.instruction(&Instruction::LocalGet(arena_idx));
        emit_call(func, reloc_enabled, import_ids["arena_free"]);
    }
}

fn emit_loop_break(
    func: &mut Function,
    control_stack: &[ControlKind],
    func_ir: &FunctionIR,
    op_idx: usize,
    kind: &str,
) {
    let depth = loop_break_depth(control_stack).unwrap_or_else(|| {
        dispatch_control_panic(&func_ir.name, op_idx, format_args!("{kind} without loop"))
    });
    func.instruction(&Instruction::BrIf(depth));
}
