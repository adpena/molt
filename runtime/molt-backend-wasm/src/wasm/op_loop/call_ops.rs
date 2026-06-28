use super::super::module_abi::WasmCallableCallSiteAbi;
use super::super::multi_return_layout::WasmMultiReturnLayout;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::{emit_call, emit_table_index_i64};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_plan::gpu_runtime_call_symbol;
use crate::wasm_values::ConstantCache;
use crate::{FunctionIR, OpIR};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use wasm_encoder::{Function, Instruction};

mod direct;
mod dynamic;
mod site;

pub(super) enum CallOpEmission {
    NotHandled,
    Handled,
    HandledAndSkipNext,
}

pub(super) struct CallOpContext<'a, 'ctx, 'm> {
    pub(super) func_ir: &'a FunctionIR,
    pub(super) call_site_abi: &'a WasmCallableCallSiteAbi<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) runtime_lookup_only_vars: &'a BTreeSet<String>,
    pub(super) locals: &'a WasmFrameLocals,
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

pub(super) fn emit_call_op(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let call_site_abi = call_ctx.call_site_abi;
    let import_ids = call_ctx.import_ids;
    let runtime_lookup_only_vars = call_ctx.runtime_lookup_only_vars;
    let locals = call_ctx.locals;
    let const_cache = call_ctx.const_cache;
    let reloc_enabled = call_ctx.reloc_enabled;
    let rc_skip_inc = call_ctx.rc_skip_inc;
    let rc_skip_dec = call_ctx.rc_skip_dec;
    let rel_idx = call_ctx.rel_idx;

    match direct::emit_direct_call_op(call_ctx, func, op) {
        CallOpEmission::Handled => return CallOpEmission::Handled,
        CallOpEmission::HandledAndSkipNext => return CallOpEmission::HandledAndSkipNext,
        CallOpEmission::NotHandled => {}
    }

    match op.kind.as_str() {
        "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" | "gpu_barrier" => {
            let runtime_name =
                gpu_runtime_call_symbol(op.kind.as_str()).expect("gpu runtime symbol");
            let import_name = runtime_name.strip_prefix("molt_").unwrap_or(runtime_name);
            let out = locals[op.out.as_ref().expect("gpu op result missing")];
            emit_call(func, reloc_enabled, import_ids[import_name]);
            func.instruction(&Instruction::LocalSet(out));
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
            let table_pair = call_site_abi.callable_table_pair(func_name, "func_new");
            emit_table_index_i64(func, reloc_enabled, table_pair.function_table_index);
            emit_table_index_i64(func, reloc_enabled, table_pair.trampoline_table_index);
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
            let table_pair = call_site_abi.callable_table_pair(func_name, "func_new_closure");
            emit_table_index_i64(func, reloc_enabled, table_pair.function_table_index);
            emit_table_index_i64(func, reloc_enabled, table_pair.trampoline_table_index);
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
            let table_idx = call_site_abi.table_index(func_name, "fn_ptr_code_set");
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
            let table_idx = call_site_abi.table_index(func_name, "asyncgen_locals_register");
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
            let table_idx = call_site_abi.table_index(func_name, "gen_locals_register");
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
            let table_pair = call_site_abi.callable_table_pair(func_name, "builtin_func");
            emit_table_index_i64(func, reloc_enabled, table_pair.function_table_index);
            emit_table_index_i64(func, reloc_enabled, table_pair.trampoline_table_index);
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
