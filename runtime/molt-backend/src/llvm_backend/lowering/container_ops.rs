use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_build_list(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        let n = op.operands.len() as u64;
        let list_new_fn = self
            .backend
            .module
            .get_function("molt_list_builder_new")
            .unwrap();
        let builder = self
            .backend
            .builder
            .build_call(list_new_fn, &[i64_ty.const_int(n, false).into()], "list")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let push_fn = self
            .backend
            .module
            .get_function("molt_list_builder_append")
            .unwrap();
        for &item_id in &op.operands {
            let item_i64 = self.materialize_dynbox_operand(item_id);
            self.backend
                .builder
                .build_call(push_fn, &[builder.into(), item_i64.into()], "list_push")
                .unwrap();
        }
        let finish_fn = self
            .backend
            .module
            .get_function("molt_list_builder_finish")
            .unwrap();
        let list = self
            .backend
            .builder
            .build_call(finish_fn, &[builder.into()], "list_finish")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, list);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_build_dict(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        let n_pairs = (op.operands.len() / 2) as u64;
        let dict_new_fn = self
            .backend
            .module
            .get_function("molt_dict_builder_new")
            .unwrap();
        let builder = self
            .backend
            .builder
            .build_call(
                dict_new_fn,
                &[i64_ty.const_int(n_pairs, false).into()],
                "dict_builder",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let dict_set_fn = self
            .backend
            .module
            .get_function("molt_dict_builder_append")
            .unwrap();
        let mut i = 0;
        while i + 1 < op.operands.len() {
            let k_i64 = self.materialize_dynbox_operand(op.operands[i]);
            let v_i64 = self.materialize_dynbox_operand(op.operands[i + 1]);
            self.backend
                .builder
                .build_call(
                    dict_set_fn,
                    &[builder.into(), k_i64.into(), v_i64.into()],
                    "dict_append",
                )
                .unwrap();
            i += 2;
        }
        let finish_fn = self
            .backend
            .module
            .get_function("molt_dict_builder_finish")
            .unwrap();
        let dict = self
            .backend
            .builder
            .build_call(finish_fn, &[builder.into()], "dict_finish")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, dict);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_build_tuple(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        let n = op.operands.len() as u64;
        let tuple_builder_new = self
            .backend
            .module
            .get_function("molt_list_builder_new")
            .unwrap();
        let builder = self
            .backend
            .builder
            .build_call(
                tuple_builder_new,
                &[i64_ty.const_int(n, false).into()],
                "tuple_builder",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let push_fn = self
            .backend
            .module
            .get_function("molt_list_builder_append")
            .unwrap();
        for &item_id in &op.operands {
            let item_i64 = self.materialize_dynbox_operand(item_id);
            self.backend
                .builder
                .build_call(push_fn, &[builder.into(), item_i64.into()], "tup_push")
                .unwrap();
        }
        let finish_fn = self
            .backend
            .module
            .get_function("molt_tuple_builder_finish")
            .unwrap();
        let tup = self
            .backend
            .builder
            .build_call(finish_fn, &[builder.into()], "tuple_finish")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, tup);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_build_set(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        let n = op.operands.len() as u64;
        let set_new_fn = self
            .backend
            .module
            .get_function("molt_set_builder_new")
            .unwrap();
        let builder = self
            .backend
            .builder
            .build_call(
                set_new_fn,
                &[i64_ty.const_int(n, false).into()],
                "set_builder",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let push_fn = self
            .backend
            .module
            .get_function("molt_set_builder_append")
            .unwrap();
        for &item_id in &op.operands {
            let item_i64 = self.materialize_dynbox_operand(item_id);
            self.backend
                .builder
                .build_call(push_fn, &[builder.into(), item_i64.into()], "set_append")
                .unwrap();
        }
        let finish_fn = self
            .backend
            .module
            .get_function("molt_set_builder_finish")
            .unwrap();
        let set = self
            .backend
            .builder
            .build_call(finish_fn, &[builder.into()], "set_finish")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, set);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_build_slice(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
        let none_val: BasicValueEnum<'ctx> = i64_ty.const_int(none_bits, false).into();

        let start = if !op.operands.is_empty() {
            let v = self.resolve(op.operands[0]);
            self.ensure_i64(v).into()
        } else {
            none_val
        };
        let stop = if op.operands.len() > 1 {
            let v = self.resolve(op.operands[1]);
            self.ensure_i64(v).into()
        } else {
            none_val
        };
        let step = if op.operands.len() > 2 {
            let v = self.resolve(op.operands[2]);
            self.ensure_i64(v).into()
        } else {
            none_val
        };

        let slice_fn = self.backend.module.get_function("molt_slice_new").unwrap();
        let result = self
            .backend
            .builder
            .build_call(slice_fn, &[start.into(), stop.into(), step.into()], "slice")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_get_iter(&mut self, op: &TirOp) {
        let obj = self.resolve(op.operands[0]);
        let obj_i64 = self.ensure_i64(obj);
        let get_iter_fn = self.backend.module.get_function("molt_get_iter").unwrap();
        let result = self
            .backend
            .builder
            .build_call(get_iter_fn, &[obj_i64.into()], "get_iter")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_iter_next(&mut self, op: &TirOp) {
        let iter = self.resolve(op.operands[0]);
        let iter_i64 = self.ensure_i64(iter);
        let iter_next_fn = self.backend.module.get_function("molt_iter_next").unwrap();
        let result = self
            .backend
            .builder
            .build_call(iter_next_fn, &[iter_i64.into()], "iter_next")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_for_iter(&mut self, op: &TirOp) {
        // Vectorization hint: when `vectorize = true` is set on this op (by the
        // vectorize analysis pass), the enclosing loop body is safe to vectorize.
        //
        // Per-loop vectorization metadata (`!{!"llvm.loop.vectorize.enable", i1 1}`)
        // requires attaching an MDNode to the loop back-edge branch instruction.
        // The inkwell API does not expose `LLVMSetMetadata` for branch instructions
        // nor the `MDNode`/`MDString` constructors needed to build loop metadata.
        // Vectorization is still enabled at the function level via `-march=native`
        // in the target machine (which enables +neon on ARM / +avx2 on x86), so
        // LLVM's loop vectorizer will analyze and vectorize eligible loops anyway.
        // To attach per-loop metadata, a raw `llvm-sys::LLVMSetMetadata` call on
        // the back-edge `BranchInst` would be needed.
        let _ = has_attr(op, "vectorize");

        let iter = self.resolve(op.operands[0]);
        let iter_i64 = self.ensure_i64(iter);
        let for_iter_fn = self.backend.module.get_function("molt_for_iter").unwrap();
        let result = self
            .backend
            .builder
            .build_call(for_iter_fn, &[iter_i64.into()], "for_iter")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_iter_next_unboxed_results(&mut self, op: &TirOp) -> bool {
        let Some(&iter_id) = op.operands.first() else {
            return false;
        };
        if op.operands.len() != 1 {
            return false;
        }
        let iter_bits = self.materialize_dynbox_operand(iter_id);
        let i64_ty = self.backend.context.i64_type();
        let val_ptr = self
            .backend
            .builder
            .build_alloca(i64_ty, "iter_next_unboxed_value")
            .unwrap();
        let val_ptr_bits = self
            .backend
            .builder
            .build_ptr_to_int(val_ptr, i64_ty, "iter_next_unboxed_value_ptr")
            .unwrap();
        let iter_next_fn = self.ensure_runtime_i64_fn("molt_iter_next_unboxed", 2);
        let done_bits = self
            .backend
            .builder
            .build_call(
                iter_next_fn,
                &[iter_bits.into(), val_ptr_bits.into()],
                "iter_next_unboxed",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let value_bits = self
            .backend
            .builder
            .build_load(i64_ty, val_ptr, "iter_next_unboxed_value_load")
            .unwrap();
        if let Some(&value_id) = op.results.first() {
            self.values.insert(value_id, value_bits);
            self.value_types.insert(value_id, TirType::DynBox);
        }
        if let Some(&done_id) = op.results.get(1) {
            self.values.insert(done_id, done_bits);
            self.value_types.insert(done_id, TirType::DynBox);
        }
        true
    }

    pub(super) fn emit_iter_next_unboxed(&mut self, op: &TirOp) {
        assert!(
            self.emit_iter_next_unboxed_results(op),
            "IterNextUnboxed requires exactly one iterator operand"
        );
    }
}
