use super::*;
use molt_tir::tir::simple_def_use::{SimpleIrResultField, visit_simple_ir_results};

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
                    } else {
                        let escaped = escape_luau_string(raw_attr);
                        self.emit_line(&format!(
                            "local {out} = if type({obj}) == \"function\" then molt_func_attr_get({obj}, \"{escaped}\") else molt_get_attr({obj}, \"{escaped}\")"
                        ));
                    }
                }
            }
            "load_method" => {
                // CPython's method-form LOAD_ATTR, lowered as its generic form:
                // the attribute (a bound method for a class function) and no
                // `self`, so the call that follows is an ordinary call.
                let mut pair = [None, None];
                visit_simple_ir_results(op, |result| match result.field {
                    SimpleIrResultField::Var => pair[0] = result.name,
                    SimpleIrResultField::Out => pair[1] = result.name,
                    SimpleIrResultField::Arg(_) => {}
                });
                let attribute = OpIR {
                    kind: "get_attr_generic_obj".to_string(),
                    args: op.args.clone(),
                    s_value: op.s_value.clone(),
                    out: pair[0].map(str::to_string),
                    ..OpIR::default()
                };
                self.emit_attribute_op(&attribute);
                if let Some(self_or_null) = pair[1] {
                    let self_or_null = sanitize_ident(self_or_null);
                    self.emit_line(&format!("local {self_or_null} = nil"));
                }
            }
            "load_super_method" => {
                self.emit_unsupported_op_with_reason(
                    op,
                    "zero-argument super() needs the CPython super object protocol",
                );
            }
            "get_attr_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = if type({obj}) == \"function\" then molt_func_attr_get({obj}, {attr_name}) else molt_get_attr({obj}, {attr_name})"));
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
                    self.emit_line(&format!("local {out} = if type({obj}) == \"function\" then (if molt_func_attr_get({obj}, {attr_name}) ~= nil then molt_func_attr_get({obj}, {attr_name}) else {default}) else molt_get_attr_default({obj}, {attr_name}, {default})"));
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
                    self.emit_line(&format!("local {out} = if type({obj}) == \"function\" then molt_func_attr_get({obj}, {attr_name}) ~= nil else molt_has_attr({obj}, {attr_name})"));
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
                    self.emit_line(&format!("if type({obj}) == \"function\" then molt_function_attr_set({obj}, {attr_name}, {value}) else molt_set_attr({obj}, {attr_name}, {value}) end"));
                }
            }
            "set_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let escaped = escape_luau_string(attr);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("if type({obj}) == \"function\" then molt_function_attr_set({obj}, \"{escaped}\", {value}) else molt_set_attr({obj}, \"{escaped}\", {value}) end"));
                }
            }
            "del_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("if type({obj}) == \"function\" then molt_function_attr_del({obj}, {attr_name}) else molt_del_attr({obj}, {attr_name}) end"));
                }
            }
            "del_attr_generic_obj" | "del_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = escape_luau_string(attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("if type({obj}) == \"function\" then molt_function_attr_del({obj}, \"{attr}\") else molt_del_attr({obj}, \"{attr}\") end"));
                }
            }

            _ => return false,
        }
        true
    }
}
