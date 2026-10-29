use super::*;

impl LuauBackend {
    pub(super) fn emit_map_op(&mut self, op: &OpIR) -> bool {
        match op.kind {
            molt_ir::OpKind::BuildDict | molt_ir::OpKind::DictNew => {
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
            molt_ir::OpKind::DictClear => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    let tbl = sanitize_ident(tbl);
                    self.emit_line(&format!("molt_dict_clear({tbl})"));
                }
            }
            molt_ir::OpKind::DictCopy => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!("local {out} = molt_dict_copy({src})"));
                }
            }
            molt_ir::OpKind::DictGet => {
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
            molt_ir::OpKind::DictSet => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("molt_dict_set({dict}, {key}, {val})"));
                }
            }
            molt_ir::OpKind::DictSetdefault => {
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
            molt_ir::OpKind::DictSetdefaultEmptyList => {
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
            molt_ir::OpKind::DictPop => {
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
            molt_ir::OpKind::DictUnpack => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() == 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_dict_unpack({dict}, {other})"));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            molt_ir::OpKind::DictUpdate => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_dict_update({dict}, {other})"));
                }
            }
            molt_ir::OpKind::DictUpdateMissing => {
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
            molt_ir::OpKind::DictUpdateKwstar => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_dict_update_kwstar({dict}, {other})"));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            molt_ir::OpKind::DictPopitem => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(dict) = args.first() {
                    let dict = sanitize_ident(dict);
                    self.emit_line(&format!("local {out} = molt_dict_popitem({dict})"));
                }
            }
            // The fused `d[k] = d.get(k, 0) + delta` stands in for the statement
            // only when it runs no Python code; this target cannot check that,
            // so the op declines, `False`, and the statement lowered beside it
            // runs.
            molt_ir::OpKind::DictStrIntInc => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = false"));
            }
            molt_ir::OpKind::DictFromObj => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!("local {out} = molt_dict_from_obj({src})"));
                }
            }
            molt_ir::OpKind::DictKeys => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_keys({})",
                        sanitize_ident(d)
                    ));
                }
            }
            molt_ir::OpKind::DictValues => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_values({})",
                        sanitize_ident(d)
                    ));
                }
            }
            molt_ir::OpKind::DictItems => {
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
