use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
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
}
