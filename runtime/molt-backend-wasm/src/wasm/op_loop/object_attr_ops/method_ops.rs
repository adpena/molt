use super::super::result_sink::store_result_or_drop;
use super::super::*;

pub(super) fn emit_method_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
) -> bool {
    match op.kind.as_str() {
        "classmethod_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(func_bits));
            emit_call(func, reloc_enabled, import_ids["classmethod_new"]);
            store_result_or_drop(func, op, locals);
        }
        "staticmethod_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(func_bits));
            emit_call(func, reloc_enabled, import_ids["staticmethod_new"]);
            store_result_or_drop(func, op, locals);
        }
        "property_new" => {
            let args = op.args.as_ref().unwrap();
            let getter = locals[&args[0]];
            let setter = locals[&args[1]];
            let deleter = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(getter));
            func.instruction(&Instruction::LocalGet(setter));
            func.instruction(&Instruction::LocalGet(deleter));
            emit_call(func, reloc_enabled, import_ids["property_new"]);
            store_result_or_drop(func, op, locals);
        }
        "bound_method_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            let self_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::LocalGet(self_bits));
            emit_call(func, reloc_enabled, import_ids["bound_method_new"]);
            store_result_or_drop(func, op, locals);
        }
        "is_bound_method" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["is_bound_method"]);
            store_result_or_drop(func, op, locals);
        }
        "call_method_ic" => {
            // Fused instance-method dispatch (LOAD_METHOD/CALL_METHOD):
            //   args = [recv, a0, a1, ...]  s_value = <method name>
            // Lowers to a single molt_call_method_icN(site, recv,
            // name_ptr, name_len, a0..) host call - no bound-method or
            // callargs allocation on the IC fast path. The runtime
            // entry is target-independent extern "C"; `name_ptr` is a
            // 32-bit linear-memory address (i32), every NaN-boxed
            // value (site/recv/args/len) is i64.
            let args_names = op.args.as_ref().unwrap();
            let recv = locals[&args_names[0]];
            let method_name = op
                .s_value
                .as_ref()
                .expect("call_method_ic missing method name");
            let bytes = method_name.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                "call_method_ic",
            ));
            let extra = &args_names[1..];
            // Stack: site, recv, name_ptr(i32), name_len, a0..
            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(recv));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            for name in extra {
                func.instruction(&Instruction::LocalGet(locals[name]));
            }
            let import = match extra.len() {
                0 => "call_method_ic0",
                1 => "call_method_ic1",
                2 => "call_method_ic2",
                3 => "call_method_ic3",
                _ => "call_method_ic4",
            };
            emit_call(func, reloc_enabled, import_ids[import]);
            store_result_or_drop(func, op, locals);
        }
        "call_super_method_ic" => {
            // Fused super().method() dispatch (no super / bound-method /
            // callargs allocation on the fast path):
            //   args = [class, self, a0, a1, ...]  s_value = <method>
            // Lowers to molt_call_super_method_icN(site, class, self,
            // name_ptr, name_len, a0..).
            let args_names = op.args.as_ref().unwrap();
            let class = locals[&args_names[0]];
            let self_local = locals[&args_names[1]];
            let method_name = op
                .s_value
                .as_ref()
                .expect("call_super_method_ic missing method name");
            let bytes = method_name.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                "call_super_method_ic",
            ));
            let extra = &args_names[2..];
            // Stack: site, class, self, name_ptr(i32), name_len, a0..
            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(class));
            func.instruction(&Instruction::LocalGet(self_local));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            for name in extra {
                func.instruction(&Instruction::LocalGet(locals[name]));
            }
            let import = match extra.len() {
                0 => "call_super_method_ic0",
                1 => "call_super_method_ic1",
                2 => "call_super_method_ic2",
                3 => "call_super_method_ic3",
                _ => "call_super_method_ic4",
            };
            emit_call(func, reloc_enabled, import_ids[import]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
