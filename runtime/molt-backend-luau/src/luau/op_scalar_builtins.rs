use super::*;

impl LuauBackend {
    pub(super) fn emit_scalar_builtin_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Len / type introspection
            // ================================================================
            "len" => {
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
            // Raw int bridge (no-op in Luau; values are already unboxed)
            // ================================================================
            "unbox_to_raw_int" | "box_from_raw_int" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(val)));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            _ => return false,
        }
        true
    }
}
