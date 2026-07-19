use super::*;

impl LuauBackend {
    /// Resolve an IR invocation target without allowing user-defined `molt_`
    /// symbols to alias compiler runtime helpers. Defined functions use the
    /// user-symbol encoder; unresolved reserved names are compiler protocol.
    pub(super) fn invocation_target_ident(&self, raw_name: &str) -> String {
        if self.function_symbols.contains(raw_name) {
            emit_function_ident(raw_name)
        } else if raw_name.starts_with("molt_") {
            raw_name.to_string()
        } else {
            sanitize_ident(raw_name)
        }
    }

    pub(super) fn out_var(&self, op: &OpIR) -> String {
        op.out
            .as_deref()
            .map(sanitize_ident)
            .unwrap_or_else(|| "_".to_string())
    }

    pub(super) fn var_ref(&self, op: &OpIR) -> String {
        op.var
            .as_deref()
            .map(sanitize_ident)
            .unwrap_or_else(|| "_".to_string())
    }

    pub(super) fn numeric_operand_expr(&self, raw_name: &str) -> String {
        let ident = sanitize_ident(raw_name);
        if self.scalar_plan.name_scalar_kind(raw_name) == Some(ScalarKind::Bool) {
            format!("(if {ident} then 1 else 0)")
        } else {
            ident
        }
    }

    pub(super) fn plan_knows_string(&self, raw_name: &str) -> bool {
        self.scalar_plan.name_scalar_kind(raw_name) == Some(ScalarKind::Str)
            || self.scalar_plan.name_container_kind(raw_name) == Some(ContainerKind::Str)
    }

    pub(super) fn plan_knows_list(&self, raw_name: &str) -> bool {
        self.scalar_plan.name_container_kind(raw_name) == Some(ContainerKind::List)
    }

    pub(super) fn emit_index_bounds_guard(&mut self, idx: &str, container: &str, message: &str) {
        self.emit_line(&format!(
            "if {idx} < 1 or {idx} > #{container} then error({{__type=\"IndexError\", __msg=\"{message}\"}}) end"
        ));
    }

    pub(super) fn emit_list_insert(&mut self, list: &str, idx: &str, val: &str) {
        self.emit_line(&format!(
            "do if type(rawget({list}, molt_sequence_length_key)) ~= \"number\" then local __idx = if {idx} >= 0 then {idx} + 1 else #{list} + {idx} + 1; if __idx < 1 then __idx = 1 end; if __idx > #{list} + 1 then __idx = #{list} + 1 end; if __idx == #{list} + 1 then {list}[#{list} + 1] = {val} else table.insert({list}, __idx, {val}) end else local __n = molt_sequence_len({list}); local __idx = if {idx} >= 0 then {idx} + 1 else __n + {idx} + 1; if __idx < 1 then __idx = 1 end; if __idx > __n + 1 then __idx = __n + 1 end; for __i = __n, __idx, -1 do rawset({list}, __i + 1, rawget({list}, __i)) end; rawset({list}, __idx, {val}); rawset({list}, molt_sequence_length_key, __n + 1) end end"
        ));
    }

    pub(super) fn emit_list_pop(&mut self, list: &str, idx: Option<&str>, out: Option<&str>) {
        match (idx, out) {
            (Some(idx), Some(out)) => self.emit_line(&format!(
                "local {out}; do local __n = molt_sequence_len({list}); local __idx = if {idx} >= 0 then {idx} + 1 else __n + {idx} + 1; if __idx < 1 or __idx > __n then error({{__type=\"IndexError\", __msg=\"pop index out of range\"}}) end; {out} = rawget({list}, __idx); for __i = __idx, __n - 1 do rawset({list}, __i, rawget({list}, __i + 1)) end; rawset({list}, __n, nil); if type(rawget({list}, molt_sequence_length_key)) == \"number\" then rawset({list}, molt_sequence_length_key, __n - 1) end end"
            )),
            (Some(idx), None) => self.emit_line(&format!(
                "do local __n = molt_sequence_len({list}); local __idx = if {idx} >= 0 then {idx} + 1 else __n + {idx} + 1; if __idx < 1 or __idx > __n then error({{__type=\"IndexError\", __msg=\"pop index out of range\"}}) end; for __i = __idx, __n - 1 do rawset({list}, __i, rawget({list}, __i + 1)) end; rawset({list}, __n, nil); if type(rawget({list}, molt_sequence_length_key)) == \"number\" then rawset({list}, molt_sequence_length_key, __n - 1) end end"
            )),
            (None, Some(out)) => self.emit_line(&format!(
                "local {out}; do local __n = molt_sequence_len({list}); if __n == 0 then error({{__type=\"IndexError\", __msg=\"pop from empty list\"}}) end; {out} = rawget({list}, __n); rawset({list}, __n, nil); if type(rawget({list}, molt_sequence_length_key)) == \"number\" then rawset({list}, molt_sequence_length_key, __n - 1) end end"
            )),
            (None, None) => self.emit_line(&format!(
                "do local __n = molt_sequence_len({list}); if __n == 0 then error({{__type=\"IndexError\", __msg=\"pop from empty list\"}}) end; rawset({list}, __n, nil); if type(rawget({list}, molt_sequence_length_key)) == \"number\" then rawset({list}, molt_sequence_length_key, __n - 1) end end"
            )),
        }
    }

    pub(super) fn emit_string_predicate_attr(&mut self, out: &str, obj: &str, method: &str) {
        let predicate = match method {
            "isalpha" => "__is_alpha and not __is_digit",
            "isdigit" => "__is_digit",
            "isalnum" => "__is_alpha or __is_digit",
            "isspace" => "__is_space",
            "isupper" => "not __is_lower",
            "islower" => "not __is_upper",
            "isidentifier" => "(__is_alpha or __is_digit or __b == 95)",
            "isprintable" => "(__b >= 32 and __b <= 126)",
            "isdecimal" | "isnumeric" => "__is_digit",
            "istitle" => "true",
            _ => "false",
        };
        let prefix = match method {
            "isidentifier" => {
                "local __first = string.byte(__s, 1); local __first_ok = ((__first >= 65 and __first <= 90) or (__first >= 97 and __first <= 122) or __first == 95);"
            }
            "istitle" => "local __prev_uncased = true;",
            _ => "",
        };
        let suffix = match method {
            "isupper" | "islower" => " and __has_cased",
            "isidentifier" => " and __first_ok",
            "istitle" => " and __has_cased",
            _ => "",
        };
        let title_update = if method == "istitle" {
            " if __is_alpha then if __prev_uncased then if not __is_upper then __ok = false; break end else if not __is_lower then __ok = false; break end end; __prev_uncased = false else __prev_uncased = true end"
        } else {
            ""
        };
        self.emit_line(&format!(
            "local {out} = function(__args) local __s = {obj}; local __ok = (#__s > 0); local __has_cased = false; {prefix} for __i = 1, #__s do local __b = string.byte(__s, __i); local __is_upper = (__b >= 65 and __b <= 90); local __is_lower = (__b >= 97 and __b <= 122); local __is_alpha = (__is_upper or __is_lower); local __is_digit = (__b >= 48 and __b <= 57); local __is_space = (__b == 32 or __b == 9 or __b == 10 or __b == 11 or __b == 12 or __b == 13); if __is_alpha then __has_cased = true end; if not ({predicate}) then __ok = false; break end{title_update} end; return __ok{suffix} end"
        ));
    }

    /// Wrap a condition identifier in `molt_bool()` if it's not a known boolean.
    /// Returns the identifier as-is for booleans, or `molt_bool(ident)` otherwise.
    pub(super) fn guard_truthiness(&self, raw_name: &str) -> String {
        let ident = sanitize_ident(raw_name);
        match self.scalar_plan.name_scalar_kind(raw_name) {
            Some(ScalarKind::Bool) => ident,
            // Strength-reduce: type-specific truthiness checks avoid
            // the multi-branch molt_bool() function call overhead.
            Some(ScalarKind::Int | ScalarKind::Float) => format!("({ident} ~= 0)"),
            Some(ScalarKind::Str) => format!("({ident} ~= \"\")"),
            Some(ScalarKind::NoneValue) => "false".to_string(),
            None => self
                .container_truthiness(raw_name, &ident)
                .unwrap_or_else(|| match ident.as_str() {
                    "true" | "false" => ident,
                    _ => format!("molt_bool({ident})"),
                }),
        }
    }

    pub(super) fn container_truthiness(&self, raw_name: &str, ident: &str) -> Option<String> {
        match self.scalar_plan.name_container_kind(raw_name) {
            Some(ContainerKind::List | ContainerKind::Tuple) => {
                Some(format!("(molt_sequence_len({ident}) > 0)"))
            }
            Some(ContainerKind::Str) => Some(format!("(#{ident} > 0)")),
            Some(ContainerKind::Dict) => Some(format!("(molt_dict_len({ident}) > 0)")),
            Some(ContainerKind::Set) => Some(format!("(molt_set_len({ident}) > 0)")),
            None => None,
        }
    }

    pub(super) fn is_known_bool_value(&self, raw_name: &str) -> bool {
        matches!(raw_name, "true" | "false")
            || self.scalar_plan.name_scalar_kind(raw_name) == Some(ScalarKind::Bool)
    }

    /// Emit the Python scalar-identity subset without collapsing source kinds
    /// into Luau's shared numeric carrier. Producer facts make cross-kind
    /// identity a compile-time constant. Admitted dynamic cases use `rawequal`
    /// so reference identity never invokes a user-defined `__eq` metamethod.
    pub(super) fn identity_comparison_expr(
        &self,
        lhs_raw: &str,
        rhs_raw: &str,
        negated: bool,
    ) -> String {
        let lhs = sanitize_ident(lhs_raw);
        let rhs = sanitize_ident(rhs_raw);
        match identity_lowering(&self.scalar_plan, lhs_raw, rhs_raw) {
            IdentityLowering::Constant(identity) => (identity != negated).to_string(),
            IdentityLowering::Direct => {
                let raw_identity = format!("molt_rawequal({lhs}, {rhs})");
                if negated {
                    format!("(not {raw_identity})")
                } else {
                    raw_identity
                }
            }
            IdentityLowering::Reject => {
                unreachable!("compile_checked validates identity provenance before emission")
            }
        }
    }

    pub(super) fn emit_line(&mut self, line: &str) {
        for _ in 0..self.indent {
            self.output.push('\t');
        }
        self.output.push_str(line);
        self.output.push('\n');
    }

    pub(super) fn push_indent(&mut self) {
        self.indent += 1;
    }

    pub(super) fn pop_indent(&mut self) {
        if self.indent > 0 {
            self.indent -= 1;
        }
    }
}
