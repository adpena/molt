use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_build_list(&mut self, op: &TirOp) {
        self.emit_sequence_builder(op, "molt_list_builder_finish");
    }

    /// Every stored element is owned by the builder. Runtime append borrows,
    /// retains on admission, and reports failure without consuming its input.
    /// Abort releases the partial builder and merges None so the enclosing TIR
    /// exception edge, not a private return, still owns handler/SSA cleanup.
    fn emit_sequence_builder(&mut self, op: &TirOp, finish_symbol: &str) {
        let i64_ty = self.backend.context.i64_type();
        let n = molt_codegen_abi::box_int_bits(op.operands.len() as i64) as u64;
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
        let suffix = self.synthetic_block_counter;
        self.synthetic_block_counter += 1;
        let abort = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("sequence_builder_abort{suffix}"));
        let ready = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("sequence_builder_ready{suffix}"));
        let merge = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("sequence_builder_merge{suffix}"));
        self.all_llvm_blocks.extend([abort, ready, merge]);
        let source = self.backend.builder.get_insert_block().unwrap();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let created = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                builder.into_int_value(),
                none,
                "sequence_builder_created",
            )
            .unwrap();
        self.backend
            .builder
            .build_conditional_branch(created, ready, abort)
            .unwrap();
        self.record_llvm_edge(source, ready);
        self.record_llvm_edge(source, abort);
        self.backend.builder.position_at_end(ready);
        let drop_ref = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
        let push_fn = self
            .backend
            .module
            .get_function("molt_list_builder_append")
            .unwrap();
        for (index, &item_id) in op.operands.iter().enumerate() {
            let (item_i64, owns_box) =
                self.materialize_dynbox_operand_with_temporary_owner(item_id);
            let status = self
                .backend
                .builder
                .build_call(push_fn, &[builder.into(), item_i64.into()], "list_push")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value();
            if owns_box {
                self.backend
                    .builder
                    .build_call(drop_ref, &[item_i64.into()], "sequence_item_release")
                    .unwrap();
            }
            let next = self.backend.context.append_basic_block(
                self.llvm_fn,
                &format!("sequence_builder_next{suffix}_{index}"),
            );
            self.all_llvm_blocks.push(next);
            let source = self.backend.builder.get_insert_block().unwrap();
            let admitted = self
                .backend
                .builder
                .build_int_compare(
                    inkwell::IntPredicate::EQ,
                    status,
                    self.backend.context.i32_type().const_zero(),
                    "sequence_item_admitted",
                )
                .unwrap();
            self.backend
                .builder
                .build_conditional_branch(admitted, next, abort)
                .unwrap();
            self.record_llvm_edge(source, next);
            self.record_llvm_edge(source, abort);
            self.backend.builder.position_at_end(next);
        }
        let finish_fn = self.backend.module.get_function(finish_symbol).unwrap();
        let list = self
            .backend
            .builder
            .build_call(finish_fn, &[builder.into()], "list_finish")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let finished = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_unconditional_branch(merge)
            .unwrap();
        self.record_llvm_edge(finished, merge);
        self.backend.builder.position_at_end(abort);
        self.backend
            .builder
            .build_call(drop_ref, &[builder.into()], "sequence_builder_release")
            .unwrap();
        self.backend
            .builder
            .build_unconditional_branch(merge)
            .unwrap();
        self.record_llvm_edge(abort, merge);
        self.backend.builder.position_at_end(merge);
        let result = self
            .backend
            .builder
            .build_phi(i64_ty, "sequence_builder_result")
            .unwrap();
        result.add_incoming(&[(&list, finished), (&none, abort)]);
        self.bind_owned_runtime_result(op, result.as_basic_value());
    }

    pub(super) fn emit_build_dict(&mut self, op: &TirOp) {
        assert_eq!(
            op.operands.len() % 2,
            0,
            "dict literal needs complete pairs"
        );
        self.emit_owned_hash_aggregate(op, "molt_dict_new", "molt_dict_set", 2);
    }

    pub(super) fn emit_build_tuple(&mut self, op: &TirOp) {
        self.emit_sequence_builder(op, "molt_tuple_builder_finish");
    }

    pub(super) fn emit_build_set(&mut self, op: &TirOp) {
        self.emit_owned_hash_aggregate(op, "molt_set_new", "molt_set_add", 1);
    }

    pub(super) fn emit_build_frozenset(&mut self, op: &TirOp) {
        self.emit_owned_hash_aggregate(op, "molt_frozenset_new", "molt_frozenset_add", 1);
    }

    /// Direct construction owns the actual dict/set/frozenset from allocation to commit.
    /// Mutators borrow every operand; fresh scalar boxes are transaction-local
    /// owners. No scratch heap kind, borrowed-edge builder or finish lane exists.
    fn emit_owned_hash_aggregate(
        &mut self,
        op: &TirOp,
        new_symbol: &str,
        mutate_symbol: &str,
        width: usize,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let temp_slots: Vec<_> = (0..width)
            .map(|_| self.build_entry_i64_alloca("aggregate_temporary_owner"))
            .collect();
        for slot in &temp_slots {
            self.backend.builder.build_store(*slot, none).unwrap();
        }
        let capacity = (op.operands.len() / width) as u64;
        // Hash-aggregate constructors share one raw usize-capacity ABI.
        let new_fn = self.ensure_runtime_i64_fn(new_symbol, 1);
        let aggregate = self
            .backend
            .builder
            .build_call(
                new_fn,
                &[i64_ty.const_int(capacity, false).into()],
                "aggregate",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let suffix = self.synthetic_block_counter;
        self.synthetic_block_counter += 1;
        let abort = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("aggregate_abort{suffix}"));
        let ready = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("aggregate_ready{suffix}"));
        let merge = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("aggregate_merge{suffix}"));
        self.all_llvm_blocks.extend([abort, ready, merge]);
        let created = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                aggregate,
                none,
                "aggregate_created",
            )
            .unwrap();
        let source = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_conditional_branch(created, ready, abort)
            .unwrap();
        self.record_llvm_edge(source, ready);
        self.record_llvm_edge(source, abort);
        self.backend.builder.position_at_end(ready);
        let mutate = self.ensure_runtime_i64_fn(mutate_symbol, width + 1);
        let mutation_return = runtime_boxed_abi(mutate_symbol, width + 1)
            .unwrap_or_else(|| panic!("hash aggregate mutator {mutate_symbol} has no boxed ABI"))
            .result;
        let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
        for (index, operands) in op.operands.chunks(width).enumerate() {
            let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                vec![aggregate.into()];
            for (position, &operand) in operands.iter().enumerate() {
                let (bits, owns_temporary) =
                    self.materialize_dynbox_operand_with_temporary_owner(operand);
                if owns_temporary {
                    self.backend
                        .builder
                        .build_store(temp_slots[position], bits)
                        .unwrap();
                }
                args.push(bits.into());
                self.aggregate_continue_unless_pending(
                    abort,
                    &format!("aggregate_operand{suffix}_{index}_{position}"),
                );
            }
            // Keep the original aggregate owner. Generated boxed-return facts
            // retire owned set/frozenset mutator results; dict_set's borrowed
            // aggregate alias acquires no owner and is simply ignored.
            let mutation_result = self
                .backend
                .builder
                .build_call(mutate, &args, "aggregate_insert")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            match mutation_return {
                RuntimeBoxedReturn::OwnedValue | RuntimeBoxedReturn::PollValue => {
                    self.backend
                        .builder
                        .build_call(
                            release,
                            &[mutation_result.into()],
                            "aggregate_insert_release",
                        )
                        .unwrap();
                }
                RuntimeBoxedReturn::BorrowedValue => {}
                RuntimeBoxedReturn::Void => {
                    panic!("hash aggregate mutator {mutate_symbol} cannot return void")
                }
            }
            for slot in &temp_slots {
                let bits = self
                    .backend
                    .builder
                    .build_load(i64_ty, *slot, "aggregate_temp")
                    .unwrap();
                self.backend
                    .builder
                    .build_call(release, &[bits.into()], "aggregate_temp_release")
                    .unwrap();
                self.backend.builder.build_store(*slot, none).unwrap();
            }
            self.aggregate_continue_unless_pending(
                abort,
                &format!("aggregate_next{suffix}_{index}"),
            );
        }
        let committed = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_unconditional_branch(merge)
            .unwrap();
        self.record_llvm_edge(committed, merge);
        self.backend.builder.position_at_end(abort);
        for slot in &temp_slots {
            let bits = self
                .backend
                .builder
                .build_load(i64_ty, *slot, "aggregate_abort_temp")
                .unwrap();
            self.backend
                .builder
                .build_call(release, &[bits.into()], "aggregate_abort_temp_release")
                .unwrap();
        }
        self.backend
            .builder
            .build_call(release, &[aggregate.into()], "aggregate_abort_release")
            .unwrap();
        self.backend
            .builder
            .build_unconditional_branch(merge)
            .unwrap();
        self.record_llvm_edge(abort, merge);
        self.backend.builder.position_at_end(merge);
        let result = self
            .backend
            .builder
            .build_phi(i64_ty, "aggregate_result")
            .unwrap();
        result.add_incoming(&[(&aggregate, committed), (&none, abort)]);
        self.bind_owned_runtime_result(op, result.as_basic_value());
    }

    fn aggregate_continue_unless_pending(&mut self, abort: BasicBlock<'ctx>, name: &str) {
        let pending_fn = self.ensure_runtime_i64_fn("molt_exception_pending", 0);
        let pending = self
            .backend
            .builder
            .build_call(pending_fn, &[], "aggregate_pending")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let ready = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                pending,
                self.backend.context.i64_type().const_zero(),
                "aggregate_no_exception",
            )
            .unwrap();
        let next = self.backend.context.append_basic_block(self.llvm_fn, name);
        self.all_llvm_blocks.push(next);
        let source = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_conditional_branch(ready, next, abort)
            .unwrap();
        self.record_llvm_edge(source, next);
        self.record_llvm_edge(source, abort);
        self.backend.builder.position_at_end(next);
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
        self.bind_owned_runtime_result(op, result);
    }

    pub(super) fn emit_get_iter(&mut self, op: &TirOp) {
        let abi = runtime_boxed_abi("molt_iter_checked", 1)
            .expect("molt_iter_checked must have a generated boxed ABI");
        self.emit_boxed_runtime_call(op, abi);
    }

    pub(super) fn emit_iter_next(&mut self, op: &TirOp) {
        let abi = runtime_boxed_abi("molt_iter_next", 1)
            .expect("molt_iter_next must have a generated boxed ABI");
        self.emit_boxed_runtime_call(op, abi);
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

        let abi = runtime_boxed_abi("molt_iter_next", 1)
            .expect("molt_iter_next must have a generated boxed ABI");
        self.emit_boxed_runtime_call(op, abi);
    }

    pub(super) fn emit_iter_next_unboxed_results(&mut self, op: &TirOp) -> bool {
        let Some(&iter_id) = op.operands.first() else {
            return false;
        };
        if op.operands.len() != 1 {
            return false;
        }
        let (iter_bits, owns_iter_temporary) =
            self.materialize_dynbox_operand_with_temporary_owner(iter_id);
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
        let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
        if owns_iter_temporary {
            self.backend
                .builder
                .build_call(release, &[iter_bits.into()], "iter_input_release")
                .unwrap();
        }
        if let Some(&value_id) = op.results.first() {
            self.values.insert(value_id, value_bits);
            self.value_types.insert(value_id, TirType::DynBox);
        } else {
            self.backend
                .builder
                .build_call(release, &[value_bits.into()], "iter_value_release")
                .unwrap();
        }
        if let Some(&done_id) = op.results.get(1) {
            self.values.insert(done_id, done_bits);
            self.value_types.insert(done_id, TirType::DynBox);
        } else {
            self.backend
                .builder
                .build_call(release, &[done_bits.into()], "iter_done_release")
                .unwrap();
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
