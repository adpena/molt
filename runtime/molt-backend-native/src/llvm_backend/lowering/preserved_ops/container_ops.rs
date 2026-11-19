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
        match kind {
            OpKind::IterNextUnboxed => self.emit_iter_next_unboxed_results(op),
            OpKind::IterNextUnpack2 => self.emit_iter_next_unpack2_results(op),

            OpKind::Len => {
                if op.operands.len() != 1 {
                    return false;
                }
                let function = self.container_len_fn(op.operands[0]);
                let abi = function
                    .boxed_abi()
                    .filter(|abi| abi.arity() == 1)
                    .unwrap_or_else(|| {
                        panic!("{} must have a generated boxed ABI", function.symbol())
                    });
                self.walker_op(|cx, b| cx.boxed_runtime_call(b, op, abi));
                true
            }

            OpKind::ListNew => {
                self.walker_op(|cx, b| cx.fixed_sequence(b, op, RuntimeFn::ListFromValues));
                true
            }

            OpKind::DictNew => {
                self.walker_op(|cx, b| cx.dict_literal(b, op));
                true
            }

            OpKind::TupleNew => {
                self.walker_op(|cx, b| cx.fixed_sequence(b, op, RuntimeFn::TupleFromValues));
                true
            }

            OpKind::SetNew => {
                self.walker_op(|cx, b| {
                    cx.set_aggregate_op(b, op, RuntimeFn::SetNew, RuntimeFn::SetAdd)
                });
                true
            }

            OpKind::FrozensetNew => {
                // Frozenset has no dedicated TIR opcode, but its raw-capacity
                // constructor and in-place construction mutator share the same
                // failure/ownership protocol as dict and set literals.
                self.walker_op(|cx, b| {
                    cx.set_aggregate_op(b, op, RuntimeFn::FrozensetNew, RuntimeFn::FrozensetAdd)
                });
                true
            }

            OpKind::UnpackSequence => {
                let mut lowered = false;
                self.walker_op(|cx, b| lowered = cx.unpack_sequence_op(b, op));
                lowered
            }

            _ => false,
        }
    }
}
