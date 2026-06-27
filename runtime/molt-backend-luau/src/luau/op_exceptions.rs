use super::*;

impl LuauBackend {
    pub(super) fn emit_exception_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "exception_push"
            | "exception_pop"
            | "exception_stack_clear"
            | "exception_stack_enter"
            | "exception_stack_exit"
            | "exception_set_last"
            | "exception_set_value"
            | "exception_set_cause"
            | "exception_context_set"
            | "exception_stack_set_depth" => {
                // Exception bookkeeping has no separate Luau runtime state.
            }
            "exception_clear" => {
                let explicit_pcall = op.value.and_then(|n| u32::try_from(n).ok());
                if let Some(n) = explicit_pcall.or_else(|| {
                    (!self.inside_pcall_body)
                        .then(|| self.try_depth_counter.last().copied())
                        .flatten()
                }) {
                    self.emit_line(&format!("__err_{n} = nil"));
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
                    self.emit_line(&format!("error({})", sanitize_ident(val)));
                } else {
                    self.emit_line("error(\"raised\")");
                }
            }
            "check_exception" => {
                // Luau exceptions unwind through error()/pcall instead of flag checks.
            }
            "loop_break_if_exception" => {}
            "exception_last" | "exception_last_pending" | "exception_finally_pending_observer" => {
                let out = self.out_var(op);
                if let Some(n) = op.value.and_then(|n| u32::try_from(n).ok()) {
                    self.emit_line(&format!("local {out} = __err_{n}"));
                } else if !self.inside_pcall_body {
                    if let Some(&n) = self.try_depth_counter.last() {
                        self.emit_line(&format!("local {out} = __err_{n}"));
                    } else {
                        self.emit_line(&format!("local {out} = nil -- [exception_last]"));
                    }
                } else {
                    self.emit_line(&format!("local {out} = nil -- [exception_last]"));
                }
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
                    self.emit_line(&format!("local {out} = false"));
                }
            }
            "exception_kind" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc_var) = args.first() {
                    let exc = sanitize_ident(exc_var);
                    self.emit_line(&format!("local {out} = molt_exception_kind({exc})"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "exception_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class_var) = args.first() {
                    let cls = sanitize_ident(class_var);
                    self.emit_line(&format!("local {out} = {cls}"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
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
                    self.emit_line(&format!("local {out} = nil -- [exception_message]"));
                }
            }
            "exception_stack_depth" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = 0"));
            }
            "exceptiongroup_match" | "exceptiongroup_combine" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
            }
            _ => return false,
        }
        true
    }
}
