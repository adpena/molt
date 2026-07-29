use super::*;

impl LuauBackend {
    pub(super) fn emit_return_op(&mut self, op: &OpIR) -> bool {
        match molt_ir::tir::op_kinds_generated::simpleir_return_shape(op.kind.as_str()) {
            molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::Value => {
                let value = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .expect("validated value return owns exactly one operand");
                self.emit_line(&format!("return {}", sanitize_ident(value)));
            }
            molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::Void => {
                self.emit_line("return");
            }
            molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::NotReturn => return false,
        }
        true
    }
}
