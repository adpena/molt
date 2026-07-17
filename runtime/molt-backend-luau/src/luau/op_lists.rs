use super::*;

impl LuauBackend {
    pub(super) fn emit_typed_list_method_call(&mut self, op: &OpIR) -> bool {
        if op.kind != "call_method" {
            return false;
        }
        let args = op.args.as_deref().unwrap_or(&[]);
        let Some(list_raw) = args.first() else {
            return false;
        };
        if !self.plan_knows_list(list_raw) {
            return false;
        }

        let list = sanitize_ident(list_raw);
        match op.s_value.as_deref().unwrap_or("unknown") {
            "append" => {
                if let Some(val) = args.get(1) {
                    let val = sanitize_ident(val);
                    self.emit_list_append_raw(&list, &val);
                }
            }
            "pop" => {
                let idx = args.get(1).map(|value| sanitize_ident(value));
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_list_pop(&list, idx.as_deref(), Some(&out));
                } else {
                    self.emit_list_pop(&list, idx.as_deref(), None);
                }
            }
            "insert" => {
                if args.len() >= 3 {
                    let idx = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_list_insert(&list, &idx, &val);
                }
            }
            "remove" => {
                if let Some(val) = args.get(1) {
                    let val = sanitize_ident(val);
                    self.emit_list_remove_value(&list, &val);
                }
            }
            "sort" => {
                self.emit_line(&format!("table.sort({list})"));
            }
            "reverse" => {
                self.emit_list_reverse_in_place(&list);
            }
            "clear" => {
                self.emit_list_clear(&list);
            }
            "copy" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_list_copy_into(&out, &list);
                }
            }
            "extend" => {
                if let Some(other) = args.get(1) {
                    let other = sanitize_ident(other);
                    self.emit_list_extend_raw(&list, &other);
                }
            }
            "count" => {
                if let (Some(out_name), Some(val)) = (&op.out, args.get(1)) {
                    let out = sanitize_ident(out_name);
                    let val = sanitize_ident(val);
                    self.emit_list_count_into(&out, &list, &val);
                }
            }
            "index" => {
                if args.len() >= 3 {
                    let out = self.out_var(op);
                    let val = sanitize_ident(&args[1]);
                    let start = sanitize_ident(&args[2]);
                    let stop = args.get(3).map(|arg| sanitize_ident(arg));
                    self.emit_list_index_range_into(
                        &out,
                        &list,
                        &val,
                        Some(&start),
                        stop.as_deref(),
                    );
                } else if let Some(val) = args.get(1) {
                    let out = self.out_var(op);
                    let val = sanitize_ident(val);
                    self.emit_list_index_into(&out, &list, &val);
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn emit_list_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            "build_list" | "list_new" => {
                let out = self.out_var(op);
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out}: {{any}} = molt_pack_list({items})"));
            }
            "list_fill_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let count = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "0".to_string());
                let fill = args
                    .get(1)
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "nil".to_string());
                self.emit_line(&format!("local {out}: {{any}} = molt_pack_list()"));
                self.emit_line(&format!("for __i = 1, math.max(0, {count}) do"));
                self.indent += 1;
                self.emit_line(&format!("{out}[__i] = {fill}"));
                self.indent -= 1;
                self.emit_line("end");
                self.emit_line(&format!(
                    "rawset({out}, molt_sequence_length_key, math.max(0, {count}))"
                ));
            }
            "range_new" | "list_from_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                match args.len() {
                    1 => {
                        let stop = sanitize_ident(&args[0]);
                        self.emit_line(&format!("local {out} = molt_range(0, {stop})"));
                    }
                    2 => {
                        let start = sanitize_ident(&args[0]);
                        let stop = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = molt_range({start}, {stop})"));
                    }
                    _ => {
                        let start = sanitize_ident(&args[0]);
                        let stop = sanitize_ident(&args[1]);
                        let step = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = molt_range({start}, {stop}, {step})"
                        ));
                    }
                }
            }
            "list_append" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_list_append_raw(&list, &val);
                }
            }
            "list_pop" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    let list = sanitize_ident(list);
                    let idx = args.get(1).map(|s| sanitize_ident(s));
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_list_pop(&list, idx.as_deref(), Some(&out));
                    } else {
                        self.emit_list_pop(&list, idx.as_deref(), None);
                    }
                }
            }
            "list_extend" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_list_extend_raw(&list, &other);
                }
            }
            "list_insert" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let list = sanitize_ident(&args[0]);
                    let idx = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_list_insert(&list, &idx, &val);
                }
            }
            "list_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_list_remove_value(&list, &val);
                }
            }
            "list_clear" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    self.emit_list_clear(&sanitize_ident(list));
                }
            }
            "list_copy" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    self.emit_list_copy_into(&out, &sanitize_ident(src));
                }
            }
            "list_reverse" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    let list = sanitize_ident(list);
                    self.emit_list_reverse_in_place(&list);
                }
            }
            "list_count" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_list_count_into(&out, &list, &val);
                }
            }
            "list_index" | "list_index_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        let start = sanitize_ident(&args[2]);
                        let stop = args.get(3).map(|arg| sanitize_ident(arg));
                        self.emit_list_index_range_into(
                            &out,
                            &list,
                            &val,
                            Some(&start),
                            stop.as_deref(),
                        );
                    } else {
                        self.emit_list_index_into(&out, &list, &val);
                    }
                }
            }
            "list_repeat_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let val = sanitize_ident(&args[0]);
                    let count = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_pack_list(); do local __n = math.max(0, {count}); rawset({out}, molt_sequence_length_key, __n); for __i = 1, __n do rawset({out}, __i, {val}) end end"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = molt_pack_list()"));
                }
            }
            _ => return false,
        }
        true
    }

    fn emit_list_append_raw(&mut self, list: &str, val: &str) {
        // rawset bypasses metamethods for plain list tables and avoids
        // __newindex overhead in Luau's native codegen.
        self.emit_line(&format!(
            "if type(rawget({list}, molt_sequence_length_key)) == \"number\" then local __n = molt_sequence_len({list}); rawset({list}, __n + 1, {val}); rawset({list}, molt_sequence_length_key, __n + 1) else rawset({list}, #{list} + 1, {val}) end"
        ));
    }

    fn emit_list_extend_raw(&mut self, list: &str, other: &str) {
        self.emit_line(&format!(
            "if type(rawget({list}, molt_sequence_length_key)) ~= \"number\" and type(rawget({other}, molt_sequence_length_key)) ~= \"number\" then table.move({other}, 1, #{other}, #{list} + 1, {list}) else local __dst_n = molt_sequence_len({list}); local __src_n = molt_sequence_len({other}); for __i = 1, __src_n do rawset({list}, __dst_n + __i, rawget({other}, __i)) end; rawset({list}, molt_sequence_length_key, __dst_n + __src_n) end"
        ));
    }

    fn emit_list_remove_value(&mut self, list: &str, val: &str) {
        self.emit_line(&format!(
            "do local __n = molt_sequence_len({list}); local __found = false; for __i = 1, __n do if rawget({list}, __i) == {val} then for __j = __i, __n - 1 do rawset({list}, __j, rawget({list}, __j + 1)) end; rawset({list}, __n, nil); if type(rawget({list}, molt_sequence_length_key)) == \"number\" then rawset({list}, molt_sequence_length_key, __n - 1) end; __found = true; break end end; if not __found then error(\"ValueError: list.remove(x): x not in list\") end end"
        ));
    }

    fn emit_list_clear(&mut self, list: &str) {
        self.emit_line(&format!(
            "do local __packed = type(rawget({list}, molt_sequence_length_key)) == \"number\"; table.clear({list}); if __packed then rawset({list}, molt_sequence_length_key, 0) end end"
        ));
    }

    fn emit_list_copy_into(&mut self, out: &str, list: &str) {
        self.emit_line(&format!("local {out} = table.clone({list})"));
    }

    fn emit_list_reverse_in_place(&mut self, list: &str) {
        self.emit_line(&format!(
            "do local __n = molt_sequence_len({list}); for __i = 1, math_floor(__n / 2) do local __j = __n - __i + 1; local __left = rawget({list}, __i); rawset({list}, __i, rawget({list}, __j)); rawset({list}, __j, __left) end end"
        ));
    }

    fn emit_list_count_into(&mut self, out: &str, list: &str, val: &str) {
        self.emit_line(&format!(
            "local {out} = 0; if type(rawget({list}, molt_sequence_length_key)) == \"number\" then for __i = 1, molt_sequence_len({list}) do if rawget({list}, __i) == {val} then {out} = {out} + 1 end end else for _, __v in ipairs({list}) do if __v == {val} then {out} = {out} + 1 end end end"
        ));
    }

    fn emit_list_index_into(&mut self, out: &str, list: &str, val: &str) {
        self.emit_line(&format!(
            "local {out} = -1; do local __found = false; if type(rawget({list}, molt_sequence_length_key)) == \"number\" then for __i = 1, molt_sequence_len({list}) do if rawget({list}, __i) == {val} then {out} = __i - 1; __found = true; break end end else for __i, __v in ipairs({list}) do if __v == {val} then {out} = __i - 1; __found = true; break end end end; if not __found then error(\"ValueError: \" .. tostring({val}) .. \" is not in list\") end end"
        ));
    }

    fn emit_list_index_range_into(
        &mut self,
        out: &str,
        list: &str,
        val: &str,
        start: Option<&str>,
        stop: Option<&str>,
    ) {
        let start_init = if let Some(start) = start {
            format!(
                "local __raw_start = {start}; local __start; if __raw_start == molt_missing_sentinel then __start = 0 elseif __raw_start < 0 then __start = __n + __raw_start else __start = __raw_start end"
            )
        } else {
            "local __start = 0".to_string()
        };
        let stop_init = if let Some(stop) = stop {
            format!(
                "local __raw_stop = {stop}; local __stop; if __raw_stop == molt_missing_sentinel then __stop = __n elseif __raw_stop < 0 then __stop = __n + __raw_stop else __stop = __raw_stop end"
            )
        } else {
            "local __stop = __n".to_string()
        };
        self.emit_line(&format!(
            "local {out} = -1; do local __n = molt_sequence_len({list}); {start_init}; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; {stop_init}; if __stop < 0 then __stop = 0 end; if __stop > __n then __stop = __n end; local __found = false; for __i = __start + 1, __stop do if rawget({list}, __i) == {val} then {out} = __i - 1; __found = true; break end end; if not __found then error(\"ValueError: \" .. tostring({val}) .. \" is not in list\") end end"
        ));
    }
}
