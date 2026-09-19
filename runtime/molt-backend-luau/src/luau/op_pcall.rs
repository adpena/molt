use super::*;

impl LuauBackend {
    pub(super) fn emit_pcall_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "pcall_wrap_begin" => {
                let n = op.value.expect("operation capture requires an identity");
                for output in op.args.as_deref().unwrap_or(&[]) {
                    let output = sanitize_ident(output);
                    if !self.hoisted_vars.contains(&output) {
                        self.emit_line(&format!("local {output}"));
                    }
                }
                self.emit_line("do");
                self.push_indent();
                self.emit_line(&format!(
                    "local __molt_pcall_frame_context_{n}, __molt_pcall_frame_owner_{n} = molt_frame_context()"
                ));
                self.emit_line(&format!(
                    "local __molt_frame_depth_{n} = __molt_pcall_frame_context_{n}.depth"
                ));
                self.emit_line(&format!(
                    "local __molt_exception_depth_{n} = #__molt_pcall_frame_context_{n}.exceptions.handlers"
                ));
                self.emit_line(&format!(
                    "local __molt_exception_baseline_{n} = __molt_pcall_frame_context_{n}.exceptions.baseline"
                ));
                self.emit_line(&format!("local __ok_{n}, __err_{n}"));
                self.emit_line(&format!("__ok_{n}, __err_{n} = pcall(function()"));
                self.push_indent();
            }
            "pcall_wrap_end" => {
                let n = op.value.expect("operation capture requires an identity");
                self.pop_indent();
                self.emit_line("end)");
                self.emit_line(&format!("if not __ok_{n} then"));
                self.push_indent();
                self.emit_line(&format!(
                    "molt_exception_capture(__molt_pcall_frame_context_{n}, __molt_pcall_frame_owner_{n}, __molt_frame_depth_{n}, __molt_exception_depth_{n}, __molt_exception_baseline_{n}, __err_{n})"
                ));
                self.pop_indent();
                self.emit_line("end");
                self.pop_indent();
                self.emit_line("end");
            }
            _ => return false,
        }
        true
    }
}
