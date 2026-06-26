use super::*;

impl LuauBackend {
    pub(super) fn emit_collection_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // ================================================================
            // Collection construction
            // ================================================================
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
            "build_dict" | "dict_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out}: {{[any]: any}} = {{}}"));
                } else {
                    let mut entries = Vec::new();
                    for pair in args.chunks(2) {
                        if pair.len() == 2 {
                            let key = sanitize_ident(&pair[0]);
                            let val = sanitize_ident(&pair[1]);
                            entries.push(format!("[{key}] = {val}"));
                        }
                    }
                    let body = entries.join(", ");
                    self.emit_line(&format!("local {out}: {{[any]: any}} = {{{body}}}"));
                }
            }
            "set_new" | "frozenset_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out} = {{}}"));
                } else {
                    let entries = args
                        .iter()
                        .map(|a| format!("[{}] = true", sanitize_ident(a)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.emit_line(&format!("local {out} = {{{entries}}}"));
                }
            }

            // ================================================================
            // Dict operations
            // ================================================================
            "dict_clear" | "set_clear" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    let tbl = sanitize_ident(tbl);
                    self.emit_line(&format!("table.clear({tbl})"));
                }
            }
            "dict_copy" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!("local {out} = table.clone({src})"));
                }
            }
            "dict_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = if {dict}[{key}] ~= nil then {dict}[{key}] else {default}"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = {dict}[{key}]"));
                    }
                }
            }
            "dict_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = {val}"));
                }
            }
            "dict_setdefault" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then {dict}[{key}] = {val} end; local {out} = {dict}[{key}]"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then {dict}[{key}] = {val} end"
                        ));
                    }
                }
            }
            "dict_setdefault_empty_list" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if {dict}[{key}] == nil then {dict}[{key}] = {{}} end"
                    ));
                }
            }
            "dict_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = if {dict}[{key}] ~= nil then {dict}[{key}] else {default}"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then error(\"KeyError: \" .. tostring({key})) end"
                        ));
                        self.emit_line(&format!("local {out} = {dict}[{key}]"));
                    }
                    self.emit_line(&format!("{dict}[{key}] = nil"));
                }
            }
            "dict_update" | "dict_update_missing" | "dict_update_kwstar" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k, __v in pairs({other}) do {dict}[__k] = __v end"
                    ));
                }
            }
            "dict_popitem" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(dict) = args.first() {
                    let dict = sanitize_ident(dict);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k, __v in pairs({dict}) do {out} = {{__k, __v}}; {dict}[__k] = nil; break end; if {out} == nil then error({{__type=\"KeyError\", __msg=\"popitem(): dictionary is empty\"}}) end"
                    ));
                }
            }
            "dict_inc" | "dict_str_int_inc" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let inc = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = ({dict}[{key}] or 0) + {inc}"));
                }
            }
            "dict_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!(
                        "local {out} = {{}}; for __k, __v in pairs({src}) do {out}[__k] = __v end"
                    ));
                }
            }

            // ================================================================
            // Set operations
            // ================================================================
            "set_add" | "set_add_probe" | "frozenset_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = true"));
                }
            }
            "set_discard" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = nil"));
                }
            }
            "set_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if {set}[{val}] == nil then error(\"KeyError: \" .. tostring({val})) end; {set}[{val}] = nil"
                    ));
                }
            }
            "set_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(set) = args.first() {
                    let set = sanitize_ident(set);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k in pairs({set}) do {out} = __k; {set}[__k] = nil; break end; if {out} == nil then error(\"KeyError: pop from an empty set\") end"
                    ));
                }
            }
            "set_update" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k in pairs({other}) do {set}[__k] = true end"
                    ));
                }
            }
            _ => return false,
        }
        true
    }
}
