use super::*;

impl LuauBackend {
    pub(super) fn emit_collection_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Collection construction
            // ================================================================
            "tuple_new" | "tuple_from_list" => {
                let out = self.out_var(op);
                // Track this variable so return sites can unpack it.
                if let Some(ref out_name) = op.out {
                    self.tuple_vars.insert(out_name.clone());
                }
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out} = {{{items}}}"));
            }
            "unpack_sequence" => {
                // Destructure a tuple/list into individual variables.
                // args[0] = source container, args[1..] = output variable names.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let src = sanitize_ident(&args[0]);
                    for (i, out_name) in args[1..].iter().enumerate() {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {src}[{}]", i + 1));
                    }
                }
            }
            "build_dict" | "dict_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out}: {{[any]: any}} = {{}}"));
                } else {
                    // args are key-value pairs: [k1, v1, k2, v2, ...]
                    let mut entries = Vec::new();
                    for pair in args.chunks(2) {
                        if pair.len() == 2 {
                            let key = sanitize_ident(&pair[0]);
                            let val = sanitize_ident(&pair[1]);
                            entries.push(format!("[{key}] = {val}"));
                        }
                    }
                    let body = entries.join(", ");
                    self.emit_line(&format!("local {out}: {{[any]: any}} = {{{body}}}"));
                }
            }
            "set_new" | "frozenset_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out} = {{}}"));
                } else {
                    // Sets are tables with value→true entries for O(1) lookup.
                    let entries = args
                        .iter()
                        .map(|a| format!("[{}] = true", sanitize_ident(a)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.emit_line(&format!("local {out} = {{{entries}}}"));
                }
            }
            // ================================================================
            // Dict operations
            // ================================================================
            "dict_clear" | "set_clear" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    let tbl = sanitize_ident(tbl);
                    self.emit_line(&format!("table.clear({tbl})"));
                }
            }
            "dict_copy" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!("local {out} = table.clone({src})"));
                }
            }
            "dict_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        // dict.get(key, default) — return default when missing.
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = if {dict}[{key}] ~= nil then {dict}[{key}] else {default}"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = {dict}[{key}]"));
                    }
                }
            }
            "dict_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = {val}"));
                }
            }
            "dict_setdefault" => {
                // Python dict.setdefault(k, v) only sets if key is absent.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then {dict}[{key}] = {val} end; local {out} = {dict}[{key}]"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then {dict}[{key}] = {val} end"
                        ));
                    }
                }
            }
            "dict_setdefault_empty_list" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if {dict}[{key}] == nil then {dict}[{key}] = {{}} end"
                    ));
                }
            }
            "dict_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        // dict.pop(key, default) — return default if missing.
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = if {dict}[{key}] ~= nil then {dict}[{key}] else {default}"
                        ));
                    } else {
                        // dict.pop(key) — raise KeyError if missing.
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then error(\"KeyError: \" .. tostring({key})) end"
                        ));
                        self.emit_line(&format!("local {out} = {dict}[{key}]"));
                    }
                    self.emit_line(&format!("{dict}[{key}] = nil"));
                }
            }
            "dict_update" | "dict_update_missing" | "dict_update_kwstar" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k, __v in pairs({other}) do {dict}[__k] = __v end"
                    ));
                }
            }
            "dict_popitem" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(dict) = args.first() {
                    let dict = sanitize_ident(dict);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k, __v in pairs({dict}) do {out} = {{__k, __v}}; {dict}[__k] = nil; break end; if {out} == nil then error({{__type=\"KeyError\", __msg=\"popitem(): dictionary is empty\"}}) end"
                    ));
                }
            }
            "dict_inc" | "dict_str_int_inc" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let inc = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = ({dict}[{key}] or 0) + {inc}"));
                }
            }
            "dict_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!(
                        "local {out} = {{}}; for __k, __v in pairs({src}) do {out}[__k] = __v end"
                    ));
                }
            }

            // ================================================================
            // Set operations
            // ================================================================
            // `set_add_probe` is `set_add` for the temporary set realized when
            // probing the operand of intersection/issubset; the Luau lane uses
            // Lua-table keys (any value hashable) so it is identical to set_add.
            "set_add" | "set_add_probe" | "frozenset_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = true"));
                }
            }
            "set_discard" => {
                // discard is silent if element is absent.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = nil"));
                }
            }
            "set_remove" => {
                // remove raises KeyError if element is absent.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if {set}[{val}] == nil then error(\"KeyError: \" .. tostring({val})) end; {set}[{val}] = nil"
                    ));
                }
            }
            "set_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(set) = args.first() {
                    let set = sanitize_ident(set);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k in pairs({set}) do {out} = __k; {set}[__k] = nil; break end; if {out} == nil then error(\"KeyError: pop from an empty set\") end"
                    ));
                }
            }
            "set_update" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k in pairs({other}) do {set}[__k] = true end"
                    ));
                }
            }
            "contains" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    let container_kind = self.scalar_plan.name_container_kind(&args[0]);
                    let is_dict = matches!(
                        container_kind,
                        Some(ContainerKind::Dict | ContainerKind::Set)
                    );
                    let is_list = matches!(container_kind, Some(ContainerKind::List));
                    if is_dict {
                        // Dict/set: key lookup.
                        self.emit_line(&format!("local {out} = ({container}[{val}] ~= nil)"));
                    } else if is_list {
                        // List: value search via table.find.
                        self.emit_line(&format!(
                            "local {out} = (table.find({container}, {val}) ~= nil)"
                        ));
                    } else {
                        // Unknown container: string→find, table→check both
                        // array values AND hash keys for correctness.
                        self.emit_line(&format!(
                            "local {out} = if type({container}) == \"string\" then \
                             (string.find({container}, {val}, 1, true) ~= nil) \
                             elseif type({container}) == \"table\" then \
                             (table.find({container}, {val}) ~= nil or {container}[{val}] ~= nil) \
                             else false"
                        ));
                    }
                }
            }

            // ================================================================
            // Indexing / subscript
            // ================================================================
            "get_item" | "subscript" | "index" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);

                    let container_kind = self.scalar_plan.name_container_kind(&args[0]);
                    let container_is_str = container_kind == Some(ContainerKind::Str);

                    // Fast-path: when the key is a known non-negative constant,
                    // skip the negative-index ternary entirely.
                    let key_is_scalar_int = self.scalar_plan.name_is_integer_family(&args[1]);
                    let key_known_nonneg = self.nonneg_consts.contains(&args[1])
                        || (key_is_scalar_int && op.value.is_some_and(|v| v >= 0));

                    if container_is_str {
                        // Luau does not support string[index]; use string.sub.
                        // Python uses 0-based indexing, Luau uses 1-based.
                        let idx_var = format!("__idx_{out}");
                        if key_known_nonneg {
                            self.emit_line(&format!("local {idx_var} = {key} + 1"));
                        } else {
                            // Handle negative indexing for strings too.
                            self.emit_line(&format!(
                                "local {idx_var} = if {key} >= 0 then {key} + 1 else #{container} + {key} + 1"
                            ));
                        }
                        self.emit_index_bounds_guard(
                            &idx_var,
                            &container,
                            "string index out of range",
                        );
                        let byte_idx_var = format!("__byte_idx_{out}");
                        let next_byte_idx_var = format!("__next_byte_idx_{out}");
                        self.emit_line(&format!(
                            "local {byte_idx_var}: number = molt_str_byte_offset({container}, {idx_var})"
                        ));
                        self.emit_line(&format!(
                            "local {next_byte_idx_var} = utf8.offset({container}, {idx_var} + 1)"
                        ));
                        self.emit_line(&format!(
                            "local {out} = string.sub({container}, {byte_idx_var}, if {next_byte_idx_var} == nil then #{container} else {next_byte_idx_var} - 1)"
                        ));
                    } else {
                        // If the container is a known list, the key is
                        // integer-indexed. Nested-list output identity must come
                        // from `ScalarRepresentationPlan`, not copied transport
                        // hints.
                        let container_is_list = matches!(container_kind, Some(ContainerKind::List));
                        let key_is_int = key_is_scalar_int || container_is_list;
                        if container_is_list {
                            let idx_var = format!("__idx_{out}");
                            if key_known_nonneg {
                                self.emit_line(&format!("local {idx_var}: number = {key} + 1"));
                            } else {
                                self.emit_line(&format!(
                                    "local {idx_var}: number = if {key} >= 0 then {key} + 1 else #{container} + {key} + 1"
                                ));
                            }
                            self.emit_index_bounds_guard(
                                &idx_var,
                                &container,
                                "list index out of range",
                            );
                            // rawget bypasses metamethods — safe for plain list
                            // tables and faster in Luau's native codegen path.
                            self.emit_line(&format!(
                                "local {out} = rawget({container}, {idx_var})"
                            ));
                        } else if key_known_nonneg {
                            // Known non-negative: skip negative index ternary.
                            self.emit_line(&format!("local {out} = {container}[{key} + 1]"));
                        } else if key_is_int {
                            // Handle negative indexing: Python a[-1] = last element.
                            self.emit_line(&format!(
                                "local {out} = {container}[if {key} >= 0 then {key} + 1 else #{container} + {key} + 1]"
                            ));
                        } else {
                            self.emit_line(&format!(
                                "local {out} = {container}[if type({key}) == \"number\" then (if {key} >= 0 then {key} + 1 else #{container} + {key} + 1) else {key}]"
                            ));
                        }
                    }
                }
            }
            "set_item" | "store_subscript" | "store_index" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    let key_is_int = self.scalar_plan.name_is_integer_family(&args[1]);
                    let key_known_nonneg = self.nonneg_consts.contains(&args[1])
                        || (key_is_int && op.value.is_some_and(|v| v >= 0));
                    let known_list_like = matches!(
                        self.scalar_plan.name_container_kind(&args[0]),
                        Some(ContainerKind::List)
                    );
                    if known_list_like {
                        let idx_expr = if key_known_nonneg {
                            format!("{key} + 1")
                        } else {
                            format!("if {key} >= 0 then {key} + 1 else #{container} + {key} + 1")
                        };
                        // rawset bypasses metamethods — safe for plain list tables.
                        self.emit_line(&format!(
                            "do local __idx: number = {idx_expr}; if __idx < 1 or __idx > #{container} then error({{__type=\"IndexError\", __msg=\"list assignment index out of range\"}}) end; rawset({container}, __idx, {value}) end"
                        ));
                    } else if key_known_nonneg {
                        self.emit_line(&format!("{container}[{key} + 1] = {value}"));
                    } else if key_is_int {
                        self.emit_line(&format!(
                            "{container}[if {key} >= 0 then {key} + 1 else #{container} + {key} + 1] = {value}"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "{container}[if type({key}) == \"number\" then (if {key} >= 0 then {key} + 1 else #{container} + {key} + 1) else {key}] = {value}"
                        ));
                    }
                }
            }
            "del_index" | "del_item" => {
                // Python del lst[i] removes the element and shifts remaining.
                // Setting to nil creates a hole that breaks # and ipairs.
                // For integer keys (list deletion), use table.remove with +1 offset.
                // For string keys (dict deletion), nil assignment is correct.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let key_is_int = self.scalar_plan.name_is_integer_family(&args[1]);
                    let key_known_nonneg = self.nonneg_consts.contains(&args[1])
                        || (key_is_int && op.value.is_some_and(|v| v >= 0));
                    let known_list_like = matches!(
                        self.scalar_plan.name_container_kind(&args[0]),
                        Some(ContainerKind::List)
                    );
                    if known_list_like {
                        let idx_expr = if key_known_nonneg {
                            format!("{key} + 1")
                        } else {
                            format!("if {key} >= 0 then {key} + 1 else #{container} + {key} + 1")
                        };
                        self.emit_line(&format!(
                            "do local __idx = {idx_expr}; if __idx < 1 or __idx > #{container} then error({{__type=\"IndexError\", __msg=\"list deletion index out of range\"}}) end; table.remove({container}, __idx) end"
                        ));
                    } else if key_known_nonneg {
                        self.emit_line(&format!("table.remove({container}, {key} + 1)"));
                    } else if key_is_int {
                        self.emit_line(&format!(
                            "table.remove({container}, if {key} >= 0 then {key} + 1 else #{container} + {key} + 1)"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if type({key}) == \"number\" then table.remove({container}, if {key} >= 0 then {key} + 1 else #{container} + {key} + 1) else {container}[{key}] = nil end"
                        ));
                    }
                }
            }

            _ => return false,
        }
        true
    }
}
