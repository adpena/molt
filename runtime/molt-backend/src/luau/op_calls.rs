use super::*;
use std::collections::HashSet;

impl LuauBackend {
    pub(super) fn emit_call_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Call argument builders
            // ================================================================
            "callargs_new" => {
                let out = self.out_var(op);
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out}: {{any}} = {{{items}}}"));
            }
            "callargs_push_pos" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let callargs = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("rawset({callargs}, #{callargs} + 1, {value})"));
                }
            }
            "callargs_expand_star" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let callargs = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "table.move({other}, 1, #{other}, #{callargs} + 1, {callargs})"
                    ));
                }
            }
            "callargs_push_kw" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let callargs = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{callargs}[{key}] = {value}"));
                }
            }
            "callargs_expand_kwstar" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let callargs = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k, __v in pairs({other}) do {callargs}[__k] = __v end"
                    ));
                }
            }

            // ================================================================
            // Callable objects and builtin wrappers
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
                            "function(a, ...) return type(a[1]) == \"function\" end"
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
            // ================================================================
            // Function calls
            // ================================================================
            "call" | "call_guarded" => {
                let out = self.out_var(op);
                // First check for s_value function name (pedagogical IR form).
                if let Some(ref func_name) = op.s_value {
                    let func_name = sanitize_ident(func_name);
                    let call_args = op
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    // Map Python builtins to Luau equivalents.
                    let mapped = match func_name.as_str() {
                        "len" | "molt_len" => format!("molt_len({call_args})"),
                        "int" | "molt_int" => format!("molt_int({call_args})"),
                        "float" | "molt_float" => format!("molt_float({call_args})"),
                        "str" | "molt_str" => format!("molt_str({call_args})"),
                        "bool" | "molt_bool" => format!("molt_bool({call_args})"),
                        "range" | "molt_range" => format!("molt_range({call_args})"),
                        "abs" => format!("math.abs({call_args})"),
                        "min" => format!("math.min({call_args})"),
                        "max" => format!("math.max({call_args})"),
                        "round" => format!("math.round({call_args})"),
                        "enumerate" | "molt_enumerate" => format!("molt_enumerate({call_args})"),
                        "zip" | "molt_zip" => format!("molt_zip({call_args})"),
                        "sorted" | "molt_sorted" => format!("molt_sorted({call_args})"),
                        "reversed" | "molt_reversed" => format!("molt_reversed({call_args})"),
                        "sum" | "molt_sum" => format!("molt_sum({call_args})"),
                        "any" | "molt_any" => format!("molt_any({call_args})"),
                        "all" | "molt_all" => format!("molt_all({call_args})"),
                        "map" | "molt_map" => format!("molt_map({call_args})"),
                        "filter" | "molt_filter" => format!("molt_filter({call_args})"),
                        "print" => format!("molt_print({call_args})"),
                        _ => format!("{func_name}({call_args})"),
                    };
                    self.emit_line(&format!("local {out} = {mapped}"));
                } else {
                    // Real IR form: args[0] is the callable, rest are arguments.
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if !args.is_empty() {
                        let func_ref = sanitize_ident(&args[0]);
                        let call_args = args[1..]
                            .iter()
                            .map(|a| sanitize_ident(a))
                            .collect::<Vec<_>>()
                            .join(", ");
                        if op.out.is_some() {
                            self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                        } else {
                            self.emit_line(&format!("{func_ref}({call_args})"));
                        }
                    }
                }
            }
            "call_internal" => {
                if let Some(ref s_val) = op.s_value {
                    let func_name = sanitize_ident(s_val);
                    let call_args = op
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {func_name}({call_args})"));
                    } else {
                        self.emit_line(&format!("{func_name}({call_args})"));
                    }
                }
            }
            "call_func" | "call_function" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = if {func_ref} then {func_ref}({call_args}) else nil"
                        ));
                    } else {
                        self.emit_line(&format!("if {func_ref} then {func_ref}({call_args}) end"));
                    }
                }
            }
            "call_method" => {
                if self.emit_typed_list_method_call(op) {
                    return true;
                }
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let obj = sanitize_ident(&args[0]);
                    let method_name = op.s_value.as_deref().unwrap_or("unknown");
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        let escaped = escape_luau_string(method_name);
                        self.emit_line(&format!(
                            "local {out}; do local __method = molt_get_attr({obj}, \"{escaped}\"); {out} = if __method then __method({call_args}) else nil end end"
                        ));
                    } else {
                        let escaped = escape_luau_string(method_name);
                        self.emit_line(&format!(
                            "do local __method = molt_get_attr({obj}, \"{escaped}\"); if __method then __method({call_args}) end end"
                        ));
                    }
                }
            }
            "call_async" => {
                // Luau has no Molt scheduler object model here, but CALL_ASYNC
                // already carries the concrete poll target in s_value. Execute
                // that target directly for the admitted synchronous subset.
                let args = op.args.as_deref().unwrap_or(&[]);
                let func_ref = sanitize_ident(
                    op.s_value
                        .as_deref()
                        .expect("call_async expects poll target in s_value"),
                );
                let call_args = args
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                } else {
                    self.emit_line(&format!("{func_ref}({call_args})"));
                }
            }
            "block_on" | "spawn" => {
                // Scheduler ownership is not represented in checked Luau yet.
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = {func_ref}({call_args}) -- [async: {}]",
                            op.kind
                        ));
                    } else {
                        self.emit_line(&format!("{func_ref}({call_args}) -- [async: {}]", op.kind));
                    }
                } else if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: {}]", op.kind));
                }
            }

            // ================================================================
            // Indirect / bound calls
            // ================================================================
            "call_indirect" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                    } else {
                        self.emit_line(&format!("{func_ref}({call_args})"));
                    }
                }
            }
            "call_bind" => {
                // In Molt IR, call_bind is a function CALL whose second arg is
                // always a callargs tuple (built via callargs_new + callargs_push_pos).
                // We must unpack the tuple so individual args are spread:
                //   func(table.unpack(args_tuple))  instead of  func(args_tuple)
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let func = sanitize_ident(&args[0]);
                    let args_tuple = sanitize_ident(&args[1]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = if {func} then {func}(table.unpack({args_tuple})) else nil"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {func} then {func}(table.unpack({args_tuple})) end"
                        ));
                    }
                } else if let Some(func) = args.first() {
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(func)));
                }
            }
            "is_callable" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = ({})", luau_callable_expr(val)));
                }
            }

            _ => return false,
        }
        true
    }

    pub(super) fn collect_invocation_forward_decls(
        &self,
        emit_funcs: &[&FunctionIR],
    ) -> Vec<String> {
        let defined_names: HashSet<String> =
            emit_funcs.iter().map(|f| sanitize_ident(&f.name)).collect();
        let mut extra_decls_set = HashSet::new();
        let mut extra_forward_decls = Vec::new();
        for func in emit_funcs {
            for op in &func.ops {
                let mut check_ident = |raw: &str| {
                    let ident = sanitize_ident(raw);
                    if should_forward_declare_invocation_target(&ident, &defined_names)
                        && !extra_decls_set.contains(&ident)
                    {
                        extra_decls_set.insert(ident.clone());
                        extra_forward_decls.push(ident);
                    }
                };
                match op.kind.as_str() {
                    "call_internal" | "func_new" | "func_new_closure" | "code_new" => {
                        if let Some(ref s_val) = op.s_value {
                            check_ident(s_val);
                        }
                    }
                    "call" | "call_guarded" => {
                        if let Some(ref s_val) = op.s_value {
                            check_ident(s_val);
                        } else if let Some(ref args) = op.args
                            && let Some(callee) = args.first()
                        {
                            check_ident(callee);
                        }
                    }
                    "call_async" => {
                        if let Some(ref s_val) = op.s_value {
                            check_ident(s_val);
                        }
                    }
                    "load_local" => {
                        if let Some(ref var) = op.var {
                            check_ident(var);
                        }
                    }
                    "call_func" | "call_function" | "call_indirect" | "call_bind" | "block_on"
                    | "spawn" => {
                        if let Some(ref args) = op.args
                            && let Some(callee) = args.first()
                        {
                            check_ident(callee);
                        }
                    }
                    _ => {}
                }
            }
        }
        extra_forward_decls
    }
}

fn luau_callable_expr(raw: &str) -> String {
    format!("type({}) == \"function\"", sanitize_ident(raw))
}

fn should_forward_declare_invocation_target(ident: &str, defined_names: &HashSet<String>) -> bool {
    let is_temp_var = ident.starts_with('v') && ident[1..].chars().all(|c| c.is_ascii_digit());
    !is_temp_var
        && !defined_names.contains(ident)
        && !ident.starts_with("__")
        && !ident.starts_with("molt_")
        && !matches!(
            ident,
            "assert"
                | "collectgarbage"
                | "error"
                | "getmetatable"
                | "ipairs"
                | "next"
                | "pairs"
                | "pcall"
                | "print"
                | "rawequal"
                | "rawget"
                | "rawlen"
                | "rawset"
                | "select"
                | "setmetatable"
                | "tonumber"
                | "tostring"
                | "type"
                | "xpcall"
                | "abs"
                | "all"
                | "any"
                | "bool"
                | "enumerate"
                | "filter"
                | "float"
                | "int"
                | "len"
                | "map"
                | "max"
                | "min"
                | "range"
                | "reversed"
                | "round"
                | "sorted"
                | "str"
                | "sum"
                | "zip"
        )
}
