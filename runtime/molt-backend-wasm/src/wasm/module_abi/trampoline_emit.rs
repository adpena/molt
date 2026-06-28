use wasm_encoder::{Function, Instruction, ValType};

use crate::wasm::WasmBackend;
use crate::wasm_abi::{GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_GENERATOR};
use crate::wasm_binary::{emit_call, emit_i32_const, emit_ref_func, emit_table_index_i64};
use crate::wasm_data::DataSegmentRef;
use crate::wasm_values::box_int;
use crate::{TrampolineKind, TrampolineSpec};

impl WasmBackend {
    pub(super) fn compile_trampoline(
        &mut self,
        reloc_enabled: bool,
        target_func_index: u32,
        table_idx: u32,
        spec: TrampolineSpec,
        multi_return_count: Option<usize>,
    ) {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
            target_has_ret: _,
        } = spec;
        self.funcs.function(5);
        self.func_count += 1;
        let mut local_types = Vec::new();
        if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            local_types.push(ValType::I64);
            local_types.push(ValType::I32);
            local_types.push(ValType::I64);
            local_types.push(ValType::I32);
        }
        // For multi-value return trampolines (Plain kind only): allocate
        // N temp locals for the return values + 1 local for the tuple builder.
        // Params occupy locals 0..=2, so extra locals start at index 3.
        let mr_locals_start: u32 = 3 + local_types.len() as u32;
        if let (Some(ret_count), TrampolineKind::Plain) = (multi_return_count, &kind) {
            // N temp locals for storing each return value
            for _ in 0..ret_count {
                local_types.push(ValType::I64);
            }
            // 1 local for the tuple builder handle
            local_types.push(ValType::I64);
            let _ = ret_count; // suppress unused warning
        }
        let mut func = Function::new_with_locals_types(local_types);
        if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            let task_local = 3;
            let base_local = 4;
            let val_local = 5;
            let args_base_local = 6;
            match kind {
                TrampolineKind::Generator => {
                    if closure_size < 0 {
                        panic!("generator closure size must be non-negative");
                    }
                    let payload_slots = arity + usize::from(has_closure);
                    let needed = GEN_CONTROL_SIZE as i64 + (payload_slots as i64) * 8;
                    if closure_size < needed {
                        panic!("generator closure size too small for trampoline");
                    }
                    emit_table_index_i64(&mut func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const(closure_size));
                    func.instruction(&Instruction::I64Const(TASK_KIND_GENERATOR));
                    emit_call(&mut func, reloc_enabled, self.import_ids["task_new"]);
                    func.instruction(&Instruction::LocalSet(task_local));
                    if payload_slots > 0 {
                        func.instruction(&Instruction::LocalGet(task_local));
                        emit_call(&mut func, reloc_enabled, self.import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(base_local));
                        if arity > 0 {
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalSet(args_base_local));
                        }
                        let mut offset = GEN_CONTROL_SIZE;
                        if has_closure {
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(0));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(0));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                            offset += 8;
                        }
                        for idx in 0..arity {
                            let arg_offset = offset + (idx as i32) * 8;
                            func.instruction(&Instruction::LocalGet(args_base_local));
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: (idx * std::mem::size_of::<u64>()) as u64,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(val_local));
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(arg_offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(val_local));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(val_local));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                        }
                    }
                    func.instruction(&Instruction::LocalGet(task_local));
                    func.instruction(&Instruction::End);
                    self.codes.function(&func);
                    return;
                }
                TrampolineKind::Coroutine => {
                    if closure_size < 0 {
                        panic!("coroutine closure size must be non-negative");
                    }
                    let payload_slots = arity + usize::from(has_closure);
                    let needed = (payload_slots as i64) * 8;
                    if closure_size < needed {
                        panic!("coroutine closure size too small for trampoline");
                    }
                    emit_table_index_i64(&mut func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const(closure_size));
                    func.instruction(&Instruction::I64Const(TASK_KIND_COROUTINE));
                    emit_call(&mut func, reloc_enabled, self.import_ids["task_new"]);
                    func.instruction(&Instruction::LocalSet(task_local));
                    if payload_slots > 0 {
                        func.instruction(&Instruction::LocalGet(task_local));
                        emit_call(&mut func, reloc_enabled, self.import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(base_local));
                        if arity > 0 {
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalSet(args_base_local));
                        }
                        let mut offset = 0;
                        if has_closure {
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(0));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(0));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                            offset += 8;
                        }
                        for idx in 0..arity {
                            let arg_offset = offset + (idx as i32) * 8;
                            func.instruction(&Instruction::LocalGet(args_base_local));
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: (idx * std::mem::size_of::<u64>()) as u64,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(val_local));
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(arg_offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(val_local));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(val_local));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                        }
                    }
                    func.instruction(&Instruction::LocalGet(task_local));
                    emit_call(
                        &mut func,
                        reloc_enabled,
                        self.import_ids["cancel_token_get_current"],
                    );
                    emit_call(
                        &mut func,
                        reloc_enabled,
                        self.import_ids["task_register_token_owned"],
                    );
                    func.instruction(&Instruction::Drop);
                    func.instruction(&Instruction::LocalGet(task_local));
                    func.instruction(&Instruction::End);
                    self.codes.function(&func);
                    return;
                }
                TrampolineKind::AsyncGen => {
                    if closure_size < 0 {
                        panic!("async generator closure size must be non-negative");
                    }
                    let payload_slots = arity + usize::from(has_closure);
                    let needed = GEN_CONTROL_SIZE as i64 + (payload_slots as i64) * 8;
                    if closure_size < needed {
                        panic!("async generator closure size too small for trampoline");
                    }
                    emit_table_index_i64(&mut func, reloc_enabled, table_idx);
                    func.instruction(&Instruction::I64Const(closure_size));
                    func.instruction(&Instruction::I64Const(TASK_KIND_GENERATOR));
                    emit_call(&mut func, reloc_enabled, self.import_ids["task_new"]);
                    func.instruction(&Instruction::LocalSet(task_local));
                    if payload_slots > 0 {
                        func.instruction(&Instruction::LocalGet(task_local));
                        emit_call(&mut func, reloc_enabled, self.import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalSet(base_local));
                        if arity > 0 {
                            func.instruction(&Instruction::LocalGet(1));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalSet(args_base_local));
                        }
                        let mut offset = GEN_CONTROL_SIZE;
                        if has_closure {
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(0));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(0));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                            offset += 8;
                        }
                        for idx in 0..arity {
                            let arg_offset = offset + (idx as i32) * 8;
                            func.instruction(&Instruction::LocalGet(args_base_local));
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: (idx * std::mem::size_of::<u64>()) as u64,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(val_local));
                            func.instruction(&Instruction::LocalGet(base_local));
                            func.instruction(&Instruction::I32Const(arg_offset));
                            func.instruction(&Instruction::I32Add);
                            func.instruction(&Instruction::LocalGet(val_local));
                            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalGet(val_local));
                            emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                        }
                    }
                    func.instruction(&Instruction::LocalGet(task_local));
                    emit_call(&mut func, reloc_enabled, self.import_ids["asyncgen_new"]);
                    func.instruction(&Instruction::End);
                    self.codes.function(&func);
                    return;
                }
                TrampolineKind::Plain => {}
            }
        }
        if has_closure {
            func.instruction(&Instruction::LocalGet(0));
        }
        for idx in 0..arity {
            func.instruction(&Instruction::LocalGet(1));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: (idx * std::mem::size_of::<u64>()) as u64,
                memory_index: 0,
            }));
        }
        emit_call(&mut func, reloc_enabled, target_func_index);
        if let Some(ret_count) = multi_return_count {
            // The target function pushed `ret_count` i64 values onto the
            // stack.  Pop them into temp locals (last return value is on
            // top, so store in reverse order) then reconstruct a tuple.
            let builder_local = mr_locals_start + ret_count as u32;
            for i in (0..ret_count).rev() {
                func.instruction(&Instruction::LocalSet(mr_locals_start + i as u32));
            }
            // list_builder_new(count) -> builder handle
            func.instruction(&Instruction::I64Const(box_int(ret_count as i64)));
            emit_call(
                &mut func,
                reloc_enabled,
                self.import_ids["list_builder_new"],
            );
            func.instruction(&Instruction::LocalSet(builder_local));
            // list_builder_append(builder, value) for each value in order
            for i in 0..ret_count {
                func.instruction(&Instruction::LocalGet(builder_local));
                func.instruction(&Instruction::LocalGet(mr_locals_start + i as u32));
                emit_call(
                    &mut func,
                    reloc_enabled,
                    self.import_ids["list_builder_append"],
                );
            }
            // tuple_builder_finish(builder) -> tuple handle (single i64)
            func.instruction(&Instruction::LocalGet(builder_local));
            emit_call(
                &mut func,
                reloc_enabled,
                self.import_ids["tuple_builder_finish"],
            );
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }

    pub(super) fn compile_table_init(
        &mut self,
        reloc_enabled: bool,
        table_base: u32,
        table_indices: &[u32],
        owned_slot_start: usize,
        shared_abi_slot_end: usize,
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(8);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        for (slot, target_index) in table_indices.iter().enumerate() {
            if slot < owned_slot_start && slot >= shared_abi_slot_end {
                continue;
            }
            let table_index = table_base + slot as u32;
            emit_i32_const(&mut func, reloc_enabled, table_index as i32);
            emit_ref_func(&mut func, reloc_enabled, *target_index);
            func.instruction(&Instruction::TableSet(0));
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    pub(super) fn compile_molt_main_wrapper(
        &mut self,
        reloc_enabled: bool,
        main_index: u32,
        table_init_index: u32,
        manifest_segment: DataSegmentRef,
        manifest_len: u32,
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(0);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        self.emit_host_init_sequence(
            reloc_enabled,
            func_index,
            &mut func,
            table_init_index,
            manifest_segment,
            manifest_len,
        );
        emit_call(&mut func, reloc_enabled, main_index);
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    pub(super) fn emit_host_init_sequence(
        &mut self,
        reloc_enabled: bool,
        func_index: u32,
        func: &mut Function,
        table_init_index: u32,
        manifest_segment: DataSegmentRef,
        manifest_len: u32,
    ) {
        emit_call(func, reloc_enabled, self.import_ids["runtime_init"]);
        func.instruction(&Instruction::Drop);
        if manifest_len > 0 {
            self.emit_data_ptr(reloc_enabled, func_index, func, manifest_segment);
            func.instruction(&Instruction::I64Const(i64::from(manifest_len)));
            emit_call(
                func,
                reloc_enabled,
                self.import_ids["set_intrinsic_manifest"],
            );
            func.instruction(&Instruction::Drop);
        }
        emit_call(func, reloc_enabled, table_init_index);
    }
}
