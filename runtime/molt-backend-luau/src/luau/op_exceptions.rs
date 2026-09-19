use super::*;

impl LuauBackend {
    pub(super) fn emit_exception_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "try_start" | "try_end" => {}
            "exception_push"
            | "exception_pop"
            | "exception_stack_clear"
            | "exception_stack_enter"
            | "exception_stack_exit"
            | "exception_stack_depth"
            | "exception_stack_set_depth"
            | "exception_context_set"
            | "exception_clear"
            | "exception_set_last"
            | "exception_last"
            | "exception_last_pending"
            | "exception_active"
            | "exception_current"
            | "exception_pending"
            | "exception_set_value"
            | "exception_set_cause" => {
                let args = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|arg| sanitize_ident(arg))
                    .collect::<Vec<_>>()
                    .join(", ");
                let call = format!("molt_{}({args})", op.kind);
                if let Some(out) = op.out.as_deref().filter(|out| *out != "none") {
                    self.emit_line(&format!("local {} = {call}", sanitize_ident(out)));
                } else {
                    self.emit_line(&call);
                }
            }
            "drop_inserted"
            | "exception_region_drops_inserted"
            | "inc_ref"
            | "dec_ref"
            | "release" => {
                // Luau is GC-managed, so shared RC/drop markers are consumed no-ops.
            }
            "exception_new" | "exception_new_builtin" | "exception_new_from_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"error\"".to_string());
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = {msg}}}"
                ));
            }
            "exception_new_builtin_empty" => {
                let out = self.out_var(op);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = \"\"}}"
                ));
            }
            "exception_new_builtin_one" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"\"".to_string());
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = {msg}}}"
                ));
            }
            "raise" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("molt_exception_set_last({})", sanitize_ident(val)));
                } else {
                    self.emit_line("molt_exception_reraise()");
                }
            }
            "check_exception" => {
                self.emit_unsupported_op_with_reason(
                    op,
                    "exception observer must use the canonical logical-flow emitter",
                );
            }
            "loop_break_if_exception" => {
                self.emit_line("if molt_exception_pending() then break end");
            }
            "exception_finally_pending_observer" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_exception_last_pending()"));
            }
            "exception_match_builtin" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc) = args.first() {
                    let class_name = op.s_value.as_deref().unwrap_or("Exception");
                    self.emit_line(&format!(
                        "local {out} = molt_exception_match({}, \"{class_name}\")",
                        sanitize_ident(exc)
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "exception_kind" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc_var) = args.first() {
                    let exc = sanitize_ident(exc_var);
                    self.emit_line(&format!("local {out} = molt_exception_kind({exc})"));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "exception_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class_var) = args.first() {
                    let cls = sanitize_ident(class_var);
                    self.emit_line(&format!("local {out} = {cls}"));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "exception_message" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc_var) = args.first() {
                    let exc = sanitize_ident(exc_var);
                    self.emit_line(&format!(
                        "local {out} = (type({exc}) == \"table\" and {exc}.__msg or tostring({exc}))"
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "exceptiongroup_match" | "exceptiongroup_combine" => {
                self.emit_unsupported_op(op);
            }
            _ => return false,
        }
        true
    }
}
