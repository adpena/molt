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

    pub(super) fn emit_op_unpack_sequence(&mut self, op: &OpIR) {
        let Some(source) = molt_tir::tir::simple_def_use::simple_ir_single_read(op) else {
            self.emit_unsupported_op(op, "unpacking requires one source operand");
            return;
        };
        let source = source.name;
        let mut output_count = 0;
        molt_tir::tir::simple_def_use::visit_simple_ir_result_names(op, |_| {
            output_count += 1;
        });
        let expected = op.value.and_then(|value| usize::try_from(value).ok());
        if expected != Some(output_count) {
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
            output_count,
        ));
        molt_tir::tir::simple_def_use::visit_simple_ir_result_names(op, |output| {
            let output = rust_ident(output);
            let assignment = declare_molt_value(
                &output,
                "__molt_unpack_values.next().expect(\"verified unpack arity\")",
                &self.hoisted_vars,
            );
            self.emit_line(&assignment);
        });
        self.pop_indent();
        self.emit_line("}");
    }

    pub(super) fn emit_op_other(&mut self, op: &OpIR) {
        self.emit_unsupported_op(op, format!("unsupported Rust backend op `{}`", op.kind));
    }
}
