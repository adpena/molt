use super::*;

impl LuauBackend {
    pub(super) fn emit_binary_op(&mut self, op: &OpIR, operator: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let arithmetic = matches!(operator, "+" | "-" | "*" | "/" | "%" | "//" | "^");
            let lhs = if arithmetic {
                self.numeric_operand_expr(&args[0])
            } else {
                sanitize_ident(&args[0])
            };
            let rhs = if arithmetic {
                self.numeric_operand_expr(&args[1])
            } else {
                sanitize_ident(&args[1])
            };
            // Parenthesize comparison/boolean results to prevent precedence
            // issues when the sink pass inlines into `not` expressions.
            // Without parens: `not a == b` → `(not a) == b` (wrong).
            // With parens: `not (a == b)` (correct).
            let is_cmp = matches!(operator, "==" | "~=" | "<" | "<=" | ">" | ">=");
            let is_logical = matches!(operator, "and" | "or");
            // Type annotation: arithmetic → number, comparisons → boolean.
            let ty_ann = if arithmetic {
                ": number"
            } else if is_cmp {
                ": boolean"
            } else {
                ""
            };
            if is_cmp || is_logical {
                self.emit_line(&format!("local {out}{ty_ann} = ({lhs} {operator} {rhs})"));
            } else {
                self.emit_line(&format!("local {out}{ty_ann} = {lhs} {operator} {rhs}"));
            }
        }
    }

    // --- helper: bit32 op ---
    pub(super) fn emit_bit_op(&mut self, op: &OpIR, func: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out}: number = bit32.{func}({lhs}, {rhs})"));
        }
    }

    // --- helpers ---

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
            "do local __idx = if {idx} >= 0 then {idx} + 1 else #{list} + {idx} + 1; if __idx < 1 then __idx = 1 end; if __idx > #{list} + 1 then __idx = #{list} + 1 end; if __idx == #{list} + 1 then {list}[#{list} + 1] = {val} else table.insert({list}, __idx, {val}) end end"
        ));
    }

    pub(super) fn emit_list_pop(&mut self, list: &str, idx: Option<&str>, out: Option<&str>) {
        match (idx, out) {
            (Some(idx), Some(out)) => self.emit_line(&format!(
                "local {out}; do local __idx = if {idx} >= 0 then {idx} + 1 else #{list} + {idx} + 1; if __idx < 1 or __idx > #{list} then error({{__type=\"IndexError\", __msg=\"pop index out of range\"}}) end; {out} = table.remove({list}, __idx) end"
            )),
            (Some(idx), None) => self.emit_line(&format!(
                "do local __idx = if {idx} >= 0 then {idx} + 1 else #{list} + {idx} + 1; if __idx < 1 or __idx > #{list} then error({{__type=\"IndexError\", __msg=\"pop index out of range\"}}) end; table.remove({list}, __idx) end"
            )),
            (None, Some(out)) => self.emit_line(&format!(
                "local {out}; if #{list} == 0 then error({{__type=\"IndexError\", __msg=\"pop from empty list\"}}) end; {out} = table.remove({list})"
            )),
            (None, None) => self.emit_line(&format!(
                "if #{list} == 0 then error({{__type=\"IndexError\", __msg=\"pop from empty list\"}}) end; table.remove({list})"
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
            Some(ContainerKind::List | ContainerKind::Tuple | ContainerKind::Str) => {
                Some(format!("(#{ident} > 0)"))
            }
            Some(ContainerKind::Dict | ContainerKind::Set) => {
                Some(format!("(next({ident}) ~= nil)"))
            }
            None => None,
        }
    }

    pub(super) fn is_known_bool_value(&self, raw_name: &str) -> bool {
        matches!(raw_name, "true" | "false")
            || self.scalar_plan.name_scalar_kind(raw_name) == Some(ScalarKind::Bool)
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
