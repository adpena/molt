mod name_index;
use name_index::SplitNameIndex;

use super::runtime_roots::is_protected_runtime_entrypoint;
use crate::tir::op_kinds_generated::{
    ExceptionRegionNestingRole, SimpleIrReturnShape, SimpleIrVerifierRegionRole,
    kind_to_opcode_table, opcode_exception_region_nesting_role_table,
    simpleir_kind_is_return_terminator, simpleir_kind_is_verifier_label_definition,
    simpleir_kind_is_verifier_label_reference, simpleir_kind_is_verifier_loop_scoped,
    simpleir_kind_uses_function_label_id, simpleir_return_shape, simpleir_verifier_region_role,
};
use crate::tir::simple_def_use::{
    simple_ir_return_has_value, visit_simple_ir_defined_names, visit_simple_ir_reads,
};
use crate::{ExecutionContextPolicy, FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};

/// The private chunk result is a transport protocol, not the owner's ABI.
/// In particular, a payload-free owner still needs value-returning chunks
/// when a cloned terminal must stop subsequent chunks.
#[derive(Clone, Copy)]
enum ChunkReturnProtocol {
    Fallthrough,
    OwnerValue,
    ContinuationStatus,
}

impl ChunkReturnProtocol {
    fn return_abi(self) -> molt_ir::FunctionReturnAbi {
        match self {
            Self::Fallthrough => molt_ir::FunctionReturnAbi::Void,
            Self::OwnerValue | Self::ContinuationStatus => molt_ir::FunctionReturnAbi::Value,
        }
    }

    fn result_name_base(self) -> &'static str {
        match self {
            Self::Fallthrough => "__molt_split_chunk_discard",
            Self::OwnerValue => "__molt_split_chunk_return",
            Self::ContinuationStatus => "__molt_split_chunk_continue",
        }
    }
}

// ---------------------------------------------------------------------------
// Megafunction splitting pass
//
// Cranelift's register allocator has O(n^2) behavior on very large functions.
// When a function exceeds max_ops (default 2000, env: MOLT_MAX_FUNCTION_OPS),
// this pass splits it at top-level statement boundaries (loop_depth=0,
// if_depth=0) into module-reserved, injectively named private chunk functions. The original
// function is replaced with sequential call_internal ops to each chunk.
//
// Safety: never splits inside loops, if-blocks, or try-blocks.
// ---------------------------------------------------------------------------

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

fn split_label_id(occupied: &mut BTreeSet<i64>, cursor: &mut i64) -> Option<i64> {
    loop {
        let candidate = *cursor;
        *cursor = cursor.checked_add(1)?;
        if occupied.insert(candidate) {
            return Some(candidate);
        }
    }
}

fn split_collect_names(ops: &[OpIR], params: &[String]) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = params
        .iter()
        .filter(|name| name.as_str() != "none")
        .cloned()
        .collect();
    for op in ops {
        visit_simple_ir_reads(op, |source| {
            names.insert(source.name.to_string());
        });
        visit_simple_ir_defined_names(op, |name| {
            names.insert(name.to_string());
        });
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
            args: Some(vec![name]),
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
        if simpleir_kind_is_return_terminator(op.kind.as_str()) && !simple_ir_return_has_value(&op)
        {
            rewritten.extend(split_status_return_ops(occupied, should_continue));
        } else {
            rewritten.push(op);
        }
    }
    rewritten
}

fn split_insert_local_frame_exits(ops: Vec<OpIR>) -> Vec<OpIR> {
    let mut with_exits = Vec::with_capacity(ops.len() + 4);
    for op in ops {
        if simpleir_kind_is_return_terminator(op.kind.as_str()) {
            with_exits.push(OpIR {
                kind: "trace_exit".to_string(),
                ..OpIR::default()
            });
        }
        with_exits.push(op);
    }
    with_exits
}

struct SplitLocalFrameOwner<'a> {
    entry_ops: &'a [OpIR],
    body_ops: &'a [OpIR],
    failure_tail: &'a [OpIR],
}

fn split_local_frame_owner(
    ops: &[OpIR],
    drop_marker_count: usize,
) -> Option<SplitLocalFrameOwner<'_>> {
    let entry_end = drop_marker_count.checked_add(2)?;
    let entry_ops = ops.get(drop_marker_count..entry_end)?;
    if entry_ops[0].kind != "trace_enter_slot"
        || entry_ops[1].kind != "check_exception"
        || ops
            .iter()
            .filter(|op| op.kind == "trace_enter_slot")
            .count()
            != 1
    {
        return None;
    }
    let failure_label = entry_ops[1].value?;
    let tail_start = ops.len().checked_sub(3)?;
    let failure_tail = &ops[tail_start..];
    if failure_tail[0].kind != "label"
        || failure_tail[0].value != Some(failure_label)
        || failure_tail[1].kind != "trace_exit"
        || simpleir_return_shape(&failure_tail[2].kind) != SimpleIrReturnShape::Void
    {
        return None;
    }
    let body_ops = ops.get(entry_end..tail_start)?;
    if body_ops
        .last()
        .is_none_or(|op| !simpleir_kind_is_return_terminator(&op.kind))
    {
        // The entry-only tail must not also own ordinary body fallthrough.
        return None;
    }
    if ops.iter().enumerate().any(|(index, op)| {
        simpleir_kind_uses_function_label_id(&op.kind)
            && op.value == Some(failure_label)
            && index != drop_marker_count + 1
            && index != tail_start
    }) {
        // Include every generated label role, not only the splitter's branch
        // subset: try, async and state edges must not retain a moved target.
        return None;
    }
    Some(SplitLocalFrameOwner {
        entry_ops,
        body_ops,
        failure_tail,
    })
}

pub(super) fn verify_split_function_def_use(func: &FunctionIR) -> Result<(), String> {
    let mut defined: BTreeSet<String> = func.params.iter().cloned().collect();
    for (idx, op) in func.ops.iter().enumerate() {
        let mut undefined = None;
        visit_simple_ir_reads(op, |source| {
            if undefined.is_none() && !defined.contains(source.name) {
                undefined = Some(source.name.to_string());
            }
        });
        if let Some(name) = undefined {
            return Err(format!(
                "function `{}` op {} `{}` reads `{}` before definition",
                func.name, idx, op.kind, name
            ));
        }
        visit_simple_ir_defined_names(op, |name| {
            defined.insert(name.to_string());
        });
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
    Ok(())
}

/// The freshly allocated frame identity, not a spelling prefix, identifies
/// generated accesses. Validate both the slot producer and the actual value
/// being loaded/stored against the layout used by the emitters.
pub(super) fn verify_split_frame_ops(
    func: &FunctionIR,
    frame_name: &str,
    frame_slot_for: &BTreeMap<String, usize>,
) -> Result<(), String> {
    for (idx, op) in func.ops.iter().enumerate() {
        let Some(args) = op.args.as_ref() else {
            continue;
        };
        if args.first().map(String::as_str) != Some(frame_name)
            || !matches!(op.kind.as_str(), "index" | "store_index")
        {
            continue;
        }
        let value_name = if op.kind == "index" && args.len() == 2 {
            op.out.as_ref()
        } else if op.kind == "store_index" && args.len() == 3 {
            args.get(2)
        } else {
            None
        };
        let slot = value_name.and_then(|name| frame_slot_for.get(name));
        let producer = idx.checked_sub(1).and_then(|index| func.ops.get(index));
        if !producer.is_some_and(|producer| {
            producer.kind == "const"
                && producer.out.as_ref() == args.get(1)
                && slot.is_some_and(|slot| producer.value == Some(*slot as i64))
        }) {
            return Err(format!(
                "function `{}` op {} has an invalid split-frame slot/value binding",
                func.name, idx
            ));
        }
    }
    Ok(())
}

fn split_control_target(op: &OpIR) -> Option<i64> {
    simpleir_kind_is_verifier_label_reference(&op.kind)
        .then_some(op.value)
        .flatten()
}

/// One balanced-region scan owns both legal partition boundaries and legal
/// suffix starts. Reject malformed nesting before any source is cloned. The
/// generated verifier roles own if/loop structure; generated exception roles
/// additionally keep a lexical try region within one chunk.
pub(super) fn split_region_boundaries(ops: &[OpIR]) -> Option<Vec<bool>> {
    let mut stack: Vec<(&str, bool)> = Vec::new();
    let mut top_level = Vec::with_capacity(ops.len() + 1);
    for op in ops {
        top_level.push(stack.is_empty());
        let role =
            simpleir_verifier_region_role(&op.kind).or_else(|| {
                match kind_to_opcode_table(&op.kind).map(opcode_exception_region_nesting_role_table)
                {
                    Some(ExceptionRegionNestingRole::Enter) => {
                        Some(("try", SimpleIrVerifierRegionRole::Start))
                    }
                    Some(ExceptionRegionNestingRole::Exit) => {
                        Some(("try", SimpleIrVerifierRegionRole::End))
                    }
                    _ => None,
                }
            });
        let Some((region, role)) = role else {
            if simpleir_kind_is_verifier_loop_scoped(&op.kind)
                && !stack.iter().any(|(region, _)| *region == "loop")
            {
                return None;
            }
            continue;
        };
        match role {
            SimpleIrVerifierRegionRole::Start => stack.push((region, false)),
            SimpleIrVerifierRegionRole::Alternate => {
                let (active, seen) = stack.last_mut()?;
                if *active != region || *seen {
                    return None;
                }
                *seen = true;
            }
            SimpleIrVerifierRegionRole::End => {
                if stack.pop()?.0 != region {
                    return None;
                }
            }
        }
    }
    if !stack.is_empty() {
        return None;
    }
    top_level.push(true);
    Some(top_level)
}

pub(super) fn is_drop_fact_marker_op(op: &OpIR) -> bool {
    matches!(
        op.kind.as_str(),
        crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR
            | crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR
    )
}

/// Default maximum number of ops before a function is split into chunks.
///
/// Native frontend module chunking already targets 2000 ops, but lower/midend
/// rewrites can still inflate a chunk well past that budget. Keep the backend
/// splitter aligned with that native default so Cranelift does not see giant
/// `*_molt_module_chunk_*` functions slip through unsplit.
const DEFAULT_MAX_FUNCTION_OPS: usize = 2000;

pub(super) fn split_chunk_name(source_function_name: &str, index: usize) -> String {
    let mut encoded_source_name = String::with_capacity(source_function_name.len() * 2);
    for byte in source_function_name.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut encoded_source_name, "{byte:02x}")
            .expect("writing hexadecimal bytes to String cannot fail");
    }
    format!(
        "__molt_chunk_v1_{}_{encoded_source_name}_{index}",
        source_function_name.len()
    )
}

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
    occupied_function_names: &mut BTreeSet<String>,
) -> Result<(FunctionIR, Vec<FunctionIR>), Box<FunctionIR>> {
    if is_protected_runtime_entrypoint(&func.name) {
        return Err(Box::new(func));
    }

    if func.ops.len() <= max_ops {
        return Err(Box::new(func));
    }
    let execution_context = func.execution_context;
    let drop_fact_markers: Vec<OpIR> = func
        .ops
        .iter()
        .take_while(|op| is_drop_fact_marker_op(op))
        .cloned()
        .collect();
    let local_frame_owner = if execution_context == ExecutionContextPolicy::Local {
        let Some(owner) = split_local_frame_owner(&func.ops, drop_fact_markers.len()) else {
            // A staged module prologue can publish state and fail before frame
            // entry. Never hoist entry across it or discard its rollback path.
            return Err(Box::new(func));
        };
        Some(owner)
    } else {
        None
    };
    let chunk_execution_context = match execution_context {
        ExecutionContextPolicy::Local | ExecutionContextPolicy::Inherited => {
            ExecutionContextPolicy::Inherited
        }
        ExecutionContextPolicy::None => ExecutionContextPolicy::None,
    };
    // Keep the source immutable through every refusal. Only the body enters
    // partition/liveness planning; checked entry and its private failure tail
    // remain one ownership unit in the replacement Local stub.
    let all_ops = local_frame_owner
        .as_ref()
        .map_or(func.ops.as_slice(), |owner| owner.body_ops);
    let Some(top_level) = split_region_boundaries(all_ops) else {
        return Err(Box::new(func));
    };
    let name_index = SplitNameIndex::new(all_ops);
    let parameter_names = func
        .params
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    // Every generated label reference is protected by the same range and
    // suffix-cloning authority, including exception and ordinary branch edges.

    // ---------------------------------------------------------------
    // 1. Find safe split points (indices where depth == 0).
    //    A split point is the index of the *first* op of a new chunk,
    //    i.e. the boundary falls just before that index.
    //
    //    Each target must remain in the referencing chunk, either retained
    //    with the original region or brought in as a balanced cloned suffix.
    // ---------------------------------------------------------------

    // Build forbidden ranges: for every generated label-reference role,
    // find the span covering the reference(s) and the label definition, and forbid splitting
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
    for (idx, op) in all_ops.iter().enumerate() {
        if simpleir_kind_is_verifier_label_definition(&op.kind) {
            let Some(id) = op.value else {
                return Err(Box::new(func));
            };
            if label_positions.insert(id, idx).is_some() {
                return Err(Box::new(func));
            }
        } else if simpleir_kind_is_verifier_label_reference(&op.kind) {
            let Some(id) = split_control_target(op) else {
                return Err(Box::new(func));
            };
            let entry = label_refs.entry(id).or_insert((idx, idx));
            entry.0 = entry.0.min(idx);
            entry.1 = entry.1.max(idx);
        }
    }
    // Compute forbidden ranges: a split point at index `sp` is forbidden
    // if it falls strictly between a label reference and its definition.
    let mut label_forbidden_ranges: Vec<(usize, usize, i64, usize)> = Vec::new();
    // A suffix is cloneable only when it has no value-return terminator.
    // One reverse location replaces a full suffix scan for every label.
    let last_value_return = all_ops.iter().rposition(simple_ir_return_has_value);
    let cloneable_suffix_labels: BTreeMap<i64, usize> = label_positions
        .iter()
        .filter_map(|(&label_id, &label_idx)| {
            (top_level[label_idx] && last_value_return.is_none_or(|last| last < label_idx))
                .then_some((label_id, label_idx))
        })
        .collect();
    for (label_id, (earliest_ref, latest_ref)) in &label_refs {
        if let Some(&label_idx) = label_positions.get(label_id) {
            let range_start = (*earliest_ref).min(label_idx);
            let range_end = (*latest_ref).max(label_idx);
            label_forbidden_ranges.push((range_start, range_end, *label_id, label_idx));
        }
    }

    let suffix_can_clone_into_range =
        |label_id: i64, chunk_start: usize, chunk_end: usize| -> bool {
            let Some(&label_idx) = cloneable_suffix_labels.get(&label_id) else {
                return false;
            };
            if label_idx < chunk_end {
                return false;
            }
            // Linear live-in facts are precisely the suffix's reads before
            // local definition. Query sparse events without cloning an entire
            // available-name set at every candidate boundary.
            name_index.live_names(label_idx).all(|name| {
                parameter_names.contains(name)
                    || name_index.is_live_before(name, chunk_start)
                    || name_index.is_defined_in(name, chunk_start, chunk_end)
            })
        };

    let chunk_refs_label = |chunk_start: usize, chunk_end: usize, label_id: i64| -> bool {
        all_ops[chunk_start..chunk_end]
            .iter()
            .any(|op| split_control_target(op) == Some(label_id))
    };

    let chunk_has_external_control_without_safe_clone =
        |chunk_start: usize, chunk_end: usize| -> bool {
            for op in &all_ops[chunk_start..chunk_end] {
                let Some(target_id) = split_control_target(op) else {
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

    for (idx, op) in all_ops.iter().enumerate() {
        // Split only at top-level statement boundaries. A raw depth==0 op
        // index is not sufficient: large class statements emit thousands of
        // ops (method FuncNew, CLASS_DEF, export, annotation wiring) without
        // increasing structured-control depth, so splitting at an arbitrary
        // op boundary can sever one logical statement across chunks.
        let is_stmt_boundary = op.kind == "line";
        if top_level[idx]
            && idx > 0
            && is_stmt_boundary
            && idx - last_split >= max_ops
            && !is_forbidden(idx, last_split)
        {
            selected.push(idx);
            last_split = idx;
        }
    }

    // If no selected splits, the function is too deeply nested to split.
    if selected.is_empty() {
        return Err(Box::new(func));
    }

    // ---------------------------------------------------------------
    // 3. Partition ops into chunks at the selected split points.
    // ---------------------------------------------------------------
    let mut boundaries: Vec<usize> = Vec::new();
    boundaries.push(0);
    boundaries.extend_from_slice(&selected);
    boundaries.push(all_ops.len());

    // Validate: ensure no chunk exceeds max_ops. If any chunk is oversized,
    // the function has a deeply nested region that can't be split cleanly.
    for window in boundaries.windows(2) {
        let chunk_size = window[1] - window[0];
        if chunk_size > max_ops.saturating_mul(2) {
            // Allow up to 2x max_ops for the final chunk — beyond that,
            // return Err to fall back to single-module compilation.
            return Err(Box::new(func));
        }
    }

    let chunk_names = (0..boundaries.len() - 1)
        .map(|index| split_chunk_name(&func.name, index))
        .collect::<Vec<_>>();
    if chunk_names
        .iter()
        .any(|name| occupied_function_names.contains(name))
    {
        return Err(Box::new(func));
    }

    // Payload shape selects the chunk transport algorithm, never the owner ABI.
    // A value-ABI function may now contain only empty returns; the stub retains
    // func.return_abi even when its chunks use the no-payload status protocol.
    let body_has_value_returns = all_ops.iter().any(simple_ir_return_has_value);
    for (idx, op) in all_ops.iter().enumerate() {
        if simpleir_kind_is_return_terminator(op.kind.as_str()) && idx + 1 != all_ops.len() {
            return Err(Box::new(func));
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
        for name in name_index.live_names(boundary) {
            if name_index.is_defined_before(name, boundary) {
                frame_names.insert(name.to_string());
            }
        }
    }
    let frame_slot_for: BTreeMap<String, usize> = frame_names
        .iter()
        .enumerate()
        .map(|(idx, name)| (name.clone(), idx))
        .collect();
    let uses_split_frame = !frame_slot_for.is_empty();
    let mut occupied_labels = func
        .ops
        .iter()
        .filter(|op| simpleir_kind_uses_function_label_id(&op.kind))
        .filter_map(|op| op.value)
        .collect::<BTreeSet<_>>();
    let mut next_synthetic_label = 0;
    let Some(exception_return_label) =
        split_label_id(&mut occupied_labels, &mut next_synthetic_label)
    else {
        return Err(Box::new(func));
    };

    struct ChunkPlan {
        name: String,
        return_protocol: ChunkReturnProtocol,
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
        let live_in: BTreeSet<String> = name_index
            .live_names(start)
            .filter(|name| frame_slot_for.contains_key(*name))
            .map(str::to_string)
            .collect();
        let live_out: BTreeSet<String> = name_index
            .live_names(end)
            .filter(|name| frame_slot_for.contains_key(*name))
            .map(str::to_string)
            .collect();

        // Collect label IDs defined in THIS chunk.
        let mut chunk_labels: std::collections::BTreeSet<i64> = chunk_ops
            .iter()
            .filter(|op| simpleir_kind_is_verifier_label_definition(&op.kind))
            .filter_map(|op| op.value)
            .collect();

        // If the chunk references a shared exception/cleanup tail that starts
        // later in the original function, clone that suffix into the chunk so
        // every generated label-reference role stays valid after splitting.
        let mut normal_skip_label_for_cloned_suffix = None;
        let suffix_clone_start = chunk_ops
            .iter()
            .filter_map(|op| {
                let target_id = split_control_target(op)?;
                if chunk_labels.contains(&target_id) {
                    return None;
                }
                let &label_idx = cloneable_suffix_labels.get(&target_id)?;
                (label_idx >= end && suffix_can_clone_into_range(target_id, start, end))
                    .then_some(label_idx)
            })
            .min();
        if let Some(suffix_start) = suffix_clone_start {
            let Some(skip_label) = split_label_id(&mut occupied_labels, &mut next_synthetic_label)
            else {
                return Err(Box::new(func));
            };
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
                .filter(|op| simpleir_kind_is_verifier_label_definition(&op.kind))
                .filter_map(|op| op.value)
                .collect();
            if chunk_ops.len() > max_ops.saturating_mul(2) {
                return Err(Box::new(func));
            }
        }

        if chunk_ops.iter().any(|op| {
            split_control_target(op).is_some_and(|target_id| !chunk_labels.contains(&target_id))
        }) {
            return Err(Box::new(func));
        }

        // The replacement stub is the sole Local frame owner. Chunks retain
        // source-line/locals work on the bound inherited context, but may never
        // create or destroy lifecycle state (including cloned cleanup tails).
        // The checked entry pair and its private failure tail were excluded
        // from body planning, so no entry guard can survive inside a chunk.
        if execution_context == ExecutionContextPolicy::Local {
            chunk_ops.retain(|op| op.kind != "trace_exit");
        }

        let chunk_name = chunk_names[i].clone();
        let return_protocol = if body_has_value_returns {
            let terminal = if chunk_ops
                .last()
                .is_some_and(|op| simpleir_kind_is_return_terminator(op.kind.as_str()))
            {
                chunk_ops.pop()
            } else {
                None
            };
            let returns_value = terminal.as_ref().is_some_and(simple_ir_return_has_value);
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
                        return Err(Box::new(func));
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
            if returns_value {
                ChunkReturnProtocol::OwnerValue
            } else {
                ChunkReturnProtocol::Fallthrough
            }
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
                        return Err(Box::new(func));
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
            ChunkReturnProtocol::ContinuationStatus
        };
        let mut chunk_params = func.params.clone();
        if uses_split_frame {
            chunk_params.push(frame_name.clone());
        }
        let chunk_param_types =
            split_param_types_for_names(&func.params, func.param_types.as_ref(), &chunk_params);
        chunks.push(FunctionIR {
            return_abi: return_protocol.return_abi(),
            name: chunk_name.clone(),
            params: chunk_params,
            ops: chunk_ops,
            param_types: chunk_param_types,
            source_file: func.source_file.clone(),
            is_extern: false,
            codegen_partition: true,
            execution_context: chunk_execution_context,
        });
        plans.push(ChunkPlan {
            name: chunk_name,
            return_protocol,
        });
    }

    // ---------------------------------------------------------------
    // 4. Build the stub parent function. Values defined in one chunk and read
    //    by later chunks travel through one explicit heap frame instead of
    //    relying on per-function entry defaults.
    // ---------------------------------------------------------------
    let mut stub_ops: Vec<OpIR> = Vec::new();
    if let Some(owner) = &local_frame_owner {
        stub_ops.extend_from_slice(owner.entry_ops);
    }
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
        stub_ops.push(OpIR {
            kind: "check_exception".to_string(),
            value: Some(exception_return_label),
            ..OpIR::default()
        });
    }
    for plan in &plans {
        let mut call_args = func.params.clone();
        if uses_split_frame {
            call_args.push(frame_name.clone());
        }
        let chunk_result_name =
            split_frame_name(plan.return_protocol.result_name_base(), &mut occupied_names);
        stub_ops.push(OpIR {
            kind: "call_internal".to_string(),
            s_value: Some(plan.name.clone()),
            args: Some(call_args),
            out: Some(chunk_result_name.clone()),
            passes_execution_context: chunk_execution_context == ExecutionContextPolicy::Inherited,
            ..OpIR::default()
        });
        stub_ops.push(OpIR {
            kind: "check_exception".to_string(),
            value: Some(exception_return_label),
            ..OpIR::default()
        });
        match plan.return_protocol {
            ChunkReturnProtocol::ContinuationStatus => {
                let Some(continue_label) =
                    split_label_id(&mut occupied_labels, &mut next_synthetic_label)
                else {
                    return Err(Box::new(func));
                };
                stub_ops.push(OpIR {
                    kind: "br_if".to_string(),
                    args: Some(vec![chunk_result_name]),
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
            }
            ChunkReturnProtocol::OwnerValue => stub_ops.push(OpIR {
                kind: "ret".to_string(),
                args: Some(vec![chunk_result_name]),
                ..OpIR::default()
            }),
            ChunkReturnProtocol::Fallthrough => {}
        }
    }
    // Empty exits preserve the authored owner ABI. Final machine lowering
    // supplies its carrier, without synthetic payloads in the split IR.
    stub_ops.push(OpIR {
        kind: "ret_void".to_string(),
        ..OpIR::default()
    });
    stub_ops.push(OpIR {
        kind: "label".to_string(),
        value: Some(exception_return_label),
        ..OpIR::default()
    });
    stub_ops.push(OpIR {
        kind: "ret_void".to_string(),
        ..OpIR::default()
    });
    if execution_context == ExecutionContextPolicy::Local {
        stub_ops = split_insert_local_frame_exits(stub_ops);
    }
    if let Some(owner) = &local_frame_owner {
        // Preserve the original failed-attempt return (including ret_void for
        // value-returning bodies). Its exit already exists, so append only
        // after synthesizing exits for the stub's body-return paths.
        stub_ops.extend_from_slice(owner.failure_tail);
    }

    let stub = FunctionIR {
        return_abi: func.return_abi,
        name: func.name,
        params: func.params,
        ops: stub_ops,
        param_types: func.param_types,
        source_file: func.source_file,
        is_extern: false,
        codegen_partition: true,
        execution_context,
    };

    for chunk in &chunks {
        if let Err(detail) = verify_split_function_def_use(chunk) {
            panic!("megafunction split produced invalid chunk IR: {detail}");
        }
        if let Err(detail) = verify_split_generated_ops(chunk) {
            panic!("megafunction split produced non-canonical chunk IR: {detail}");
        }
        if uses_split_frame
            && let Err(detail) = verify_split_frame_ops(chunk, &frame_name, &frame_slot_for)
        {
            panic!("megafunction split produced invalid frame transport: {detail}");
        }
    }
    if let Err(detail) = verify_split_function_def_use(&stub) {
        panic!("megafunction split produced invalid stub IR: {detail}");
    }
    if let Err(detail) = verify_split_generated_ops(&stub) {
        panic!("megafunction split produced non-canonical stub IR: {detail}");
    }
    let transformed = SimpleIR {
        functions: std::iter::once(stub).chain(chunks).collect(),
        profile: None,
    };
    if let Err(detail) = crate::validate_simple_ir(&transformed) {
        panic!("megafunction split produced invalid execution-context IR: {detail}");
    }

    occupied_function_names.extend(chunk_names);

    let mut functions = transformed.functions.into_iter();
    let stub = functions
        .next()
        .expect("split validation preserves the parent");
    Ok((stub, functions.collect()))
}

/// Apply megafunction splitting to all oversized functions in the IR.
///
/// Call this before the main compilation loop so that the chunk functions
/// are present in `ir.functions` and will be compiled normally.
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub fn split_megafunctions(ir: &mut SimpleIR) -> BTreeMap<String, String> {
    split_megafunctions_with_filter(ir, |_| true)
}

pub fn split_megafunctions_with_filter(
    ir: &mut SimpleIR,
    should_split: impl Fn(&FunctionIR) -> bool,
) -> BTreeMap<String, String> {
    let max_ops: usize = std::env::var("MOLT_MAX_FUNCTION_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_FUNCTION_OPS);

    split_megafunctions_at_limit(ir, max_ops, should_split)
}

pub(super) fn split_megafunctions_at_limit(
    ir: &mut SimpleIR,
    max_ops: usize,
    should_split: impl Fn(&FunctionIR) -> bool,
) -> BTreeMap<String, String> {
    let mut split_sources = BTreeMap::new();
    let mut new_functions: Vec<FunctionIR> = Vec::new();
    let old_functions = std::mem::take(&mut ir.functions);
    let mut occupied_function_names = old_functions
        .iter()
        .map(|function| function.name.clone())
        .collect::<BTreeSet<_>>();

    for func in old_functions {
        let op_count = func.ops.len();
        if !should_split(&func) {
            new_functions.push(func);
            continue;
        }
        match split_large_function(func, max_ops, &mut occupied_function_names) {
            Ok((stub, chunks)) => {
                eprintln!(
                    "MOLT_BACKEND: split `{}` ({} ops) into {} chunks",
                    stub.name,
                    op_count,
                    chunks.len()
                );
                for chunk in &chunks {
                    split_sources.insert(chunk.name.clone(), stub.name.clone());
                }
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
    split_sources
}
