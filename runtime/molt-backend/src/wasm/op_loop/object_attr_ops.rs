use super::super::context::CompileFuncContext;
use super::*;

#[allow(unused_variables)]
pub(super) fn emit_object_attr_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    ctx: &CompileFuncContext<'_>,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
) -> bool {
    match op.kind.as_str() {
        "dataclass_new" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let fields = locals[&args[1]];
            let values = locals[&args[2]];
            let flags = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(fields));
            func.instruction(&Instruction::LocalGet(values));
            func.instruction(&Instruction::LocalGet(flags));
            emit_call(func, reloc_enabled, import_ids["dataclass_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dataclass_new_values" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let fields = locals[&args[1]];
            let flags = locals[&args[2]];
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(box_int(args[3..].len() as i64)));
            emit_call(func, reloc_enabled, import_ids["list_builder_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for value_name in &args[3..] {
                let value = locals[value_name];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(value));
                emit_call(func, reloc_enabled, import_ids["list_builder_append"]);
            }
            func.instruction(&Instruction::LocalGet(out));
            emit_call(func, reloc_enabled, import_ids["tuple_builder_finish"]);
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(fields));
            func.instruction(&Instruction::LocalGet(out));
            func.instruction(&Instruction::LocalGet(flags));
            emit_call(func, reloc_enabled, import_ids["dataclass_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "dataclass_get" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            emit_call(func, reloc_enabled, import_ids["dataclass_get"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dataclass_set" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["dataclass_set"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "dataclass_set_class" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_obj = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(class_obj));
            emit_call(func, reloc_enabled, import_ids["dataclass_set_class"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "class_new" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["class_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "class_def" => {
            let args = op.args.as_ref().unwrap();
            let meta = op.s_value.as_deref().expect("class_def needs s_value");
            let mut parts = meta.split(',');
            let nbases = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .expect("class_def metadata missing base count");
            let nattrs = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .expect("class_def metadata missing attr count");
            let layout_size = parts
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .expect("class_def metadata missing layout size");
            let layout_version = parts
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .expect("class_def metadata missing layout version");
            let flags = parts
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .expect("class_def metadata missing flags");

            let spill_base = ctx.class_def_spill_offset;
            let bases_words = nbases.max(1) as u32;
            let attrs_base = spill_base + bases_words * 8;
            let attrs_start = 1 + nbases;

            // `class_def` spills boxed handles through shared linear memory
            // before the runtime helper snapshots them. Pin every handle
            // across that helper call so RC cleanup cannot reclaim or reuse
            // any object between the spill stores and `guarded_class_def`.
            for arg_name in args {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(arg));
                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
            }

            for (i, base_name) in args[1..1 + nbases].iter().enumerate() {
                let base = locals[base_name];
                func.instruction(&Instruction::I32Const((spill_base + (i as u32) * 8) as i32));
                func.instruction(&Instruction::LocalGet(base));
                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
            }

            for i in 0..nattrs {
                let key = locals[&args[attrs_start + i * 2]];
                let val = locals[&args[attrs_start + i * 2 + 1]];
                func.instruction(&Instruction::I32Const(
                    (attrs_base + (i as u32) * 16) as i32,
                ));
                func.instruction(&Instruction::LocalGet(key));
                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::I32Const(
                    (attrs_base + (i as u32) * 16 + 8) as i32,
                ));
                func.instruction(&Instruction::LocalGet(val));
                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
            }

            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::I32Const(spill_base as i32));
            func.instruction(&Instruction::I64Const(nbases as i64));
            func.instruction(&Instruction::I32Const(attrs_base as i32));
            func.instruction(&Instruction::I64Const(nattrs as i64));
            func.instruction(&Instruction::I64Const(layout_size));
            func.instruction(&Instruction::I64Const(layout_version));
            func.instruction(&Instruction::I64Const(flags));
            emit_call(func, reloc_enabled, import_ids["guarded_class_def"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
            for arg_name in args.iter().rev() {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(arg));
                emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
            }
        }
        "class_set_base" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            let base_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(base_bits));
            emit_call(func, reloc_enabled, import_ids["class_set_base"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "class_apply_set_name" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            emit_call(func, reloc_enabled, import_ids["class_apply_set_name"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "super_new" => {
            let args = op.args.as_ref().unwrap();
            let type_bits = locals[&args[0]];
            let obj_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(type_bits));
            func.instruction(&Instruction::LocalGet(obj_bits));
            emit_call(func, reloc_enabled, import_ids["super_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "builtin_type" => {
            let args = op.args.as_ref().unwrap();
            let tag = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(tag));
            emit_call(func, reloc_enabled, import_ids["builtin_type"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "type_of" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["type_of"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "class_layout_version" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            emit_call(func, reloc_enabled, import_ids["class_layout_version"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "class_set_layout_version" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            let version_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(version_bits));
            emit_call(func, reloc_enabled, import_ids["class_set_layout_version"]);
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
        "class_merge_layout" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            let offsets_bits = locals[&args[1]];
            let size_bits = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(offsets_bits));
            func.instruction(&Instruction::LocalGet(size_bits));
            emit_call(func, reloc_enabled, import_ids["class_merge_layout"]);
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
        "isinstance" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let cls = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(cls));
            emit_call(func, reloc_enabled, import_ids["isinstance"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_match_builtin" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            let tag = op.value.expect("exception_match_builtin missing tag value");
            func.instruction(&Instruction::LocalGet(exc));
            func.instruction(&Instruction::I64Const(tag));
            emit_call(func, reloc_enabled, import_ids["exception_match_builtin"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "issubclass" => {
            let args = op.args.as_ref().unwrap();
            let sub = locals[&args[0]];
            let cls = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(sub));
            func.instruction(&Instruction::LocalGet(cls));
            emit_call(func, reloc_enabled, import_ids["issubclass"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "object_new" => {
            emit_call(func, reloc_enabled, import_ids["object_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "object_new_bound" => {
            let args = op
                .args
                .as_ref()
                .expect("object_new_bound requires class arg");
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            if let Some(payload_size) = op.value.filter(|size| *size > 0) {
                func.instruction(&Instruction::I64Const(payload_size));
                emit_call(func, reloc_enabled, import_ids["object_new_bound_sized"]);
            } else {
                emit_call(func, reloc_enabled, import_ids["object_new_bound"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "object_new_bound_stack" => {
            let args = op
                .args
                .as_ref()
                .expect("object_new_bound_stack requires class arg");
            let payload_size = op
                .value
                .filter(|size| *size > 0)
                .expect("object_new_bound_stack requires positive payload byte size");
            let class_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::I64Const(payload_size));
            emit_call(func, reloc_enabled, import_ids["object_new_bound_sized"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "classmethod_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(func_bits));
            emit_call(func, reloc_enabled, import_ids["classmethod_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "staticmethod_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(func_bits));
            emit_call(func, reloc_enabled, import_ids["staticmethod_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
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
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "object_set_class" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_obj = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::LocalGet(class_obj));
            emit_call(func, reloc_enabled, import_ids["object_set_class"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "get_attr_generic_ptr" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["get_attr_ptr"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "call_method_ic" => {
            // Fused instance-method dispatch (LOAD_METHOD/CALL_METHOD):
            //   args = [recv, a0, a1, ...]  s_value = <method name>
            // Lowers to a single molt_call_method_icN(site, recv,
            // name_ptr, name_len, a0..) host call — no bound-method or
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
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
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
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "get_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                "get_attr_generic_obj",
            ));
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::I64Const(site_bits));
            emit_call(func, reloc_enabled, import_ids["get_attr_object_ic"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "get_attr_special_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["get_attr_special"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_attr_generic_ptr" => {
            // The `_generic_ptr` SETATTR form can target a tagged
            // non-pointer receiver (e.g. `typing.final(42)`). Resolving
            // it to a pointer first (`handle_resolve`) then calling
            // `set_attr_ptr` (which dereferences the object header)
            // would fault on a tagged value. Route through the
            // bits-validating `set_attr_object` instead — identical to
            // the `set_attr_generic_obj` arm — so a tagged receiver
            // raises a clean AttributeError/TypeError. This keeps the
            // native and WASM backends at parity (see the native
            // `fc::attrs` fix).
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let val = locals[&args[1]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["set_attr_object"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = *locals.get(&args[0]).unwrap_or_else(|| {
                panic!(
                    "missing local {} in {} for {}",
                    args[0], func_ir.name, op.kind
                )
            });
            let val = *locals.get(&args[1]).unwrap_or_else(|| {
                panic!(
                    "missing local {} in {} for {}",
                    args[1], func_ir.name, op.kind
                )
            });
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["set_attr_object"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "del_attr_generic_ptr" => {
            // Mirror the `set_attr_generic_ptr` fix: a tagged
            // non-pointer receiver must not be `handle_resolve`'d and
            // dereferenced by `del_attr_ptr`. Route through the
            // bits-validating `del_attr_object` (same as
            // `del_attr_generic_obj`) for native/WASM parity.
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["del_attr_object"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "del_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let attr = op.s_value.as_ref().unwrap();
            let bytes = attr.as_bytes();
            let data = backend.add_data_segment(reloc_enabled, bytes);
            func.instruction(&Instruction::LocalGet(obj));
            backend.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            emit_call(func, reloc_enabled, import_ids["del_attr_object"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "get_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["get_attr_name"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "get_attr_name_default" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            let default_val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(default_val));
            emit_call(func, reloc_enabled, import_ids["get_attr_name_default"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "has_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["has_attr_name"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "set_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["set_attr_name"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "del_attr_name" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["del_attr_name"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }

        _ => return false,
    }
    true
}
