use super::*;
use molt_ir::OpKind;

// Only container operations with custom lowering belong here. Exact
// `molt_<kind>` positional boxed calls deliberately fall through to the
// generated runtime ABI dispatcher so argument boxing, availability, and
// owned/borrowed result custody have one authority.
define_llvm_op_kinds!(
    OpKind::IterNextUnboxed,
    OpKind::IterNextUnpack2,
    OpKind::Len,
    OpKind::ListNew,
    OpKind::DictNew,
    OpKind::TupleNew,
    OpKind::SetNew,
    OpKind::FrozensetNew,
    OpKind::UnpackSequence,
);

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(in crate::llvm_backend::lowering) fn lower_preserved_container_op(
        &mut self,
        op: &TirOp,
        kind: OpKind,
    ) -> bool {
        let mut lowered = false;
        self.walker_op(|cx, b| lowered = cx.container_preserved_op(b, op, kind));
        lowered
    }
}
