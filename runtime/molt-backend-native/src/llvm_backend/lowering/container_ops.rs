use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
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
