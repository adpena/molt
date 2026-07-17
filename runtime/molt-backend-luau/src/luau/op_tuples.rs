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
                self.emit_line(&format!("local {out} = molt_pack_tuple({items})"));
            }
            "unpack_sequence" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src_name) = args.first() {
                    let expected = args.len() - 1;
                    let src = sanitize_ident(src_name);
                    let shape = match self.scalar_plan.name_container_kind(src_name) {
                        Some(ContainerKind::List | ContainerKind::Tuple) => "sequence",
                        Some(ContainerKind::Dict | ContainerKind::Set) => "mapping",
                        Some(ContainerKind::Str) => "string",
                        None => "auto",
                    };
                    let unpacked = format!("__molt_unpacked_{}", self.temp_counter);
                    self.temp_counter += 1;
                    self.emit_line(&format!(
                        "local {unpacked} = molt_unpack_sequence({src}, {expected}, \"{shape}\")"
                    ));
                    for (i, out_name) in args[1..].iter().enumerate() {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {unpacked}[{}]", i + 1));
                    }
                }
            }
            _ => return false,
        }
        true
    }
}
