use super::*;

impl RustBackend {
    pub(super) fn emit_op_builtin_func(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let builtin = op.s_value.as_deref().unwrap_or("");
        self.emit_line(&declare(
            &o,
            &format!("molt_builtin_func({})", rust_string_literal(builtin)),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_print(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        let arg_list = args
            .iter()
            .map(|a| rust_clone(a))
            .collect::<Vec<_>>()
            .join(", ");
        self.emit_line(&format!("molt_print(&[{arg_list}]);"));
    }

    pub(super) fn emit_op_len(&mut self, op: &OpIR) {
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
            &format!("molt_len(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_int(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Int(molt_int(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_int_from_obj(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Int(molt_int(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_int_from_str_of_obj(&mut self, op: &OpIR) {
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
        let a = args
            .first()
            .map(|s| rust_value(s))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        let base = args
            .get(1)
            .map(|s| rust_value(s))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        let has_base = args
            .get(2)
            .map(|s| rust_value(s))
            .unwrap_or_else(|| "MoltValue::Bool(false)".to_string());
        self.emit_line(&declare(
                    &o,
                    &format!(
                        "{{ let __s = molt_str(&{a}); if molt_bool(&{has_base}) {{ let __base = molt_int(&{base}); MoltValue::Int(if (2..=36).contains(&__base) {{ i64::from_str_radix(__s.trim(), __base as u32).unwrap_or(0) }} else {{ 0 }}) }} else {{ MoltValue::Int(molt_int(&MoltValue::Str(__s))) }} }}"
                    ),
                    &self.hoisted_vars.clone(),
                ));
    }

    pub(super) fn emit_op_float(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Float(molt_float(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_float_from_obj(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Float(molt_float(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_str(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Str(molt_str(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_bool(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_bool(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_chr(&mut self, op: &OpIR) {
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
            &format!("molt_chr(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_ord(&mut self, op: &OpIR) {
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
            &format!("molt_ord(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_ord_at(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (obj, key) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_ord_at(&{obj}, &{key})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_abs(&mut self, op: &OpIR) {
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
            &format!("molt_abs({a}.clone())"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_build_list(&mut self, op: &OpIR) {
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

    pub(super) fn emit_op_build_dict(&mut self, op: &OpIR) {
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
        // args: [k0, v0, k1, v1, ...]
        let mut pairs = Vec::new();
        let mut i = 0;
        while i + 1 < args.len() {
            let k = rust_ident(&args[i]);
            let v = rust_ident(&args[i + 1]);
            pairs.push(format!("({k}.clone(), {v}.clone())"));
            i += 2;
        }
        let rhs = format!("MoltValue::Dict(vec![{}])", pairs.join(", "));
        self.emit_line(&declare(&o, &rhs, &self.hoisted_vars.clone()));
    }

    pub(super) fn emit_op_list_append(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let list = rust_ident(&args[0]);
            let val = rust_ident(&args[1]);
            self.emit_line(&format!("molt_list_append(&mut {list}, {val}.clone());"));
            self.emit_alias_writeback(&list);
        }
    }

    pub(super) fn emit_op_get_item(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let (obj, key) = args2(op);
        self.emit_line(&declare(
            &o,
            &format!("molt_get_item(&{obj}, &{key})"),
            &self.hoisted_vars.clone(),
        ));
        let alias_key = format!("__alias_key_{o}");
        self.emit_line(&declare(
            &alias_key,
            &format!("{key}.clone()"),
            &self.hoisted_vars.clone(),
        ));
        self.note_indexed_alias(o, obj, alias_key);
    }

    pub(super) fn emit_op_dict_get(&mut self, op: &OpIR) {
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
        let obj = args
            .first()
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        let key = args
            .get(1)
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::None".to_string());
        if let Some(default) = args.get(2) {
            let default = rust_ident(default);
            self.emit_line(&declare(
                        &o,
                        &format!(
                            "{{ let __v = molt_get_item(&{obj}, &{key}); if matches!(__v, MoltValue::None) {{ {default}.clone() }} else {{ __v }} }}"
                        ),
                        &self.hoisted_vars.clone(),
                    ));
        } else {
            self.emit_line(&declare(
                &o,
                &format!("molt_get_item(&{obj}, &{key})"),
                &self.hoisted_vars.clone(),
            ));
        }
    }

    pub(super) fn emit_op_set_item(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 3 {
            let obj = rust_ident(&args[0]);
            let key = rust_ident(&args[1]);
            let val = rust_ident(&args[2]);
            // Record phi→frame mapping so loop_index_next can write back.
            self.phi_to_frame
                .insert(val.clone(), (obj.clone(), key.clone()));
            self.emit_line(&format!(
                "molt_set_item(&mut {obj}, {key}.clone(), {val}.clone());"
            ));
            self.emit_alias_writeback(&obj);
        }
    }

    pub(super) fn emit_op_dict_set(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 3 {
            let obj = rust_ident(&args[0]);
            let key = rust_ident(&args[1]);
            let val = rust_ident(&args[2]);
            self.emit_line(&format!(
                "molt_set_item(&mut {obj}, {key}.clone(), {val}.clone());"
            ));
            self.emit_alias_writeback(&obj);
        }
    }

    pub(super) fn emit_op_get_attr(&mut self, op: &OpIR) {
        let out = || out_var(op);
        let declare = |out_name: &str, rhs: &str, hoisted: &BTreeSet<String>| -> String {
            if hoisted.contains(out_name) {
                format!("{out_name} = {rhs};")
            } else {
                format!("let mut {out_name}: MoltValue = {rhs};")
            }
        };

        let o = out();
        let obj = arg0(op);
        let attr = op
            .s_value
            .as_deref()
            .or_else(|| op.args.as_ref().and_then(|a| a.get(1)).map(|s| s.as_str()))
            .unwrap_or("__unknown__");
        self.emit_line(&declare(
            &o,
            &format!(
                "molt_get_attr(&{obj}, {attr_lit})",
                attr_lit = rust_string_literal(attr)
            ),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_get_attr_name(&mut self, op: &OpIR) {
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
            let obj = rust_value(&args[0]);
            let attr = rust_value(&args[1]);
            self.emit_line(&declare(
                &o,
                &format!("molt_get_attr_name(&{obj}, &{attr})"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
        }
    }

    pub(super) fn emit_op_get_attr_name_default(&mut self, op: &OpIR) {
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
            let obj = rust_value(&args[0]);
            let attr = rust_value(&args[1]);
            let default = args
                .get(2)
                .map(|name| rust_value(name))
                .unwrap_or_else(|| "MoltValue::None".to_string());
            self.emit_line(&declare(
                &o,
                &format!("molt_get_attr_name_default(&{obj}, &{attr}, &{default})"),
                &self.hoisted_vars.clone(),
            ));
        } else {
            self.emit_line(&declare(&o, "MoltValue::None", &self.hoisted_vars.clone()));
        }
    }

    pub(super) fn emit_op_set_attr(&mut self, op: &OpIR) {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let obj = rust_ident(&args[0]);
            let value_index = if args.len() >= 3 { 2 } else { 1 };
            let value = rust_clone(&args[value_index]);
            let attr = op
                .s_value
                .as_deref()
                .or_else(|| args.get(1).map(|s| s.as_str()))
                .unwrap_or("__unknown__");
            if is_assignable_var(&obj) {
                self.emit_line(&format!(
                            "molt_set_attr_name(&mut {obj}, MoltValue::Str({attr_lit}.to_string()), {value});",
                            attr_lit = rust_string_literal(attr)
                        ));
                self.emit_alias_writeback(&obj);
            }
        }
    }

    pub(super) fn emit_op_enumerate(&mut self, op: &OpIR) {
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
        let start = op
            .args
            .as_ref()
            .and_then(|a| a.get(1))
            .map(|s| rust_ident(s))
            .unwrap_or_else(|| "MoltValue::Int(0)".to_string());
        self.emit_line(&declare(
            &o,
            &format!("molt_enumerate(&{a}, molt_int(&{start}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_zip(&mut self, op: &OpIR) {
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
            &format!("molt_zip(&{a}, &{b})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_sorted(&mut self, op: &OpIR) {
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
            &format!("molt_sorted(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_reversed(&mut self, op: &OpIR) {
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
            &format!("molt_reversed(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_sum(&mut self, op: &OpIR) {
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
            &format!("molt_sum(&{a})"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_any(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_any(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_all(&mut self, op: &OpIR) {
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
            &format!("MoltValue::Bool(molt_all(&{a}))"),
            &self.hoisted_vars.clone(),
        ));
    }

    pub(super) fn emit_op_range(&mut self, op: &OpIR) {
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
        let (start, stop, step) = match args.len() {
            1 => (
                "MoltValue::Int(0)".to_string(),
                rust_ident(&args[0]),
                "MoltValue::Int(1)".to_string(),
            ),
            2 => (
                rust_ident(&args[0]),
                rust_ident(&args[1]),
                "MoltValue::Int(1)".to_string(),
            ),
            _ => (
                rust_ident(&args[0]),
                rust_ident(&args[1]),
                rust_ident(&args[2]),
            ),
        };
        self.emit_line(&declare(
            &o,
            &format!("molt_range(molt_int(&{start}), molt_int(&{stop}), molt_int(&{step}))"),
            &self.hoisted_vars.clone(),
        ));
    }
}
