use super::*;

impl LuauBackend {
    pub(super) fn emit_op(&mut self, op: &OpIR) {
        if self.emit_collection_op(op) {
            return;
        }
        if self.emit_attribute_op(op) {
            return;
        }
        if self.emit_string_op(op) {
            return;
        }
        if self.emit_object_op(op) {
            return;
        }

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
            // Control flow: labels and jumps
            // ================================================================
            "label" | "state_label" => {
                // Emit real Luau labels — Roblox Studio supports goto/labels.
                if let Some(id) = op.value {
                    self.emit_line(&format!("::label_{id}::"));
                } else if let Some(ref s) = op.s_value {
                    let label = sanitize_label(s);
                    self.emit_line(&format!("::{label}::"));
                }
            }
            "jump" | "goto" => {
                if let Some(id) = op.value {
                    self.emit_line(&format!("goto label_{id}"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("goto {target}"));
                }
            }
            "pcall_failure_jump" => {
                let pcall_id = op
                    .s_value
                    .as_deref()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                if let Some(id) = op.value {
                    self.emit_line(&format!("if not __ok_{pcall_id} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if not __ok_{pcall_id} then goto {target} end"));
                }
            }
            "br_if" => {
                // Emit real conditional goto with Python truthiness guard.
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    if let Some(id) = op.value {
                        self.emit_line(&format!("if {cond} then goto label_{id} end"));
                    } else if let Some(ref target) = op.s_value {
                        let target = sanitize_label(target);
                        self.emit_line(&format!("if {cond} then goto {target} end"));
                    } else {
                        let cond_ident = sanitize_ident(cond_raw);
                        self.emit_line(&format!(
                            "error(\"[unsupported op: br_if {cond_ident} missing target label]\")"
                        ));
                    }
                }
            }
            "branch" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond_raw = args.first().map(|s| s.as_str()).or(op.var.as_deref());
                let cond = if let Some(raw) = cond_raw {
                    self.guard_truthiness(raw)
                } else {
                    "true".to_string()
                };
                if let Some(id) = op.value {
                    self.emit_line(&format!("if {cond} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if {cond} then goto {target} end"));
                } else {
                    self.emit_line(&format!(
                        "error(\"[unsupported op: branch {cond} missing target label]\")"
                    ));
                }
            }
            "branch_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond_raw = args.first().map(|s| s.as_str()).or(op.var.as_deref());
                let cond = if let Some(raw) = cond_raw {
                    self.guard_truthiness(raw)
                } else {
                    "false".to_string()
                };
                let not_cond = if cond.starts_with("molt_bool(") {
                    format!("not ({cond})")
                } else {
                    format!("not {cond}")
                };
                if let Some(id) = op.value {
                    self.emit_line(&format!("if {not_cond} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if {not_cond} then goto {target} end"));
                } else {
                    self.emit_line(&format!(
                        "error(\"[unsupported op: branch_false {cond} missing target label]\")"
                    ));
                }
            }

            // ================================================================
            // Structured if/else/end_if
            // ================================================================
            "if" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    let cond_ident = sanitize_ident(cond);
                    // Python truthiness: 0, "", [], {} are falsy but Luau
                    // treats them as truthy. Known boolean producers and
                    // literal booleans can be used directly. Otherwise wrap in
                    // molt_bool().
                    let is_bool = self.is_known_bool_value(cond);
                    if is_bool {
                        self.emit_line(&format!("if {cond_ident} then"));
                    } else {
                        self.emit_line(&format!("if molt_bool({cond_ident}) then"));
                    }
                    self.push_indent();
                }
            }
            "else" => {
                self.pop_indent();
                self.emit_line("else");
                self.push_indent();
            }
            "end_if" => {
                self.pop_indent();
                self.emit_line("end");
            }

            // ================================================================
            // Loops
            // ================================================================
            "loop_start" => {
                // Check if the next op is loop_index_start — if so, its
                // initialization is handled via the pending_loop_index
                // mechanism to ensure it runs before the while opens.
                self.emit_line("while true do");
                self.push_indent();
            }
            "loop_index_start" => {
                // No-op here — initialization is emitted before the while
                // by the loop_start handler via pending_loop_index.
                // The pre-declared variable is set before loop entry.
            }
            "loop_end" => {
                self.pop_indent();
                self.emit_line("end");
            }
            "loop_break" => {
                self.emit_line("break");
            }
            "loop_break_if_true" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    self.emit_line(&format!("if {cond} then break end"));
                }
            }
            "loop_break_if_exception" => {
                // Value-less exception-flag break.  On native/WASM the producer
                // returns a None sentinel on a mid-iteration raise and this op
                // breaks the consumption loop so it cannot spin forever.  In the
                // Luau backend, Python exceptions are raised via Lua `error()`,
                // which unwinds the call stack immediately out of the iterator
                // closure call (`iter_var()` in the `iter_next` lowering) — the
                // loop body after `iter_next` never executes, so there is no
                // sentinel-driven spin to break.  The op is therefore a no-op
                // here: the unwinding `error()` already exited the loop and is
                // caught by the enclosing `pcall` for the active try/except.
            }
            "loop_break_if_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    // Use parens only when cond is a molt_bool() call (compound expr).
                    // Plain idents don't need parens and must not have them
                    // (optimization passes pattern-match `if not vN then`).
                    if cond.starts_with("molt_bool(") {
                        self.emit_line(&format!("if not ({cond}) then break end"));
                    } else {
                        self.emit_line(&format!("if not {cond} then break end"));
                    }
                }
            }
            "loop_continue" => {
                self.emit_line("continue");
            }
            "loop_index_next" => {
                // Update loop counter: out = args[0].
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(new_val) = args.first() {
                        let val = sanitize_ident(new_val);
                        self.emit_line(&format!("{out} = {val}"));
                    }
                }
            }
            "loop_carry_init" | "loop_carry_update" => {
                // Internal loop bookkeeping — skip.
            }
            "for_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let start = sanitize_ident(&args[0]);
                    let stop = sanitize_ident(&args[1]);
                    let step = if args.len() >= 3 {
                        sanitize_ident(&args[2])
                    } else {
                        "1".to_string()
                    };
                    // Python range is exclusive of stop; Luau for-loop is inclusive.
                    // For positive step: limit = stop - 1
                    // For negative step: limit = stop + 1
                    // When step is a variable, emit a ternary at runtime.
                    let limit_expr = if step == "1" {
                        format!("{stop} - 1")
                    } else if step == "-1" {
                        format!("{stop} + 1")
                    } else if let Ok(n) = step.parse::<i64>() {
                        if n > 0 {
                            format!("{stop} - 1")
                        } else {
                            format!("{stop} + 1")
                        }
                    } else {
                        format!("if {step} > 0 then {stop} - 1 else {stop} + 1")
                    };
                    self.emit_line(&format!("for {out} = {start}, {limit_expr}, {step} do"));
                    self.push_indent();
                }
            }
            "for_iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let iterable = sanitize_ident(iterable);
                    self.emit_line(&format!("for _, {out} in ipairs({iterable}) do"));
                    self.push_indent();
                }
            }
            "end_for" => {
                self.pop_indent();
                self.emit_line("end");
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
            "call_func" => {
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
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let obj = sanitize_ident(&args[0]);
                    let method_name = op.s_value.as_deref().unwrap_or("unknown");
                    let method = sanitize_ident(method_name);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    // When the receiver is a known list, emit direct table
                    // operations instead of method calls (Luau tables don't
                    // have Python list methods).
                    let obj_is_list = self.plan_knows_list(&args[0]);
                    if obj_is_list {
                        match method_name {
                            "append" => {
                                if let Some(val) = args.get(1) {
                                    self.emit_line(&format!(
                                        "{obj}[#{obj} + 1] = {}",
                                        sanitize_ident(val)
                                    ));
                                }
                                // append returns None in Python; skip output.
                            }
                            "pop" => {
                                let idx = args.get(1).map(|s| sanitize_ident(s));
                                if let Some(ref out_name) = op.out {
                                    let out = sanitize_ident(out_name);
                                    self.emit_list_pop(&obj, idx.as_deref(), Some(&out));
                                } else {
                                    self.emit_list_pop(&obj, idx.as_deref(), None);
                                }
                            }
                            "insert" => {
                                if args.len() >= 3 {
                                    let idx = sanitize_ident(&args[1]);
                                    let val = sanitize_ident(&args[2]);
                                    self.emit_list_insert(&obj, &idx, &val);
                                }
                            }
                            "remove" => {
                                if let Some(val) = args.get(1) {
                                    let val = sanitize_ident(val);
                                    // Guard: table.find returns nil when not found,
                                    // and table.remove(t, nil) silently removes the
                                    // LAST element. Must check and raise ValueError.
                                    self.emit_line(&format!(
                                        "do local __idx = table.find({obj}, {val}); if __idx then table.remove({obj}, __idx) else error(\"ValueError: list.remove(x): x not in list\") end end"
                                    ));
                                }
                            }
                            "sort" => {
                                self.emit_line(&format!("table.sort({obj})"));
                            }
                            "reverse" => {
                                // In-place reverse via swap loop.
                                self.emit_line(&format!(
                                    "for __i = 1, math.floor(#{obj} / 2) do {obj}[__i], {obj}[#{obj} - __i + 1] = {obj}[#{obj} - __i + 1], {obj}[__i] end"
                                ));
                            }
                            "clear" => {
                                self.emit_line(&format!("table.clear({obj})"));
                            }
                            "copy" => {
                                if let Some(ref out_name) = op.out {
                                    let out = sanitize_ident(out_name);
                                    self.emit_line(&format!("local {out} = table.clone({obj})"));
                                }
                            }
                            "extend" => {
                                if let Some(other) = args.get(1) {
                                    let other = sanitize_ident(other);
                                    self.emit_line(&format!(
                                        "table.move({other}, 1, #{other}, #{obj} + 1, {obj})"
                                    ));
                                }
                            }
                            _ => {
                                // Fall through to generic method call for count/index/etc.
                                if let Some(ref out_name) = op.out {
                                    let out = sanitize_ident(out_name);
                                    self.emit_line(&format!(
                                        "local {out} = {obj}:{method}({call_args})"
                                    ));
                                } else {
                                    self.emit_line(&format!("{obj}:{method}({call_args})"));
                                }
                            }
                        }
                    } else if let Some(ref out_name) = op.out {
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
            // Return
            // ================================================================
            "ret" | "return" | "return_value" => {
                if let Some(ref args) = op.args {
                    if args.len() == 1 {
                        let val = sanitize_ident(&args[0]);
                        if self.tuple_vars.contains(&args[0]) {
                            self.emit_line(&format!("return table.unpack({val})"));
                        } else {
                            self.emit_line(&format!("return {val}"));
                        }
                    } else if args.len() > 1 {
                        let vals: Vec<String> = args.iter().map(|a| sanitize_ident(a)).collect();
                        self.emit_line(&format!("return {}", vals.join(", ")));
                    } else {
                        self.emit_line("return");
                    }
                } else if let Some(ref var) = op.var {
                    let val = sanitize_ident(var);
                    if self.tuple_vars.contains(var) {
                        self.emit_line(&format!("return table.unpack({val})"));
                    } else {
                        self.emit_line(&format!("return {val}"));
                    }
                } else {
                    self.emit_line("return");
                }
            }
            "ret_void" => {
                self.emit_line("return");
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
            // Exception handling (stubs)
            // ================================================================
            "exception_push"
            | "exception_pop"
            | "exception_stack_clear"
            | "exception_stack_enter"
            | "exception_stack_exit"
            | "exception_set_last"
            | "exception_set_value"
            | "exception_set_cause"
            | "exception_context_set"
            | "exception_stack_set_depth" => {
                // Exception bookkeeping — no Luau equivalent.
            }
            "exception_clear" => {
                // Clear the pcall error so subsequent exception_last returns nil.
                let explicit_pcall = op.value.and_then(|n| u32::try_from(n).ok());
                if let Some(n) = explicit_pcall.or_else(|| {
                    (!self.inside_pcall_body)
                        .then(|| self.try_depth_counter.last().copied())
                        .flatten()
                }) {
                    self.emit_line(&format!("__err_{n} = nil"));
                }
            }
            "drop_inserted"
            | "exception_region_drops_inserted"
            | "inc_ref"
            | "dec_ref"
            | "release" => {
                // Shared TIR DropInsertion artifacts are consumed explicitly.
                // Luau is GC-managed, so RC barriers and drop-fact markers are
                // semantic no-ops here rather than unsupported transport.
            }
            "exception_new" | "exception_new_builtin" | "exception_new_from_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"error\"".to_string());
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = {msg}}}"
                ));
            }
            "exception_new_builtin_empty" => {
                let out = self.out_var(op);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = \"\"}}"
                ));
            }
            "exception_new_builtin_one" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"\"".to_string());
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = {msg}}}"
                ));
            }
            "raise" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("error({})", sanitize_ident(val)));
                } else {
                    self.emit_line("error(\"raised\")");
                }
            }
            "check_exception" => {
                // Suppress — exception handler jumps are no-ops in Luau.
            }
            "exception_last" | "exception_last_pending" | "exception_finally_pending_observer" => {
                let out = self.out_var(op);
                if let Some(n) = op.value.and_then(|n| u32::try_from(n).ok()) {
                    self.emit_line(&format!("local {out} = __err_{n}"));
                } else if !self.inside_pcall_body {
                    if let Some(&n) = self.try_depth_counter.last() {
                        // After pcall — read the captured error.
                        self.emit_line(&format!("local {out} = __err_{n}"));
                    } else {
                        self.emit_line(&format!("local {out} = nil -- [exception_last]"));
                    }
                } else {
                    // Inside pcall body — no exception yet, return nil.
                    self.emit_line(&format!("local {out} = nil -- [exception_last]"));
                }
            }
            "exception_kind" => {
                // exception_kind extracts the type from an exception object.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc_var) = args.first() {
                    let exc = sanitize_ident(exc_var);
                    self.emit_line(&format!("local {out} = molt_exception_kind({exc})"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "exception_class" => {
                // exception_class returns the class name for isinstance matching.
                // args[0] is already the class name string — pass it through.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class_var) = args.first() {
                    let cls = sanitize_ident(class_var);
                    self.emit_line(&format!("local {out} = {cls}"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "exception_message" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc_var) = args.first() {
                    let exc = sanitize_ident(exc_var);
                    self.emit_line(&format!(
                        "local {out} = (type({exc}) == \"table\" and {exc}.__msg or tostring({exc}))"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil -- [exception_message]"));
                }
            }
            "exception_stack_depth" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = 0"));
            }
            "exceptiongroup_match" | "exceptiongroup_combine" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
            }

            // ================================================================
            // Context manager stubs
            // ================================================================
            "context_null" | "context_enter" | "context_exit" | "context_closing"
            | "context_unwind" | "context_unwind_to" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [context: {}]", op.kind));
                }
            }
            "context_depth" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = 0"));
            }

            // ================================================================
            // Async/generator stubs
            // ================================================================
            "state_yield" => {
                // Generator yield: yield the value to the coroutine consumer.
                // args[0] is the value (typically a {result, false} tuple).
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("coroutine.yield({})", sanitize_ident(val)));
                }
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: state_yield]"));
                }
            }
            "state_switch"
            | "state_transition"
            | "chan_new"
            | "chan_drop"
            | "chan_send_yield"
            | "chan_recv_yield"
            | "cancel_token_new"
            | "cancel_token_clone"
            | "cancel_token_drop"
            | "cancel_token_cancel"
            | "cancel_token_is_cancelled"
            | "cancel_token_set_current"
            | "cancel_token_get_current"
            | "cancelled"
            | "cancel_current"
            | "future_cancel"
            | "future_cancel_msg"
            | "future_cancel_clear"
            | "promise_new"
            | "promise_set_result"
            | "promise_set_exception"
            | "thread_submit"
            | "task_register_token_owned" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: {}]", op.kind));
                }
            }
            "is_native_awaitable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = false"));
                }
            }

            // ================================================================
            // File I/O stubs
            // ================================================================
            "file_open" | "file_read" | "file_write" | "file_close" | "file_flush" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [file: {}]", op.kind));
                }
            }

            // ================================================================
            // Misc intrinsics
            // ================================================================
            "getargv" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }
            "sys_executable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = \"\""));
                }
            }
            "getframe" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "bridge_unavailable" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let msg = args
                    .first()
                    .map(|arg| {
                        format!(
                            "\"Molt bridge unavailable: \" .. tostring({})",
                            sanitize_ident(arg)
                        )
                    })
                    .unwrap_or_else(|| "\"Molt bridge unavailable\"".to_string());
                let diagnostic = format!("{{__type=\"RuntimeError\", __msg={msg}}}");
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out}: any = error({diagnostic})"));
                } else {
                    self.emit_line(&format!("error({diagnostic})"));
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

            // ================================================================
            // Closure/code internals
            // ================================================================
            "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "function_closure_bits" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [internal: {}]", op.kind));
                }
            }
            "code_slot_set" | "code_slots_init" | "frame_locals_set" => {
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "trace_enter_slot" | "trace_exit" => {
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }

            // ================================================================
            // Line info (debug)
            // ================================================================
            "line" => {
                // Skip line markers in production output — they add
                // ~3% to file size with no runtime benefit.
                // Uncomment for debugging: self.emit_line(&format!("-- line {val}"));
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

            // ================================================================
            // Vectorized reduction ops (emit stubs)
            // ================================================================
            kind if kind.starts_with("vec_sum_")
                || kind.starts_with("vec_prod_")
                || kind.starts_with("vec_min_")
                || kind.starts_with("vec_max_") =>
            {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                // Vectorized reduction: args[0] = iterable.
                // Emit as a Luau loop that computes the reduction, returning
                // a tuple-like table {result, false} on success.
                // The subsequent get_item(out, 0) and get_item(out, 1) unpack it.
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
                    // No iterable arg — emit nil tuple (will fall through to loop).
                    self.emit_line(&format!(
                        "local {out} = {{nil, true}} -- [vectorized: {kind}]"
                    ));
                }
            }

            // ================================================================
            // Serialization stubs
            // ================================================================
            "json_parse" | "msgpack_parse" | "cbor_parse" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }
            "invoke_ffi" => {
                let diagnostic =
                    "{__type=\"RuntimeError\", __msg=\"Luau target does not support FFI\"}";
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out}: any = error({diagnostic})"));
                } else {
                    self.emit_line(&format!("error({diagnostic})"));
                }
            }

            // ================================================================
            // Memoryview / complex / bytearray stubs
            // ================================================================
            "memoryview_new" | "memoryview_tobytes" | "memoryview_cast" | "complex_from_obj" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
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
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }

            "bytearray_fill_range" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 4 {
                    let bytearray = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    let stop = sanitize_ident(&args[2]);
                    let value = sanitize_ident(&args[3]);
                    self.emit_line(&format!(
                        "do local __ba = {bytearray}; local __start = {start}; local __stop = {stop}; local __byte = {value}; if __byte < 0 or __byte > 255 then error({{__type=\"ValueError\", __msg=\"byte must be in range(0, 256)\"}}) end; if __start < 0 or __stop < __start or __stop > #__ba then error({{__type=\"IndexError\", __msg=\"bytearray fill range out of range\"}}) end; {bytearray} = string.sub(__ba, 1, __start) .. string.rep(string.char(__byte), __stop - __start) .. string.sub(__ba, __stop + 1) end"
                    ));
                }
            }

            // ================================================================
            // Iterator ops
            // ================================================================
            "iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let it = sanitize_ident(iterable);
                    // Create a stateful iterator closure over the table.
                    // Each call returns {value, nil} or {nil, true} when exhausted.
                    self.emit_line(&format!(
                        "local {out}; do local _t = {it}; local _i = 0; \
                         {out} = function() _i = _i + 1; \
                         if _i <= #_t then return {{_t[_i], nil}} \
                         else return {{nil, true}} end; end; end"
                    ));
                }
            }
            "iter_next" => {
                // Call the stateful iterator closure created by the `iter` op.
                // Returns {value, nil} or {nil, true} when exhausted.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iter_var) = args.first() {
                    let iter_var = sanitize_ident(iter_var);
                    self.emit_line(&format!("local {out} = {iter_var}()"));
                }
            }
            "iter_next_unboxed" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iter_var) = args.first() {
                    let iter_var = sanitize_ident(iter_var);
                    let done_out = op.out.as_deref().map(sanitize_ident);
                    let value_out = op.var.as_deref().map(sanitize_ident);
                    let tmp_seed = done_out
                        .as_deref()
                        .or(value_out.as_deref())
                        .unwrap_or("iter");
                    let tmp = format!("__next_{tmp_seed}");
                    self.emit_line(&format!("local {tmp} = {iter_var}()"));
                    if let Some(done) = done_out {
                        self.emit_line(&format!("local {done} = {tmp}[2]"));
                    }
                    if let Some(value) = value_out {
                        self.emit_line(&format!("local {value} = {tmp}[1]"));
                    }
                }
            }

            "checked_add" => {
                // 2-result op: op.var = wrapping sum (results[0]), op.out =
                // overflow flag (results[1]) — the IterNextUnboxed transport
                // convention. Luau supports multi-return destructuring, so no
                // tmp table is needed (unlike iter_next_unboxed's table
                // unpack). The helper returns (a + b, false): f64 addition
                // never overflows i64, so the flag is constant-false and the
                // peel's slow loop is dead on this target by design.
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
                // 2-result op: op.var = wrapping product (results[0]), op.out =
                // overflow/inexactness flag (results[1]) — the IterNextUnboxed
                // transport convention. The helper returns (a * b, flag) where
                // flag is CONSERVATIVE: true whenever the f64 product may have
                // lost precision (|product| >= 2^53), forcing the boxed BigInt
                // slow loop. A structural (a * b, false) here would be a SILENT
                // WRONG ANSWER above 2^53 — see molt_checked_i64_mul.
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
                    self.emit_line(&format!(
                        "local {out} = (type({}) == \"function\")",
                        sanitize_ident(val)
                    ));
                }
            }

            // ================================================================
            // Try/except blocks
            // ================================================================
            "try_start" => {
                // Handled by lower_try_to_pcall — should not reach here.
                self.emit_line("-- [try_start]");
            }
            "try_end" => {
                // Handled by lower_try_to_pcall — should not reach here.
                self.emit_line("-- [try_end]");
            }
            "pcall_wrap_begin" => {
                let n = op.value.unwrap_or(0) as u32;
                self.emit_line(&format!("local __ok_{n}, __err_{n}"));
                self.emit_line(&format!("__ok_{n}, __err_{n} = pcall(function()"));
                self.push_indent();
                self.try_depth_counter.push(n);
                self.inside_pcall_body = true;
            }
            "pcall_wrap_end" => {
                self.pop_indent();
                self.emit_line("end)");
                self.inside_pcall_body = false;
                // Do NOT pop try_depth_counter here — the handler code
                // after pcall_wrap_end needs to reference __err_N via
                // exception_last.
            }
            // ================================================================
            // Slice
            // ================================================================
            "slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    let stop = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = {{table.unpack({obj}, {start} + 1, {stop})}}"
                    ));
                } else if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = {{table.unpack({obj}, {start} + 1)}}"
                    ));
                }
            }

            // ================================================================
            // Enumerate op (distinct from call to enumerate builtin)
            // ================================================================
            "enumerate" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_enumerate({})",
                        sanitize_ident(iterable)
                    ));
                }
            }

            // ================================================================
            // Dict key/value/items ops
            // ================================================================
            "dict_keys" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_keys({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_values" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_values({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_items" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_items({})",
                        sanitize_ident(d)
                    ));
                }
            }

            // ================================================================
            // Phi nodes (SSA merge — no-op in sequential Luau) / nop
            // ================================================================
            "phi" => {}
            "nop" => {}
            "pcall_handler_end" => {
                // Pop pcall counter at the end of the handler dispatch zone.
                if !self.try_depth_counter.is_empty() {
                    self.try_depth_counter.pop();
                }
            }

            // ================================================================
            // Default: unsupported op
            // ================================================================
            _ => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!(
                        "local {out} = nil -- [unsupported op: {}]",
                        op.kind
                    ));
                } else {
                    self.emit_line(&format!("-- [unsupported op: {}]", op.kind));
                }
            }
        }
    }

    // --- helper: binary op ---
}
