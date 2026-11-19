use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_build_set(&mut self, op: &TirOp) {
        self.emit_owned_set_aggregate(op, RuntimeFn::SetNew, RuntimeFn::SetAdd);
    }

    pub(super) fn emit_build_frozenset(&mut self, op: &TirOp) {
        self.emit_owned_set_aggregate(op, RuntimeFn::FrozensetNew, RuntimeFn::FrozensetAdd);
    }

    /// Incremental construction owns the actual set/frozenset from allocation to
    /// commit. Mutators borrow every operand. All entries share one
    /// borrowed-operand custody whose operands are requested lazily, per entry,
    /// only after the previous insertion succeeded: a later entry is never boxed
    /// before an earlier entry's hashing and insertion ran, a value repeated
    /// across entries keeps one identity, and a failed box, allocation or
    /// insertion releases the partial aggregate and every minted box while the
    /// first exception stays pending. No scratch heap kind, borrowed-edge builder
    /// or finish lane exists.
    fn emit_owned_set_aggregate(
        &mut self,
        op: &TirOp,
        new_symbol: RuntimeFn,
        mutate_symbol: RuntimeFn,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let mut custody = self.begin_borrowed_operands(&op.operands, "aggregate");
        let capacity = op.operands.len() as u64;
        // Hash-aggregate constructors share one raw usize-capacity ABI.
        let new_fn = self.ensure_runtime_import(new_symbol);
        let aggregate = self
            .backend
            .builder
            .build_execution_call(
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
        let mutate = self.ensure_runtime_import(mutate_symbol);
        let mutation_return = mutate_symbol
            .boxed_abi()
            .filter(|abi| abi.arity() == 2)
            .unwrap_or_else(|| {
                panic!(
                    "hash aggregate mutator {} has no boxed ABI",
                    mutate_symbol.symbol()
                )
            })
            .result;
        let release = self
            .ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Release);
        for &operand in &op.operands {
            let word = self.borrowed_operand(&mut custody, operand);
            let args = [aggregate.into(), word.into()];
            // Keep the original aggregate owner and retire the mutator's
            // generated owned-result contract independently.
            let mutation_result = self
                .backend
                .builder
                .build_execution_call(mutate, &args, "aggregate_insert")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            match mutation_return {
                RuntimeBoxedReturn::OwnedValue | RuntimeBoxedReturn::PollValue => {
                    release.emit(&self.backend.builder, mutation_result);
                }
                RuntimeBoxedReturn::BorrowedValue => {}
                RuntimeBoxedReturn::Void => {
                    panic!(
                        "hash aggregate mutator {} cannot return void",
                        mutate_symbol.symbol()
                    )
                }
            }
            self.borrowed_operands_continue_if_clear(&mut custody, "inserted");
        }
        let result =
            self.finish_borrowed_operands(custody, aggregate, "aggregate_result", |this| {
                let release = this.ensure_refcount_site(
                    molt_codegen_abi::inline_refcount::RefcountOperation::Release,
                );
                release.emit(&this.backend.builder, aggregate.into());
            });
        self.bind_owned_runtime_result(op, result.into());
    }

    pub(super) fn emit_get_iter(&mut self, op: &TirOp) {
        let call = molt_ir::runtime_calls::RuntimeCall::from_opcode(op.opcode)
            .expect("typed runtime-call contract");
        self.walker_op(|cx, b| cx.contract_runtime_call(b, op, call));
    }

    pub(super) fn emit_iter_next(&mut self, op: &TirOp) {
        let abi = RuntimeFn::IterNext
            .boxed_abi()
            .filter(|abi| abi.arity() == 1)
            .expect("molt_iter_next must have a generated boxed ABI");
        self.walker_op(|cx, b| cx.boxed_runtime_call(b, op, abi));
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

        let abi = RuntimeFn::IterNext
            .boxed_abi()
            .filter(|abi| abi.arity() == 1)
            .expect("molt_iter_next must have a generated boxed ABI");
        self.walker_op(|cx, b| cx.boxed_runtime_call(b, op, abi));
    }

    pub(super) fn emit_iter_next_unboxed_results(&mut self, op: &TirOp) -> bool {
        let (mut cx, mut b) = self.walker();
        cx.iterator_out_words(
            &mut b,
            op,
            RuntimeFn::IterNextUnboxed,
            1,
            "iter_next_unboxed",
        )
    }

    pub(super) fn emit_iter_next_unpack2_results(&mut self, op: &TirOp) -> bool {
        let (mut cx, mut b) = self.walker();
        cx.iterator_out_words(
            &mut b,
            op,
            RuntimeFn::IterNextUnpack2,
            2,
            "iter_next_unpack2",
        )
    }

    pub(super) fn emit_iter_next_unpack2(&mut self, op: &TirOp) {
        assert!(
            op.results.len() == 3 && self.emit_iter_next_unpack2_results(op),
            "IterNextUnpack2 requires one iterator operand and three results"
        );
    }

    pub(super) fn emit_iter_next_unboxed(&mut self, op: &TirOp) {
        assert!(
            self.emit_iter_next_unboxed_results(op),
            "IterNextUnboxed requires exactly one iterator operand"
        );
    }
}
