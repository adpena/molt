use super::*;

impl RustBackend {
    pub(super) fn emit_op_add(&mut self, op: &OpIR) {
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
        if self.op_prefers_integer_runtime_lane(op) {
            self.emit_line(&declare(
                &o,
                &format!("MoltValue::Int(molt_int_add(molt_int(&{a}), molt_int(&{b})))"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_line(&declare(
                &o,
                &format!("molt_add({a}.clone(), {b}.clone())"),
                &self.hoisted_vars.clone(),
            ));
        }
    }

    pub(super) fn emit_op_sub(&mut self, op: &OpIR) {
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
        if self.op_prefers_integer_runtime_lane(op) {
            self.emit_line(&declare(
                &o,
                &format!("MoltValue::Int(molt_int_sub(molt_int(&{a}), molt_int(&{b})))"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_line(&declare(
                &o,
                &format!("molt_sub({a}.clone(), {b}.clone())"),
                &self.hoisted_vars.clone(),
            ));
        }
    }

    pub(super) fn emit_op_mul(&mut self, op: &OpIR) {
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
        if self.op_prefers_integer_runtime_lane(op) {
            self.emit_line(&declare(
                &o,
                &format!("MoltValue::Int(molt_int_mul(molt_int(&{a}), molt_int(&{b})))"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_line(&declare(
                &o,
                &format!("molt_mul({a}.clone(), {b}.clone())"),
                &self.hoisted_vars.clone(),
            ));
        }
    }

    pub(super) fn emit_op_div(&mut self, op: &OpIR) {
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
            &format!("molt_div({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_floor_div(&mut self, op: &OpIR) {
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
            &format!("molt_floor_div({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_mod(&mut self, op: &OpIR) {
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
            &format!("molt_mod({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_pow(&mut self, op: &OpIR) {
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
            &format!("molt_pow({a}.clone(), {b}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_neg(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_neg({a}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_unary_not(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let a = arg0(op);
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool(!molt_bool(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_band(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Int(molt_int(&{a}) & molt_int(&{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_bor(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Int(molt_int(&{a}) | molt_int(&{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_bxor(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Int(molt_int(&{a}) ^ molt_int(&{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_lshift(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Int(molt_int(&{a}) << (molt_int(&{b}) as u32))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_rshift(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Int(molt_int(&{a}) >> (molt_int(&{b}) as u32))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_eq(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_eq(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_ne(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(!molt_eq(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_lt(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_lt(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_le(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_le(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_gt(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_gt(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_ge(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_ge(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_is(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        // Python `is` — identity check (use == for value equality in Rust)
        let o = out();
        let (a, b) = args2(op);
        let negate = op.kind == "is_not";
        let cmp = if negate { "!" } else { "" };
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool({cmp}molt_eq(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_in(&mut self, op: &OpIR) {
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
        let negate = op.kind == "not_in";
        let prefix = if negate { "!" } else { "" };
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::Bool({prefix}molt_in(&{a}, &{b}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_contains(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let container = rust_ident(&args[0]);
            let value = rust_ident(&args[1]);
            self.emit_line(&declare(
                &o,
                &format!("MoltValue::Bool(molt_in(&{value}, &{container}))"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_unsupported_op(op, "membership comparison requires value and container");
        }
    }
}
