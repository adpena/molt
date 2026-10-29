use super::emit_helpers::rust_value;
use crate::OpIR;

#[derive(Clone, Copy)]
pub(super) struct RuntimeValueCall {
    rust_fn: &'static str,
    arity: usize,
}

impl RuntimeValueCall {
    pub(super) fn rhs(self, op: &OpIR) -> Result<String, String> {
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() != self.arity {
            return Err(format!(
                "{} requires {} runtime-surface argument(s), got {}",
                op.kind,
                self.arity,
                args.len()
            ));
        }
        let rendered_args = (0..self.arity)
            .map(|idx| {
                let value = rust_value(&args[idx]);
                format!("&{value}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!("{}({rendered_args})", self.rust_fn))
    }
}

pub(super) fn runtime_value_call_for_kind(kind: molt_ir::OpKind) -> Option<RuntimeValueCall> {
    match kind {
        molt_ir::OpKind::StrFromObj => Some(RuntimeValueCall {
            rust_fn: "molt_str_from_obj",
            arity: 1,
        }),
        molt_ir::OpKind::ReprFromObj => Some(RuntimeValueCall {
            rust_fn: "molt_repr",
            arity: 1,
        }),
        molt_ir::OpKind::AsciiFromObj => Some(RuntimeValueCall {
            rust_fn: "molt_ascii_from_obj",
            arity: 1,
        }),
        _ => None,
    }
}
