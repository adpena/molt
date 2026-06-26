use super::def_use::{split_ir_defined_names, split_ir_read_names};
use super::runtime_roots::is_protected_runtime_entrypoint;
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};

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

pub(super) fn verify_split_function_def_use(func: &FunctionIR) -> Result<(), String> {
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

pub(super) fn verify_split_generated_ops(func: &FunctionIR) -> Result<(), String> {
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

pub(super) fn is_drop_fact_marker_op(op: &OpIR) -> bool {
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
