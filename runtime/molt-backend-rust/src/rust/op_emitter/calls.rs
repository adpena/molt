use super::*;

impl RustBackend {
    pub(super) fn emit_op_call(&mut self, op: &OpIR) {
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
        if let Some(ref fn_name) = op.s_value {
            // Direct static call with mutable arg-vector writeback.
            let fn_ident = rust_ident(fn_name);
            let call_args: Vec<String> = args.iter().map(|a| rust_clone(a)).collect();
            self.emit_line(&format!(
                "let mut __call_args: Vec<MoltValue> = vec![{}];",
                call_args.join(", ")
            ));
            self.emit_line(&format!(
                "let mut __call_ret: MoltValue = {fn_ident}(&mut __call_args);"
            ));
            for (idx, arg) in args.iter().enumerate() {
                let var = rust_ident(arg);
                if is_assignable_var(&var) {
                    self.emit_line(&format!(
                        "{var} = __call_args.get({idx}).cloned().unwrap_or({var}.clone());"
                    ));
                    self.emit_alias_writeback(&var);
                }
            }
            if o == "_" || o == "none" {
                self.emit_line("__call_ret;");
            } else {
                self.emit_line(&declare(
                    &o,
                    "__call_ret.clone()",
                    &self.hoisted_vars.clone(),
                ));
            }
        } else if args.is_empty() {
            self.emit_unsupported_op(op, "dynamic call requires a callable argument");
        } else {
            // Dynamic call: args[0] is the MoltValue::Func to invoke.
            let func_var = rust_ident(&args[0]);
            let call_args: Vec<String> = args[1..].iter().map(|a| rust_clone(a)).collect();
            self.emit_line(&format!(
                "let mut __call_args: Vec<MoltValue> = vec![{}];",
                call_args.join(", ")
            ));
            self.emit_line(&format!(
                "let mut __call_ret: MoltValue = molt_call(&{func_var}, &mut __call_args);"
            ));
            for (idx, arg) in args[1..].iter().enumerate() {
                let var = rust_ident(arg);
                if is_assignable_var(&var) {
                    self.emit_line(&format!(
                        "{var} = __call_args.get({idx}).cloned().unwrap_or({var}.clone());"
                    ));
                    self.emit_alias_writeback(&var);
                }
            }
            if o == "_" || o == "none" {
                self.emit_line("__call_ret;");
            } else {
                self.emit_line(&declare(
                    &o,
                    "__call_ret.clone()",
                    &self.hoisted_vars.clone(),
                ));
            }
        }
    }

    pub(super) fn emit_op_call_method(&mut self, op: &OpIR) {
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
        // args: [obj, arg0, arg1, ...]; s_value carries the method name.
        let obj = args
            .first()
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "_".to_string());
        let method = op.s_value.as_deref().unwrap_or("");
        let call_args: Vec<String> = args[1..].iter().map(|a| rust_clone(a)).collect();
        if method == "append" {
            let arg = call_args
                .first()
                .cloned()
                .unwrap_or_else(|| "MoltValue::None".to_string());
            self.emit_line(&format!("molt_list_append(&mut {obj}, {arg});"));
            self.emit_alias_writeback(&obj);
            if o != "_" && o != "none" {
                self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
            }
        } else {
            let rhs = match method {
                "keys" => format!("molt_dict_keys(&{obj})"),
                "values" => format!("molt_dict_values(&{obj})"),
                "items" => format!("molt_dict_items(&{obj})"),
                "get" => {
                    let key = call_args
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    let default = call_args
                        .get(1)
                        .cloned()
                        .unwrap_or_else(|| "MoltValue::None".to_string());
                    format!(
                        "{{ let __k = {key}; if let Some((_, v)) = if let MoltValue::Dict(d) = &{obj} {{ d.iter().find(|(k,_)| molt_eq(k, &__k)) }} else {{ None }} {{ v.clone() }} else {{ {default} }} }}"
                    )
                }
                _ => {
                    self.emit_unsupported_op(
                        op,
                        format!("unsupported method `{method}` on `{obj}`"),
                    );
                    return;
                }
            };
            if o == "_" || o == "none" {
                self.emit_line(&format!("{rhs};"));
            } else {
                self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
            }
        }
    }

    pub(super) fn emit_op_call_bind(&mut self, op: &OpIR) {
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
        let rhs = if args.len() >= 2 {
            let func = rust_ident(&args[0]);
            let builder = rust_ident(&args[1]);
            let extra_args = args[2..]
                .iter()
                .map(|a| rust_clone(a))
                .collect::<Vec<_>>()
                .join(", ");
            let extra_stmt = if extra_args.is_empty() {
                String::new()
            } else {
                format!("__call_args.extend(vec![{extra_args}]);")
            };
            format!(
                "{{ let mut __call_args = Vec::new(); \
                           if let MoltValue::List(__pos) = &{builder} {{ \
                               __call_args.extend(__pos.iter().cloned()); \
                           }} else if !matches!({builder}, MoltValue::None) {{ \
                               __call_args.push({builder}.clone()); \
                           }} \
                           {extra_stmt} \
                           let __ret = molt_call(&{func}, &mut __call_args); \
                           __ret }}"
            )
        } else if let Some(func) = args.first() {
            format!(
                "{{ let mut __call_args = Vec::new(); molt_call(&{}, &mut __call_args) }}",
                rust_ident(func)
            )
        } else {
            self.emit_unsupported_op(op, "call_bind requires a callable argument");
            return;
        };
        if o == "_" || o == "none" {
            self.emit_line(&format!("{rhs};"));
        } else {
            self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
        }
    }

    pub(super) fn emit_op_callargs_new(&mut self, op: &OpIR) {
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

    pub(super) fn emit_op_callargs_push_pos(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let list = rust_ident(&args[0]);
            let val = rust_ident(&args[1]);
            self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
            self.emit_alias_writeback(&list);
        } else {
            self.emit_unsupported_op(op, "callargs_push_pos requires builder and value");
        }
    }

    pub(super) fn emit_op_callargs_expand_star(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let list = rust_ident(&args[0]);
            let other = rust_ident(&args[1]);
            self.emit_line(&format!(
                        "for __item in molt_iter_list(&{other}) {{ molt_list_append(&mut {list}, __item); }}"
                    ));
            self.emit_alias_writeback(&list);
        } else {
            self.emit_unsupported_op(op, "callargs_expand_star requires builder and iterable");
        }
    }

    pub(super) fn emit_op_callargs_push_kw(&mut self, op: &OpIR) {
        self.emit_unsupported_op(
            op,
            "keyword argument builders are not supported by the Rust backend",
        );
    }

    pub(super) fn emit_op_func_new(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let Some(ref fn_name) = op.s_value else {
            self.emit_unsupported_op(op, "func_new requires a static function target");
            return;
        };
        let rhs = {
            let fn_ident = rust_ident(fn_name);
            format!("MoltValue::Func(Arc::new(move |args: &mut Vec<MoltValue>| {fn_ident}(args)))")
        };
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_code_new(&mut self, op: &OpIR) {
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
        if args.len() >= 9 {
            let filename = rust_ident(&args[0]);
            let name = rust_ident(&args[1]);
            let firstlineno = rust_ident(&args[2]);
            let linetable = rust_ident(&args[3]);
            let varnames = rust_ident(&args[4]);
            let names = rust_ident(&args[5]);
            let argcount = rust_ident(&args[6]);
            let posonlyargcount = rust_ident(&args[7]);
            let kwonlyargcount = rust_ident(&args[8]);
            self.emit_line(&declare(
                        &o,
                        &format!(
                            "molt_code_new(&{filename}, &{name}, &{firstlineno}, &{linetable}, &{varnames}, &{names}, &{argcount}, &{posonlyargcount}, &{kwonlyargcount})"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
        } else {
            self.emit_unsupported_op(op, "code_new requires its complete 9-argument schema");
        }
    }

    pub(super) fn emit_op_code_slots_init(&mut self, op: &OpIR) {
        let count = op.value.unwrap_or(0);
        self.emit_line(&format!("molt_code_slots_init({count});"));
    }

    pub(super) fn emit_op_code_slot_set(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if let Some(code) = args.first() {
            let code = rust_ident(code);
            let code_id = op.value.unwrap_or(0);
            self.emit_line(&format!("molt_code_slot_set({code_id}, &{code});"));
        } else {
            self.emit_unsupported_op(op, "code_slot_set requires a code object");
        }
    }
}
