use super::super::multi_return_layout::WasmMultiReturnLayout;
use super::*;

mod dynamic;
use std::collections::HashSet;

pub(super) enum CallOpEmission {
    NotHandled,
    Handled,
    HandledAndSkipNext,
}

pub(super) struct CallOpContext<'a, 'ctx, 'm> {
    pub(super) func_ir: &'a FunctionIR,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) func_map: &'a BTreeMap<String, u32>,
    pub(super) func_indices: &'a BTreeMap<String, u32>,
    pub(super) trampoline_map: &'a BTreeMap<String, u32>,
    pub(super) table_base: u32,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) closure_functions: &'a BTreeSet<String>,
    pub(super) runtime_lookup_only_vars: &'a BTreeSet<String>,
    pub(super) locals: &'a BTreeMap<String, u32>,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) multi_return_candidates: &'a BTreeMap<String, usize>,
    pub(super) multi_return: &'a WasmMultiReturnLayout,
    pub(super) reloc_enabled: bool,
    pub(super) tail_call_eligible: bool,
    pub(super) arena_local: Option<u32>,
    pub(super) tail_call_count: &'a Cell<usize>,
    pub(super) ops: &'a [OpIR],
    pub(super) last_use_local: &'m BTreeMap<String, usize>,
    pub(super) rc_skip_inc: &'m HashSet<usize>,
    pub(super) rc_skip_dec: &'m HashSet<String>,
    pub(super) rel_idx: usize,
    pub(super) op_idx: usize,
    pub(super) try_stack_is_empty: bool,
}

fn collect_live_object_locals_for_call(
    locals: &BTreeMap<String, u32>,
    last_use_local: &BTreeMap<String, usize>,
    rel_idx: usize,
    out_name: Option<&String>,
) -> Vec<u32> {
    let mut live = BTreeSet::new();
    for (name, &local_idx) in locals {
        if name == "none" {
            continue;
        }
        if out_name.is_some_and(|out| out == name) {
            continue;
        }
        if name.starts_with("__molt_tmp") || name.ends_with("_ptr") || name.ends_with("_len") {
            continue;
        }
        if last_use_local.get(name).is_none_or(|last| *last <= rel_idx) {
            continue;
        }
        live.insert(local_idx);
    }
    live.into_iter().collect()
}

pub(super) fn emit_call_op(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let func_ir = call_ctx.func_ir;
    let ctx = call_ctx.ctx;
    let func_map = call_ctx.func_map;
    let func_indices = call_ctx.func_indices;
    let trampoline_map = call_ctx.trampoline_map;
    let table_base = call_ctx.table_base;
    let import_ids = call_ctx.import_ids;
    let runtime_lookup_only_vars = call_ctx.runtime_lookup_only_vars;
    let locals = call_ctx.locals;
    let const_cache = call_ctx.const_cache;
    let multi_return_candidates = call_ctx.multi_return_candidates;
    let multi_return = call_ctx.multi_return;
    let reloc_enabled = call_ctx.reloc_enabled;
    let tail_call_eligible = call_ctx.tail_call_eligible;
    let arena_local = call_ctx.arena_local;
    let tail_call_count = call_ctx.tail_call_count;
    let ops = call_ctx.ops;
    let last_use_local = call_ctx.last_use_local;
    let rc_skip_inc = call_ctx.rc_skip_inc;
    let rc_skip_dec = call_ctx.rc_skip_dec;
    let rel_idx = call_ctx.rel_idx;
    let try_stack_is_empty = call_ctx.try_stack_is_empty;
    let live_object_locals_for_call = |rel_idx: usize, out_name: Option<&String>| -> Vec<u32> {
        collect_live_object_locals_for_call(locals, last_use_local, rel_idx, out_name)
    };

    match op.kind.as_str() {
        "call_async" => {
            let payload_len = op.args.as_ref().map(|args| args.len()).unwrap_or(0);
            let target_name = op.s_value.as_ref().expect("call_async target missing");
            let table_slot = *func_map
                .get(target_name)
                .unwrap_or_else(|| panic!("call_async table target not found: {target_name}"));
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
        "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" | "gpu_barrier" => {
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
                return CallOpEmission::Handled;
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
                            && try_stack_is_empty
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
                return CallOpEmission::HandledAndSkipNext;
            }

            emit_call(func, reloc_enabled, func_idx);
            // Multi-value return (Section 3.1): pop N results
            // into dedicated locals for later tuple_index.
            if multi_return.is_promoted_call_tuple(out_name) {
                let ret_count = multi_return_candidates[target_name];
                for k in (0..ret_count).rev() {
                    let local_idx = multi_return
                        .promoted_call_value_local(out_name, k as i64)
                        .expect("multi-return call result local missing");
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
        "call_guarded" | "call_func" | "invoke_ffi" | "call_bind" | "call_indirect"
        | "call_method" => {
            return dynamic::emit_dynamic_call_op(call_ctx, func, op);
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
                return CallOpEmission::Handled;
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
        _ => return CallOpEmission::NotHandled,
    }

    CallOpEmission::Handled
}
