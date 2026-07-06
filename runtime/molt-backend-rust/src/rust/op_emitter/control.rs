use super::*;

impl RustBackend {
    pub(super) fn emit_op_and(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("if !molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_or(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (a, b) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("if molt_bool(&{a}) {{ {a}.clone() }} else {{ {b}.clone() }}"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_if(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if molt_bool(&{cond}) {{"));
        self.indent += 1;
    }

    pub(super) fn emit_op_if_not(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if !molt_bool(&{cond}) {{"));
        self.indent += 1;
    }

    pub(super) fn emit_op_else(&mut self, _op: &OpIR) {
        self.indent -= 1;
        self.emit_line("} else {");
        self.indent += 1;
    }

    pub(super) fn emit_op_end_if(&mut self, _op: &OpIR) {
        self.indent -= 1;
        self.emit_line("}");
    }

    pub(super) fn emit_op_loop_start(&mut self, _op: &OpIR) {
        self.emit_line("loop {");
        self.indent += 1;
    }

    pub(super) fn emit_op_loop_end(&mut self, _op: &OpIR) {
        self.indent -= 1;
        self.emit_line("}");
    }

    pub(super) fn emit_op_loop_break_if_false(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if !molt_bool(&{cond}) {{ break; }}"));
    }

    pub(super) fn emit_op_loop_break_if_true(&mut self, op: &OpIR) {
        let cond = arg0(op);
        self.emit_line(&format!("if molt_bool(&{cond}) {{ break; }}"));
    }

    pub(super) fn emit_op_loop_break_if_exception(&mut self, _op: &OpIR) {
        // Value-less exception-flag break: exit an iterator-consumer loop
        // when a runtime exception is pending (the producer returned the
        // None sentinel on a mid-iteration raise).  Reads the same
        // sacrosanct flag the runtime CHECK_EXCEPTION uses; the still
        // pending exception then rides up the lazy-return path.
        self.emit_line("if molt_exception_pending() != 0 { break; }");
    }

    pub(super) fn emit_op_loop_break(&mut self, _op: &OpIR) {
        self.emit_line("break;");
    }

    pub(super) fn emit_op_loop_continue(&mut self, _op: &OpIR) {
        self.emit_line("continue;");
    }

    pub(super) fn emit_op_loop_index_next(&mut self, op: &OpIR) {
        // Update loop index — 1-arg: assign; 2-arg: add-step.
        // After updating the phi var, also write back to the locals frame slot
        // (if any) so that post-loop index reads see the correct value.
        if let Some(ref out_name) = op.out {
            let o = rust_ident(out_name);
            let args = op.args.as_deref().unwrap_or(&[]);
            let new_val_expr = if args.len() >= 2 {
                let current = rust_ident(&args[0]);
                let step = rust_ident(&args[1]);
                format!("molt_add({current}.clone(), {step}.clone())")
            } else if let Some(new_val) = args.first() {
                format!("{}.clone()", rust_ident(new_val))
            } else {
                String::new()
            };
            if !new_val_expr.is_empty() {
                self.emit_line(&format!("{o} = {new_val_expr};"));
                // Write the updated phi value back to the locals frame so
                // post-loop `index` ops read the final (not stale) value.
                if let Some((frame, slot)) = self.phi_to_frame.get(&o).cloned() {
                    self.emit_line(&format!(
                        "molt_set_item(&mut {frame}, {slot}.clone(), {o}.clone());"
                    ));
                }
            }
        }
    }

    pub(super) fn emit_op_loop_index_start(&mut self, _op: &OpIR) {

        // Initialization is handled in the loop preamble above; skip here.
    }

    pub(super) fn emit_op_iter(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let src = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_iter(&{src})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_iter_next(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let iter_var = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_iter_next(&mut {iter_var})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_for_range(&mut self, op: &OpIR) {
        // for_range: args = [out_var, start, stop, step]
        let args = op.args.as_deref().unwrap_or(&[]);
        let iter_var = args
            .first()
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "_".to_string());
        let start = args
            .get(1)
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        let stop = args
            .get(2)
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        let step = args
            .get(3)
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Int(1)".to_string());
        // Emit as a while loop to keep MoltValue
        self.emit_line(&format!("{{ let mut __range_i = molt_int(&{start}); let __range_stop = molt_int(&{stop}); let __range_step = molt_int(&{step});"));
        self.emit_line("while (__range_step > 0 && __range_i < __range_stop) || (__range_step < 0 && __range_i > __range_stop) {");
        self.indent += 1;
        self.emit_line(&format!(
            "let mut {iter_var}: MoltValue = MoltValue::Int(__range_i);"
        ));
    }

    pub(super) fn emit_op_for_iter(&mut self, op: &OpIR) {
        let out = || out_var(op);

        // for_iter (comprehension-inlined): out = loop_var, args[0] = iterable.
        // The comprehension inliner in lib.rs always emits this convention.
        let iter_var = out();
        let iterable = arg0(op);
        self.emit_line(&format!("for {iter_var} in molt_iter_list(&{iterable}) {{"));
        self.indent += 1;
    }

    pub(super) fn emit_op_range_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        // range_new(start, stop, step) — used by comprehension-inlined source_ops.
        let o = out();
        let args = op.args.as_deref().unwrap_or(&[]);
        let start = args
            .first()
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        let stop = args
            .get(1)
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        let step = args
            .get(2)
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Int(1)".to_string());
        self.emit_line(&declare(
            &o,
            &format!("molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_end_for(&mut self, op: &OpIR) {
        // Range loops open an extra block + while; make sure the index
        // advances before closing the while body.
        let closes_range = op.args.as_ref().is_some_and(|args| !args.is_empty());
        if closes_range {
            self.emit_line("__range_i += __range_step;");
        }
        if self.indent > 0 {
            self.indent -= 1;
        }
        self.emit_line("}");
        if closes_range {
            if self.indent > 0 {
                self.indent -= 1;
            }
            self.emit_line("}");
        }
    }

    pub(super) fn emit_op_break(&mut self, _op: &OpIR) {
        self.emit_line("break;");
    }

    pub(super) fn emit_op_continue(&mut self, _op: &OpIR) {
        self.emit_line("continue;");
    }

    pub(super) fn emit_op_return(&mut self, op: &OpIR) {
        if self.current_is_main {
            self.emit_param_writeback();
            self.emit_line("return;");
        } else if let Some(val) = op.args.as_ref().and_then(|a| a.first()) {
            let v = rust_ident(val);
            self.emit_param_writeback();
            self.emit_line(&format!("return {v}.clone();"));
        } else if let Some(ref v) = op.var {
            let v = rust_ident(v);
            self.emit_param_writeback();
            self.emit_line(&format!("return {v}.clone();"));
        } else {
            self.emit_param_writeback();
            self.emit_line("return MoltValue::None;");
        }
    }

    pub(super) fn emit_op_return_none(&mut self, _op: &OpIR) {
        self.emit_param_writeback();
        if self.current_is_main {
            self.emit_line("return;");
        } else {
            self.emit_line("return MoltValue::None;");
        }
    }
}
