use super::*;

impl LuauBackend {
    pub(super) fn emit_string_op(&mut self, op: &OpIR) -> bool {
        match op.kind {
            // ================================================================
            // String ops
            // ================================================================
            molt_ir::OpKind::StringJoin => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let sep = sanitize_ident(&args[0]);
                    let list = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = table.concat({list}, {sep})"));
                }
            }
            molt_ir::OpKind::StringFormat => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let fmt_str = sanitize_ident(&args[0]);
                    let fmt_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if fmt_args.is_empty() {
                        self.emit_line(&format!("local {out} = {fmt_str}"));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = string.format({fmt_str}, {fmt_args})"
                        ));
                    }
                }
            }
            molt_ir::OpKind::StringStrip => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^%s*(.-)%s*$\"))"));
                }
            }
            molt_ir::OpKind::StringLstrip => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^%s*(.+)\") or \"\")"));
                }
            }
            molt_ir::OpKind::StringRstrip => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^(.-)%s*$\"))"));
                }
            }
            molt_ir::OpKind::StringUpper => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.upper({})",
                        sanitize_ident(s)
                    ));
                }
            }
            molt_ir::OpKind::StringLower => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.lower({})",
                        sanitize_ident(s)
                    ));
                }
            }
            molt_ir::OpKind::StringStartswith | molt_ir::OpKind::StringStartswithSlice => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let prefix = sanitize_ident(&args[1]);
                    let prefix_is_tuple = self.tuple_vars.contains(&args[1]);
                    if prefix_is_tuple && args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = false; for __i = 1, #{prefix} do local __cand = {prefix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for startswith must only contain str\"}}) end; if __cand == \"\" then if __start_raw <= __n and __start <= __end then {out} = true; break end elseif string.sub(__slice, 1, #__cand) == __cand then {out} = true; break end end end"
                        ));
                    } else if prefix_is_tuple {
                        self.emit_line(&format!(
                            "local {out} = false; for __i = 1, #{prefix} do local __cand = {prefix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for startswith must only contain str\"}}) end; if __cand == \"\" or string.sub({s}, 1, #__cand) == __cand then {out} = true; break end end"
                        ));
                    } else if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = if {prefix} == \"\" then (__start_raw <= __n and __start <= __end) else (string.sub(__slice, 1, #{prefix}) == {prefix}) end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = ({prefix} == \"\" or string.sub({s}, 1, #{prefix}) == {prefix})"
                        ));
                    }
                }
            }
            molt_ir::OpKind::StringEndswith | molt_ir::OpKind::StringEndswithSlice => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let suffix = sanitize_ident(&args[1]);
                    let suffix_is_tuple = self.tuple_vars.contains(&args[1]);
                    if suffix_is_tuple && args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = false; for __i = 1, #{suffix} do local __cand = {suffix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for endswith must only contain str\"}}) end; if __cand == \"\" then if __start_raw <= __n and __start <= __end then {out} = true; break end elseif string.sub(__slice, -#__cand) == __cand then {out} = true; break end end end"
                        ));
                    } else if suffix_is_tuple {
                        self.emit_line(&format!(
                            "local {out} = false; for __i = 1, #{suffix} do local __cand = {suffix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for endswith must only contain str\"}}) end; if __cand == \"\" or string.sub({s}, -#__cand) == __cand then {out} = true; break end end"
                        ));
                    } else if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = if {suffix} == \"\" then (__start_raw <= __n and __start <= __end) else (string.sub(__slice, -#{suffix}) == {suffix}) end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = ({suffix} == \"\" or string.sub({s}, -#{suffix}) == {suffix})"
                        ));
                    }
                }
            }
            molt_ir::OpKind::StringReplace => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let old = sanitize_ident(&args[1]);
                    let new_val = sanitize_ident(&args[2]);
                    // Escape Lua pattern magic characters in search string so gsub
                    // does literal matching. Also escape % in replacement string
                    // since gsub interprets %0, %1, etc. as capture references.
                    if args.len() >= 4 {
                        let count = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __pattern = {old}:gsub(\"[%(%)%.%%%+%-%*%?%[%]%^%$]\", \"%%%0\"); local __replacement = ({new_val}):gsub(\"%%\", \"%%%%\"); if {count} >= 0 then {out} = (string.gsub({s}, __pattern, __replacement, {count})) else {out} = (string.gsub({s}, __pattern, __replacement)) end end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = (string.gsub({s}, \
                             {old}:gsub(\"[%(%)%.%%%+%-%*%?%[%]%^%$]\", \"%%%0\"), \
                             ({new_val}):gsub(\"%%\", \"%%%%\")))"
                        ));
                    }
                }
            }
            molt_ir::OpKind::StringFind
            | molt_ir::OpKind::StringFindSlice
            | molt_ir::OpKind::StringIndex
            | molt_ir::OpKind::StringIndexSlice => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    let needs_error = matches!(
                        op.kind,
                        molt_ir::OpKind::StringIndex | molt_ir::OpKind::StringIndexSlice
                    );
                    let error_guard = if needs_error {
                        format!(
                            "; if {out} == -1 then error({{__type=\"ValueError\", __msg=\"substring not found\"}}) end"
                        )
                    } else {
                        String::new()
                    };
                    if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; if {sub} == \"\" then {out} = if __start_raw <= __n and __start <= __end then __start else -1 else local __found = string.find({s}, {sub}, __start + 1, true); if __found and __found <= __end then {out} = __found - 1 else {out} = -1 end end{error_guard} end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = (string.find({s}, {sub}, 1, true) or 0) - 1{error_guard}"
                        ));
                    }
                }
            }
            molt_ir::OpKind::StringRfind
            | molt_ir::OpKind::StringRfindSlice
            | molt_ir::OpKind::StringRindex
            | molt_ir::OpKind::StringRindexSlice => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    let needs_error = matches!(
                        op.kind,
                        molt_ir::OpKind::StringRindex | molt_ir::OpKind::StringRindexSlice
                    );
                    let error_guard = if needs_error {
                        format!(
                            "; if {out} == -1 then error({{__type=\"ValueError\", __msg=\"substring not found\"}}) end"
                        )
                    } else {
                        String::new()
                    };
                    let bounds = if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        format!(
                            "local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end;"
                        )
                    } else {
                        format!(
                            "local __n = #{s}; local __start_raw = 0; local __start = 0; local __end = __n;"
                        )
                    };
                    self.emit_line(&format!(
                        "local {out}; do {bounds} if {sub} == \"\" then {out} = if __start_raw <= __n and __start <= __end then __end else -1 else local __last = -1; local __pos = __start + 1; while true do local __found = string.find({s}, {sub}, __pos, true); if not __found or __found > __end then break end; __last = __found - 1; __pos = __found + 1 end; {out} = __last end{error_guard} end"
                    ));
                }
            }
            molt_ir::OpKind::StringCount | molt_ir::OpKind::StringCountSlice => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    let source_expr = if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        format!(
                            "do local __n = #{s}; local __start = if {start} < 0 then __n + {start} else {start}; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __src = string.sub({s}, __start + 1, __end);"
                        )
                    } else {
                        format!("do local __src = {s};")
                    };
                    self.emit_line(&format!(
                        "local {out}; {source_expr} local __sub = {sub}; if __sub == \"\" then {out} = #__src + 1 else local __count = 0; local __pos = 1; while true do local __i, __j = string.find(__src, __sub, __pos, true); if not __i then break end; __count += 1; __pos = __j + 1 end; {out} = __count end end"
                    ));
                }
            }
            molt_ir::OpKind::StringPartition => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out}; do if {sep} == \"\" then error({{__type=\"ValueError\", __msg=\"empty separator\"}}) end; local __i, __j = string.find({s}, {sep}, 1, true); if __i then {out} = {{string.sub({s}, 1, __i - 1), {sep}, string.sub({s}, __j + 1)}} else {out} = {{{s}, \"\", \"\"}} end end"
                    ));
                    if let Some(ref out_name) = op.out {
                        self.tuple_vars.insert(out_name.clone());
                    }
                }
            }
            molt_ir::OpKind::StringRpartition => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out}; do if {sep} == \"\" then error({{__type=\"ValueError\", __msg=\"empty separator\"}}) end; local __last_i, __last_j = nil, nil; local __pos = 1; while true do local __i, __j = string.find({s}, {sep}, __pos, true); if not __i then break end; __last_i, __last_j = __i, __j; __pos = __i + 1 end; if __last_i then {out} = {{string.sub({s}, 1, __last_i - 1), {sep}, string.sub({s}, __last_j + 1)}} else {out} = {{\"\", \"\", {s}}} end end"
                    ));
                    if let Some(ref out_name) = op.out {
                        self.tuple_vars.insert(out_name.clone());
                    }
                }
            }
            molt_ir::OpKind::StringSplitlines => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    let keep = args
                        .get(1)
                        .map(|arg| sanitize_ident(arg))
                        .unwrap_or_else(|| "false".to_string());
                    self.emit_line(&format!(
                        "local {out}; do local __keep = {keep}; local __lines = {{}}; local __n = 0; local __line_start = 1; local __i = 1; while __i <= #{s} do local __c = string.sub({s}, __i, __i); if __c == \"\\n\" or __c == \"\\r\" then local __line_end = __i - 1; local __next = __i + 1; if __c == \"\\r\" and __next <= #{s} and string.sub({s}, __next, __next) == \"\\n\" then __next += 1 end; __n += 1; if __keep then __lines[__n] = string.sub({s}, __line_start, __next - 1) else __lines[__n] = string.sub({s}, __line_start, __line_end) end; __line_start = __next; __i = __next else __i += 1 end end; if __line_start <= #{s} then __n += 1; __lines[__n] = string.sub({s}, __line_start) end; {out} = __lines end"
                    ));
                }
            }
            molt_ir::OpKind::StringSplit => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    if args.len() >= 2 {
                        let sep = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = molt_string.split({s}, {sep})"));
                    } else {
                        // Python str.split() with no args splits on any
                        // whitespace and strips leading/trailing.  The
                        // molt_string.split helper handles sep==nil correctly
                        // using %s+ pattern matching.
                        self.emit_line(&format!("local {out} = molt_string.split({s})"));
                    }
                }
            }
            molt_ir::OpKind::StringSplitValidate => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_validate({s}, {sep})"
                    ));
                }
            }
            molt_ir::OpKind::StringSplitField => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    let idx = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_field({s}, {sep}, {idx})"
                    ));
                }
            }
            molt_ir::OpKind::StringSplitFieldLen => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    let idx = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_field_len({s}, {sep}, {idx})"
                    ));
                }
            }
            molt_ir::OpKind::StringSplitFieldEq => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 4 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    let idx = sanitize_ident(&args[2]);
                    let expected = sanitize_ident(&args[3]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_field_eq({s}, {sep}, {idx}, {expected})"
                    ));
                }
            }
            molt_ir::OpKind::StringConcat => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let a = sanitize_ident(&args[0]);
                    let b = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = {a} .. {b}"));
                }
            }
            molt_ir::OpKind::StringRepeat => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let n = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = string.rep({s}, {n})"));
                }
            }
            // A fused split/count loop stands in for its loop only when it runs no
            // Python code and reads words as `str.split` does; this target
            // cannot check either, so the op declines, `(None, False)`, and the
            // ordinary loop lowered beside it runs.
            molt_ir::OpKind::StringSplitWsDictInc | molt_ir::OpKind::StringSplitSepDictInc => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{nil, false}}"));
                if let Some(ref out_name) = op.out {
                    self.tuple_vars.insert(out_name.clone());
                }
            }

            _ => return false,
        }
        true
    }
}
