use super::*;

impl LuauBackend {
    pub(super) fn emit_exception_op(&mut self, op: &OpIR) -> bool {
        match op.kind {
            molt_ir::OpKind::TryStart | molt_ir::OpKind::TryEnd => {}
            molt_ir::OpKind::ExceptionPush
            | molt_ir::OpKind::ExceptionPop
            | molt_ir::OpKind::ExceptionStackClear
            | molt_ir::OpKind::ExceptionStackEnter
            | molt_ir::OpKind::ExceptionStackExit
            | molt_ir::OpKind::ExceptionStackDepth
            | molt_ir::OpKind::ExceptionStackSetDepth
            | molt_ir::OpKind::ExceptionContextSet
            | molt_ir::OpKind::ExceptionClear
            | molt_ir::OpKind::ExceptionSetLast
            | molt_ir::OpKind::ExceptionLast
            | molt_ir::OpKind::ExceptionLastPending
            | molt_ir::OpKind::ExceptionActive
            | molt_ir::OpKind::ExceptionCurrent
            | molt_ir::OpKind::ExceptionPending => {
                let args = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|arg| sanitize_ident(arg))
                    .collect::<Vec<_>>()
                    .join(", ");
                let call = format!("molt_{}({args})", op.kind);
                if let Some(out) = op.out.as_deref().filter(|out| *out != "none") {
                    self.emit_line(&format!("local {} = {call}", sanitize_ident(out)));
                } else {
                    self.emit_line(&call);
                }
            }
            molt_ir::OpKind::DropInserted
            | molt_ir::OpKind::ExceptionRegionDropsInserted
            | molt_ir::OpKind::IncRef
            | molt_ir::OpKind::DecRef
            | molt_ir::OpKind::Release => {
                // Luau is GC-managed, so shared RC/drop markers are consumed no-ops.
            }
            molt_ir::OpKind::ExceptionNew
            | molt_ir::OpKind::ExceptionNewBuiltin
            | molt_ir::OpKind::ExceptionNewFromClass => {
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
            molt_ir::OpKind::ExceptionNewBuiltinEmpty => {
                let out = self.out_var(op);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = \"\"}}"
                ));
            }
            molt_ir::OpKind::ExceptionNewBuiltinOne => {
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
            molt_ir::OpKind::Raise => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("molt_exception_set_last({})", sanitize_ident(val)));
                } else {
                    self.emit_line("molt_exception_reraise()");
                }
            }
            molt_ir::OpKind::CheckException => {
                self.emit_unsupported_op_with_reason(
                    op,
                    "exception observer must use the canonical logical-flow emitter",
                );
            }
            molt_ir::OpKind::LoopBreakIfException => {
                self.emit_line("if molt_exception_pending() then break end");
            }
            molt_ir::OpKind::ExceptionFinallyPendingObserver => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_exception_last_pending()"));
            }
            molt_ir::OpKind::ExceptionMatchBuiltin => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc) = args.first() {
                    let class_name = op.s_value.as_deref().unwrap_or("Exception");
                    self.emit_line(&format!(
                        "local {out} = molt_exception_match({}, \"{class_name}\")",
                        sanitize_ident(exc)
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            molt_ir::OpKind::ExceptionClass => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class_var) = args.first() {
                    let cls = sanitize_ident(class_var);
                    self.emit_line(&format!("local {out} = {cls}"));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            molt_ir::OpKind::ExceptiongroupMatch | molt_ir::OpKind::ExceptiongroupCombine => {
                self.emit_unsupported_op(op);
            }
            _ => return false,
        }
        true
    }
}
