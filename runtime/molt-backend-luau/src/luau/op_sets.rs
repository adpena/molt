use super::*;

impl LuauBackend {
    pub(super) fn emit_set_op(&mut self, op: &OpIR) -> bool {
        match op.kind {
            molt_ir::OpKind::SetNew | molt_ir::OpKind::FrozensetNew => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let frozen = op.kind == molt_ir::OpKind::FrozensetNew;
                let kind = if frozen { "frozenset" } else { "set" };
                self.emit_line(&format!("local {out} = molt_set_new(\"{kind}\")"));
                for value in args {
                    self.emit_line(&format!("molt_set_add({out}, {})", sanitize_ident(value)));
                }
                if frozen {
                    self.emit_line(&format!("molt_set_freeze({out})"));
                }
            }
            molt_ir::OpKind::SetClear => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    let tbl = sanitize_ident(tbl);
                    self.emit_line(&format!("molt_set_clear({tbl})"));
                }
            }
            molt_ir::OpKind::SetAdd | molt_ir::OpKind::SetAddProbe => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_set_add({set}, {val})"));
                }
            }
            molt_ir::OpKind::FrozensetAdd => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_frozenset_build_add({set}, {val})"));
                }
            }
            molt_ir::OpKind::SetDiscard => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_set_discard({set}, {val}, true)"));
                }
            }
            molt_ir::OpKind::SetRemove => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_set_discard({set}, {val}, false)"));
                }
            }
            molt_ir::OpKind::SetPop => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(set) = args.first() {
                    let set = sanitize_ident(set);
                    self.emit_line(&format!("local {out} = molt_set_pop({set})"));
                }
            }
            molt_ir::OpKind::SetUpdate => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_set_update({set}, {other})"));
                }
            }
            _ => return false,
        }
        true
    }
}
