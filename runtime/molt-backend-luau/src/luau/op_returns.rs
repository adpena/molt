use super::*;

impl LuauBackend {
    pub(super) fn emit_return_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
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
            _ => return false,
        }
        true
    }
}
