use crate::representation_plan::{ScalarKind, ScalarRepresentationPlan};
use crate::tir::passes::effects::simple_ir_has_static_module_class_binding_effect_proof;
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[cfg_attr(
    not(any(feature = "native-backend", feature = "llvm")),
    allow(dead_code)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum ReturnAliasSummary {
    Param(usize),
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "llvm")),
    allow(dead_code)
)]
fn alias_source_name<'a>(
    op: &'a OpIR,
    summaries: &BTreeMap<String, ReturnAliasSummary>,
) -> Option<&'a str> {
    match op.kind.as_str() {
        "copy" | "box" | "unbox" | "cast" | "widen" | "identity_alias" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str),
        "copy_var" | "load_var" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str)
            .or(op.var.as_deref()),
        "store_var" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str),
        "call" => {
            let callee = op.s_value.as_ref()?;
            let ReturnAliasSummary::Param(param_idx) = *summaries.get(callee)?;
            op.args
                .as_ref()
                .and_then(|args| args.get(param_idx))
                .map(String::as_str)
        }
        _ => None,
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "llvm")),
    allow(dead_code)
)]
fn compute_function_return_alias_summary(
    func: &FunctionIR,
    known: &BTreeMap<String, ReturnAliasSummary>,
) -> Option<ReturnAliasSummary> {
    let trace_alias = std::env::var("MOLT_DEBUG_RETURN_ALIAS").as_deref() == Ok("1");
    let mut alias_roots: BTreeMap<String, String> = BTreeMap::new();
    for param in &func.params {
        if param != "none" {
            alias_roots.insert(param.clone(), param.clone());
        }
    }

    for op in &func.ops {
        let logical_out = op.out.as_ref().or_else(|| {
            if op.kind == "store_var" {
                op.var.as_ref()
            } else {
                None
            }
        });
        let Some(out) = logical_out else {
            continue;
        };
        if out == "none" {
            continue;
        }
        if let Some(src) = alias_source_name(op, known) {
            let root = alias_roots
                .get(src)
                .cloned()
                .unwrap_or_else(|| src.to_string());
            alias_roots.insert(out.clone(), root);
            if trace_alias {
                eprintln!(
                    "[molt alias] func={} op={} out={} src={} root={}",
                    func.name, op.kind, out, src, alias_roots[out]
                );
            }
        }
    }

    let const_none_names: BTreeSet<&str> = func
        .ops
        .iter()
        .filter(|op| op.kind == "const_none")
        .filter_map(|op| op.out.as_deref())
        .collect();

    let mut summary: Option<ReturnAliasSummary> = None;
    let mut saw_ret = false;
    for (ret_idx, op) in func.ops.iter().enumerate() {
        match op.kind.as_str() {
            "ret" => {
                let ret_name = op.var.as_ref()?;
                if const_none_names.contains(ret_name.as_str()) {
                    let mut scan_idx = ret_idx;
                    let mut synthetic_raise_tail = false;
                    while scan_idx > 0 {
                        scan_idx -= 1;
                        let prev = &func.ops[scan_idx];
                        match prev.kind.as_str() {
                            "const_none" if prev.out.as_deref() == Some(ret_name.as_str()) => {}
                            "line" | "check_exception" => {}
                            "raise" => {
                                synthetic_raise_tail = true;
                                break;
                            }
                            _ => break,
                        }
                    }
                    if synthetic_raise_tail {
                        continue;
                    }
                }
                saw_ret = true;
                let root = alias_roots
                    .get(ret_name)
                    .cloned()
                    .unwrap_or_else(|| ret_name.clone());
                let param_idx = func.params.iter().position(|param| param == &root)?;
                if trace_alias {
                    eprintln!(
                        "[molt alias] func={} ret_name={} root={} param_idx={}",
                        func.name, ret_name, root, param_idx
                    );
                }
                let current = ReturnAliasSummary::Param(param_idx);
                match summary {
                    None => summary = Some(current),
                    Some(existing) if existing == current => {}
                    Some(_) => return None,
                }
            }
            "ret_void" => {}
            _ => {}
        }
    }

    saw_ret.then_some(summary).flatten()
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "llvm")),
    allow(dead_code)
)]
pub fn compute_return_alias_summaries(
    functions: &[FunctionIR],
) -> BTreeMap<String, ReturnAliasSummary> {
    let mut summaries: BTreeMap<String, ReturnAliasSummary> = BTreeMap::new();
    loop {
        let mut changed = false;
        for func in functions {
            let next = compute_function_return_alias_summary(func, &summaries);
            match next {
                Some(summary) => {
                    if summaries.get(&func.name).copied() != Some(summary) {
                        summaries.insert(func.name.clone(), summary);
                        changed = true;
                    }
                }
                None => {
                    if summaries.remove(&func.name).is_some() {
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return summaries;
        }
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn elide_dead_struct_allocs(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_STRUCT_ELIDE").is_ok() {
        return;
    }
    let mut remove = vec![false; func_ir.ops.len()];
    let alloc_kinds = ["alloc_class", "alloc_class_trusted", "alloc_class_static"];
    let allowed_use_kinds = [
        "store",
        "store_init",
        "guarded_field_set",
        "guarded_field_init",
        "object_set_class",
    ];

    let mut uses_by_name: BTreeMap<&str, Vec<(usize, usize, &str)>> = BTreeMap::new();
    for (use_idx, use_op) in func_ir.ops.iter().enumerate() {
        let Some(args) = use_op.args.as_ref() else {
            continue;
        };
        let kind = use_op.kind.as_str();
        for (pos, arg) in args.iter().enumerate() {
            uses_by_name
                .entry(arg.as_str())
                .or_default()
                .push((use_idx, pos, kind));
        }
    }

    for (idx, op) in func_ir.ops.iter().enumerate() {
        if !alloc_kinds.contains(&op.kind.as_str()) {
            continue;
        }
        let Some(out_name) = op.out.as_deref() else {
            continue;
        };
        let Some(uses) = uses_by_name.get(out_name) else {
            remove[idx] = true;
            continue;
        };
        let mut allowed = true;
        for &(_, pos, use_kind) in uses {
            if pos != 0 || !allowed_use_kinds.contains(&use_kind) {
                allowed = false;
                break;
            }
        }
        if allowed {
            remove[idx] = true;
            for &(use_idx, _, _) in uses {
                remove[use_idx] = true;
            }
        }
    }

    if remove.iter().any(|&flag| flag) {
        let mut new_ops = Vec::with_capacity(func_ir.ops.len());
        for (idx, op) in func_ir.ops.iter().enumerate() {
            if !remove[idx] {
                new_ops.push(op.clone());
            }
        }
        func_ir.ops = new_ops;
    }
}

/// Split-field read deforestation (the P-perf release-blocker fix).
///
/// A non-escaping `s.split(sep)[idx]` field that is consumed ONLY by read-only
/// string ops (`len(field)`, `ord(field[i])`, `field == const`) never needs to
/// MATERIALIZE as a heap `str` — the field bytes already live in the source
/// string. The frontend already deforests the OUT-OF-LINE cases
/// (`len`/`==`→`string_split_field_*`, `int`→`string_split_field_to_int`); this
/// SimpleIR pass is the IN-LINE complement: after the inliner splices a user
/// `parse_int(field)` (admitted by the split-field-enabled inliner gate), the
/// field flows into the inlined `len(text)` / `ord(text[i])` loop, which this
/// pass rewrites against the field's byte BOUNDS — eliminating the per-field
/// `alloc_string` that dominated the csv/etl ETL profiles.
///
/// Shape (the keystone that avoids the O(n²) per-char-split-rescan trap):
/// ```text
///   string_split_field(hay, sep, idx) -> field          ──┐ replaced by 3
///   ... len(field), ord_at(field, i), field == c ...      │ field-property
/// becomes:                                                 │ ops scanned
///   string_split_field_start   (hay, sep, idx) -> start  ──┤ ONCE here, then
///   string_split_field_end     (hay, sep, idx) -> end      │ O(1) bounds reads
///   string_split_field_is_ascii(hay, sep, idx) -> is_asc   │ per consumer:
///   check_exception                                        │
///   ... string_split_field_len_from_bounds(hay,start,end,is_asc)
///       string_split_field_ord_at_bounds(hay,start,end,is_asc,i)
///       string_split_field_eq(hay,sep,idx,c) ...         ──┘
/// ```
/// Fires ONLY when EVERY use of `field` is bounds-expressible (else the
/// materializing `string_split_field` stays — fail-closed, byte-identical).
/// Byte offsets / the ASCII flag are ordinary boxed ints, so nothing about the
/// representation plumbing changes; the three property ops raise the SAME
/// `IndexError` the materializing path would (caught by the inserted
/// `check_exception`).
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn deforest_split_field_reads(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_SPLIT_FIELD_DEFOREST").is_ok() {
        return;
    }
    // Pass 1: map every value name to the (op_index, arg_position, kind) of each
    // op that reads it. A use at arg position != 0 of len/ord_at, or in any other
    // op kind, makes the field non-bounds-expressible.
    struct Use {
        op_index: usize,
        arg_pos: usize,
        kind_is_len: bool,
        kind_is_ord_at: bool,
        kind_is_eq: bool,
        kind_is_typeguard: bool,
    }
    let mut uses_by_name: BTreeMap<String, Vec<Use>> = BTreeMap::new();
    for (op_index, op) in func_ir.ops.iter().enumerate() {
        let Some(args) = op.args.as_ref() else {
            continue;
        };
        let kind = op.kind.as_str();
        let (is_len, is_ord_at, is_eq) = (kind == "len", kind == "ord_at", kind == "eq");
        // A `guard_tag` / `guard_type` asserting the field's type is redundant
        // (a `string_split_field` provably yields a `str` or raises first), so it
        // is a bounds-expressible use that the rewrite simply DROPS.
        let is_typeguard = kind == "guard_tag" || kind == "guard_type";
        for (arg_pos, arg) in args.iter().enumerate() {
            uses_by_name.entry(arg.clone()).or_default().push(Use {
                op_index,
                arg_pos,
                kind_is_len: is_len,
                kind_is_ord_at: is_ord_at,
                kind_is_eq: is_eq,
                kind_is_typeguard: is_typeguard,
            });
        }
    }
    // A value name that also appears as a non-arg reference (the `var` field of a
    // store/load/copy, an `out` aliasing — none apply to a fresh split field, but
    // be defensive) is excluded by simply not finding it bounds-expressible below.

    // Const-string set: an `eq` against a constant string can be expressed as the
    // existing zero-alloc `string_split_field_eq`. (Mirrors the frontend fusion's
    // const tracking; here we only need to know the *other* eq operand is const.)
    let mut const_strings: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for op in &func_ir.ops {
        if op.kind == "const_str"
            && let Some(out) = op.out.as_deref()
        {
            const_strings.insert(out);
        }
    }

    // Pass 2: find each `string_split_field` whose result is used ONLY by
    // bounds-expressible reads. Record the rewrite plan.
    struct Plan {
        field_op_index: usize,
        hay: String,
        sep: String,
        idx: String,
        field: String,
        // (op_index, kind) of each consumer to rewrite.
        len_ops: Vec<usize>,
        ord_ops: Vec<usize>,
        eq_ops: Vec<usize>,
        // Redundant type-guard ops on the field — dropped by the rewrite.
        guard_ops: Vec<usize>,
    }
    let mut plans: Vec<Plan> = Vec::new();
    for (field_op_index, op) in func_ir.ops.iter().enumerate() {
        if op.kind != "string_split_field" {
            continue;
        }
        let Some(args) = op.args.as_ref() else {
            continue;
        };
        if args.len() != 3 {
            continue;
        }
        let Some(field) = op.out.as_deref() else {
            continue;
        };
        // The three property ops we splice in place of `string_split_field` can
        // raise `IndexError` (out-of-range field index) exactly as it does. We
        // rely on the EXISTING `check_exception` the frontend always emits right
        // after the field access to catch it. Only deforest when that invariant
        // holds for THIS op (fail-closed: keep materializing otherwise — the
        // materializing path's own check_exception is then unchanged).
        let followed_by_check = func_ir
            .ops
            .get(field_op_index + 1)
            .is_some_and(|next| next.kind == "check_exception");
        if !followed_by_check {
            continue;
        }
        let Some(uses) = uses_by_name.get(field) else {
            // No uses → DCE will drop it; leave for the generic path.
            continue;
        };
        let mut len_ops = Vec::new();
        let mut ord_ops = Vec::new();
        let mut eq_ops = Vec::new();
        let mut guard_ops = Vec::new();
        let mut all_bounds_expressible = true;
        for u in uses {
            if u.kind_is_len && u.arg_pos == 0 {
                len_ops.push(u.op_index);
            } else if u.kind_is_ord_at && u.arg_pos == 0 {
                ord_ops.push(u.op_index);
            } else if u.kind_is_typeguard && u.arg_pos == 0 {
                guard_ops.push(u.op_index);
            } else if u.kind_is_eq {
                // field == other: expressible iff the OTHER operand is a const
                // string (so `string_split_field_eq` applies).
                let eq_op = &func_ir.ops[u.op_index];
                let eq_args = eq_op.args.as_deref().unwrap_or(&[]);
                let other_is_const = eq_args.len() == 2
                    && eq_args
                        .iter()
                        .enumerate()
                        .any(|(p, a)| p != u.arg_pos && const_strings.contains(a.as_str()));
                if other_is_const {
                    eq_ops.push(u.op_index);
                } else {
                    all_bounds_expressible = false;
                    break;
                }
            } else {
                all_bounds_expressible = false;
                break;
            }
        }
        if !all_bounds_expressible {
            continue;
        }
        plans.push(Plan {
            field_op_index,
            hay: args[0].clone(),
            sep: args[1].clone(),
            idx: args[2].clone(),
            field: field.to_string(),
            len_ops,
            ord_ops,
            eq_ops,
            guard_ops,
        });
    }

    if plans.is_empty() {
        return;
    }

    // Pass 3: apply. Rewrite consumer ops in place; replace each field op with the
    // three property ops + a check_exception (assembled as a multi-op splice).
    // Build a replacement map: op_index -> Vec<OpIR> (1:N). All other ops copy
    // through. Fresh property-result names derive from the field name (unique per
    // field op since the field var is itself unique).
    let mut splice: BTreeMap<usize, Vec<OpIR>> = BTreeMap::new();
    let mut remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for plan in &plans {
        let start_var = format!("{}__sfstart", plan.field);
        let end_var = format!("{}__sfend", plan.field);
        let asc_var = format!("{}__sfasc", plan.field);

        // Drop the redundant type-guard ops (the field is a `str` by
        // construction; the guard would only fire on an impossible non-str).
        for &i in &plan.guard_ops {
            remove.insert(i);
        }

        // Rewrite len(field) -> string_split_field_len_from_bounds.
        for &i in &plan.len_ops {
            let span = (func_ir.ops[i].col_offset, func_ir.ops[i].end_col_offset);
            func_ir.ops[i] = OpIR {
                kind: "string_split_field_len_from_bounds".to_string(),
                args: Some(vec![
                    plan.hay.clone(),
                    start_var.clone(),
                    end_var.clone(),
                    asc_var.clone(),
                ]),
                out: func_ir.ops[i].out.clone(),
                col_offset: span.0,
                end_col_offset: span.1,
                ..OpIR::default()
            };
        }
        // Rewrite ord_at(field, i) -> string_split_field_ord_at_bounds.
        for &i in &plan.ord_ops {
            let char_idx = func_ir.ops[i]
                .args
                .as_ref()
                .and_then(|a| a.get(1).cloned())
                .unwrap_or_default();
            let span = (func_ir.ops[i].col_offset, func_ir.ops[i].end_col_offset);
            func_ir.ops[i] = OpIR {
                kind: "string_split_field_ord_at_bounds".to_string(),
                args: Some(vec![
                    plan.hay.clone(),
                    start_var.clone(),
                    end_var.clone(),
                    asc_var.clone(),
                    char_idx,
                ]),
                out: func_ir.ops[i].out.clone(),
                col_offset: span.0,
                end_col_offset: span.1,
                ..OpIR::default()
            };
        }
        // Rewrite field == const -> string_split_field_eq(hay, sep, idx, const).
        for &i in &plan.eq_ops {
            let eq_args = func_ir.ops[i].args.clone().unwrap_or_default();
            // The const operand is the one that is NOT the field.
            let const_operand = eq_args
                .iter()
                .find(|a| a.as_str() != plan.field)
                .cloned()
                .unwrap_or_default();
            let span = (func_ir.ops[i].col_offset, func_ir.ops[i].end_col_offset);
            func_ir.ops[i] = OpIR {
                kind: "string_split_field_eq".to_string(),
                args: Some(vec![
                    plan.hay.clone(),
                    plan.sep.clone(),
                    plan.idx.clone(),
                    const_operand,
                ]),
                out: func_ir.ops[i].out.clone(),
                col_offset: span.0,
                end_col_offset: span.1,
                ..OpIR::default()
            };
        }

        // Replace the field op with start/end/is_ascii + check_exception.
        let span = (
            func_ir.ops[plan.field_op_index].col_offset,
            func_ir.ops[plan.field_op_index].end_col_offset,
        );
        let three_args = vec![plan.hay.clone(), plan.sep.clone(), plan.idx.clone()];
        let mk = |kind: &str, out: &str| OpIR {
            kind: kind.to_string(),
            args: Some(three_args.clone()),
            out: Some(out.to_string()),
            col_offset: span.0,
            end_col_offset: span.1,
            ..OpIR::default()
        };
        // No fresh `check_exception` is appended: the `string_split_field` we
        // replace was ALWAYS immediately followed by the frontend's own
        // `check_exception` (it can raise IndexError), which stays in place and
        // now catches the three property ops' identical IndexError. The property
        // ops short-circuit once the exception is pending (see
        // `split_field_bounds_bytes`), so only the FIRST raises and the trailing
        // ops are no-ops on the error path.
        splice.insert(
            plan.field_op_index,
            vec![
                mk("string_split_field_start", &start_var),
                mk("string_split_field_end", &end_var),
                mk("string_split_field_is_ascii", &asc_var),
            ],
        );
    }

    let mut new_ops = Vec::with_capacity(func_ir.ops.len() + plans.len() * 3);
    for (idx, op) in func_ir.ops.drain(..).enumerate() {
        if let Some(replacement) = splice.remove(&idx) {
            new_ops.extend(replacement);
        } else if !remove.contains(&idx) {
            new_ops.push(op);
        }
    }
    func_ir.ops = new_ops;
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn apply_profile_order(ir: &mut SimpleIR) {
    let Some(profile) = ir.profile.as_ref() else {
        return;
    };
    if profile.hot_functions.is_empty() {
        return;
    }
    let mut ranks: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, name) in profile.hot_functions.iter().enumerate() {
        ranks.entry(name.clone()).or_insert(idx);
    }
    let mut original: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, func) in ir.functions.iter().enumerate() {
        original.entry(func.name.clone()).or_insert(idx);
    }
    ir.functions.sort_by(|left, right| {
        let left_rank = ranks.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_rank = ranks.get(&right.name).copied().unwrap_or(usize::MAX);
        if left_rank != right_rank {
            return left_rank.cmp(&right_rank);
        }
        let left_idx = original.get(&left.name).copied().unwrap_or(usize::MAX);
        let right_idx = original.get(&right.name).copied().unwrap_or(usize::MAX);
        left_idx
            .cmp(&right_idx)
            .then_with(|| left.name.cmp(&right.name))
    });
}

// ---------------------------------------------------------------------------
// Constant folding pass (peephole, pre-emission)
//
// Scans IR ops in forward order, tracking which variables hold known constant
// values. When an integer arithmetic op's inputs are both known integer
// constants, the op is replaced with a `const` op holding the computed result.
// This eliminates redundant unbox-compute-box sequences in the emitted code,
// yielding a 3-5% binary size reduction on constant-heavy code.
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn fold_constants(ops: &mut [OpIR]) {
    // Map from variable name -> known constant integer value (raw, unboxed).
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();
    // Map from variable name -> known constant boolean value.
    let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();

    for op in ops.iter_mut() {
        match op.kind.as_str() {
            "const" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), val);
                }
            }
            "const_bool" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_bools.insert(out.clone(), val != 0);
                }
            }

            // Binary integer arithmetic: add, sub, mul, inplace_add, inplace_sub, inplace_mul
            "add" | "sub" | "mul" | "inplace_add" | "inplace_sub" | "inplace_mul" => {
                if let Some(ref args) = op.args
                    && args.len() == 2
                {
                    let a_val = const_ints.get(&args[0]).copied();
                    let b_val = const_ints.get(&args[1]).copied();
                    if let (Some(a), Some(b)) = (a_val, b_val) {
                        let result = match op.kind.as_str() {
                            "add" | "inplace_add" => a.wrapping_add(b),
                            "sub" | "inplace_sub" => a.wrapping_sub(b),
                            "mul" | "inplace_mul" => a.wrapping_mul(b),
                            _ => unreachable!(),
                        };
                        op.kind = "const".to_string();
                        op.value = Some(result);
                        op.args = None;
                        if let Some(ref out) = op.out {
                            const_ints.insert(out.clone(), result);
                        }
                        continue;
                    }
                }
                // Output variable is no longer a known constant.
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Bitwise integer ops: bit_and, bit_or, bit_xor and inplace variants
            "bit_and" | "bit_or" | "bit_xor" | "inplace_bit_and" | "inplace_bit_or"
            | "inplace_bit_xor" => {
                if let Some(ref args) = op.args
                    && args.len() == 2
                {
                    let a_val = const_ints.get(&args[0]).copied();
                    let b_val = const_ints.get(&args[1]).copied();
                    if let (Some(a), Some(b)) = (a_val, b_val) {
                        let result = match op.kind.as_str() {
                            "bit_and" | "inplace_bit_and" => a & b,
                            "bit_or" | "inplace_bit_or" => a | b,
                            "bit_xor" | "inplace_bit_xor" => a ^ b,
                            _ => unreachable!(),
                        };
                        op.kind = "const".to_string();
                        op.value = Some(result);
                        op.args = None;
                        if let Some(ref out) = op.out {
                            const_ints.insert(out.clone(), result);
                        }
                        continue;
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Boolean not: `not` on a known bool constant.
            "not" => {
                if let Some(ref args) = op.args
                    && args.len() == 1
                    && let Some(&val) = const_bools.get(&args[0])
                {
                    let result = !val;
                    op.kind = "const_bool".to_string();
                    op.value = Some(if result { 1 } else { 0 });
                    op.args = None;
                    if let Some(ref out) = op.out {
                        const_bools.insert(out.clone(), result);
                        const_ints.remove(out);
                    }
                    continue;
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // Control flow boundaries: clear all tracked constants.
            "if" | "else" | "end_if" | "loop_start" | "loop_end" | "try_start" | "try_end"
            | "jump" | "label" | "state_switch" => {
                const_ints.clear();
                const_bools.clear();
            }

            // Any other op that writes an output kills the constant for that variable.
            _ => {
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cross-block constant propagation pass
//
// Extends fold_constants with dominator-aware constant tracking across
// structured control flow (if / else / end_if).  Constants defined before
// a branch are available in both arms.  At merge points (end_if), only
// constants that agree in both arms survive.
//
// For unstructured control flow (loops, try/except, jumps, labels) we
// conservatively clear all tracked constants, same as the intra-block pass.
//
// This pass fully subsumes fold_constants — it performs the same peephole
// arithmetic folding AND propagates constants across basic block boundaries.
// ---------------------------------------------------------------------------

/// Saved constant state at a control-flow split point.
#[allow(dead_code)]
struct BranchSnapshot {
    /// Constants known at the point just before the `if` op.
    pre_ints: BTreeMap<String, i64>,
    pre_bools: BTreeMap<String, bool>,
    /// Constants accumulated in the *then* arm (captured when we hit `else`).
    then_ints: Option<BTreeMap<String, i64>>,
    then_bools: Option<BTreeMap<String, bool>>,
}

#[allow(dead_code)]
pub fn fold_constants_cross_block(ops: &mut [OpIR]) {
    let mut const_ints: BTreeMap<String, i64> = BTreeMap::new();
    let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();

    // Stack of snapshots for nested if/else/end_if.
    let mut branch_stack: Vec<BranchSnapshot> = Vec::new();

    for op in ops.iter_mut() {
        match op.kind.as_str() {
            // ----- constant definitions -----
            "const" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_ints.insert(out.clone(), val);
                }
            }
            "const_bool" => {
                if let (Some(out), Some(val)) = (op.out.as_ref(), op.value) {
                    const_bools.insert(out.clone(), val != 0);
                }
            }

            // ----- binary integer arithmetic -----
            "add" | "sub" | "mul" | "inplace_add" | "inplace_sub" | "inplace_mul" => {
                if let Some(ref args) = op.args
                    && args.len() == 2
                {
                    let a_val = const_ints.get(&args[0]).copied();
                    let b_val = const_ints.get(&args[1]).copied();
                    if let (Some(a), Some(b)) = (a_val, b_val) {
                        let result = match op.kind.as_str() {
                            "add" | "inplace_add" => a.wrapping_add(b),
                            "sub" | "inplace_sub" => a.wrapping_sub(b),
                            "mul" | "inplace_mul" => a.wrapping_mul(b),
                            _ => unreachable!(),
                        };
                        op.kind = "const".to_string();
                        op.value = Some(result);
                        op.args = None;
                        if let Some(ref out) = op.out {
                            const_ints.insert(out.clone(), result);
                        }
                        continue;
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // ----- bitwise integer ops -----
            "bit_and" | "bit_or" | "bit_xor" | "inplace_bit_and" | "inplace_bit_or"
            | "inplace_bit_xor" => {
                if let Some(ref args) = op.args
                    && args.len() == 2
                {
                    let a_val = const_ints.get(&args[0]).copied();
                    let b_val = const_ints.get(&args[1]).copied();
                    if let (Some(a), Some(b)) = (a_val, b_val) {
                        let result = match op.kind.as_str() {
                            "bit_and" | "inplace_bit_and" => a & b,
                            "bit_or" | "inplace_bit_or" => a | b,
                            "bit_xor" | "inplace_bit_xor" => a ^ b,
                            _ => unreachable!(),
                        };
                        op.kind = "const".to_string();
                        op.value = Some(result);
                        op.args = None;
                        if let Some(ref out) = op.out {
                            const_ints.insert(out.clone(), result);
                        }
                        continue;
                    }
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // ----- boolean not -----
            "not" => {
                if let Some(ref args) = op.args
                    && args.len() == 1
                    && let Some(&val) = const_bools.get(&args[0])
                {
                    let result = !val;
                    op.kind = "const_bool".to_string();
                    op.value = Some(if result { 1 } else { 0 });
                    op.args = None;
                    if let Some(ref out) = op.out {
                        const_bools.insert(out.clone(), result);
                        const_ints.remove(out);
                    }
                    continue;
                }
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }

            // ----- structured control flow: if / else / end_if -----
            "if" => {
                branch_stack.push(BranchSnapshot {
                    pre_ints: const_ints.clone(),
                    pre_bools: const_bools.clone(),
                    then_ints: None,
                    then_bools: None,
                });
            }
            "else" => {
                if let Some(snapshot) = branch_stack.last_mut() {
                    snapshot.then_ints = Some(const_ints.clone());
                    snapshot.then_bools = Some(const_bools.clone());
                    const_ints = snapshot.pre_ints.clone();
                    const_bools = snapshot.pre_bools.clone();
                } else {
                    const_ints.clear();
                    const_bools.clear();
                }
            }
            "end_if" => {
                if let Some(snapshot) = branch_stack.pop() {
                    if let (Some(then_ints), Some(then_bools)) =
                        (snapshot.then_ints, snapshot.then_bools)
                    {
                        let else_ints = const_ints;
                        let else_bools = const_bools;

                        let mut merged_ints = BTreeMap::new();
                        for (name, then_val) in &then_ints {
                            if let Some(&else_val) = else_ints.get(name)
                                && then_val == &else_val
                            {
                                merged_ints.insert(name.clone(), *then_val);
                            }
                        }

                        let mut merged_bools = BTreeMap::new();
                        for (name, then_val) in &then_bools {
                            if let Some(&else_val) = else_bools.get(name)
                                && then_val == &else_val
                            {
                                merged_bools.insert(name.clone(), *then_val);
                            }
                        }

                        const_ints = merged_ints;
                        const_bools = merged_bools;
                    } else {
                        let then_ints = const_ints;
                        let then_bools = const_bools;

                        let mut merged_ints = BTreeMap::new();
                        for (name, pre_val) in &snapshot.pre_ints {
                            if let Some(&then_val) = then_ints.get(name)
                                && pre_val == &then_val
                            {
                                merged_ints.insert(name.clone(), *pre_val);
                            }
                        }

                        let mut merged_bools = BTreeMap::new();
                        for (name, pre_val) in &snapshot.pre_bools {
                            if let Some(&then_val) = then_bools.get(name)
                                && pre_val == &then_val
                            {
                                merged_bools.insert(name.clone(), *pre_val);
                            }
                        }

                        const_ints = merged_ints;
                        const_bools = merged_bools;
                    }
                } else {
                    const_ints.clear();
                    const_bools.clear();
                }
            }

            // ----- unstructured / opaque control flow: conservative clear -----
            "loop_start" | "loop_end" | "try_start" | "try_end" | "jump" | "label"
            | "state_switch" => {
                const_ints.clear();
                const_bools.clear();
                branch_stack.clear();
            }

            // ----- default: kill the output variable -----
            _ => {
                if let Some(ref out) = op.out {
                    const_ints.remove(out);
                    const_bools.remove(out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Escape analysis pass
//
// Scans the IR op stream for short-lived object allocations (tuple_new,
// list_new, dict_new) and determines whether the resulting object "escapes"
// the current function.  An allocation escapes if its result variable is:
//
//   - Returned from the function (ret)
//   - Passed to a function call (call, call_internal, call_method, etc.)
//   - Stored to a non-local / global / attribute / closure variable
//   - Stored into another object (store_index, dict_set, list_append, etc.)
//   - Used by yield / yield_from / await
//
// If an allocation does NOT escape, it is marked `stack_eligible = true`,
// signalling the native backend that it may use a stack slot instead of a
// heap allocation.  The primary beneficiary is the (value, done) tuple from
// `iter_next`, which is created on every loop iteration, immediately
// destructured via `index`, and never referenced again.
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn escape_analysis(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_ESCAPE_ANALYSIS").is_ok() {
        return;
    }

    // Allocation op kinds eligible for stack promotion.
    let alloc_kinds = ["tuple_new", "list_new", "dict_new"];

    // Op kinds where any argument reference is a "safe" (non-escaping) use.
    // The object is consumed locally — read-only or iteration.
    let safe_use_kinds: BTreeSet<&str> = [
        "index",        // subscript / destructure
        "len",          // len() intrinsic
        "type",         // type() intrinsic
        "is",           // identity check
        "is_not",       // identity check
        "bool_test",    // truthiness test
        "iter",         // create an iterator (reads the container)
        "contains",     // `in` operator
        "not_contains", // `not in` operator
        "unpack",       // tuple unpacking (reads elements)
        "unpack_ex",    // star unpacking
        "compare",      // comparison
        "copy",         // local alias — tracked transitively below
    ]
    .iter()
    .copied()
    .collect();

    // Op kinds that definitely cause escape for any argument.
    let escaping_ops: BTreeSet<&str> = [
        "ret",
        "call",
        "call_internal",
        "call_method",
        "call_method_ic",
        "call_super_method_ic",
        "call_function_ex",
        "call_intrinsic",
        "store_global",
        "store_nonlocal",
        "store_attr",
        "store_index",
        "store_closure",
        "dict_set",
        "list_append",
        "list_extend",
        "set_add",
        "yield",
        "yield_from",
        "await",
        "raise",
        "store",
        "store_init",
        "guarded_field_set",
        "guarded_field_init",
        "object_set_class",
    ]
    .iter()
    .copied()
    .collect();

    // Phase 1: Collect all allocation sites.
    // Map from output variable name (owned) → op index.
    let mut alloc_sites: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, op) in func_ir.ops.iter().enumerate() {
        if alloc_kinds.contains(&op.kind.as_str())
            && let Some(ref out) = op.out
        {
            alloc_sites.insert(out.clone(), idx);
        }
    }

    if alloc_sites.is_empty() {
        return;
    }

    // Phase 2: Build a use-list for each allocation.
    // Track which alloc vars escape.
    let mut escaped: BTreeSet<String> = BTreeSet::new();
    // Track copy aliases: if `copy x -> y`, then y is an alias for x's alloc.
    // Maps alias name → root alloc name.
    let mut alias_to_alloc: BTreeMap<String, String> = BTreeMap::new();
    // Initialize: each alloc name maps to itself.
    for name in alloc_sites.keys() {
        alias_to_alloc.insert(name.clone(), name.clone());
    }

    // Forward scan: resolve copy aliases and check uses.
    for op in func_ir.ops.iter() {
        let kind = op.kind.as_str();

        // Handle copy aliases: if source is a tracked alloc, propagate.
        if kind == "copy" {
            if let (Some(args), Some(out)) = (&op.args, &op.out)
                && args.len() == 1
                && let Some(root) = alias_to_alloc.get(&args[0]).cloned()
            {
                alias_to_alloc.insert(out.clone(), root);
            }
            continue;
        }

        // Check arguments of this op.
        if let Some(ref args) = op.args {
            for arg in args {
                let root = match alias_to_alloc.get(arg).cloned() {
                    Some(r) => r,
                    None => continue,
                };
                if escaped.contains(&root) {
                    continue; // already known to escape
                }

                if safe_use_kinds.contains(kind) {
                    continue; // non-escaping use
                }

                if escaping_ops.contains(kind) {
                    escaped.insert(root);
                    continue;
                }

                // Conservative: unknown op → assume escape.
                escaped.insert(root);
            }
        }

        // Also check `var` field (used by ret and some other ops).
        if let Some(ref var) = op.var
            && let Some(root) = alias_to_alloc.get(var).cloned()
            && (kind == "ret" || escaping_ops.contains(kind))
        {
            escaped.insert(root);
        }
    }

    // Phase 3: Mark non-escaping allocations as stack-eligible.
    for (name, idx) in &alloc_sites {
        if !escaped.contains(name) {
            func_ir.ops[*idx].stack_eligible = Some(true);
        }
    }
}

// ---------------------------------------------------------------------------
// Pre-built constant integer map for O(1) lookups during compilation.
//
// Scans all ops once and records the first `const` definition for each
// variable name. This replaces any backward scan pattern (O(n) per lookup)
// with a single O(n) build step + O(log n) BTreeMap lookups.
//
// Only the first definition is stored, which is correct for SSA-like
// variable naming where each name is defined exactly once.
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn build_const_int_map(ops: &[OpIR]) -> BTreeMap<String, i64> {
    let mut map = BTreeMap::new();
    for op in ops {
        if op.kind == "const"
            && let (Some(out), Some(val)) = (op.out.as_ref(), op.value)
        {
            // Only store the first definition (SSA correctness).
            map.entry(out.clone()).or_insert(val);
        }
    }
    map
}

/// Identify pairs of `inc_ref`/`dec_ref` ops that cancel within a basic block.
/// Returns: (set of op indices to skip, set of variable names whose dec_ref to skip).
pub fn compute_rc_coalesce_skips(
    ops: &[OpIR],
    last_use: &BTreeMap<String, usize>,
) -> (HashSet<usize>, HashSet<String>) {
    const CONTROL_FLOW: &[&str] = &[
        "if",
        "else",
        "end_if",
        "jump",
        "br_if",
        "label",
        "check_exception",
        "state_transition",
        "state_yield",
        "state_switch",
        "state_label",
        "exception_push",
        "exception_pop",
        "chan_send_yield",
        "chan_recv_yield",
        "ret",
        "ret_void",
        "loop_start",
        "loop_index_start",
        "loop_end",
        "loop_break_if_true",
        "loop_break_if_false",
        // Value-less conditional break gated on the runtime exception flag.  It
        // is a real control-flow boundary (the loop may exit here on a pending
        // exception), so inc_ref/dec_ref coalescing MUST NOT scan across it —
        // otherwise a dec_ref placed after the break could be skipped on the
        // exception-exit path, leaking the referenced object.
        "loop_break_if_exception",
        "loop_continue",
    ];
    let cf_set: HashSet<&str> = CONTROL_FLOW.iter().copied().collect();
    let mut skip_ops: HashSet<usize> = HashSet::new();
    let mut skip_dec_ref: HashSet<String> = HashSet::new();

    for i in 0..ops.len() {
        if skip_ops.contains(&i) {
            continue;
        }
        let a = &ops[i];
        let a_is_inc = matches!(a.kind.as_str(), "inc_ref" | "borrow");
        let a_is_dec = matches!(a.kind.as_str(), "dec_ref" | "release");
        if !a_is_inc && !a_is_dec {
            continue;
        }
        let a_arg = match a.args.as_ref().and_then(|v| v.first()) {
            Some(name) => name.clone(),
            None => continue,
        };
        for j in (i + 1)..ops.len() {
            let b = &ops[j];
            if cf_set.contains(b.kind.as_str()) {
                break;
            }
            let b_kind = b.kind.as_str();
            let b_arg = b.args.as_ref().and_then(|v| v.first());
            let is_match = if a_is_inc {
                matches!(b_kind, "dec_ref" | "release") && b_arg.map(String::as_str) == Some(&a_arg)
            } else {
                matches!(b_kind, "inc_ref" | "borrow") && b_arg.map(String::as_str) == Some(&a_arg)
            };
            if is_match && !skip_ops.contains(&j) {
                skip_ops.insert(i);
                skip_ops.insert(j);
                break;
            }
            let uses_var = b
                .args
                .as_ref()
                .map(|args| args.iter().any(|n| n == &a_arg))
                .unwrap_or(false)
                || b.var.as_ref().map(|v| v == &a_arg).unwrap_or(false)
                || b.out.as_ref().map(|o| o == &a_arg).unwrap_or(false);
            if uses_var {
                break;
            }
        }
    }

    for (idx, op) in ops.iter().enumerate() {
        if skip_ops.contains(&idx) {
            continue;
        }
        if !matches!(op.kind.as_str(), "inc_ref" | "borrow") {
            continue;
        }
        let out_name = match op.out.as_deref() {
            Some(name) if name != "none" => name,
            _ => continue,
        };
        // If the variable appears in last_use, check if its final use is at or
        // before this inc_ref — that means the inc_ref output is dead. If the
        // variable is completely absent from last_use (never used anywhere),
        // the inc_ref is also dead. We explicitly distinguish these cases to
        // avoid silently eliding an inc_ref due to variable name mismatches.
        let is_dead = match last_use.get(out_name) {
            Some(&last) => last <= idx,
            None => true, // Variable never used after definition — dead inc_ref.
        };
        if is_dead {
            skip_ops.insert(idx);
            skip_dec_ref.insert(out_name.to_string());
        }
    }

    (skip_ops, skip_dec_ref)
}

/// Build a last-use map: for each variable name, the index of the last op that
/// references it (via `var`, `args`, or `out`).
fn build_last_use_map(ops: &[OpIR]) -> BTreeMap<String, usize> {
    let mut last_use = BTreeMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let Some(var) = &op.var
            && var != "none"
        {
            last_use.insert(var.clone(), i);
        }
        if let Some(args) = &op.args {
            for name in args {
                if name != "none" {
                    last_use.insert(name.clone(), i);
                }
            }
        }
        if let Some(out) = &op.out
            && out != "none"
        {
            last_use.insert(out.clone(), i);
        }
    }
    last_use
}

/// RC coalescing pass: eliminate redundant `inc_ref`/`dec_ref` pairs within
/// basic blocks.  When an `inc_ref(x)` is followed by `dec_ref(x)` (or vice
/// versa) with no intervening store, call, control-flow, or other use that
/// could observe the refcount, the pair is removed.  Also removes trailing
/// `inc_ref` ops whose output is never used (the corresponding `dec_ref` at
/// function exit is skipped as well).
///
/// This is the IR-level counterpart of `compute_rc_coalesce_skips`, which is
/// applied at codegen time.  Running it as an early pass shrinks the op stream
/// for all downstream analyses and backends.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn rc_coalescing(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_RC_COALESCE").is_ok() {
        return;
    }

    let last_use = build_last_use_map(&func_ir.ops);
    let (skip_ops, skip_dec_ref) = compute_rc_coalesce_skips(&func_ir.ops, &last_use);

    if skip_ops.is_empty() && skip_dec_ref.is_empty() {
        return;
    }

    let mut new_ops = Vec::with_capacity(func_ir.ops.len());
    for (idx, op) in func_ir.ops.iter().enumerate() {
        // Skip ops identified as redundant inc_ref/dec_ref pairs by index.
        if skip_ops.contains(&idx) {
            continue;
        }
        // Skip dec_ref/release ops whose variable was flagged by the
        // dead-inc_ref analysis (the inc_ref was removed, so the dec_ref
        // must be removed too).
        if matches!(op.kind.as_str(), "dec_ref" | "release")
            && let Some(arg) = op.args.as_ref().and_then(|a| a.first())
            && skip_dec_ref.contains(arg.as_str())
        {
            continue;
        }
        new_ops.push(op.clone());
    }
    func_ir.ops = new_ops;
}

// ---------------------------------------------------------------------------
// Loop-Invariant Code Motion (LICM)
// ---------------------------------------------------------------------------

const HOISTABLE_OPS: &[&str] = &[
    "const",
    "const_int",
    "const_float",
    "const_str",
    "const_bool",
    "const_none",
    "const_bytes",
    "list_new",
    "tuple_new",
];

fn is_hoistable(op: &OpIR) -> bool {
    let kind = op.kind.as_str();
    // list_new/tuple_new/dict_new allocate fresh heap objects.
    // Hoisting them out of loops causes ONE allocation to be shared across
    // all iterations, leading to aliasing corruption if the object is mutated.
    if matches!(kind, "list_new" | "tuple_new" | "dict_new" | "set_new") {
        return false;
    }
    HOISTABLE_OPS.contains(&kind)
}

/// Eliminate `check_exception` ops that follow operations known to never
/// raise exceptions. This reduces branch overhead in tight inner loops
/// (e.g., fib: 10 checks/call -> fewer).
///
/// Safe-to-elide predecessors: inc_ref, dec_ref, dec_ref_obj, const_int,
/// const_float, const_bool, const_none, nop, line.
/// Eliminate UnboundLocalError check sequences from the SimpleIR.
///
/// The frontend emits a `missing` + `is(var, missing)` + `br_if` +
/// `raise UnboundLocalError` guard for every local variable access.
/// In type-annotated functions and most generated code, variables are
/// always initialized before use, making these checks pure dead weight.
///
/// Each check sequence is ~11 ops and involves two function calls
/// (`molt_missing`, `molt_is`). In a tight inner loop like mandelbrot
/// with 12 variable accesses, this adds ~132 ops + 24 function calls
/// per iteration on top of the ~12 actual computation ops.
///
/// The pattern matched (with optional nop gaps):
///
/// ```text
/// [missing]       out=M
/// [is]            out=R  args=[V, M]
/// [jump]          val=L1
/// [label]         val=L1
/// [br_if]         args=[R]  val=L_raise
/// [jump]          val=L_ok
/// [label]         val=L_raise
/// [tuple_new]     sval="cannot access local variable ..."
/// [exception_new] / [exception_new_builtin] for "UnboundLocalError"
/// [raise]
/// [label]         val=L_ok
/// ```
///
/// This pass removes the entire sequence and any preceding nop,
/// leaving only the final continuation label intact.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn eliminate_unbound_local_checks(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_UNBOUND_ELIM").is_ok() {
        return;
    }
    let ops = &func_ir.ops;
    let len = ops.len();
    if len < 11 {
        return;
    }

    // Collect output names of all `missing` ops for fast lookup.
    let missing_outputs: HashSet<&str> = ops
        .iter()
        .filter(|op| op.kind == "missing")
        .filter_map(|op| op.out.as_deref())
        .collect();

    if missing_outputs.is_empty() {
        return;
    }

    // Pre-build a set of const_str names whose value is "UnboundLocalError"
    // to avoid rescanning the entire ops array for every match.
    let unbound_error_names: HashSet<&str> = ops
        .iter()
        .filter(|op| op.kind == "const_str" && op.s_value.as_deref() == Some("UnboundLocalError"))
        .filter_map(|op| op.out.as_deref())
        .collect();
    let is_unbound_exception_new = |op: &OpIR| -> bool {
        if matches!(
            op.kind.as_str(),
            "exception_new_builtin" | "exception_new_builtin_empty" | "exception_new_builtin_one"
        ) {
            return op.s_value.as_deref() == Some("UnboundLocalError");
        }
        if op.kind != "exception_new" {
            return false;
        }
        op.args
            .as_ref()
            .is_some_and(|args| !args.is_empty() && unbound_error_names.contains(args[0].as_str()))
    };

    let mut remove = vec![false; len];
    let mut i = 0;
    while i + 9 < len {
        // Skip optional nop before the `is` op.
        let base = i;
        if ops[i].kind == "nop" {
            i += 1;
            if i + 9 >= len {
                break;
            }
        }

        // [0] is out=R args=[V, M]  — second arg must be a known missing sentinel
        if ops[i].kind != "is" {
            i = base + 1;
            continue;
        }
        let is_args = match ops[i].args.as_ref() {
            Some(args) if args.len() == 2 => args,
            _ => {
                i = base + 1;
                continue;
            }
        };
        if !missing_outputs.contains(is_args[1].as_str()) {
            i = base + 1;
            continue;
        }
        let is_out = match ops[i].out.as_deref() {
            Some(r) => r,
            None => {
                i = base + 1;
                continue;
            }
        };

        // ── Variant B: is → if → tuple_new → exception_new → raise → end_if ──
        // The frontend emits `if` directly (before TIR adds jump/label/br_if).
        let j1 = i + 1;
        if j1 < len && ops[j1].kind == "if" {
            let if_args = ops[j1].args.as_ref();
            let if_matches = if_args.is_some_and(|a| !a.is_empty() && a[0] == is_out);
            if if_matches {
                // Scan forward for tuple_new → exception_new → raise → end_if
                let mut k = j1 + 1;
                let mut found_tuple_new = false;
                let mut found_exc_new = false;
                let mut found_raise = false;
                let mut end_idx = 0usize;
                let max_scan = (j1 + 8).min(len);
                while k < max_scan {
                    match ops[k].kind.as_str() {
                        "tuple_new" if !found_tuple_new => found_tuple_new = true,
                        _ if found_tuple_new
                            && !found_exc_new
                            && is_unbound_exception_new(&ops[k]) =>
                        {
                            found_exc_new = true;
                        }
                        "raise" if found_exc_new && !found_raise => found_raise = true,
                        "end_if" | "else" if found_raise => {
                            end_idx = k;
                            break;
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if found_raise && end_idx > 0 {
                    // Match confirmed.  Remove the entire is → if → ... → raise → end_if
                    // sequence.  Both the `if` and `end_if`/`else` must be removed
                    // together to keep structured control flow consistent.
                    if base != i {
                        remove[base] = true;
                    }
                    for idx in i..=end_idx {
                        remove[idx] = true;
                    }
                    i = end_idx + 1;
                    continue;
                }
            }
            i = base + 1;
            continue;
        }

        // ── Variant A: is → jump → label → br_if → jump → label → tuple_new → exception_new → raise → label ──
        if j1 >= len || ops[j1].kind != "jump" {
            i = base + 1;
            continue;
        }

        // [2] label val=L1
        let j2 = j1 + 1;
        if j2 >= len || ops[j2].kind != "label" {
            i = base + 1;
            continue;
        }

        // [3] br_if args=[R] val=L_raise
        let j3 = j2 + 1;
        if j3 >= len || ops[j3].kind != "br_if" {
            i = base + 1;
            continue;
        }
        let brif_args = match ops[j3].args.as_ref() {
            Some(args) if !args.is_empty() => args,
            _ => {
                i = base + 1;
                continue;
            }
        };
        if brif_args[0] != is_out {
            i = base + 1;
            continue;
        }

        // [4] jump val=L_ok
        let j4 = j3 + 1;
        if j4 >= len || ops[j4].kind != "jump" {
            i = base + 1;
            continue;
        }

        // [5] label val=L_raise
        let j5 = j4 + 1;
        if j5 >= len || ops[j5].kind != "label" {
            i = base + 1;
            continue;
        }

        // [6] tuple_new (exception message)
        let j6 = j5 + 1;
        if j6 >= len || ops[j6].kind != "tuple_new" {
            i = base + 1;
            continue;
        }

        // [7] exception_new / exception_new_builtin with "UnboundLocalError"
        let j7 = j6 + 1;
        if j7 >= len || !is_unbound_exception_new(&ops[j7]) {
            i = base + 1;
            continue;
        }

        // [8] raise
        let j8 = j7 + 1;
        if j8 >= len || ops[j8].kind != "raise" {
            i = base + 1;
            continue;
        }

        // [9] label val=L_ok  (continuation)
        let j9 = j8 + 1;
        if j9 >= len || ops[j9].kind != "label" {
            i = base + 1;
            continue;
        }

        // Match confirmed. Mark the entire sequence for removal,
        // EXCEPT the final continuation label (j9) which other
        // code may jump to.
        if base != i {
            // We skipped a nop before the `is` op
            remove[base] = true;
        }
        for idx in i..=j8 {
            remove[idx] = true;
        }
        // Keep j9 (continuation label).

        i = j9 + 1;
    }

    // Also remove orphaned `missing` ops whose outputs are no longer
    // referenced after we stripped the `is` ops above.
    if remove.iter().any(|&r| r) {
        let surviving_args: HashSet<&str> = ops
            .iter()
            .enumerate()
            .filter(|&(idx, _)| !remove[idx])
            .flat_map(|(_, op)| {
                op.args
                    .as_ref()
                    .into_iter()
                    .flat_map(|a| a.iter().map(String::as_str))
            })
            .collect();
        for (idx, op) in ops.iter().enumerate() {
            if op.kind == "missing"
                && let Some(out) = op.out.as_deref()
                && !surviving_args.contains(out)
            {
                remove[idx] = true;
            }
        }
    }

    let count = remove.iter().filter(|&&r| r).count();
    if count > 0 {
        let mut new_ops = Vec::with_capacity(len - count);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if !remove[i] {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
    if std::env::var("MOLT_TRACE_UNBOUND_ELIM").is_ok() {
        let surviving_missing = func_ir.ops.iter().filter(|op| op.kind == "missing").count();
        if surviving_missing > 0 {
            eprintln!(
                "UNBOUND_ELIM: {} removed={} surviving_missing={}",
                func_ir.name, count, surviving_missing
            );
        }
    }
}

fn is_lowered_block_arg_store(op: &OpIR) -> bool {
    op.kind == "store_var"
        && op
            .var
            .as_deref()
            .is_some_and(|var| var.starts_with("_bb") && var.contains("_arg"))
}

fn skip_lowered_block_arg_stores(ops: &[OpIR], mut idx: usize) -> usize {
    while idx < ops.len() && is_lowered_block_arg_store(&ops[idx]) {
        idx += 1;
    }
    idx
}

fn lowered_block_arg_store_start(ops: &[OpIR], mut idx: usize) -> usize {
    while idx > 0 && is_lowered_block_arg_store(&ops[idx - 1]) {
        idx -= 1;
    }
    idx
}

fn op_first_arg(op: &OpIR) -> Option<&str> {
    op.args
        .as_ref()
        .and_then(|args| args.first())
        .map(String::as_str)
}

fn remove_marked_ops(ops: &mut Vec<OpIR>, remove: Vec<bool>) -> bool {
    if !remove.iter().any(|remove_op| *remove_op) {
        return false;
    }
    *ops = ops
        .iter()
        .enumerate()
        .filter(|(idx, _)| !remove[*idx])
        .map(|(_, op)| op.clone())
        .collect();
    true
}

fn raise_flows_directly_to_target(ops: &[OpIR], raise_idx: usize, target: i64) -> bool {
    let mut idx = skip_lowered_block_arg_stores(ops, raise_idx + 1);
    if idx < ops.len() && ops[idx].kind == "check_exception" && ops[idx].value == Some(target) {
        idx = skip_lowered_block_arg_stores(ops, idx + 1);
    }
    idx < ops.len() && ops[idx].kind == "jump" && ops[idx].value == Some(target)
}

/// Canonicalize explicit raise exits after the TIR roundtrip.
///
/// Lowering materializes exception-handler block arguments at every
/// `check_exception` edge. An explicit `raise` followed by an unconditional
/// jump to the same handler already carries required state on that jump; the
/// extra `check_exception` edge only duplicates the same block-argument stores.
/// For canonical builtin exception construction, the constructor is also
/// non-throwing, so a pre-raise handler poll is redundant.
pub fn canonicalize_direct_raise_edges(func_ir: &mut FunctionIR) {
    if !func_ir.ops.iter().any(|op| op.kind == "raise") {
        return;
    }

    let mut remove = vec![false; func_ir.ops.len()];
    for raise_idx in 0..func_ir.ops.len() {
        if func_ir.ops[raise_idx].kind != "raise" {
            continue;
        }
        let check_idx = skip_lowered_block_arg_stores(&func_ir.ops, raise_idx + 1);
        if check_idx >= func_ir.ops.len() || func_ir.ops[check_idx].kind != "check_exception" {
            continue;
        }
        let Some(target) = func_ir.ops[check_idx].value else {
            continue;
        };
        let jump_idx = skip_lowered_block_arg_stores(&func_ir.ops, check_idx + 1);
        if jump_idx >= func_ir.ops.len()
            || func_ir.ops[jump_idx].kind != "jump"
            || func_ir.ops[jump_idx].value != Some(target)
        {
            continue;
        }
        for slot in remove.iter_mut().take(check_idx + 1).skip(raise_idx + 1) {
            *slot = true;
        }
    }
    remove_marked_ops(&mut func_ir.ops, remove);

    let mut remove = vec![false; func_ir.ops.len()];
    for check_idx in 0..func_ir.ops.len() {
        if func_ir.ops[check_idx].kind != "check_exception" {
            continue;
        }
        let Some(target) = func_ir.ops[check_idx].value else {
            continue;
        };
        let raise_idx = check_idx + 1;
        if raise_idx >= func_ir.ops.len() || func_ir.ops[raise_idx].kind != "raise" {
            continue;
        }
        if !raise_flows_directly_to_target(&func_ir.ops, raise_idx, target) {
            continue;
        }
        let Some(raise_arg) = op_first_arg(&func_ir.ops[raise_idx]) else {
            continue;
        };
        let store_start = lowered_block_arg_store_start(&func_ir.ops, check_idx);
        let Some(producer_idx) = store_start.checked_sub(1) else {
            continue;
        };
        let producer = &func_ir.ops[producer_idx];
        if !matches!(
            producer.kind.as_str(),
            "exception_new_builtin" | "exception_new_builtin_empty" | "exception_new_builtin_one"
        ) || producer.out.as_deref() != Some(raise_arg)
        {
            continue;
        }
        for slot in remove.iter_mut().take(check_idx + 1).skip(store_start) {
            *slot = true;
        }
    }
    remove_marked_ops(&mut func_ir.ops, remove);
}

/// Eliminate redundant `guard_tag` ops on typed float/int variables.
///
/// `guard_tag(val, expected_tag)` calls `molt_guard_type` — a runtime
/// function call — to assert the NaN-boxing tag matches. For variables
/// that are provably typed (the result of `const_float`, `const`,
/// float/int arithmetic, or loaded from a typed `store_var` chain),
/// the tag is guaranteed correct and the guard is dead weight.
///
/// In the mandelbrot inner loop, two `guard_tag` ops per iteration add
/// two unnecessary function calls.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
/// Count every textual use of a value name across an op's `args`, `var`, and
/// (deliberately excluded) `out`.  Used by `fuse_method_dispatch` to prove a
/// getattr / callargs temporary is single-use before fusing it away.
fn fuse_count_value_reads(ops: &[OpIR], name: &str) -> usize {
    let mut n = 0usize;
    for op in ops {
        if let Some(args) = &op.args {
            for a in args {
                if a == name {
                    n += 1;
                }
            }
        }
        if op.var.as_deref() == Some(name) {
            n += 1;
        }
    }
    n
}

/// Fuse the `obj.method(args...)` dispatch idiom into a single `call_method_ic`
/// op (the CPython `LOAD_METHOD`/`CALL_METHOD` optimisation).
///
/// The frontend lowers a user-method call on a same-module instance to:
///
/// ```text
/// get_attr_generic_ptr  out=T   args=[recv]  s_value=<method>   # alloc bound method
/// (check_exception/line/nop ...)
/// callargs_new          out=CA                                  # alloc callargs
/// callargs_push_pos     out=_   args=[CA, a0]
/// callargs_push_pos     out=_   args=[CA, a1] ...
/// call_bind             out=R   args=[T, CA]                    # generic dispatch
/// ```
///
/// Both `get_attr_generic_ptr` (bound-method alloc) and `callargs_new`
/// (callargs alloc) recur every call.  This pass rewrites the quartet to:
///
/// ```text
/// call_method_ic        out=R   args=[recv, a0, a1, ...]  s_value=<method>
/// ```
///
/// which lowers to a single allocation-free runtime call (`molt_call_method_icN`).
///
/// SOUNDNESS (each is required before fusing; otherwise the site is left as-is):
///   * `T` (the getattr result) is referenced by EXACTLY this `call_bind` and
///     nowhere else — proven by a whole-function read count.
///   * `CA` (the callargs) is referenced ONLY by its `callargs_push_pos` chain
///     and this `call_bind` — no `callargs_push_kw`, no escape.
///   * Every `callargs_push_pos` for `CA` lies between `callargs_new` and
///     `call_bind` with no intervening control-flow boundary (label/jump/br_if/
///     loop_*/ret/raise), so positional order is preserved.
///   * The `get_attr_generic_ptr` has a single recv arg and an `s_value` method
///     name (it is a method getattr, not a field/dunder access shape).
///
/// The runtime op reproduces getattr+call semantics including all descriptor /
/// instance-shadow / `__getattribute__` fallbacks, so behaviour is preserved.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn fuse_method_dispatch(func_ir: &mut FunctionIR) {
    fuse_method_dispatch_inner(func_ir, std::env::var("MOLT_DISABLE_METHOD_FUSION").is_ok())
}

/// [`fuse_method_dispatch`] with the disable lever explicitly controlled
/// (rather than read from the process-global env), so tests can force it
/// deterministically without racing other parallel tests — `set_var` in one
/// test flips the gate under every concurrently-running test (the
/// poisoned-env-lock / flaky-fusion-test class).
fn fuse_method_dispatch_inner(func_ir: &mut FunctionIR, disabled: bool) {
    if disabled {
        return;
    }
    let len = func_ir.ops.len();
    if len < 3 {
        return;
    }

    fn is_control_boundary(kind: &str) -> bool {
        matches!(
            kind,
            "label"
                | "state_label"
                | "jump"
                | "br_if"
                | "if"
                | "else"
                | "end_if"
                | "phi"
                | "loop_start"
                | "loop_end"
                | "loop_continue"
                | "loop_break"
                | "loop_break_if_true"
                | "loop_break_if_false"
                | "loop_break_if_exception"
                | "ret"
                | "raise"
        )
    }

    // Map each value name to the op index that defines it (its `out`).
    let mut def_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, op) in func_ir.ops.iter().enumerate() {
        if let Some(out) = &op.out
            && out != "none"
        {
            def_idx.insert(out.clone(), i);
        }
    }

    // remove[i]=true => drop op i; replace[i]=Some(op) => substitute op i.
    let mut remove = vec![false; len];
    let mut replacement: Vec<Option<OpIR>> = (0..len).map(|_| None).collect();

    for idx in 0..len {
        if func_ir.ops[idx].kind != "call_bind" {
            continue;
        }
        let call = &func_ir.ops[idx];
        let Some(call_args) = call.args.as_ref() else {
            continue;
        };
        if call_args.len() != 2 {
            continue;
        }
        let callee_name = call_args[0].clone();
        let callargs_name = call_args[1].clone();

        // The callee must be defined by a method `get_attr_generic_ptr`.
        let Some(&getattr_idx) = def_idx.get(&callee_name) else {
            continue;
        };
        let getattr = &func_ir.ops[getattr_idx];
        if getattr.kind != "get_attr_generic_ptr" {
            continue;
        }
        let Some(getattr_args) = getattr.args.as_ref() else {
            continue;
        };
        if getattr_args.len() != 1 {
            continue;
        }
        let Some(method_name) = getattr.s_value.clone() else {
            continue;
        };
        let recv_name = getattr_args[0].clone();

        // The callargs must be defined by a `callargs_new`.
        let Some(&callargs_new_idx) = def_idx.get(&callargs_name) else {
            continue;
        };
        if func_ir.ops[callargs_new_idx].kind != "callargs_new" {
            continue;
        }
        if callargs_new_idx >= idx || getattr_idx >= idx {
            continue;
        }

        // The callee temporary must be single-use (this call_bind only).
        if fuse_count_value_reads(&func_ir.ops, &callee_name) != 1 {
            continue;
        }

        // Collect the positional pushes for this callargs builder, in order,
        // and confirm the builder is used ONLY by its pushes and this call.
        let mut arg_names: Vec<String> = Vec::new();
        let mut push_indices: Vec<usize> = Vec::new();
        let mut callargs_extra_use = false;
        let mut ok = true;
        for (j, op) in func_ir.ops.iter().enumerate() {
            if j == callargs_new_idx || j == idx {
                continue;
            }
            let uses_ca = op
                .args
                .as_ref()
                .is_some_and(|a| a.iter().any(|x| x == &callargs_name))
                || op.var.as_deref() == Some(&callargs_name);
            if !uses_ca {
                continue;
            }
            if op.kind == "callargs_push_pos" {
                // Must be inside the new..call window so order is preserved.
                if j <= callargs_new_idx || j >= idx {
                    ok = false;
                    break;
                }
                let a = op.args.as_ref().unwrap();
                if a.len() != 2 || a[0] != callargs_name {
                    ok = false;
                    break;
                }
                push_indices.push(j);
                arg_names.push(a[1].clone());
            } else {
                // Any other consumer (push_kw, expand_star, escape) => bail.
                callargs_extra_use = true;
                break;
            }
        }
        if !ok || callargs_extra_use {
            continue;
        }
        // No control-flow boundary may sit between callargs_new and call_bind,
        // or positional ordering could differ at runtime.
        if (callargs_new_idx + 1..idx).any(|k| is_control_boundary(func_ir.ops[k].kind.as_str())) {
            continue;
        }
        // The fast path family covers 0..=4 positional args; higher arity keeps
        // the legacy lowering (no regression).
        if arg_names.len() > 4 {
            continue;
        }

        // Build the fused op: args = [recv, a0, a1, ...].
        let mut fused_args = Vec::with_capacity(1 + arg_names.len());
        fused_args.push(recv_name);
        fused_args.extend(arg_names);
        let mut fused = OpIR {
            kind: "call_method_ic".to_string(),
            ..Default::default()
        };
        fused.out = func_ir.ops[idx].out.clone();
        fused.args = Some(fused_args);
        fused.s_value = Some(method_name);
        // Preserve traceback span from the original call_bind.
        fused.col_offset = func_ir.ops[idx].col_offset;
        fused.end_col_offset = func_ir.ops[idx].end_col_offset;

        replacement[idx] = Some(fused);
        remove[getattr_idx] = true;
        remove[callargs_new_idx] = true;
        for p in push_indices {
            remove[p] = true;
        }
    }

    // ── super().method(args) — fuse super_new + get_attr_generic_obj +
    //    callargs + call_indirect into a single `call_super_method_ic`. ──
    for idx in 0..len {
        if remove[idx] || replacement[idx].is_some() {
            continue;
        }
        if func_ir.ops[idx].kind != "call_indirect" {
            continue;
        }
        let call = &func_ir.ops[idx];
        let Some(call_args) = call.args.as_ref() else {
            continue;
        };
        if call_args.len() != 2 {
            continue;
        }
        let callee_name = call_args[0].clone();
        let callargs_name = call_args[1].clone();

        // Callee must be `get_attr_generic_obj(super_obj)` with a method name.
        let Some(&getattr_idx) = def_idx.get(&callee_name) else {
            continue;
        };
        if remove[getattr_idx] {
            continue;
        }
        let getattr = &func_ir.ops[getattr_idx];
        if getattr.kind != "get_attr_generic_obj" {
            continue;
        }
        let Some(getattr_args) = getattr.args.as_ref() else {
            continue;
        };
        if getattr_args.len() != 1 {
            continue;
        }
        let Some(method_name) = getattr.s_value.clone() else {
            continue;
        };
        let super_name = getattr_args[0].clone();

        // The super object must come from `super_new(class, self)`.
        let Some(&super_idx) = def_idx.get(&super_name) else {
            continue;
        };
        if remove[super_idx] {
            continue;
        }
        let super_op = &func_ir.ops[super_idx];
        if super_op.kind != "super_new" {
            continue;
        }
        let Some(super_args) = super_op.args.as_ref() else {
            continue;
        };
        if super_args.len() != 2 {
            continue;
        }
        let class_name = super_args[0].clone();
        let self_name = super_args[1].clone();

        // Callargs must come from a callargs_new and be used only by its pushes.
        let Some(&callargs_new_idx) = def_idx.get(&callargs_name) else {
            continue;
        };
        if remove[callargs_new_idx] || func_ir.ops[callargs_new_idx].kind != "callargs_new" {
            continue;
        }
        if super_idx >= idx || getattr_idx >= idx || callargs_new_idx >= idx {
            continue;
        }

        // The getattr result AND the super object must each be single-use.
        if fuse_count_value_reads(&func_ir.ops, &callee_name) != 1 {
            continue;
        }
        if fuse_count_value_reads(&func_ir.ops, &super_name) != 1 {
            continue;
        }

        let mut arg_names: Vec<String> = Vec::new();
        let mut push_indices: Vec<usize> = Vec::new();
        let mut bail = false;
        for (j, op) in func_ir.ops.iter().enumerate() {
            if j == callargs_new_idx || j == idx {
                continue;
            }
            let uses_ca = op
                .args
                .as_ref()
                .is_some_and(|a| a.iter().any(|x| x == &callargs_name))
                || op.var.as_deref() == Some(&callargs_name);
            if !uses_ca {
                continue;
            }
            if op.kind == "callargs_push_pos" {
                if j <= callargs_new_idx || j >= idx {
                    bail = true;
                    break;
                }
                let a = op.args.as_ref().unwrap();
                if a.len() != 2 || a[0] != callargs_name {
                    bail = true;
                    break;
                }
                push_indices.push(j);
                arg_names.push(a[1].clone());
            } else {
                bail = true;
                break;
            }
        }
        if bail || arg_names.len() > 4 {
            continue;
        }
        if (callargs_new_idx + 1..idx).any(|k| is_control_boundary(func_ir.ops[k].kind.as_str())) {
            continue;
        }

        // Build: call_super_method_ic  args=[class, self, a0, ...]  s_value=M.
        let mut fused_args = Vec::with_capacity(2 + arg_names.len());
        fused_args.push(class_name);
        fused_args.push(self_name);
        fused_args.extend(arg_names);
        let mut fused = OpIR {
            kind: "call_super_method_ic".to_string(),
            ..Default::default()
        };
        fused.out = func_ir.ops[idx].out.clone();
        fused.args = Some(fused_args);
        fused.s_value = Some(method_name);
        fused.col_offset = func_ir.ops[idx].col_offset;
        fused.end_col_offset = func_ir.ops[idx].end_col_offset;

        replacement[idx] = Some(fused);
        remove[super_idx] = true;
        remove[getattr_idx] = true;
        remove[callargs_new_idx] = true;
        for p in push_indices {
            remove[p] = true;
        }
    }

    if remove.iter().any(|&r| r) || replacement.iter().any(|r| r.is_some()) {
        let mut new_ops = Vec::with_capacity(len);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if remove[i] {
                continue;
            }
            if let Some(rep) = replacement[i].take() {
                new_ops.push(rep);
            } else {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
}

pub fn eliminate_redundant_guard_tags(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_GUARD_ELIM").is_ok() {
        return;
    }
    let ops = &func_ir.ops;
    let len = ops.len();
    if len == 0 {
        return;
    }

    // Collect names of all values produced by ops that guarantee a
    // correct NaN-box tag by construction.
    let mut typed_outputs: HashSet<String> = HashSet::new();
    for op in ops.iter() {
        let guaranteed = matches!(
            op.kind.as_str(),
            "const"
                | "const_int"
                | "const_float"
                | "const_bool"
                | "const_none"
                | "const_str"
                | "const_bytes"
                | "add"
                | "sub"
                | "mul"
                | "div"
                | "floordiv"
                | "mod"
                | "pow"
                | "neg"
                | "unary_neg"
                | "lt"
                | "le"
                | "gt"
                | "ge"
                | "eq"
                | "ne"
                | "is"
                | "and"
                | "or"
                | "not"
                | "band"
                | "bor"
                | "bxor"
                | "lshift"
                | "rshift"
                | "invert"
                | "list_new"
                | "tuple_new"
                | "dict_new"
                | "set_new"
                | "list_getitem"
                | "tuple_getitem"
                | "dict_getitem"
        );
        if guaranteed && let Some(out) = op.out.as_ref() {
            typed_outputs.insert(out.clone());
        }
    }

    let mut remove = vec![false; len];
    for (idx, op) in ops.iter().enumerate() {
        if op.kind != "guard_tag" && op.kind != "guard_type" {
            continue;
        }
        let args = match op.args.as_ref() {
            Some(args) if !args.is_empty() => args,
            _ => continue,
        };
        // If the value being guarded is provably typed, remove the guard.
        if typed_outputs.contains(&args[0]) {
            remove[idx] = true;
        }
    }

    let count = remove.iter().filter(|&&r| r).count();
    if count > 0 {
        let mut new_ops = Vec::with_capacity(len - count);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if !remove[i] {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn elide_safe_exception_checks(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_EXC_ELIDE").is_ok() {
        return;
    }
    /// Operations that are guaranteed to never set the exception flag.
    const NEVER_RAISES: &[&str] = &[
        "inc_ref",
        "dec_ref",
        "dec_ref_obj",
        "inc_ref_obj",
        "const_int",
        "const_float",
        "const_bool",
        "const_none",
        "const_string",
        "nop",
        "line",
        "label",
        "state_label",
    ];
    let ops = &func_ir.ops;
    let len = ops.len();
    if len < 2 {
        return;
    }
    let mut remove = vec![false; len];
    for i in 1..len {
        if ops[i].kind != "check_exception" {
            continue;
        }
        // Walk backwards skipping nops, labels, and other non-raising ops
        // to find the "real" predecessor.
        let mut pred_idx = i - 1;
        while pred_idx > 0
            && matches!(
                ops[pred_idx].kind.as_str(),
                "nop" | "line" | "label" | "state_label"
            )
        {
            pred_idx -= 1;
        }
        let pred_kind = ops[pred_idx].kind.as_str();
        if NEVER_RAISES.contains(&pred_kind) {
            remove[i] = true;
        }
    }
    let count = remove.iter().filter(|&&r| r).count();
    if count > 0 {
        let mut new_ops = Vec::with_capacity(len - count);
        for (i, op) in func_ir.ops.drain(..).enumerate() {
            if !remove[i] {
                new_ops.push(op);
            }
        }
        func_ir.ops = new_ops;
    }
}

#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn hoist_loop_invariants(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_LICM").is_ok() {
        return;
    }
    let ops = &func_ir.ops;
    let len = ops.len();
    if len == 0 {
        return;
    }
    let mut loop_regions: Vec<(usize, usize)> = Vec::new();
    let mut loop_start_stack: Vec<usize> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                let next_is_index = ops
                    .get(idx + 1)
                    .is_some_and(|next| next.kind == "loop_index_start");
                if !next_is_index {
                    loop_start_stack.push(idx);
                }
            }
            "loop_index_start" => {
                loop_start_stack.push(idx);
            }
            "loop_end" => {
                if let Some(start) = loop_start_stack.pop() {
                    loop_regions.push((start, idx));
                }
            }
            _ => {}
        }
    }
    if loop_regions.is_empty() {
        return;
    }
    let mut hoist_before: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut hoisted_set: BTreeSet<usize> = BTreeSet::new();
    for &(start, end) in &loop_regions {
        let mut defined_in_loop: HashSet<String> = HashSet::new();
        for idx in (start + 1)..end {
            if let Some(out) = ops[idx].out.as_deref() {
                defined_in_loop.insert(out.to_string());
            }
        }
        let mut to_hoist: Vec<usize> = Vec::new();
        for idx in (start + 1)..end {
            let op = &ops[idx];
            if !is_hoistable(op) {
                continue;
            }
            let inputs_outside = op
                .args
                .as_ref()
                .is_none_or(|args| args.iter().all(|arg| !defined_in_loop.contains(arg)));
            if !inputs_outside {
                continue;
            }
            if let Some(out) = op.out.as_deref() {
                let mut write_count = 0;
                for j in (start + 1)..end {
                    if ops[j].out.as_deref() == Some(out) {
                        write_count += 1;
                    }
                }
                if write_count > 1 {
                    continue;
                }
            }
            to_hoist.push(idx);
        }
        if !to_hoist.is_empty() {
            for &idx in &to_hoist {
                hoisted_set.insert(idx);
            }
            hoist_before.entry(start).or_default().extend(to_hoist);
        }
    }
    if hoisted_set.is_empty() {
        return;
    }
    let mut new_ops: Vec<OpIR> = Vec::with_capacity(len);
    for (idx, op) in ops.iter().enumerate() {
        if let Some(hoisted_indices) = hoist_before.get(&idx) {
            for &hi in hoisted_indices {
                new_ops.push(ops[hi].clone());
            }
        }
        if hoisted_set.contains(&idx) {
            continue;
        }
        new_ops.push(op.clone());
    }
    func_ir.ops = new_ops;
}

/// Dead-function elimination: remove functions that are never referenced from
/// any reachable function.  The entry function (first in the list, typically
/// `<module>`) is always retained; any function reachable from it through
/// `call_internal`, `func_new`, `func_new_closure`, `func_new_builtin`,
/// or `code_new` references is kept.
///
/// This pass runs after inlining — if a callee was fully inlined into all
/// call sites, it becomes unreachable and will be eliminated here.
/// Applies to both native and WASM backends.
/// Inject `molt_runtime_exit(0)` before the final `ret` in `molt_main`.
///
/// This calls `_exit(0)` after all user code and atexit callbacks have run,
/// skipping C-level global destructors and TLS teardown that cause
/// intermittent SIGSEGV on exit. Same approach as CPython.
pub fn inject_runtime_exit(ir: &mut SimpleIR) {
    for func in &mut ir.functions {
        if func.name != "molt_main" {
            continue;
        }
        // Find the last `ret` op and insert `call molt_runtime_exit` before it.
        let ret_idx = func.ops.iter().rposition(|op| op.kind == "ret");
        if let Some(idx) = ret_idx {
            let exit_op = OpIR {
                kind: "call".to_string(),
                args: Some(vec!["__molt_zero__".to_string()]),
                s_value: Some("molt_runtime_exit".to_string()),
                ..OpIR::default()
            };
            // Also need a const 0 for the exit code arg.
            let const_op = OpIR {
                kind: "const".to_string(),
                out: Some("__molt_zero__".to_string()),
                value: Some(0),
                ..OpIR::default()
            };
            func.ops.insert(idx, exit_op);
            func.ops.insert(idx, const_op);
        }
        break;
    }
}

pub fn eliminate_dead_functions(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_DEAD_FUNC_ELIM").is_ok() {
        return;
    }
    if ir.functions.is_empty() {
        return;
    }

    // Build the call graph: function name -> set of referenced function names.
    // Use owned Strings so that `ir.functions` is not borrowed when we call retain().
    let defined: BTreeSet<String> = ir.functions.iter().map(|f| f.name.clone()).collect();
    let mut references: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for func in &ir.functions {
        let mut refs: BTreeSet<String> = BTreeSet::new();
        for op in &func.ops {
            match op.kind.as_str() {
                "call" | "call_internal" | "func_new" | "func_new_closure" | "func_new_builtin"
                | "code_new" | "call_guarded" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                "call_indirect" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                // alloc_task's s_value is the poll function name directly
                // (e.g., "foo_poll"). generator_create/coro_create reference
                // a base function whose companion _poll must also be kept.
                "alloc_task" | "generator_create" | "coro_create" => {
                    if let Some(name) = op.s_value.as_ref() {
                        if defined.contains(name.as_str()) {
                            refs.insert(name.clone());
                        }
                        // generator_create/coro_create reference the base
                        // function; the backends derive "{base}_poll" at
                        // compile time, so mark both.
                        if !name.ends_with("_poll") {
                            let poll_name = format!("{name}_poll");
                            if defined.contains(poll_name.as_str()) {
                                refs.insert(poll_name);
                            }
                        }
                    }
                }
                // Ops that take a function pointer address via s_value.
                "fn_ptr_code_set" | "asyncgen_locals_register" | "gen_locals_register" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                // Other op kinds that legitimately reference functions by name.
                "task_new" | "generator_send" | "spawn" | "call_func" | "call_method"
                | "import_from" | "import_name" | "class_def" | "decorator" | "super_call"
                | "yield_from" | "await" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                _ => {}
            }
        }
        references.insert(func.name.clone(), refs);
    }

    // BFS from entry roots to find all reachable functions.
    // Roots: (1) the first function (entry), (2) well-known linker/runtime
    // entry points, (3) any function whose name matches a keep-pattern.
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    let seed =
        |name: String, r: &mut BTreeSet<String>, q: &mut std::collections::VecDeque<String>| {
            if r.insert(name.clone()) {
                q.push_back(name);
            }
        };

    // (1) First function is always the module entry.
    seed(ir.functions[0].name.clone(), &mut reachable, &mut queue);

    // (2) + (3) Scan all functions for keep-patterns.
    //
    // molt_init_* functions are NOT blanket-kept.  They are referenced by
    // static CALL ops in the IR (emitted by the frontend's _emit_module_load)
    // so the BFS discovers them naturally.
    //
    // molt_isolate_* functions MUST be kept with their full bodies because
    // the runtime references them as extern "C" symbols for dynamic imports
    // and isolate startup. Stubbing them based on local reachability breaks
    // Python-level import paths (`__import__`, importlib helpers, intrinsic
    // module loads) that route through the runtime rather than direct IR edges.
    // Binary size should be controlled by the module graph itself, not by
    // mutating the semantics of runtime entrypoints during DFE.
    for func in &ir.functions {
        if is_protected_runtime_entrypoint(&func.name) {
            seed(func.name.clone(), &mut reachable, &mut queue);
        }
    }

    while let Some(current) = queue.pop_front() {
        if let Some(refs) = references.get(&current) {
            for target in refs {
                if reachable.insert(target.clone()) {
                    queue.push_back(target.clone());
                }
            }
        }
    }

    let original_count = ir.functions.len();
    ir.functions.retain(|f| reachable.contains(&f.name));
    let eliminated = original_count - ir.functions.len();

    if eliminated > 0 && std::env::var("MOLT_DEBUG_DEAD_FUNC_ELIM").is_ok() {
        eprintln!(
            "dead-func-elim: removed {eliminated} of {original_count} functions ({} retained)",
            ir.functions.len()
        );
    }
}

fn is_protected_runtime_entrypoint(name: &str) -> bool {
    const RUNTIME_ENTRYPOINTS: &[&str] = &["molt_main", "molt_host_init", "_start"];
    const RUNTIME_ENTRYPOINT_PREFIXES: &[&str] = &["molt_isolate_"];

    RUNTIME_ENTRYPOINTS.contains(&name)
        || RUNTIME_ENTRYPOINT_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

// ---------------------------------------------------------------------------
// SimpleIR dead op elimination (intra-function)
//
// Removes ops within each function whose results are never consumed by any
// subsequent op. This is the SimpleIR equivalent of TIR DCE — it catches
// waste from frontend codegen before TIR lifting even sees it.
//
// Safety: only removes ops that are provably pure (no side effects).
// Side-effecting ops (calls, stores, raises, imports) are always preserved.
// ---------------------------------------------------------------------------

/// Returns `true` for SimpleIR ops that provably cannot introduce a Python
/// exception. This is intentionally stricter than "has no writes": expression
/// statements still have to execute user dispatch and raise the same exceptions
/// as CPython even when their produced value is unused.
pub struct SimpleIrScalarPurityFacts<'a> {
    plan: Option<&'a ScalarRepresentationPlan>,
    literal_kinds: BTreeMap<String, ScalarKind>,
}

impl<'a> SimpleIrScalarPurityFacts<'a> {
    pub fn for_function(func: &FunctionIR, plan: Option<&'a ScalarRepresentationPlan>) -> Self {
        let literal_kinds = func
            .ops
            .iter()
            .filter_map(|op| {
                let out = op.out.as_ref()?;
                Some((out.clone(), simple_ir_literal_scalar_kind(op)?))
            })
            .collect();
        Self {
            plan,
            literal_kinds,
        }
    }

    fn name_scalar_kind(&self, name: &str) -> Option<ScalarKind> {
        self.literal_kinds
            .get(name)
            .copied()
            .or_else(|| self.plan.and_then(|plan| plan.name_scalar_kind(name)))
    }

    fn name_is_integer_family(&self, name: &str) -> bool {
        matches!(
            self.name_scalar_kind(name),
            Some(ScalarKind::Int | ScalarKind::Bool)
        ) || self
            .plan
            .is_some_and(|plan| plan.name_is_integer_family(name))
    }
}

fn simple_ir_literal_scalar_kind(op: &OpIR) -> Option<ScalarKind> {
    match op.kind.as_str() {
        "const" => op.value.map(|_| ScalarKind::Int),
        "const_bool" => Some(ScalarKind::Bool),
        "const_float" => Some(ScalarKind::Float),
        "const_str" => Some(ScalarKind::Str),
        "const_none" => Some(ScalarKind::NoneValue),
        _ => None,
    }
}

pub fn simple_ir_op_is_provably_nonthrowing_with_facts(
    facts: Option<&SimpleIrScalarPurityFacts<'_>>,
    op: &OpIR,
) -> bool {
    let kind = op.kind.as_str();

    if simple_ir_op_has_static_module_class_binding_effect_proof(op) {
        return true;
    }

    if matches!(
        kind,
        "const"
            | "const_int"
            | "const_float"
            | "const_str"
            | "const_bool"
            | "const_none"
            | "const_bytes"
            | "const_bigint"
            | "const_ellipsis"
            | "missing"
    ) {
        return true;
    }

    if matches!(
        kind,
        "load_var"
            | "store_var"
            | "load_fast"
            | "store_fast"
            | "load_var_slot"
            | "store_var_slot"
            | "load_closure"
            | "store_closure"
    ) {
        return true;
    }

    if matches!(
        kind,
        "copy" | "copy_var" | "identity_alias" | "box" | "unbox" | "cast" | "widen" | "phi"
    ) {
        return true;
    }

    if let Some(facts) = facts
        && simple_ir_scalar_op_is_provably_nonthrowing(facts, op)
    {
        return true;
    }

    if matches!(kind, "is" | "is_not") {
        return true;
    }

    if matches!(
        kind,
        "guard_tag" | "guard_layout" | "guard_int" | "guard_float" | "type_guard"
    ) {
        return true;
    }

    if matches!(kind, "store" | "load") {
        return true;
    }

    if matches!(
        kind,
        "if" | "else"
            | "end_if"
            | "loop_start"
            | "loop_end"
            | "loop_continue"
            | "loop_break"
            | "loop_break_if_false"
            | "loop_index_start"
            | "loop_index_next"
            | "jump"
            | "label"
            | "line"
    ) {
        return true;
    }

    if matches!(kind, "code_slots_init" | "code_slot_set" | "code_new") {
        return true;
    }

    if matches!(
        kind,
        "trace_enter_slot"
            | "trace_exit"
            | "exception_clear"
            | "exception_last"
            | "exception_last_pending"
            | "exception_finally_pending_observer"
            | "exception_stack_enter"
            | "exception_stack_clear"
            | "exception_stack_depth"
            | "context_depth"
            | "check_exception"
    ) {
        return true;
    }

    false
}

fn simple_ir_scalar_op_is_provably_nonthrowing(
    facts: &SimpleIrScalarPurityFacts<'_>,
    op: &OpIR,
) -> bool {
    let args = op.args.as_deref().unwrap_or(&[]);
    let arg_kind = |name: &str| facts.name_scalar_kind(name);
    let arg_is_numeric = |name: &str| {
        matches!(
            arg_kind(name),
            Some(ScalarKind::Int | ScalarKind::Bool | ScalarKind::Float)
        )
    };
    let all_args_numeric =
        || !args.is_empty() && args.iter().all(|arg| arg_is_numeric(arg.as_str()));
    let all_args_str = || {
        !args.is_empty()
            && args
                .iter()
                .all(|arg| arg_kind(arg.as_str()) == Some(ScalarKind::Str))
    };
    let all_args_scalar = || !args.is_empty() && args.iter().all(|arg| arg_kind(arg).is_some());
    let first_source_kind = || {
        op.var
            .as_deref()
            .or_else(|| args.first().map(String::as_str))
            .and_then(arg_kind)
    };

    match op.kind.as_str() {
        "add" | "inplace_add" => all_args_numeric() || all_args_str(),
        "sub" | "mul" | "inplace_sub" | "inplace_mul" => all_args_numeric(),
        "neg" | "pos" => matches!(
            first_source_kind(),
            Some(ScalarKind::Int | ScalarKind::Bool | ScalarKind::Float)
        ),
        "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "bitand" | "bitor" | "bitxor"
        | "inplace_bit_and" | "inplace_bit_or" | "inplace_bit_xor" => {
            !args.is_empty()
                && args
                    .iter()
                    .all(|arg| facts.name_is_integer_family(arg.as_str()))
        }
        "eq" | "ne" => all_args_scalar(),
        "lt" | "le" | "gt" | "ge" => all_args_numeric() || all_args_str(),
        _ => false,
    }
}

fn simple_ir_op_needs_scalar_plan_for_nonthrowing(op: &OpIR) -> bool {
    matches!(
        op.kind.as_str(),
        "add"
            | "sub"
            | "mul"
            | "inplace_add"
            | "inplace_sub"
            | "inplace_mul"
            | "neg"
            | "pos"
            | "bit_and"
            | "bit_or"
            | "bit_xor"
            | "bit_not"
            | "bitand"
            | "bitor"
            | "bitxor"
            | "inplace_bit_and"
            | "inplace_bit_or"
            | "inplace_bit_xor"
            | "lt"
            | "le"
            | "gt"
            | "ge"
            | "eq"
            | "ne"
    )
}

/// Returns `true` when an unused-result op can be erased without dropping
/// Python-observable behaviour.
fn simple_ir_unused_result_is_removable(
    facts: Option<&SimpleIrScalarPurityFacts<'_>>,
    op: &OpIR,
) -> bool {
    if !simple_ir_op_is_provably_nonthrowing_with_facts(facts, op) {
        return false;
    }

    matches!(
        op.kind.as_str(),
        "const"
            | "const_int"
            | "const_float"
            | "const_str"
            | "const_bool"
            | "const_none"
            | "const_bytes"
            | "const_bigint"
            | "const_ellipsis"
            | "missing"
            | "copy"
            | "copy_var"
            | "load_var"
            | "load_fast"
            | "load_var_slot"
            | "load_closure"
            | "add"
            | "sub"
            | "mul"
            | "inplace_add"
            | "inplace_sub"
            | "inplace_mul"
            | "neg"
            | "pos"
            | "bit_and"
            | "bit_or"
            | "bit_xor"
            | "bit_not"
            | "bitand"
            | "bitor"
            | "bitxor"
            | "shl"
            | "shr"
            | "lshift"
            | "rshift"
            | "inplace_bit_and"
            | "inplace_bit_or"
            | "inplace_bit_xor"
            | "inplace_lshift"
            | "inplace_rshift"
            | "lt"
            | "le"
            | "gt"
            | "ge"
            | "eq"
            | "ne"
            | "is"
            | "is_not"
            | "module_cache_get"
            | "module_get_attr"
            | "box"
            | "unbox"
            | "cast"
            | "widen"
            | "identity_alias"
            | "build_list"
            | "build_tuple"
            | "build_dict"
            | "build_set"
            | "build_slice"
            | "type_guard"
    )
}

fn simple_ir_op_has_static_module_class_binding_effect_proof(op: &OpIR) -> bool {
    simple_ir_has_static_module_class_binding_effect_proof(&op.kind, op.effect_proof.as_deref())
}

fn simple_ir_var_field_is_read(op: &OpIR) -> bool {
    if matches!(op.kind.as_str(), "copy_var" | "load_var")
        && op.args.as_ref().is_some_and(|args| !args.is_empty())
    {
        return false;
    }
    !matches!(
        op.kind.as_str(),
        // Assignment targets and fused iterator value outputs are definitions,
        // not source reads.
        "store_var" | "store_fast" | "iter_next_unboxed"
    )
}

fn simple_ir_defined_names(op: &OpIR) -> Vec<&str> {
    let mut defined = Vec::new();
    if let Some(out) = op.out.as_deref()
        && out != "none"
    {
        defined.push(out);
    }
    if op.kind == "iter_next_unboxed"
        && let Some(var) = op.var.as_deref()
        && var != "none"
    {
        defined.push(var);
    }
    defined
}

fn push_split_name(out: &mut Vec<String>, seen: &mut BTreeSet<String>, name: &str) {
    if name != "none" && seen.insert(name.to_string()) {
        out.push(name.to_string());
    }
}

fn split_ir_read_names(op: &OpIR) -> Vec<String> {
    let mut read = Vec::new();
    let mut seen = BTreeSet::new();
    match op.kind.as_str() {
        "unpack_sequence" => {
            if let Some(args) = op.args.as_ref()
                && let Some(seq) = args.first()
            {
                push_split_name(&mut read, &mut seen, seq);
            }
        }
        _ => {
            if let Some(args) = op.args.as_ref() {
                for arg in args {
                    push_split_name(&mut read, &mut seen, arg);
                }
            }
        }
    }
    if simple_ir_var_field_is_read(op)
        && let Some(var) = op.var.as_deref()
    {
        push_split_name(&mut read, &mut seen, var);
    }
    read
}

fn split_ir_defined_names(op: &OpIR) -> Vec<String> {
    let mut defined = Vec::new();
    let mut seen = BTreeSet::new();
    for name in simple_ir_defined_names(op) {
        push_split_name(&mut defined, &mut seen, name);
    }
    match op.kind.as_str() {
        "store_var" | "store_fast" => {
            if let Some(var) = op.var.as_deref().or(op.out.as_deref()) {
                push_split_name(&mut defined, &mut seen, var);
            }
        }
        "unpack_sequence" => {
            if let Some(args) = op.args.as_ref() {
                for arg in args.iter().skip(1) {
                    push_split_name(&mut defined, &mut seen, arg);
                }
            }
        }
        _ => {}
    }
    defined
}

fn split_live_before_sets(ops: &[OpIR]) -> Vec<BTreeSet<String>> {
    let mut live = BTreeSet::new();
    let mut live_before = vec![BTreeSet::new(); ops.len() + 1];
    live_before[ops.len()] = live.clone();
    for idx in (0..ops.len()).rev() {
        for name in split_ir_defined_names(&ops[idx]) {
            live.remove(&name);
        }
        for name in split_ir_read_names(&ops[idx]) {
            live.insert(name);
        }
        live_before[idx] = live.clone();
    }
    live_before
}

fn split_param_types_for_names(
    original_params: &[String],
    original_param_types: Option<&Vec<String>>,
    params: &[String],
) -> Option<Vec<String>> {
    let original_param_types = original_param_types?;
    if original_param_types.len() != original_params.len() {
        return None;
    }
    Some(
        params
            .iter()
            .map(|name| {
                original_params
                    .iter()
                    .position(|param| param == name)
                    .map(|idx| original_param_types[idx].clone())
                    .unwrap_or_else(|| "dyn".to_string())
            })
            .collect(),
    )
}

fn split_frame_name(base: &str, occupied: &mut BTreeSet<String>) -> String {
    if occupied.insert(base.to_string()) {
        return base.to_string();
    }
    let mut idx = 0usize;
    loop {
        let candidate = format!("{base}_{idx}");
        if occupied.insert(candidate.clone()) {
            return candidate;
        }
        idx += 1;
    }
}

fn split_defined_before_sets(ops: &[OpIR]) -> Vec<BTreeSet<String>> {
    let mut defined = BTreeSet::new();
    let mut defined_before = vec![BTreeSet::new(); ops.len() + 1];
    defined_before[0] = defined.clone();
    for (idx, op) in ops.iter().enumerate() {
        for name in split_ir_defined_names(op) {
            defined.insert(name);
        }
        defined_before[idx + 1] = defined.clone();
    }
    defined_before
}

fn split_suffix_external_reads(ops: &[OpIR]) -> BTreeSet<String> {
    let mut defined = BTreeSet::new();
    let mut external_reads = BTreeSet::new();
    for op in ops {
        for name in split_ir_read_names(op) {
            if !defined.contains(&name) {
                external_reads.insert(name);
            }
        }
        for name in split_ir_defined_names(op) {
            defined.insert(name);
        }
    }
    external_reads
}

fn split_available_names_for_suffix_clone(
    params: &[String],
    live_before: &[BTreeSet<String>],
    ops: &[OpIR],
    start: usize,
    end: usize,
) -> BTreeSet<String> {
    let mut available: BTreeSet<String> = params
        .iter()
        .filter(|name| name.as_str() != "none")
        .cloned()
        .collect();
    available.extend(live_before[start].iter().cloned());
    for op in &ops[start..end] {
        for name in split_ir_defined_names(op) {
            available.insert(name.to_string());
        }
    }
    available
}

fn split_collect_names(ops: &[OpIR], params: &[String]) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = params
        .iter()
        .filter(|name| name.as_str() != "none")
        .cloned()
        .collect();
    for op in ops {
        for name in split_ir_read_names(op) {
            names.insert(name);
        }
        for name in split_ir_defined_names(op) {
            names.insert(name);
        }
    }
    names
}

fn split_frame_load_ops(
    frame_name: &str,
    frame_slot_for: &BTreeMap<String, usize>,
    live_names: &BTreeSet<String>,
    occupied: &mut BTreeSet<String>,
) -> Vec<OpIR> {
    let mut ops = Vec::new();
    for name in live_names {
        let Some(slot) = frame_slot_for.get(name) else {
            continue;
        };
        let slot_name = split_frame_name("__molt_split_frame_index", occupied);
        ops.push(OpIR {
            kind: "const".to_string(),
            value: Some(*slot as i64),
            out: Some(slot_name.clone()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "index".to_string(),
            args: Some(vec![frame_name.to_string(), slot_name]),
            out: Some(name.clone()),
            ..OpIR::default()
        });
    }
    ops
}

fn split_frame_store_ops(
    frame_name: &str,
    frame_slot_for: &BTreeMap<String, usize>,
    live_names: &BTreeSet<String>,
    occupied: &mut BTreeSet<String>,
) -> Vec<OpIR> {
    let mut ops = Vec::new();
    for name in live_names {
        let Some(slot) = frame_slot_for.get(name) else {
            continue;
        };
        let slot_name = split_frame_name("__molt_split_frame_store_index", occupied);
        ops.push(OpIR {
            kind: "const".to_string(),
            value: Some(*slot as i64),
            out: Some(slot_name.clone()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "store_index".to_string(),
            args: Some(vec![frame_name.to_string(), slot_name, name.clone()]),
            ..OpIR::default()
        });
    }
    ops
}

fn split_status_return_ops(occupied: &mut BTreeSet<String>, should_continue: bool) -> Vec<OpIR> {
    let name = split_frame_name(
        if should_continue {
            "__molt_split_continue_true"
        } else {
            "__molt_split_continue_false"
        },
        occupied,
    );
    vec![
        OpIR {
            kind: "const_bool".to_string(),
            value: Some(i64::from(should_continue)),
            out: Some(name.clone()),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret".to_string(),
            var: Some(name),
            ..OpIR::default()
        },
    ]
}

fn split_rewrite_void_terminals_to_status(
    ops: Vec<OpIR>,
    occupied: &mut BTreeSet<String>,
    should_continue: bool,
) -> Vec<OpIR> {
    let mut rewritten = Vec::with_capacity(ops.len());
    for op in ops {
        if op.kind == "ret_void" {
            rewritten.extend(split_status_return_ops(occupied, should_continue));
        } else {
            rewritten.push(op);
        }
    }
    rewritten
}

fn verify_split_function_def_use(func: &FunctionIR) -> Result<(), String> {
    let mut defined: BTreeSet<String> = func.params.iter().cloned().collect();
    for (idx, op) in func.ops.iter().enumerate() {
        for name in split_ir_read_names(op) {
            if !defined.contains(&name) {
                return Err(format!(
                    "function `{}` op {} `{}` reads `{}` before definition",
                    func.name, idx, op.kind, name
                ));
            }
        }
        for name in split_ir_defined_names(op) {
            defined.insert(name);
        }
    }
    Ok(())
}

fn verify_split_generated_ops(func: &FunctionIR) -> Result<(), String> {
    for (idx, op) in func.ops.iter().enumerate() {
        if op.kind == "load_index" {
            return Err(format!(
                "function `{}` op {} uses non-canonical generated op `load_index`; use `index`",
                func.name, idx
            ));
        }
    }
    for (idx, op) in func.ops.iter().enumerate() {
        if op.out.as_deref().is_some_and(|out| {
            out.starts_with("__molt_split_frame_load_index")
                || out.starts_with("__molt_split_frame_store_index")
        }) {
            let next = func.ops.get(idx + 1).ok_or_else(|| {
                format!(
                    "function `{}` op {} split-frame slot const is missing consumer",
                    func.name, idx
                )
            })?;
            let expected_consumer = if op
                .out
                .as_deref()
                .is_some_and(|out| out.starts_with("__molt_split_frame_load_index"))
            {
                "index"
            } else {
                "store_index"
            };
            if next.kind != expected_consumer {
                return Err(format!(
                    "function `{}` op {} split-frame slot const is consumed by `{}` not `{}`",
                    func.name, idx, next.kind, expected_consumer
                ));
            }
        }
    }
    Ok(())
}

fn is_drop_fact_marker_op(op: &OpIR) -> bool {
    matches!(
        op.kind.as_str(),
        crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR
            | crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR
    )
}

/// Eliminate dead ops within each function of the SimpleIR.
///
/// An op is dead when:
/// 1. It is pure (no side effects).
/// 2. None of its defined names are referenced by any subsequent op's data
///    inputs in the same function.
///
/// Iterates to fixpoint (max 5 rounds) to catch cascading dead chains.
pub fn eliminate_dead_ops(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_DEAD_OP_ELIM").is_ok() {
        return;
    }

    let trace = std::env::var("MOLT_DEBUG_DEAD_OP_ELIM").is_ok();
    let mut total_removed = 0usize;

    for func in &mut ir.functions {
        for _round in 0..5 {
            // Build a set of all consumed names. `args` are always data
            // inputs; `var` is input for read/copy/return-like ops but is a
            // target definition for store_var and iter_next_unboxed.
            let mut consumed: HashSet<String> = HashSet::new();

            for op in &func.ops {
                if let Some(args) = &op.args {
                    for arg in args {
                        consumed.insert(arg.clone());
                    }
                }
                if let Some(v) = &op.var
                    && simple_ir_var_field_is_read(op)
                {
                    consumed.insert(v.clone());
                }
                // s_value can reference function names or variable names
                // in certain ops — conservatively keep anything it references.
                if let Some(sv) = &op.s_value {
                    // Only count as consumed if this is a load/copy-like op
                    // that reads the value. Calls reference functions, not locals.
                    if matches!(
                        op.kind.as_str(),
                        "copy_var" | "load_var" | "load_fast" | "store_var"
                    ) {
                        consumed.insert(sv.clone());
                    }
                }
            }

            let before = func.ops.len();
            let needs_scalar_plan = func.ops.iter().any(|op| {
                simple_ir_op_needs_scalar_plan_for_nonthrowing(op)
                    && !simple_ir_defined_names(op).is_empty()
                    && simple_ir_defined_names(op)
                        .iter()
                        .all(|name| !consumed.contains(*name))
            });
            let scalar_plan =
                needs_scalar_plan.then(|| ScalarRepresentationPlan::for_function_ir(func));
            let scalar_facts = needs_scalar_plan
                .then(|| SimpleIrScalarPurityFacts::for_function(func, scalar_plan.as_ref()));

            func.ops.retain(|op| {
                // Keep all ops whose execution is observable, including
                // potential exceptions and user-code dispatch.
                if !simple_ir_unused_result_is_removable(scalar_facts.as_ref(), op) {
                    return true;
                }

                // Keep nops (they're just markers, trivial to keep).
                if op.kind == "nop" {
                    return true;
                }

                let defined = simple_ir_defined_names(op);
                if !defined.is_empty() {
                    return defined.iter().any(|name| consumed.contains(*name));
                }

                // Ops without a result variable but with no side effects
                // are dead (e.g., a bare `build_list` with no assignment).
                // Conservatively keep them — they might be consumed by
                // stack-based implicit references we can't see.
                true
            });

            let removed = before - func.ops.len();
            total_removed += removed;

            if removed == 0 {
                break; // fixpoint
            }
        }
    }

    if trace && total_removed > 0 {
        eprintln!("dead-op-elim: removed {total_removed} dead ops across all functions");
    }
}

// ---------------------------------------------------------------------------
// Dead import elimination
//
// Removes `import` and `import_from` ops whose loaded module/name is never
// referenced by any subsequent op in the same function. This prevents pulling
// in entire stdlib modules for imports that the user code never actually uses.
// ---------------------------------------------------------------------------

/// Eliminate imports whose results are never consumed.
pub fn eliminate_dead_imports(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_DEAD_IMPORT_ELIM").is_ok() {
        return;
    }

    let trace = std::env::var("MOLT_DEBUG_DEAD_IMPORT_ELIM").is_ok();
    let mut total_removed = 0usize;

    for func in &mut ir.functions {
        // Build the set of all consumed variable names.
        let mut consumed: HashSet<String> = HashSet::new();
        for op in &func.ops {
            if let Some(args) = &op.args {
                for arg in args {
                    consumed.insert(arg.clone());
                }
            }
        }

        let before = func.ops.len();

        func.ops.retain(|op| {
            // Only target import ops.
            if !matches!(op.kind.as_str(), "import_name" | "import_from") {
                return true;
            }

            // If the import result is consumed, keep it.
            if let Some(var) = &op.var {
                if consumed.contains(var) {
                    return true;
                }
                // The import result is never referenced → dead import.
                return false;
            }

            // No result var — keep conservatively.
            true
        });

        let removed = before - func.ops.len();
        total_removed += removed;
    }

    if trace && total_removed > 0 {
        eprintln!("dead-import-elim: removed {total_removed} dead imports across all functions");
    }
}

// ---------------------------------------------------------------------------
// Megafunction splitting pass
//
// Cranelift's register allocator has O(n^2) behavior on very large functions.
// When a function exceeds max_ops (default 2000, env: MOLT_MAX_FUNCTION_OPS),
// this pass splits it at top-level statement boundaries (loop_depth=0,
// if_depth=0) into private __molt_chunk_{name}_{n} functions.  The original
// function is replaced with sequential call_internal ops to each chunk.
//
// Safety: never splits inside loops, if-blocks, or try-blocks.
// ---------------------------------------------------------------------------

/// Default maximum number of ops before a function is split into chunks.
///
/// Native frontend module chunking already targets 2000 ops, but lower/midend
/// rewrites can still inflate a chunk well past that budget. Keep the backend
/// splitter aligned with that native default so Cranelift does not see giant
/// `*_molt_module_chunk_*` functions slip through unsplit.
const DEFAULT_MAX_FUNCTION_OPS: usize = 2000;

/// Split a single large function into multiple chunk functions.
///
/// Returns `Err(func)` (giving back the original) if the function is small
/// enough or no safe split points exist; otherwise returns `Ok((stub, chunks))`
/// where `stub` is the replacement parent function and `chunks` are the
/// extracted pieces.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn split_large_function(
    func: FunctionIR,
    max_ops: usize,
) -> Result<(FunctionIR, Vec<FunctionIR>), Box<FunctionIR>> {
    if is_protected_runtime_entrypoint(&func.name) {
        return Err(Box::new(func));
    }

    if func.ops.len() <= max_ops {
        return Err(Box::new(func));
    }
    let original_for_split_failure = func.clone();
    let all_ops = &func.ops;
    let drop_fact_markers: Vec<OpIR> = all_ops
        .iter()
        .take_while(|op| is_drop_fact_marker_op(op))
        .cloned()
        .collect();
    let live_before = split_live_before_sets(all_ops);
    let defined_before = split_defined_before_sets(all_ops);

    // Exception handling ops (check_exception) are protected by the
    // forbidden-range mechanism below — the splitter never separates a
    // check_exception from its target label.  This allows safe splitting
    // of large stdlib functions that contain exception handlers.

    // ---------------------------------------------------------------
    // 1. Find safe split points (indices where depth == 0).
    //    A split point is the index of the *first* op of a new chunk,
    //    i.e. the boundary falls just before that index.
    //
    //    Additionally, we must not split between a `check_exception`
    //    and its target `label`/`state_label`, since the function
    //    compiler expects both to be in the same chunk.
    // ---------------------------------------------------------------

    // Build forbidden ranges: for each label_id referenced by
    // check_exception, jump, or br_if, find the span covering the
    // reference(s) and the label definition, and forbid splitting
    // within that range.
    //
    // This is critical after TIR optimization, which replaces structured
    // loop markers (loop_start/loop_end/loop_break_if_*) with
    // unstructured label/jump/br_if ops.  Without this protection, the
    // depth tracker sees depth=0 everywhere (no loop_start/loop_end to
    // increment/decrement) and the splitter can cut through the middle
    // of a linearized loop body, producing chunk functions whose control
    // flow falls through to Cranelift trap instructions (SIGILL).
    let mut label_positions: std::collections::BTreeMap<i64, usize> =
        std::collections::BTreeMap::new();
    let mut label_refs: std::collections::BTreeMap<i64, (usize, usize)> =
        std::collections::BTreeMap::new();
    for (idx, op) in func.ops.iter().enumerate() {
        match op.kind.as_str() {
            "label" | "state_label" => {
                if let Some(id) = op.value {
                    label_positions.insert(id, idx);
                }
            }
            "check_exception" | "jump" | "br_if" => {
                if let Some(id) = op.value {
                    let entry = label_refs.entry(id).or_insert((idx, idx));
                    entry.0 = entry.0.min(idx);
                    entry.1 = entry.1.max(idx);
                }
            }
            _ => {}
        }
    }
    // Compute forbidden ranges: a split point at index `sp` is forbidden
    // if it falls strictly between a label reference and its definition.
    let mut label_forbidden_ranges: Vec<(usize, usize, i64, usize)> = Vec::new();
    let cloneable_suffix_labels: std::collections::BTreeMap<i64, usize> = label_positions
        .iter()
        .filter_map(|(label_id, &label_idx)| {
            let suffix_ops = &func.ops[label_idx..];
            if suffix_ops.iter().any(|op| op.kind == "ret") {
                None
            } else {
                Some((*label_id, label_idx))
            }
        })
        .collect();
    let suffix_external_reads: std::collections::BTreeMap<i64, BTreeSet<String>> =
        cloneable_suffix_labels
            .iter()
            .map(|(&label_id, &label_idx)| {
                (
                    label_id,
                    split_suffix_external_reads(&func.ops[label_idx..]),
                )
            })
            .collect();
    for (label_id, (earliest_ref, latest_ref)) in &label_refs {
        if let Some(&label_idx) = label_positions.get(label_id) {
            let range_start = (*earliest_ref).min(label_idx);
            let range_end = (*latest_ref).max(label_idx);
            label_forbidden_ranges.push((range_start, range_end, *label_id, label_idx));
        }
    }

    // Also forbid splitting between matched if/end_if pairs.
    // After TIR optimization, the depth tracker may see depth=0 between
    // an `if` and its `end_if` because TIR-inserted store_var/load_var
    // ops reset the apparent nesting. Protecting the full if→end_if span
    // ensures the function compiler always sees matched pairs.
    let mut structural_forbidden_ranges: Vec<(usize, usize)> = Vec::new();
    {
        let mut if_stack: Vec<usize> = Vec::new();
        for (idx, op) in func.ops.iter().enumerate() {
            match op.kind.as_str() {
                "if" => if_stack.push(idx),
                "end_if" => {
                    if let Some(if_idx) = if_stack.pop() {
                        structural_forbidden_ranges.push((if_idx, idx));
                    }
                }
                _ => {}
            }
        }
    }

    let control_target = |op: &OpIR| -> Option<i64> {
        matches!(op.kind.as_str(), "check_exception" | "jump" | "br_if")
            .then_some(op.value)
            .flatten()
    };

    let suffix_can_clone_into_range =
        |label_id: i64, chunk_start: usize, chunk_end: usize| -> bool {
            let Some(&label_idx) = cloneable_suffix_labels.get(&label_id) else {
                return false;
            };
            if label_idx < chunk_end {
                return false;
            }
            let Some(required) = suffix_external_reads.get(&label_id) else {
                return false;
            };
            let available = split_available_names_for_suffix_clone(
                &func.params,
                &live_before,
                all_ops,
                chunk_start,
                chunk_end,
            );
            required.iter().all(|name| available.contains(name))
        };

    let chunk_refs_label = |chunk_start: usize, chunk_end: usize, label_id: i64| -> bool {
        all_ops[chunk_start..chunk_end]
            .iter()
            .any(|op| control_target(op) == Some(label_id))
    };

    let chunk_has_external_control_without_safe_clone =
        |chunk_start: usize, chunk_end: usize| -> bool {
            for op in &all_ops[chunk_start..chunk_end] {
                let Some(target_id) = control_target(op) else {
                    continue;
                };
                let Some(&label_idx) = label_positions.get(&target_id) else {
                    return true;
                };
                if (chunk_start..chunk_end).contains(&label_idx) {
                    continue;
                }
                if label_idx >= chunk_end
                    && suffix_can_clone_into_range(target_id, chunk_start, chunk_end)
                {
                    continue;
                }
                return true;
            }
            false
        };

    let is_forbidden = |sp: usize, chunk_start: usize| -> bool {
        for &(start, end) in &structural_forbidden_ranges {
            // sp is the first index of the new chunk; splitting here means
            // indices [0..sp) go to one chunk and [sp..) go to the next.
            // Forbidden if the range straddles the split point.
            if start < sp && sp <= end {
                return true;
            }
        }
        for &(start, end, label_id, label_idx) in &label_forbidden_ranges {
            if start < sp && sp <= end {
                if label_idx < sp {
                    return true;
                }
                if chunk_refs_label(chunk_start, sp, label_id)
                    && !suffix_can_clone_into_range(label_id, chunk_start, sp)
                {
                    return true;
                }
            }
        }
        chunk_has_external_control_without_safe_clone(chunk_start, sp)
    };

    let mut selected: Vec<usize> = Vec::new();
    let mut last_split = 0usize;
    let mut depth: i32 = 0;

    for (idx, op) in func.ops.iter().enumerate() {
        // Split only at top-level statement boundaries. A raw depth==0 op
        // index is not sufficient: large class statements emit thousands of
        // ops (method FuncNew, CLASS_DEF, export, annotation wiring) without
        // increasing structured-control depth, so splitting at an arbitrary
        // op boundary can sever one logical statement across chunks.
        let is_stmt_boundary = op.kind == "line";
        if depth == 0
            && idx > 0
            && is_stmt_boundary
            && idx - last_split >= max_ops
            && !is_forbidden(idx, last_split)
        {
            selected.push(idx);
            last_split = idx;
        }

        match op.kind.as_str() {
            // Openers -- increase nesting depth
            "if" | "loop_start" | "loop_index_start" | "for_iter_start" | "while_start"
            | "try_start" | "async_for_start" => {
                depth += 1;
            }
            // Closers -- decrease nesting depth
            "end_if" | "loop_end" | "loop_index_end" | "for_iter_end" | "while_end" | "try_end"
            | "async_for_end" => {
                depth -= 1;
            }
            _ => {}
        }
    }

    // If no selected splits, the function is too deeply nested to split.
    if selected.is_empty() {
        return Err(Box::new(original_for_split_failure));
    }

    // ---------------------------------------------------------------
    // 3. Partition ops into chunks at the selected split points.
    // ---------------------------------------------------------------
    let mut boundaries: Vec<usize> = Vec::new();
    boundaries.push(0);
    boundaries.extend_from_slice(&selected);
    boundaries.push(func.ops.len());

    // Validate: ensure no chunk exceeds max_ops. If any chunk is oversized,
    // the function has a deeply nested region that can't be split cleanly.
    for window in boundaries.windows(2) {
        let chunk_size = window[1] - window[0];
        if chunk_size > max_ops * 2 {
            // Allow up to 2x max_ops for the final chunk — beyond that,
            // return Err to fall back to single-module compilation.
            return Err(Box::new(original_for_split_failure));
        }
    }

    let sanitized_name = func
        .name
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "_");

    let func_returns_value = func.ops.iter().any(|op| op.kind == "ret");
    for (idx, op) in func.ops.iter().enumerate() {
        if matches!(op.kind.as_str(), "ret" | "ret_void") && idx + 1 != func.ops.len() {
            return Err(Box::new(original_for_split_failure.clone()));
        }
    }
    let mut occupied_names = split_collect_names(all_ops, &func.params);
    let frame_name = split_frame_name("__molt_split_frame", &mut occupied_names);
    let mut frame_names = BTreeSet::new();
    for &boundary in boundaries
        .iter()
        .skip(1)
        .take(boundaries.len().saturating_sub(2))
    {
        for name in &live_before[boundary] {
            if defined_before[boundary].contains(name) {
                frame_names.insert(name.clone());
            }
        }
    }
    let frame_slot_for: BTreeMap<String, usize> = frame_names
        .iter()
        .enumerate()
        .map(|(idx, name)| (name.clone(), idx))
        .collect();
    let uses_split_frame = !frame_slot_for.is_empty();
    let mut next_synthetic_label = label_positions
        .keys()
        .max()
        .copied()
        .unwrap_or(0)
        .saturating_add(1);
    let exception_return_label = next_synthetic_label;
    next_synthetic_label = next_synthetic_label.saturating_add(1);

    struct ChunkPlan {
        name: String,
        returns_value: bool,
        returns_control_status: bool,
    }

    let mut chunks: Vec<FunctionIR> = Vec::new();
    let mut plans: Vec<ChunkPlan> = Vec::new();
    for i in 0..boundaries.len() - 1 {
        let start = boundaries[i];
        let end = boundaries[i + 1];
        let mut chunk_ops: Vec<OpIR> = all_ops[start..end].to_vec();
        if !drop_fact_markers.is_empty() {
            chunk_ops.retain(|op| !is_drop_fact_marker_op(op));
            let mut prefixed = drop_fact_markers.clone();
            prefixed.extend(chunk_ops);
            chunk_ops = prefixed;
        }
        let live_in: BTreeSet<String> = live_before[start]
            .iter()
            .filter(|name| frame_slot_for.contains_key(*name))
            .cloned()
            .collect();
        let live_out: BTreeSet<String> = live_before[end]
            .iter()
            .filter(|name| frame_slot_for.contains_key(*name))
            .cloned()
            .collect();

        // Collect label IDs defined in THIS chunk.
        let mut chunk_labels: std::collections::BTreeSet<i64> = chunk_ops
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "label" | "state_label"))
            .filter_map(|op| op.value)
            .collect();

        // If the chunk references a shared exception/cleanup tail that starts
        // later in the original function, clone that suffix into the chunk so
        // local check_exception/jump/br_if targets stay valid after splitting.
        let mut normal_skip_label_for_cloned_suffix = None;
        let suffix_clone_start = chunk_ops
            .iter()
            .filter_map(|op| {
                if !matches!(op.kind.as_str(), "check_exception" | "jump" | "br_if") {
                    return None;
                }
                let target_id = op.value?;
                if chunk_labels.contains(&target_id) {
                    return None;
                }
                let &label_idx = cloneable_suffix_labels.get(&target_id)?;
                (label_idx >= end && suffix_can_clone_into_range(target_id, start, end))
                    .then_some(label_idx)
            })
            .min();
        if let Some(suffix_start) = suffix_clone_start {
            let skip_label = next_synthetic_label;
            next_synthetic_label = next_synthetic_label.saturating_add(1);
            normal_skip_label_for_cloned_suffix = Some(skip_label);
            chunk_ops.push(OpIR {
                kind: "jump".to_string(),
                value: Some(skip_label),
                ..OpIR::default()
            });
            chunk_ops.extend_from_slice(&all_ops[suffix_start..]);
            chunk_ops.push(OpIR {
                kind: "label".to_string(),
                value: Some(skip_label),
                ..OpIR::default()
            });
            chunk_labels = chunk_ops
                .iter()
                .filter(|op| matches!(op.kind.as_str(), "label" | "state_label"))
                .filter_map(|op| op.value)
                .collect();
            if chunk_ops.len() > max_ops * 2 {
                return Err(Box::new(original_for_split_failure.clone()));
            }
        }

        if chunk_ops.iter().any(|op| {
            control_target(op).is_some_and(|target_id| !chunk_labels.contains(&target_id))
        }) {
            return Err(Box::new(original_for_split_failure.clone()));
        }

        let chunk_name = format!("__molt_chunk_{sanitized_name}_{i}");
        let (returns_value, returns_control_status) = if func_returns_value {
            let terminal = if chunk_ops
                .last()
                .is_some_and(|op| matches!(op.kind.as_str(), "ret" | "ret_void"))
            {
                chunk_ops.pop()
            } else {
                None
            };
            let returns_value = terminal.as_ref().is_some_and(|op| op.kind == "ret");
            if uses_split_frame {
                let stores = split_frame_store_ops(
                    &frame_name,
                    &frame_slot_for,
                    &live_out,
                    &mut occupied_names,
                );
                if let Some(skip_label) = normal_skip_label_for_cloned_suffix {
                    let Some(insert_idx) = chunk_ops
                        .iter()
                        .position(|op| op.kind == "jump" && op.value == Some(skip_label))
                    else {
                        return Err(Box::new(original_for_split_failure.clone()));
                    };
                    chunk_ops.splice(insert_idx..insert_idx, stores);
                } else {
                    chunk_ops.extend(stores);
                }
                let mut prefixed = split_frame_load_ops(
                    &frame_name,
                    &frame_slot_for,
                    &live_in,
                    &mut occupied_names,
                );
                prefixed.extend(chunk_ops);
                chunk_ops = prefixed;
            }
            chunk_ops.push(terminal.unwrap_or_else(|| OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }));
            (returns_value, false)
        } else {
            chunk_ops =
                split_rewrite_void_terminals_to_status(chunk_ops, &mut occupied_names, false);
            if uses_split_frame {
                let stores = split_frame_store_ops(
                    &frame_name,
                    &frame_slot_for,
                    &live_out,
                    &mut occupied_names,
                );
                if let Some(skip_label) = normal_skip_label_for_cloned_suffix {
                    let Some(insert_idx) = chunk_ops
                        .iter()
                        .position(|op| op.kind == "jump" && op.value == Some(skip_label))
                    else {
                        return Err(Box::new(original_for_split_failure.clone()));
                    };
                    chunk_ops.splice(insert_idx..insert_idx, stores);
                } else {
                    chunk_ops.extend(stores);
                }
                let mut prefixed = split_frame_load_ops(
                    &frame_name,
                    &frame_slot_for,
                    &live_in,
                    &mut occupied_names,
                );
                prefixed.extend(chunk_ops);
                chunk_ops = prefixed;
            }
            chunk_ops.extend(split_status_return_ops(&mut occupied_names, true));
            (false, true)
        };
        let mut chunk_params = func.params.clone();
        if uses_split_frame {
            chunk_params.push(frame_name.clone());
        }
        let chunk_param_types =
            split_param_types_for_names(&func.params, func.param_types.as_ref(), &chunk_params);
        chunks.push(FunctionIR {
            name: chunk_name.clone(),
            params: chunk_params,
            ops: chunk_ops,
            param_types: chunk_param_types,
            source_file: None,
            is_extern: false,
        });
        plans.push(ChunkPlan {
            name: chunk_name,
            returns_value,
            returns_control_status,
        });
    }

    // ---------------------------------------------------------------
    // 4. Build the stub parent function. Values defined in one chunk and read
    //    by later chunks travel through one explicit heap frame instead of
    //    relying on per-function entry defaults.
    // ---------------------------------------------------------------
    let mut stub_ops: Vec<OpIR> = Vec::new();
    if uses_split_frame {
        let mut frame_init_args = Vec::with_capacity(frame_slot_for.len());
        for _ in 0..frame_slot_for.len() {
            let slot_init = split_frame_name("__molt_split_frame_init", &mut occupied_names);
            stub_ops.push(OpIR {
                kind: "const_none".to_string(),
                out: Some(slot_init.clone()),
                ..OpIR::default()
            });
            frame_init_args.push(slot_init);
        }
        stub_ops.push(OpIR {
            kind: "list_new".to_string(),
            args: Some(frame_init_args),
            out: Some(frame_name.clone()),
            ..OpIR::default()
        });
    }
    for (ci, plan) in plans.iter().enumerate() {
        let mut call_args = func.params.clone();
        if uses_split_frame {
            call_args.push(frame_name.clone());
        }
        let chunk_continue_name = format!("__chunk_continue_{ci}");
        stub_ops.push(OpIR {
            kind: "call_internal".to_string(),
            s_value: Some(plan.name.clone()),
            args: Some(call_args),
            out: Some(if plan.returns_control_status {
                chunk_continue_name.clone()
            } else if plan.returns_value {
                "__chunk_ret".to_string()
            } else {
                format!("__chunk_discard_{ci}")
            }),
            ..OpIR::default()
        });
        stub_ops.push(OpIR {
            kind: "check_exception".to_string(),
            value: Some(exception_return_label),
            ..OpIR::default()
        });
        if plan.returns_control_status {
            let continue_label = next_synthetic_label;
            next_synthetic_label = next_synthetic_label.saturating_add(1);
            stub_ops.push(OpIR {
                kind: "br_if".to_string(),
                args: Some(vec![chunk_continue_name]),
                value: Some(continue_label),
                ..OpIR::default()
            });
            stub_ops.push(OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            });
            stub_ops.push(OpIR {
                kind: "label".to_string(),
                value: Some(continue_label),
                ..OpIR::default()
            });
            continue;
        }
        if plan.returns_value {
            stub_ops.push(OpIR {
                kind: "ret".to_string(),
                var: Some("__chunk_ret".to_string()),
                ..OpIR::default()
            });
            continue;
        }
    }
    if func_returns_value {
        stub_ops.push(OpIR {
            kind: "const_none".to_string(),
            out: Some("__chunk_missing_ret".to_string()),
            ..OpIR::default()
        });
        stub_ops.push(OpIR {
            kind: "ret".to_string(),
            var: Some("__chunk_missing_ret".to_string()),
            ..OpIR::default()
        });
    } else {
        stub_ops.push(OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        });
    }
    stub_ops.push(OpIR {
        kind: "label".to_string(),
        value: Some(exception_return_label),
        ..OpIR::default()
    });
    if func_returns_value {
        stub_ops.push(OpIR {
            kind: "const_none".to_string(),
            out: Some("__chunk_exception_ret".to_string()),
            ..OpIR::default()
        });
        stub_ops.push(OpIR {
            kind: "ret".to_string(),
            var: Some("__chunk_exception_ret".to_string()),
            ..OpIR::default()
        });
    } else {
        stub_ops.push(OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        });
    }

    let stub = FunctionIR {
        name: func.name,
        params: func.params,
        ops: stub_ops,
        param_types: func.param_types,
        source_file: None,
        is_extern: false,
    };

    for chunk in &chunks {
        if let Err(detail) = verify_split_function_def_use(chunk) {
            panic!("megafunction split produced invalid chunk IR: {detail}");
        }
        if let Err(detail) = verify_split_generated_ops(chunk) {
            panic!("megafunction split produced non-canonical chunk IR: {detail}");
        }
    }
    if let Err(detail) = verify_split_function_def_use(&stub) {
        panic!("megafunction split produced invalid stub IR: {detail}");
    }
    if let Err(detail) = verify_split_generated_ops(&stub) {
        panic!("megafunction split produced non-canonical stub IR: {detail}");
    }

    Ok((stub, chunks))
}

/// Apply megafunction splitting to all oversized functions in the IR.
///
/// Call this before the main compilation loop so that the chunk functions
/// are present in `ir.functions` and will be compiled normally.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn split_megafunctions(ir: &mut SimpleIR) {
    split_megafunctions_with_filter(ir, |_| true);
}

pub fn split_megafunctions_with_filter(
    ir: &mut SimpleIR,
    should_split: impl Fn(&FunctionIR) -> bool,
) {
    let max_ops: usize = std::env::var("MOLT_MAX_FUNCTION_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_FUNCTION_OPS);

    let mut new_functions: Vec<FunctionIR> = Vec::new();
    let old_functions = std::mem::take(&mut ir.functions);

    for func in old_functions {
        let op_count = func.ops.len();
        if !should_split(&func) {
            new_functions.push(func);
            continue;
        }
        match split_large_function(func, max_ops) {
            Ok((stub, chunks)) => {
                eprintln!(
                    "MOLT_BACKEND: split `{}` ({} ops) into {} chunks",
                    stub.name,
                    op_count,
                    chunks.len()
                );
                // Insert chunks first so they are defined before the stub calls them.
                new_functions.extend(chunks);
                new_functions.push(stub);
            }
            Err(original) => {
                new_functions.push(*original);
            }
        }
    }

    ir.functions = new_functions;
}

/// Rewrite loops that contain `state_yield` in stateful (generator/async)
/// functions so the native backend can resume inside the loop body.
///
/// Problem: the native Cranelift backend tracks loop context on a runtime
/// `loop_stack`.  When a generator yields inside a loop and later resumes,
/// the `loop_start` that pushed the frame is skipped (the state machine
/// jumps directly to the resume block).  Any subsequent `loop_continue`
/// finds an empty `loop_stack` and falls through, ending the generator
/// after a single yield.
///
/// Fix: for every loop that encloses a `state_yield`, insert a
/// `state_label` at the loop body start and rewrite `loop_continue` ops
/// to `jump` ops targeting that label.  The native backend treats
/// `state_label` as a resume-eligible block and `jump` as an
/// unconditional branch — both work correctly across state-machine
/// boundaries.
pub fn rewrite_stateful_loops(func_ir: &mut FunctionIR) {
    // Only transform stateful functions (generators / async).
    let is_stateful = func_ir.ops.iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "state_switch"
                | "state_transition"
                | "state_yield"
                | "chan_send_yield"
                | "chan_recv_yield"
        )
    });
    if !is_stateful {
        return;
    }

    // Find the maximum state ID already in use so we can allocate fresh IDs.
    let mut max_state_id: i64 = 0;
    for op in &func_ir.ops {
        if let Some(id) = op.value
            && matches!(
                op.kind.as_str(),
                "state_yield"
                    | "state_transition"
                    | "state_label"
                    | "label"
                    | "chan_send_yield"
                    | "chan_recv_yield"
            )
        {
            max_state_id = max_state_id.max(id);
        }
    }
    let mut next_state_id = max_state_id + 100; // leave headroom

    // Build a stack of loop start indices and find which loops contain yields.
    struct LoopInfo {
        start_idx: usize,
        end_idx: usize,
        has_yield: bool,
        continues: Vec<usize>,
        breaks: Vec<usize>,
        break_if_trues: Vec<usize>,
        break_if_falses: Vec<usize>,
    }
    let mut loop_stack: Vec<LoopInfo> = Vec::new();
    let mut finished_loops: Vec<LoopInfo> = Vec::new();

    for (idx, op) in func_ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" | "loop_index_start" => {
                loop_stack.push(LoopInfo {
                    start_idx: idx,
                    end_idx: 0,
                    has_yield: false,
                    continues: Vec::new(),
                    breaks: Vec::new(),
                    break_if_trues: Vec::new(),
                    break_if_falses: Vec::new(),
                });
            }
            "state_yield" | "chan_send_yield" | "chan_recv_yield" => {
                for frame in loop_stack.iter_mut() {
                    frame.has_yield = true;
                }
            }
            "loop_continue" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.continues.push(idx);
                }
            }
            "loop_break" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.breaks.push(idx);
                }
            }
            "loop_break_if_true" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.break_if_trues.push(idx);
                }
            }
            "loop_break_if_false" => {
                if let Some(frame) = loop_stack.last_mut() {
                    frame.break_if_falses.push(idx);
                }
            }
            "loop_break_if_exception" => {
                // `loop_break_if_exception` is emitted by the frontend ONLY in
                // functions WITHOUT the exception stack (function_exception_label
                // == None).  Generator/coroutine bodies — the only functions
                // lowered to a yield state machine here — are always compiled
                // with the exception stack (generator polls default to
                // needs_exception_stack=True), so this op can never appear inside
                // a yield-bearing loop.  Assert the invariant: if it is ever
                // violated, the state-machine rewrite below would silently drop
                // the exception break and re-introduce the infinite-loop/OOM bug.
                debug_assert!(
                    false,
                    "loop_break_if_exception inside a yield state-machine loop \
                     ({}@op{}) — generator polls must be needs_exception_stack=True",
                    func_ir.name, idx
                );
            }
            "loop_end" => {
                if let Some(mut frame) = loop_stack.pop() {
                    frame.end_idx = idx;
                    finished_loops.push(frame);
                }
            }
            _ => {}
        }
    }

    let yield_loops: Vec<LoopInfo> = finished_loops.into_iter().filter(|l| l.has_yield).collect();

    if yield_loops.is_empty() {
        return;
    }

    // For each yield-containing loop, allocate TWO state labels:
    //   body_label  — at loop body start (continue / back-edge target)
    //   after_label — after loop_end (break target)
    // Replace ALL structured loop ops with labels and jumps so the
    // state machine works correctly on resume.
    let mut body_label_for_start: BTreeMap<usize, i64> = BTreeMap::new();
    let mut after_label_for_end: BTreeMap<usize, i64> = BTreeMap::new();
    let mut continue_target: BTreeMap<usize, i64> = BTreeMap::new();
    let mut break_target: BTreeMap<usize, i64> = BTreeMap::new();

    for info in &yield_loops {
        let body_label = next_state_id;
        next_state_id += 1;
        let after_label = next_state_id;
        next_state_id += 1;

        body_label_for_start.insert(info.start_idx, body_label);
        after_label_for_end.insert(info.end_idx, after_label);

        for &ci in &info.continues {
            continue_target.insert(ci, body_label);
        }
        for &bi in &info.breaks {
            break_target.insert(bi, after_label);
        }
        for &bi in &info.break_if_trues {
            break_target.insert(bi, after_label);
        }
        for &bi in &info.break_if_falses {
            break_target.insert(bi, after_label);
        }
    }

    // Rebuild the ops list, replacing structured loop ops with labels/jumps.
    let old_ops = std::mem::take(&mut func_ir.ops);
    let mut new_ops: Vec<OpIR> = Vec::with_capacity(old_ops.len() + yield_loops.len() * 4);

    for (idx, op) in old_ops.into_iter().enumerate() {
        if let Some(&body_label) = body_label_for_start.get(&idx) {
            // Replace loop_start with state_label (loop body entry).
            new_ops.push(OpIR {
                kind: "state_label".to_string(),
                value: Some(body_label),
                ..OpIR::default()
            });
        } else if let Some(&after_label) = after_label_for_end.get(&idx) {
            // Replace loop_end with state_label (break target).
            new_ops.push(OpIR {
                kind: "state_label".to_string(),
                value: Some(after_label),
                ..OpIR::default()
            });
        } else if let Some(&target) = continue_target.get(&idx) {
            // Replace loop_continue with jump to body label.
            new_ops.push(OpIR {
                kind: "jump".to_string(),
                value: Some(target),
                ..OpIR::default()
            });
        } else if let Some(&target) = break_target.get(&idx) {
            match op.kind.as_str() {
                "loop_break" => {
                    new_ops.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(target),
                        ..OpIR::default()
                    });
                }
                "loop_break_if_true" => {
                    // Expand: if(cond) { jump(after_label) } end_if
                    new_ops.push(OpIR {
                        kind: "if".to_string(),
                        args: op.args.clone(),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(target),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    });
                }
                "loop_break_if_false" => {
                    let cond = op
                        .args
                        .as_ref()
                        .and_then(|a| a.first().cloned())
                        .unwrap_or_default();
                    let not_var = format!("__slr_not_{idx}");
                    new_ops.push(OpIR {
                        kind: "not".to_string(),
                        args: Some(vec![cond]),
                        out: Some(not_var.clone()),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "if".to_string(),
                        args: Some(vec![not_var]),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(target),
                        ..OpIR::default()
                    });
                    new_ops.push(OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    });
                }
                _ => new_ops.push(op),
            }
        } else {
            new_ops.push(op);
        }
    }

    func_ir.ops = new_ops;
}

/// Compute the set of intrinsic names the app resolves through the name-based
/// runtime lookup path (`molt_require_intrinsic_runtime` /
/// `molt_load_intrinsic_runtime`).  This mirrors the WASM manifest scan in
/// `wasm.rs` (the `manifest_intrinsic_names` construction): for each function,
/// a `const_str` op's output is the candidate intrinsic name, a `builtin_func`
/// op producing the runtime-lookup helper marks its output as a lookup var, and
/// a `call_func` whose first arg is a lookup var and whose second arg is a
/// const-string output names the intrinsic the app reaches by name.
///
/// The native backend emits a per-app Cranelift resolver covering exactly this
/// set, so `resolve_symbol`/`resolve_core_symbol` (which address-take every
/// intrinsic) become native-unreachable and are dead-stripped.
///
/// This MUST be called over ALL functions including externs (before the
/// `is_extern` filter), so the `stdlib_shared.o` partition's intrinsic uses are
/// covered.
/// Compute the per-app intrinsic manifest, obtaining the linked runtime
/// staticlib's symbol set on demand — fail-closed exactly when it matters.
///
/// A module with NO `molt_`-prefixed `const_str` anywhere (the CLI's empty
/// post-build feature-probe module, or an app that reaches every intrinsic by
/// direct call) has a necessarily-empty manifest under ANY symbol set: every
/// manifest entry is a `const_str` value that is a member of the symbol set, and
/// all runtime intrinsics are `molt_`-prefixed (the CLI extractor keeps exactly
/// the staticlib's `molt_*` text symbols). The resolver then emits its trivial
/// zero-entry "always not found" form with no relocations, so the staticlib
/// symbol set is not a precondition there. Requiring it unconditionally made the
/// CLI's post-build backend feature probe — which runs the backend on an empty
/// module with no symbol file staged — panic in
/// [`runtime_intrinsic_symbols_required`](crate::intrinsic_symbols::runtime_intrinsic_symbols_required),
/// wedging every backend rebuild behind a "feature mismatch; cleaning and
/// rebuilding" loop. For any module that COULD name an intrinsic (some
/// `molt_`-prefixed `const_str` exists), the symbol set remains a hard
/// precondition and absence still fails the build closed — the
/// dangling-relocation corruption class this guards is unchanged.
///
/// This is the single manifest entry point for every resolver-emitting caller
/// (native `SimpleBackend::compile`, the orchestrator's split/batch paths); the
/// two-argument [`compute_intrinsic_manifest`] is the set-supplied core it and
/// the unit tests share.
pub fn compute_intrinsic_manifest_checked(functions: &[FunctionIR]) -> BTreeSet<String> {
    let any_candidate = functions.iter().any(|f| {
        f.ops.iter().any(|op| {
            op.kind == "const_str"
                && op
                    .s_value
                    .as_deref()
                    .is_some_and(|v| v.starts_with("molt_"))
        })
    });
    if !any_candidate {
        return BTreeSet::new();
    }
    let runtime_intrinsic_symbols = crate::intrinsic_symbols::runtime_intrinsic_symbols_required();
    compute_intrinsic_manifest(functions, &runtime_intrinsic_symbols)
}

pub fn compute_intrinsic_manifest(
    functions: &[FunctionIR],
    runtime_intrinsic_symbols: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut manifest_intrinsic_names: BTreeSet<String> = BTreeSet::new();
    // Every `const_str` whose value names a runtime intrinsic is a candidate the
    // app may resolve by name at runtime. The intrinsic name reaches
    // `require_intrinsic` / `load_intrinsic` either directly (`require_intrinsic(
    // "molt_foo")`) or — crucially — through a stdlib wrapper such as
    // `_require_callable_intrinsic("molt_gc_collect")` or `gc.py`'s
    // `_require_intrinsic(name)` where `name` flows from a constant. The narrow
    // `call_func(runtime_lookup_var, const_str)` shape only catches the direct,
    // single-call case and misses every wrapper-indirected name, so the resolver
    // would return 0 for them and the runtime would raise "intrinsic unavailable"
    // (and, because `resolve_core_symbol` is dead-stripped on native, the symbol
    // is not even present). Capturing the const-string names structurally — and
    // validating them against the symbols the linked runtime staticlib actually
    // defines — is the complete, robust manifest.
    // Any `const_str` whose value is a real runtime intrinsic symbol is a name
    // the app may resolve dynamically. The name reaches `require_intrinsic` /
    // `load_intrinsic` through arbitrary data flow: directly
    // (`require_intrinsic("molt_foo")`), through a wrapper call
    // (`_require_callable_intrinsic("molt_gc_collect")`), or — crucially — stored
    // in an object field and read back later (sys.py's
    // `_LazyIntrinsic("molt_sys_version_info", default)` stashes the name in
    // `self._name` and calls `_require_intrinsic(self._name)` on first use). A
    // call-argument-only scan misses the object-field case, which silently
    // degrades to the wrapper's fallback value (e.g. `sys.version_info` reverting
    // to the 3.12 default under a 3.13 target) rather than crashing — exactly the
    // class of bug a too-narrow manifest produces. Capturing every const_str
    // whose value is a real intrinsic symbol is the only data-flow-complete
    // manifest. The `is_candidate_intrinsic_name` filter (exact membership in the
    // linked staticlib's intrinsic symbol set) keeps this precise: it excludes
    // free-text strings that merely begin with `molt_` and intrinsics feature-gated
    // out of the active stdlib profile, so the resolver never takes the address of a
    // symbol the linker cannot resolve, and `-dead_strip` still removes every
    // intrinsic whose name appears nowhere as a string constant. The symbol set is
    // required (no heuristic fallback): an unknown set fails the build closed at the
    // caller rather than guessing and re-creating the dangling-relocation corruption.
    // The runtime resolves intrinsics through the per-app resolver by manifest
    // symbol. Intrinsic names are required to match linker symbols, so the
    // manifest scan keys directly on the captured const string and fails closed
    // when a name has no runtime symbol.
    for func_ir in functions {
        for op in &func_ir.ops {
            if op.kind == "const_str"
                && let Some(val) = op.s_value.as_deref()
                && is_candidate_intrinsic_name(val, runtime_intrinsic_symbols)
            {
                manifest_intrinsic_names.insert(val.to_owned());
            }
        }
    }
    manifest_intrinsic_names
}

/// Decide whether a `const_str` value names a runtime intrinsic the app resolver
/// may safely take the address of.
///
/// Membership in the set of intrinsic symbols the linked runtime staticlib
/// *defines* (extracted by the CLI for the active stdlib profile and threaded in)
/// is the authoritative, exact filter: it excludes diagnostic strings that merely
/// begin with `molt_` (e.g. `"molt_sys_platform intrinsic unavailable"`) AND
/// intrinsics that are feature-gated out of the active stdlib profile (e.g. crypto
/// on the micro profile). Taking the address of an absent symbol via a pointer
/// relocation would leave an unresolved relocation the linker cannot satisfy — the
/// precise cause of the link failure / Mach-O header corruption this resolver
/// design exists to prevent.
///
/// There is deliberately NO heuristic fallback: a `molt_`-prefixed identifier that
/// passes a structural shape check can still be absent from the active profile's
/// staticlib, so guessing re-creates the dangling-relocation corruption class. The
/// exact symbol set is therefore a hard precondition — native callers that feed
/// the resolver obtain it via `runtime_intrinsic_symbols_required`, which fails the
/// build closed (with an actionable diagnostic) when it is unavailable rather than
/// emitting a corrupt binary.
fn is_candidate_intrinsic_name(name: &str, runtime_intrinsic_symbols: &BTreeSet<String>) -> bool {
    runtime_intrinsic_symbols.contains(name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::passes::effects::EffectProof;

    fn make_op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    fn make_const_int(out: &str, val: i64) -> OpIR {
        OpIR {
            kind: "const".to_string(),
            value: Some(val),
            out: Some(out.to_string()),
            ..Default::default()
        }
    }

    fn make_store_var(var: &str, arg: &str) -> OpIR {
        OpIR {
            kind: "store_var".to_string(),
            var: Some(var.to_string()),
            args: Some(vec![arg.to_string()]),
            ..Default::default()
        }
    }

    fn make_const_str(out: &str, value: &str) -> OpIR {
        OpIR {
            kind: "const_str".to_string(),
            out: Some(out.to_string()),
            s_value: Some(value.to_string()),
            ..Default::default()
        }
    }

    fn make_call_func(out: &str, callee: &str, args: &[&str]) -> OpIR {
        let mut full_args = vec![callee.to_string()];
        full_args.extend(args.iter().map(|a| a.to_string()));
        OpIR {
            kind: "call_func".to_string(),
            out: Some(out.to_string()),
            args: Some(full_args),
            ..Default::default()
        }
    }

    fn manifest_func(ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: "m".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops,
            ..Default::default()
        }
    }

    /// A const-string intrinsic name passed as a call argument (the wrapper case,
    /// e.g. `_require_callable_intrinsic("molt_gc_collect")`) is captured even
    /// though it is not the direct `call_func(require_intrinsic, const_str)` shape.
    /// This is the regression the broadened scan fixes (`gc.collect()` etc.).
    #[test]
    fn manifest_captures_wrapper_indirected_intrinsic_name() {
        let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
        let func = manifest_func(vec![
            make_const_str("name", "molt_gc_collect"),
            // `_require_callable_intrinsic(name)` — a user wrapper call.
            make_call_func("res", "wrapper", &["name"]),
        ]);
        let manifest = compute_intrinsic_manifest(&[func], &symbols);
        assert!(
            manifest.contains("molt_gc_collect"),
            "wrapper-indirected intrinsic name must be in the manifest"
        );
    }

    /// `compute_intrinsic_manifest_checked` on a module with NO `molt_`-prefixed
    /// const_str (the CLI's empty post-build feature-probe module, or an app
    /// reaching every intrinsic by direct call) yields the empty manifest WITHOUT
    /// requiring the staticlib symbol set. This is the regression for the
    /// probe-panic loop: the probe runs the backend on `functions: []` with no
    /// `MOLT_RUNTIME_INTRINSIC_SYMBOLS` staged, and unconditionally requiring the
    /// set wedged every backend rebuild behind "feature mismatch; cleaning and
    /// rebuilding".
    #[test]
    fn manifest_checked_empty_module_needs_no_symbol_set() {
        // The empty probe module.
        assert!(compute_intrinsic_manifest_checked(&[]).is_empty());
        // A module with ops but no molt_-prefixed const_str.
        let func = manifest_func(vec![
            make_const_str("s", "hello world"),
            make_call_func("res", "print", &["s"]),
        ]);
        assert!(compute_intrinsic_manifest_checked(&[func]).is_empty());
    }

    /// A const-string that names a symbol absent from the linked staticlib (e.g.
    /// a crypto intrinsic on the micro profile) must NOT be captured — taking its
    /// address would leave an unresolvable relocation.
    #[test]
    fn manifest_excludes_intrinsic_absent_from_staticlib() {
        let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
        let func = manifest_func(vec![
            make_const_str("name", "molt_pbkdf2_hmac"), // not in `symbols`
            make_call_func("res", "wrapper", &["name"]),
        ]);
        let manifest = compute_intrinsic_manifest(&[func], &symbols);
        assert!(
            !manifest.contains("molt_pbkdf2_hmac"),
            "an intrinsic absent from the staticlib must never be address-taken"
        );
    }

    /// A const-string that merely begins with `molt_` but is free-text (a
    /// diagnostic message, not a symbol) must not be captured.
    #[test]
    fn manifest_excludes_non_symbol_molt_strings() {
        let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
        let func = manifest_func(vec![
            make_const_str("msg", "molt_sys_platform intrinsic unavailable"),
            make_call_func("res", "panic", &["msg"]),
        ]);
        let manifest = compute_intrinsic_manifest(&[func], &symbols);
        assert!(
            manifest.is_empty(),
            "free-text molt_ strings must not enter the manifest"
        );
    }

    /// A const-string intrinsic name that is stored (not passed directly to a
    /// call) MUST still be captured: the name can flow through an object field
    /// and be resolved later (sys.py's `_LazyIntrinsic` stashes the name in
    /// `self._name`). Missing it silently degrades to the wrapper's fallback
    /// value, so the data-flow-complete scan keeps every intrinsic-named
    /// const_str.
    #[test]
    fn manifest_captures_stored_intrinsic_name() {
        let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
        let func = manifest_func(vec![
            make_const_str("name", "molt_gc_collect"),
            // `name` is stored in an object field, not passed directly to a call —
            // it is still resolved later via `_require_intrinsic(self._name)`.
            make_store_var("slot", "name"),
        ]);
        let manifest = compute_intrinsic_manifest(&[func], &symbols);
        assert!(
            manifest.contains("molt_gc_collect"),
            "an intrinsic name stored for later resolution must be captured"
        );
    }

    /// The filter is EXACT membership in the staticlib symbol set — there is no
    /// structural heuristic fallback. A well-formed `molt_`-prefixed identifier that
    /// is NOT in the set (e.g. an intrinsic feature-gated out of the active stdlib
    /// profile) must be excluded, because address-taking an absent symbol leaves a
    /// dangling relocation that corrupts the binary. This locks in the contract that
    /// replaced the prior "degrade safely" heuristic (which itself enabled the
    /// corruption class).
    #[test]
    fn manifest_excludes_well_formed_name_absent_from_symbol_set() {
        // Only `molt_gc_collect` is defined by the (simulated) staticlib.
        let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
        let func = manifest_func(vec![
            make_const_str("present", "molt_gc_collect"),
            make_call_func("r1", "wrapper", &["present"]),
            // A structurally valid intrinsic identifier that is feature-gated out of
            // this profile's staticlib — must NOT be address-taken.
            make_const_str("absent", "molt_pbkdf2_hmac"),
            make_call_func("r2", "wrapper", &["absent"]),
        ]);
        let manifest = compute_intrinsic_manifest(&[func], &symbols);
        assert!(manifest.contains("molt_gc_collect"));
        assert!(
            !manifest.contains("molt_pbkdf2_hmac"),
            "a well-formed molt_ identifier absent from the staticlib must be excluded"
        );
    }

    #[test]
    fn manifest_captures_async_sleep_public_symbol_directly() {
        let symbols: BTreeSet<String> = ["molt_async_sleep".to_string()].into_iter().collect();
        let func = manifest_func(vec![
            make_const_str("nm", "molt_async_sleep"),
            make_call_func("res", "require_intrinsic", &["nm"]),
        ]);
        let manifest = compute_intrinsic_manifest(&[func], &symbols);
        assert!(manifest.contains("molt_async_sleep"));
        assert!(!manifest.contains("molt_async_sleep_new"));
    }

    /// A non-override intrinsic name is captured verbatim (the common case must be
    /// untouched by the override remapping).
    #[test]
    fn manifest_keeps_non_override_name_verbatim() {
        let symbols: BTreeSet<String> = ["molt_gc_collect".to_string()].into_iter().collect();
        let func = manifest_func(vec![
            make_const_str("nm", "molt_gc_collect"),
            make_call_func("res", "require_intrinsic", &["nm"]),
        ]);
        let manifest = compute_intrinsic_manifest(&[func], &symbols);
        assert!(manifest.contains("molt_gc_collect"));
    }

    #[test]
    fn direct_raise_edge_canonicalization_removes_duplicate_handler_edges() {
        let mut func = FunctionIR {
            name: "direct_raise".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "exception_new_builtin".to_string(),
                    out: Some("exc".to_string()),
                    value: Some(5),
                    ..Default::default()
                },
                make_store_var("_bb7_arg0", "acc"),
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(100),
                    ..Default::default()
                },
                OpIR {
                    kind: "raise".to_string(),
                    args: Some(vec!["exc".to_string()]),
                    ..Default::default()
                },
                make_store_var("_bb7_arg0", "acc"),
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(100),
                    ..Default::default()
                },
                make_store_var("_bb7_arg0", "acc"),
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(100),
                    ..Default::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(100),
                    ..Default::default()
                },
            ],
        };

        canonicalize_direct_raise_edges(&mut func);

        assert!(
            !func.ops.iter().any(|op| op.kind == "check_exception"),
            "direct raise-to-handler edge must not keep redundant polls: {:?}",
            func.ops
        );
        let raise_idx = func
            .ops
            .iter()
            .position(|op| op.kind == "raise")
            .expect("raise must remain");
        assert_eq!(func.ops[raise_idx + 1].kind, "store_var");
        assert_eq!(func.ops[raise_idx + 2].kind, "jump");
        assert_eq!(func.ops[raise_idx + 2].value, Some(100));
    }

    fn make_arith(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn return_alias_summary_ignores_exception_ret_void_tail() {
        let summaries = compute_return_alias_summaries(&[FunctionIR {
            name: "expect_str_like".to_string(),
            params: vec!["value".to_string(), "label".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("value".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(1),
                    ..Default::default()
                },
                make_op("ret_void"),
            ],
        }]);

        assert_eq!(
            summaries.get("expect_str_like"),
            Some(&ReturnAliasSummary::Param(0))
        );
    }

    #[test]
    fn return_alias_summary_rejects_mixed_alias_and_fresh_return() {
        let summaries = compute_return_alias_summaries(&[FunctionIR {
            name: "mixed_return".to_string(),
            params: vec!["value".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("value".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("fresh".to_string()),
                    s_value: Some("fresh".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("fresh".to_string()),
                    ..Default::default()
                },
            ],
        }]);

        assert_eq!(summaries.get("mixed_return"), None);
    }

    #[test]
    fn return_alias_summary_uses_args_based_copy_var_value_source() {
        let summaries = compute_return_alias_summaries(&[FunctionIR {
            name: "copy_var_alias".to_string(),
            params: vec!["value".to_string(), "metadata_slot".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "copy_var".to_string(),
                    var: Some("metadata_slot".to_string()),
                    args: Some(vec!["value".to_string()]),
                    out: Some("alias".to_string()),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("alias".to_string()),
                    args: Some(vec!["alias".to_string()]),
                    ..Default::default()
                },
            ],
        }]);

        assert_eq!(
            summaries.get("copy_var_alias"),
            Some(&ReturnAliasSummary::Param(0)),
            "args[0] is the copied value; var is only local-name transport metadata"
        );
    }

    #[test]
    fn dead_op_elim_keeps_copy_var_when_output_is_consumed() {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "param_copy".to_string(),
                params: vec!["n".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "copy_var".to_string(),
                        var: Some("n".to_string()),
                        out: Some("_v8".to_string()),
                        ..Default::default()
                    },
                    make_const_int("_v11", 1),
                    make_arith("add", &["_v8", "_v11"], "_v12"),
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("_v12".to_string()),
                        args: Some(vec!["_v12".to_string()]),
                        ..Default::default()
                    },
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter()
                .any(|op| op.kind == "copy_var" && op.out.as_deref() == Some("_v8")),
            "dead-op elimination must preserve copy_var definitions consumed through op.out: {ops:?}"
        );
    }

    #[test]
    fn dead_op_elim_counts_copy_var_source_as_consumed_input() {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "copy_source".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    make_const_int("_v0", 40),
                    make_const_int("_v1", 2),
                    make_arith("add", &["_v0", "_v1"], "_sum"),
                    OpIR {
                        kind: "copy_var".to_string(),
                        var: Some("_sum".to_string()),
                        out: Some("_alias".to_string()),
                        ..Default::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("_alias".to_string()),
                        args: Some(vec!["_alias".to_string()]),
                        ..Default::default()
                    },
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter()
                .any(|op| op.kind == "add" && op.out.as_deref() == Some("_sum")),
            "dead-op elimination must preserve producers consumed through copy_var.var: {ops:?}"
        );
    }

    #[test]
    fn dead_op_elim_ignores_args_based_copy_var_metadata_var() {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "copy_source_metadata".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    make_const_int("_source", 40),
                    make_const_int("_metadata", 2),
                    OpIR {
                        kind: "copy_var".to_string(),
                        var: Some("_metadata".to_string()),
                        args: Some(vec!["_source".to_string()]),
                        out: Some("_alias".to_string()),
                        ..Default::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("_alias".to_string()),
                        args: Some(vec!["_alias".to_string()]),
                        ..Default::default()
                    },
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter()
                .any(|op| op.kind == "const" && op.out.as_deref() == Some("_source")),
            "dead-op elimination must preserve the args[0] value source: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| op.kind == "const" && op.out.as_deref() == Some("_metadata")),
            "copy_var.var is metadata when args[0] is present and must not keep dead producers alive: {ops:?}"
        );
    }

    #[test]
    fn dead_op_elim_keeps_unused_potentially_throwing_index() {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "unused_index".to_string(),
                params: vec!["mapping".to_string(), "key".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "index".to_string(),
                        args: Some(vec!["mapping".to_string(), "key".to_string()]),
                        out: Some("_unused".to_string()),
                        ..Default::default()
                    },
                    make_op("ret_void"),
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter().any(|op| op.kind == "index"),
            "dead-op elimination must preserve unused index ops because __getitem__/__missing__ exceptions are observable: {ops:?}"
        );
    }

    #[test]
    fn dead_op_elim_removes_effect_proven_static_module_class_lookup_chain() {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "dead_static_class_guard".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("__main__".to_string()),
                        out: Some("module_name".to_string()),
                        ..Default::default()
                    },
                    OpIR {
                        kind: "module_cache_get".to_string(),
                        args: Some(vec!["module_name".to_string()]),
                        out: Some("module".to_string()),
                        effect_proof: Some(
                            EffectProof::StaticModuleClassBinding.name().to_string(),
                        ),
                        ..Default::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("Point".to_string()),
                        out: Some("attr_name".to_string()),
                        ..Default::default()
                    },
                    OpIR {
                        kind: "module_get_attr".to_string(),
                        args: Some(vec!["module".to_string(), "attr_name".to_string()]),
                        out: Some("class_ref".to_string()),
                        effect_proof: Some(
                            EffectProof::StaticModuleClassBinding.name().to_string(),
                        ),
                        ..Default::default()
                    },
                    make_op("ret_void"),
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter()
                .all(|op| !matches!(op.kind.as_str(), "module_cache_get" | "module_get_attr")),
            "effect-proven dead static class guard should be removed: {ops:?}"
        );
    }

    #[test]
    fn dead_op_elim_keeps_unused_untyped_arithmetic() {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "unused_untyped_add".to_string(),
                params: vec!["left".to_string(), "right".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    make_arith("add", &["left", "right"], "_unused"),
                    make_op("ret_void"),
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter().any(|op| op.kind == "add"),
            "dead-op elimination must preserve unused untyped arithmetic because protocol dispatch can raise: {ops:?}"
        );
    }

    #[test]
    fn dead_op_elim_keeps_transport_hinted_unknown_arithmetic() {
        let mut add = make_arith("add", &["left", "right"], "_unused");
        add.fast_int = Some(true);
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "unused_transport_hint_add".to_string(),
                params: vec!["left".to_string(), "right".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![add, make_op("ret_void")],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter().any(|op| op.kind == "add"),
            "transport hints must not prove unused arithmetic is nonthrowing without typed facts: {ops:?}"
        );
    }

    #[test]
    fn dead_op_elim_removes_unused_typed_param_arithmetic_without_transport_hints() {
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "unused_typed_param_add".to_string(),
                params: vec!["left".to_string(), "right".to_string()],
                param_types: Some(vec!["int".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    make_arith("add", &["left", "right"], "_unused"),
                    make_op("ret_void"),
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter().all(|op| op.kind != "add"),
            "typed scalar facts, not transport hints, should prove unused int arithmetic removable: {ops:?}"
        );
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, "ret_void");
    }

    #[test]
    fn dead_op_elim_removes_unused_typed_const_arithmetic_chain() {
        let add = make_arith("add", &["_v0", "_v1"], "_unused");
        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "unused_typed_const_add".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    make_const_int("_v0", 40),
                    make_const_int("_v1", 2),
                    add,
                    make_op("ret_void"),
                ],
            }],
            profile: None,
        };

        eliminate_dead_ops(&mut ir);

        let ops = &ir.functions[0].ops;
        assert!(
            ops.iter().all(|op| op.kind != "add" && op.out.is_none()),
            "dead-op elimination should still remove provably nonthrowing unused typed value chains: {ops:?}"
        );
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, "ret_void");
    }

    // --- RC coalescing tests ---

    fn make_ref_op(kind: &str, arg: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(vec![arg.to_string()]),
            ..Default::default()
        }
    }

    #[test]
    fn rc_coalescing_eliminates_adjacent_inc_dec_pair() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["x".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_ref_op("inc_ref", "x"),
                make_ref_op("dec_ref", "x"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // Both inc_ref and dec_ref should be eliminated.
        assert_eq!(func.ops.len(), 1);
        assert_eq!(func.ops[0].kind, "ret_void");
    }

    #[test]
    fn rc_coalescing_preserves_pair_across_control_flow() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["x".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_ref_op("inc_ref", "x"),
                make_op("if"),
                make_ref_op("dec_ref", "x"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // The pair should NOT be eliminated because `if` is control flow.
        assert_eq!(func.ops.len(), 4);
    }

    #[test]
    fn rc_coalescing_handles_borrow_release_pair() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["y".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_ref_op("borrow", "y"),
                make_ref_op("release", "y"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        assert_eq!(func.ops.len(), 1);
        assert_eq!(func.ops[0].kind, "ret_void");
    }

    #[test]
    fn rc_coalescing_preserves_pair_with_intervening_use() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["x".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_ref_op("inc_ref", "x"),
                // An op that uses x as an argument — breaks the window.
                make_arith("add", &["x", "x"], "y"),
                make_ref_op("dec_ref", "x"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // The pair should NOT be eliminated because of the intervening use.
        assert_eq!(func.ops.len(), 4);
    }

    #[test]
    fn rc_coalescing_eliminates_different_vars_independently() {
        let mut func = FunctionIR {
            name: "test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_ref_op("inc_ref", "a"),
                make_ref_op("inc_ref", "b"),
                make_ref_op("dec_ref", "a"),
                make_ref_op("dec_ref", "b"),
                make_op("ret_void"),
            ],
        };

        rc_coalescing(&mut func);

        // inc_ref(a)/dec_ref(a) cannot be eliminated because inc_ref(b) intervenes
        // (it doesn't use 'a' though). Let's check what actually happens.
        // The scan finds inc_ref(a) at 0, then looks at 1 (inc_ref(b) — not a
        // dec_ref of a, and doesn't use a), then at 2 (dec_ref(a) — match!).
        // So indices 0,2 are eliminated. Then inc_ref(b) at 1, looks at 3
        // (dec_ref(b) — match!), indices 1,3 eliminated.
        assert_eq!(func.ops.len(), 1);
        assert_eq!(func.ops[0].kind, "ret_void");
    }

    #[test]
    fn protected_runtime_entrypoint_detection_is_explicit() {
        assert!(is_protected_runtime_entrypoint("molt_main"));
        assert!(is_protected_runtime_entrypoint("molt_host_init"));
        assert!(is_protected_runtime_entrypoint("_start"));
        assert!(is_protected_runtime_entrypoint("molt_isolate_import"));
        assert!(is_protected_runtime_entrypoint("molt_isolate_bootstrap"));
        assert!(!is_protected_runtime_entrypoint("molt_init_math"));
        assert!(!is_protected_runtime_entrypoint("user_entry"));
    }

    #[test]
    fn eliminate_dead_functions_retains_runtime_dispatch_closure() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "entry".to_string(),
                    params: vec![],
                    ops: vec![make_op("ret_void")],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_isolate_import".to_string(),
                    params: vec!["p0".to_string()],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("molt_init_math".to_string()),
                            out: Some("v0".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            args: Some(vec!["v0".to_string()]),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_math".to_string(),
                    params: vec![],
                    ops: vec![make_op("ret_void")],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        eliminate_dead_functions(&mut ir);

        let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
        assert!(retained.contains("molt_isolate_import"));
        assert!(retained.contains("molt_init_math"));
        let dispatch = ir
            .functions
            .iter()
            .find(|func| func.name == "molt_isolate_import")
            .expect("runtime dispatch entrypoint must remain");
        assert!(
            dispatch
                .ops
                .iter()
                .any(|op| op.s_value.as_deref() == Some("molt_init_math")),
            "runtime dispatch body must keep its transitive module-init references",
        );
    }

    #[test]
    fn eliminate_dead_functions_retains_molt_host_init_and_transitive_refs() {
        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "entry".to_string(),
                    params: vec![],
                    ops: vec![make_op("ret_void")],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_host_init".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("host_init_helper".to_string()),
                            out: Some("v0".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            args: Some(vec!["v0".to_string()]),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "host_init_helper".to_string(),
                    params: vec![],
                    ops: vec![make_op("ret_void")],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        eliminate_dead_functions(&mut ir);

        let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
        assert!(retained.contains("molt_host_init"));
        assert!(retained.contains("host_init_helper"));
        let host_init = ir
            .functions
            .iter()
            .find(|func| func.name == "molt_host_init")
            .expect("molt_host_init must remain");
        assert!(
            host_init
                .ops
                .iter()
                .any(|op| op.s_value.as_deref() == Some("host_init_helper")),
            "molt_host_init must keep its transitive references",
        );
    }

    #[test]
    fn eliminate_dead_functions_does_not_root_stdlib_from_partition_env() {
        let prior = std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok();
        unsafe {
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
        }

        let mut ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "entry".to_string(),
                    params: vec![],
                    ops: vec![make_op("ret_void")],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "call".to_string(),
                        s_value: Some("sys__helper".to_string()),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "sys__helper".to_string(),
                    params: vec![],
                    ops: vec![make_op("ret_void")],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_json".to_string(),
                    params: vec![],
                    ops: vec![make_op("ret_void")],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        eliminate_dead_functions(&mut ir);

        match prior {
            Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", value) },
            None => unsafe { std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS") },
        }

        let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
        assert!(!retained.contains("molt_init_sys"));
        assert!(!retained.contains("sys__helper"));
        assert!(!retained.contains("molt_init_json"));
    }

    #[test]
    fn split_large_function_preserves_protected_runtime_import_entrypoint() {
        let func = FunctionIR {
            name: "molt_isolate_import".to_string(),
            params: vec!["p0".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_const_int("v0", 1),
                make_const_int("v1", 2),
                make_arith("add", &["p0", "v0"], "v2"),
                make_arith("add", &["v2", "v1"], "v3"),
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["v3".to_string()]),
                    ..OpIR::default()
                },
            ],
        };

        let result = split_large_function(func, 2);

        let original = result.expect_err("protected import entrypoint must not split");
        assert_eq!(original.name, "molt_isolate_import");
        assert_eq!(original.params, vec!["p0".to_string()]);
        assert_eq!(original.ops.len(), 5);
    }

    #[test]
    fn split_large_function_preserves_protected_runtime_bootstrap_entrypoint() {
        let func = FunctionIR {
            name: "molt_isolate_bootstrap".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_op("const_none"),
                make_op("const_none"),
                make_op("const_none"),
                make_op("const_none"),
                make_op("ret_void"),
            ],
        };

        let result = split_large_function(func, 2);

        let original = result.expect_err("protected bootstrap entrypoint must not split");
        assert_eq!(original.name, "molt_isolate_bootstrap");
        assert!(original.params.is_empty());
        assert_eq!(original.ops.len(), 5);
    }

    #[test]
    fn split_large_function_still_splits_regular_large_functions() {
        let func = FunctionIR {
            name: "user_large".to_string(),
            params: vec!["p0".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "line".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                make_const_int("v0", 1),
                OpIR {
                    kind: "line".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                make_const_int("v1", 2),
                OpIR {
                    kind: "line".to_string(),
                    value: Some(3),
                    ..OpIR::default()
                },
                make_arith("add", &["p0", "v0"], "v2"),
                OpIR {
                    kind: "line".to_string(),
                    value: Some(4),
                    ..OpIR::default()
                },
                make_arith("add", &["v2", "v1"], "v3"),
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["v3".to_string()]),
                    ..OpIR::default()
                },
            ],
        };

        let (stub, chunks) = split_large_function(func, 2).expect("expected split");

        assert_eq!(stub.name, "user_large");
        assert!(!chunks.is_empty());
        let stub_chunk_calls: Vec<&OpIR> = stub
            .ops
            .iter()
            .filter(|op| op.kind == "call_internal")
            .collect();
        assert_eq!(stub_chunk_calls.len(), chunks.len());
        for (call, chunk) in stub_chunk_calls.iter().zip(chunks.iter()) {
            assert_eq!(
                call.s_value.as_deref(),
                Some(chunk.name.as_str()),
                "stub call must target the matching private chunk",
            );
            assert_eq!(
                call.args.as_ref(),
                Some(&chunk.params),
                "stub call must forward the live-in chunk ABI, not just original params",
            );
        }
        assert!(
            chunks.iter().skip(1).any(|chunk| chunk
                .ops
                .iter()
                .any(|op| op.kind == "index" && op.out.as_deref() == Some("v0"))),
            "later chunks must load values defined by earlier chunks from the split frame"
        );
        assert!(
            chunks
                .iter()
                .flat_map(|chunk| chunk.ops.iter())
                .all(|op| op.kind != "load_index"),
            "split frame reads must use the backend-canonical index op"
        );
        assert!(
            stub.ops.iter().any(|op| {
                op.kind == "list_new"
                    && op
                        .out
                        .as_deref()
                        .is_some_and(|out| out.starts_with("__molt_split_frame"))
            }),
            "stub must allocate the split frame used for cross-chunk live values"
        );
        assert!(
            stub.ops
                .iter()
                .any(|op| op.kind == "ret" && op.var.as_deref() == Some("__chunk_ret")),
            "split stub must return the named propagated chunk result",
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.name.starts_with("__molt_chunk_user_large_"))
        );
        assert!(
            chunks.iter().any(|chunk| chunk.ops.iter().any(|op| {
                op.kind == "store_index"
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == "v0"))
            })),
            "split chunks must store cross-chunk live values into the split frame"
        );
    }

    #[test]
    fn split_large_function_preserves_drop_authority_on_chunks_only() {
        let func = FunctionIR {
            name: "drop_inserted_large".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                make_op(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
                OpIR {
                    kind: "line".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("a".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dec_ref".to_string(),
                    args: Some(vec!["a".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("b".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dec_ref".to_string(),
                    args: Some(vec!["b".to_string()]),
                    ..OpIR::default()
                },
                make_op("ret_void"),
            ],
        };

        let (stub, chunks) = split_large_function(func, 2).expect("expected split");

        assert!(
            !stub.ops.iter().any(is_drop_fact_marker_op),
            "synthetic split stub creates its own frame values and must not inherit full-RC authority"
        );
        let chunks_with_dec_ref = chunks
            .iter()
            .filter(|chunk| chunk.ops.iter().any(|op| op.kind == "dec_ref"))
            .count();
        assert!(
            chunks_with_dec_ref > 0,
            "test must exercise extracted chunks containing TIR-inserted drops"
        );
        for chunk in &chunks {
            assert_eq!(
                chunk.ops.first().map(|op| op.kind.as_str()),
                Some(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
                "chunk {} must start with the full-RC authority marker",
                chunk.name
            );
            assert_eq!(
                chunk
                    .ops
                    .iter()
                    .filter(|op| is_drop_fact_marker_op(op))
                    .count(),
                1,
                "chunk {} must not duplicate transport markers",
                chunk.name
            );
        }
    }

    #[test]
    fn split_large_function_threads_cross_chunk_builtin_type_tag() {
        let func = FunctionIR {
            name: "threading__molt_module_chunk_3".to_string(),
            params: vec!["__molt_module_obj__".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "line".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                make_const_int("object_type_tag", 100),
                OpIR {
                    kind: "line".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "builtin_type".to_string(),
                    args: Some(vec!["object_type_tag".to_string()]),
                    out: Some("object_type".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        };

        let (stub, chunks) = split_large_function(func, 2).expect("expected split");

        assert!(
            chunks
                .iter()
                .skip(1)
                .any(|chunk| chunk.ops.iter().any(|op| {
                    op.kind == "index" && op.out.as_deref() == Some("object_type_tag")
                })),
            "the builtin_type chunk must load the tag value from the split frame"
        );
        assert!(
            chunks
                .iter()
                .flat_map(|chunk| chunk.ops.iter())
                .all(|op| op.kind != "load_index"),
            "split frame reads must not introduce non-canonical IR ops"
        );
        assert!(
            chunks.iter().any(|chunk| {
                chunk.ops.iter().any(|op| {
                    op.kind == "store_index"
                        && op
                            .args
                            .as_ref()
                            .is_some_and(|args| args.iter().any(|arg| arg == "object_type_tag"))
                })
            }),
            "the defining chunk must store the tag into the split frame"
        );
        assert!(
            stub.ops.iter().any(|op| {
                op.kind == "list_new"
                    && op
                        .out
                        .as_deref()
                        .is_some_and(|out| out.starts_with("__molt_split_frame"))
            }),
            "the stub must allocate frame storage for the transported tag"
        );
        for chunk in &chunks {
            verify_split_function_def_use(chunk).expect("generated chunk def-use must verify");
        }
        verify_split_function_def_use(&stub).expect("generated stub def-use must verify");
    }

    #[test]
    fn split_generated_op_verifier_rejects_noncanonical_frame_load() {
        let func = FunctionIR {
            name: "__molt_chunk_bad_0".to_string(),
            params: vec!["__molt_split_frame".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(0),
                    out: Some("__molt_split_frame_load_index".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_index".to_string(),
                    args: Some(vec![
                        "__molt_split_frame".to_string(),
                        "__molt_split_frame_load_index".to_string(),
                    ]),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                make_op("ret_void"),
            ],
        };

        let err = verify_split_generated_ops(&func).expect_err("load_index must reject");
        assert!(err.contains("non-canonical generated op `load_index`"));
    }

    #[test]
    fn split_large_function_clones_shared_suffix_exception_handler() {
        let mut ops = Vec::new();
        for i in 0..40 {
            ops.push(OpIR {
                kind: "line".to_string(),
                value: Some(i),
                ..OpIR::default()
            });
            ops.push(make_const_int(&format!("v{i}"), i));
            ops.push(OpIR {
                kind: "check_exception".to_string(),
                value: Some(32),
                ..OpIR::default()
            });
        }
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(99),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "module_get_attr".to_string(),
            args: Some(vec!["__molt_module_obj__".to_string(), "v0".to_string()]),
            out: Some("loaded_v0".to_string()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "jump".to_string(),
            value: Some(32),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "label".to_string(),
            value: Some(32),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "exception_last".to_string(),
            out: Some("exc".to_string()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const_none".to_string(),
            out: Some("none_exc".to_string()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "is".to_string(),
            args: Some(vec!["exc".to_string(), "none_exc".to_string()]),
            out: Some("exc_is_none".to_string()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "not".to_string(),
            args: Some(vec!["exc_is_none".to_string()]),
            out: Some("exc_pending".to_string()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "jump".to_string(),
            value: Some(430),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "label".to_string(),
            value: Some(430),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "br_if".to_string(),
            args: Some(vec!["exc_pending".to_string()]),
            value: Some(523),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "jump".to_string(),
            value: Some(352),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "label".to_string(),
            value: Some(523),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const_str".to_string(),
            s_value: Some("builtins".to_string()),
            out: Some("module_name".to_string()),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "module_cache_del".to_string(),
            args: Some(vec!["module_name".to_string()]),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "jump".to_string(),
            value: Some(352),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "label".to_string(),
            value: Some(352),
            ..OpIR::default()
        });
        ops.push(make_op("ret_void"));

        let func = FunctionIR {
            name: "builtins__molt_module_chunk_2".to_string(),
            params: vec!["__molt_module_obj__".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops,
        };

        let (stub, chunks) = split_large_function(func, 40)
            .expect("shared suffix exception handler should not block splitting");

        assert_eq!(stub.name, "builtins__molt_module_chunk_2");
        assert!(chunks.len() >= 2);
        let control_outs: std::collections::BTreeSet<String> = stub
            .ops
            .iter()
            .filter(|op| op.kind == "call_internal")
            .filter_map(|op| op.out.clone())
            .filter(|out| out.starts_with("__chunk_continue_"))
            .collect();
        assert_eq!(
            control_outs.len(),
            chunks.len(),
            "void split chunks must return an explicit continuation status"
        );
        for out in &control_outs {
            assert!(
                stub.ops.iter().any(|op| {
                    op.kind == "br_if"
                        && op
                            .args
                            .as_ref()
                            .is_some_and(|args| args.iter().any(|arg| arg == out))
                }),
                "stub must branch on chunk continuation status `{out}`"
            );
        }
        let mut observed_live_out_store_before_cloned_suffix = false;
        let mut observed_cloned_suffix_stop_return = false;
        for chunk in &chunks {
            assert!(
                chunk.ops.len() <= 80,
                "cloned shared suffix must not recreate an oversized chunk: {} ops",
                chunk.ops.len()
            );
            let labels: std::collections::BTreeSet<i64> = chunk
                .ops
                .iter()
                .filter(|op| matches!(op.kind.as_str(), "label" | "state_label"))
                .filter_map(|op| op.value)
                .collect();
            let cloned_skip_labels: Vec<i64> =
                labels.iter().copied().filter(|id| *id > 523).collect();
            let cloned_handler = chunk
                .ops
                .iter()
                .position(|op| op.kind == "label" && op.value == Some(32));
            if !cloned_skip_labels.is_empty() {
                let handler_idx =
                    cloned_handler.expect("cloned chunk must include handler label 32");
                assert!(handler_idx > 0);
                let guard = &chunk.ops[handler_idx - 1];
                assert_eq!(
                    guard.kind, "jump",
                    "normal chunk fallthrough must skip the cloned exception tail"
                );
                assert_ne!(guard.value, Some(32));
                observed_cloned_suffix_stop_return |=
                    chunk.ops[handler_idx..].windows(2).any(|window| {
                        window[0].kind == "const_bool"
                            && window[0].value == Some(0)
                            && window[0]
                                .out
                                .as_ref()
                                .is_some_and(|out| window[1].var.as_ref() == Some(out))
                            && window[1].kind == "ret"
                    });
                for (idx, op) in chunk.ops.iter().enumerate() {
                    if op.kind == "store_index"
                        && op
                            .args
                            .as_ref()
                            .is_some_and(|args| args.iter().any(|arg| arg == "v0"))
                    {
                        assert!(
                            idx < handler_idx - 1,
                            "split-frame live-out stores must execute before skipping cloned tails"
                        );
                        observed_live_out_store_before_cloned_suffix = true;
                    }
                }
            }
            for op in &chunk.ops {
                if matches!(op.kind.as_str(), "check_exception" | "jump" | "br_if")
                    && let Some(target) = op.value
                {
                    assert!(
                        labels.contains(&target),
                        "chunk `{}` retains external control-flow target {}",
                        chunk.name,
                        target
                    );
                }
            }
        }
        assert!(
            observed_live_out_store_before_cloned_suffix,
            "test must cover a live-out split-frame store in a suffix-cloned chunk"
        );
        assert!(
            observed_cloned_suffix_stop_return,
            "cloned terminal suffixes must tell the stub not to run later chunks"
        );
    }

    #[test]
    fn split_large_function_delays_suffix_clone_until_cleanup_reads_are_available() {
        let mut ops = vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            make_const_int("early", 1),
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(90),
                ..OpIR::default()
            },
        ];
        for i in 0..5 {
            ops.push(make_const_int(&format!("filler_{i}"), i));
        }
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(2),
            ..OpIR::default()
        });
        ops.push(make_const_int("cleanup_owned", 99));
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(3),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "label".to_string(),
            value: Some(90),
            ..OpIR::default()
        });
        ops.push(make_ref_op("dec_ref", "cleanup_owned"));
        ops.push(make_op("ret_void"));

        let func = FunctionIR {
            name: "cleanup_suffix".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops,
        };

        let (stub, chunks) = split_large_function(func, 8)
            .expect("later safe split point should carry cleanup suffix inputs");

        assert_eq!(stub.name, "cleanup_suffix");
        assert_eq!(
            chunks.len(),
            2,
            "the first eligible line boundary is unsafe, so splitting must wait for the cleanup input definition"
        );
        for chunk in &chunks {
            verify_split_function_def_use(chunk).expect("chunk def-use must verify");
        }
        verify_split_function_def_use(&stub).expect("stub def-use must verify");

        let first = &chunks[0];
        let cleanup_def = first
            .ops
            .iter()
            .position(|op| op.out.as_deref() == Some("cleanup_owned"))
            .expect("safe chunk must include the cleanup-owned definition");
        let cleanup_drop = first
            .ops
            .iter()
            .position(|op| {
                op.kind == "dec_ref"
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == "cleanup_owned"))
            })
            .expect("safe chunk must clone the cleanup drop");
        assert!(
            cleanup_def < cleanup_drop,
            "cloned cleanup suffix must not read a value before the extracted chunk defines it"
        );
    }

    #[test]
    fn split_large_function_void_only_stub_returns_none() {
        let func = FunctionIR {
            name: "void_only".to_string(),
            params: vec!["p0".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "line".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".to_string(),
                    value: Some(3),
                    ..OpIR::default()
                },
                make_op("ret_void"),
            ],
        };

        let (stub, chunks) = split_large_function(func, 2).expect("expected split");

        assert!(!chunks.is_empty());
        assert_eq!(
            stub.ops.last().map(|op| op.kind.as_str()),
            Some("ret_void"),
            "void-only split stubs must terminate explicitly with ret_void",
        );
    }

    #[test]
    fn split_megafunctions_splits_module_chunks_at_native_default_threshold() {
        let previous = std::env::var("MOLT_MAX_FUNCTION_OPS").ok();
        unsafe {
            std::env::remove_var("MOLT_MAX_FUNCTION_OPS");
        }

        let mut ops = Vec::new();
        for i in 0..1401 {
            ops.push(OpIR {
                kind: "line".to_string(),
                value: Some(i),
                ..OpIR::default()
            });
            ops.push(make_const_int(&format!("v{i}"), i));
        }
        ops.push(make_op("ret_void"));

        let mut ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "builtins__molt_module_chunk_2".to_string(),
                params: vec!["__molt_module_obj__".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops,
            }],
            profile: None,
        };

        split_megafunctions(&mut ir);

        let names: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
        assert!(
            names.contains("builtins__molt_module_chunk_2"),
            "stub must keep the original module chunk symbol"
        );
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("__molt_chunk_builtins__molt_module_chunk_2_")),
            "module chunk should be split into backend private chunks at the native default threshold"
        );

        match previous {
            Some(value) => unsafe { std::env::set_var("MOLT_MAX_FUNCTION_OPS", value) },
            None => unsafe { std::env::remove_var("MOLT_MAX_FUNCTION_OPS") },
        }
    }

    fn op_with(kind: &str, out: Option<&str>, s_value: Option<&str>, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: out.map(str::to_string),
            s_value: s_value.map(str::to_string),
            args: if args.is_empty() {
                None
            } else {
                Some(args.iter().map(|a| a.to_string()).collect())
            },
            ..Default::default()
        }
    }

    #[test]
    fn fuse_method_dispatch_rewrites_getattr_call_idiom() {
        // get_attr_generic_ptr(recv, "compute") -> callargs -> call_bind
        // must collapse to a single call_method_ic(recv, x) op.
        let mut func = FunctionIR {
            name: "f".to_string(),
            params: vec!["recv".to_string(), "x".to_string()],
            ops: vec![
                op_with(
                    "get_attr_generic_ptr",
                    Some("t"),
                    Some("compute"),
                    &["recv"],
                ),
                op_with("check_exception", None, None, &[]),
                op_with("callargs_new", Some("ca"), None, &[]),
                op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
                op_with("call_bind", Some("r"), None, &["t", "ca"]),
                op_with("ret", None, None, &["r"]),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        fuse_method_dispatch(&mut func);
        // The getattr / callargs_new / callargs_push_pos / call_bind quartet
        // collapses to a single call_method_ic; check_exception + ret survive.
        let fused: Vec<&str> = func.ops.iter().map(|o| o.kind.as_str()).collect();
        assert_eq!(fused, vec!["check_exception", "call_method_ic", "ret"]);
        let ic = func
            .ops
            .iter()
            .find(|o| o.kind == "call_method_ic")
            .unwrap();
        assert_eq!(ic.out.as_deref(), Some("r"));
        assert_eq!(ic.s_value.as_deref(), Some("compute"));
        assert_eq!(
            ic.args.as_ref().unwrap(),
            &vec!["recv".to_string(), "x".to_string()]
        );
    }

    #[test]
    fn fuse_method_dispatch_skips_multi_use_getattr() {
        // If the getattr result is used by something other than the call_bind
        // callee (here a second store_var), fusion must NOT fire (the bound
        // method escapes and its identity may be observed).
        let mut func = FunctionIR {
            name: "f".to_string(),
            params: vec!["recv".to_string(), "x".to_string()],
            ops: vec![
                op_with(
                    "get_attr_generic_ptr",
                    Some("t"),
                    Some("compute"),
                    &["recv"],
                ),
                op_with("store_var", Some("_s"), None, &["t"]),
                op_with("callargs_new", Some("ca"), None, &[]),
                op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
                op_with("call_bind", Some("r"), None, &["t", "ca"]),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let before: Vec<String> = func.ops.iter().map(|o| o.kind.clone()).collect();
        fuse_method_dispatch(&mut func);
        let after: Vec<String> = func.ops.iter().map(|o| o.kind.clone()).collect();
        assert_eq!(before, after, "multi-use getattr must not be fused");
    }

    #[test]
    fn fuse_method_dispatch_rewrites_super_idiom() {
        // super_new(class, self) -> get_attr_generic_obj -> callargs ->
        // call_indirect must collapse to call_super_method_ic(class, self, x).
        let mut func = FunctionIR {
            name: "m".to_string(),
            params: vec!["self".to_string(), "x".to_string()],
            ops: vec![
                op_with("super_new", Some("sup"), None, &["cls", "self"]),
                op_with("get_attr_generic_obj", Some("t"), Some("compute"), &["sup"]),
                op_with("callargs_new", Some("ca"), None, &[]),
                op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
                op_with("call_indirect", Some("r"), None, &["t", "ca"]),
                op_with("ret", None, None, &["r"]),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        fuse_method_dispatch(&mut func);
        let kinds: Vec<&str> = func.ops.iter().map(|o| o.kind.as_str()).collect();
        assert_eq!(kinds, vec!["call_super_method_ic", "ret"]);
        let ic = func
            .ops
            .iter()
            .find(|o| o.kind == "call_super_method_ic")
            .unwrap();
        assert_eq!(ic.s_value.as_deref(), Some("compute"));
        assert_eq!(
            ic.args.as_ref().unwrap(),
            &vec!["cls".to_string(), "self".to_string(), "x".to_string()]
        );
    }

    #[test]
    fn fuse_method_dispatch_disabled_by_env() {
        // The lever is exercised through the explicit parameter — mutating
        // the process-global env here races every concurrently-running test
        // that calls the env-reading wrapper.
        let mut func = FunctionIR {
            name: "f".to_string(),
            params: vec!["recv".to_string(), "x".to_string()],
            ops: vec![
                op_with(
                    "get_attr_generic_ptr",
                    Some("t"),
                    Some("compute"),
                    &["recv"],
                ),
                op_with("callargs_new", Some("ca"), None, &[]),
                op_with("callargs_push_pos", Some("_p"), None, &["ca", "x"]),
                op_with("call_bind", Some("r"), None, &["t", "ca"]),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        fuse_method_dispatch_inner(&mut func, true);
        assert!(
            func.ops.iter().all(|o| o.kind != "call_method_ic"),
            "fusion must be a no-op when disabled by env"
        );
    }
}
