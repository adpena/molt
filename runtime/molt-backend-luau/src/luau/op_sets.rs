use super::*;

impl LuauBackend {
    pub(super) fn emit_set_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "set_new" | "frozenset_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let kind = if op.kind == "frozenset_new" {
                    "frozenset"
                } else {
                    "set"
                };
                self.emit_line(&format!("local {out} = molt_set_new(\"{kind}\")"));
                for value in args {
                    self.emit_line(&format!("molt_set_add({out}, {})", sanitize_ident(value)));
                }
                if kind == "frozenset" {
                    self.emit_line(&format!("molt_set_freeze({out})"));
                }
            }
            "set_clear" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    let tbl = sanitize_ident(tbl);
                    self.emit_line(&format!("molt_set_clear({tbl})"));
                }
            }
            "set_add" | "set_add_probe" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_set_add({set}, {val})"));
                }
            }
            "frozenset_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_frozenset_build_add({set}, {val})"));
                }
            }
            "set_discard" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_set_discard({set}, {val}, true)"));
                }
            }
            "set_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_set_discard({set}, {val}, false)"));
                }
            }
            "set_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(set) = args.first() {
                    let set = sanitize_ident(set);
                    self.emit_line(&format!("local {out} = molt_set_pop({set})"));
                }
            }
            "set_update" => {
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
