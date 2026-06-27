use super::*;

impl LuauBackend {
    pub(super) fn emit_scalar_op(&mut self, op: &OpIR) -> bool {
        self.emit_scalar_expr_op(op)
            || self.emit_scalar_builtin_op(op)
            || self.emit_scalar_kernel_op(op)
    }
}
