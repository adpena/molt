use super::*;

impl LuauBackend {
    pub(super) fn emit_control_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "label" | "state_label" => {
                if let Some(id) = op.value {
                    self.emit_line(&format!("::label_{id}::"));
                } else if let Some(ref s) = op.s_value {
                    let label = sanitize_label(s);
                    self.emit_line(&format!("::{label}::"));
                }
            }
            "jump" | "goto" => {
                if let Some(id) = op.value {
                    self.emit_line(&format!("goto label_{id}"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("goto {target}"));
                }
            }
            "br_if" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    if let Some(id) = op.value {
                        self.emit_line(&format!("if {cond} then goto label_{id} end"));
                    } else if let Some(ref target) = op.s_value {
                        let target = sanitize_label(target);
                        self.emit_line(&format!("if {cond} then goto {target} end"));
                    } else {
                        let cond_ident = sanitize_ident(cond_raw);
                        self.emit_line(&format!(
                            "error(\"[unsupported op: br_if {cond_ident} missing target label]\")"
                        ));
                    }
                }
            }
            "branch" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond_raw = args.first().map(|s| s.as_str()).or(op.var.as_deref());
                let cond = if let Some(raw) = cond_raw {
                    self.guard_truthiness(raw)
                } else {
                    "true".to_string()
                };
                if let Some(id) = op.value {
                    self.emit_line(&format!("if {cond} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if {cond} then goto {target} end"));
                } else {
                    self.emit_line(&format!(
                        "error(\"[unsupported op: branch {cond} missing target label]\")"
                    ));
                }
            }
            "branch_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond_raw = args.first().map(|s| s.as_str()).or(op.var.as_deref());
                let cond = if let Some(raw) = cond_raw {
                    self.guard_truthiness(raw)
                } else {
                    "false".to_string()
                };
                let not_cond = if cond.starts_with("molt_bool(") {
                    format!("not ({cond})")
                } else {
                    format!("not {cond}")
                };
                if let Some(id) = op.value {
                    self.emit_line(&format!("if {not_cond} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if {not_cond} then goto {target} end"));
                } else {
                    self.emit_line(&format!(
                        "error(\"[unsupported op: branch_false {cond} missing target label]\")"
                    ));
                }
            }
            "if" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    let cond_ident = sanitize_ident(cond);
                    if self.is_known_bool_value(cond) {
                        self.emit_line(&format!("if {cond_ident} then"));
                    } else {
                        self.emit_line(&format!("if molt_bool({cond_ident}) then"));
                    }
                    self.push_indent();
                }
            }
            "else" => {
                self.pop_indent();
                self.emit_line("else");
                self.push_indent();
            }
            "end_if" => {
                self.pop_indent();
                self.emit_line("end");
            }
            "loop_start" => {
                self.emit_line("while true do");
                self.push_indent();
            }
            "loop_index_start" => {}
            "loop_end" => {
                self.pop_indent();
                self.emit_line("end");
            }
            "loop_break" => {
                self.emit_line("break");
            }
            "loop_break_if_true" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    self.emit_line(&format!("if {cond} then break end"));
                }
            }
            "loop_break_if_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    if cond.starts_with("molt_bool(") {
                        self.emit_line(&format!("if not ({cond}) then break end"));
                    } else {
                        self.emit_line(&format!("if not {cond} then break end"));
                    }
                }
            }
            "loop_continue" => {
                self.emit_line("continue");
            }
            "loop_index_next" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(new_val) = args.first() {
                        let val = sanitize_ident(new_val);
                        self.emit_line(&format!("{out} = {val}"));
                    }
                }
            }
            "loop_carry_init" | "loop_carry_update" => {}
            "for_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let start = sanitize_ident(&args[0]);
                    let stop = sanitize_ident(&args[1]);
                    let step = if args.len() >= 3 {
                        sanitize_ident(&args[2])
                    } else {
                        "1".to_string()
                    };
                    let limit_expr = if step == "1" {
                        format!("{stop} - 1")
                    } else if step == "-1" {
                        format!("{stop} + 1")
                    } else if let Ok(n) = step.parse::<i64>() {
                        if n > 0 {
                            format!("{stop} - 1")
                        } else {
                            format!("{stop} + 1")
                        }
                    } else {
                        format!("if {step} > 0 then {stop} - 1 else {stop} + 1")
                    };
                    self.emit_line(&format!("for {out} = {start}, {limit_expr}, {step} do"));
                    self.push_indent();
                }
            }
            "for_iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let iterable = sanitize_ident(iterable);
                    self.emit_line(&format!("for _, {out} in ipairs({iterable}) do"));
                    self.push_indent();
                }
            }
            "end_for" => {
                self.pop_indent();
                self.emit_line("end");
            }
            _ => return false,
        }
        true
    }
}
