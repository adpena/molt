use super::*;

impl RustBackend {
    pub(super) fn emit_op_module_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        self.emit_line(&declare(
            &o,
            "MoltValue::Dict(vec![])",
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_class_new(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "{} requires a Rust backend object/type representation",
                op.kind
            ),
        );
    }

    pub(super) fn emit_op_bound_method_new(&mut self, op: &OpIR) {
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
            let method = rust_value(&args[0]);
            let obj = rust_value(&args[1]);
            self.emit_line(&declare(
                        &o,
                        &format!(
                            "{{ let __bound_method = {method}.clone(); let __bound_self = {obj}.clone(); MoltValue::Func(Arc::new(move |args: &mut Vec<MoltValue>| {{ let mut __bound = vec![__bound_self.clone()]; __bound.extend(args.iter().cloned()); molt_call(&__bound_method, &mut __bound) }})) }}"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
        } else {
            self.emit_unsupported_op(op, "bound_method_new requires method and self");
        }
    }

    pub(super) fn emit_op_alloc_class(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!(
                "{} requires a Rust backend class instance representation",
                op.kind
            ),
        );
    }

    pub(super) fn emit_op_object_set_class(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "object_set_class requires a Rust backend object/type representation",
        );
    }

    pub(super) fn emit_op_class_set_base(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "class_set_base requires a Rust backend class representation",
        );
    }

    pub(super) fn emit_op_class_set_layout_version(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "class_set_layout_version requires a Rust backend class representation",
        );
    }

    pub(super) fn emit_op_class_merge_layout(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "class_merge_layout requires a Rust backend class representation",
        );
    }

    pub(super) fn emit_op_class_apply_set_name(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            format!("{} requires a Rust backend class representation", op.kind),
        );
    }

    pub(super) fn emit_op_module_cache_get(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let name = op
            .args
            .as_deref()
            .and_then(|args| args.first())
            .map(|name| rust_value(name))
            .or_else(|| {
                op.s_value.as_deref().map(|name| {
                    format!("MoltValue::Str({}.to_string())", rust_string_literal(name))
                })
            })
            .unwrap_or_else(|| "MoltValue::None".to_string());
        if o != "_" && o != "none" && !o.is_empty() {
            self.emit_line(&declare(
                &o,
                &format!("molt_module_cache_get(&{name})"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_line(&format!("molt_module_cache_get(&{name});"));
        }
    }

    pub(super) fn emit_op_module_cache_set(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let name = rust_value(&args[0]);
            let module = rust_clone(&args[1]);
            let expr = format!("molt_module_cache_set(&{name}, {module})");
            let o = out();
            if o != "_" && o != "none" && !o.is_empty() {
                self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
            } else {
                self.emit_line(&format!("{expr};"));
            }
        }
    }

    pub(super) fn emit_op_module_cache_del(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let args = op.args.as_deref().unwrap_or(&[]);
        if let Some(name_arg) = args.first() {
            let name = rust_value(name_arg);
            let expr = format!("molt_module_cache_del(&{name})");
            let o = out();
            if o != "_" && o != "none" && !o.is_empty() {
                self.emit_line(&declare(&o, &expr, &self.hoisted_vars.clone()));
            } else {
                self.emit_line(&format!("{expr};"));
            }
        }
    }

    pub(super) fn emit_op_module_import(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let module = op
            .args
            .as_deref()
            .and_then(|args| args.first())
            .map(|name| rust_value(name))
            .or_else(|| {
                op.s_value.as_deref().map(|name| {
                    format!("MoltValue::Str({}.to_string())", rust_string_literal(name))
                })
            })
            .unwrap_or_else(|| "MoltValue::None".to_string());
        self.emit_line(&declare(
            &o,
            &format!("molt_import_module(&{module})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_module_get_attr(&mut self, op: &OpIR) {
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
        if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
            let Some(module) = args.first().map(|name| rust_value(name)) else {
                self.emit_unsupported_op(op, "module_get_attr requires a module object");
                return;
            };
            self.emit_line(&declare(
                &o,
                &format!(
                    "molt_get_attr_name(&{module}, &MoltValue::Str({}.to_string()))",
                    rust_string_literal(attr_str)
                ),
                &self.hoisted_vars.clone(),
            ));
        } else if args.len() >= 2 {
            let module = rust_value(&args[0]);
            let attr = rust_value(&args[1]);
            self.emit_line(&declare(
                &o,
                &format!("molt_get_attr_name(&{module}, &{attr})"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_unsupported_op(op, "module_get_attr requires module and attribute");
        }
    }

    pub(super) fn emit_op_module_set_attr(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 3 {
            let module = rust_ident(&args[0]);
            let attr = rust_clone(&args[1]);
            let value = rust_clone(&args[2]);
            if is_assignable_var(&module) {
                self.emit_line(&format!(
                    "molt_set_attr_name(&mut {module}, {attr}, {value});"
                ));
                self.emit_alias_writeback(&module);
            }
        } else {
            self.emit_unsupported_op(op, "module_set_attr requires module, attribute, and value");
        }
    }
}
