use super::*;

#[path = "preserved_ops/callable_ops.rs"]
mod callable_ops;
#[path = "preserved_ops/container_ops.rs"]
mod container_ops;
#[path = "preserved_ops/direct_ops.rs"]
mod direct_ops;
#[path = "preserved_ops/op_family.rs"]
mod op_family;
#[path = "preserved_ops/vector_reductions.rs"]
mod vector_reductions;

use op_family::LlvmPreservedOpFamily;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_preserved_simpleir_op(&mut self, op: &TirOp, kind: &str) -> bool {
        match op_family::llvm_preserved_op_family(kind) {
            Some(LlvmPreservedOpFamily::Direct) => self.lower_preserved_direct_op(op, kind),
            Some(LlvmPreservedOpFamily::VectorReduction) => {
                self.lower_preserved_vec_reduction_op(op, kind)
            }
            Some(LlvmPreservedOpFamily::Container) => self.lower_preserved_container_op(op, kind),
            Some(LlvmPreservedOpFamily::Callable) => self.lower_preserved_callable_op(op, kind),
            None => {
                // Generic preserved-op runtime-call fallback. Every other operator /
                // conversion kind the frontend emits that has no dedicated TIR opcode
                // is restored from `_original_kind` and lowered as `molt_<kind>` only
                // when the runtime ABI classifier proves the boxed call is exact.
                self.try_lower_preserved_runtime_call(op, kind)
            }
        }
    }
}
