use crate::OpIR;
use std::collections::{BTreeMap, BTreeSet};

/// Strip dead code after unconditional returns at the same nesting depth.
///
/// Tracks nesting depth via structured control flow ops (if/else/end_if,
/// loop_start/loop_end, for_range/for_iter/end_for). Code after a return
/// at depth 0 (function body level) is removed. Code after a return inside
/// a structured block is kept because the block's `end` re-establishes
/// reachability for the parent scope.
/// Detects "store retval + jump to exit" patterns and converts them into
/// direct return ops, eliminating the need for goto in early-return patterns.
///
/// Pattern detected (inside if blocks):
///   store_index(retval_slot, return_value, value)
///   jump(exit_label)
/// Where exit_label leads to:
///   label(exit_label)
///   index(out, retval_slot, slot_index)
///   ret(out)
///
/// Transformed to:
///   ret_direct(value)      — a synthetic op that emits `return value`
///   jump (kept for dead-code elimination to mark rest as unreachable)
/// Rewrite `iter` + `while true` + `iter_next` + `get_item` patterns into
/// `for_iter` + `end_for`, producing idiomatic `for _, v in ipairs(t) do`.
///
/// The Python frontend emits iteration as:
///   iter(iterable) → loop_start → iter_next → get_item(result,1) [exhausted?]
///   → if → break → end_if → get_item(result,0) [value] → body → loop_end
///
/// We detect this pattern and collapse it to for_iter/end_for.
/// Lower `try_start`/`try_end` pairs into `pcall_wrap_begin`/`pcall_wrap_end`.
///
/// Returns rewritten ops plus variables that escape pcall scope.
fn infer_pcall_handler_label(
    ops: &[OpIR],
    try_start_idx: usize,
    fallback: Option<i64>,
) -> Option<i64> {
    let mut nested_try_depth = 0i32;
    let mut explicit_raise_target = None;
    let mut previous_was_raise = false;

    for op in ops.iter().skip(try_start_idx + 1) {
        match op.kind.as_str() {
            "try_start" => {
                nested_try_depth += 1;
                previous_was_raise = false;
                continue;
            }
            "try_end" if nested_try_depth > 0 => {
                nested_try_depth -= 1;
                previous_was_raise = false;
                continue;
            }
            "try_end" => break,
            _ if nested_try_depth > 0 => {
                previous_was_raise = op.kind == "raise";
                continue;
            }
            _ => {}
        }

        if op.kind == "check_exception"
            && let Some(label) = op.value
        {
            return Some(label);
        }
        if previous_was_raise
            && op.kind == "jump"
            && let Some(label) = op.value
        {
            explicit_raise_target.get_or_insert(label);
        }
        previous_was_raise = op.kind == "raise";
    }

    explicit_raise_target.or(fallback)
}

pub(super) fn lower_try_to_pcall(ops: &[OpIR]) -> (Vec<OpIR>, BTreeSet<String>) {
    if !ops.iter().any(|op| op.kind == "try_start") {
        return (ops.to_vec(), BTreeSet::new());
    }

    let mut result: Vec<OpIR> = Vec::with_capacity(ops.len());
    let mut counter: u32 = 0;
    let mut try_stack: Vec<(u32, i32, Option<i64>)> = Vec::new();
    let mut depth: i32 = 0;
    let mut pcall_ranges: Vec<(usize, usize, u32)> = Vec::new();
    let mut active_pcalls: Vec<u32> = Vec::new();
    let mut handler_entry_labels: BTreeMap<i64, Vec<u32>> = BTreeMap::new();
    let mut cleanup_end_labels: BTreeMap<i64, Vec<u32>> = BTreeMap::new();
    let mut previous_protected_raise_target: Option<i64> = None;
    // Track recently popped try IDs so handler-closing try_end ops
    // (which arrive when try_stack is already empty) can correctly
    // emit pcall_handler_end instead of being dropped as unmatched.
    let mut recently_popped_main: Vec<u32> = Vec::new();

    for (idx, op) in ops.iter().enumerate() {
        let kind = op.kind.as_str();

        if kind == "label" {
            if let Some(label) = op.value {
                if let Some(ids) = cleanup_end_labels.remove(&label) {
                    for id in ids {
                        if active_pcalls.last().copied() == Some(id) {
                            active_pcalls.pop();
                        } else if let Some(pos) =
                            active_pcalls.iter().rposition(|&active| active == id)
                        {
                            active_pcalls.remove(pos);
                        }
                    }
                }
                if let Some(ids) = handler_entry_labels.get(&label) {
                    for id in ids {
                        if !active_pcalls.contains(id) {
                            active_pcalls.push(*id);
                        }
                    }
                }
            }
        }

        if kind == "jump"
            && previous_protected_raise_target.is_some_and(|target| op.value == Some(target))
        {
            previous_protected_raise_target = None;
            continue;
        }

        match kind {
            "try_start" => {
                previous_protected_raise_target = None;
                let n = counter;
                counter += 1;
                let handler_label = infer_pcall_handler_label(ops, idx, op.value);
                try_stack.push((n, depth, handler_label));
                depth += 1;
                let start_idx = result.len();
                result.push(OpIR {
                    kind: "pcall_wrap_begin".to_string(),
                    value: Some(n as i64),
                    ..OpIR::default()
                });
                pcall_ranges.push((start_idx, 0, n));
            }
            "try_end" => {
                previous_protected_raise_target = None;
                if let Some(&(n, pre_depth, handler_label)) = try_stack.last() {
                    if depth == pre_depth + 1 {
                        depth -= 1;
                        try_stack.pop();
                        recently_popped_main.push(n);
                        let end_idx = result.len();
                        result.push(OpIR {
                            kind: "pcall_wrap_end".to_string(),
                            value: Some(n as i64),
                            ..OpIR::default()
                        });
                        active_pcalls.push(n);
                        if let Some(label) = handler_label {
                            handler_entry_labels.entry(label).or_default().push(n);
                            result.push(OpIR {
                                kind: "pcall_failure_jump".to_string(),
                                value: Some(label),
                                s_value: Some(n.to_string()),
                                ..OpIR::default()
                            });
                        }
                        if let Some(range) = pcall_ranges.iter_mut().rev().find(|r| r.2 == n) {
                            range.1 = end_idx;
                        }
                    } else {
                        result.push(OpIR {
                            kind: "pcall_handler_end".to_string(),
                            ..OpIR::default()
                        });
                    }
                } else if recently_popped_main.pop().is_some() {
                    // Handler-closing try_end after the body-closing try_end
                    // already popped the stack.  Emit pcall_handler_end so
                    // the try_depth_counter is properly popped during codegen.
                    result.push(OpIR {
                        kind: "pcall_handler_end".to_string(),
                        ..OpIR::default()
                    });
                } else {
                    result.push(OpIR {
                        kind: "nop".to_string(),
                        s_value: Some("try_end (no matching start)".to_string()),
                        ..OpIR::default()
                    });
                }
            }
            "exception_last"
            | "exception_last_pending"
            | "exception_finally_pending_observer"
            | "exception_clear" => {
                previous_protected_raise_target = None;
                let mut rewritten = op.clone();
                if let Some(n) = active_pcalls.last() {
                    rewritten.value = Some(*n as i64);
                }
                result.push(rewritten);
            }
            "exception_pop" => {
                previous_protected_raise_target = None;
                let mut rewritten = op.clone();
                if let Some(n) = active_pcalls.last().copied() {
                    rewritten.value = Some(n as i64);
                    let cleanup_jump = ops
                        .iter()
                        .skip(idx + 1)
                        .find(|next| !matches!(next.kind.as_str(), "line" | "nop"));
                    if let Some(next) = cleanup_jump {
                        if next.kind == "jump" {
                            if let Some(label) = next.value {
                                let labels = cleanup_end_labels.entry(label).or_default();
                                if !labels.contains(&n) {
                                    labels.push(n);
                                }
                            } else {
                                active_pcalls.pop();
                            }
                        } else {
                            active_pcalls.pop();
                        }
                    } else {
                        active_pcalls.pop();
                    }
                }
                result.push(rewritten);
            }
            _ => {
                result.push(op.clone());
                previous_protected_raise_target = if kind == "raise" {
                    try_stack
                        .last()
                        .and_then(|(_, _, handler_label)| *handler_label)
                } else {
                    None
                };
            }
        }
    }
    // Find variables that escape pcall scope.
    let mut escaped: BTreeSet<String> = BTreeSet::new();
    let mut defined_in_pcall: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for &(start, end, _n) in &pcall_ranges {
        if end == 0 {
            continue;
        }
        for (idx, op) in result.iter().enumerate() {
            if idx > start
                && idx < end
                && let Some(ref out_name) = op.out
                && out_name != "none"
                && !op.kind.starts_with("nop")
            {
                defined_in_pcall
                    .entry(out_name.clone())
                    .or_default()
                    .push((start, end));
            }
        }
    }
    for (idx, op) in result.iter().enumerate() {
        let refs: Vec<&str> = op
            .args
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|s| s.as_str())
            .chain(op.var.as_deref())
            .collect();
        for r in refs {
            if let Some(ranges) = defined_in_pcall.get(r) {
                let inside_any = ranges.iter().any(|&(s, e)| idx > s && idx < e);
                if !inside_any {
                    escaped.insert(r.to_string());
                }
            }
        }
    }
    (result, escaped)
}

fn is_exception_edge_block_arg_store(op: &OpIR) -> bool {
    if op.kind != "store_var" {
        return false;
    }
    op.var
        .as_deref()
        .or(op.out.as_deref())
        .is_some_and(|name| name.starts_with("_bb") && name.contains("_arg"))
}

fn store_reads_value(store: &OpIR, value: &str) -> bool {
    store
        .args
        .as_deref()
        .is_some_and(|args| args.iter().any(|arg| arg == value))
        || store.var.as_deref() == Some(value)
        || store.out.as_deref() == Some(value)
}

pub(super) fn hoist_exception_edge_block_arg_stores(ops: &[OpIR]) -> Vec<OpIR> {
    let mut result = Vec::with_capacity(ops.len());
    let mut i = 0usize;

    while i < ops.len() {
        let op = &ops[i];
        if op.kind == "raise" && i + 2 < ops.len() {
            let mut stores_end = i + 1;
            while stores_end < ops.len() && is_exception_edge_block_arg_store(&ops[stores_end]) {
                stores_end += 1;
            }
            if stores_end > i + 1 && stores_end < ops.len() && ops[stores_end].kind == "jump" {
                result.extend(ops[(i + 1)..stores_end].iter().cloned());
                result.push(op.clone());
                i = stores_end;
                continue;
            }
        }
        if i + 2 < ops.len() && !is_exception_edge_block_arg_store(op) {
            let mut stores_end = i + 1;
            while stores_end < ops.len() && is_exception_edge_block_arg_store(&ops[stores_end]) {
                stores_end += 1;
            }
            if stores_end > i + 1
                && stores_end < ops.len()
                && ops[stores_end].kind == "check_exception"
            {
                let depends_on_previous_result = op.out.as_deref().is_some_and(|out| {
                    ops[(i + 1)..stores_end]
                        .iter()
                        .any(|store| store_reads_value(store, out))
                });
                if !depends_on_previous_result {
                    result.extend(ops[(i + 1)..stores_end].iter().cloned());
                    result.push(op.clone());
                    i = stores_end;
                    continue;
                }
            }
        }

        result.push(op.clone());
        i += 1;
    }

    result
}
pub(super) fn lower_iter_to_for(ops: &[OpIR]) -> Vec<OpIR> {
    if ops.is_empty() {
        return ops.to_vec();
    }

    let mut result: Vec<OpIR> = Vec::with_capacity(ops.len());
    let mut i = 0;

    while i < ops.len() {
        // Look for: iter(iterable) at position i
        if ops[i].kind == "iter" {
            let iter_op = &ops[i];
            let iter_out = iter_op.out.as_deref().unwrap_or("");
            let iterable = iter_op
                .args
                .as_deref()
                .and_then(|a| a.first())
                .cloned()
                .unwrap_or_default();

            // Scan forward for the matching loop pattern.
            // We need: ... → loop_start → ... → iter_next(iter_out) → get_item → if → break
            // The pattern can have nil-checks and TypeError guards between iter and loop_start.
            let mut found_pattern = false;
            let mut loop_start_idx = None;
            let mut iter_next_idx = None;
            let mut iter_next_out = String::new();
            let mut value_var = String::new();
            let mut loop_end_idx = None;
            // ops to skip (boilerplate)

            // Find loop_start — skip exception boilerplate (check_exception, raise,
            // exception_last, const_none, is, not, if, end_if, etc.).
            // The frontend emits ~30 boilerplate ops between iter and loop_start.
            for j in (i + 1)..ops.len().min(i + 50) {
                if ops[j].kind == "loop_start" {
                    loop_start_idx = Some(j);
                    break;
                }
            }

            if let Some(ls_idx) = loop_start_idx {
                // Find iter_next — skip check_exception boilerplate after loop_start.
                for j in (ls_idx + 1)..ops.len().min(ls_idx + 15) {
                    if ops[j].kind == "iter_next" {
                        let args = ops[j].args.as_deref().unwrap_or(&[]);
                        if let Some(arg) = args.first() {
                            // The iter_next should reference the iter output or the
                            // iterable variable directly.
                            if arg == iter_out || arg == &iterable {
                                iter_next_idx = Some(j);
                                iter_next_out = ops[j].out.as_deref().unwrap_or("").to_string();
                                break;
                            }
                        }
                    }
                }
            }

            if let Some(in_idx) = iter_next_idx {
                // Find the value extraction from iter_next result.
                // Pattern:
                //   iter_next → index(result, 1) [exhausted] → loop_break_if_true
                //   → index(result, 0) [value] → body
                // The VALUE is the index op that comes AFTER the break check,
                // not the first one (which is the exhausted flag).
                let mut found_break = false;
                let mut exhausted_flag_var: Option<String> = None;
                let mut break_cond_var: Option<String> = None;
                for j in (in_idx + 1)..ops.len().min(in_idx + 30) {
                    if matches!(ops[j].kind.as_str(), "get_item" | "subscript" | "index") {
                        let args = ops[j].args.as_deref().unwrap_or(&[]);
                        if args.len() >= 2 && args[0] == iter_next_out {
                            let out = ops[j].out.as_deref().unwrap_or("").to_string();
                            if !found_break {
                                if exhausted_flag_var.is_none() {
                                    exhausted_flag_var = Some(out);
                                }
                            } else if value_var.is_empty() {
                                value_var = out;
                                break;
                            }
                        }
                        continue;
                    }
                    if matches!(
                        ops[j].kind.as_str(),
                        "loop_break_if_true" | "loop_break_if_false"
                    ) {
                        if let Some(arg) = ops[j].args.as_deref().and_then(|a| a.first()) {
                            break_cond_var = Some(arg.clone());
                        }
                        found_break = true;
                        continue;
                    }
                    // Legacy if/break/end_if forms are intentionally skipped:
                    // without a direct loop_break_if_* guard variable we cannot
                    // prove this is the iterator exhaustion check safely.
                    if !found_break && ops[j].kind == "break" {
                        break;
                    }
                }

                // Find the matching loop_end by counting nesting.
                if let Some(ls_idx) = loop_start_idx {
                    let mut depth = 1i32;
                    for j in (ls_idx + 1)..ops.len() {
                        match ops[j].kind.as_str() {
                            "loop_start" => depth += 1,
                            "loop_end" => {
                                depth -= 1;
                                if depth == 0 {
                                    loop_end_idx = Some(j);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }

                let break_checks_exhaust_flag = matches!(
                    (&exhausted_flag_var, &break_cond_var),
                    (Some(flag), Some(cond)) if flag == cond
                );

                if break_checks_exhaust_flag && !value_var.is_empty() && loop_end_idx.is_some() {
                    found_pattern = true;
                }
            }

            if found_pattern {
                let ls_idx = loop_start_idx.unwrap();
                let in_idx = iter_next_idx.unwrap();
                let le_idx = loop_end_idx.unwrap();

                // Collect all variables referenced by the loop body so we
                // can hoist constant definitions that the body depends on.
                let mut body_refs: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
                for j in (in_idx + 1)..le_idx {
                    if let Some(ref args) = ops[j].args {
                        for a in args {
                            body_refs.insert(a.clone());
                        }
                    }
                    if let Some(ref v) = ops[j].var {
                        body_refs.insert(v.clone());
                    }
                }

                // Emit constant definitions from the skipped region between
                // `iter` and `loop_start` that the body references.  The
                // frontend hoists loop-invariant index constants and string
                // keys before the loop, but collapsing the iter pattern to
                // for_iter drops them.
                for j in (i + 1)..ls_idx {
                    if matches!(
                        ops[j].kind.as_str(),
                        "const"
                            | "const_int"
                            | "const_str"
                            | "const_bool"
                            | "const_float"
                            | "list_new"
                    ) && let Some(ref out) = ops[j].out
                        && body_refs.contains(out)
                    {
                        result.push(ops[j].clone());
                    }
                }

                // Emit for_iter op.
                result.push(OpIR {
                    kind: "for_iter".to_string(),
                    out: Some(value_var.clone()),
                    args: Some(vec![iterable.clone()]),
                    ..OpIR::default()
                });

                // Find where the loop body starts: after the break-on-exhausted
                // pattern (iter_next → get_item → if → break → end_if → get_item).
                // We need to skip the boilerplate and emit only the body.
                // The body starts after the last get_item that unpacks iter_next_out
                // (which assigns the loop variable into a slot).
                let mut body_start = in_idx + 1;

                // Scan past the unpack + break pattern to find body start.
                // Look for the pattern: get_item, const, get_item, if, break, end_if,
                // then optional store into a slot variable.
                let mut break_end = in_idx + 1;
                #[allow(unused_assignments)]
                let mut depth = 0i32;
                let mut seen_break_check = false;
                for j in (in_idx + 1)..le_idx {
                    match ops[j].kind.as_str() {
                        "if" => depth += 1,
                        "end_if" => {
                            depth -= 1;
                            if depth < 0 {
                                break_end = j + 1;
                                depth = 0;
                            }
                        }
                        _ => {}
                    }
                    // Check if this op still references the iter_next output (part of unpack)
                    let refs_iter = ops[j]
                        .args
                        .as_deref()
                        .is_some_and(|args| args.iter().any(|a| a == &iter_next_out));
                    if refs_iter
                        || matches!(
                            ops[j].kind.as_str(),
                            "const_int"
                                | "const"
                                | "break"
                                | "check_exception"
                                | "exception_last"
                                | "exception_last_pending"
                                | "exception_finally_pending_observer"
                                | "const_none"
                                | "is"
                                | "not"
                                | "if"
                                | "end_if"
                                | "raise"
                                | "jump"
                                | "nop"
                                | "line"
                                | "exception_new"
                                | "exception_new_builtin"
                                | "exception_new_builtin_empty"
                                | "exception_new_builtin_one"
                                | "exception_stack_set_depth"
                                | "exception_stack_exit"
                                | "tuple_new"
                                | "const_str"
                                | "loop_break_if_true"
                                | "loop_break_if_false"
                                | "loop_break_if_exception"
                        )
                    {
                        body_start = j + 1;
                    }
                    // Track when we've passed the break check.
                    if matches!(
                        ops[j].kind.as_str(),
                        "loop_break_if_true"
                            | "loop_break_if_false"
                            | "loop_break_if_exception"
                            | "break"
                    ) {
                        seen_break_check = true;
                    }
                    // Stop scanning after end_if at depth 0, but ONLY after we've
                    // already passed the break check. Exception-handling end_if ops
                    // appear BEFORE the break check and we must not stop there.
                    if seen_break_check && ops[j].kind == "end_if" && depth <= 0 && j > in_idx + 2 {
                        body_start = j + 1;
                        break;
                    }
                    // After the break check, once we find the value extraction
                    // (an index op referencing iter_next_out), we're done.
                    if seen_break_check
                        && refs_iter
                        && matches!(ops[j].kind.as_str(), "get_item" | "subscript" | "index")
                    {
                        body_start = j + 1;
                        break;
                    }
                }
                body_start = body_start.max(break_end);

                // Now find the actual value extraction: look for set_item or store
                // ops that write the unpacked value into a usable slot.
                // These appear right after the break check.
                let scan_limit = le_idx.min(body_start + 8);
                let mut body_cursor = body_start;
                for j in body_start..scan_limit {
                    let refs_value = ops[j]
                        .args
                        .as_deref()
                        .is_some_and(|args| args.iter().any(|a| a == &value_var));
                    if refs_value && matches!(ops[j].kind.as_str(), "set_item" | "store_local") {
                        // This stores the loop variable into a slot — part of boilerplate.
                        body_cursor = j + 1;
                    } else if !refs_value && ops[j].kind == "const_int" {
                        // Index constant for the set_item — skip.
                        body_cursor = j + 1;
                    } else {
                        break;
                    }
                }
                body_start = body_cursor;

                // Skip any ops between iter and loop body that are boilerplate
                // (nil checks, TypeError, etc.) — they're between i and ls_idx.
                // We already emitted for_iter, so skip from i to body_start.

                // Emit the body ops (from body_start to loop_end, exclusive).
                for j in body_start..le_idx {
                    // Skip `continue` at the end of the loop body — it's implicit in for loops.
                    if j == le_idx - 1 && ops[j].kind == "continue" {
                        continue;
                    }
                    result.push(ops[j].clone());
                }

                // Emit end_for.
                result.push(OpIR {
                    kind: "end_for".to_string(),
                    ..OpIR::default()
                });

                // Skip past the entire original pattern.
                i = le_idx + 1;

                // Also skip any ops between the original iter and loop_start
                // that were nil-check boilerplate (they're now unnecessary).
                continue;
            }
        }

        result.push(ops[i].clone());
        i += 1;
    }

    result
}

pub(super) fn lower_early_returns(ops: &[OpIR]) -> Vec<OpIR> {
    if ops.is_empty() {
        return ops.to_vec();
    }

    // Phase 1: Find the "return label" pattern.
    // Look for: label(N) → ... → index(out, slot, idx) → ret(out)
    // This tells us which label is the "return exit" and which slot holds
    // the return value.
    let mut return_labels: BTreeMap<i64, (String, String)> = BTreeMap::new(); // label_id → (slot_var, index_var)

    for i in 0..ops.len() {
        if ops[i].kind == "label"
            && let Some(label_id) = ops[i].value
        {
            // Scan forward past exception boilerplate for index → ret.
            // The exit label may contain an exception re-raise block:
            //   exception_stack_set_depth, exception_stack_exit,
            //   exception_last, const_none, is, not,
            //   if → raise → [const_none, ret] → end_if
            // followed by the actual index → ret.
            let mut j = i + 1;
            while j < ops.len() {
                let k = ops[j].kind.as_str();
                if matches!(
                    k,
                    "exception_stack_set_depth"
                        | "exception_stack_exit"
                        | "exception_stack_enter"
                        | "check_exception"
                        | "exception_last"
                        | "exception_last_pending"
                        | "exception_finally_pending_observer"
                        | "const_none"
                        | "is"
                        | "not"
                        | "if"
                        | "raise"
                        | "end_if"
                        | "ret_void"
                        | "nop"
                        | "line"
                ) {
                    j += 1;
                    continue;
                }
                // Skip bare `ret` ops inside the exception re-raise
                // block (no var, no args, followed by a nearby end_if).
                if k == "ret"
                    && ops[j].var.is_none()
                    && ops[j].args.as_ref().is_none_or(|a| a.is_empty())
                {
                    let has_end_if = (j + 1..ops.len()).take(5).any(|m| ops[m].kind == "end_if");
                    if has_end_if {
                        j += 1;
                        continue;
                    }
                }
                if k == "index"
                    && let (Some(out), Some(args)) = (&ops[j].out, &ops[j].args)
                    && args.len() >= 2
                {
                    let slot = &args[0];
                    // Look for ret following this index
                    let mut m = j + 1;
                    while m < ops.len() {
                        let mk = ops[m].kind.as_str();
                        if matches!(
                            mk,
                            "check_exception"
                                | "exception_stack_set_depth"
                                | "exception_stack_exit"
                                | "nop"
                                | "line"
                        ) {
                            m += 1;
                            continue;
                        }
                        if mk == "ret" {
                            // Match ret with explicit var reference.
                            if let Some(ref ret_var) = ops[m].var
                                && ret_var == out
                            {
                                return_labels.insert(label_id, (slot.clone(), args[1].clone()));
                            }
                            // Also match bare ret (no var/args) that
                            // follows index — the index already read
                            // the return value into scope.
                            if ops[m].var.is_none()
                                && ops[m].args.as_ref().is_none_or(|a| a.is_empty())
                            {
                                return_labels.insert(label_id, (slot.clone(), args[1].clone()));
                            }
                        }
                        break;
                    }
                }
                break;
            }
        }
    }

    if return_labels.is_empty() {
        return ops.to_vec();
    }

    // Phase 2: Find store_index(slot, idx, value) → jump(exit_label) patterns
    // and replace with direct return.
    let mut result = Vec::with_capacity(ops.len());
    let mut i = 0;
    'outer: while i < ops.len() {
        if ops[i].kind == "store_index"
            && let Some(ref args) = ops[i].args
            && args.len() >= 3
        {
            let slot = &args[0];
            let idx = &args[1];
            let value = &args[2];

            // Look ahead past exception boilerplate for a jump to a return label.
            let mut j = i + 1;
            while j < ops.len() {
                let k = ops[j].kind.as_str();
                if matches!(
                    k,
                    "check_exception"
                        | "exception_stack_set_depth"
                        | "exception_stack_exit"
                        | "exception_last"
                        | "exception_last_pending"
                        | "exception_finally_pending_observer"
                        | "const_none"
                        | "is"
                        | "not"
                        | "if"
                        | "raise"
                        | "end_if"
                        | "nop"
                        | "line"
                ) {
                    j += 1;
                    continue;
                }
                if (k == "jump" || k == "label")
                    && let Some(target_label) = ops[j].value
                    && let Some((ret_slot, ret_idx)) = return_labels.get(&target_label)
                    && slot == ret_slot
                    && idx == ret_idx
                {
                    // Match! Replace store_index + boilerplate with ret.
                    result.push(OpIR {
                        kind: "ret".to_string(),
                        var: Some(value.clone()),
                        ..OpIR::default()
                    });
                    if k == "jump" {
                        i = j + 1;
                    } else {
                        // label fall-through: keep the label
                        i = j;
                    }
                    continue 'outer;
                }
                break;
            }
        }
        // Phase 3: Handle direct store_index → [boilerplate] → index → ret
        // without any jump/label. This pattern appears when a function has
        // exactly one code path (no early returns).
        if ops[i].kind == "store_index"
            && let Some(ref args) = ops[i].args
            && args.len() >= 3
        {
            let slot = &args[0];
            let idx = &args[1];
            let value = &args[2];

            // Scan forward for index(out, slot, idx) → ret
            let mut j = i + 1;
            let mut found_index_out = None;
            while j < ops.len() {
                let k = ops[j].kind.as_str();
                if matches!(
                    k,
                    "check_exception"
                        | "exception_stack_set_depth"
                        | "exception_stack_exit"
                        | "exception_stack_enter"
                        | "exception_last"
                        | "exception_last_pending"
                        | "exception_finally_pending_observer"
                        | "const_none"
                        | "is"
                        | "not"
                        | "if"
                        | "raise"
                        | "end_if"
                        | "ret_void"
                        | "nop"
                        | "line"
                ) {
                    j += 1;
                    continue;
                }
                // Skip bare ret inside exception re-raise blocks.
                if k == "ret"
                    && ops[j].var.is_none()
                    && ops[j].args.as_ref().is_none_or(|a| a.is_empty())
                {
                    let has_end_if = (j + 1..ops.len()).take(5).any(|m| ops[m].kind == "end_if");
                    if has_end_if {
                        j += 1;
                        continue;
                    }
                }
                if k == "index"
                    && let Some(ref idx_args) = ops[j].args
                    && idx_args.len() >= 2
                    && &idx_args[0] == slot
                    && &idx_args[1] == idx
                {
                    found_index_out = ops[j].out.clone();
                    j += 1;
                    continue;
                }
                // Found a bare ret after the index — replace the
                // whole sequence with ret(value).
                if k == "ret" && found_index_out.is_some() {
                    let bare =
                        ops[j].var.is_none() && ops[j].args.as_ref().is_none_or(|a| a.is_empty());
                    let refs_index = ops[j].var.as_ref() == found_index_out.as_ref();
                    if bare || refs_index {
                        result.push(OpIR {
                            kind: "ret".to_string(),
                            var: Some(value.clone()),
                            ..OpIR::default()
                        });
                        i = j + 1;
                        continue 'outer;
                    }
                }
                break;
            }
        }

        result.push(ops[i].clone());
        i += 1;
    }

    result
}

pub(super) fn strip_dead_after_return(ops: &[OpIR]) -> Vec<OpIR> {
    let mut result = Vec::with_capacity(ops.len());
    let mut depth: i32 = 0;
    let mut dead_at_depth: Option<i32> = None; // depth at which we became dead
    let referenced_labels: BTreeSet<i64> = ops
        .iter()
        .filter_map(|op| {
            matches!(
                op.kind.as_str(),
                "jump"
                    | "goto"
                    | "br_if"
                    | "branch"
                    | "branch_false"
                    | "check_exception"
                    | "try_start"
                    | "state_block_start"
                    | "loop_break_if_true"
                    | "loop_break_if_false"
                    | "loop_break_if_exception"
            )
            .then_some(op.value)
            .flatten()
        })
        .collect();

    for op in ops {
        let kind = op.kind.as_str();

        // Track structured nesting.
        let is_open = matches!(kind, "if" | "loop_start" | "for_range" | "for_iter");
        let is_mid = matches!(kind, "else");
        let is_close = matches!(kind, "end_if" | "loop_end" | "end_for");

        if is_open {
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            depth += 1;
            continue;
        }
        if is_mid {
            // `else` doesn't change depth but resets dead state if we're
            // dead at this depth (the other branch may not have returned).
            if dead_at_depth == Some(depth) {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }
        if is_close {
            depth -= 1;
            // Closing a block may bring us back to a reachable state.
            if let Some(d) = dead_at_depth
                && d > depth
            {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }

        // try_start/try_end are structural markers that must always be
        // preserved for pcall lowering, even in dead-code regions.
        if matches!(kind, "try_start" | "try_end") {
            result.push(op.clone());
            continue;
        }

        // Out-of-line exception handlers and branch targets can legally appear
        // after a return in the linearized stream. A live label starts a new
        // reachable block even when the preceding block is closed.
        if matches!(kind, "label" | "state_label")
            && op
                .value
                .is_some_and(|label| referenced_labels.contains(&label))
        {
            dead_at_depth = None;
            result.push(op.clone());
            continue;
        }

        // If we're in dead code, skip this op.
        if let Some(d) = dead_at_depth {
            if depth >= d {
                continue;
            }
            // We're at a shallower depth now — no longer dead.
            dead_at_depth = None;
        }

        // Check if this op is an unconditional return.
        let is_return = matches!(kind, "ret" | "return" | "return_value" | "ret_void");
        result.push(op.clone());

        if is_return {
            dead_at_depth = Some(depth);
        }
    }

    result
}
