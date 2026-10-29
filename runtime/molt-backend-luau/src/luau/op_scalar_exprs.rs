use super::*;

impl LuauBackend {
    pub(super) fn emit_scalar_expr_op(&mut self, op: &OpIR) -> bool {
        match op.kind {
            // ================================================================
            // Arithmetic ops (real IR op kinds)
            // ================================================================
            molt_ir::OpKind::Add | molt_ir::OpKind::InplaceAdd => {
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
            molt_ir::OpKind::Sub | molt_ir::OpKind::InplaceSub => {
                self.emit_scalar_binary_op(op, "-")
            }
            molt_ir::OpKind::Mul | molt_ir::OpKind::InplaceMul => {
                self.emit_scalar_binary_op(op, "*")
            }
            molt_ir::OpKind::Div | molt_ir::OpKind::InplaceDiv => {
                // Luau 1/0 = inf (IEEE 754), Python raises ZeroDivisionError.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    let float_operands = args.iter().take(2).any(|arg| {
                        self.scalar_plan.name_scalar_kind(arg) == Some(ScalarKind::Float)
                    });
                    let kind = if float_operands { "float" } else { "int" };
                    self.emit_line(&format!(
                        "if {rhs_num} == 0 then molt_numeric_error(\"truediv:{kind}\") end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} / {rhs_num}"));
                }
            }
            molt_ir::OpKind::Mod | molt_ir::OpKind::InplaceMod => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    let float_operands = args.iter().take(2).any(|arg| {
                        self.scalar_plan.name_scalar_kind(arg) == Some(ScalarKind::Float)
                    });
                    let kind = if float_operands { "float" } else { "int" };
                    self.emit_line(&format!(
                        "if {rhs_num} == 0 then molt_numeric_error(\"mod:{kind}\") end"
                    ));
                    if float_operands {
                        self.emit_line(&format!(
                            "local {out}: number = molt_float_mod({lhs}, {rhs_num})"
                        ));
                    } else {
                        self.emit_line(&format!("local {out}: number = {lhs} % {rhs_num}"));
                    }
                }
            }
            molt_ir::OpKind::Floordiv | molt_ir::OpKind::InplaceFloordiv => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    let float_operands = args.iter().take(2).any(|arg| {
                        self.scalar_plan.name_scalar_kind(arg) == Some(ScalarKind::Float)
                    });
                    let kind = if float_operands { "float" } else { "int" };
                    self.emit_line(&format!(
                        "if {rhs_num} == 0 then molt_numeric_error(\"floordiv:{kind}\") end"
                    ));
                    if float_operands {
                        self.emit_line(&format!(
                            "local {out}: number = molt_float_floor_div({lhs}, {rhs_num})"
                        ));
                    } else {
                        self.emit_line(&format!("local {out}: number = {lhs} // {rhs_num}"));
                    }
                }
            }
            molt_ir::OpKind::Pow | molt_ir::OpKind::InplacePow => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!(
                        "if {lhs} == 0 and {rhs} < 0 then molt_numeric_error(\"pow:real\") end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} ^ {rhs}"));
                }
            }
            molt_ir::OpKind::PowMod => {
                // Python pow(base, exp, mod) uses modular exponentiation;
                // computing base^exp directly overflows for large exponents.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let base = sanitize_ident(&args[0]);
                    let exp = sanitize_ident(&args[1]);
                    let modulus = self.numeric_operand_expr(&args[2]);
                    self.emit_line(&format!(
                        "if {modulus} == 0 then molt_numeric_error(\"pow_mod:int\") end"
                    ));
                    self.emit_line(&format!(
                        "local {out}; do local __b, __e, __m = {base} % {modulus}, {exp}, {modulus}; \
                         local __r = 1; while __e > 0 do \
                         if __e % 2 == 1 then __r = (__r * __b) % __m end; \
                         __b = (__b * __b) % __m; __e = __e // 2 end; \
                         {out} = __r end"
                    ));
                }
            }
            molt_ir::OpKind::Matmul | molt_ir::OpKind::InplaceMatmul => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let helper = if op.kind == molt_ir::OpKind::InplaceMatmul {
                        "molt_inplace_matmul"
                    } else {
                        "molt_matmul"
                    };
                    self.emit_line(&format!("local {out} = {helper}({lhs}, {rhs})"));
                }
            }
            molt_ir::OpKind::Round => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = math.round({})",
                        sanitize_ident(val)
                    ));
                }
            }
            molt_ir::OpKind::Trunc => {
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
            molt_ir::OpKind::BitAnd | molt_ir::OpKind::InplaceBitAnd => {
                self.emit_scalar_bit_op(op, molt_ir::OpKind::Band)
            }
            molt_ir::OpKind::BitOr | molt_ir::OpKind::InplaceBitOr => {
                self.emit_scalar_bit_op(op, molt_ir::OpKind::Bor)
            }
            molt_ir::OpKind::BitXor | molt_ir::OpKind::InplaceBitXor => {
                self.emit_scalar_bit_op(op, molt_ir::OpKind::Bxor)
            }
            molt_ir::OpKind::Lshift | molt_ir::OpKind::Shl => {
                self.emit_scalar_bit_op(op, molt_ir::OpKind::Lshift)
            }
            molt_ir::OpKind::Rshift | molt_ir::OpKind::Shr => {
                self.emit_scalar_bit_op(op, molt_ir::OpKind::Rshift)
            }

            // ================================================================
            // Unary ops (real IR op kinds)
            // ================================================================
            molt_ir::OpKind::Not => {
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
            molt_ir::OpKind::Invert => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = bit32.bnot({})",
                        sanitize_ident(val)
                    ));
                }
            }
            molt_ir::OpKind::Abs => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = math.abs({})", sanitize_ident(val)));
                }
            }

            // ================================================================
            // Comparison ops (real IR op kinds)
            // ================================================================
            molt_ir::OpKind::Lt
            | molt_ir::OpKind::Le
            | molt_ir::OpKind::Gt
            | molt_ir::OpKind::Ge
            | molt_ir::OpKind::StringEq => {
                let operator = match op.kind {
                    molt_ir::OpKind::Lt => "<",
                    molt_ir::OpKind::Le => "<=",
                    molt_ir::OpKind::Gt => ">",
                    molt_ir::OpKind::Ge => ">=",
                    molt_ir::OpKind::Eq | molt_ir::OpKind::StringEq => "==",
                    molt_ir::OpKind::Ne => "~=",
                    _ => unreachable!(),
                };
                self.emit_scalar_binary_op(op, operator);
            }
            molt_ir::OpKind::Eq | molt_ir::OpKind::Ne => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let lhs_kind = self.scalar_plan.name_scalar_kind(&args[0]);
                    let rhs_kind = self.scalar_plan.name_scalar_kind(&args[1]);
                    let bool_number_cross = matches!(
                        (lhs_kind, rhs_kind),
                        (
                            Some(ScalarKind::Bool),
                            Some(ScalarKind::Int | ScalarKind::Float)
                        ) | (
                            Some(ScalarKind::Int | ScalarKind::Float),
                            Some(ScalarKind::Bool)
                        )
                    );
                    if lhs_kind.is_some() && rhs_kind.is_some() && !bool_number_cross {
                        let operator = if op.kind == molt_ir::OpKind::Ne {
                            "~="
                        } else {
                            "=="
                        };
                        self.emit_line(&format!("local {out}: boolean = ({lhs} {operator} {rhs})"));
                    } else {
                        let negation = if op.kind == molt_ir::OpKind::Ne {
                            "not "
                        } else {
                            ""
                        };
                        self.emit_line(&format!(
                            "local {out}: boolean = {negation}molt_equal({lhs}, {rhs})"
                        ));
                    }
                }
            }
            molt_ir::OpKind::Is | molt_ir::OpKind::IsNot => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let identity = self.identity_comparison_expr(
                        &args[0],
                        &args[1],
                        op.kind == molt_ir::OpKind::IsNot,
                    );
                    self.emit_line(&format!("local {out}: boolean = {identity}"));
                }
            }

            // ================================================================
            // Logical ops - Python truthiness differs from Luau.
            // ================================================================
            molt_ir::OpKind::And => {
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
            molt_ir::OpKind::Or => {
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
            molt_ir::OpKind::Binop => {
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
            molt_ir::OpKind::Compare => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let cmp = op.s_value.as_deref().unwrap_or("==");
                    if matches!(cmp, "==" | "!=" | "<>") {
                        let negation = if cmp == "==" { "" } else { "not " };
                        self.emit_line(&format!(
                            "local {out} = {negation}molt_equal({lhs}, {rhs})"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = {lhs} {cmp} {rhs}"));
                    }
                }
            }
            molt_ir::OpKind::UnaryOp => {
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

    fn emit_scalar_bit_op(&mut self, op: &OpIR, func: molt_ir::OpKind) {
        let func = func.as_str();
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out}: number = bit32.{func}({lhs}, {rhs})"));
        }
    }
}
