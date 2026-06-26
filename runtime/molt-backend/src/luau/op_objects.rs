use super::*;

impl LuauBackend {
    pub(super) fn emit_object_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Guarded field access
            // ================================================================
            "guarded_field_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    if let Some(attr) = op.s_value.as_deref() {
                        let attr = sanitize_ident(attr);
                        self.emit_line(&format!("local {out} = {obj}.{attr}"));
                    } else if args.len() >= 2 {
                        let key = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = {obj}[{key}]"));
                    }
                }
            }
            "guarded_field_set" | "guarded_field_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    if let Some(attr) = op.s_value.as_deref() {
                        let attr = sanitize_ident(attr);
                        let val = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{attr} = {val}"));
                    } else if args.len() >= 3 {
                        let key = sanitize_ident(&args[1]);
                        let val = sanitize_ident(&args[2]);
                        self.emit_line(&format!("{obj}[{key}] = {val}"));
                    }
                }
            }

            // ================================================================
            // Function/class/module objects
            // ================================================================
            "func_new" | "func_new_closure" | "code_new" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let name = op
                        .s_value
                        .as_deref()
                        .map(sanitize_ident)
                        .unwrap_or_else(|| "nil".to_string());
                    self.emit_line(&format!("local {out} = {name}"));
                }
            }
            "builtin_func" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let s_val = op.s_value.as_deref().unwrap_or("");
                    // Map known Python builtins to Luau function references.
                    // The call_func IR op passes (args_tuple, kwargs, varkw),
                    // so we wrap Luau functions in a closure that unpacks the
                    // positional args tuple for the correct calling convention.
                    let mapped = match s_val {
                        "molt_max_builtin" => {
                            "function(a, ...) return math.max(table.unpack(a)) end"
                        }
                        "molt_min_builtin" => {
                            "function(a, ...) return math.min(table.unpack(a)) end"
                        }
                        "molt_abs_builtin" => "function(a, ...) return math.abs(a[1]) end",
                        "molt_round_builtin" => "function(a, ...) return math.round(a[1]) end",
                        "molt_print_builtin" => {
                            "function(a, ...) return molt_print(table.unpack(a)) end"
                        }
                        "molt_len" => "function(a, ...) return molt_len(a[1]) end",
                        "molt_int_builtin" | "molt_int" => {
                            "function(a, ...) return molt_int(a[1]) end"
                        }
                        "molt_float_builtin" | "molt_float" => {
                            "function(a, ...) return molt_float(a[1]) end"
                        }
                        "molt_str_builtin" | "molt_str" => {
                            "function(a, ...) return molt_str(a[1]) end"
                        }
                        "molt_bool_builtin" | "molt_bool" => {
                            "function(a, ...) return molt_bool(a[1]) end"
                        }
                        "molt_sum_builtin" => "function(a, ...) return molt_sum(a[1]) end",
                        "molt_any_builtin" => "function(a, ...) return molt_any(a[1]) end",
                        "molt_all_builtin" => "function(a, ...) return molt_all(a[1]) end",
                        "molt_sorted_builtin" => {
                            "function(a, ...) return molt_sorted(table.unpack(a)) end"
                        }
                        "molt_reversed_builtin" => {
                            "function(a, ...) return molt_reversed(a[1]) end"
                        }
                        "molt_enumerate_builtin" => {
                            "function(a, ...) return molt_enumerate(table.unpack(a)) end"
                        }
                        "molt_zip_builtin" => {
                            "function(a, ...) return molt_zip(table.unpack(a)) end"
                        }
                        "molt_isinstance" => {
                            "function(a, ...) return molt_isinstance(a[1], a[2]) end"
                        }
                        "molt_issubclass" => {
                            "function(a, ...) return molt_issubclass(a[1], a[2]) end"
                        }
                        "molt_classmethod_new" => {
                            "function(a, ...) return {__molt_descriptor_kind=\"classmethod\", __func=a[1]} end"
                        }
                        "molt_staticmethod_new" => {
                            "function(a, ...) return {__molt_descriptor_kind=\"staticmethod\", __func=a[1]} end"
                        }
                        "molt_property_new" => {
                            "function(a, ...) return {__molt_descriptor_kind=\"property\", __get=a[1], __set=a[2], __del=a[3]} end"
                        }
                        "molt_hash_builtin" => "function(a, ...) return molt_hash(a[1]) end",
                        "molt_ord" => "function(a, ...) return molt_ord(a[1]) end",
                        "molt_chr" => "function(a, ...) return string.char(a[1]) end",
                        "molt_repr_builtin" => "function(a, ...) return molt_repr(a[1]) end",
                        "molt_id" => "function(a, ...) return molt_id(a[1]) end",
                        "molt_callable_builtin" => {
                            "function(a, ...) return molt_callable(a[1]) end"
                        }
                        "molt_iter_checked" => "function(a, ...) return molt_iter(a[1]) end",
                        "molt_next_builtin" => {
                            "function(a, ...) return molt_next(table.unpack(a)) end"
                        }
                        "molt_getattr_builtin" => {
                            "function(a, ...) local value = molt_get_attr(a[1], a[2]); if value ~= nil then return value end; if a[3] ~= nil then return a[3] end; error({__type=\"AttributeError\", __msg=tostring(a[2])}) end"
                        }
                        "molt_set_attr_name" => {
                            "function(a, ...) return molt_set_attr(a[1], a[2], a[3]) end"
                        }
                        "molt_del_attr_name" => {
                            "function(a, ...) return molt_del_attr(a[1], a[2]) end"
                        }
                        "molt_has_attr_name" => {
                            "function(a, ...) return molt_has_attr(a[1], a[2]) end"
                        }
                        "molt_divmod_builtin" => {
                            "function(a, ...) return molt_divmod(a[1], a[2]) end"
                        }
                        "molt_hex_builtin" => "function(a, ...) return molt_hex(a[1]) end",
                        "molt_oct_builtin" => "function(a, ...) return molt_oct(a[1]) end",
                        "molt_bin_builtin" => "function(a, ...) return molt_bin(a[1]) end",
                        "molt_ascii_from_obj" => "function(a, ...) return molt_ascii(a[1]) end",
                        "molt_format_builtin" => {
                            "function(a, ...) return molt_format(table.unpack(a)) end"
                        }
                        "molt_dir_builtin" => "function(a, ...) return molt_dir(a[1]) end",
                        "molt_vars_builtin" => "function(a, ...) return molt_vars(a[1]) end",
                        // Runtime intrinsics that have no Luau equivalent.
                        "molt_function_init_metadata_packed"
                        | "molt_function_set_builtin"
                        | "molt_function_set_defaults"
                        | "molt_open_builtin"
                        | "molt_aiter"
                        | "molt_anext_builtin" => "nil",
                        _ => "nil",
                    };
                    self.emit_line(&format!("local {out} = {mapped}"));
                }
            }
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
            "bound_method_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    // IR contract: args[0] = function, args[1] = self.
                    let method = sanitize_ident(&args[0]);
                    let obj = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = function(...) local __m = {method}; if __m then return __m({obj}, ...) end; return nil end"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil -- bound_method missing args"));
                }
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
            "class_set_layout_version" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class = sanitize_ident(&args[0]);
                    let version = sanitize_ident(&args[1]);
                    self.emit_line("do");
                    self.push_indent();
                    self.emit_line(&format!("local __cls = {class}"));
                    self.emit_line(&format!("local __version = {version}"));
                    self.emit_line("if type(__version) ~= \"number\" or __version < 0 then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"layout version must be int\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__cls) == \"table\" then");
                    self.push_indent();
                    self.emit_line("__cls.__molt_layout_version = __version");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                }
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "class_merge_layout" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let class = sanitize_ident(&args[0]);
                    let offsets = sanitize_ident(&args[1]);
                    let size = sanitize_ident(&args[2]);
                    self.emit_line("do");
                    self.push_indent();
                    self.emit_line(&format!("local __cls = {class}"));
                    self.emit_line(&format!("local __offsets = {offsets}"));
                    self.emit_line(&format!("local __size = {size}"));
                    self.emit_line("if type(__cls) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"class layout merge expects type\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__size) ~= \"number\" or __size < 0 then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"__molt_layout_size__ must be int\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("local __merged = __cls.__molt_field_offsets__");
                    self.emit_line("if __offsets ~= nil then");
                    self.push_indent();
                    self.emit_line("if type(__offsets) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"__molt_field_offsets__ must be dict or None\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__merged) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line("__merged = {}");
                    self.emit_line("__cls.__molt_field_offsets__ = __merged");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("for __k, __v in pairs(__offsets) do");
                    self.push_indent();
                    self.emit_line(
                        "if type(__k) == \"string\" and type(__v) == \"number\" and __v >= 0 and __merged[__k] == nil then",
                    );
                    self.push_indent();
                    self.emit_line("__merged[__k] = __v");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("elseif __merged ~= nil and type(__merged) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"__molt_field_offsets__ must be dict or None\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("local __layout_size = __cls.__molt_layout_size__");
                    self.emit_line(
                        "if type(__layout_size) ~= \"number\" or __layout_size < 0 then",
                    );
                    self.push_indent();
                    self.emit_line("__layout_size = 0");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if __size > __layout_size then");
                    self.push_indent();
                    self.emit_line("__layout_size = __size");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__merged) == \"table\" then");
                    self.push_indent();
                    self.emit_line("for _, __offset in pairs(__merged) do");
                    self.push_indent();
                    self.emit_line("if type(__offset) == \"number\" and __offset >= 0 then");
                    self.push_indent();
                    self.emit_line("local __end = __offset + 16");
                    self.emit_line("if __end > __layout_size then");
                    self.push_indent();
                    self.emit_line("__layout_size = __end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if __layout_size == 0 then");
                    self.push_indent();
                    self.emit_line("__layout_size = 8");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("__cls.__molt_layout_size__ = __layout_size");
                    self.pop_indent();
                    self.emit_line("end");
                }
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
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
            "module_import" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    let module_name = op.s_value.as_deref().unwrap_or("");
                    let mapped = match module_name {
                        "math" => "molt_math",
                        "json" => "json",
                        "time" => "molt_time",
                        "os" => "molt_os",
                        "sys" => "molt_sys_ensure_module()",
                        _ => "",
                    };
                    if !mapped.is_empty() {
                        self.emit_line(&format!("local {out} = {mapped}"));
                    } else if let Some(name_var) = args.first() {
                        let nv = sanitize_ident(name_var);
                        self.emit_line(&format!("local {out} = molt_luau_import_module({nv})"));
                    } else {
                        self.emit_line(&format!("local {out} = nil"));
                    }
                }
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
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_cache_del" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(name_var) = args.first() {
                    let name = sanitize_ident(name_var);
                    self.emit_line(&format!("molt_module_cache[{name}] = nil"));
                }
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_import_star" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
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
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_get_attr" | "module_import_from" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
                    // Static attribute name — use dot access.
                    let attr = sanitize_ident(attr_str);
                    if let Some(module) = args.first() {
                        let module = sanitize_ident(module);
                        self.emit_line(&format!("local {out} = {module}.{attr}"));
                    }
                } else if args.len() >= 2 {
                    // module_get_attr: args[0] = module table, args[1] = attr name var.
                    // Look up attribute directly on the module.
                    let module = sanitize_ident(&args[0]);
                    let attr_var = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = if type({module}) == \"table\" then {module}[{attr_var}] else nil"
                    ));
                } else if let Some(module) = args.first() {
                    let module = sanitize_ident(module);
                    self.emit_line(&format!("local {out} = {module}"));
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
                        "if type({module}) == \"table\" then {module}[{attr_name}] = {value} end"
                    ));
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
            "alloc_class" | "alloc_class_trusted" | "alloc_class_static" => {
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
            "alloc" | "alloc_task" => {
                let out = self.out_var(op);
                // If this is a genexpr/listcomp task, create a coroutine-based
                // iterator that eagerly collects all yielded values into a list.
                let task_func = op.s_value.as_deref().unwrap_or("");
                if task_func.contains("genexpr") || task_func.contains("listcomp") {
                    // Create a list by running the generator to completion.
                    // The genexpr function uses state_yield to produce values
                    // as {value, false} tuples and returns {nil, true} when done.
                    let func_name = sanitize_ident(task_func);
                    self.emit_line(&format!(
                        "local {out} = (function()\n\
                         \t\tlocal __result = {{}}\n\
                         \t\tlocal __n = 0\n\
                         \t\tlocal __co = coroutine.wrap({func_name})\n\
                         \t\twhile true do\n\
                         \t\t\tlocal __item = __co()\n\
                         \t\t\tif __item == nil then break end\n\
                         \t\t\tif type(__item) == \"table\" then\n\
                         \t\t\t\tif __item[2] == true then break end\n\
                         \t\t\t\t__n += 1; __result[__n] = __item[1]\n\
                         \t\t\telse\n\
                         \t\t\t\t__n += 1; __result[__n] = __item\n\
                         \t\t\tend\n\
                         \t\tend\n\
                         \t\treturn __result\n\
                         \tend)()"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
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
