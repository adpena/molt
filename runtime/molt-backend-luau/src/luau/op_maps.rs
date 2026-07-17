use super::*;

impl LuauBackend {
    pub(super) fn emit_map_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "build_dict" | "dict_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                self.emit_line(&format!("local {out}: {{[any]: any}} = molt_dict_new()"));
                for pair in args.chunks(2) {
                    if pair.len() == 2 {
                        let key = sanitize_ident(&pair[0]);
                        let val = sanitize_ident(&pair[1]);
                        self.emit_line(&format!("molt_dict_set({out}, {key}, {val})"));
                    }
                }
            }
            "dict_clear" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    let tbl = sanitize_ident(tbl);
                    self.emit_line(&format!("molt_dict_clear({tbl})"));
                }
            }
            "dict_copy" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!("local {out} = molt_dict_copy({src})"));
                }
            }
            "dict_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = molt_dict_get({dict}, {key}, {default})"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = molt_dict_get({dict}, {key}, nil)"));
                    }
                }
            }
            "dict_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("molt_dict_set({dict}, {key}, {val})"));
                }
            }
            "dict_setdefault" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_dict_setdefault({dict}, {key}, {val})"
                        ));
                    } else {
                        self.emit_line(&format!("molt_dict_setdefault({dict}, {key}, {val})"));
                    }
                }
            }
            "dict_setdefault_empty_list" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_dict_setdefault_empty_list({dict}, {key})"
                        ));
                    } else {
                        self.emit_line(&format!("molt_dict_setdefault_empty_list({dict}, {key})"));
                    }
                }
            }
            "dict_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = molt_dict_pop({dict}, {key}, true, {default})"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = molt_dict_pop({dict}, {key}, false, nil)"
                        ));
                    }
                }
            }
            "dict_update" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_dict_update({dict}, {other})"));
                }
            }
            "dict_update_missing" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "molt_dict_update_missing({dict}, {key}, {value}, molt_missing_sentinel)"
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "dict_update_kwstar" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_dict_update_kwstar({dict}, {other})"));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "dict_popitem" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(dict) = args.first() {
                    let dict = sanitize_ident(dict);
                    self.emit_line(&format!("local {out} = molt_dict_popitem({dict})"));
                }
            }
            "dict_inc" | "dict_str_int_inc" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let inc = sanitize_ident(&args[2]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_dict_inc({dict}, {key}, {inc})"
                        ));
                    } else {
                        self.emit_line(&format!("molt_dict_inc({dict}, {key}, {inc})"));
                    }
                }
            }
            "dict_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!("local {out} = molt_dict_from_obj({src})"));
                }
            }
            "dict_keys" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_keys({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_values" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_values({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_items" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_items({})",
                        sanitize_ident(d)
                    ));
                }
            }
            _ => return false,
        }
        true
    }
}
