use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_build_list(&mut self, op: &TirOp) {
        self.emit_fixed_sequence(op, "molt_list_from_values");
    }

    pub(super) fn emit_build_tuple(&mut self, op: &TirOp) {
        self.emit_fixed_sequence(op, "molt_tuple_from_values");
    }

    pub(super) fn emit_build_dict(&mut self, op: &TirOp) {
        assert_eq!(
            op.operands.len() % 2,
            0,
            "dict literal needs complete pairs"
        );
        self.emit_owned_hash_aggregate(op, "molt_dict_new", "molt_dict_set", 2);
    }

    /// Fixed-arity lists and tuples: one constructor transaction over one
    /// borrowed word range. The runtime copies and retains every word and
    /// returns None only with an exception pending.
    fn emit_fixed_sequence(&mut self, op: &TirOp, constructor_symbol: &str) {
        let result =
            self.with_borrowed_boxed_operands(&op.operands, "sequence", |this, words, name| {
                let (address, count) = this.store_borrowed_word_range(words, "sequence_values");
                let constructor = this.ensure_runtime_i64_fn(constructor_symbol, 2);
                this.build_constructor_call(constructor, &[address, count], name)
            });
        self.bind_owned_runtime_result(op, result);
    }

    /// `dataclass_new_values(name, field_names, flags, *values)`: the header
    /// words are direct operands and the values one borrowed range of the same
    /// constructor transaction.
    pub(super) fn emit_dataclass_from_values(&mut self, op: &TirOp) -> bool {
        if op.operands.len() < 3 {
            return false;
        }
        let result =
            self.with_borrowed_boxed_operands(&op.operands, "dataclass", |this, words, name| {
                let (address, count) =
                    this.store_borrowed_word_range(&words[3..], "dataclass_values");
                let constructor = this.ensure_runtime_i64_fn("molt_dataclass_new_from_values", 5);
                this.build_constructor_call(
                    constructor,
                    &[words[0], words[1], address, count, words[2]],
                    name,
                )
            });
        self.bind_owned_runtime_result(op, result);
        true
    }

    /// Tuple-form `dataclass_new(name, field_names, values, flags)`: four direct
    /// operands of one constructor transaction.
    pub(super) fn emit_dataclass_new(&mut self, op: &TirOp) -> bool {
        if op.operands.len() != 4 {
            return false;
        }
        let result =
            self.with_borrowed_boxed_operands(&op.operands, "dataclass", |this, words, name| {
                let constructor = this.ensure_runtime_i64_fn("molt_dataclass_new", 4);
                this.build_constructor_call(constructor, words, name)
            });
        self.bind_owned_runtime_result(op, result);
        true
    }

    /// `class_def(name, *bases, *namespace_pairs)`: the bases and the namespace
    /// pairs are two borrowed ranges of one constructor transaction.
    pub(super) fn emit_class_definition(
        &mut self,
        op: &TirOp,
        nbases: usize,
        nattrs: usize,
        layout_size: i64,
        layout_version: i64,
        flags: i64,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let result =
            self.with_borrowed_boxed_operands(&op.operands, "class", |this, words, name| {
                let (bases, base_count) =
                    this.store_borrowed_word_range(&words[1..1 + nbases], "class_bases");
                let (attrs, _) =
                    this.store_borrowed_word_range(&words[1 + nbases..], "class_attrs");
                let constructor = this.ensure_runtime_i64_fn("molt_guarded_class_def", 8);
                this.build_constructor_call(
                    constructor,
                    &[
                        words[0],
                        bases,
                        base_count,
                        attrs,
                        i64_ty.const_int(nattrs as u64, false),
                        i64_ty.const_int(layout_size as u64, true),
                        i64_ty.const_int(layout_version as u64, true),
                        i64_ty.const_int(flags as u64, true),
                    ],
                    name,
                )
            });
        self.bind_owned_runtime_result(op, result);
    }

    /// Store one borrowed word range in a static entry-block slot owned by this
    /// operation and return its address and word count. Every execution reuses
    /// the slot, so construction in a loop never grows the stack. An empty range
    /// transports a null address.
    fn store_borrowed_word_range(
        &self,
        words: &[inkwell::values::IntValue<'ctx>],
        name: &str,
    ) -> (
        inkwell::values::IntValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let count = i64_ty.const_int(words.len() as u64, false);
        if words.is_empty() {
            return (i64_ty.const_zero(), count);
        }
        let range = self.build_entry_i64_array_alloca(words.len() as u64, name);
        for (index, &word) in words.iter().enumerate() {
            let slot = unsafe {
                self.backend
                    .builder
                    .build_gep(
                        i64_ty,
                        range,
                        &[i64_ty.const_int(index as u64, false)],
                        &format!("{name}_slot"),
                    )
                    .unwrap()
            };
            self.backend.builder.build_store(slot, word).unwrap();
        }
        let address = self
            .backend
            .builder
            .build_ptr_to_int(range, i64_ty, &format!("{name}_ptr"))
            .unwrap();
        (address, count)
    }

    fn build_constructor_call(
        &self,
        constructor: FunctionValue<'ctx>,
        args: &[inkwell::values::IntValue<'ctx>],
        name: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            args.iter().map(|&arg| arg.into()).collect();
        self.backend
            .builder
            .build_call(constructor, &args, name)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    pub(super) fn emit_build_set(&mut self, op: &TirOp) {
        self.emit_owned_hash_aggregate(op, "molt_set_new", "molt_set_add", 1);
    }

    pub(super) fn emit_build_frozenset(&mut self, op: &TirOp) {
        self.emit_owned_hash_aggregate(op, "molt_frozenset_new", "molt_frozenset_add", 1);
    }

    /// Direct construction owns the actual dict/set/frozenset from allocation to
    /// commit. Mutators borrow every operand. All entries share one
    /// borrowed-operand custody whose operands are requested lazily, per entry,
    /// only after the previous insertion succeeded: a later entry is never boxed
    /// before an earlier entry's hashing and insertion ran, a value repeated
    /// across entries keeps one identity, and a failed box, allocation or
    /// insertion releases the partial aggregate and every minted box while the
    /// first exception stays pending. No scratch heap kind, borrowed-edge builder
    /// or finish lane exists.
    fn emit_owned_hash_aggregate(
        &mut self,
        op: &TirOp,
        new_symbol: &str,
        mutate_symbol: &str,
        width: usize,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let mut custody = self.begin_borrowed_operands(&op.operands, "aggregate");
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
        self.borrowed_operands_continue_if(&mut custody, created, "ready");
        let mutate = self.ensure_runtime_i64_fn(mutate_symbol, width + 1);
        let mutation_return = runtime_boxed_abi(mutate_symbol, width + 1)
            .unwrap_or_else(|| panic!("hash aggregate mutator {mutate_symbol} has no boxed ABI"))
            .result;
        let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
        for entry in op.operands.chunks(width) {
            let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                vec![aggregate.into()];
            for &operand in entry {
                args.push(self.borrowed_operand(&mut custody, operand).into());
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
            self.borrowed_operands_continue_if_clear(&mut custody, "inserted");
        }
        let result = self.finish_borrowed_operands(custody, aggregate, "aggregate_result", |this| {
            let release = this.ensure_runtime_import(MOLT_DEC_REF_OBJ);
            this.backend
                .builder
                .build_call(release, &[aggregate.into()], "aggregate_abort_release")
                .unwrap();
        });
        self.bind_owned_runtime_result(op, result.into());
    }

    /// `slice(start, stop, step)`: the present bounds are direct operands of one
    /// constructor transaction; an omitted bound is boxed None, which needs no
    /// materialization, owner or range.
    pub(super) fn emit_build_slice(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        if op.operands.len() > 3 {
            self.record_fatal(format!(
                "BuildSlice takes at most start, stop and step, got {} operands",
                op.operands.len()
            ));
            let undef: BasicValueEnum<'ctx> = i64_ty.get_undef().into();
            for &result in &op.results {
                self.values.insert(result, undef);
                self.value_types.insert(result, TirType::DynBox);
            }
            return;
        }
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let result =
            self.with_borrowed_boxed_operands(&op.operands, "slice", |this, words, name| {
                let bound = |index: usize| words.get(index).copied().unwrap_or(none);
                let constructor = this.ensure_runtime_i64_fn("molt_slice_new", 3);
                this.build_constructor_call(constructor, &[bound(0), bound(1), bound(2)], name)
            });
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
        let i64_ty = self.backend.context.i64_type();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let val_ptr = self.build_entry_i64_alloca("iter_next_unboxed_value");
        // The iterator is borrowed by the runtime; a box minted for a raw
        // carrier is released after the call, and a failed box skips it.
        let mut custody = self.begin_borrowed_operands(&op.operands, "iter_next_unboxed");
        let iter_bits = self.borrowed_operand(&mut custody, iter_id);
        let val_ptr_bits = self
            .backend
            .builder
            .build_ptr_to_int(val_ptr, i64_ty, "iter_next_unboxed_value_ptr")
            .unwrap();
        let iter_next_fn = self.ensure_runtime_i64_fn("molt_iter_next_unboxed", 2);
        let call_name = if custody.can_fail() {
            "iter_next_unboxed_call"
        } else {
            "iter_next_unboxed"
        };
        let done_bits = self
            .backend
            .builder
            .build_call(
                iter_next_fn,
                &[iter_bits.into(), val_ptr_bits.into()],
                call_name,
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        // An iterator that could not be boxed was never advanced: its value
        // slot publishes None, never a previous iteration's word.
        let done_bits = self.finish_borrowed_operands(custody, done_bits, "iter_next_unboxed", |this| {
            this.backend.builder.build_store(val_ptr, none).unwrap();
        });
        let value_bits = self
            .backend
            .builder
            .build_load(i64_ty, val_ptr, "iter_next_unboxed_value_load")
            .unwrap();
        let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
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
            self.values.insert(done_id, done_bits.into());
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
