use super::*;

impl LuauBackend {
    pub(super) fn emit_container_access_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
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
                        // Unknown container: string->find, table->check both
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
                            // rawget bypasses metamethods: safe for plain list
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
                        // rawset bypasses metamethods: safe for plain list tables.
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
