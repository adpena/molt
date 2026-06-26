use crate::{FunctionIR, OpIR};
use std::collections::BTreeMap;

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
            let source_site = func_ir.ops[i].source_site();
            let mut rewritten = OpIR {
                kind: "string_split_field_len_from_bounds".to_string(),
                args: Some(vec![
                    plan.hay.clone(),
                    start_var.clone(),
                    end_var.clone(),
                    asc_var.clone(),
                ]),
                out: func_ir.ops[i].out.clone(),
                ..OpIR::default()
            };
            source_site.apply_to_op(&mut rewritten);
            func_ir.ops[i] = rewritten;
        }
        // Rewrite ord_at(field, i) -> string_split_field_ord_at_bounds.
        for &i in &plan.ord_ops {
            let char_idx = func_ir.ops[i]
                .args
                .as_ref()
                .and_then(|a| a.get(1).cloned())
                .unwrap_or_default();
            let source_site = func_ir.ops[i].source_site();
            let mut rewritten = OpIR {
                kind: "string_split_field_ord_at_bounds".to_string(),
                args: Some(vec![
                    plan.hay.clone(),
                    start_var.clone(),
                    end_var.clone(),
                    asc_var.clone(),
                    char_idx,
                ]),
                out: func_ir.ops[i].out.clone(),
                ..OpIR::default()
            };
            source_site.apply_to_op(&mut rewritten);
            func_ir.ops[i] = rewritten;
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
            let source_site = func_ir.ops[i].source_site();
            let mut rewritten = OpIR {
                kind: "string_split_field_eq".to_string(),
                args: Some(vec![
                    plan.hay.clone(),
                    plan.sep.clone(),
                    plan.idx.clone(),
                    const_operand,
                ]),
                out: func_ir.ops[i].out.clone(),
                ..OpIR::default()
            };
            source_site.apply_to_op(&mut rewritten);
            func_ir.ops[i] = rewritten;
        }

        // Replace the field op with start/end/is_ascii + check_exception.
        let field_source_site = func_ir.ops[plan.field_op_index].source_site();
        let three_args = vec![plan.hay.clone(), plan.sep.clone(), plan.idx.clone()];
        let mk = |kind: &str, out: &str| {
            let mut op = OpIR {
                kind: kind.to_string(),
                args: Some(three_args.clone()),
                out: Some(out.to_string()),
                ..OpIR::default()
            };
            field_source_site.apply_to_op(&mut op);
            op
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
