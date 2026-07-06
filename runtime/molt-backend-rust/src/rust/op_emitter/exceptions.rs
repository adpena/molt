use super::*;

impl RustBackend {
    pub(super) fn emit_op_exception_last(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        if o != "_" && o != "none" && !o.is_empty() {
            let helper = if matches!(
                op.kind.as_str(),
                "exception_last_pending" | "exception_finally_pending_observer"
            ) {
                "molt_exception_last_pending()"
            } else {
                "molt_exception_last()"
            };
            self.emit_line(&declare(&o, helper, &self.hoisted_vars.clone()));
        }
    }

    pub(super) fn emit_op_exception_stack_depth(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        if o != "_" && o != "none" && !o.is_empty() {
            let helper = if op.kind == "exception_stack_enter" {
                "molt_exception_stack_enter()"
            } else {
                "molt_exception_stack_depth()"
            };
            self.emit_line(&declare(&o, helper, &self.hoisted_vars.clone()));
        }
    }

    pub(super) fn emit_op_exception_clear(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        if o != "_" && o != "none" && !o.is_empty() {
            self.emit_line(&declare(
                &o,
                "molt_exception_clear()",
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_line("molt_exception_clear();");
        }
    }

    pub(super) fn emit_op_exception_stack_exit(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let prev = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        self.emit_line(&format!("molt_exception_stack_exit(&{prev});"));
    }

    pub(super) fn emit_op_exception_stack_set_depth(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let depth = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        self.emit_line(&format!("molt_exception_stack_set_depth(&{depth});"));
    }

    pub(super) fn emit_op_exception_stack_clear(&mut self, _op: &OpIR) {
        self.emit_line("molt_exception_stack_clear();");
    }

    pub(super) fn emit_op_exception_set_last(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let exc = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        self.emit_line(&format!("molt_exception_set_last(&{exc});"));
    }

    pub(super) fn emit_op_exception_active(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        if o != "_" && o != "none" && !o.is_empty() {
            self.emit_line(&declare(
                &o,
                "molt_exception_active()",
                &self.hoisted_vars.clone(),
            ));
        }
    }

    pub(super) fn emit_op_trace_enter_slot(&mut self, op: &OpIR) {
        let code_id = op.value.unwrap_or(0);
        self.emit_line(&format!("molt_trace_enter_slot({code_id});"));
    }

    pub(super) fn emit_op_trace_exit(&mut self, _op: &OpIR) {
        self.emit_line("molt_trace_exit();");
    }

    pub(super) fn emit_op_frame_locals_set(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let locals = args
            .first()
            .map(|arg| rust_ident(arg))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        self.emit_line(&format!("molt_frame_locals_set(&{locals});"));
    }
}
