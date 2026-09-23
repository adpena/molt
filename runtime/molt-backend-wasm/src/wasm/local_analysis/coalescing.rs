use crate::FunctionIR;
use crate::OpIR;
use std::collections::{BTreeMap, BTreeSet};

/// Map every coalescable optimizer temporary to the temporary whose wasm local
/// it shares (a temporary maps to itself when it owns the slot).
///
/// Slots are assigned by a linear scan over live ranges in op order. A live
/// range is `[first write, last read]` in op order, widened so that sharing a
/// slot can never clobber a value that is still needed:
///
/// * Forward control (`if`/`else`, forward `jump`/`br_if`/`check_exception`,
///   early `ret`) executes ops at most once in increasing op order, so the
///   linear range is a superset of the true live range and overlap in op
///   order is a superset of true interference.
/// * Repeated execution comes only from loops and backward edges. Any range
///   that intersects an iteration region (a structured loop body, or the span
///   from a label to a later op that transfers to it) is widened to cover the
///   whole region: a value read in an iteration may be read again by the next
///   one before it is rewritten, and a value written in one may be read after
///   the region by a path that skipped a later write.
/// * Resumable state machines (`state_label`/`state_switch`/`state_yield`/
///   `state_transition`/channel yields) re-enter the body at arbitrary
///   labels from the dispatcher, so they keep one local per temporary.
///
/// The old coalescer refused every function with non-linear control flow,
/// which left module-init chunks and large functions (thousands of ops, a
/// try/except or a loop somewhere) with one wasm local per temporary; the
/// numpy/scipy witness carried a function with 18,280 locals, and Binaryen's
/// `coalesce-locals` (quadratic in locals) needed more than 12 GB for it.
pub(super) fn coalesced_locals(
    func_ir: &FunctionIR,
    read_vars: &BTreeSet<String>,
    param_set: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let ops = &func_ir.ops;
    if ops
        .iter()
        .any(|op| is_resumable_dispatch_kind(op.kind.as_str()))
    {
        return BTreeMap::new();
    }

    let mut first_write: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_read: BTreeMap<String, usize> = BTreeMap::new();
    for (op_idx, op) in ops.iter().enumerate() {
        if let Some(out) = &op.out {
            first_write.entry(out.clone()).or_insert(op_idx);
        }
        if let Some(args) = &op.args {
            for arg in args {
                last_read.insert(arg.clone(), op_idx);
            }
        }
        if let Some(var) = &op.var {
            last_read.insert(var.clone(), op_idx);
        }
    }

    let regions = iteration_regions(ops);

    let mut ranges: Vec<(usize, usize, String)> = Vec::new();
    for (name, start) in &first_write {
        if !is_coalescable_local(name, read_vars, param_set) {
            continue;
        }
        let end = last_read.get(name).copied().unwrap_or(*start);
        let (start, end) = widen_to_iteration_regions(*start, end.max(*start), &regions);
        ranges.push((start, end, name.clone()));
    }
    ranges.sort_by_key(|range| range.0);

    let mut slot_end: Vec<usize> = Vec::new();
    let mut slot_repr: Vec<String> = Vec::new();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (start, end, name) in &ranges {
        let mut assigned = false;
        for (idx, slot_end_idx) in slot_end.iter_mut().enumerate() {
            if *slot_end_idx < *start {
                *slot_end_idx = *end;
                map.insert(name.clone(), slot_repr[idx].clone());
                assigned = true;
                break;
            }
        }
        if !assigned {
            slot_end.push(*end);
            slot_repr.push(name.clone());
            map.insert(name.clone(), name.clone());
        }
    }
    map
}

/// Op kinds whose function body is re-entered at arbitrary labels by a
/// resumption dispatcher; op order says nothing about execution order there.
fn is_resumable_dispatch_kind(kind: &str) -> bool {
    matches!(
        kind,
        "state_label"
            | "state_switch"
            | "state_yield"
            | "state_transition"
            | "chan_send_yield"
            | "chan_recv_yield"
    )
}

fn is_loop_opener(kind: &str) -> bool {
    matches!(
        kind,
        "loop_start" | "loop_index_start" | "for_iter_start" | "while_start" | "async_for_start"
    )
}

fn is_loop_closer(kind: &str) -> bool {
    matches!(
        kind,
        "loop_end" | "loop_index_end" | "for_iter_end" | "while_end" | "async_for_end"
    )
}

/// Op kinds that transfer control to the label whose id is in `op.value`.
fn is_label_transfer_kind(kind: &str) -> bool {
    matches!(kind, "jump" | "goto" | "br_if" | "check_exception")
}

/// Maximal disjoint op-index spans that may execute more than once: structured
/// loop bodies (opener..closer, nested ones merged into their parent) and the
/// span from a label to every later op that transfers back to it. Sorted by
/// start; no two overlap (adjacent regions stay separate: a loop that ends
/// before the next one starts never re-executes it).
fn iteration_regions(ops: &[OpIR]) -> Vec<(usize, usize)> {
    let mut regions: Vec<(usize, usize)> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut label_positions: BTreeMap<i64, usize> = BTreeMap::new();
    for (idx, op) in ops.iter().enumerate() {
        let kind = op.kind.as_str();
        if is_loop_opener(kind) {
            open.push(idx);
        } else if is_loop_closer(kind) {
            // An unmatched closer widens to the function start: better a
            // wider region than a missed iteration.
            let start = open.pop().unwrap_or(0);
            regions.push((start, idx));
        } else if kind == "label" {
            if let Some(id) = op.value {
                label_positions.insert(id, idx);
            }
        } else if is_label_transfer_kind(kind)
            && let Some(id) = op.value
            && let Some(&label_idx) = label_positions.get(&id)
            && label_idx < idx
        {
            regions.push((label_idx, idx));
        }
    }
    // Openers left open reach the end of the function.
    let last = ops.len().saturating_sub(1);
    regions.extend(open.into_iter().map(|start| (start, last)));
    merge_regions(regions)
}

fn merge_regions(mut regions: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    regions.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(regions.len());
    for (start, end) in regions {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Widen `[start, end]` to cover every iteration region it intersects,
/// repeating until the range is stable (widening can reach further regions).
fn widen_to_iteration_regions(
    mut start: usize,
    mut end: usize,
    regions: &[(usize, usize)],
) -> (usize, usize) {
    loop {
        let before = (start, end);
        for &(region_start, region_end) in regions {
            if region_start > end {
                break;
            }
            if region_end >= start {
                start = start.min(region_start);
                end = end.max(region_end);
            }
        }
        if (start, end) == before {
            return (start, end);
        }
    }
}

fn is_coalescable_local(
    name: &str,
    read_vars: &BTreeSet<String>,
    param_set: &BTreeSet<String>,
) -> bool {
    is_optimizer_temp_value_name(name) && !param_set.contains(name) && read_vars.contains(name)
}

fn is_optimizer_temp_value_name(name: &str) -> bool {
    name.starts_with("__tmp") || name.starts_with("__v")
}

#[cfg(test)]
mod tests {
    use super::super::collect_read_vars;
    use super::*;

    fn op(kind: &str, args: &[&str], out: Option<&str>) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: if args.is_empty() {
                None
            } else {
                Some(args.iter().map(|arg| arg.to_string()).collect())
            },
            out: out.map(str::to_string),
            ..OpIR::default()
        }
    }

    fn labelled(kind: &str, label: i64) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value: Some(label),
            ..OpIR::default()
        }
    }

    fn coalesce(ops: Vec<OpIR>) -> BTreeMap<String, String> {
        let func = FunctionIR {
            name: "f".to_string(),
            ops,
            ..FunctionIR::default()
        };
        let read_vars = collect_read_vars(&func.ops);
        coalesced_locals(&func, &read_vars, &BTreeSet::new())
    }

    fn shares(map: &BTreeMap<String, String>, a: &str, b: &str) -> bool {
        map.get(a).is_some() && map.get(a) == map.get(b)
    }

    #[test]
    fn straight_line_temporaries_reuse_a_dead_slot() {
        let map = coalesce(vec![
            op("const", &[], Some("__tmp1")),
            op("use", &["__tmp1"], Some("__tmp2")),
            op("use", &["__tmp2"], Some("__tmp3")),
            op("sink", &["__tmp3"], None),
        ]);
        // __tmp1 dies at op 1, so __tmp3 (born at op 2) takes its slot.
        assert!(shares(&map, "__tmp1", "__tmp3"));
        assert!(!shares(&map, "__tmp1", "__tmp2"));
    }

    #[test]
    fn a_branch_no_longer_disables_coalescing() {
        let map = coalesce(vec![
            op("const", &[], Some("__tmp1")),
            op("use", &["__tmp1"], Some("__tmp2")),
            op("if", &["__tmp2"], None),
            op("const", &[], Some("__tmp3")),
            op("sink", &["__tmp3"], None),
            op("end_if", &[], None),
            op("const", &[], Some("__tmp4")),
            op("sink", &["__tmp4"], None),
        ]);
        assert!(shares(&map, "__tmp1", "__tmp3"));
        assert!(shares(&map, "__tmp1", "__tmp4"));
    }

    #[test]
    fn a_value_read_inside_a_loop_stays_live_for_the_whole_loop() {
        let map = coalesce(vec![
            op("const", &[], Some("__tmp1")),
            op("loop_start", &[], None),
            op("use", &["__tmp1"], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
            // Born after __tmp1's last read in op order, but the next
            // iteration reads __tmp1 again: no sharing allowed.
            op("const", &[], Some("__tmp3")),
            op("sink", &["__tmp3"], None),
            op("loop_end", &[], None),
            op("const", &[], Some("__tmp4")),
            op("sink", &["__tmp4"], None),
        ]);
        assert!(!shares(&map, "__tmp1", "__tmp3"));
        assert!(!shares(&map, "__tmp1", "__tmp2"));
        // After the loop everything in it is dead: __tmp4 reuses a slot.
        assert!(shares(&map, "__tmp1", "__tmp4") || shares(&map, "__tmp2", "__tmp4"));
    }

    #[test]
    fn a_backward_jump_is_an_iteration_region() {
        let map = coalesce(vec![
            op("const", &[], Some("__tmp1")),
            labelled("label", 7),
            op("use", &["__tmp1"], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
            op("const", &[], Some("__tmp3")),
            op("sink", &["__tmp3"], None),
            labelled("br_if", 7),
            op("const", &[], Some("__tmp4")),
            op("sink", &["__tmp4"], None),
        ]);
        assert!(!shares(&map, "__tmp1", "__tmp3"));
        assert!(shares(&map, "__tmp1", "__tmp4") || shares(&map, "__tmp2", "__tmp4"));
    }

    #[test]
    fn a_forward_exception_edge_does_not_pin_a_dead_temporary() {
        let map = coalesce(vec![
            op("const", &[], Some("__tmp1")),
            op("use", &["__tmp1"], Some("__tmp2")),
            labelled("check_exception", 3),
            op("const", &[], Some("__tmp3")),
            op("sink", &["__tmp2", "__tmp3"], None),
            labelled("label", 3),
            op("const", &[], Some("__tmp4")),
            op("sink", &["__tmp4"], None),
        ]);
        assert!(shares(&map, "__tmp1", "__tmp3"));
    }

    #[test]
    fn nested_loops_widen_to_the_outer_loop() {
        let map = coalesce(vec![
            op("loop_start", &[], None),
            op("const", &[], Some("__tmp1")),
            op("loop_start", &[], None),
            op("use", &["__tmp1"], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
            op("loop_end", &[], None),
            op("const", &[], Some("__tmp3")),
            op("sink", &["__tmp3"], None),
            op("loop_end", &[], None),
        ]);
        // __tmp1 is rewritten each outer iteration before any read, but the
        // conservative region rule keeps everything inside the outer loop
        // apart.
        assert!(!shares(&map, "__tmp1", "__tmp3"));
        assert!(!shares(&map, "__tmp2", "__tmp3"));
    }

    #[test]
    fn resumable_state_machines_keep_one_local_per_temporary() {
        let map = coalesce(vec![
            op("const", &[], Some("__tmp1")),
            op("sink", &["__tmp1"], None),
            labelled("state_label", 1),
            op("const", &[], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
        ]);
        assert!(map.is_empty());
    }

    #[test]
    fn regions_merge_nested_and_adjacent_spans() {
        // Overlapping spans merge; adjacent ones stay separate iterations.
        assert_eq!(
            merge_regions(vec![(5, 9), (1, 3), (2, 4), (10, 12)]),
            vec![(1, 4), (5, 9), (10, 12)]
        );
        let regions = [(1, 4), (5, 9), (10, 12)];
        assert_eq!(widen_to_iteration_regions(6, 6, &regions), (5, 9));
        assert_eq!(widen_to_iteration_regions(0, 1, &regions), (0, 4));
        // A range spanning two regions widens to cover both.
        assert_eq!(widen_to_iteration_regions(8, 10, &regions), (5, 12));
        assert_eq!(widen_to_iteration_regions(13, 14, &regions), (13, 14));
    }
}
