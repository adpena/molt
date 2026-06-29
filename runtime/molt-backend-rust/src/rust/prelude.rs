use super::RustBackend;

impl RustBackend {
    // ── File header ──────────────────────────────────────────────────────────

    pub(super) fn emit_header(&mut self) {
        self.output.push_str(concat!(
            "// Molt → Rust transpiled output\n",
            "// Auto-generated — do not edit\n",
            "#![allow(\n",
            "    unused_assignments, unused_mut, unused_variables, dead_code, non_snake_case,\n",
            "    clippy::needless_pass_by_value, clippy::clone_on_copy,\n",
            "    clippy::useless_vec,\n",
            ")]\n\n",
        ));
        if !self.use_crate {
            self.output.push_str("use std::sync::Arc;\n\n");
        }
    }

    pub(super) fn emit_prelude_conditional(&mut self, func_body: &str) {
        let used = |name: &str| func_body.contains(name);

        // Always emit the MoltValue enum — it is the foundation of everything.
        // Func variant uses Arc<dyn Fn>, which can't derive Debug or PartialEq,
        // so we implement them manually below.
        self.output.push_str(concat!(
            "#[derive(Clone)]\n",
            "pub enum MoltValue {\n",
            "    None,\n",
            "    Bool(bool),\n",
            "    Int(i64),\n",
            "    Float(f64),\n",
            "    Str(String),\n",
            "    List(Vec<MoltValue>),\n",
            "    Dict(Vec<(MoltValue, MoltValue)>),\n",
            "    Func(Arc<dyn Fn(&mut Vec<MoltValue>) -> MoltValue + Send + Sync>),\n",
            "}\n",
            "impl std::fmt::Debug for MoltValue {\n",
            "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n",
            "        match self {\n",
            "            MoltValue::None => write!(f, \"None\"),\n",
            "            MoltValue::Bool(b) => write!(f, \"{b}\"),\n",
            "            MoltValue::Int(n) => write!(f, \"{n}\"),\n",
            "            MoltValue::Float(v) => write!(f, \"{v}\"),\n",
            "            MoltValue::Str(s) => write!(f, \"{s:?}\"),\n",
            "            MoltValue::List(v) => write!(f, \"{v:?}\"),\n",
            "            MoltValue::Dict(d) => write!(f, \"{d:?}\"),\n",
            "            MoltValue::Func(_) => write!(f, \"<function>\"),\n",
            "        }\n",
            "    }\n",
            "}\n",
            "impl PartialEq for MoltValue {\n",
            "    fn eq(&self, other: &Self) -> bool {\n",
            "        match (self, other) {\n",
            "            (MoltValue::None, MoltValue::None) => true,\n",
            "            (MoltValue::Bool(a), MoltValue::Bool(b)) => a == b,\n",
            "            (MoltValue::Int(a), MoltValue::Int(b)) => a == b,\n",
            "            (MoltValue::Float(a), MoltValue::Float(b)) => a == b,\n",
            "            (MoltValue::Str(a), MoltValue::Str(b)) => a == b,\n",
            "            (MoltValue::List(a), MoltValue::List(b)) => a == b,\n",
            "            (MoltValue::Dict(a), MoltValue::Dict(b)) => a == b,\n",
            "            (MoltValue::Func(_), MoltValue::Func(_)) => false, // functions never equal\n",
            "            _ => false,\n",
            "        }\n",
            "    }\n",
            "}\n\n",
        ));

        self.output.push_str(concat!(
            "impl std::fmt::Display for MoltValue {\n",
            "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n",
            "        write!(f, \"{}\", molt_str(self))\n",
            "    }\n",
            "}\n\n",
        ));

        // Core coercion helpers — always emitted (used by all others).
        self.output.push_str(concat!(
            "fn molt_bool(x: &MoltValue) -> bool {\n",
            "    match x {\n",
            "        MoltValue::None => false,\n",
            "        MoltValue::Bool(b) => *b,\n",
            "        MoltValue::Int(n) => *n != 0,\n",
            "        MoltValue::Float(f) => *f != 0.0 && !f.is_nan(),\n",
            "        MoltValue::Str(s) => !s.is_empty(),\n",
            "        MoltValue::List(v) => !v.is_empty(),\n",
            "        MoltValue::Dict(d) => !d.is_empty(),\n",
            "        MoltValue::Func(_) => true,\n",
            "    }\n",
            "}\n\n",
            "fn molt_int(x: &MoltValue) -> i64 {\n",
            "    match x {\n",
            "        MoltValue::Int(n) => *n,\n",
            "        MoltValue::Float(f) => *f as i64,\n",
            "        MoltValue::Bool(b) => *b as i64,\n",
            "        MoltValue::Str(s) => s.trim().parse::<i64>().unwrap_or(0),\n",
            "        _ => 0,\n",
            "    }\n",
            "}\n\n",
            "fn molt_float(x: &MoltValue) -> f64 {\n",
            "    match x {\n",
            "        MoltValue::Float(f) => *f,\n",
            "        MoltValue::Int(n) => *n as f64,\n",
            "        MoltValue::Bool(b) => *b as i64 as f64,\n",
            "        MoltValue::Str(s) => s.trim().parse::<f64>().unwrap_or(0.0),\n",
            "        _ => 0.0,\n",
            "    }\n",
            "}\n\n",
        ));

        // molt_str — always emitted (Display impl references it).
        let needs_repr = used("molt_repr(");
        self.output.push_str(concat!(
            "fn molt_str(x: &MoltValue) -> String {\n",
            "    match x {\n",
            "        MoltValue::None => \"None\".to_string(),\n",
            "        MoltValue::Bool(true) => \"True\".to_string(),\n",
            "        MoltValue::Bool(false) => \"False\".to_string(),\n",
            "        MoltValue::Int(n) => n.to_string(),\n",
            "        MoltValue::Float(f) => format_float(*f),\n",
            "        MoltValue::Str(s) => s.clone(),\n",
            "        MoltValue::List(v) => {\n",
            "            let parts: Vec<String> = v.iter().map(molt_repr_inner).collect();\n",
            "            format!(\"[{}]\", parts.join(\", \"))\n",
            "        }\n",
            "        MoltValue::Dict(d) => {\n",
            "            let parts: Vec<String> = d.iter()\n",
            "                .map(|(k, v)| format!(\"{}: {}\", molt_repr_inner(k), molt_repr_inner(v)))\n",
            "                .collect();\n",
            "            format!(\"{{{}}}\", parts.join(\", \"))\n",
            "        }\n",
            "        MoltValue::Func(_) => \"<function>\".to_string(),\n",
            "    }\n",
            "}\n\n",
            "fn format_float(f: f64) -> String {\n",
            "    if f.fract() == 0.0 && f.is_finite() {\n",
            "        format!(\"{f:.1}\")\n",
            "    } else {\n",
            "        format!(\"{f}\")\n",
            "    }\n",
            "}\n\n",
            "fn molt_repr_inner(x: &MoltValue) -> String {\n",
            "    match x {\n",
            "        MoltValue::Str(s) => format!(\"'{s}'\"),\n",
            "        other => molt_str(other),\n",
            "    }\n",
            "}\n\n",
        ));

        if needs_repr {
            self.output.push_str(concat!(
                "fn molt_repr(x: &MoltValue) -> MoltValue {\n",
                "    MoltValue::Str(molt_repr_inner(x))\n",
                "}\n\n",
            ));
        }

        // print
        if used("molt_print(") {
            self.output.push_str(concat!(
                "fn molt_print(args: &[MoltValue]) {\n",
                "    let parts: Vec<String> = args.iter().map(molt_str).collect();\n",
                "    println!(\"{}\", parts.join(\" \"));\n",
                "}\n\n",
            ));
        }

        // len
        if used("molt_len(") {
            self.output.push_str(concat!(
                "fn molt_len(x: &MoltValue) -> MoltValue {\n",
                "    let n = match x {\n",
                "        MoltValue::Str(s) => s.chars().count() as i64,\n",
                "        MoltValue::List(v) => v.len() as i64,\n",
                "        MoltValue::Dict(d) => d.len() as i64,\n",
                "        _ => 0,\n",
                "    };\n",
                "    MoltValue::Int(n)\n",
                "}\n\n",
            ));
        }

        // range
        if used("molt_range(") || used("molt_builtin_func(") {
            self.output.push_str(concat!(
                "fn molt_range(start: i64, stop: i64, step: i64) -> MoltValue {\n",
                "    let mut result = Vec::new();\n",
                "    let mut i = start;\n",
                "    while (step > 0 && i < stop) || (step < 0 && i > stop) {\n",
                "        result.push(MoltValue::Int(i));\n",
                "        i += step;\n",
                "    }\n",
                "    MoltValue::List(result)\n",
                "}\n\n",
            ));
        }

        // arithmetic helpers
        if used("molt_add(") {
            self.output.push_str(concat!(
                "fn molt_add(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(x.wrapping_add(*y)),\n",
                "        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x + y),\n",
                "        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 + y),\n",
                "        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x + *y as f64),\n",
                "        (MoltValue::Str(x), MoltValue::Str(y)) => MoltValue::Str(format!(\"{x}{y}\")),\n",
                "        (MoltValue::List(x), MoltValue::List(y)) => {\n",
                "            let mut v = x.clone(); v.extend_from_slice(y); MoltValue::List(v)\n",
                "        }\n",
                "        _ => MoltValue::Int(molt_int(&a).wrapping_add(molt_int(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_sub(") {
            self.output.push_str(concat!(
                "fn molt_sub(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(x.wrapping_sub(*y)),\n",
                "        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x - y),\n",
                "        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 - y),\n",
                "        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x - *y as f64),\n",
                "        _ => MoltValue::Int(molt_int(&a).wrapping_sub(molt_int(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_mul(") {
            self.output.push_str(concat!(
                "fn molt_mul(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(x.wrapping_mul(*y)),\n",
                "        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x * y),\n",
                "        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 * y),\n",
                "        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x * *y as f64),\n",
                "        (MoltValue::Str(s), MoltValue::Int(n)) => MoltValue::Str(s.repeat(*n as usize)),\n",
                "        _ => MoltValue::Int(molt_int(&a).wrapping_mul(molt_int(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_div(") {
            self.output.push_str(concat!(
                "fn molt_div(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    let bv = molt_float(&b);\n",
                "    if bv == 0.0 { return MoltValue::Float(f64::NAN); }\n",
                "    MoltValue::Float(molt_float(&a) / bv)\n",
                "}\n\n",
            ));
        }
        if used("molt_floor_div(") {
            self.output.push_str(concat!(
                "fn molt_floor_div(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) if *y != 0 => {\n",
                "            MoltValue::Int(x.div_euclid(*y) - if (x % y != 0) && ((x < &0) != (y < &0)) { 1 } else { 0 })\n",
                "        }\n",
                "        _ => {\n",
                "            let bv = molt_float(&b);\n",
                "            if bv == 0.0 { return MoltValue::Float(f64::NAN); }\n",
                "            MoltValue::Float((molt_float(&a) / bv).floor())\n",
                "        }\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_mod(") {
            self.output.push_str(concat!(
                "fn molt_mod(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) if *y != 0 => {\n",
                "            let r = x % y; MoltValue::Int(if r != 0 && (r < 0) != (y < &0) { r + y } else { r })\n",
                "        }\n",
                "        _ => {\n",
                "            let av = molt_float(&a); let bv = molt_float(&b);\n",
                "            MoltValue::Float(av - (av / bv).floor() * bv)\n",
                "        }\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_pow(") {
            self.output.push_str(concat!(
                "fn molt_pow(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) if *y >= 0 => {\n",
                "            MoltValue::Int(x.wrapping_pow(*y as u32))\n",
                "        }\n",
                "        _ => MoltValue::Float(molt_float(&a).powf(molt_float(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_neg(") {
            self.output.push_str(concat!(
                "fn molt_neg(a: MoltValue) -> MoltValue {\n",
                "    match a {\n",
                "        MoltValue::Int(n) => MoltValue::Int(-n),\n",
                "        MoltValue::Float(f) => MoltValue::Float(-f),\n",
                "        other => MoltValue::Int(-molt_int(&other)),\n",
                "    }\n",
                "}\n\n",
            ));
        }

        // Comparison helpers — produce MoltValue::Bool.
        // Some collection helpers depend on `molt_eq`/`molt_numeric_cmp` even when
        // user IR never emits direct comparison ops, so include those dependencies.
        let needs_compare_helpers = used("molt_cmp(")
            || used("molt_eq(")
            || used("molt_ne(")
            || used("molt_lt(")
            || used("molt_le(")
            || used("molt_gt(")
            || used("molt_ge(")
            || used("molt_get_item(")
            || used("molt_ord_at(")
            || used("molt_set_item(")
            || used("molt_get_attr(")
            || used("molt_get_attr_name(")
            || used("molt_get_attr_name_default(")
            || used("molt_set_attr_name(")
            || used("molt_in(")
            || used("molt_sorted(")
            || used("molt_min(")
            || used("molt_max(");
        if needs_compare_helpers {
            self.output.push_str(concat!(
                "fn molt_is_numeric(x: &MoltValue) -> bool {\n",
                "    matches!(x, MoltValue::Bool(_) | MoltValue::Int(_) | MoltValue::Float(_))\n",
                "}\n",
                "fn molt_numeric_cmp(a: &MoltValue, b: &MoltValue) -> std::cmp::Ordering {\n",
                "    match (a, b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => x.cmp(y),\n",
                "        _ => molt_float(a).partial_cmp(&molt_float(b)).unwrap_or(std::cmp::Ordering::Equal),\n",
                "    }\n",
                "}\n",
                "fn molt_eq(a: &MoltValue, b: &MoltValue) -> bool {\n",
                "    match (a, b) {\n",
                "        (MoltValue::None, MoltValue::None) => true,\n",
                "        (MoltValue::Bool(x), MoltValue::Bool(y)) => x == y,\n",
                "        (MoltValue::Str(x), MoltValue::Str(y)) => x == y,\n",
                "        (MoltValue::List(x), MoltValue::List(y)) => x == y,\n",
                "        (MoltValue::Dict(x), MoltValue::Dict(y)) => x == y,\n",
                "        _ if molt_is_numeric(a) && molt_is_numeric(b) => {\n",
                "            matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Equal)\n",
                "        }\n",
                "        _ => false,\n",
                "    }\n",
                "}\n",
                "fn molt_lt(a: &MoltValue, b: &MoltValue) -> bool { matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Less) }\n",
                "fn molt_le(a: &MoltValue, b: &MoltValue) -> bool { !matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Greater) }\n",
                "fn molt_gt(a: &MoltValue, b: &MoltValue) -> bool { matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Greater) }\n",
                "fn molt_ge(a: &MoltValue, b: &MoltValue) -> bool { !matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Less) }\n\n",
            ));
        }

        // Collection helpers
        if used("molt_get_item(") || used("molt_ord_at(") {
            self.output.push_str(concat!(
                "fn molt_get_item(obj: &MoltValue, key: &MoltValue) -> MoltValue {\n",
                "    match obj {\n",
                "        MoltValue::List(v) => {\n",
                "            let idx = molt_int(key);\n",
                "            let i = if idx < 0 { (v.len() as i64 + idx).max(0) as usize } else { idx as usize };\n",
                "            v.get(i).cloned().unwrap_or(MoltValue::None)\n",
                "        }\n",
                "        MoltValue::Dict(d) => d.iter().find(|(k, _)| molt_eq(k, key))\n",
                "            .map(|(_, v)| v.clone()).unwrap_or(MoltValue::None),\n",
                "        MoltValue::Str(s) => {\n",
                "            let idx = molt_int(key);\n",
                "            let chars: Vec<char> = s.chars().collect();\n",
                "            let i = if idx < 0 { (chars.len() as i64 + idx).max(0) as usize } else { idx as usize };\n",
                "            chars.get(i).map(|c| MoltValue::Str(c.to_string())).unwrap_or(MoltValue::None)\n",
                "        }\n",
                "        _ => MoltValue::None,\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_set_item(") || used("molt_set_attr_name(") {
            self.output.push_str(concat!(
                "fn molt_set_item(obj: &mut MoltValue, key: MoltValue, val: MoltValue) {\n",
                "    match obj {\n",
                "        MoltValue::List(v) => {\n",
                "            let idx = molt_int(&key);\n",
                "            let i = if idx < 0 { (v.len() as i64 + idx).max(0) as usize } else { idx as usize };\n",
                "            if i < v.len() { v[i] = val; }\n",
                "        }\n",
                "        MoltValue::Dict(d) => {\n",
                "            if let Some(entry) = d.iter_mut().find(|(k, _)| molt_eq(k, &key)) {\n",
                "                entry.1 = val;\n",
                "            } else {\n",
                "                d.push((key, val));\n",
                "            }\n",
                "        }\n",
                "        _ => {}\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_list_append(") {
            self.output.push_str(concat!(
                "fn molt_list_append(list: &mut MoltValue, val: MoltValue) {\n",
                "    if let MoltValue::List(v) = list { v.push(val); }\n",
                "}\n\n",
            ));
        }
        if used("molt_get_attr(") {
            self.output.push_str(concat!(
                "fn molt_get_attr(obj: &MoltValue, attr: &str) -> MoltValue {\n",
                "    molt_get_attr_name(obj, &MoltValue::Str(attr.to_string()))\n",
                "}\n\n",
            ));
        }
        if used("molt_get_attr(")
            || used("molt_get_attr_name(")
            || used("molt_get_attr_name_default(")
        {
            self.output.push_str(concat!(
                "fn molt_get_attr_name(obj: &MoltValue, name: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Dict(d) = obj {\n",
                "        if let Some((_, v)) = d.iter().find(|(k, _)| molt_eq(k, name)) {\n",
                "            return v.clone();\n",
                "        }\n",
                "        let class_key = MoltValue::Str(\"__class__\".to_string());\n",
                "        if let Some((_, class_obj)) = d.iter().find(|(k, _)| molt_eq(k, &class_key)) {\n",
                "            if let MoltValue::Dict(class_dict) = class_obj {\n",
                "                if let Some((_, v)) = class_dict.iter().find(|(k, _)| molt_eq(k, name)) {\n",
                "                    return v.clone();\n",
                "                }\n",
                "            }\n",
                "        }\n",
                "    }\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_get_attr_name_default(obj: &MoltValue, name: &MoltValue, default: &MoltValue) -> MoltValue {\n",
                "    let value = molt_get_attr_name(obj, name);\n",
                "    if matches!(value, MoltValue::None) { default.clone() } else { value }\n",
                "}\n\n",
            ));
        }
        if used("molt_set_attr_name(") {
            self.output.push_str(concat!(
                "fn molt_set_attr_name(obj: &mut MoltValue, name: MoltValue, val: MoltValue) {\n",
                "    molt_set_item(obj, name, val);\n",
                "}\n\n",
            ));
        }
        if used("molt_class_merge_layout(") {
            self.output.push_str(
                r#"fn molt_class_merge_layout(class_obj: &mut MoltValue, offsets: MoltValue, size: MoltValue) -> MoltValue {
    let class_dict = match class_obj {
        MoltValue::Dict(d) => d,
        _ => panic!("class layout merge expects type"),
    };
    let hinted_size = match size {
        MoltValue::Int(v) if v >= 0 => v as usize,
        _ => panic!("__molt_layout_size__ must be int"),
    };
    let mut merged_offsets: Option<Vec<(MoltValue, MoltValue)>> = None;
    match offsets {
        MoltValue::None => {
            if let Some((_, MoltValue::Dict(existing))) = class_dict.iter().find(
                |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_field_offsets__"),
            ) {
                merged_offsets = Some(existing.clone());
            }
        }
        MoltValue::Dict(source_offsets) => {
            let target_index = if let Some(index) = class_dict.iter().position(
                |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_field_offsets__"),
            ) {
                index
            } else {
                class_dict.push((
                    MoltValue::Str("__molt_field_offsets__".to_string()),
                    MoltValue::Dict(vec![]),
                ));
                class_dict.len() - 1
            };
            let target_offsets = match &mut class_dict[target_index].1 {
                MoltValue::Dict(d) => d,
                _ => panic!("__molt_field_offsets__ must be dict"),
            };
            for (name, offset) in source_offsets {
                if target_offsets.iter().any(|(existing, _)| existing == &name) {
                    continue;
                }
                target_offsets.push((name, offset));
            }
            merged_offsets = Some(target_offsets.clone());
        }
        _ => panic!("__molt_field_offsets__ must be dict or None"),
    }

    let mut layout_size = class_dict
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (MoltValue::Str(name), MoltValue::Int(existing)) if name == "__molt_layout_size__" && *existing > 0 => {
                Some(*existing as usize)
            }
            _ => None,
        })
        .unwrap_or(0);
    layout_size = layout_size.max(hinted_size);
    if let Some(offsets_dict) = merged_offsets.as_ref() {
        let mut max_end = 0usize;
        for (_, offset) in offsets_dict.iter() {
            if let MoltValue::Int(value) = offset {
                if *value < 0 {
                    continue;
                }
                let end = (*value as usize).saturating_add(std::mem::size_of::<u64>());
                if end > max_end {
                    max_end = end;
                }
            }
        }
        layout_size = layout_size.max(max_end.saturating_add(std::mem::size_of::<u64>()));
    }
    if layout_size == 0 {
        layout_size = std::mem::size_of::<u64>();
    }

    if let Some((_, value)) = class_dict.iter_mut().find(
        |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_layout_size__"),
    ) {
        *value = MoltValue::Int(layout_size as i64);
    } else {
        class_dict.push((
            MoltValue::Str("__molt_layout_size__".to_string()),
            MoltValue::Int(layout_size as i64),
        ));
    }
    MoltValue::None
}

"#,
            );
        }
        if used("molt_in(") {
            self.output.push_str(concat!(
                "fn molt_in(elem: &MoltValue, container: &MoltValue) -> bool {\n",
                "    match container {\n",
                "        MoltValue::List(v) => v.iter().any(|x| molt_eq(x, elem)),\n",
                "        MoltValue::Dict(d) => d.iter().any(|(k, _)| molt_eq(k, elem)),\n",
                "        MoltValue::Str(s) => {\n",
                "            if let MoltValue::Str(sub) = elem { s.contains(sub.as_str()) } else { false }\n",
                "        }\n",
                "        _ => false,\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_code_new(") || used("molt_code_slot_set(") || used("molt_code_slots_init(") {
            self.output.push_str(concat!(
                "thread_local! {\n",
                "    static MOLT_CODE_SLOTS: std::cell::RefCell<Vec<Option<MoltValue>>> = const { std::cell::RefCell::new(Vec::new()) };\n",
                "}\n\n",
                "fn molt_expect_code_str(value: &MoltValue, field: &str) -> String {\n",
                "    if let MoltValue::Str(s) = value { s.clone() } else { panic!(\"TypeError: code {field} must be str\") }\n",
                "}\n\n",
                "fn molt_expect_code_count(value: &MoltValue, field: &str) -> i64 {\n",
                "    let n = molt_int(value);\n",
                "    if n < 0 { panic!(\"ValueError: code {field} must be >= 0\") }\n",
                "    n\n",
                "}\n\n",
                "fn molt_code_is_code(value: &MoltValue) -> bool {\n",
                "    if let MoltValue::Dict(fields) = value {\n",
                "        fields.iter().any(|(key, value)| matches!((key, value), (MoltValue::Str(k), MoltValue::Str(v)) if k == \"__molt_type__\" && v == \"code\"))\n",
                "    } else { false }\n",
                "}\n\n",
                "fn molt_code_new(filename: &MoltValue, name: &MoltValue, firstlineno: &MoltValue, linetable: &MoltValue, varnames: &MoltValue, names: &MoltValue, argcount: &MoltValue, posonlyargcount: &MoltValue, kwonlyargcount: &MoltValue) -> MoltValue {\n",
                "    let filename = molt_expect_code_str(filename, \"filename\");\n",
                "    let name = molt_expect_code_str(name, \"name\");\n",
                "    let firstlineno = molt_int(firstlineno);\n",
                "    let argcount = molt_expect_code_count(argcount, \"argcount\");\n",
                "    let posonlyargcount = molt_expect_code_count(posonlyargcount, \"posonlyargcount\");\n",
                "    let kwonlyargcount = molt_expect_code_count(kwonlyargcount, \"kwonlyargcount\");\n",
                "    MoltValue::Dict(vec![\n",
                "        (MoltValue::Str(\"__molt_type__\".to_string()), MoltValue::Str(\"code\".to_string())),\n",
                "        (MoltValue::Str(\"co_filename\".to_string()), MoltValue::Str(filename)),\n",
                "        (MoltValue::Str(\"co_name\".to_string()), MoltValue::Str(name)),\n",
                "        (MoltValue::Str(\"co_firstlineno\".to_string()), MoltValue::Int(firstlineno)),\n",
                "        (MoltValue::Str(\"co_linetable\".to_string()), linetable.clone()),\n",
                "        (MoltValue::Str(\"co_varnames\".to_string()), varnames.clone()),\n",
                "        (MoltValue::Str(\"co_names\".to_string()), names.clone()),\n",
                "        (MoltValue::Str(\"co_argcount\".to_string()), MoltValue::Int(argcount)),\n",
                "        (MoltValue::Str(\"co_posonlyargcount\".to_string()), MoltValue::Int(posonlyargcount)),\n",
                "        (MoltValue::Str(\"co_kwonlyargcount\".to_string()), MoltValue::Int(kwonlyargcount)),\n",
                "    ])\n",
                "}\n\n",
                "fn molt_code_slots_init(count: i64) -> MoltValue {\n",
                "    if count < 0 { panic!(\"MemoryError: code slot count too large\") }\n",
                "    MOLT_CODE_SLOTS.with(|slots| {\n",
                "        let mut slots = slots.borrow_mut();\n",
                "        if slots.is_empty() {\n",
                "            slots.resize(count as usize, None);\n",
                "        }\n",
                "    });\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_code_slot_set(code_id: i64, code: &MoltValue) -> MoltValue {\n",
                "    if !molt_code_is_code(code) { panic!(\"TypeError: code slot expects code object\") }\n",
                "    let Some(idx) = usize::try_from(code_id).ok() else { panic!(\"IndexError: code slot out of range\") };\n",
                "    MOLT_CODE_SLOTS.with(|slots| {\n",
                "        let mut slots = slots.borrow_mut();\n",
                "        if idx >= slots.len() { panic!(\"IndexError: code slot out of range\") }\n",
                "        slots[idx] = Some(code.clone());\n",
                "    });\n",
                "    MoltValue::None\n",
                "}\n\n",
            ));
        }
        if used("molt_trace_enter_slot(")
            || used("molt_trace_exit(")
            || used("molt_frame_locals_set(")
        {
            self.output.push_str(concat!(
                "#[derive(Clone)]\n",
                "struct MoltFrame {\n",
                "    code_id: i64,\n",
                "    locals: Option<MoltValue>,\n",
                "}\n\n",
                "thread_local! {\n",
                "    static MOLT_FRAME_STACK: std::cell::RefCell<Vec<MoltFrame>> = const { std::cell::RefCell::new(Vec::new()) };\n",
                "}\n\n",
                "fn molt_trace_enter_slot(code_id: i64) -> MoltValue {\n",
                "    MOLT_FRAME_STACK.with(|stack| stack.borrow_mut().push(MoltFrame { code_id, locals: None }));\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_trace_exit() -> MoltValue {\n",
                "    MOLT_FRAME_STACK.with(|stack| { stack.borrow_mut().pop(); });\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_frame_locals_set(locals: &MoltValue) -> MoltValue {\n",
                "    MOLT_FRAME_STACK.with(|stack| {\n",
                "        if let Some(frame) = stack.borrow_mut().last_mut() {\n",
                "            frame.locals = Some(locals.clone());\n",
                "        }\n",
                "    });\n",
                "    MoltValue::None\n",
                "}\n\n",
            ));
        }
        if used("molt_exception_") {
            self.output.push_str(concat!(
                "thread_local! {\n",
                "    static MOLT_EXCEPTION_LAST: std::cell::RefCell<Option<MoltValue>> = const { std::cell::RefCell::new(None) };\n",
                "    static MOLT_EXCEPTION_CONTEXT: std::cell::RefCell<Option<MoltValue>> = const { std::cell::RefCell::new(None) };\n",
                "    static MOLT_EXCEPTION_STACK_DEPTH: std::cell::RefCell<usize> = const { std::cell::RefCell::new(0) };\n",
                "    static MOLT_EXCEPTION_STACK_BASELINE: std::cell::RefCell<usize> = const { std::cell::RefCell::new(0) };\n",
                "}\n\n",
                "fn molt_exception_is_exception(value: &MoltValue) -> bool {\n",
                "    if let MoltValue::Dict(fields) = value {\n",
                "        fields.iter().any(|(key, value)| matches!((key, value), (MoltValue::Str(k), MoltValue::Str(v)) if k == \"__molt_type__\" && v == \"exception\"))\n",
                "    } else { false }\n",
                "}\n\n",
                "fn molt_exception_last() -> MoltValue {\n",
                "    if let Some(exc) = MOLT_EXCEPTION_LAST.with(|slot| slot.borrow().clone()) {\n",
                "        return exc;\n",
                "    }\n",
                "    MOLT_EXCEPTION_CONTEXT.with(|slot| slot.borrow().clone().unwrap_or(MoltValue::None))\n",
                "}\n\n",
                "fn molt_exception_last_pending() -> MoltValue {\n",
                "    MOLT_EXCEPTION_LAST.with(|slot| slot.borrow().clone().unwrap_or(MoltValue::None))\n",
                "}\n\n",
                "fn molt_exception_active() -> MoltValue {\n",
                "    MOLT_EXCEPTION_CONTEXT.with(|slot| slot.borrow().clone().unwrap_or(MoltValue::None))\n",
                "}\n\n",
                "fn molt_exception_clear() -> MoltValue {\n",
                "    MOLT_EXCEPTION_LAST.with(|slot| *slot.borrow_mut() = None);\n",
                "    MOLT_EXCEPTION_CONTEXT.with(|slot| *slot.borrow_mut() = None);\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_exception_set_last(exc: &MoltValue) -> MoltValue {\n",
                "    if matches!(exc, MoltValue::None) || !molt_exception_is_exception(exc) {\n",
                "        return molt_exception_clear();\n",
                "    }\n",
                "    MOLT_EXCEPTION_LAST.with(|slot| *slot.borrow_mut() = Some(exc.clone()));\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_exception_stack_enter() -> MoltValue {\n",
                "    let prev = MOLT_EXCEPTION_STACK_BASELINE.with(|baseline| *baseline.borrow());\n",
                "    let depth = MOLT_EXCEPTION_STACK_DEPTH.with(|depth| *depth.borrow());\n",
                "    MOLT_EXCEPTION_STACK_BASELINE.with(|baseline| *baseline.borrow_mut() = depth);\n",
                "    MoltValue::Int(prev as i64)\n",
                "}\n\n",
                "fn molt_exception_stack_depth() -> MoltValue {\n",
                "    let depth = MOLT_EXCEPTION_STACK_DEPTH.with(|depth| *depth.borrow());\n",
                "    MoltValue::Int(depth as i64)\n",
                "}\n\n",
                "fn molt_exception_stack_exit(prev: &MoltValue) -> MoltValue {\n",
                "    let prev = molt_int(prev);\n",
                "    MOLT_EXCEPTION_STACK_BASELINE.with(|baseline| *baseline.borrow_mut() = if prev >= 0 { prev as usize } else { 0 });\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_exception_stack_set_depth(depth: &MoltValue) -> MoltValue {\n",
                "    let depth = molt_int(depth);\n",
                "    MOLT_EXCEPTION_STACK_DEPTH.with(|slot| *slot.borrow_mut() = if depth >= 0 { depth as usize } else { 0 });\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_exception_stack_clear() -> MoltValue {\n",
                "    MOLT_EXCEPTION_STACK_DEPTH.with(|depth| *depth.borrow_mut() = 0);\n",
                "    MOLT_EXCEPTION_STACK_BASELINE.with(|baseline| *baseline.borrow_mut() = 0);\n",
                "    MoltValue::None\n",
                "}\n\n",
            ));
        }

        // Higher-order helpers
        if used("molt_enumerate(") || used("molt_builtin_func(") {
            self.output.push_str(concat!(
                "fn molt_enumerate(t: &MoltValue, start: i64) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        let result = v.iter().enumerate()\n",
                "            .map(|(i, x)| MoltValue::List(vec![MoltValue::Int(start + i as i64), x.clone()]))\n",
                "            .collect();\n",
                "        MoltValue::List(result)\n",
                "    } else { MoltValue::List(vec![]) }\n",
                "}\n\n",
            ));
        }
        if used("molt_zip(") || used("molt_builtin_func(") {
            self.output.push_str(concat!(
                "fn molt_zip(a: &MoltValue, b: &MoltValue) -> MoltValue {\n",
                "    match (a, b) {\n",
                "        (MoltValue::List(av), MoltValue::List(bv)) => {\n",
                "            let result = av.iter().zip(bv.iter())\n",
                "                .map(|(x, y)| MoltValue::List(vec![x.clone(), y.clone()]))\n",
                "                .collect();\n",
                "            MoltValue::List(result)\n",
                "        }\n",
                "        _ => MoltValue::List(vec![]),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_sorted(") {
            self.output.push_str(concat!(
                "fn molt_sorted(t: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        let mut copy = v.clone();\n",
                "        copy.sort_by(|a, b| molt_numeric_cmp(a, b));\n",
                "        MoltValue::List(copy)\n",
                "    } else { t.clone() }\n",
                "}\n\n",
            ));
        }
        if used("molt_reversed(") {
            self.output.push_str(concat!(
                "fn molt_reversed(t: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        MoltValue::List(v.iter().rev().cloned().collect())\n",
                "    } else { t.clone() }\n",
                "}\n\n",
            ));
        }
        if used("molt_sum(") {
            self.output.push_str(concat!(
                "fn molt_sum(t: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        v.iter().fold(MoltValue::Int(0), |acc, x| molt_add(acc, x.clone()))\n",
                "    } else { MoltValue::Int(0) }\n",
                "}\n\n",
            ));
        }
        if used("molt_any(") {
            self.output.push_str(concat!(
                "fn molt_any(t: &MoltValue) -> bool {\n",
                "    if let MoltValue::List(v) = t { v.iter().any(|x| molt_bool(x)) } else { false }\n",
                "}\n\n",
            ));
        }
        if used("molt_all(") {
            self.output.push_str(concat!(
                "fn molt_all(t: &MoltValue) -> bool {\n",
                "    if let MoltValue::List(v) = t { v.iter().all(|x| molt_bool(x)) } else { true }\n",
                "}\n\n",
            ));
        }
        if used("molt_dict_keys(") {
            self.output.push_str(concat!(
                "fn molt_dict_keys(d: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Dict(pairs) = d {\n",
                "        MoltValue::List(pairs.iter().map(|(k, _)| k.clone()).collect())\n",
                "    } else { MoltValue::List(vec![]) }\n",
                "}\n\n",
            ));
        }
        if used("molt_dict_values(") {
            self.output.push_str(concat!(
                "fn molt_dict_values(d: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(v) = d { MoltValue::List(v.clone()) }\n",
                "    else if let MoltValue::Dict(pairs) = d {\n",
                "        MoltValue::List(pairs.iter().map(|(_, v)| v.clone()).collect())\n",
                "    } else { MoltValue::List(vec![]) }\n",
                "}\n\n",
            ));
        }
        if used("molt_dict_items(") {
            self.output.push_str(concat!(
                "fn molt_dict_items(d: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Dict(pairs) = d {\n",
                "        MoltValue::List(pairs.iter()\n",
                "            .map(|(k, v)| MoltValue::List(vec![k.clone(), v.clone()]))\n",
                "            .collect())\n",
                "    } else { MoltValue::List(vec![]) }\n",
                "}\n\n",
            ));
        }

        // iter helper for for_iter ops
        if used("molt_iter_list(") || used("molt_iter(") || used("molt_iter_next(") {
            self.output.push_str(concat!(
                "fn molt_iter_list(x: &MoltValue) -> Vec<MoltValue> {\n",
                "    match x {\n",
                "        MoltValue::List(v) => v.clone(),\n",
                "        MoltValue::Dict(d) => d.iter().map(|(k, _)| k.clone()).collect(),\n",
                "        MoltValue::Str(s) => s.chars().map(|c| MoltValue::Str(c.to_string())).collect(),\n",
                "        _ => vec![],\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_iter(") {
            self.output.push_str(concat!(
                "fn molt_iter(x: &MoltValue) -> MoltValue {\n",
                "    let items = molt_iter_list(x);\n",
                "    MoltValue::List(vec![MoltValue::Int(0), MoltValue::List(items)])\n",
                "}\n\n",
            ));
        }
        if used("molt_iter_next(") {
            self.output.push_str(concat!(
                "fn molt_iter_next(iter: &mut MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(state) = iter {\n",
                "        if state.len() >= 2 {\n",
                "            let idx = molt_int(&state[0]);\n",
                "            if let MoltValue::List(items) = &state[1] {\n",
                "                let done = idx < 0 || (idx as usize) >= items.len();\n",
                "                if done {\n",
                "                    return MoltValue::List(vec![MoltValue::None, MoltValue::Bool(true)]);\n",
                "                }\n",
                "                let value = items[idx as usize].clone();\n",
                "                state[0] = MoltValue::Int(idx + 1);\n",
                "                return MoltValue::List(vec![value, MoltValue::Bool(false)]);\n",
                "            }\n",
                "        }\n",
                "    }\n",
                "    MoltValue::List(vec![MoltValue::None, MoltValue::Bool(true)])\n",
                "}\n\n",
            ));
        }
        if used("molt_unpack_sequence(") {
            self.output.push_str(
                r#"fn molt_unpack_type_name(seq: &MoltValue) -> &'static str {
    match seq {
        MoltValue::None => "NoneType",
        MoltValue::Bool(_) => "bool",
        MoltValue::Int(_) => "int",
        MoltValue::Float(_) => "float",
        MoltValue::Str(_) => "str",
        MoltValue::List(_) => "list",
        MoltValue::Dict(_) => "dict",
        MoltValue::Func(_) => "function",
    }
}

fn molt_unpack_too_many_message(expected_count: usize, actual: usize) -> String {
    if molt_runtime_target_at_least(3, 14) {
        format!("too many values to unpack (expected {}, got {})", expected_count, actual)
    } else {
        format!("too many values to unpack (expected {})", expected_count)
    }
}

fn molt_unpack_sequence(seq: &MoltValue, expected_count: usize) -> Vec<MoltValue> {
    let items = match seq {
        MoltValue::List(v) => v.clone(),
        MoltValue::Dict(d) => d.iter().map(|(k, _)| k.clone()).collect(),
        MoltValue::Str(s) => s.chars().map(|c| MoltValue::Str(c.to_string())).collect(),
        _ => panic!("cannot unpack non-iterable {} object", molt_unpack_type_name(seq)),
    };
    let actual = items.len();
    if actual < expected_count {
        panic!("not enough values to unpack (expected {}, got {})", expected_count, actual);
    }
    if actual > expected_count {
        panic!("{}", molt_unpack_too_many_message(expected_count, actual));
    }
    items
}

"#,
            );
        }

        // abs
        if used("molt_abs(") {
            self.output.push_str(concat!(
                "fn molt_abs(x: MoltValue) -> MoltValue {\n",
                "    match x {\n",
                "        MoltValue::Int(n) => MoltValue::Int(n.abs()),\n",
                "        MoltValue::Float(f) => MoltValue::Float(f.abs()),\n",
                "        other => other,\n",
                "    }\n",
                "}\n\n",
            ));
        }

        // min/max
        if used("molt_min(") {
            self.output.push_str(concat!(
                "fn molt_min(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    if matches!(molt_numeric_cmp(&a, &b), std::cmp::Ordering::Less | std::cmp::Ordering::Equal) { a } else { b }\n",
                "}\n\n",
            ));
        }
        if used("molt_max(") {
            self.output.push_str(concat!(
                "fn molt_max(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    if matches!(molt_numeric_cmp(&a, &b), std::cmp::Ordering::Greater | std::cmp::Ordering::Equal) { a } else { b }\n",
                "}\n\n",
            ));
        }

        // chr/ord
        if used("molt_chr(") {
            self.output.push_str(concat!(
                "fn molt_chr(x: &MoltValue) -> MoltValue {\n",
                "    let n = molt_int(x) as u32;\n",
                "    MoltValue::Str(char::from_u32(n).map(|c| c.to_string()).unwrap_or_default())\n",
                "}\n\n",
            ));
        }
        if used("molt_ord(") || used("molt_ord_at(") {
            self.output.push_str(concat!(
                "fn molt_ord(x: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Str(s) = x {\n",
                "        MoltValue::Int(s.chars().next().map(|c| c as i64).unwrap_or(0))\n",
                "    } else { MoltValue::Int(0) }\n",
                "}\n\n",
            ));
        }
        if used("molt_ord_at(") {
            self.output.push_str(concat!(
                "fn molt_ord_at(obj: &MoltValue, key: &MoltValue) -> MoltValue {\n",
                "    let item = molt_get_item(obj, key);\n",
                "    molt_ord(&item)\n",
                "}\n\n",
            ));
        }

        // sys target-version state. The frontend stamps this before user code,
        // and standalone Rust must preserve the same contract as native/WASM.
        let needs_module_import = used("molt_import_module(");
        let needs_sys_version_state = used("molt_sys_set_version_info(")
            || used("molt_sys_version_info(")
            || used("molt_sys_version(")
            || used("molt_sys_hexversion(")
            || used("molt_unpack_sequence(")
            || needs_module_import;
        let needs_module_cache = used("molt_module_cache_get(")
            || used("molt_module_cache_set(")
            || used("molt_module_cache_del(");
        if needs_sys_version_state {
            self.output.push_str(
                r#"#[derive(Clone)]
struct MoltSysVersionInfo {
    major: i64,
    minor: i64,
    micro: i64,
    releaselevel: String,
    serial: i64,
    version: String,
}

impl Default for MoltSysVersionInfo {
    fn default() -> Self {
        Self {
            major: 3,
            minor: 12,
            micro: 0,
            releaselevel: "final".to_string(),
            serial: 0,
            version: "3.12.0 (molt)".to_string(),
        }
    }
}

impl MoltSysVersionInfo {
    fn formatted_version(&self) -> String {
        let suffix = match self.releaselevel.as_str() {
            "alpha" => format!("a{}", self.serial),
            "beta" => format!("b{}", self.serial),
            "candidate" => format!("rc{}", self.serial),
            "final" | "" => String::new(),
            other => format!("{other}{}", self.serial),
        };
        format!("{}.{}.{}{} (molt)", self.major, self.minor, self.micro, suffix)
    }

    fn hexversion(&self) -> i64 {
        let release_nibble = match self.releaselevel.as_str() {
            "alpha" => 0xA,
            "beta" => 0xB,
            "candidate" => 0xC,
            "final" => 0xF,
            _ => 0xF,
        };
        ((self.major & 0xFF) << 24)
            | ((self.minor & 0xFF) << 16)
            | ((self.micro & 0xFF) << 8)
            | ((release_nibble & 0xF) << 4)
            | (self.serial & 0xF)
    }
}

fn molt_sys_version_state() -> &'static std::sync::Mutex<MoltSysVersionInfo> {
    static STATE: std::sync::OnceLock<std::sync::Mutex<MoltSysVersionInfo>> =
        std::sync::OnceLock::new();
    STATE.get_or_init(|| std::sync::Mutex::new(MoltSysVersionInfo::default()))
}

fn molt_runtime_target_at_least(major: i64, minor: i64) -> bool {
    let state = molt_sys_version_state().lock().unwrap().clone();
    (state.major, state.minor) >= (major, minor)
}

fn molt_sys_arg_int(args: &[MoltValue], index: usize, default: i64) -> i64 {
    args.get(index).map_or(default, molt_int)
}

fn molt_sys_arg_str(args: &[MoltValue], index: usize, default: &str) -> String {
    args.get(index)
        .map(molt_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn molt_sys_set_version_info(args: &mut Vec<MoltValue>) -> MoltValue {
    let mut next = MoltSysVersionInfo {
        major: molt_sys_arg_int(args, 0, 3),
        minor: molt_sys_arg_int(args, 1, 12),
        micro: molt_sys_arg_int(args, 2, 0),
        releaselevel: molt_sys_arg_str(args, 3, "final"),
        serial: molt_sys_arg_int(args, 4, 0),
        version: molt_sys_arg_str(args, 5, ""),
    };
    if next.version.is_empty() {
        next.version = next.formatted_version();
    }
    *molt_sys_version_state().lock().unwrap() = next;
    MoltValue::None
}

fn molt_sys_version_info(_args: &mut Vec<MoltValue>) -> MoltValue {
    let state = molt_sys_version_state().lock().unwrap().clone();
    MoltValue::List(vec![
        MoltValue::Int(state.major),
        MoltValue::Int(state.minor),
        MoltValue::Int(state.micro),
        MoltValue::Str(state.releaselevel),
        MoltValue::Int(state.serial),
    ])
}

fn molt_sys_version(_args: &mut Vec<MoltValue>) -> MoltValue {
    let state = molt_sys_version_state().lock().unwrap().clone();
    MoltValue::Str(state.version.clone())
}

fn molt_sys_hexversion(_args: &mut Vec<MoltValue>) -> MoltValue {
    let state = molt_sys_version_state().lock().unwrap().clone();
    MoltValue::Int(state.hexversion())
}

"#,
            );
        }

        if needs_module_cache {
            self.output.push_str(concat!(
                "fn molt_module_cache() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, MoltValue>> {\n",
                "    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<String, MoltValue>>> = std::sync::OnceLock::new();\n",
                "    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))\n",
                "}\n\n",
                "fn molt_module_cache_get(name: &MoltValue) -> MoltValue {\n",
                "    let key = molt_str(name);\n",
                "    molt_module_cache().lock().unwrap().get(&key).cloned().unwrap_or(MoltValue::None)\n",
                "}\n\n",
                "fn molt_module_cache_set(name: &MoltValue, module: MoltValue) -> MoltValue {\n",
                "    let key = molt_str(name);\n",
                "    let mut cache = molt_module_cache().lock().unwrap();\n",
                "    if let Some(existing) = cache.get(&key) {\n",
                "        if !matches!(existing, MoltValue::None) && existing != &module {\n",
                "            return existing.clone();\n",
                "        }\n",
                "    }\n",
                "    cache.insert(key, module);\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_module_cache_del(name: &MoltValue) -> MoltValue {\n",
                "    let key = molt_str(name);\n",
                "    molt_module_cache().lock().unwrap().remove(&key);\n",
                "    MoltValue::None\n",
                "}\n\n",
            ));
        }

        if needs_module_import {
            self.output.push_str(concat!(
                "fn molt_import_module(name: &MoltValue) -> MoltValue {\n",
                "    let module_name = molt_str(name);\n",
                "    match module_name.as_str() {\n",
                "        \"sys\" => {\n",
                "            let mut args = Vec::new();\n",
                "            let version_info = molt_sys_version_info(&mut args);\n",
                "            let version = molt_sys_version(&mut args);\n",
                "            let hexversion = molt_sys_hexversion(&mut args);\n",
                "            MoltValue::Dict(vec![\n",
                "                (MoltValue::Str(\"version_info\".to_string()), version_info),\n",
                "                (MoltValue::Str(\"version\".to_string()), version),\n",
                "                (MoltValue::Str(\"hexversion\".to_string()), hexversion),\n",
                "            ])\n",
                "        }\n",
                "        other => panic!(\"unsupported module import in Rust backend: {other}\"),\n",
                "    }\n",
                "}\n\n",
            ));
        }

        // Runtime lifecycle hooks are no-ops for standalone binaries.
        if used("molt_runtime_init(") {
            self.output.push_str(concat!(
                "fn molt_runtime_init(_args: &mut Vec<MoltValue>) -> MoltValue { MoltValue::None }\n",
                "fn molt_runtime_shutdown(_args: &mut Vec<MoltValue>) -> MoltValue { MoltValue::None }\n\n",
            ));
        }

        // Dynamic call dispatch
        if used("molt_call(") {
            self.output.push_str(concat!(
                "fn molt_call(f: &MoltValue, args: &mut Vec<MoltValue>) -> MoltValue {\n",
                "    if let MoltValue::Func(func) = f { func(args) } else { MoltValue::None }\n",
                "}\n\n",
            ));
        }
        if used("molt_builtin_func(") {
            self.output.push_str(concat!(
                "fn molt_builtin_positional(args: &[MoltValue]) -> Vec<MoltValue> {\n",
                "    if let Some(MoltValue::List(v)) = args.first() {\n",
                "        let rest_is_none = args.get(1..).is_some_and(|rest| rest.iter().all(|x| matches!(x, MoltValue::None)));\n",
                "        if args.len() == 1 || rest_is_none {\n",
                "            return v.clone();\n",
                "        }\n",
                "    }\n",
                "    args.to_vec()\n",
                "}\n\n",
                "fn molt_builtin_func(name: &str) -> MoltValue {\n",
                "    let canonical = match name {\n",
                "        \"int\" => \"molt_int_builtin\",\n",
                "        \"float\" => \"molt_float_builtin\",\n",
                "        \"bool\" => \"molt_bool_builtin\",\n",
                "        \"str\" => \"molt_str_builtin\",\n",
                "        \"len\" => \"molt_len_builtin\",\n",
                "        \"min\" => \"molt_min_builtin\",\n",
                "        \"max\" => \"molt_max_builtin\",\n",
                "        \"sum\" => \"molt_sum_builtin\",\n",
                "        \"enumerate\" => \"molt_enumerate_builtin\",\n",
                "        \"zip\" => \"molt_zip_builtin\",\n",
                "        \"range\" => \"molt_range_builtin\",\n",
                "        other => other,\n",
                "    };\n",
                "    match canonical {\n",
                "        \"molt_function_init_metadata_packed\" | \"molt_function_set_defaults\" => MoltValue::Func(Arc::new(|_args: &mut Vec<MoltValue>| {\n",
                "            MoltValue::None\n",
                "        })),\n",
                "        \"molt_function_set_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            molt_builtin_positional(args).into_iter().next().unwrap_or(MoltValue::None)\n",
                "        })),\n",
                "        \"molt_int_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            MoltValue::Int(pos.first().map_or(0, molt_int))\n",
                "        })),\n",
                "        \"molt_float_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            MoltValue::Float(pos.first().map_or(0.0, molt_float))\n",
                "        })),\n",
                "        \"molt_bool_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            MoltValue::Bool(pos.first().is_some_and(molt_bool))\n",
                "        })),\n",
                "        \"molt_str_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            MoltValue::Str(pos.first().map_or_else(String::new, molt_str))\n",
                "        })),\n",
                "        \"molt_len_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            if let Some(v) = pos.first() {\n",
                "                match v {\n",
                "                    MoltValue::Str(s) => MoltValue::Int(s.chars().count() as i64),\n",
                "                    MoltValue::List(items) => MoltValue::Int(items.len() as i64),\n",
                "                    MoltValue::Dict(items) => MoltValue::Int(items.len() as i64),\n",
                "                    _ => MoltValue::Int(0),\n",
                "                }\n",
                "            } else {\n",
                "                MoltValue::Int(0)\n",
                "            }\n",
                "        })),\n",
                "        \"molt_min_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let mut pos = molt_builtin_positional(args);\n",
                "            if pos.len() == 1 { if let MoltValue::List(items) = &pos[0] { pos = items.clone(); } }\n",
                "            if pos.is_empty() {\n",
                "                return MoltValue::None;\n",
                "            }\n",
                "            let mut best = pos[0].clone();\n",
                "            for value in pos.into_iter().skip(1) {\n",
                "                if molt_int(&value) < molt_int(&best) {\n",
                "                    best = value;\n",
                "                }\n",
                "            }\n",
                "            best\n",
                "        })),\n",
                "        \"molt_max_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let mut pos = molt_builtin_positional(args);\n",
                "            if pos.len() == 1 { if let MoltValue::List(items) = &pos[0] { pos = items.clone(); } }\n",
                "            if pos.is_empty() {\n",
                "                return MoltValue::None;\n",
                "            }\n",
                "            let mut best = pos[0].clone();\n",
                "            for value in pos.into_iter().skip(1) {\n",
                "                if molt_int(&value) > molt_int(&best) {\n",
                "                    best = value;\n",
                "                }\n",
                "            }\n",
                "            best\n",
                "        })),\n",
                "        \"molt_sum_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            let mut acc = if pos.len() > 1 { pos[1].clone() } else { MoltValue::Int(0) };\n",
                "            if let Some(MoltValue::List(items)) = pos.first() {\n",
                "                for value in items {\n",
                "                    let next = molt_int(&acc).wrapping_add(molt_int(value));\n",
                "                    acc = MoltValue::Int(next);\n",
                "                }\n",
                "            }\n",
                "            acc\n",
                "        })),\n",
                "        \"molt_enumerate_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            if let Some(iterable) = pos.first() {\n",
                "                let start = pos.get(1).map_or(0, molt_int);\n",
                "                molt_enumerate(iterable, start)\n",
                "            } else {\n",
                "                MoltValue::List(vec![])\n",
                "            }\n",
                "        })),\n",
                "        \"molt_zip_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            if pos.len() >= 2 {\n",
                "                molt_zip(&pos[0], &pos[1])\n",
                "            } else {\n",
                "                MoltValue::List(vec![])\n",
                "            }\n",
                "        })),\n",
                "        \"molt_range_builtin\" => MoltValue::Func(Arc::new(|args: &mut Vec<MoltValue>| {\n",
                "            let pos = molt_builtin_positional(args);\n",
                "            let (start, stop, step) = match pos.len() {\n",
                "                0 => (0, 0, 1),\n",
                "                1 => (0, molt_int(&pos[0]), 1),\n",
                "                2 => (molt_int(&pos[0]), molt_int(&pos[1]), 1),\n",
                "                _ => (molt_int(&pos[0]), molt_int(&pos[1]), molt_int(&pos[2])),\n",
                "            };\n",
                "            molt_range(start, stop, step)\n",
                "        })),\n",
                "        _ => MoltValue::None,\n",
                "    }\n",
                "}\n\n",
            ));
        }

        // Math module shim
        if used("molt_math_") {
            self.output.push_str(concat!(
                "mod molt_math {\n",
                "    pub fn floor(x: f64) -> f64 { x.floor() }\n",
                "    pub fn ceil(x: f64) -> f64 { x.ceil() }\n",
                "    pub fn sqrt(x: f64) -> f64 { x.sqrt() }\n",
                "    pub fn abs_f(x: f64) -> f64 { x.abs() }\n",
                "    pub fn sin(x: f64) -> f64 { x.sin() }\n",
                "    pub fn cos(x: f64) -> f64 { x.cos() }\n",
                "    pub fn tan(x: f64) -> f64 { x.tan() }\n",
                "    pub fn log(x: f64) -> f64 { x.ln() }\n",
                "    pub fn log10(x: f64) -> f64 { x.log10() }\n",
                "    pub fn exp(x: f64) -> f64 { x.exp() }\n",
                "    pub fn atan2(y: f64, x: f64) -> f64 { y.atan2(x) }\n",
                "    pub const PI: f64 = std::f64::consts::PI;\n",
                "    pub const E: f64 = std::f64::consts::E;\n",
                "    pub const INF: f64 = f64::INFINITY;\n",
                "}\n\n",
            ));
        }
    }

    // ── Function emission ─────────────────────────────────────────────────────
}
