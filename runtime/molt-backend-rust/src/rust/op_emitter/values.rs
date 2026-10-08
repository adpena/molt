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

    fn emit_literal_value(&mut self, op: &OpIR, rhs: &str) {
        self.emit_line(&declare_molt_value(&out_var(op), rhs, &self.hoisted_vars));
    }

    /// Materialization consumes the shared validated literal value, whose
    /// exhaustive generated shape owns both opcode membership and aliases.
    pub(super) fn emit_op_literal(&mut self, op: &OpIR) -> bool {
        use molt_ir::literal_payload::SimpleLiteral;
        use molt_ir::tir::op_kinds_generated::OwnedLiteralPayloadKind;

        let literal = match SimpleLiteral::from_simple(op) {
            Ok(Some(literal)) => literal,
            Ok(None) => return false,
            Err(reason) => {
                self.emit_unsupported_op(op, reason);
                return true;
            }
        };
        let rhs: Result<String, String> = match literal {
            SimpleLiteral::Int(value) => Ok(format!("MoltValue::Int({value}i64)")),
            SimpleLiteral::Float(value) => Ok(format!(
                "MoltValue::Float(f64::from_bits({}u64))",
                value.to_bits()
            )),
            SimpleLiteral::Bool(value) => Ok(format!("MoltValue::Bool({value})")),
            SimpleLiteral::None => Ok("MoltValue::None".into()),
            SimpleLiteral::Owned(OwnedLiteralPayloadKind::String, payload) => {
                let bytes = payload.as_bytes();
                Ok(format!(
                    "MoltValue::Str(PythonString::from_utf8_surrogatepass(&{bytes:?}).expect(\"admitted Python text\"))"
                ))
            }
            SimpleLiteral::Owned(OwnedLiteralPayloadKind::BigintDecimal, _) => literal
                .exact_integer_value(i64::MAX as u128 + 1)
                .and_then(|value| i64::try_from(value).ok())
                .map(|value| format!("MoltValue::Int({value}i64)"))
                .ok_or_else(|| {
                    "bigint literal exceeds Rust backend i64 value representation".into()
                }),
            SimpleLiteral::Owned(OwnedLiteralPayloadKind::Bytes, _) => {
                Err("bytes literal requires the target's Python object representation".into())
            }
        };
        match rhs {
            Ok(rhs) => self.emit_literal_value(op, &rhs),
            Err(reason) => self.emit_unsupported_op(op, reason),
        }
        true
    }

    pub(super) fn emit_op_warn_stderr(&mut self, op: &OpIR) {
        let Some([source]) = op.args.as_deref() else {
            self.emit_unsupported_op(op, "warning output requires one message operand");
            return;
        };
        let message = rust_value(source);
        self.emit_line(&format!("if let MoltValue::Str(message) = &{message} {{"));
        self.push_indent();
        self.emit_line("use std::io::Write;");
        self.emit_line("let _ = std::io::stdout().flush();");
        self.emit_line("eprintln!(\"{}\", message.to_utf8_backslashreplace());");
        self.pop_indent();
        self.emit_line("}");
    }

    pub(super) fn emit_op_const_ellipsis(&mut self, op: &OpIR) {
        self.emit_literal_value(op, "MoltValue::Ellipsis");
    }

    pub(super) fn emit_op_const_not_implemented(&mut self, op: &OpIR) {
        self.emit_literal_value(op, "MoltValue::NotImplemented");
    }

    /// The runtime's absent-value sentinel. Natively it is a plain `object`
    /// instance, so it is truthy and equal only to itself.
    pub(super) fn emit_op_missing(&mut self, op: &OpIR) {
        self.emit_literal_value(op, "MoltValue::Missing");
    }

    pub(super) fn emit_op_representation_copy(&mut self, op: &OpIR) {
        // compile_checked admits every spelling through the shared wire shape;
        // dispatch's typed round trip preserves this unary operand transport.
        let Some([source]) = op.args.as_deref() else {
            unreachable!("representation conversion must have passed shared shape admission");
        };
        // The source target keeps every admitted value in MoltValue already;
        // neither conversion has a raw carrier to materialize or extract.
        let Some(output) = molt_tir::tir::simple_def_use::simple_ir_out_result(op) else {
            return;
        };
        let output = rust_ident(output);
        self.emit_line(&declare_molt_value(
            &output,
            &rust_clone(source),
            &self.hoisted_vars,
        ));
        if is_assignable_var(source) {
            self.note_alias(output, rust_ident(source));
        }
    }

    pub(super) fn emit_op_local_copy(&mut self, op: &OpIR) {
        // The shared field-role authority distinguishes a slot load from an
        // SSA copy: load_var/copy_var's var is metadata when args is present.
        // Use the same source that CFG liveness and normalization consume.
        let Some(source) = molt_tir::tir::simple_def_use::simple_ir_single_read(op) else {
            self.emit_unsupported_op(op, "local copy requires exactly one source operand");
            return;
        };
        let source = source.name;
        let output = out_var(op);
        self.emit_line(&declare_molt_value(
            &output,
            &rust_clone(source),
            &self.hoisted_vars,
        ));
        if is_assignable_var(source) {
            self.note_alias(output, rust_ident(source));
        }
    }

    pub(super) fn emit_op_store_var(&mut self, op: &OpIR) {
        let Some(binding) = molt_tir::tir::simple_def_use::simple_ir_binding(op) else {
            self.emit_unsupported_op(op, "store_var requires a destination");
            return;
        };
        if binding.destination.is_empty() || binding.destination == "none" {
            self.emit_unsupported_op(
                op,
                "store_var requires a non-empty, non-reserved destination",
            );
        } else {
            let dst = rust_ident(binding.destination);
            let Some(source) = molt_tir::tir::simple_def_use::simple_ir_single_read(op) else {
                self.emit_unsupported_op(op, "store_var requires exactly one source operand");
                return;
            };
            let source = source.name;
            let rhs = rust_clone(source);
            self.emit_line(&format!("{dst} = {rhs};"));
            if is_assignable_var(source) {
                self.note_alias(dst.clone(), rust_ident(source));
            }
            if let Some(result) = binding.result {
                let result = rust_ident(result);
                self.emit_line(&declare_molt_value(
                    &result,
                    &format!("{dst}.clone()"),
                    &self.hoisted_vars,
                ));
                if is_assignable_var(source) {
                    // The snapshot shares the value assigned at this point,
                    // not the mutable destination binding. A later destination
                    // rebind must not retarget or sever the snapshot's alias.
                    self.note_alias(result, rust_ident(source));
                }
            }
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

    /// A frame's binding home: the function-level `__molt_home_<slot>`
    /// variable the function prologue declares. The transpiled program runs
    /// no drop insertion; a home holds its binding as a value.
    fn frame_home_var(&mut self, op: &OpIR) -> Option<String> {
        match op.value {
            Some(slot) if slot >= 0 => Some(format!("__molt_home_{slot}")),
            _ => {
                self.emit_unsupported_op(op, "frame home op requires a code slot");
                None
            }
        }
    }

    pub(super) fn emit_op_frame_home_store(&mut self, op: &OpIR) {
        let Some(home) = self.frame_home_var(op) else {
            return;
        };
        let Some([source]) = op.args.as_deref() else {
            self.emit_unsupported_op(op, "frame home store requires exactly one operand");
            return;
        };
        self.emit_line(&format!("{home} = {};", rust_clone(source)));
        // The result is a view of the stored value.
        if let Some(output) = molt_tir::tir::simple_def_use::simple_ir_out_result(op) {
            let output = rust_ident(output);
            self.emit_line(&declare_molt_value(
                &output,
                &rust_clone(source),
                &self.hoisted_vars,
            ));
            if is_assignable_var(source) {
                self.note_alias(output, rust_ident(source));
            }
        }
    }

    pub(super) fn emit_op_frame_home_load(&mut self, op: &OpIR) {
        let Some(home) = self.frame_home_var(op) else {
            return;
        };
        let output = out_var(op);
        self.emit_line(&declare_molt_value(
            &output,
            &format!("{home}.clone()"),
            &self.hoisted_vars,
        ));
    }

    pub(super) fn emit_op_frame_home_take(&mut self, op: &OpIR) {
        let Some(home) = self.frame_home_var(op) else {
            return;
        };
        let output = out_var(op);
        self.emit_line(&declare_molt_value(
            &output,
            &format!("std::mem::replace(&mut {home}, MoltValue::None)"),
            &self.hoisted_vars,
        ));
    }

    pub(super) fn emit_op_frame_home_clear(&mut self, op: &OpIR) {
        let Some(home) = self.frame_home_var(op) else {
            return;
        };
        self.emit_line(&format!("{home} = MoltValue::None;"));
    }

    pub(super) fn emit_op_phi(&mut self, _op: &OpIR) {

        // Phi nodes are handled by the hoisting logic above; skip here.
    }
}
