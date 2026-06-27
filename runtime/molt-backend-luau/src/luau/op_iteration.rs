use super::*;

impl LuauBackend {
    pub(super) fn emit_iteration_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let it = sanitize_ident(iterable);
                    self.emit_line(&format!(
                        "local {out}; do local _t = {it}; local _i = 0; \
                         {out} = function() _i = _i + 1; \
                         if _i <= #_t then return {{_t[_i], nil}} \
                         else return {{nil, true}} end; end; end"
                    ));
                }
            }
            "iter_next" => {
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
            _ => return false,
        }
        true
    }
}
