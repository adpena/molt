use super::*;

impl RustBackend {
    pub(super) fn emit_op_runtime_value_call(&mut self, op: &OpIR) {
        let Some(call) = runtime_value_call_for_kind(op.kind.as_str()) else {
            self.emit_op_other(op);
            return;
        };
        let rhs = match call.rhs(op) {
            Ok(rhs) => rhs,
            Err(reason) => {
                self.emit_unsupported_op(op, reason);
                return;
            }
        };
        let o = out_var(op);
        if is_assignable_var(&o) {
            self.emit_line(&declare_molt_value(&o, &rhs, &self.hoisted_vars));
        } else {
            self.emit_line(&format!("{rhs};"));
        }
    }

    pub(super) fn emit_op_const(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let rhs = if let Some(v) = op.value {
            format!("MoltValue::Int({v})")
        } else if let Some(f) = op.f_value {
            format!("MoltValue::Float({f:.17})")
        } else if let Some(ref s) = op.s_value {
            format!("MoltValue::Str({}.to_string())", rust_string_literal(s))
        } else {
            self.emit_unsupported_op(op, "constant has no literal payload");
            return;
        };
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_const_float(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let Some(f) = op.f_value else {
            self.emit_unsupported_op(op, "float constant has no f_value payload");
            return;
        };
        let o = out();
        let rhs = format!("MoltValue::Float({f:.17})");
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_const_str(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let Some(s) = op.s_value.as_deref() else {
            self.emit_unsupported_op(op, "string constant has no s_value payload");
            return;
        };
        let o = out();
        let rhs = format!("MoltValue::Str({}.to_string())", rust_string_literal(s));
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_const_bool(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let Some(value) = op.value else {
            self.emit_unsupported_op(op, "bool constant has no value payload");
            return;
        };
        let o = out();
        let b = value != 0;
        let rhs = format!("MoltValue::Bool({b})");
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_const_none(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_const_bytes(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "bytes literals require a Rust backend bytes value representation",
        );
    }

    pub(super) fn emit_op_const_bigint(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let Some(s) = op.s_value.as_deref() else {
            self.emit_unsupported_op(op, "bigint constant has no decimal payload");
            return;
        };
        let o = out();
        if let Ok(value) = s.parse::<i64>() {
            let rhs = format!("MoltValue::Int({value}i64)");
            self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
        } else {
            self.emit_unsupported_op(
                op,
                "bigint literal exceeds Rust backend i64 value representation",
            );
        }
    }

    pub(super) fn emit_op_const_not_implemented(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "literal `{}` requires a dedicated Rust backend value representation",
                op.kind
            ),
        );
    }

    pub(super) fn emit_op_box(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let rhs = op
            .args
            .as_deref()
            .and_then(|args| args.first())
            .map(|src| rust_clone(src))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_load_local(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let v = var_ref(op);
        self.emit_line(&declare(
            &o,
            &format!("{v}.clone()"),
            &self.hoisted_vars.clone(),
        ));
        self.note_alias(o, v);
    }

    pub(super) fn emit_op_load_var(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let v = var_ref(op);
        self.emit_line(&declare(
            &o,
            &format!("{v}.clone()"),
            &self.hoisted_vars.clone(),
        ));
        self.note_alias(o, v);
    }

    pub(super) fn emit_op_store_var(&mut self, op: &OpIR) {
        if let Some(name) = op.var.as_deref().or(op.out.as_deref()) {
            let dst = rust_ident(name);
            self.clear_alias(&dst);
            let rhs = op
                .args
                .as_deref()
                .and_then(|args| args.first())
                .map(|src| rust_clone(src))
                .unwrap_or_else(|| "MoltValue::None".to_string());
            self.emit_line(&format!("{dst} = {rhs};"));
        }
    }

    pub(super) fn emit_op_load(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        if let Some(obj) = op.args.as_ref().and_then(|a| a.first()) {
            let obj = rust_value(obj);
            let slot_key = rust_slot_key(op.value.unwrap_or(0));
            self.emit_line(&declare(
                &o,
                &format!("molt_get_item(&{obj}, &{slot_key})"),
                &self.hoisted_vars.clone(),
            ));
            let alias_key = format!("__alias_key_{o}");
            self.emit_line(&declare(
                &alias_key,
                &format!("{slot_key}.clone()"),
                &self.hoisted_vars.clone(),
            ));
            self.note_indexed_alias(o, obj, alias_key);
        } else {
            self.emit_unsupported_op(op, "load requires a source object");
        }
    }

    pub(super) fn emit_op_closure_load(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let slot = if let Some(slot) = op.args.as_ref().and_then(|a| a.first()) {
            format!("__closure_{}", rust_ident(slot))
        } else if op.var.as_deref().is_some_and(|name| !name.is_empty()) {
            var_ref(op)
        } else {
            self.emit_unsupported_op(op, "closure_load requires a closure slot");
            return;
        };
        self.emit_line(&declare(
            &o,
            &format!("{slot}.clone()"),
            &self.hoisted_vars.clone(),
        ));
        self.note_alias(o, slot);
    }

    pub(super) fn emit_op_store_local(&mut self, op: &OpIR) {
        let v = var_ref(op);
        if let Some(src) = op.args.as_ref().and_then(|a| a.first()) {
            let s = rust_ident(src);
            self.emit_line(&format!("{v} = {s}.clone();"));
            self.note_alias(v, s);
        } else {
            self.emit_unsupported_op(op, "store_local requires a source value");
        }
    }

    pub(super) fn emit_op_store(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let obj = rust_ident(&args[0]);
            let value = rust_clone(&args[1]);
            let slot_key = rust_slot_key(op.value.unwrap_or(0));
            if is_assignable_var(&obj) {
                self.emit_line(&format!("molt_set_item(&mut {obj}, {slot_key}, {value});"));
                self.emit_alias_writeback(&obj);
            }
        } else {
            self.emit_unsupported_op(op, "store requires destination object and value");
        }
    }

    pub(super) fn emit_op_closure_store(&mut self, op: &OpIR) {
        if let Some(args) = &op.args
            && args.len() >= 2
        {
            let slot = format!("__closure_{}", rust_ident(&args[0]));
            let src = rust_ident(&args[1]);
            self.emit_line(&format!("{slot} = {src}.clone();"));
        } else {
            self.emit_unsupported_op(op, "closure_store requires slot and source value");
        }
    }

    pub(super) fn emit_op_phi(&mut self, _op: &OpIR) {

        // Phi nodes are handled by the hoisting logic above; skip here.
    }
}
