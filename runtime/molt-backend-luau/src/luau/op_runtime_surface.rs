use super::*;

impl LuauBackend {
    pub(super) fn emit_runtime_surface_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "context_null" | "context_enter" | "context_exit" | "context_closing"
            | "context_unwind" | "context_unwind_to" => {
                self.emit_unsupported_op(op);
            }
            "context_depth" => {
                self.emit_unsupported_op(op);
            }
            "file_open" | "file_read" | "file_write" | "file_close" | "file_flush" => {
                self.emit_unsupported_op(op);
            }
            "getargv" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }
            "sys_executable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = \"\""));
                }
            }
            "getframe" => {
                self.emit_unsupported_op_with_reason(
                    op,
                    "Python-visible frame objects require exact whole-program locals activation",
                );
            }
            "bridge_unavailable" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let msg = args
                    .first()
                    .map(|arg| {
                        format!(
                            "\"Molt bridge unavailable: \" .. tostring({})",
                            sanitize_ident(arg)
                        )
                    })
                    .unwrap_or_else(|| "\"Molt bridge unavailable\"".to_string());
                let diagnostic = format!("{{__type=\"RuntimeError\", __msg={msg}}}");
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out}: any = error({diagnostic})"));
                } else {
                    self.emit_line(&format!("error({diagnostic})"));
                }
            }
            "function_closure_bits" => {
                self.emit_unsupported_op(op);
            }
            "code_slot_set" => {
                let args = op.args.as_deref().expect("admitted code_slot_set operands");
                let slot = op.value.expect("admitted code_slot_set ID");
                self.emit_line(&format!(
                    "molt_code_slots[{slot}] = molt_frame_bind_code({slot}, {}, {})",
                    sanitize_ident(&args[0]),
                    sanitize_ident(&args[1]),
                ));
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "code_slots_init" => {
                let count = op.value.expect("admitted code_slots_init count");
                self.emit_line(&format!("molt_code_slots = table.create({count})"));
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "trace_enter_slot" => {
                let code_id = op.value.expect("admitted trace_enter_slot ID");
                self.emit_line(&format!(
                    "local __molt_frame_context, __molt_frame_depth, __molt_frame_code, __molt_frame_owner = molt_frame_enter_slot(molt_code_slots[{code_id}])"
                ));
            }
            "trace_exit" => {
                self.emit_line(
                    "molt_frame_exit(__molt_frame_context, __molt_frame_depth, __molt_frame_code, __molt_frame_owner)",
                );
            }
            "frame_locals_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(locals) = args.first() {
                    let context = self.frame_context_expr();
                    self.emit_line(&format!(
                        "molt_frame_locals_set({context}, {})",
                        sanitize_ident(locals)
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            "line" => {
                let context = self.frame_context_expr();
                let line = op.value.unwrap_or(0);
                let col = op
                    .col_offset
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "nil".to_string());
                let end_col = op
                    .end_col_offset
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "nil".to_string());
                self.emit_line(&format!(
                    "molt_frame_set_line({context}, {line}, {col}, {end_col})"
                ));
            }
            "json_parse" | "msgpack_parse" | "cbor_parse" => {
                self.emit_unsupported_op(op);
            }
            "invoke_ffi" => {
                let diagnostic =
                    "{__type=\"RuntimeError\", __msg=\"Luau target does not support FFI\"}";
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out}: any = error({diagnostic})"));
                } else {
                    self.emit_line(&format!("error({diagnostic})"));
                }
            }
            "memoryview_new" | "memoryview_tobytes" | "memoryview_cast" | "complex_from_obj" => {
                self.emit_unsupported_op(op);
            }
            "bytearray_fill_range" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 4 {
                    let bytearray = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    let stop = sanitize_ident(&args[2]);
                    let value = sanitize_ident(&args[3]);
                    self.emit_line(&format!(
                        "do local __meta = molt_binary_metadata[{bytearray}]; if __meta == nil or __meta.kind ~= \"bytearray\" then error({{__type=\"TypeError\", __msg=\"bytearray operation requires bytearray\"}}) end; local __ba = __meta.value; local __start = {start}; local __stop = {stop}; local __byte = {value}; if __byte < 0 or __byte > 255 then error({{__type=\"ValueError\", __msg=\"byte must be in range(0, 256)\"}}) end; if __start < 0 or __stop < __start or __stop > #__ba then error({{__type=\"IndexError\", __msg=\"bytearray fill range out of range\"}}) end; __meta.value = string.sub(__ba, 1, __start) .. string.rep(string.char(__byte), __stop - __start) .. string.sub(__ba, __stop + 1) end"
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
            }
            _ => return false,
        }
        true
    }
}
