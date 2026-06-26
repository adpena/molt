use super::*;

impl LuauBackend {
    pub(super) fn emit_scalar_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Constants
            // ================================================================
            "const" => {
                let out = self.out_var(op);
                if let Some(v) = op.value {
                    self.emit_line(&format!("local {out}: number = {v}"));
                } else if let Some(f) = op.f_value {
                    self.emit_line(&format!("local {out}: number = {f}"));
                } else if let Some(ref s) = op.s_value {
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "const_float" => {
                let out = self.out_var(op);
                let val = op.f_value.unwrap_or(0.0);
                self.emit_line(&format!("local {out}: number = {val}"));
            }
            "const_int" => {
                let out = self.out_var(op);
                let val = op.value.unwrap_or(0);
                self.emit_line(&format!("local {out}: number = {val}"));
            }
            "const_str" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out}: string = \"{escaped}\""));
            }
            "const_bytes" => {
                let out = self.out_var(op);
                if let Some(ref bytes) = op.bytes {
                    let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                } else {
                    let s = op.s_value.as_deref().unwrap_or("");
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                }
            }
            "const_bool" | "bool_const" => {
                let out = self.out_var(op);
                let val = if op.value.unwrap_or(0) != 0 {
                    "true"
                } else {
                    "false"
                };
                self.emit_line(&format!("local {out}: boolean = {val}"));
            }
            "const_none" | "none_const" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil"));
            }
            "string_const" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out}: string = \"{escaped}\""));
            }
            "const_bigint" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("0");
                self.emit_line(&format!("local {out} = tonumber(\"{s}\") or 0"));
            }
            "const_not_implemented" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_not_implemented"));
            }
            "const_ellipsis" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- {}", op.kind));
            }
            "missing" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_missing_sentinel"));
            }

            // ================================================================
            // Variable load/store (both pedagogical and real IR forms)
            // ================================================================
            "load_local" => {
                let out = self.out_var(op);
                let var = self.var_ref(op);
                self.emit_line(&format!("local {out} = {var}"));
            }
            "load_var" | "copy_var" => {
                let out = self.out_var(op);
                let var = op
                    .var
                    .as_deref()
                    .or_else(|| {
                        op.args
                            .as_deref()
                            .and_then(|args| args.first().map(String::as_str))
                    })
                    .map(sanitize_ident)
                    .unwrap_or_else(|| "_".to_string());
                self.emit_line(&format!("local {out} = {var}"));
            }
            "load" | "guarded_load" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    // Field offsets are byte offsets in 8-byte MoltValue slots.
                    let slot = (op.value.unwrap_or(0) / 8) + 1;
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("local {out} = {obj}[{slot}]"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "guard_type" | "guard_tag" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let value = sanitize_ident(&args[0]);
                    let expected = sanitize_ident(&args[1]);
                    if let Some(ref out_name) = op.out
                        && out_name != "none"
                    {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_guard_type({value}, {expected})"
                        ));
                    } else {
                        self.emit_line(&format!("molt_guard_type({value}, {expected})"));
                    }
                }
            }
            "closure_load" => {
                // closure_load: args[0] = slot name, out = destination var
                // Reads from closure slot (stored via closure_store).
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(slot) = args.first() {
                    let slot = sanitize_ident(slot);
                    self.emit_line(&format!("local {out} = __closure_{slot}"));
                } else {
                    let var = self.var_ref(op);
                    self.emit_line(&format!("local {out} = {var}"));
                }
            }
            "store_local" => {
                let var = self.var_ref(op);
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    // Propagate tuple tracking: if the source is a known
                    // tuple variable, the destination inherits that status.
                    if self.tuple_vars.contains(src)
                        && let Some(ref var_name) = op.var
                    {
                        self.tuple_vars.insert(var_name.clone());
                    }
                    self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                }
            }
            "store_var" => {
                let var = op
                    .var
                    .as_deref()
                    .or(op.out.as_deref())
                    .map(sanitize_ident)
                    .unwrap_or_else(|| "_".to_string());
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    if self.tuple_vars.contains(src)
                        && let Some(ref var_name) = op.var
                    {
                        self.tuple_vars.insert(var_name.clone());
                    }
                    self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                }
            }
            "store" | "store_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    // Field offsets are byte offsets in 8-byte MoltValue slots.
                    let slot = (op.value.unwrap_or(0) / 8) + 1;
                    let obj = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{obj}[{slot}] = {value}"));
                }
            }
            "closure_store" => {
                // closure_store: args[0] = slot name, args[1] = value
                // Stores value into a closure slot variable.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let slot = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("__closure_{slot} = {value}"));
                }
            }
            "identity_alias" | "binding_alias" => {
                let out = self.out_var(op);
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    // Propagate tuple tracking through aliases.
                    if self.tuple_vars.contains(src)
                        && let Some(ref out_name) = op.out
                    {
                        self.tuple_vars.insert(out_name.clone());
                    }
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(src)));
                }
            }

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
            "sub" | "inplace_sub" => self.emit_binary_op(op, "-"),
            "mul" | "inplace_mul" => self.emit_binary_op(op, "*"),
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
                    // Direct ^ operator — no helper call overhead.
                    self.emit_line(&format!("local {out}: number = {lhs} ^ {rhs}"));
                }
            }
            "pow_mod" => {
                // Python pow(base, exp, mod) uses modular exponentiation —
                // computing base^exp directly overflows for large exponents.
                // Emit a loop-based modular exponentiation (square-and-multiply).
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
                // Python math.trunc: truncates toward zero.
                // math.floor truncates toward negative infinity — wrong for negatives.
                // math.modf returns (integer_part, fractional_part).
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
            "bit_and" | "inplace_bit_and" => self.emit_bit_op(op, "band"),
            "bit_or" | "inplace_bit_or" => self.emit_bit_op(op, "bor"),
            "bit_xor" | "inplace_bit_xor" => self.emit_bit_op(op, "bxor"),
            "lshift" | "shl" => self.emit_bit_op(op, "lshift"),
            "rshift" | "shr" => self.emit_bit_op(op, "rshift"),

            // ================================================================
            // Unary ops (real IR op kinds)
            // ================================================================
            "not" => {
                // Python `not x` uses Python truthiness (0, "", [], {} are falsy).
                // Luau `not x` only treats nil/false as falsy.
                // Use molt_bool for Python-compatible truthiness when type is unknown.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let v = sanitize_ident(val);
                    let is_bool = self.is_known_bool_value(val);
                    if is_bool {
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
                self.emit_binary_op(op, operator);
            }
            "is" => {
                // Python `is` checks identity, not equality.  For `x is None`
                // this maps correctly to `x == nil` (fine since nil is a
                // singleton).  For non-None operands, `==` checks value
                // equality which differs, but there's no Luau equivalent for
                // identity.  This is an accepted semantic gap.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out}: boolean = ({lhs} == {rhs})"));
                }
            }

            // ================================================================
            // Logical ops — Python truthiness differs from Luau.
            // Python treats 0, "", [], {} as falsy; Luau only nil/false.
            // When operands are known-boolean (from comparisons), use native
            // and/or.  Otherwise use molt_bool() to get Python semantics.
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
                        // Python `a and b`: if a is falsy return a, else return b
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
                        // Python `a or b`: if a is truthy return a, else return b
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

            // ================================================================
            // Len / type introspection
            // ================================================================
            "len" => {
                // # operator (LOP_LENGTH) is a single opcode — 2-3x faster than
                // molt_len() function call. Use # directly when type is known;
                // fall back to molt_len() for unknown types (handles 0 for non-table/string).
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj_s = sanitize_ident(obj);
                    let is_known_lennable = matches!(
                        self.scalar_plan.name_container_kind(obj),
                        Some(ContainerKind::List | ContainerKind::Tuple | ContainerKind::Str)
                    );
                    if is_known_lennable {
                        self.emit_line(&format!("local {out} = #{obj_s}"));
                    } else {
                        self.emit_line(&format!("local {out} = molt_len({obj_s})"));
                    }
                }
            }
            "id" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(obj)));
                }
            }
            "ord" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_ord({})", sanitize_ident(val)));
                }
            }
            "ord_at" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_ord_at({container}, {key})"));
                }
            }
            "chr" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.char({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "type_of" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_type_of({})",
                        sanitize_ident(obj)
                    ));
                }
            }
            "isinstance" | "issubclass" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let cls = sanitize_ident(&args[1]);
                    let helper = if op.kind == "isinstance" {
                        "molt_isinstance"
                    } else {
                        "molt_issubclass"
                    };
                    self.emit_line(&format!("local {out} = {helper}({obj}, {cls})"));
                } else {
                    self.emit_line(&format!("local {out} = molt_isinstance(nil, nil)"));
                }
            }
            "exception_match_builtin" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc) = args.first() {
                    let class_name = op.s_value.as_deref().unwrap_or("Exception");
                    self.emit_line(&format!(
                        "local {out} = molt_exception_match({}, \"{class_name}\")",
                        sanitize_ident(exc)
                    ));
                } else {
                    self.emit_line(&format!("local {out} = false"));
                }
            }

            // ================================================================
            // Type casting
            // ================================================================
            "int_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_int({})", sanitize_ident(val)));
                }
            }
            "int_from_str_of_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let val = sanitize_ident(&args[0]);
                    let base = sanitize_ident(&args[1]);
                    let has_base = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = if molt_bool({has_base}) then math.floor(tonumber(molt_str({val}), molt_int({base})) or 0) else molt_int(molt_str({val}))"
                    ));
                }
            }
            "float_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_float({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "str_from_obj" | "ascii_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_str({})", sanitize_ident(val)));
                }
            }
            "repr_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_repr({})", sanitize_ident(val)));
                }
            }
            "bytes_from_obj" | "bytes_from_str" | "bytearray_from_str" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(val)));
                }
            }
            "bytearray_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let val = sanitize_ident(val);
                    self.emit_line(&format!(
                        "local {out} = if type({val}) == \"number\" then string.rep(string.char(0), math.max(0, {val})) else tostring({val})"
                    ));
                }
            }

            // ================================================================
            // Print
            // ================================================================
            "print" => {
                let call_args = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("molt_print({call_args})"));
            }
            "print_newline" => {
                self.emit_line("molt_print()");
            }

            // ================================================================
            // Raw int bridge (no-op in Luau — values are already unboxed)
            // ================================================================
            "unbox_to_raw_int" | "box_from_raw_int" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(val)));
                }
            }

            kind if kind.starts_with("vec_sum_")
                || kind.starts_with("vec_prod_")
                || kind.starts_with("vec_min_")
                || kind.starts_with("vec_max_") =>
            {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let iterable = sanitize_ident(iterable);
                    let (init, body_op) = if kind.starts_with("vec_sum_") {
                        ("0", "acc = acc + v")
                    } else if kind.starts_with("vec_prod_") {
                        ("1", "acc = acc * v")
                    } else if kind.starts_with("vec_min_") {
                        ("math.huge", "if v < acc then acc = v end")
                    } else {
                        ("-math.huge", "if v > acc then acc = v end")
                    };
                    self.emit_line(&format!("local {out}; do -- [vectorized: {kind}]"));
                    self.push_indent();
                    self.emit_line(&format!(
                        "if type({iterable}) == \"table\" and #({iterable}) > 0 then"
                    ));
                    self.push_indent();
                    self.emit_line(&format!("local acc = {init}"));
                    self.emit_line(&format!(
                        "for __vi = 1, #{iterable} do local v = {iterable}[__vi]; {body_op} end"
                    ));
                    self.emit_line(&format!("{out} = {{acc, false}}"));
                    self.pop_indent();
                    self.emit_line("else");
                    self.push_indent();
                    self.emit_line(&format!("{out} = {{nil, true}}"));
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                } else {
                    self.emit_line(&format!(
                        "local {out} = {{nil, true}} -- [vectorized: {kind}]"
                    ));
                }
            }
            "intarray_from_seq" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(seq) = args.first() {
                    let seq = sanitize_ident(seq);
                    self.emit_line(&format!("local {out}"));
                    self.emit_line("do");
                    self.push_indent();
                    self.emit_line(&format!("local __seq = {seq}"));
                    self.emit_line("if type(__seq) == \"table\" then");
                    self.push_indent();
                    self.emit_line("local __arr = {}");
                    self.emit_line("local __ok = true");
                    self.emit_line("for __i = 1, #__seq do");
                    self.push_indent();
                    self.emit_line("local __v = __seq[__i]");
                    self.emit_line("if type(__v) == \"number\" and math.floor(__v) == __v then");
                    self.push_indent();
                    self.emit_line("__arr[__i] = __v");
                    self.pop_indent();
                    self.emit_line("else");
                    self.push_indent();
                    self.emit_line("__ok = false");
                    self.emit_line("break");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line(&format!("{out} = if __ok then __arr else nil"));
                    self.pop_indent();
                    self.emit_line("else");
                    self.push_indent();
                    self.emit_line(&format!("{out} = nil"));
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                }
            }
            "checked_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let flag_out = op.out.as_deref().map(sanitize_ident);
                    let sum_out = op.var.as_deref().map(sanitize_ident);
                    match (sum_out, flag_out) {
                        (Some(sum), Some(flag)) => {
                            self.emit_line(&format!(
                                "local {sum}: number, {flag}: boolean = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (Some(sum), None) => {
                            self.emit_line(&format!(
                                "local {sum}: number = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (None, Some(flag)) => {
                            self.emit_line(&format!(
                                "local _, {flag}: boolean = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }
            "checked_mul" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let flag_out = op.out.as_deref().map(sanitize_ident);
                    let product_out = op.var.as_deref().map(sanitize_ident);
                    match (product_out, flag_out) {
                        (Some(product), Some(flag)) => {
                            self.emit_line(&format!(
                                "local {product}: number, {flag}: boolean = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (Some(product), None) => {
                            self.emit_line(&format!(
                                "local {product}: number = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (None, Some(flag)) => {
                            self.emit_line(&format!(
                                "local _, {flag}: boolean = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }

            _ => return false,
        }
        true
    }
}
