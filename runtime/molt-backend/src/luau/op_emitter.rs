use super::*;

impl LuauBackend {
    pub(super) fn emit_op(&mut self, op: &OpIR) {
        if self.emit_list_op(op) {
            return;
        }
        if self.emit_container_access_op(op) {
            return;
        }
        if self.emit_tuple_op(op) {
            return;
        }
        if self.emit_map_op(op) {
            return;
        }
        if self.emit_set_op(op) {
            return;
        }
        if self.emit_attribute_op(op) {
            return;
        }
        if self.emit_string_op(op) {
            return;
        }
        if self.emit_object_op(op) {
            return;
        }
        if self.emit_value_op(op) {
            return;
        }
        if self.emit_scalar_op(op) {
            return;
        }
        if self.emit_control_op(op) {
            return;
        }
        if self.emit_call_op(op) {
            return;
        }
        if self.emit_runtime_surface_op(op) {
            return;
        }
        if self.emit_iteration_op(op) {
            return;
        }

        match op.kind.as_str() {
            "phi" | "nop" => {}
            _ => self.emit_unsupported_op(op),
        }
    }

    fn emit_unsupported_op(&mut self, op: &OpIR) {
        if let Some(ref out_name) = op.out {
            let out = sanitize_ident(out_name);
            self.emit_line(&format!(
                "local {out} = nil -- [unsupported op: {}]",
                op.kind
            ));
        } else {
            self.emit_line(&format!("-- [unsupported op: {}]", op.kind));
        }
    }
}
