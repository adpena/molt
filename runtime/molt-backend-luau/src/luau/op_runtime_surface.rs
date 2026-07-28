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
            "state_yield" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    if let Some(val) = args.first() {
                        self.emit_line(&format!(
                            "local {out} = coroutine.yield({})",
                            sanitize_ident(val)
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = coroutine.yield()"));
                    }
                } else if let Some(val) = args.first() {
                    self.emit_line(&format!("coroutine.yield({})", sanitize_ident(val)));
                } else {
                    self.emit_line("coroutine.yield()");
                }
            }
            "state_switch"
            | "state_transition"
            | "chan_new"
            | "chan_drop"
            | "chan_send_yield"
            | "chan_recv_yield"
            | "cancel_token_new"
            | "cancel_token_clone"
            | "cancel_token_drop"
            | "cancel_token_cancel"
            | "cancel_token_is_cancelled"
            | "cancel_token_set_current"
            | "cancel_token_get_current"
            | "cancelled"
            | "cancel_current"
            | "future_cancel"
            | "future_cancel_msg"
            | "future_cancel_clear"
            | "promise_new"
            | "promise_set_result"
            | "promise_set_exception"
            | "thread_submit"
            | "task_register_token_owned" => {
                self.emit_unsupported_op(op);
            }
            "is_native_awaitable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = false"));
                }
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
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
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
            "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "function_closure_bits" => {
                self.emit_unsupported_op(op);
            }
            "code_slot_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(code) = args.first() {
                    let slot = op.value.unwrap_or(0);
                    self.emit_line(&format!(
                        "molt_code_slots[{slot}] = {}",
                        sanitize_ident(code)
                    ));
                } else {
                    self.emit_unsupported_op(op);
                }
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "code_slots_init" => {
                let count = op.value.unwrap_or(0).max(0);
                self.emit_line(&format!("molt_code_slots = table.create({count})"));
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "frame_locals_set" | "trace_enter_slot" | "trace_exit" | "line" => {
                // These operations are observable through frame locals,
                // traceback positions, and active-frame inspection. Target
                // admission rejects them before source generation; keep the
                // emitter fail-closed too so a future capability flip cannot
                // silently revive the retired no-op lane.
                self.emit_unsupported_op(op);
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
