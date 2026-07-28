use super::*;

impl RustBackend {
    pub(super) fn emit_op_nop(&mut self, op: &OpIR) {
        let out = out_var(op);
        if out != "_" && out != "none" && !out.is_empty() {
            self.emit_unsupported_op(
                op,
                format!("marker op `{}` unexpectedly produces output", op.kind),
            );
        }
    }

    pub(super) fn emit_op_unstructured_branch(&mut self, op: &OpIR) {
        self.emit_unsupported_op(op, format!("{} requires CFG/block lowering", op.kind));
    }

    pub(super) fn emit_op_runtime_control_gap(&mut self, op: &OpIR) {
        if op.kind == "async_work_poll" {
            self.emit_unsupported_op(
                op,
                "canonical pending-call/eval-breaker runtime boundary is unavailable",
            );
            return;
        }
        self.emit_unsupported_op(
            op,
            format!("{} requires a runtime-control representation", op.kind),
        );
    }

    pub(super) fn emit_op_inc_ref(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "reference markers require deterministic Python lifetime semantics",
        );
    }

    pub(super) fn emit_op_dec_ref(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "release markers require deterministic Python lifetime semantics",
        );
    }

    pub(super) fn emit_op_alloc_instance(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!("instance op `{}` requires a Python object model", op.kind),
        );
    }

    pub(super) fn emit_op_raise(&mut self, op: &OpIR) {
        self.emit_unsupported_op(op, "raise requires structured Python exception propagation");
    }

    pub(super) fn emit_op_try_start(&mut self, op: &OpIR) {
        self.emit_unsupported_op(op, "exception regions require structured Python unwinding");
    }

    pub(super) fn emit_op_format_string(&mut self, op: &OpIR) {
        self.emit_unsupported_op(op, "formatting requires the Python __format__ protocol");
    }

    pub(super) fn emit_op_tuple_new(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "tuple construction requires a distinct immutable tuple representation",
        );
    }

    pub(super) fn emit_op_list_fill_new(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "list fill requires exact index coercion and aliasing semantics",
        );
    }

    pub(super) fn emit_op_unpack_sequence(&mut self, op: &OpIR) {
        let reads = molt_tir::tir::simple_def_use::simple_ir_read_names(op);
        let outputs = molt_tir::tir::simple_def_use::simple_ir_result_names(op);
        let Some(source) = reads.first() else {
            self.emit_unsupported_op(op, "unpacking requires one source operand");
            return;
        };
        let expected = op.value.and_then(|value| usize::try_from(value).ok());
        if expected != Some(outputs.len()) {
            self.emit_unsupported_op(
                op,
                "unpacking expected count must equal the output-variable count",
            );
            return;
        }

        self.emit_line("{");
        self.push_indent();
        self.emit_line(&format!(
            "let mut __molt_unpack_values = molt_unpack_sequence(&{}, {}).into_iter();",
            rust_value(source),
            outputs.len(),
        ));
        for output in &outputs {
            let output = rust_ident(output);
            let assignment = declare_molt_value(
                &output,
                "__molt_unpack_values.next().expect(\"verified unpack arity\")",
                &self.hoisted_vars,
            );
            self.emit_line(&assignment);
        }
        self.pop_indent();
        self.emit_line("}");
    }

    pub(super) fn emit_op_string_join(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "str.join requires the Python iterable and string-item protocols",
        );
    }

    pub(super) fn emit_op_other(&mut self, op: &OpIR) {
        self.emit_unsupported_op(op, format!("unsupported Rust backend op `{}`", op.kind));
    }
}
