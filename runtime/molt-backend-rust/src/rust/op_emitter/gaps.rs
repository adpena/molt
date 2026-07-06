use super::*;

impl RustBackend {
    pub(super) fn emit_op_nop(&mut self, op: &OpIR) {
        let out = || out_var(op);

        let o = out();
        if o != "_" && o != "none" && !o.is_empty() {
            self.emit_unsupported_op(
                op,
                format!("marker op `{}` unexpectedly produces output", op.kind),
            );
        }
    }

    pub(super) fn emit_op_unstructured_branch(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!("{} requires Rust backend CFG/block lowering", op.kind),
        );
    }

    pub(super) fn emit_op_runtime_control_gap(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "{} requires Rust backend runtime-control representation",
                op.kind
            ),
        );
    }

    pub(super) fn emit_op_inc_ref(&mut self, op: &OpIR) {
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
        if o != "_"
            && o != "none"
            && !o.is_empty()
            && let Some(src) = args.first()
        {
            let src = rust_clone(src);
            self.emit_line(&declare(&o, &src, &self.hoisted_vars.clone()));
        }
    }

    pub(super) fn emit_op_dec_ref(&mut self, _op: &OpIR) {}

    pub(super) fn emit_op_alloc_instance(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!("instance op `{}` has no Rust backend lowering", op.kind),
        );
    }

    pub(super) fn emit_op_raise(&mut self, op: &OpIR) {
        // In stub/native-Rust mode, Python exceptions cannot propagate
        // through the Rust call stack.  Instead of silently returning
        // None (which hides real errors), we panic with context so the
        // failure is immediately visible during testing.
        let msg = if op.args.as_ref().is_none_or(|a| a.is_empty()) {
            "\"Python raise with no argument\"".to_string()
        } else {
            format!(
                "\"Python raise: {{:?}}\", {}",
                &op.args.as_ref().unwrap()[0]
            )
        };
        self.emit_line(&format!("panic!({msg});"));
    }

    pub(super) fn emit_op_try_start(&mut self, _op: &OpIR) {

        // No Rust equivalent in v1 — exception control flow ops are
        // structural markers only.  The actual error handling is done
        // via Result propagation in the generated Rust code.
    }

    pub(super) fn emit_op_format_string(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        // Simple f-string: just convert all args to string and concat
        let args = op.args.as_deref().unwrap_or(&[]);
        let parts = args
            .iter()
            .map(|a| format!("molt_str(&{})", rust_ident(a)))
            .collect::<Vec<_>>()
            .join(" + &");
        let rhs = if parts.is_empty() {
            "MoltValue::Str(String::new())".to_string()
        } else {
            format!("MoltValue::Str({parts})")
        };
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_tuple_new(&mut self, op: &OpIR) {
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
        let items = args
            .iter()
            .map(|a| rust_clone(a))
            .collect::<Vec<_>>()
            .join(", ");
        self.emit_line(&declare(
            &o,
            &format!("MoltValue::List(vec![{items}])"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_list_fill_new(&mut self, op: &OpIR) {
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
        let count = args
            .first()
            .map(|a| rust_ident(a))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        let fill = args
            .get(1)
            .map(|a| rust_ident(a))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        let rhs = format!(
            "{{ let __n = match &{count} {{ MoltValue::Int(v) => (*v).max(0) as usize, MoltValue::Bool(v) => if *v {{ 1 }} else {{ 0 }}, _ => 0 }}; MoltValue::List(vec![{fill}.clone(); __n]) }}"
        );
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_unpack_sequence(&mut self, op: &OpIR) {
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let args = op.args.as_deref().unwrap_or(&[]);
        if let Some(seq_name) = args.first() {
            let seq = rust_ident(seq_name);
            let outputs = &args[1..];
            let expected_count = op.value.unwrap_or(outputs.len() as i64).max(0) as usize;
            self.emit_line(&format!(
                "let __unpack_seq = molt_unpack_sequence(&{seq}, {expected_count});"
            ));
            for (index, out_name) in outputs.iter().take(expected_count).enumerate() {
                let out = rust_ident(out_name);
                self.emit_line(&declare(
                    &out,
                    &format!("__unpack_seq[{index}].clone()"),
                    &self.hoisted_vars.clone(),
                ));
            }
        }
    }

    pub(super) fn emit_op_string_join(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        // string_join(sep, iterable) → sep.join(str(x) for x in iterable)
        let o = out();
        let args = op.args.as_deref().unwrap_or(&[]);
        let sep = args
            .first()
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Str(\"\".to_string())".to_string());
        let seq = args
            .get(1)
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "_seq".to_string());
        let rhs = format!(
            "{{ let __sep = molt_str(&{sep}); if let MoltValue::List(ref __items) = {seq} {{ MoltValue::Str(__items.iter().map(|x| molt_str(x)).collect::<Vec<_>>().join(&__sep)) }} else {{ MoltValue::Str(molt_str(&{seq})) }} }}"
        );
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_other(&mut self, op: &OpIR) {
        let other = op.kind.as_str();

        self.emit_unsupported_op(op, format!("unsupported Rust backend op `{other}`"));
    }
}
