use super::*;

impl LuauBackend {
    pub(super) fn emit_op(&mut self, op: &OpIR) {
        if molt_tir::target_admission::simpleir_op_runtime_requirements(op).is_some_and(
            |requirements| {
                requirements.contains(
                    molt_tir::tir::op_kinds_generated::SimpleIrRuntimeRequirements::FRAME_INTROSPECTION,
                )
            },
        ) {
            self.emit_unsupported_op_with_reason(
                op,
                "Python-visible frame objects and trace/profile hooks require exact whole-program locals activation",
            );
            return;
        }
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
        if self.emit_return_op(op) {
            return;
        }
        if self.emit_exception_op(op) {
            return;
        }
        if self.emit_pcall_op(op) {
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

    pub(super) fn emit_unsupported_op(&mut self, op: &OpIR) {
        // The dispatch records failure, but deliberately emits no source.
        // `emit_source` is private and `compile_checked` rejects this record,
        // so no caller can observe either a partial program or a fabricated
        // value for an unsupported operation.
        self.unsupported_ops
            .push(format!("`{}` (luau backend)", op.kind));
    }

    pub(super) fn emit_unsupported_op_with_reason(&mut self, op: &OpIR, reason: impl Into<String>) {
        let reason = reason.into();
        // Preserve the same fail-closed source boundary as the generic path,
        // while making a known target-capability gap actionable.
        self.unsupported_ops
            .push(format!("`{}` (luau backend): {reason}", op.kind));
    }
}
