use super::*;

impl LuauBackend {
    pub(super) fn emit_scalar_expr_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Arithmetic ops (real IR op kinds)
            // ================================================================
            "add" | "inplace_add" => {
                // Python + is overloaded: numeric add for numbers, concat for strings.
                // Only producer-derived operand facts may skip the type check:
                // the current op's result-side type_hint is passive metadata.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let lhs_num = self.numeric_operand_expr(&args[0]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    let is_numeric = self.scalar_plan.op_prefers_integer_runtime_lane(op)
                        || matches!(self.scalar_plan.op_scalar_lane(op), Some(ScalarKind::Float));
                    if is_numeric {
                        self.emit_line(&format!("local {out}: number = {lhs_num} + {rhs_num}"));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = if type({lhs}) == \"string\" or type({rhs}) == \"string\" then tostring({lhs}) .. tostring({rhs}) else {lhs_num} + {rhs_num}"
                        ));
                    }
                }
            }
            "sub" | "inplace_sub" => self.emit_scalar_binary_op(op, "-"),
            "mul" | "inplace_mul" => self.emit_scalar_binary_op(op, "*"),
            "div" => {
                // Luau 1/0 = inf (IEEE 754), Python raises ZeroDivisionError.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!(
                        "if {rhs} == 0 then error({{__type=\"ZeroDivisionError\", __msg=\"division by zero\"}}) end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} / {rhs_num}"));
                }
            }
            "mod" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!(
                        "if {rhs} == 0 then error({{__type=\"ZeroDivisionError\", __msg=\"integer modulo by zero\"}}) end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} % {rhs_num}"));
                }
            }
            "floordiv" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!(
                        "if {rhs} == 0 then error({{__type=\"ZeroDivisionError\", __msg=\"integer division or modulo by zero\"}}) end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} // {rhs_num}"));
                }
            }
            "pow" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!("local {out}: number = {lhs} ^ {rhs}"));
                }
            }
            "pow_mod" => {
                // Python pow(base, exp, mod) uses modular exponentiation;
                // computing base^exp directly overflows for large exponents.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let base = sanitize_ident(&args[0]);
                    let exp = sanitize_ident(&args[1]);
                    let modulus = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out}; do local __b, __e, __m = {base} % {modulus}, {exp}, {modulus}; \
                         local __r = 1; while __e > 0 do \
                         if __e % 2 == 1 then __r = (__r * __b) % __m end; \
                         __b = (__b * __b) % __m; __e = __e // 2 end; \
                         {out} = __r end"
                    ));
                }
            }
            "matmul" | "inplace_matmul" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let helper = if op.kind == "inplace_matmul" {
                        "molt_inplace_matmul"
                    } else {
                        "molt_matmul"
                    };
                    self.emit_line(&format!("local {out} = {helper}({lhs}, {rhs})"));
                }
            }
            "round" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = math.round({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "trunc" => {
                // Python math.trunc truncates toward zero.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let v = sanitize_ident(val);
                    self.emit_line(&format!(
                        "local {out} = if {v} >= 0 then math_floor({v}) else math.ceil({v})"
                    ));
                }
            }

            // ================================================================
            // Bitwise ops (real IR op kinds)
            // ================================================================
            "bit_and" | "inplace_bit_and" => self.emit_scalar_bit_op(op, "band"),
            "bit_or" | "inplace_bit_or" => self.emit_scalar_bit_op(op, "bor"),
            "bit_xor" | "inplace_bit_xor" => self.emit_scalar_bit_op(op, "bxor"),
            "lshift" | "shl" => self.emit_scalar_bit_op(op, "lshift"),
            "rshift" | "shr" => self.emit_scalar_bit_op(op, "rshift"),

            // ================================================================
            // Unary ops (real IR op kinds)
            // ================================================================
            "not" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let v = sanitize_ident(val);
                    if self.is_known_bool_value(val) {
                        self.emit_line(&format!("local {out}: boolean = not {v}"));
                    } else {
                        let truthy = self.guard_truthiness(val);
                        self.emit_line(&format!("local {out}: boolean = not {truthy}"));
                    }
                }
            }
            "invert" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = bit32.bnot({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "abs" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = math.abs({})", sanitize_ident(val)));
                }
            }

            // ================================================================
            // Comparison ops (real IR op kinds)
            // ================================================================
            "lt" | "le" | "gt" | "ge" | "eq" | "string_eq" | "ne" => {
                let operator = match op.kind.as_str() {
                    "lt" => "<",
                    "le" => "<=",
                    "gt" => ">",
                    "ge" => ">=",
                    "eq" | "string_eq" => "==",
                    "ne" => "~=",
                    _ => unreachable!(),
                };
                self.emit_scalar_binary_op(op, operator);
            }
            "is" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out}: boolean = ({lhs} == {rhs})"));
                }
            }

            // ================================================================
            // Logical ops - Python truthiness differs from Luau.
            // ================================================================
            "and" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let a = sanitize_ident(&args[0]);
                    let b = sanitize_ident(&args[1]);
                    if self.is_known_bool_value(&args[0]) && self.is_known_bool_value(&args[1]) {
                        self.emit_line(&format!("local {out} = {a} and {b}"));
                    } else {
                        let truthy = self.guard_truthiness(&args[0]);
                        self.emit_line(&format!("local {out} = if {truthy} then {b} else {a}"));
                    }
                }
            }
            "or" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let a = sanitize_ident(&args[0]);
                    let b = sanitize_ident(&args[1]);
                    if self.is_known_bool_value(&args[0]) && self.is_known_bool_value(&args[1]) {
                        self.emit_line(&format!("local {out} = {a} or {b}"));
                    } else {
                        let truthy = self.guard_truthiness(&args[0]);
                        self.emit_line(&format!("local {out} = if {truthy} then {a} else {b}"));
                    }
                }
            }

            // ================================================================
            // Pedagogical composite ops (binop/compare/unary_op with s_value)
            // ================================================================
            "binop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let op_str = op.s_value.as_deref().unwrap_or("+");
                    let expr = match op_str {
                        "+" | "-" | "*" | "/" | "%" => format!("{lhs} {op_str} {rhs}"),
                        "//" => format!("{lhs} // {rhs}"),
                        "**" => format!("{lhs} ^ {rhs}"),
                        "&" => format!("bit32.band({lhs}, {rhs})"),
                        "|" => format!("bit32.bor({lhs}, {rhs})"),
                        "^" => format!("bit32.bxor({lhs}, {rhs})"),
                        "<<" => format!("bit32.lshift({lhs}, {rhs})"),
                        ">>" => format!("bit32.rshift({lhs}, {rhs})"),
                        _ => format!("{lhs} {op_str} {rhs}"),
                    };
                    self.emit_line(&format!("local {out} = {expr}"));
                }
            }
            "compare" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let cmp = op.s_value.as_deref().unwrap_or("==");
                    let luau_cmp = match cmp {
                        "!=" | "<>" => "~=",
                        other => other,
                    };
                    self.emit_line(&format!("local {out} = {lhs} {luau_cmp} {rhs}"));
                }
            }
            "unary_op" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(operand) = args.first() {
                    let operand = sanitize_ident(operand);
                    let uop = op.s_value.as_deref().unwrap_or("-");
                    let expr = match uop {
                        "-" => format!("-{operand}"),
                        "not" => {
                            let truthy = self.guard_truthiness(args.first().unwrap());
                            format!("not {truthy}")
                        }
                        "~" => format!("bit32.bnot({operand})"),
                        _ => format!("-{operand}"),
                    };
                    self.emit_line(&format!("local {out} = {expr}"));
                }
            }
            _ => return false,
        }
        true
    }

    fn emit_scalar_binary_op(&mut self, op: &OpIR, operator: &str) {
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
            let is_cmp = matches!(operator, "==" | "~=" | "<" | "<=" | ">" | ">=");
            let is_logical = matches!(operator, "and" | "or");
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

    fn emit_scalar_bit_op(&mut self, op: &OpIR, func: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out}: number = bit32.{func}({lhs}, {rhs})"));
        }
    }
}
