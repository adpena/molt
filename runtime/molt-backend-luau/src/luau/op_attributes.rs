use super::*;

impl LuauBackend {
    pub(super) fn emit_attribute_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Attribute access
            // ================================================================
            "get_attr"
            | "get_attr_generic_obj"
            | "get_attr_generic_ptr"
            | "get_attr_special_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let raw_attr = op.s_value.as_deref().unwrap_or("unknown");
                if let Some(obj) = args.first() {
                    let raw_obj = obj.as_str();
                    let obj = sanitize_ident(raw_obj);
                    let obj_is_str = self.plan_knows_string(raw_obj);
                    if obj_is_str && raw_attr == "removeprefix" {
                        self.emit_line(&format!(
                            "local {out} = function(__args) local __prefix = __args[1]; if __prefix ~= \"\" and string.sub({obj}, 1, #__prefix) == __prefix then return string.sub({obj}, #__prefix + 1) end; return {obj} end"
                        ));
                    } else if obj_is_str && raw_attr == "removesuffix" {
                        self.emit_line(&format!(
                            "local {out} = function(__args) local __suffix = __args[1]; if __suffix ~= \"\" and string.sub({obj}, -#__suffix) == __suffix then return string.sub({obj}, 1, #{obj} - #__suffix) end; return {obj} end"
                        ));
                    } else if obj_is_str
                        && matches!(
                            raw_attr,
                            "isalpha"
                                | "isdigit"
                                | "isalnum"
                                | "isspace"
                                | "isupper"
                                | "islower"
                                | "isidentifier"
                                | "isprintable"
                                | "isdecimal"
                                | "isnumeric"
                                | "istitle"
                        )
                    {
                        self.emit_string_predicate_attr(&out, &obj, raw_attr);
                    } else if raw_attr.starts_with("__") && raw_attr.ends_with("__") {
                        let escaped = escape_luau_string(raw_attr);
                        self.emit_line(&format!(
                            "local {out} = if type({obj}) == \"function\" and molt_func_attrs[{obj}] ~= nil then molt_func_attrs[{obj}][\"{escaped}\"] else molt_get_attr({obj}, \"{escaped}\")"
                        ));
                    } else {
                        let escaped = escape_luau_string(raw_attr);
                        self.emit_line(&format!(
                            "local {out} = molt_get_attr({obj}, \"{escaped}\")"
                        ));
                    }
                }
            }
            "get_attr_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_get_attr({obj}, {attr_name})"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "get_attr_name_default" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    let default = if args.len() >= 3 {
                        sanitize_ident(&args[2])
                    } else {
                        "nil".to_string()
                    };
                    self.emit_line(&format!(
                        "local {out} = molt_get_attr_default({obj}, {attr_name}, {default})"
                    ));
                } else if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    let attr = escape_luau_string(op.s_value.as_deref().unwrap_or("unknown"));
                    self.emit_line(&format!(
                        "local {out} = molt_get_attr_default({obj}, \"{attr}\", nil)"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "has_attr_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_has_attr({obj}, {attr_name})"));
                } else if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    let attr = escape_luau_string(op.s_value.as_deref().unwrap_or("unknown"));
                    self.emit_line(&format!("local {out} = molt_has_attr({obj}, \"{attr}\")"));
                } else {
                    self.emit_line(&format!("local {out} = false"));
                }
            }
            "set_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    self.emit_line(&format!("molt_set_attr({obj}, {attr_name}, {value})"));
                }
            }
            "set_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let escaped = escape_luau_string(attr);
                if attr.starts_with("__") && attr.ends_with("__") {
                    // Functions cannot hold attrs in Luau; table-backed
                    // classes and objects use the normal attribute authority.
                    if args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        self.emit_line(&format!(
                            "if type({obj}) == \"function\" then if molt_func_attrs[{obj}] == nil then molt_func_attrs[{obj}] = {{}} end; molt_func_attrs[{obj}][\"{escaped}\"] = {value} else molt_set_attr({obj}, \"{escaped}\", {value}) end"
                        ));
                    }
                } else {
                    if args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        self.emit_line(&format!("molt_set_attr({obj}, \"{escaped}\", {value})"));
                    }
                }
            }
            "del_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_del_attr({obj}, {attr_name})"));
                }
            }
            "del_attr_generic_obj" | "del_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = escape_luau_string(attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("molt_del_attr({obj}, \"{attr}\")"));
                }
            }

            _ => return false,
        }
        true
    }
}
