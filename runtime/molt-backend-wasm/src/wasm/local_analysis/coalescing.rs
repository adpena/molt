use crate::{FunctionIR, OpIR};
use molt_tir::tir::op_kinds_generated::{
    SimpleIrVerifierRegionRole, simpleir_kind_is_conditional_branch,
    simpleir_kind_is_exception_check, simpleir_kind_is_terminator,
    simpleir_kind_is_verifier_label_definition, simpleir_kind_is_verifier_label_reference,
    simpleir_kind_is_wasm_stateful_dispatch, simpleir_verifier_region_role,
};
use molt_tir::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// Map every coalescable optimizer temporary to the temporary whose wasm local
/// it shares (a temporary maps to itself when it owns the slot).
///
/// Slots are assigned by a linear scan over live ranges in op order. A live
/// range is `[first write, last access]` in op order, widened so that sharing a
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
/// * Resumable state machines, classified by the generated frame contract,
///   re-enter the body at arbitrary
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
        .any(|op| simpleir_kind_is_wasm_stateful_dispatch(op.kind.as_str()))
    {
        return BTreeMap::new();
    }

    let mut first_write: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_access: BTreeMap<String, usize> = BTreeMap::new();
    for (op_idx, op) in func_ir.ops.iter().enumerate() {
        visit_simple_ir_defined_names(op, |name| {
            first_write.entry(name.to_string()).or_insert(op_idx);
            // A later write still touches the physical slot even when its
            // value is dead; it cannot overwrite a new occupant after reuse.
            last_access.insert(name.to_string(), op_idx);
        });
        visit_simple_ir_reads(op, |read| {
            last_access.insert(read.name.to_string(), op_idx);
        });
    }

    let regions = iteration_regions(ops);

    let mut ranges: Vec<(usize, usize, String)> = Vec::new();
    for (name, start) in &first_write {
        if !is_coalescable_local(name, read_vars, param_set) {
            continue;
        }
        let end = last_access.get(name).copied().unwrap_or(*start);
        let (start, end) = widen_to_iteration_regions(*start, end.max(*start), &regions);
        ranges.push((start, end, name.clone()));
    }
    ranges.sort_by_key(|range| range.0);

    // Expire slots by last access, then choose the lowest free slot. This
    // preserves deterministic first-free allocation without scanning every
    // live slot for every temporary (quadratic at high register pressure).
    let mut active: BinaryHeap<Reverse<(usize, usize)>> = BinaryHeap::new();
    let mut free: BinaryHeap<Reverse<usize>> = BinaryHeap::new();
    let mut slot_repr: Vec<String> = Vec::new();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (start, end, name) in &ranges {
        while active.peek().is_some_and(|Reverse((last, _))| last < start) {
            let Reverse((_, slot)) = active.pop().unwrap();
            free.push(Reverse(slot));
        }
        let slot = if let Some(Reverse(slot)) = free.pop() {
            slot
        } else {
            slot_repr.push(name.clone());
            slot_repr.len() - 1
        };
        active.push(Reverse((*end, slot)));
        map.insert(name.clone(), slot_repr[slot].clone());
    }
    map
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
        if simpleir_verifier_region_role(kind) == Some(("loop", SimpleIrVerifierRegionRole::Start))
        {
            open.push(idx);
        } else if simpleir_verifier_region_role(kind)
            == Some(("loop", SimpleIrVerifierRegionRole::End))
        {
            // An unmatched closer widens to the function start: better a
            // wider region than a missed iteration.
            let start = open.pop().unwrap_or(0);
            regions.push((start, idx));
        } else if simpleir_kind_is_verifier_label_definition(kind) {
            if let Some(id) = op.value {
                label_positions.insert(id, idx);
            }
        } else if simpleir_kind_is_verifier_label_reference(kind)
            && (simpleir_kind_is_terminator(kind)
                || simpleir_kind_is_conditional_branch(kind)
                || simpleir_kind_is_exception_check(kind))
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

/// The regions are sorted and disjoint, so widening to the first/last
/// intersected endpoints cannot reach another region. Two binary searches
/// replace a repeated scan: O(log R) per temporary instead of O(R).
fn widen_to_iteration_regions(
    start: usize,
    end: usize,
    regions: &[(usize, usize)],
) -> (usize, usize) {
    let first = regions.partition_point(|&(_, region_end)| region_end < start);
    let after_last = regions.partition_point(|&(region_start, _)| region_start <= end);
    if first < after_last {
        (
            start.min(regions[first].0),
            end.max(regions[after_last - 1].1),
        )
    } else {
        (start, end)
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
    use super::super::collect_value_names;
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
        let (read_vars, _) = collect_value_names(&func.ops);
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
            op("state_switch", &[], None),
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

    #[test]
    fn every_direct_backward_transfer_keeps_iteration_values_apart() {
        for kind in [
            "jump",
            "goto",
            "br_if",
            "check_exception",
            "async_work_poll",
        ] {
            let map = coalesce(vec![
                op("const", &[], Some("__tmp1")),
                labelled("label", 7),
                op("sink", &["__tmp1"], None),
                op("const", &[], Some("__tmp2")),
                op("sink", &["__tmp2"], None),
                labelled(kind, 7),
                op("const", &[], Some("__tmp3")),
                op("sink", &["__tmp3"], None),
            ]);
            assert!(!shares(&map, "__tmp1", "__tmp2"), "{kind}");
            assert!(shares(&map, "__tmp1", "__tmp3"), "{kind}");
        }
    }

    #[test]
    fn counted_marker_is_not_a_second_loop_and_region_markers_are_not_transfers() {
        let ops = vec![
            labelled("label", 7),
            op("loop_start", &[], None),
            op("loop_index_start", &[], None),
            op("const", &[], Some("__tmp1")),
            op("sink", &["__tmp1"], None),
            op("loop_end", &[], None),
            labelled("try_start", 7),
            labelled("try_end", 7),
            op("const", &[], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
        ];
        assert_eq!(iteration_regions(&ops), vec![(1, 5)]);
        assert!(shares(&coalesce(ops), "__tmp1", "__tmp2"));
    }

    #[test]
    fn label_only_functions_use_the_same_nonstateful_frame_contract() {
        let map = coalesce(vec![
            labelled("state_label", 1),
            op("const", &[], Some("__tmp1")),
            op("sink", &["__tmp1"], None),
            op("const", &[], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
        ]);
        assert!(shares(&map, "__tmp1", "__tmp2"));
    }

    #[test]
    fn late_binding_writes_and_secondary_results_keep_their_physical_slots() {
        let mut late_write = op("store_var", &["source"], None);
        late_write.var = Some("__tmp1".into());
        let map = coalesce(vec![
            op("const", &[], Some("__tmp1")),
            op("sink", &["__tmp1"], None),
            op("unpack_sequence", &["sequence", "__tmp2", "__tmp3"], None),
            late_write,
            op("sink", &["__tmp2", "__tmp3"], None),
        ]);
        assert!(!shares(&map, "__tmp1", "__tmp2"));
        assert!(!shares(&map, "__tmp2", "__tmp3"));
        assert!(map.contains_key("__tmp2") && map.contains_key("__tmp3"));
    }

    #[test]
    fn binary_region_lookup_matches_linear_intersection_oracle() {
        let regions = merge_regions(vec![(2, 5), (4, 8), (11, 12), (12, 15), (18, 20)]);
        for start in 0..24 {
            for end in start..24 {
                let mut expected = (start, end);
                for &(lo, hi) in &regions {
                    if lo <= end && hi >= start {
                        expected.0 = expected.0.min(lo);
                        expected.1 = expected.1.max(hi);
                    }
                }
                assert_eq!(widen_to_iteration_regions(start, end, &regions), expected);
            }
        }
    }

    #[test]
    fn high_pressure_then_expiry_reuses_lowest_slot_deterministically() {
        let names: Vec<_> = (0..20_000)
            .map(|index| format!("__tmp{index:05}"))
            .collect();
        let mut ops: Vec<_> = names
            .iter()
            .map(|name| op("const", &[], Some(name)))
            .collect();
        ops.push(op(
            "sink",
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
            None,
        ));
        ops.push(op("const", &[], Some("__tmpnext")));
        ops.push(op("sink", &["__tmpnext"], None));
        let map = coalesce(ops);
        for name in &names {
            assert_eq!(map.get(name), Some(name));
        }
        assert_eq!(map.get("__tmpnext"), Some(&names[0]));
    }
}
