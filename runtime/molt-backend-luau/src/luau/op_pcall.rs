use super::*;

impl LuauBackend {
    pub(super) fn emit_pcall_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "pcall_failure_jump" => {
                let pcall_id = op
                    .s_value
                    .as_deref()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                if let Some(id) = op.value {
                    self.emit_line(&format!("if not __ok_{pcall_id} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if not __ok_{pcall_id} then goto {target} end"));
                }
            }
            "try_start" => {
                self.emit_line("-- [try_start]");
            }
            "try_end" => {
                self.emit_line("-- [try_end]");
            }
            "pcall_wrap_begin" => {
                let n = op.value.unwrap_or(0) as u32;
                self.emit_line(&format!("local __ok_{n}, __err_{n}"));
                self.emit_line(&format!("__ok_{n}, __err_{n} = pcall(function()"));
                self.push_indent();
                self.try_depth_counter.push(n);
                self.inside_pcall_body = true;
            }
            "pcall_wrap_end" => {
                self.pop_indent();
                self.emit_line("end)");
                self.inside_pcall_body = false;
            }
            "pcall_handler_end" => {
                if !self.try_depth_counter.is_empty() {
                    self.try_depth_counter.pop();
                }
            }
            _ => return false,
        }
        true
    }
}
