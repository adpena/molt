use super::*;

impl LuauBackend {
    pub(super) fn emit_value_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Constants
            // ================================================================
            "const" => {
                let out = self.out_var(op);
                if let Some(v) = op.value {
                    self.emit_line(&format!("local {out}: number = {v}"));
                } else if let Some(f) = op.f_value {
                    self.emit_line(&format!("local {out}: number = {f}"));
                } else if let Some(ref s) = op.s_value {
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "const_float" => {
                let out = self.out_var(op);
                let val = op.f_value.unwrap_or(0.0);
                self.emit_line(&format!("local {out}: number = {val}"));
            }
            "const_int" => {
                let out = self.out_var(op);
                let val = op.value.unwrap_or(0);
                self.emit_line(&format!("local {out}: number = {val}"));
            }
            "const_str" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out}: string = \"{escaped}\""));
            }
            "const_bytes" => {
                let out = self.out_var(op);
                if let Some(ref bytes) = op.bytes {
                    let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                } else {
                    let s = op.s_value.as_deref().unwrap_or("");
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                }
            }
            "const_bool" | "bool_const" => {
                let out = self.out_var(op);
                let val = if op.value.unwrap_or(0) != 0 {
                    "true"
                } else {
                    "false"
                };
                self.emit_line(&format!("local {out}: boolean = {val}"));
            }
            "const_none" | "none_const" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil"));
            }
            "string_const" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out}: string = \"{escaped}\""));
            }
            "const_bigint" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("0");
                self.emit_line(&format!("local {out} = tonumber(\"{s}\") or 0"));
            }
            "const_not_implemented" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_not_implemented"));
            }
            "const_ellipsis" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- {}", op.kind));
            }
            "missing" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_missing_sentinel"));
            }

            // ================================================================
            // Variable load/store (both pedagogical and real IR forms)
            // ================================================================
            "load_local" => {
                let out = self.out_var(op);
                let var = self.var_ref(op);
                self.emit_line(&format!("local {out} = {var}"));
            }
            "load_var" | "copy_var" => {
                let out = self.out_var(op);
                let var = op
                    .var
                    .as_deref()
                    .or_else(|| {
                        op.args
                            .as_deref()
                            .and_then(|args| args.first().map(String::as_str))
                    })
                    .map(sanitize_ident)
                    .unwrap_or_else(|| "_".to_string());
                self.emit_line(&format!("local {out} = {var}"));
            }
            "load" | "guarded_load" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    // Field offsets are byte offsets in 8-byte MoltValue slots.
                    let slot = (op.value.unwrap_or(0) / 8) + 1;
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("local {out} = {obj}[{slot}]"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "guard_type" | "guard_tag" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let value = sanitize_ident(&args[0]);
                    let expected = sanitize_ident(&args[1]);
                    if let Some(ref out_name) = op.out
                        && out_name != "none"
                    {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_guard_type({value}, {expected})"
                        ));
                    } else {
                        self.emit_line(&format!("molt_guard_type({value}, {expected})"));
                    }
                }
            }
            "closure_load" => {
                // closure_load: args[0] = slot name, out = destination var
                // Reads from closure slot (stored via closure_store).
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(slot) = args.first() {
                    let slot = sanitize_ident(slot);
                    self.emit_line(&format!("local {out} = __closure_{slot}"));
                } else {
                    let var = self.var_ref(op);
                    self.emit_line(&format!("local {out} = {var}"));
                }
            }
            "store_local" => {
                let var = self.var_ref(op);
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    // Propagate tuple tracking: if the source is a known
                    // tuple variable, the destination inherits that status.
                    if self.tuple_vars.contains(src)
                        && let Some(ref var_name) = op.var
                    {
                        self.tuple_vars.insert(var_name.clone());
                    }
                    self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                }
            }
            "store_var" => {
                let var = op
                    .var
                    .as_deref()
                    .or(op.out.as_deref())
                    .map(sanitize_ident)
                    .unwrap_or_else(|| "_".to_string());
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    if self.tuple_vars.contains(src)
                        && let Some(ref var_name) = op.var
                    {
                        self.tuple_vars.insert(var_name.clone());
                    }
                    self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                }
            }
            "store" | "store_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    // Field offsets are byte offsets in 8-byte MoltValue slots.
                    let slot = (op.value.unwrap_or(0) / 8) + 1;
                    let obj = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{obj}[{slot}] = {value}"));
                }
            }
            "closure_store" => {
                // closure_store: args[0] = slot name, args[1] = value
                // Stores value into a closure slot variable.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let slot = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("__closure_{slot} = {value}"));
                }
            }
            "identity_alias" | "binding_alias" => {
                let out = self.out_var(op);
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    // Propagate tuple tracking through aliases.
                    if self.tuple_vars.contains(src)
                        && let Some(ref out_name) = op.out
                    {
                        self.tuple_vars.insert(out_name.clone());
                    }
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(src)));
                }
            }
            _ => return false,
        }
        true
    }
}
