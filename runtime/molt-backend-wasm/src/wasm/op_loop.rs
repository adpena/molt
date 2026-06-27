use super::constant_ops::{ConstantOpContext, emit_constant_op};
use super::context::CompileFuncContext;
use super::*;

mod call_ops;
mod core_runtime_ops;
mod local_state_ops;
mod numeric_ops;
mod object_attr_ops;
mod runtime_service_ops;

use call_ops::{CallOpContext, CallOpEmission, emit_call_op};
use core_runtime_ops::emit_core_runtime_op;
use local_state_ops::emit_local_state_op;
use numeric_ops::emit_numeric_op;
use object_attr_ops::emit_object_attr_op;
use runtime_service_ops::{RuntimeServiceOpContext, emit_runtime_service_op};

#[derive(Clone, Copy)]
pub(super) enum ControlKind {
    Block,
    Loop,
    If,
    Try,
}

pub(super) struct WasmFunctionEmitContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) func_ir: &'a FunctionIR,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) func_map: &'a BTreeMap<String, u32>,
    pub(super) func_indices: &'a BTreeMap<String, u32>,
    pub(super) trampoline_map: &'a BTreeMap<String, u32>,
    pub(super) table_base: u32,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) closure_functions: &'a BTreeSet<String>,
    pub(super) runtime_lookup_only_vars: &'a BTreeSet<String>,
    pub(super) seeded_runtime_const_op_indices: &'a BTreeSet<usize>,
    pub(super) exception_handler_region_indices: &'a BTreeSet<usize>,
    pub(super) locals: &'a BTreeMap<String, u32>,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) scalar_plan: &'a ScalarRepresentationPlan,
    pub(super) multi_return_candidates: &'a BTreeMap<String, usize>,
    pub(super) is_multi_return_callee: Option<usize>,
    pub(super) multi_ret_locals: &'a [u32],
    pub(super) multi_ret_tuple_vars: &'a BTreeSet<String>,
    pub(super) multi_ret_call_locals: &'a BTreeMap<(String, i64), u32>,
    pub(super) multi_ret_call_vars: &'a BTreeSet<String>,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
    pub(super) tail_call_eligible: bool,
    pub(super) arena_local: Option<u32>,
    pub(super) tail_call_count: &'a Cell<usize>,
}

impl<'a, 'ctx> WasmFunctionEmitContext<'a, 'ctx> {
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
        let func_map = self.func_map;
        let func_indices = self.func_indices;
        let trampoline_map = self.trampoline_map;
        let table_base = self.table_base;
        let import_ids = self.import_ids;
        let closure_functions = self.closure_functions;
        let runtime_lookup_only_vars = self.runtime_lookup_only_vars;
        let seeded_runtime_const_op_indices = self.seeded_runtime_const_op_indices;
        let exception_handler_region_indices = self.exception_handler_region_indices;
        let locals = self.locals;
        let const_cache = self.const_cache;
        let scalar_plan = self.scalar_plan;
        let multi_return_candidates = self.multi_return_candidates;
        let is_multi_return_callee = self.is_multi_return_callee;
        let multi_ret_locals = self.multi_ret_locals;
        let multi_ret_tuple_vars = self.multi_ret_tuple_vars;
        let multi_ret_call_locals = self.multi_ret_call_locals;
        let multi_ret_call_vars = self.multi_ret_call_vars;
        let func_index = self.func_index;
        let reloc_enabled = self.reloc_enabled;
        let native_eh_enabled = self.native_eh_enabled;
        let tail_call_eligible = self.tail_call_eligible;
        let arena_local = self.arena_local;
        let tail_call_count = self.tail_call_count;

        // --- RC coalescing: eliminate redundant inc_ref/dec_ref pairs ---
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
        // Peephole state: track WASM locals whose raw (unboxed) integer
        // value is known at compile time.  Populated by `const` ops;
        // invalidated when a local is overwritten by a non-const op or
        // control flow diverges.
        let mut known_raw_ints: BTreeMap<u32, i64> = BTreeMap::new();

        // Tail call skip flag: when we emit a return_call for a
        // call_internal op, we set this to skip the immediately
        // following `ret` op that is now subsumed.
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
                is_multi_return_callee,
                multi_ret_locals,
                multi_ret_tuple_vars,
                multi_ret_call_locals,
                multi_ret_call_vars,
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
                ctx,
                func_map,
                func_indices,
                trampoline_map,
                table_base,
                import_ids,
                closure_functions,
                runtime_lookup_only_vars,
                locals,
                const_cache,
                multi_return_candidates,
                multi_ret_call_locals,
                multi_ret_call_vars,
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
                    func_map,
                    table_base,
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

            match op.kind.as_str() {
                "ret" => {
                    let ret_var = op.var.as_ref();
                    // Multi-value return (Section 3.1): push individual
                    // __multi_ret_N locals instead of the tuple handle.
                    if is_multi_return_callee.is_some()
                        && ret_var.is_some_and(|v| multi_ret_tuple_vars.contains(v))
                        && !multi_ret_locals.is_empty()
                    {
                        for &local_idx in multi_ret_locals {
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
                    // Scope arena teardown: free the per-function arena
                    // before returning. `arena_free` is `(i64) -> ()` so
                    // it consumes the handle without disturbing the
                    // return value already on the operand stack.
                    if let Some(arena_idx) = arena_local {
                        func.instruction(&Instruction::LocalGet(arena_idx));
                        emit_call(func, reloc_enabled, import_ids["arena_free"]);
                    }
                    func.instruction(&Instruction::Return);
                }
                "ret_void" => {
                    if let Some(arena_idx) = arena_local {
                        func.instruction(&Instruction::LocalGet(arena_idx));
                        emit_call(func, reloc_enabled, import_ids["arena_free"]);
                    }
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
                    let truthy_import =
                        if wasm_scalar_truthiness_fast_path_for_name(&scalar_plan, &args[0]) {
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
                    // Block+Loop already emitted by preceding loop_start;
                    // do NOT push a second Block+Loop pair here.
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
                    // Find depth to the enclosing Block that wraps the Loop.
                    let mut depth = 0u32;
                    let mut found_loop = false;
                    for entry in control_stack.iter().rev() {
                        match entry {
                            ControlKind::Block if found_loop => break,
                            ControlKind::Loop => {
                                found_loop = true;
                            }
                            _ => {}
                        }
                        depth += 1;
                    }
                    func.instruction(&Instruction::BrIf(depth));
                }
                "loop_break_if_false" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = locals[&args[0]];
                    emit_branch_truthiness_i32(func, cond, import_ids["is_truthy"], reloc_enabled);
                    // Break when the condition is *falsy*: invert truthiness.
                    func.instruction(&Instruction::I32Eqz);
                    // Find depth to the enclosing Block that wraps the Loop.
                    let mut depth = 0u32;
                    let mut found_loop = false;
                    for entry in control_stack.iter().rev() {
                        match entry {
                            ControlKind::Block if found_loop => break,
                            ControlKind::Loop => {
                                found_loop = true;
                            }
                            _ => {}
                        }
                        depth += 1;
                    }
                    func.instruction(&Instruction::BrIf(depth));
                }
                "loop_break_if_exception" => {
                    // Value-less conditional break: exit the loop when a
                    // runtime exception is pending.  Emitted after ITER_NEXT
                    // in iterator-consumer loops compiled WITHOUT the function
                    // exception stack, where the consumption loop is driven
                    // off the done flag alone and would otherwise spin forever
                    // (OOM) when the producer raises mid-iteration (it returns
                    // the None sentinel, so `done` never becomes truthy).
                    //
                    // Reads the same sacrosanct `exception_pending` flag the
                    // WASM `check_exception` lowering uses, compares `!= 0`,
                    // and breaks to the enclosing Block that wraps the Loop —
                    // identical depth resolution to `loop_break_if_true`.  The
                    // still-pending exception then rides up the lazy-return
                    // path to the caller's handler.
                    emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                    func.instruction(&Instruction::I64Const(0));
                    func.instruction(&Instruction::I64Ne);
                    let mut depth = 0u32;
                    let mut found_loop = false;
                    for entry in control_stack.iter().rev() {
                        match entry {
                            ControlKind::Block if found_loop => break,
                            ControlKind::Loop => {
                                found_loop = true;
                            }
                            _ => {}
                        }
                        depth += 1;
                    }
                    func.instruction(&Instruction::BrIf(depth));
                }
                "loop_break" => {
                    // Find depth to the enclosing Block that wraps the Loop.
                    // The loop structure is Block { Loop { ... } }, so we
                    // need to find the Block that immediately precedes
                    // the innermost Loop on the control stack.
                    let mut depth = 0u32;
                    let mut found_loop = false;
                    for entry in control_stack.iter().rev() {
                        match entry {
                            ControlKind::Block if found_loop => break,
                            ControlKind::Loop => {
                                found_loop = true;
                            }
                            _ => {}
                        }
                        depth += 1;
                    }
                    func.instruction(&Instruction::Br(depth));
                }
                "loop_continue" => {
                    // Find depth to the innermost Loop on the control stack.
                    let mut depth = 0u32;
                    for entry in control_stack.iter().rev() {
                        if matches!(entry, ControlKind::Loop) {
                            break;
                        }
                        depth += 1;
                    }
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
                        // Native EH: two-level block for try_table:
                        //   block $catch_dest (result i64)
                        //     try_table (catch $molt_exception $catch_dest)
                        //       ... body ...
                        //     end
                        //     i64.const <box_none>  ;; normal path sentinel
                        //   end
                        //   ;; catch: exception handle on stack
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
                        // Close try_table
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                        try_stack.pop();
                        // Normal path: push None sentinel for outer block result
                        const_cache.emit_none(func);
                        // Close outer catch-destination block
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                        // Drop the i64 result (exception handle or sentinel)
                        func.instruction(&Instruction::Drop);
                    } else {
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                        try_stack.pop();
                    }
                }
                "check_exception" => {
                    if native_eh_enabled {
                        // Native EH: no-op; WASM catches automatically.
                    } else if exception_handler_region_indices.contains(&op_idx) {
                        // Handler bodies work against the currently pending
                        // exception. Re-polling before exception_clear would
                        // re-branch out of the handler and skip its body.
                    } else if let Some(&try_index) = try_stack.last() {
                        emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        let depth = control_stack.len().saturating_sub(1 + try_index);
                        func.instruction(&Instruction::BrIf(depth as u32));
                    }
                }
                kind if is_shared_drop_fact_marker(kind) => {
                    // Shared TIR drop-fact markers are compile-time
                    // evidence only. WASM consumes the materialized
                    // inc_ref/dec_ref/release ops, so marker ops must be
                    // explicit no-ops instead of falling through the
                    // unknown-op default.
                }
                _ => {
                    dispatch_control_panic(
                        &func_ir.name,
                        op_idx,
                        format_args!("unsupported op kind `{}`", op.kind),
                    );
                }
            }

            // --- Peephole: invalidate known_raw_ints tracking ---
            // Control-flow ops make compile-time value tracking
            // unreliable across branches; clear everything.
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
                // `const` already recorded its value above; skip invalidation.
                "const" => {}
                // All other ops: invalidate only the output local (if any),
                // since only that local's value changed.
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
