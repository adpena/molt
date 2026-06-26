use super::*;

impl LuauBackend {
    pub(super) fn emit_tuple_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "tuple_new" | "tuple_from_list" => {
                let out = self.out_var(op);
                if let Some(ref out_name) = op.out {
                    self.tuple_vars.insert(out_name.clone());
                }
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out} = {{{items}}}"));
            }
            "unpack_sequence" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let src = sanitize_ident(&args[0]);
                    for (i, out_name) in args[1..].iter().enumerate() {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {src}[{}]", i + 1));
                    }
                }
            }
            _ => return false,
        }
        true
    }
}
