use super::*;

impl LuauBackend {
    pub(super) fn emit_object_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Guarded field access
            // ================================================================
            // Luau tables hold attributes by name, so a layout guard has no
            // fast path to admit: operands = [obj, expected_version] for a
            // read and [obj, expected_version, value] for a write (HF-220).
            "guarded_field_get" => {
                let out = self.out_var(op);
                let args = op
                    .args
                    .as_deref()
                    .expect("admitted guarded_field_get operands");
                let obj = sanitize_ident(&args[0]);
                let attr = sanitize_ident(op.s_value.as_deref().expect("admitted field name"));
                self.emit_line(&format!("local {out} = {obj}.{attr}"));
            }
            "guarded_field_set" => {
                let args = op
                    .args
                    .as_deref()
                    .expect("admitted guarded_field_set operands");
                let obj = sanitize_ident(&args[0]);
                let val = sanitize_ident(&args[2]);
                let attr = sanitize_ident(op.s_value.as_deref().expect("admitted field name"));
                self.emit_line(&format!("{obj}.{attr} = {val}"));
            }

            // ================================================================
            // Function/class/module objects
            // ================================================================
            "class_new" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{__molt_is_type = true}}"));
                // Self-referential __index enables Luau's inline caching
                self.emit_line(&format!("{out}.__index = {out}"));
            }
            "module_new" | "object_new" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }
            "builtin_type" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let tag = args
                    .first()
                    .map(|value| sanitize_ident(value))
                    .unwrap_or_else(|| "nil".to_string());
                self.emit_line(&format!("local {out} = molt_builtin_type({tag})"));
            }
            "super_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class = sanitize_ident(&args[0]);
                    self.emit_line(&format!(
                        "local {out} = setmetatable({{}}, {{__index = getmetatable({class}).__index}})"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }
            "classmethod_new" | "staticmethod_new" | "property_new" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    match op.kind.as_str() {
                        "classmethod_new" => {
                            let func = args
                                .first()
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            self.emit_line(&format!(
                                "local {out} = {{__molt_descriptor_kind=\"classmethod\", __func={func}}}"
                            ));
                        }
                        "staticmethod_new" => {
                            let func = args
                                .first()
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            self.emit_line(&format!(
                                "local {out} = {{__molt_descriptor_kind=\"staticmethod\", __func={func}}}"
                            ));
                        }
                        "property_new" => {
                            let get = args
                                .first()
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            let set = args
                                .get(1)
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            let del = args
                                .get(2)
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            self.emit_line(&format!(
                                "local {out} = {{__molt_descriptor_kind=\"property\", __get={get}, __set={set}, __del={del}}}"
                            ));
                        }
                        _ => unreachable!(),
                    }
                }
            }

            "class_set_base" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class = sanitize_ident(&args[0]);
                    let base = sanitize_ident(&args[1]);
                    self.emit_line(&format!("setmetatable({class}, {{__index = {base}}})"));
                }
            }
            "object_set_class" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let class = sanitize_ident(&args[1]);
                    self.emit_line(&format!("setmetatable({obj}, {class})"));
                }
            }
            "class_layout_version" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(class) = args.first() {
                        let class = sanitize_ident(class);
                        self.emit_line(&format!(
                            "local {out} = if type({class}) == \"table\" and type({class}.__molt_layout_version) == \"number\" then {class}.__molt_layout_version else 0"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = 0"));
                    }
                }
            }
            // The class statement's program slot, recorded beside the layout
            // version on the class table it created (HF-220).
            "class_set_definition" => {
                let args = op
                    .args
                    .as_deref()
                    .expect("admitted class_set_definition operand");
                let class = sanitize_ident(&args[0]);
                let slot = self
                    .code_slots
                    .resolve_class_definition(op)
                    .expect("admitted class statement binding");
                self.emit_line(&format!(
                    "if type({class}) == \"table\" then {class}.__molt_class_definition = {slot} end"
                ));
            }
            "class_apply_set_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class) = args.first() {
                    let class = sanitize_ident(class);
                    self.emit_line(&format!("molt_class_apply_set_name({class})"));
                }
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_import" | "module_import_from" | "module_import_star" => {
                self.emit_unsupported_op(op);
            }
            "module_cache_get" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(name_var) = args.first() {
                        let nv = sanitize_ident(name_var);
                        self.emit_line(&format!("local {out} = molt_module_cache[{nv}]"));
                    } else {
                        self.emit_line(&format!("local {out} = nil"));
                    }
                }
            }
            "module_cache_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let name = sanitize_ident(&args[0]);
                    let module = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_module_cache[{name}] = {module}"));
                }
            }
            "module_cache_del" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(name_var) = args.first() {
                    let name = sanitize_ident(name_var);
                    self.emit_line(&format!("molt_module_cache[{name}] = nil"));
                }
            }
            "module_get_global" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let module = sanitize_ident(&args[0]);
                    let name_var = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_module_get_global({module}, {name_var})"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_get_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let module = sanitize_ident(&args[0]);
                    let name_var = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_module_get_name({module}, {name_var})"
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "module_get_attr" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
                    let attr = escape_luau_string(attr_str);
                    if let Some(module) = args.first() {
                        let module = sanitize_ident(module);
                        self.emit_line(&format!(
                            "local {out} = molt_module_get_name({module}, \"{attr}\")"
                        ));
                    } else {
                        self.emit_unsupported_op(op);
                    }
                } else if args.len() >= 2 {
                    let module = sanitize_ident(&args[0]);
                    let attr_var = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_module_get_name({module}, {attr_var})"
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "module_set_attr" => {
                // Store user-visible variables into the module cache so they can
                // be accessed by name later (e.g., `nums = [1,2,3]`).
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let module = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    // Only emit for non-dunder attributes (user variables).
                    // Dunder metadata writes are unnecessary in Luau.
                    self.emit_line(&format!(
                        "if type({module}) ~= \"table\" then error({{__type=\"TypeError\", __msg=\"module attribute assignment expects module\"}}) elseif molt_dict_is_ordered({module}) then molt_dict_set({module}, {attr_name}, {value}) else {module}[{attr_name}] = {value} end"
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "module_del_global" | "module_del_global_if_present" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let module = sanitize_ident(&args[0]);
                    let name = sanitize_ident(&args[1]);
                    let missing_ok = op.kind == "module_del_global_if_present";
                    if let Some(ref out_name) = op.out
                        && out_name != "none"
                    {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_module_del_global({module}, {name}, {missing_ok})"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "molt_module_del_global({module}, {name}, {missing_ok})"
                        ));
                    }
                }
            }

            // ================================================================
            // Alloc / memory (table stubs)
            // ================================================================
            "alloc_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class_var) = args.first() {
                    let class_ref = sanitize_ident(class_var);
                    self.emit_line(&format!("local {out} = setmetatable({{}}, {class_ref})"));
                } else if let Some(ref class_name) = op.s_value {
                    let class_ref = sanitize_ident(class_name);
                    self.emit_line(&format!("local {out} = setmetatable({{}}, {class_ref})"));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }
            "alloc" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }

            // ================================================================
            // Dataclass
            // ================================================================
            "dataclass_new" | "dataclass_new_values" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }
            "dataclass_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    if let Some(attr) = op.s_value.as_deref() {
                        self.emit_line(&format!("local {out} = {obj}.{}", sanitize_ident(attr)));
                    } else if args.len() >= 2 {
                        let idx = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = {obj}[{idx}]"));
                    }
                }
            }
            "dataclass_set" | "dataclass_set_class" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    if let Some(attr) = op.s_value.as_deref() {
                        let val = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{} = {val}", sanitize_ident(attr)));
                    }
                }
            }

            _ => return false,
        }
        true
    }
}
