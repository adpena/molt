use super::context::CompileFuncContext;
use super::*;

mod core_runtime_ops;
mod local_state_ops;
mod numeric_ops;
mod object_attr_ops;

use core_runtime_ops::emit_core_runtime_op;
use local_state_ops::emit_local_state_op;
use numeric_ops::emit_numeric_op;
use object_attr_ops::emit_object_attr_op;

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
        let live_object_locals_for_call = |rel_idx: usize, out_name: Option<&String>| -> Vec<u32> {
            let mut live = BTreeSet::new();
            for (name, &local_idx) in locals {
                if name == "none" {
                    continue;
                }
                if out_name.is_some_and(|out| out == name) {
                    continue;
                }
                if name.starts_with("__molt_tmp")
                    || name.ends_with("_ptr")
                    || name.ends_with("_len")
                {
                    continue;
                }
                if last_use_local.get(name).is_none_or(|last| *last <= rel_idx) {
                    continue;
                }
                live.insert(local_idx);
            }
            live.into_iter().collect()
        };

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

            match op.kind.as_str() {
                "const" => {
                    let val = op.value.unwrap();
                    func.instruction(&Instruction::I64Const(box_int(val)));
                    let local_idx = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(local_idx));
                    // Record the known raw value for this local so
                    // subsequent fast_int unbox can be elided.
                    known_raw_ints.insert(local_idx, val);
                }
                "const_bool" => {
                    let val = op.value.unwrap();
                    func.instruction(&Instruction::I64Const(box_bool(val)));
                    let local_idx = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(local_idx));
                }
                "const_float" => {
                    let val = op.f_value.expect("Float value not found");
                    func.instruction(&Instruction::I64Const(box_float(val)));
                    let local_idx = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(local_idx));
                }
                "const_none" => {
                    const_cache.emit_none(func);
                    let local_idx = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(local_idx));
                }
                "const_not_implemented" => {
                    emit_call(func, reloc_enabled, import_ids["not_implemented"]);
                    let local_idx = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(local_idx));
                }
                "const_ellipsis" => {
                    emit_call(func, reloc_enabled, import_ids["ellipsis"]);
                    let local_idx = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(local_idx));
                }
                "const_str" => {
                    let out_name = op.out.as_ref().unwrap();
                    let bytes = op
                        .bytes
                        .as_deref()
                        .unwrap_or_else(|| op.s_value.as_ref().unwrap().as_bytes());
                    let data = backend.add_data_segment(reloc_enabled, bytes);

                    let ptr_local = locals[&format!("{out_name}_ptr")];
                    let len_local = locals[&format!("{out_name}_len")];
                    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
                    func.instruction(&Instruction::LocalSet(ptr_local));
                    func.instruction(&Instruction::I64Const(bytes.len() as i64));
                    func.instruction(&Instruction::LocalSet(len_local));

                    // Use the fixed scratch slot in linear memory instead
                    // of heap-allocating an 8-byte buffer per const_str.
                    // This eliminates the per-string alloc(8) call, the
                    // handle_resolve round-trip, and the leaked
                    // intermediate object — saving ~48 bytes of heap per
                    // string constant and reducing heap pressure that can
                    // push the allocator into the output data region in
                    // the split-runtime layout.
                    let scratch_seg = ctx.const_str_scratch_segment;

                    // string_from_bytes(data_ptr: i32, len: i64, out: i32) -> i32
                    func.instruction(&Instruction::LocalGet(ptr_local));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::LocalGet(len_local));
                    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
                    emit_call(func, reloc_enabled, import_ids["string_from_bytes"]);
                    func.instruction(&Instruction::Drop);

                    // Load the string handle written by string_from_bytes.
                    let out_local = locals[out_name];
                    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
                    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                        align: 3,
                        offset: 0,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalSet(out_local));
                }
                "const_bigint" => {
                    let s = op.s_value.as_ref().unwrap();
                    let out_name = op.out.as_ref().unwrap();
                    let bytes = s.as_bytes();
                    let data = backend.add_data_segment(reloc_enabled, bytes);

                    let ptr_local = locals[&format!("{out_name}_ptr")];
                    let len_local = locals[&format!("{out_name}_len")];
                    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
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
                    let bytes = op.bytes.as_ref().expect("Bytes not found");
                    let out_name = op.out.as_ref().unwrap();
                    let data = backend.add_data_segment(reloc_enabled, bytes);

                    let ptr_local = locals[&format!("{out_name}_ptr")];
                    let len_local = locals[&format!("{out_name}_len")];
                    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
                    func.instruction(&Instruction::LocalSet(ptr_local));
                    func.instruction(&Instruction::I64Const(bytes.len() as i64));
                    func.instruction(&Instruction::LocalSet(len_local));

                    // Use fixed scratch slot (same as const_str).
                    let scratch_seg = ctx.const_str_scratch_segment;

                    func.instruction(&Instruction::LocalGet(ptr_local));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::LocalGet(len_local));
                    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
                    emit_call(func, reloc_enabled, import_ids["bytes_from_bytes"]);
                    func.instruction(&Instruction::Drop);

                    let out_local = locals[out_name];
                    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_seg);
                    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                        align: 3,
                        offset: 0,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalSet(out_local));
                }
                "call_async" => {
                    let payload_len = op.args.as_ref().map(|args| args.len()).unwrap_or(0);
                    let target_name = op.s_value.as_ref().expect("call_async target missing");
                    let table_slot = *func_map.get(target_name).unwrap_or_else(|| {
                        panic!("call_async table target not found: {target_name}")
                    });
                    let table_idx = table_base + table_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const((payload_len * 8) as i64));
                    func.instruction(&Instruction::I64Const(TASK_KIND_FUTURE));
                    emit_call(func, reloc_enabled, import_ids["task_new"]);
                    let res = if let Some(out) = op.out.as_ref() {
                        let r = locals[out];
                        func.instruction(&Instruction::LocalSet(r));
                        r
                    } else {
                        func.instruction(&Instruction::Drop);
                        0
                    };
                    if let Some(args) = op.args.as_ref() {
                        for (idx, arg) in args.iter().enumerate() {
                            let arg_val = locals[arg];
                            func.instruction(&Instruction::LocalGet(res));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::I32Const((idx * 8) as i32));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(arg_val));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(arg_val));
                            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        }
                    }
                }
                "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim"
                | "gpu_barrier" => {
                    let runtime_name =
                        gpu_runtime_call_symbol(op.kind.as_str()).expect("gpu runtime symbol");
                    let import_name = runtime_name.strip_prefix("molt_").unwrap_or(runtime_name);
                    let out = locals[op.out.as_ref().expect("gpu op result missing")];
                    emit_call(func, reloc_enabled, import_ids[import_name]);
                    func.instruction(&Instruction::LocalSet(out));
                }
                "call" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
                    let out = locals[op.out.as_ref().unwrap()];
                    let live_object_locals = live_object_locals_for_call(rel_idx, op.out.as_ref());
                    for local_idx in &live_object_locals {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }
                    let returns_alias_param = ctx
                        .return_alias_summaries
                        .get(target_name)
                        .and_then(|summary| match summary {
                            crate::passes::ReturnAliasSummary::Param(param_idx)
                                if *param_idx < args_names.len() =>
                            {
                                Some(*param_idx)
                            }
                            _ => None,
                        })
                        .is_some();
                    if returns_alias_param
                        && std::env::var("MOLT_DEBUG_WASM_RETURN_ALIAS").as_deref() == Ok("1")
                    {
                        eprintln!(
                            "[molt wasm return-alias] kind=call caller={} callee={}",
                            func_ir.name, target_name
                        );
                    }
                    let func_idx = *func_indices.get(target_name).unwrap_or_else(|| {
                        panic!(
                            "call target not found: '{}' in func '{}'",
                            target_name, func_ir.name
                        )
                    });
                    let bootstrap_call = func_idx == import_ids["runtime_init"];
                    if bootstrap_call {
                        for arg_name in args_names {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }
                        emit_call(func, reloc_enabled, func_idx);
                        func.instruction(&Instruction::LocalSet(out));
                        continue;
                    }
                    // Direct call: push args, call function, store result.
                    // The recursion guard + trace_enter/exit overhead
                    // was causing the return value to be lost (the
                    // if/else block left `out` as None even on the
                    // success path in some WASM engines).  Module chunk
                    // calls and devirtualized calls now use a flat
                    // sequence; CHECK_EXCEPTION after the call catches
                    // any exception the callee raises.
                    for arg_name in args_names {
                        let arg = locals[arg_name];
                        func.instruction(&Instruction::LocalGet(arg));
                    }
                    emit_call(func, reloc_enabled, func_idx);
                    if returns_alias_param {
                        func.instruction(&Instruction::LocalTee(out));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    } else {
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    for local_idx in live_object_locals.iter().rev() {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                    }
                }
                "call_internal" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
                    let out_name = op.out.as_ref().unwrap();
                    let out = locals[out_name];
                    let live_object_locals = live_object_locals_for_call(rel_idx, op.out.as_ref());
                    for local_idx in &live_object_locals {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }
                    let returns_alias_param = ctx
                        .return_alias_summaries
                        .get(target_name)
                        .and_then(|summary| match summary {
                            crate::passes::ReturnAliasSummary::Param(param_idx)
                                if *param_idx < args_names.len() =>
                            {
                                Some(*param_idx)
                            }
                            _ => None,
                        })
                        .is_some();
                    if returns_alias_param
                        && std::env::var("MOLT_DEBUG_WASM_RETURN_ALIAS").as_deref() == Ok("1")
                    {
                        eprintln!(
                            "[molt wasm return-alias] kind=call_internal caller={} callee={}",
                            func_ir.name, target_name
                        );
                    }
                    let func_idx = *func_indices
                        .get(target_name)
                        .expect("call_internal target not found");

                    // --- Tail call detection (WASM tail calls proposal §3.5) ---
                    // A call_internal is in tail position when:
                    //   1. The function is eligible (non-stateful)
                    //   2. The very next op is `ret`
                    //   3. The ret's var matches this call's output
                    //   4. There are no cleanup ops (dec_ref) between call and return
                    //   5. We are not inside a try block (return_call would
                    //      skip the exception handler)
                    //   6. Caller and callee have the same arity — return_call
                    //      requires the stack to match the callee's full param
                    //      list, which differs from call+return.
                    let is_tail_call = tail_call_eligible
                            && try_stack.is_empty()
                            && rel_idx + 1 < ops.len()
                            && ops[rel_idx + 1].kind == "ret"
                            && ops[rel_idx + 1].var.as_deref() == Some(out_name.as_str())
                            // Exclude calls to multi-return candidates: return_call
                            // would forward N values but the caller's type signature
                            // expects a single i64 return, causing an ABI mismatch.
                            && !multi_return_candidates.contains_key(target_name)
                            // Exclude chunk calls: the stub may pass fewer args than
                            // the chunk expects, causing return_call stack underflow.
                            && !target_name.contains("__molt_chunk_")
                            // Exclude calls where caller arity != callee param count.
                            // return_call requires exactly the callee's param count
                            // on the stack; a regular call+return handles mismatches.
                            && args_names.len() == func_ir.params.len();

                    // Scope arena teardown before tail call: once
                    // `return_call` replaces the current frame, the
                    // arena handle local disappears — so we must free
                    // the arena while it is still live. We do this
                    // before pushing the callee args so the operand
                    // stack discipline stays correct (`arena_free`
                    // consumes exactly its own argument).
                    if is_tail_call && let Some(arena_idx) = arena_local {
                        func.instruction(&Instruction::LocalGet(arena_idx));
                        emit_call(func, reloc_enabled, import_ids["arena_free"]);
                    }

                    for arg_name in args_names {
                        let arg = locals[arg_name];
                        func.instruction(&Instruction::LocalGet(arg));
                    }

                    if is_tail_call {
                        // Emit return_call: callee's return value becomes
                        // our return value without growing the WASM stack.
                        emit_return_call(func, reloc_enabled, func_idx);
                        tail_call_count.set(tail_call_count.get() + 1);
                        // Skip the next op (ret) since return_call subsumes it.
                        skip_next = true;
                        continue;
                    }

                    emit_call(func, reloc_enabled, func_idx);
                    // Multi-value return (Section 3.1): pop N results
                    // into dedicated locals for later tuple_index.
                    if multi_ret_call_vars.contains(out_name) {
                        let ret_count = multi_return_candidates[target_name];
                        for k in (0..ret_count).rev() {
                            let local_idx = multi_ret_call_locals[&(out_name.clone(), k as i64)];
                            func.instruction(&Instruction::LocalSet(local_idx));
                        }
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::LocalSet(out));
                    } else {
                        if returns_alias_param {
                            func.instruction(&Instruction::LocalTee(out));
                            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        } else {
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    for local_idx in live_object_locals.iter().rev() {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                    }
                }
                "inc_ref" | "borrow" => {
                    if !rc_skip_inc.contains(&rel_idx) {
                        let args_names = op.args.as_ref().expect("inc_ref/borrow args missing");
                        let src_name = args_names
                            .first()
                            .expect("inc_ref/borrow requires one source arg");
                        let src = locals[src_name];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            let out = locals[out_name];
                            func.instruction(&Instruction::LocalGet(src));
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    } else if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        // RC coalesced: still alias output to input.
                        let args_names = op.args.as_ref().unwrap();
                        let src_name = args_names.first().unwrap();
                        let src = locals[src_name];
                        let out = locals[out_name];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalSet(out));
                    }
                }
                "dec_ref" | "release" => {
                    let args_names = op.args.as_ref().expect("dec_ref/release args missing");
                    let src_name = args_names
                        .first()
                        .expect("dec_ref/release requires one source arg");
                    if !rc_skip_inc.contains(&rel_idx) && !rc_skip_dec.contains(src_name.as_str()) {
                        let src = locals[src_name];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                        if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            let out = locals[out_name];
                            const_cache.emit_none(func);
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                }
                "store_var" => {
                    let args_names = op.args.as_ref().expect("store_var args missing");
                    let src_name = args_names
                        .first()
                        .expect("store_var requires one source arg");
                    let src = locals[src_name];
                    let dst_name = op
                        .var
                        .as_ref()
                        .or(op.out.as_ref())
                        .expect("store_var requires destination");
                    let dst = locals[dst_name];
                    func.instruction(&Instruction::LocalGet(src));
                    func.instruction(&Instruction::LocalSet(dst));
                }
                "load_var" | "copy_var" | "copy" | "identity_alias" | "binding_alias" => {
                    let src_name = op
                        .var
                        .as_ref()
                        .or_else(|| op.args.as_ref().and_then(|args| args.first()))
                        .expect("load_var/copy_var requires source");
                    let src = locals[src_name];
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        // These ops create a second live alias of the
                        // source object bits. Take a new ref for the
                        // destination so later cleanup of the source
                        // name cannot invalidate the alias.
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        let out = locals[out_name];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalSet(out));
                    }
                }
                "box" | "unbox" | "cast" | "widen" => {
                    let args_names = op.args.as_ref().expect("conversion args missing");
                    let src_name = args_names
                        .first()
                        .expect("conversion op requires one source arg");
                    let src = locals[src_name];
                    func.instruction(&Instruction::LocalGet(src));
                    if let Some(out_name) = op.out.as_ref() {
                        if out_name != "none" {
                            // Output aliases input bits — inc_ref to prevent
                            // use-after-free when the input name is dec_ref'd
                            // independently by tracking/check_exception cleanup.
                            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            func.instruction(&Instruction::LocalGet(src));
                            let out = locals[out_name];
                            func.instruction(&Instruction::LocalSet(out));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "call_guarded" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
                    let callee_bits = locals[&args_names[0]];
                    let out = locals[op.out.as_ref().unwrap()];
                    let callargs_tmp = locals["__molt_tmp0"];
                    let tmp_ptr = locals["__molt_tmp1"];
                    let arity = args_names.len().saturating_sub(1);
                    let escaped_target = ctx.escaped_callable_targets.contains(target_name);
                    let func_idx = *func_indices
                        .get(target_name)
                        .expect("call_guarded target not found");
                    let table_slot = func_map[target_name];
                    let table_idx = table_base + table_slot;
                    if escaped_target {
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        emit_call(func, reloc_enabled, import_ids["is_function_obj"]);
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        let code_id = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(code_id));
                        emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                        func.instruction(&Instruction::Drop);
                        let spill_base = ctx.call_func_spill_offset;
                        for (i, arg_name) in args_names[1..].iter().enumerate() {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::I32Const(
                                (spill_base + (i as u32) * 8) as i32,
                            ));
                            func.instruction(&Instruction::LocalGet(arg));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                        }
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        func.instruction(&Instruction::I64Const(spill_base as i64));
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(code_id));
                        emit_call(func, reloc_enabled, import_ids["call_func_dispatch"]);
                        func.instruction(&Instruction::LocalSet(out));
                        emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                        func.instruction(&Instruction::Drop);
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
                        func.instruction(&Instruction::Else);
                        // Recursion guard failed — exception is already pending.
                        // Return immediately so the pending RecursionError
                        // propagates to the caller instead of being silently
                        // swallowed as None (which caused TypeError downstream).
                        const_cache.emit_none(func);
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "call_guarded_nonfunc",
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                        continue;
                    }
                    func.instruction(&Instruction::LocalGet(callee_bits));
                    emit_call(func, reloc_enabled, import_ids["is_function_obj"]);
                    emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                    func.instruction(&Instruction::I64Const(0));
                    func.instruction(&Instruction::I64Ne);
                    func.instruction(&Instruction::If(BlockType::Empty));

                    // callee is a function object: resolve and compare against expected target
                    func.instruction(&Instruction::LocalGet(callee_bits));
                    emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                    func.instruction(&Instruction::I64ExtendI32U);
                    func.instruction(&Instruction::LocalSet(tmp_ptr));
                    func.instruction(&Instruction::LocalGet(tmp_ptr));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                        align: 3,
                        offset: 0,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalSet(tmp_ptr));
                    func.instruction(&Instruction::LocalGet(tmp_ptr));
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Eq);
                    func.instruction(&Instruction::If(BlockType::Empty));

                    // fast path: callee matches expected target
                    emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
                    func.instruction(&Instruction::I64Const(0));
                    func.instruction(&Instruction::I64Ne);
                    func.instruction(&Instruction::If(BlockType::Empty));
                    let code_id = op.value.unwrap_or(0);
                    func.instruction(&Instruction::I64Const(code_id));
                    emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                    func.instruction(&Instruction::Drop);
                    // For closure functions, extract the closure environment
                    // from the callee object and push it as the leading arg.
                    // The WASM signature of closure functions is
                    //   (closure_env, arg1, arg2, …) → i64
                    // so we must prepend the env before the user arguments.
                    if closure_functions.contains(target_name) {
                        func.instruction(&Instruction::LocalGet(callee_bits));
                        emit_call(func, reloc_enabled, import_ids["function_closure_bits"]);
                    }
                    for arg_name in &args_names[1..] {
                        let arg = locals[arg_name];
                        func.instruction(&Instruction::LocalGet(arg));
                    }
                    emit_call(func, reloc_enabled, func_idx);
                    func.instruction(&Instruction::LocalSet(out));
                    emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                    func.instruction(&Instruction::Drop);
                    emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
                    func.instruction(&Instruction::Else);
                    // Recursion guard failed — exception is already pending.
                    // Return immediately so the pending RecursionError
                    // propagates to the caller instead of being silently
                    // swallowed as None (which caused TypeError downstream).
                    const_cache.emit_none(func);
                    func.instruction(&Instruction::Return);
                    func.instruction(&Instruction::End);

                    // slow path: function object does not match expected target
                    func.instruction(&Instruction::Else);
                    func.instruction(&Instruction::I64Const(arity as i64));
                    func.instruction(&Instruction::I64Const(0));
                    emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                    func.instruction(&Instruction::LocalSet(callargs_tmp));
                    for arg_name in &args_names[1..] {
                        let arg = locals[arg_name];
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                        func.instruction(&Instruction::Drop);
                    }
                    let site_bits = box_int(stable_ic_site_id(
                        func_ir.name.as_str(),
                        op_idx,
                        "call_guarded_slow_match_miss",
                    ));
                    func.instruction(&Instruction::I64Const(site_bits));
                    func.instruction(&Instruction::LocalGet(callee_bits));
                    func.instruction(&Instruction::LocalGet(callargs_tmp));
                    emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                    func.instruction(&Instruction::LocalSet(out));
                    func.instruction(&Instruction::End);

                    // not a function object: fallback to call_bind
                    func.instruction(&Instruction::Else);
                    func.instruction(&Instruction::I64Const(arity as i64));
                    func.instruction(&Instruction::I64Const(0));
                    emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                    func.instruction(&Instruction::LocalSet(callargs_tmp));
                    for arg_name in &args_names[1..] {
                        let arg = locals[arg_name];
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                        func.instruction(&Instruction::Drop);
                    }
                    let site_bits = box_int(stable_ic_site_id(
                        func_ir.name.as_str(),
                        op_idx,
                        "call_guarded_nonfunc",
                    ));
                    func.instruction(&Instruction::I64Const(site_bits));
                    func.instruction(&Instruction::LocalGet(callee_bits));
                    func.instruction(&Instruction::LocalGet(callargs_tmp));
                    emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                    func.instruction(&Instruction::LocalSet(out));
                    func.instruction(&Instruction::End);
                }
                "func_new" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap_or(0);
                    let table_slot = func_map[func_name];
                    let table_idx = table_base + table_slot;
                    let tramp_slot = trampoline_map[func_name];
                    let tramp_idx = table_base + tramp_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    emit_table_index_i64(func, reloc_enabled, tramp_idx);
                    func.instruction(&Instruction::I64Const(arity));
                    emit_call(func, reloc_enabled, import_ids["func_new"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "func_new_closure" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap_or(0);
                    let closure_name = op
                        .args
                        .as_ref()
                        .and_then(|args| args.first())
                        .expect("func_new_closure expects closure arg");
                    let closure_bits = locals[closure_name];
                    let table_slot = func_map[func_name];
                    let table_idx = table_base + table_slot;
                    let tramp_slot = trampoline_map[func_name];
                    let tramp_idx = table_base + tramp_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    emit_table_index_i64(func, reloc_enabled, tramp_idx);
                    func.instruction(&Instruction::I64Const(arity));
                    func.instruction(&Instruction::LocalGet(closure_bits));
                    emit_call(func, reloc_enabled, import_ids["func_new_closure"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "code_new" => {
                    let args = op.args.as_ref().unwrap();
                    let filename_bits = locals[&args[0]];
                    let name_bits = locals[&args[1]];
                    let firstlineno_bits = locals[&args[2]];
                    let linetable_bits = locals[&args[3]];
                    let varnames_bits = locals[&args[4]];
                    let names_bits = locals[&args[5]];
                    let argcount_bits = locals[&args[6]];
                    let posonlyargcount_bits = locals[&args[7]];
                    let kwonlyargcount_bits = locals[&args[8]];
                    func.instruction(&Instruction::LocalGet(filename_bits));
                    func.instruction(&Instruction::LocalGet(name_bits));
                    func.instruction(&Instruction::LocalGet(firstlineno_bits));
                    func.instruction(&Instruction::LocalGet(linetable_bits));
                    func.instruction(&Instruction::LocalGet(varnames_bits));
                    func.instruction(&Instruction::LocalGet(names_bits));
                    func.instruction(&Instruction::LocalGet(argcount_bits));
                    func.instruction(&Instruction::LocalGet(posonlyargcount_bits));
                    func.instruction(&Instruction::LocalGet(kwonlyargcount_bits));
                    emit_call(func, reloc_enabled, import_ids["code_new"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "code_slot_set" => {
                    let args = op.args.as_ref().unwrap();
                    let code_bits = locals[&args[0]];
                    let code_id = op.value.unwrap_or(0);
                    func.instruction(&Instruction::I64Const(code_id));
                    func.instruction(&Instruction::LocalGet(code_bits));
                    emit_call(func, reloc_enabled, import_ids["code_slot_set"]);
                    func.instruction(&Instruction::Drop);
                }
                "fn_ptr_code_set" => {
                    let args = op.args.as_ref().unwrap();
                    let code_bits = locals[&args[0]];
                    let func_name = op.s_value.as_ref().unwrap();
                    let table_slot = func_map[func_name];
                    let table_idx = table_base + table_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::LocalGet(code_bits));
                    emit_call(func, reloc_enabled, import_ids["fn_ptr_code_set"]);
                    func.instruction(&Instruction::Drop);
                }
                "asyncgen_locals_register" => {
                    let args = op.args.as_ref().unwrap();
                    let names_bits = locals[&args[0]];
                    let offsets_bits = locals[&args[1]];
                    let func_name = op.s_value.as_ref().unwrap();
                    let table_slot = func_map[func_name];
                    let table_idx = table_base + table_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::LocalGet(names_bits));
                    func.instruction(&Instruction::LocalGet(offsets_bits));
                    emit_call(func, reloc_enabled, import_ids["asyncgen_locals_register"]);
                    func.instruction(&Instruction::Drop);
                }
                "gen_locals_register" => {
                    let args = op.args.as_ref().unwrap();
                    let names_bits = locals[&args[0]];
                    let offsets_bits = locals[&args[1]];
                    let func_name = op.s_value.as_ref().unwrap();
                    let table_slot = func_map[func_name];
                    let table_idx = table_base + table_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::LocalGet(names_bits));
                    func.instruction(&Instruction::LocalGet(offsets_bits));
                    emit_call(func, reloc_enabled, import_ids["gen_locals_register"]);
                    func.instruction(&Instruction::Drop);
                }
                "code_slots_init" => {
                    let count = op.value.unwrap_or(0);
                    func.instruction(&Instruction::I64Const(count));
                    emit_call(func, reloc_enabled, import_ids["code_slots_init"]);
                    func.instruction(&Instruction::Drop);
                }
                "trace_enter_slot" => {
                    let code_id = op.value.unwrap_or(0);
                    func.instruction(&Instruction::I64Const(code_id));
                    emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                    func.instruction(&Instruction::Drop);
                }
                "trace_exit" => {
                    emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                    func.instruction(&Instruction::Drop);
                }
                "line" => {
                    let line = op.value.unwrap_or(0);
                    func.instruction(&Instruction::I64Const(line));
                    emit_call(func, reloc_enabled, import_ids["trace_set_line"]);
                    func.instruction(&Instruction::Drop);
                }
                "frame_locals_set" => {
                    let args = op.args.as_ref().expect("frame_locals_set args missing");
                    let dict_bits = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(dict_bits));
                    emit_call(func, reloc_enabled, import_ids["frame_locals_set"]);
                    func.instruction(&Instruction::Drop);
                }
                "builtin_func" => {
                    if op.s_value.as_deref() == Some("molt_require_intrinsic_runtime")
                        && op
                            .out
                            .as_ref()
                            .is_some_and(|out| runtime_lookup_only_vars.contains(out))
                    {
                        continue;
                    }
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap_or(0);
                    let table_slot = func_map[func_name];
                    let table_idx = table_base + table_slot;
                    let tramp_slot = trampoline_map[func_name];
                    let tramp_idx = table_base + tramp_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    emit_table_index_i64(func, reloc_enabled, tramp_idx);
                    func.instruction(&Instruction::I64Const(arity));
                    emit_call(func, reloc_enabled, import_ids["func_new_builtin"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "missing" => {
                    let out = locals[op.out.as_ref().unwrap()];
                    emit_call(func, reloc_enabled, import_ids["missing"]);
                    func.instruction(&Instruction::LocalSet(out));
                }
                "function_closure_bits" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = locals[&args[0]];
                    let out = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalGet(func_bits));
                    emit_call(func, reloc_enabled, import_ids["function_closure_bits"]);
                    func.instruction(&Instruction::LocalSet(out));
                    func.instruction(&Instruction::LocalGet(out));
                    emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                }
                "bound_method_new" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = locals[&args[0]];
                    let self_bits = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(func_bits));
                    func.instruction(&Instruction::LocalGet(self_bits));
                    emit_call(func, reloc_enabled, import_ids["bound_method_new"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "call_func" => {
                    let args_names = op.args.as_ref().unwrap();
                    let live_object_locals = live_object_locals_for_call(rel_idx, op.out.as_ref());
                    for local_idx in &live_object_locals {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }
                    if args_names.len() == 3 && runtime_lookup_only_vars.contains(&args_names[0]) {
                        let name_bits = locals[&args_names[1]];
                        let namespace_bits = locals[&args_names[2]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(name_bits));
                        func.instruction(&Instruction::LocalGet(namespace_bits));
                        emit_call(func, reloc_enabled, import_ids["require_intrinsic_runtime"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for local_idx in live_object_locals.iter().rev() {
                            func.instruction(&Instruction::LocalGet(*local_idx));
                            emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                        }
                        continue;
                    }
                    // Outlined: spill args to linear memory, then delegate
                    // to molt_call_func_dispatch runtime helper.
                    let func_bits = locals[&args_names[0]];
                    let out = locals[op.out.as_ref().unwrap()];
                    let nargs = args_names.len().saturating_sub(1);
                    let spill_base = ctx.call_func_spill_offset;

                    // Spill each arg to consecutive i64 slots in linear memory.
                    for (i, arg_name) in args_names[1..].iter().enumerate() {
                        let arg = locals[arg_name];
                        // addr (i32) = spill_base + i * 8
                        func.instruction(&Instruction::I32Const(
                            (spill_base + (i as u32) * 8) as i32,
                        ));
                        func.instruction(&Instruction::LocalGet(arg));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                    }

                    // Push args: func_bits, args_ptr, nargs, code_id
                    func.instruction(&Instruction::LocalGet(func_bits));
                    func.instruction(&Instruction::I64Const(spill_base as i64));
                    func.instruction(&Instruction::I64Const(nargs as i64));
                    let code_id = op.value.unwrap_or(0);
                    func.instruction(&Instruction::I64Const(code_id));
                    emit_call(func, reloc_enabled, import_ids["call_func_dispatch"]);
                    func.instruction(&Instruction::LocalSet(out));
                    for local_idx in live_object_locals.iter().rev() {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                    }
                }
                "invoke_ffi" => {
                    let args_names = op.args.as_ref().unwrap();
                    let live_object_locals = live_object_locals_for_call(rel_idx, op.out.as_ref());
                    for local_idx in &live_object_locals {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }
                    let func_bits = locals[&args_names[0]];
                    let out = locals[op.out.as_ref().unwrap()];
                    let callargs_tmp = locals["__molt_tmp0"];
                    let arity = args_names.len().saturating_sub(1);
                    func.instruction(&Instruction::I64Const(arity as i64));
                    func.instruction(&Instruction::I64Const(0));
                    emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                    func.instruction(&Instruction::LocalSet(callargs_tmp));
                    for arg_name in &args_names[1..] {
                        let arg = locals[arg_name];
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                        func.instruction(&Instruction::Drop);
                    }
                    let invoke_bridge_lane = op.s_value.as_deref() == Some("bridge");
                    let call_site_label = if invoke_bridge_lane {
                        "invoke_ffi_bridge"
                    } else {
                        "invoke_ffi_deopt"
                    };
                    let site_bits = box_int(stable_ic_site_id(
                        func_ir.name.as_str(),
                        op_idx,
                        call_site_label,
                    ));
                    func.instruction(&Instruction::I64Const(site_bits));
                    func.instruction(&Instruction::LocalGet(func_bits));
                    func.instruction(&Instruction::LocalGet(callargs_tmp));
                    let require_bridge_cap = if invoke_bridge_lane { 1 } else { 0 };
                    func.instruction(&Instruction::I64Const(box_bool(require_bridge_cap)));
                    emit_call(func, reloc_enabled, import_ids["invoke_ffi_ic"]);
                    func.instruction(&Instruction::LocalSet(out));
                    for local_idx in live_object_locals.iter().rev() {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                    }
                }
                "call_bind" | "call_indirect" => {
                    let args_names = op.args.as_ref().unwrap();
                    let func_bits = locals[&args_names[0]];
                    let builder_ptr = locals[&args_names[1]];
                    let out = op.out.as_ref().and_then(|name| locals.get(name).copied());
                    let live_object_locals = live_object_locals_for_call(rel_idx, op.out.as_ref());
                    for local_idx in &live_object_locals {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }
                    let call_site_label = if op.kind == "call_indirect" {
                        "call_indirect"
                    } else {
                        "call_bind"
                    };
                    let site_bits = box_int(stable_ic_site_id(
                        func_ir.name.as_str(),
                        op_idx,
                        call_site_label,
                    ));
                    func.instruction(&Instruction::I64Const(site_bits));
                    func.instruction(&Instruction::LocalGet(func_bits));
                    func.instruction(&Instruction::LocalGet(builder_ptr));
                    if op.kind == "call_indirect" {
                        emit_call(func, reloc_enabled, import_ids["call_indirect_ic"]);
                    } else {
                        emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                    }
                    if let Some(out_local) = out {
                        func.instruction(&Instruction::LocalSet(out_local));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                    for local_idx in live_object_locals.iter().rev() {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                    }
                }
                "call_method" => {
                    let args_names = op.args.as_ref().unwrap();
                    let method_bits = locals[&args_names[0]];
                    let out = locals[op.out.as_ref().unwrap()];
                    let live_object_locals = live_object_locals_for_call(rel_idx, op.out.as_ref());
                    for local_idx in &live_object_locals {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }

                    // Fast-path: dispatch known bound-method patterns
                    // directly without callargs allocation or IC lookup.
                    let fast_dispatched = if let Some(sv) = op.s_value.as_deref() {
                        let arity = args_names.len().saturating_sub(1);
                        match sv {
                            "BoundMethod:list:append" if arity == 1 => {
                                let arg = locals[&args_names[1]];
                                func.instruction(&Instruction::LocalGet(method_bits));
                                func.instruction(&Instruction::LocalGet(arg));
                                emit_call(func, reloc_enabled, import_ids["fast_list_append"]);
                                true
                            }
                            "BoundMethod:str:join" if arity == 1 => {
                                let arg = locals[&args_names[1]];
                                func.instruction(&Instruction::LocalGet(method_bits));
                                func.instruction(&Instruction::LocalGet(arg));
                                emit_call(func, reloc_enabled, import_ids["fast_str_join"]);
                                true
                            }
                            "BoundMethod:dict:get" if arity == 2 => {
                                let key = locals[&args_names[1]];
                                let default = locals[&args_names[2]];
                                func.instruction(&Instruction::LocalGet(method_bits));
                                func.instruction(&Instruction::LocalGet(key));
                                func.instruction(&Instruction::LocalGet(default));
                                emit_call(func, reloc_enabled, import_ids["fast_dict_get"]);
                                true
                            }
                            "BoundMethod:str:startswith" if arity == 1 => {
                                let arg = locals[&args_names[1]];
                                func.instruction(&Instruction::LocalGet(method_bits));
                                func.instruction(&Instruction::LocalGet(arg));
                                emit_call(func, reloc_enabled, import_ids["fast_str_startswith"]);
                                true
                            }
                            "BoundMethod:str:upper" if arity == 0 => {
                                func.instruction(&Instruction::LocalGet(method_bits));
                                emit_call(func, reloc_enabled, import_ids["fast_str_upper"]);
                                true
                            }
                            "BoundMethod:str:lower" if arity == 0 => {
                                func.instruction(&Instruction::LocalGet(method_bits));
                                emit_call(func, reloc_enabled, import_ids["fast_str_lower"]);
                                true
                            }
                            "BoundMethod:str:strip" if arity == 0 => {
                                func.instruction(&Instruction::LocalGet(method_bits));
                                emit_call(func, reloc_enabled, import_ids["fast_str_strip"]);
                                true
                            }
                            _ => false,
                        }
                    } else {
                        false
                    };

                    if !fast_dispatched {
                        // Generic path: allocate callargs and dispatch via IC.
                        let callargs_tmp = locals["__molt_tmp0"];
                        let arity = args_names.len().saturating_sub(1);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        let site_bits = box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "call_method",
                        ));
                        func.instruction(&Instruction::I64Const(site_bits));
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                    }
                    func.instruction(&Instruction::LocalSet(out));
                    for local_idx in live_object_locals.iter().rev() {
                        func.instruction(&Instruction::LocalGet(*local_idx));
                        emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
                    }
                }
                "chan_new" => {
                    let args = op.args.as_ref().unwrap();
                    let cap = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(cap));
                    emit_call(func, reloc_enabled, import_ids["chan_new"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "chan_drop" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(chan));
                    emit_call(func, reloc_enabled, import_ids["chan_drop"]);
                    func.instruction(&Instruction::Drop);
                }
                "module_new" => {
                    let args = op.args.as_ref().unwrap();
                    let name = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(name));
                    emit_call(func, reloc_enabled, import_ids["module_new"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_cache_get" => {
                    let args = op.args.as_ref().unwrap();
                    let name = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(name));
                    emit_call(func, reloc_enabled, import_ids["module_cache_get"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_import" => {
                    let args = op.args.as_ref().unwrap();
                    let name = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(name));
                    emit_call(func, reloc_enabled, import_ids["module_import"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_cache_set" => {
                    let args = op.args.as_ref().unwrap();
                    let name = locals[&args[0]];
                    let module = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(name));
                    func.instruction(&Instruction::LocalGet(module));
                    emit_call(func, reloc_enabled, import_ids["module_cache_set"]);
                    if let Some(out) = op.out.as_ref() {
                        if out != "none" {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_cache_del" => {
                    let args = op.args.as_ref().unwrap();
                    let name = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(name));
                    emit_call(func, reloc_enabled, import_ids["module_cache_del"]);
                    if let Some(out) = op.out.as_ref() {
                        if out != "none" {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_get_attr" | "module_import_from" => {
                    let args = op.args.as_ref().unwrap();
                    let module = locals[&args[0]];
                    let name = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(module));
                    func.instruction(&Instruction::LocalGet(name));
                    // `from M import name` uses CPython IMPORT_FROM semantics
                    // (ImportError on miss + sys.modules submodule fallback);
                    // plain `M.name` raises AttributeError.
                    let import_symbol = if op.kind == "module_import_from" {
                        "module_import_from"
                    } else {
                        "module_get_attr"
                    };
                    emit_call(func, reloc_enabled, import_ids[import_symbol]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_get_global" => {
                    let args = op.args.as_ref().unwrap();
                    let module = locals[&args[0]];
                    let name = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(module));
                    func.instruction(&Instruction::LocalGet(name));
                    emit_call(func, reloc_enabled, import_ids["module_get_global"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_del_global" | "module_del_global_if_present" => {
                    let args = op.args.as_ref().unwrap();
                    let module = locals[&args[0]];
                    let name = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(module));
                    func.instruction(&Instruction::LocalGet(name));
                    emit_call(func, reloc_enabled, import_ids[op.kind.as_str()]);
                    if let Some(out) = op.out.as_ref() {
                        if out != "none" {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_get_name" => {
                    let args = op.args.as_ref().unwrap();
                    let module = locals[&args[0]];
                    let name = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(module));
                    func.instruction(&Instruction::LocalGet(name));
                    emit_call(func, reloc_enabled, import_ids["module_get_name"]);
                    if let Some(out) = op.out.as_ref() {
                        let res = locals[out];
                        func.instruction(&Instruction::LocalSet(res));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_set_attr" => {
                    let args = op.args.as_ref().unwrap();
                    let module = locals[&args[0]];
                    let name = locals[&args[1]];
                    let val = locals[&args[2]];
                    func.instruction(&Instruction::LocalGet(module));
                    func.instruction(&Instruction::LocalGet(name));
                    func.instruction(&Instruction::LocalGet(val));
                    emit_call(func, reloc_enabled, import_ids["module_set_attr"]);
                    if let Some(out) = op.out.as_ref() {
                        if out != "none" {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "module_import_star" => {
                    let args = op.args.as_ref().unwrap();
                    let src = locals[&args[0]];
                    let dst = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(src));
                    func.instruction(&Instruction::LocalGet(dst));
                    emit_call(func, reloc_enabled, import_ids["module_import_star"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "alloc_task" => {
                    let total = op.value.unwrap_or(0);
                    let task_kind = op.task_kind.as_deref().unwrap_or("future");
                    let (kind_bits, payload_base) = match task_kind {
                        "generator" => (TASK_KIND_GENERATOR, GEN_CONTROL_SIZE),
                        "future" => (TASK_KIND_FUTURE, 0),
                        "coroutine" => (TASK_KIND_COROUTINE, 0),
                        _ => panic!("unknown task kind: {task_kind}"),
                    };
                    let target_name = op.s_value.as_ref().expect("alloc_task target missing");
                    let table_slot = *func_map.get(target_name).unwrap_or_else(|| {
                        panic!("alloc_task table target not found: {target_name}")
                    });
                    let table_idx = table_base + table_slot;
                    emit_table_index_i64(func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const(total));
                    func.instruction(&Instruction::I64Const(kind_bits));
                    emit_call(func, reloc_enabled, import_ids["task_new"]);
                    let res = if let Some(out) = op.out.as_ref() {
                        let r = locals[out];
                        func.instruction(&Instruction::LocalSet(r));
                        r
                    } else {
                        func.instruction(&Instruction::Drop);
                        0
                    };
                    // Resolve the task handle pointer once when we need to
                    // materialize closure/argument payload slots after the
                    // runtime-owned control block.
                    let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
                    if has_args {
                        let resolve_local = locals["__wasm_alloc_resolve"];
                        func.instruction(&Instruction::LocalGet(res));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(resolve_local));
                    }
                    if let Some(args) = op.args.as_ref()
                        && !args.is_empty()
                    {
                        let resolve_local = locals["__wasm_alloc_resolve"];
                        for (i, name) in args.iter().enumerate() {
                            let arg_local = locals[name];
                            func.instruction(&Instruction::LocalGet(resolve_local));
                            func.instruction(&Instruction::I32Const(payload_base + (i as i32) * 8));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(arg_local));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(arg_local));
                            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        }
                    }
                    if matches!(task_kind, "future" | "coroutine") {
                        func.instruction(&Instruction::LocalGet(res));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
                        emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
                        func.instruction(&Instruction::Drop);
                    }
                }
                "state_yield" => {
                    let args = op.args.as_ref().unwrap();
                    func.instruction(&Instruction::LocalGet(0));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I64Const(op.value.unwrap()));
                    emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                    let pair = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(pair));
                    emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalGet(pair));
                        func.instruction(&Instruction::LocalSet(locals[out]));
                        func.instruction(&Instruction::LocalGet(locals[out]));
                    } else {
                        func.instruction(&Instruction::LocalGet(pair));
                    }
                    func.instruction(&Instruction::Return);
                }
                "context_null" => {
                    let args = op.args.as_ref().unwrap();
                    let payload = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(payload));
                    emit_call(func, reloc_enabled, import_ids["context_null"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "context_enter" => {
                    let args = op.args.as_ref().unwrap();
                    let ctx = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(ctx));
                    emit_call(func, reloc_enabled, import_ids["context_enter"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "context_exit" => {
                    let args = op.args.as_ref().unwrap();
                    let ctx = locals[&args[0]];
                    let exc = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(ctx));
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["context_exit"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "context_unwind" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["context_unwind"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "context_depth" => {
                    emit_call(func, reloc_enabled, import_ids["context_depth"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "context_unwind_to" => {
                    let args = op.args.as_ref().unwrap();
                    let depth = locals[&args[0]];
                    let exc = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(depth));
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["context_unwind_to"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "context_closing" => {
                    let args = op.args.as_ref().unwrap();
                    let payload = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(payload));
                    emit_call(func, reloc_enabled, import_ids["context_closing"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_push" => {
                    if native_eh_enabled {
                        // Native EH: no-op; WASM runtime manages handler stack.
                        const_cache.emit_none(func);
                    } else {
                        emit_call(func, reloc_enabled, import_ids["exception_push"]);
                    }
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_pop" => {
                    if native_eh_enabled {
                        const_cache.emit_none(func);
                    } else {
                        emit_call(func, reloc_enabled, import_ids["exception_pop"]);
                    }
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_stack_clear" => {
                    emit_call(func, reloc_enabled, import_ids["exception_stack_clear"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_last" => {
                    emit_call(func, reloc_enabled, import_ids["exception_last"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_last_pending" | "exception_finally_pending_observer" => {
                    emit_call(func, reloc_enabled, import_ids["exception_last_pending"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_active" => {
                    emit_call(func, reloc_enabled, import_ids["exception_active"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_current" => {
                    emit_call(func, reloc_enabled, import_ids["exception_current"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_enter_handler" => {
                    let args = op.args.as_ref().unwrap();
                    let captured = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(captured));
                    emit_call(func, reloc_enabled, import_ids["exception_enter_handler"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_resolve_captured" => {
                    let args = op.args.as_ref().unwrap();
                    let captured = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(captured));
                    emit_call(
                        func,
                        reloc_enabled,
                        import_ids["exception_resolve_captured"],
                    );
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_new" => {
                    let args = op.args.as_ref().unwrap();
                    let kind = locals[&args[0]];
                    let args_bits = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(kind));
                    func.instruction(&Instruction::LocalGet(args_bits));
                    emit_call(func, reloc_enabled, import_ids["exception_new"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_new_builtin" => {
                    let args = op.args.as_ref().unwrap();
                    let tag = op.value.expect("exception_new_builtin missing tag value");
                    let args_bits = locals[&args[0]];
                    func.instruction(&Instruction::I64Const(tag));
                    func.instruction(&Instruction::LocalGet(args_bits));
                    emit_call(func, reloc_enabled, import_ids["exception_new_builtin"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_new_builtin_empty" => {
                    let tag = op
                        .value
                        .expect("exception_new_builtin_empty missing tag value");
                    func.instruction(&Instruction::I64Const(tag));
                    emit_call(
                        func,
                        reloc_enabled,
                        import_ids["exception_new_builtin_empty"],
                    );
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_new_builtin_one" => {
                    let args = op.args.as_ref().unwrap();
                    let tag = op
                        .value
                        .expect("exception_new_builtin_one missing tag value");
                    let arg_bits = locals[&args[0]];
                    func.instruction(&Instruction::I64Const(tag));
                    func.instruction(&Instruction::LocalGet(arg_bits));
                    emit_call(func, reloc_enabled, import_ids["exception_new_builtin_one"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_new_from_class" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits = locals[&args[0]];
                    let args_bits = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(class_bits));
                    func.instruction(&Instruction::LocalGet(args_bits));
                    emit_call(func, reloc_enabled, import_ids["exception_new_from_class"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exceptiongroup_match" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    let matcher = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(exc));
                    func.instruction(&Instruction::LocalGet(matcher));
                    emit_call(func, reloc_enabled, import_ids["exceptiongroup_match"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exceptiongroup_combine" => {
                    let args = op.args.as_ref().unwrap();
                    let items = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(items));
                    emit_call(func, reloc_enabled, import_ids["exceptiongroup_combine"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_clear" => {
                    emit_call(func, reloc_enabled, import_ids["exception_clear"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_kind" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["exception_kind"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_class" => {
                    let args = op.args.as_ref().unwrap();
                    let kind = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(kind));
                    emit_call(func, reloc_enabled, import_ids["exception_class"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_message" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["exception_message"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_set_cause" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    let cause = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(exc));
                    func.instruction(&Instruction::LocalGet(cause));
                    emit_call(func, reloc_enabled, import_ids["exception_set_cause"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_set_value" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    let value = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(exc));
                    func.instruction(&Instruction::LocalGet(value));
                    emit_call(func, reloc_enabled, import_ids["exception_set_value"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_context_set" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["exception_context_set"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "exception_set_last" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["exception_set_last"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "raise" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(exc));
                    if native_eh_enabled {
                        // Native EH: call host raise to register the exception
                        // (traceback, __context__), then throw via WASM EH.
                        emit_call(func, reloc_enabled, import_ids["raise"]);
                        func.instruction(&Instruction::Drop);
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Throw(TAG_EXCEPTION_INDEX));
                    } else {
                        emit_call(func, reloc_enabled, import_ids["raise"]);
                        if let Some(ref out) = op.out {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            // raise with no output — drop the result from the stack
                            func.instruction(&Instruction::Drop);
                        }
                    }
                }
                "bridge_unavailable" => {
                    let args = op.args.as_ref().unwrap();
                    let msg = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(msg));
                    emit_call(func, reloc_enabled, import_ids["bridge_unavailable"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "file_open" => {
                    let args = op.args.as_ref().unwrap();
                    let path = locals[&args[0]];
                    let mode = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(path));
                    func.instruction(&Instruction::LocalGet(mode));
                    emit_call(func, reloc_enabled, import_ids["file_open"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "file_read" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = locals[&args[0]];
                    let size = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(handle));
                    func.instruction(&Instruction::LocalGet(size));
                    emit_call(func, reloc_enabled, import_ids["file_read"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "file_write" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = locals[&args[0]];
                    let data = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(handle));
                    func.instruction(&Instruction::LocalGet(data));
                    emit_call(func, reloc_enabled, import_ids["file_write"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "file_close" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(handle));
                    emit_call(func, reloc_enabled, import_ids["file_close"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "file_flush" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(handle));
                    emit_call(func, reloc_enabled, import_ids["file_flush"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancel_token_new" => {
                    let args = op.args.as_ref().unwrap();
                    let parent = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(parent));
                    emit_call(func, reloc_enabled, import_ids["cancel_token_new"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancel_token_clone" => {
                    let args = op.args.as_ref().unwrap();
                    let token = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(token));
                    emit_call(func, reloc_enabled, import_ids["cancel_token_clone"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancel_token_drop" => {
                    let args = op.args.as_ref().unwrap();
                    let token = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(token));
                    emit_call(func, reloc_enabled, import_ids["cancel_token_drop"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancel_token_cancel" => {
                    let args = op.args.as_ref().unwrap();
                    let token = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(token));
                    emit_call(func, reloc_enabled, import_ids["cancel_token_cancel"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "future_cancel" => {
                    let args = op.args.as_ref().unwrap();
                    let future = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(future));
                    emit_call(func, reloc_enabled, import_ids["future_cancel"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "future_cancel_msg" => {
                    let args = op.args.as_ref().unwrap();
                    let future = locals[&args[0]];
                    let msg = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(future));
                    func.instruction(&Instruction::LocalGet(msg));
                    emit_call(func, reloc_enabled, import_ids["future_cancel_msg"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "future_cancel_clear" => {
                    let args = op.args.as_ref().unwrap();
                    let future = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(future));
                    emit_call(func, reloc_enabled, import_ids["future_cancel_clear"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "promise_new" => {
                    emit_call(func, reloc_enabled, import_ids["promise_new"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "promise_set_result" => {
                    let args = op.args.as_ref().unwrap();
                    let future = locals[&args[0]];
                    let result = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(future));
                    func.instruction(&Instruction::LocalGet(result));
                    emit_call(func, reloc_enabled, import_ids["promise_set_result"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "promise_set_exception" => {
                    let args = op.args.as_ref().unwrap();
                    let future = locals[&args[0]];
                    let exc = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(future));
                    func.instruction(&Instruction::LocalGet(exc));
                    emit_call(func, reloc_enabled, import_ids["promise_set_exception"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "thread_submit" => {
                    let args = op.args.as_ref().unwrap();
                    let callable = locals[&args[0]];
                    let call_args = locals[&args[1]];
                    let call_kwargs = locals[&args[2]];
                    func.instruction(&Instruction::LocalGet(callable));
                    func.instruction(&Instruction::LocalGet(call_args));
                    func.instruction(&Instruction::LocalGet(call_kwargs));
                    emit_call(func, reloc_enabled, import_ids["thread_submit"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "task_register_token_owned" => {
                    let args = op.args.as_ref().unwrap();
                    let task = locals[&args[0]];
                    let token = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(task));
                    func.instruction(&Instruction::LocalGet(token));
                    emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "spawn" => {
                    let args = op.args.as_ref().unwrap();
                    func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                    emit_call(func, reloc_enabled, import_ids["spawn"]);
                }
                "cancel_token_is_cancelled" => {
                    let args = op.args.as_ref().unwrap();
                    let token = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(token));
                    emit_call(func, reloc_enabled, import_ids["cancel_token_is_cancelled"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancel_token_set_current" => {
                    let args = op.args.as_ref().unwrap();
                    let token = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(token));
                    emit_call(func, reloc_enabled, import_ids["cancel_token_set_current"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancel_token_get_current" => {
                    emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancelled" => {
                    emit_call(func, reloc_enabled, import_ids["cancelled"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "cancel_current" => {
                    emit_call(func, reloc_enabled, import_ids["cancel_current"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
                "block_on" => {
                    let args = op.args.as_ref().unwrap();
                    func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                    emit_call(func, reloc_enabled, import_ids["block_on"]);
                    if let Some(out) = op.out.as_ref() {
                        func.instruction(&Instruction::LocalSet(locals[out]));
                    } else {
                        func.instruction(&Instruction::Drop);
                    }
                }
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
                // ---------------------------------------------------------------
                // memory_copy: bulk linear-memory copy (WASM 2.0 bulk-memory op)
                //
                // IR signature:  memory_copy(dst, src, len)
                //   dst, src  – i64 boxed integers holding i32 linear-memory byte
                //               offsets (e.g. from handle_resolve)
                //   len       – i64 boxed integer holding the byte count
                //
                // Emits:  memory.copy  (dst_mem=0, src_mem=0)
                //         stack: [dst:i32, src:i32, len:i32]
                //
                // This intrinsic enables the IR to emit efficient buffer-to-buffer
                // copies without round-tripping through host imports.  See
                // WASM_OPTIMIZATION_PLAN.md Section 3.3.
                // ---------------------------------------------------------------
                "memory_copy" => {
                    let args = op.args.as_ref().unwrap();
                    debug_assert!(
                        args.len() == 3,
                        "memory_copy requires exactly 3 args (dst, src, len)"
                    );
                    let dst = locals[&args[0]];
                    let src = locals[&args[1]];
                    let len = locals[&args[2]];
                    // Unbox each i64 value to i32 for the memory.copy instruction.
                    func.instruction(&Instruction::LocalGet(dst));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::LocalGet(src));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::LocalGet(len));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::MemoryCopy {
                        src_mem: 0,
                        dst_mem: 0,
                    });
                }
                // ---------------------------------------------------------------
                // memory_fill: bulk linear-memory fill (WASM 2.0 bulk-memory op)
                //
                // IR signature:  memory_fill(dst, val, len)
                //   dst  – i64 boxed integer holding i32 linear-memory byte offset
                //   val  – i64 boxed integer holding the fill byte (0-255)
                //   len  – i64 boxed integer holding the byte count
                //
                // Emits:  memory.fill  (mem=0)
                //         stack: [dst:i32, val:i32, len:i32]
                //
                // Enables efficient zero-init and constant-fill of linear memory
                // regions without round-tripping through host imports or byte loops.
                // ---------------------------------------------------------------
                "memory_fill" => {
                    let args = op.args.as_ref().unwrap();
                    debug_assert!(
                        args.len() == 3,
                        "memory_fill requires exactly 3 args (dst, val, len)"
                    );
                    let dst = locals[&args[0]];
                    let val = locals[&args[1]];
                    let len = locals[&args[2]];
                    // Unbox each i64 value to i32 for the memory.fill instruction.
                    func.instruction(&Instruction::LocalGet(dst));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::LocalGet(val));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::LocalGet(len));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::MemoryFill(0));
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
