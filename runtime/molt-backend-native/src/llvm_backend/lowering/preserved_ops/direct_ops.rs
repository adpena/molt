use super::*;
use molt_ir::OpKind;

// Handler-owned authority for LLVM preserved SimpleIR ops lowered directly by
// this module. The dispatcher in `preserved_ops.rs` routes through this slice;
// audit tooling compares it with the adjacent `match kind` arms so routing and
// lowering cannot drift independently. A kind whose runtime entry is exactly
// `molt_<kind>` with a generated positional boxed-ABI row has no arm here: it
// takes the shared admitted boxed-call path.
// Kinds the walker owns (`codegen_ssa::preserved`) never reach this module.
define_llvm_op_kinds!(OpKind::CallAsync,);

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_preserved_direct_op(&mut self, op: &TirOp, kind: OpKind) -> bool {
        match kind {
            OpKind::CallAsync => self.lower_preserved_call_async_op(op),
            _ => unreachable!("llvm preserved direct-op family routed unsupported kind `{kind}`"),
        }
    }
}
