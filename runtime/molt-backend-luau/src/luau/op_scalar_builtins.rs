use super::*;

impl LuauBackend {
    pub(super) fn emit_scalar_builtin_op(&mut self, op: &OpIR) -> bool {
        match op.kind {
            // ================================================================
            // Len / type introspection
            // ================================================================
            molt_ir::OpKind::Len => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj_s = sanitize_ident(obj);
                    let container_kind = self.scalar_plan.name_container_kind(obj);
                    if matches!(
                        container_kind,
                        Some(ContainerKind::List | ContainerKind::Tuple)
                    ) {
                        self.emit_line(&format!("local {out} = molt_sequence_len({obj_s})"));
                    } else if container_kind == Some(ContainerKind::Dict) {
                        self.emit_line(&format!("local {out} = molt_dict_len({obj_s})"));
                    } else {
                        self.emit_line(&format!("local {out} = molt_len({obj_s})"));
                    }
                }
            }
            molt_ir::OpKind::Id => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(obj)));
                }
            }
            molt_ir::OpKind::Ord => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_ord({})", sanitize_ident(val)));
                }
            }
            molt_ir::OpKind::OrdAt => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_ord_at({container}, {key})"));
                }
            }
            molt_ir::OpKind::Chr => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.char({})",
                        sanitize_ident(val)
                    ));
                }
            }
            molt_ir::OpKind::TypeOf => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_type_of({})",
                        sanitize_ident(obj)
                    ));
                }
            }
            molt_ir::OpKind::Isinstance | molt_ir::OpKind::Issubclass => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let cls = sanitize_ident(&args[1]);
                    let helper = if op.kind == molt_ir::OpKind::Isinstance {
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
            molt_ir::OpKind::IntFromObj => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_int({})", sanitize_ident(val)));
                }
            }
            molt_ir::OpKind::IntFromStrOfObj => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let val = sanitize_ident(&args[0]);
                    let base = sanitize_ident(&args[1]);
                    let has_base = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = if molt_bool({has_base}) then (function() local __value = tonumber(molt_str({val}), molt_int({base})); if __value == nil then error({{__type=\"ValueError\", __msg=\"invalid literal for int()\"}}) end; return math.floor(__value) end)() else molt_int(molt_str({val}))"
                    ));
                }
            }
            molt_ir::OpKind::FloatFromObj => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_float({})",
                        sanitize_ident(val)
                    ));
                }
            }
            molt_ir::OpKind::StrFromObj | molt_ir::OpKind::AsciiFromObj => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_str({})", sanitize_ident(val)));
                }
            }
            molt_ir::OpKind::ReprFromObj => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_repr({})", sanitize_ident(val)));
                }
            }
            molt_ir::OpKind::BytesFromObj | molt_ir::OpKind::BytesFromStr => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_binary_new(\"bytes\", tostring({}))",
                        sanitize_ident(val)
                    ));
                }
            }
            molt_ir::OpKind::BytearrayFromStr => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_binary_new(\"bytearray\", tostring({}))",
                        sanitize_ident(val)
                    ));
                }
            }
            molt_ir::OpKind::BytearrayFromObj => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let val = sanitize_ident(val);
                    self.emit_line(&format!("local {out} = molt_binary_new(\"bytearray\", if type({val}) == \"number\" then string.rep(string.char(0), math.max(0, {val})) else tostring({val}))"));
                }
            }

            // ================================================================
            // Print
            // ================================================================
            molt_ir::OpKind::Print => {
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
            molt_ir::OpKind::PrintNewline => {
                self.emit_line("molt_print()");
            }

            // ================================================================
            // Representation bridge (Luau already uses VM value carriers).
            // ================================================================
            molt_ir::OpKind::Box
            | molt_ir::OpKind::BoxFromRawInt
            | molt_ir::OpKind::Unbox
            | molt_ir::OpKind::UnboxToRawInt => {
                let Some([source]) = op.args.as_deref() else {
                    self.emit_unsupported_op_with_reason(
                        op,
                        "representation conversion requires exactly one operand",
                    );
                    return true;
                };
                if let Some(output) = molt_tir::tir::simple_def_use::simple_ir_out_result(op) {
                    if self.tuple_vars.contains(source) {
                        self.tuple_vars.insert(output.to_string());
                    }
                    let value = if source == "none" {
                        "nil".to_string()
                    } else {
                        sanitize_ident(source)
                    };
                    self.emit_line(&format!("local {} = {value}", sanitize_ident(output)));
                }
            }
            _ => return false,
        }
        true
    }
}
