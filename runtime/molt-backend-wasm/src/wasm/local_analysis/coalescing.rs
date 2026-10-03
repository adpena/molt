use crate::wasm::WasmFrameLocals;
use crate::{FunctionIR, OpIR};
use molt_tir::tir::cfg_liveness::{SimpleCfgLivenessFacts, analyze_simple_cfg_liveness_facts};
use molt_tir::tir::op_kinds_generated::{
    simpleir_kind_is_pre_ssa_rewritten, simpleir_kind_is_wasm_stateful_dispatch,
};
use molt_tir::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};

/// Dispatch frames write entry seeds here, before operation 0 executes.
const ENTRY_POSITION: u32 = 0;

/// Existing slots probed for an unoccupied gap per value. The bound keeps
/// dense frames linearithmic; a miss only costs a fresh local.
const HOLE_PROBE_LIMIT: usize = 32;

/// Operation `op_idx` reads its operands at this position and its results are
/// visible at the next one. A result occupies both: emitters may write a
/// result before they finish reading operands (sequence builders, task
/// constructors) and multi-result operations write every result at once.
fn read_position(op_idx: usize) -> u32 {
    u32::try_from(2 * op_idx + 1).expect("WASM function exceeds local storage positions")
}

fn result_position(op_idx: usize) -> u32 {
    read_position(op_idx) + 1
}

/// Physical storage for a frame's SimpleIR values: which values share a WASM
/// local, and the exact occupancy that proves each share.
#[derive(Default)]
pub(in crate::wasm) struct LocalStoragePlan {
    slots: HashMap<String, u32>,
    occupancy: Option<ValueOccupancy>,
}

impl LocalStoragePlan {
    pub(in crate::wasm) fn shared_slot(&self, name: &str) -> Option<u32> {
        self.slots.get(name).copied()
    }

    pub(in crate::wasm) fn occupancy(&self) -> Option<&ValueOccupancy> {
        self.occupancy.as_ref()
    }

    pub(in crate::wasm) fn into_occupancy(self) -> Option<ValueOccupancy> {
        self.occupancy
    }
}

/// Program positions at which each value's storage holds something a later
/// operation can observe, derived from shared CFG liveness (structured,
/// unstructured, exception and state-resume edges). Values that share a
/// local never occupy a common position, so a local holds a given value
/// exactly where that value is occupied.
pub(in crate::wasm) struct ValueOccupancy {
    ranges: HashMap<String, Vec<(u32, u32)>>,
    entry_live: BTreeSet<String>,
}

impl ValueOccupancy {
    /// Whether `name` is occupied both where `op_idx` reads and where its
    /// results appear: a value live across the operation, or one of the
    /// operation's own results. Callers asking what survives the operation
    /// exclude its results.
    pub(in crate::wasm) fn occupies_operation(&self, name: &str, op_idx: usize) -> bool {
        let (read, result) = (read_position(op_idx), result_position(op_idx));
        self.ranges.get(name).is_some_and(|ranges| {
            let after = ranges.partition_point(|&(start, _)| start <= read);
            after > 0 && ranges[after - 1].1 > result
        })
    }

    /// Whether a value can be observed before any operation writes it. Only
    /// such values need a dispatch entry seed; they all occupy the entry
    /// position and so never share a local with one another.
    pub(in crate::wasm) fn is_entry_live(&self, name: &str) -> bool {
        self.entry_live.contains(name)
    }
}

/// Plan physical locals for every SimpleIR value in `func_ir`.
///
/// Storage follows exact liveness over the shared SimpleIR CFG, so reads
/// reached through exception, backward or entry-seed paths keep their value's
/// local occupied; spelling, producer and annotation are never proof. Every
/// value the function reads other than an ABI parameter or the reserved
/// `none` input is eligible, and two values share a local only when their
/// occupied positions are disjoint.
///
/// Resumable state machines re-enter at dispatcher-selected labels with frame
/// values persisted outside WASM locals, and counted-loop index operations
/// continue past their initializer in dispatch emission. The shared CFG
/// models neither, so those frames keep one local per name.
pub(super) fn plan_local_storage(
    func_ir: &FunctionIR,
    read_vars: &BTreeSet<String>,
    param_set: &BTreeSet<String>,
) -> LocalStoragePlan {
    let ops = &func_ir.ops;
    if ops.iter().any(|op| {
        simpleir_kind_is_wasm_stateful_dispatch(op.kind.as_str())
            || simpleir_kind_is_pre_ssa_rewritten(op.kind.as_str())
    }) {
        return LocalStoragePlan::default();
    }

    let facts = analyze_simple_cfg_liveness_facts(ops);
    let ranges = occupied_ranges(ops, &facts);
    let entry_live: BTreeSet<String> = facts
        .op_to_block
        .first()
        .map(|&block| {
            facts.live_in_by_block[block]
                .iter()
                .map(|id| facts.names.name(id).to_string())
                .collect()
        })
        .unwrap_or_default();

    let mut values: Vec<u32> = (0..facts.names.len())
        .map(|id| id as u32)
        .filter(|&id| {
            let name = facts.names.name(id);
            read_vars.contains(name)
                && !param_set.contains(name)
                && name != WasmFrameLocals::NONE_NAME
                && !ranges[id as usize].is_empty()
        })
        .collect();
    values.sort_by_key(|&id| (ranges[id as usize][0].0, id));
    let mut sweep = SlotSweep::default();
    let slots = values
        .into_iter()
        .map(|id| {
            let slot = sweep.assign(&ranges[id as usize]);
            (facts.names.name(id).to_string(), slot)
        })
        .collect();
    let ranges = ranges
        .into_iter()
        .enumerate()
        .map(|(id, occupied)| (facts.names.name(id as u32).to_string(), occupied))
        .collect();
    LocalStoragePlan {
        slots,
        occupancy: Some(ValueOccupancy { ranges, entry_live }),
    }
}

/// Exact occupancy per name, ascending and merged. Each block is replayed
/// backwards from its CFG live-out set: a value is occupied from its
/// definition (or block entry) through its last observable read on any path.
fn occupied_ranges(ops: &[OpIR], facts: &SimpleCfgLivenessFacts) -> Vec<Vec<(u32, u32)>> {
    let mut reversed: Vec<Vec<(u32, u32)>> = vec![Vec::new(); facts.names.len()];
    for block in (0..facts.block_count()).rev() {
        let (start, end) = facts.block_ops[block];
        let (block_from, block_to) = (read_position(start), read_position(end));
        for id in facts.live_out_by_block[block].iter() {
            add_range(&mut reversed[id as usize], block_from, block_to);
        }
        facts.visit_block_backward(ops, block, |op_idx, live_after| {
            let (read, result) = (read_position(op_idx), result_position(op_idx));
            visit_simple_ir_defined_names(&ops[op_idx], |name| {
                let Some(id) = facts.names.id(name) else {
                    return;
                };
                let ranges = &mut reversed[id as usize];
                if live_after.contains(id) {
                    let open = ranges
                        .last_mut()
                        .expect("a live result has an open occupancy range");
                    debug_assert!(open.0 <= result && result < open.1);
                    open.0 = read;
                } else {
                    // A dead write still stores into its physical local.
                    add_range(ranges, read, result + 1);
                }
            });
            visit_simple_ir_reads(&ops[op_idx], |source| {
                if let Some(id) = facts.names.id(source.name) {
                    add_range(&mut reversed[id as usize], block_from, read + 1);
                }
            });
        });
    }
    for ranges in &mut reversed {
        ranges.reverse();
    }
    if let Some(&entry) = facts.op_to_block.first() {
        for id in facts.live_in_by_block[entry].iter() {
            if let Some(first) = reversed[id as usize].first_mut() {
                first.0 = ENTRY_POSITION;
            }
        }
    }
    reversed
}

/// Ranges are built backwards, so a new range starts no later than the
/// previous one. Overlapping or adjacent ranges merge.
fn add_range(ranges: &mut Vec<(u32, u32)>, start: u32, end: u32) {
    if let Some(last) = ranges.last_mut()
        && start <= last.1
        && last.0 <= end
    {
        last.0 = last.0.min(start);
        last.1 = last.1.max(end);
        return;
    }
    ranges.push((start, end));
}

/// Deterministic slot assignment for values visited by first occupied
/// position: each value takes the lowest slot that is fully free, or whose
/// unoccupied gaps cover every one of the value's ranges.
#[derive(Default)]
struct SlotSweep {
    occupied: Vec<BTreeMap<u32, u32>>,
    last_end: Vec<u32>,
    covered: Vec<bool>,
    holes: BTreeSet<u32>,
    free: BinaryHeap<Reverse<u32>>,
    events: BinaryHeap<Reverse<(u32, bool, u32)>>,
}

impl SlotSweep {
    fn assign(&mut self, ranges: &[(u32, u32)]) -> u32 {
        let start = ranges[0].0;
        self.advance(start);
        let free = self.lowest_free(start);
        let hole = self
            .holes
            .range(..free.unwrap_or(u32::MAX))
            .take(HOLE_PROBE_LIMIT)
            .copied()
            .find(|&slot| fits(&self.occupied[slot as usize], ranges));
        let slot = if let Some(slot) = hole {
            self.holes.remove(&slot);
            slot
        } else if let Some(slot) = free {
            self.free.pop();
            slot
        } else {
            self.open_slot()
        };
        self.occupy(slot, ranges);
        slot
    }

    /// Apply every range boundary at or before `position`. Ranges are
    /// half-open, so an end sorts before a start at the same position.
    fn advance(&mut self, position: u32) {
        while let Some(&Reverse((at, starts, slot))) = self.events.peek() {
            if at > position {
                break;
            }
            self.events.pop();
            let index = slot as usize;
            self.covered[index] = starts;
            if starts {
                self.holes.remove(&slot);
            } else if self.last_end[index] <= position {
                self.holes.remove(&slot);
                self.free.push(Reverse(slot));
            } else {
                self.holes.insert(slot);
            }
        }
    }

    fn lowest_free(&mut self, position: u32) -> Option<u32> {
        while let Some(&Reverse(slot)) = self.free.peek() {
            let index = slot as usize;
            if !self.covered[index] && self.last_end[index] <= position {
                return Some(slot);
            }
            // Stale: the slot was reused after this entry was pushed.
            self.free.pop();
        }
        None
    }

    fn open_slot(&mut self) -> u32 {
        let slot = u32::try_from(self.occupied.len()).expect("WASM frame exceeds u32 locals");
        self.occupied.push(BTreeMap::new());
        self.last_end.push(0);
        self.covered.push(false);
        slot
    }

    fn occupy(&mut self, slot: u32, ranges: &[(u32, u32)]) {
        let index = slot as usize;
        for &(start, end) in ranges {
            self.occupied[index].insert(start, end);
            self.events.push(Reverse((start, true, slot)));
            self.events.push(Reverse((end, false, slot)));
        }
        let end = ranges.last().map_or(0, |&(_, end)| end);
        self.last_end[index] = self.last_end[index].max(end);
    }
}

fn fits(occupied: &BTreeMap<u32, u32>, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().all(|&(start, end)| {
        occupied
            .range(..end)
            .next_back()
            .is_none_or(|(_, &occupied_end)| occupied_end <= start)
    })
}

#[cfg(test)]
mod tests {
    use super::super::collect_value_names;
    use super::*;
    use molt_tir::tir::cfg::CFG;
    use molt_tir::tir::cfg_liveness::liveness_block_successors;

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

    fn bind(source: &str, destination: &str) -> OpIR {
        let mut store = op("store_var", &[source], None);
        store.var = Some(destination.to_string());
        store
    }

    fn plan_with_params(ops: Vec<OpIR>, params: &[&str]) -> (FunctionIR, LocalStoragePlan) {
        let func = FunctionIR {
            name: "f".to_string(),
            params: params.iter().map(|param| param.to_string()).collect(),
            ops,
            ..FunctionIR::default()
        };
        let (read_vars, _) = collect_value_names(&func.ops);
        let param_set = func.params.iter().cloned().collect();
        let storage = plan_local_storage(&func, &read_vars, &param_set);
        (func, storage)
    }

    fn plan(ops: Vec<OpIR>) -> LocalStoragePlan {
        plan_with_params(ops, &[]).1
    }

    fn shares(storage: &LocalStoragePlan, a: &str, b: &str) -> bool {
        storage.shared_slot(a).is_some() && storage.shared_slot(a) == storage.shared_slot(b)
    }

    #[test]
    fn values_share_by_liveness_regardless_of_spelling() {
        let storage = plan(vec![
            op("const", &[], Some("v1")),
            op("use", &["v1"], Some("_v2")),
            op("use", &["_v2"], Some("slot_x")),
            op("sink", &["slot_x"], None),
            op("const", &[], Some("_bb3_arg0")),
            op("sink", &["_bb3_arg0"], None),
        ]);
        // No spelling is admitted or excluded: every read value is planned.
        for name in ["v1", "_v2", "slot_x", "_bb3_arg0"] {
            assert!(storage.shared_slot(name).is_some(), "{name}");
        }
        assert!(shares(&storage, "v1", "slot_x"));
        assert!(shares(&storage, "v1", "_bb3_arg0"));
        assert!(
            !shares(&storage, "v1", "_v2"),
            "a result never reuses an operand of its own operation"
        );
        assert!(!shares(&storage, "_v2", "slot_x"));
    }

    #[test]
    fn sibling_branch_values_share_one_local() {
        let storage = plan(vec![
            op("const", &[], Some("cond")),
            op("if", &["cond"], None),
            op("const", &[], Some("then_value")),
            op("sink", &["then_value"], None),
            op("else", &[], None),
            op("const", &[], Some("else_value")),
            op("sink", &["else_value"], None),
            op("end_if", &[], None),
            op("const", &[], Some("after")),
            op("sink", &["after"], None),
        ]);
        assert!(shares(&storage, "then_value", "else_value"));
        assert!(shares(&storage, "cond", "after"));
    }

    #[test]
    fn a_value_live_around_a_backward_edge_keeps_its_local_for_the_loop() {
        let mut again = op("br_if", &["cond"], None);
        again.value = Some(7);
        let storage = plan(vec![
            op("const", &[], Some("carried")),
            labelled("label", 7),
            op("sink", &["carried"], None),
            op("const", &[], Some("iteration")),
            op("sink", &["iteration"], None),
            op("const", &[], Some("cond")),
            again,
            op("const", &[], Some("after")),
            op("sink", &["after"], None),
        ]);
        // The next iteration reads `carried` again before any write.
        assert!(!shares(&storage, "carried", "iteration"));
        assert!(!shares(&storage, "carried", "cond"));
        assert!(shares(&storage, "carried", "after"));
    }

    #[test]
    fn a_value_read_inside_a_structured_loop_stays_live_for_the_whole_loop() {
        let storage = plan(vec![
            op("const", &[], Some("__tmp1")),
            op("loop_start", &[], None),
            op("use", &["__tmp1"], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
            // Born after __tmp1's last read in op order, but the next
            // iteration reads __tmp1 again.
            op("const", &[], Some("__tmp3")),
            op("sink", &["__tmp3"], None),
            op("loop_end", &[], None),
            op("const", &[], Some("__tmp4")),
            op("sink", &["__tmp4"], None),
        ]);
        assert!(!shares(&storage, "__tmp1", "__tmp3"));
        assert!(!shares(&storage, "__tmp1", "__tmp2"));
        assert!(shares(&storage, "__tmp1", "__tmp4") || shares(&storage, "__tmp2", "__tmp4"));
    }

    #[test]
    fn values_rewritten_before_every_read_share_inside_a_loop() {
        let mut again = op("br_if", &["cond"], None);
        again.value = Some(7);
        let storage = plan_with_params(
            vec![
                labelled("label", 7),
                op("const", &[], Some("first")),
                op("sink", &["first"], None),
                op("const", &[], Some("second")),
                op("sink", &["second"], None),
                again,
                op("ret", &["cond"], None),
            ],
            &["cond"],
        )
        .1;
        // Neither value is live around the backward edge; widening every
        // range to the loop would needlessly keep them apart.
        assert!(shares(&storage, "first", "second"));
    }

    #[test]
    fn exception_handler_values_free_their_local_after_the_last_transfer() {
        let storage = plan_with_params(
            vec![
                bind("first", "held"),
                labelled("check_exception", 9),
                bind("second", "early"),
                bind("early", "early_sink"),
                labelled("check_exception", 9),
                bind("second", "late"),
                op("ret", &["late"], None),
                labelled("label", 9),
                op("ret", &["held"], None),
            ],
            &["first", "second"],
        )
        .1;
        assert!(
            !shares(&storage, "held", "early"),
            "the second transfer can still observe held"
        );
        assert!(
            shares(&storage, "held", "late"),
            "no transfer to the handler follows, so held's local is free"
        );
    }

    #[test]
    fn a_read_before_every_definition_keeps_the_entry_seed_local() {
        let storage = plan_with_params(
            vec![
                labelled("jump", 1),
                labelled("label", 3),
                op("ret", &["seeded"], None),
                labelled("label", 1),
                bind("first", "scratch"),
                bind("scratch", "sink"),
                labelled("jump", 3),
                op("const", &[], Some("seeded")),
                op("ret", &["sink"], None),
            ],
            &["first"],
        )
        .1;
        let occupancy = storage
            .occupancy()
            .expect("jumpful frames plan exact storage");
        assert!(occupancy.is_entry_live("seeded"));
        assert!(!occupancy.is_entry_live("scratch"));
        // First-write intervals would place `scratch` in `seeded`'s local and
        // overwrite the entry seed before the jump back reads it.
        assert!(!shares(&storage, "seeded", "scratch"));
    }

    #[test]
    fn every_backward_transfer_keeps_a_value_read_around_it_apart() {
        // The exception transfers target the label leading their own block.
        for kind in [
            "jump",
            "goto",
            "br_if",
            "check_exception",
            "async_work_poll",
        ] {
            let storage = plan(vec![
                op("const", &[], Some("__tmp1")),
                labelled("label", 7),
                op("sink", &["__tmp1"], None),
                op("const", &[], Some("__tmp2")),
                op("sink", &["__tmp2"], None),
                labelled(kind, 7),
                op("const", &[], Some("__tmp3")),
                op("sink", &["__tmp3"], None),
            ]);
            assert!(!shares(&storage, "__tmp1", "__tmp2"), "{kind}");
            assert!(shares(&storage, "__tmp1", "__tmp3"), "{kind}");
        }
    }

    #[test]
    fn a_forward_exception_edge_does_not_pin_a_dead_value() {
        let storage = plan(vec![
            op("const", &[], Some("__tmp1")),
            op("use", &["__tmp1"], Some("__tmp2")),
            labelled("check_exception", 3),
            op("const", &[], Some("__tmp3")),
            op("sink", &["__tmp2", "__tmp3"], None),
            labelled("label", 3),
            op("const", &[], Some("__tmp4")),
            op("sink", &["__tmp4"], None),
        ]);
        assert!(shares(&storage, "__tmp1", "__tmp3"));
        assert!(!shares(&storage, "__tmp2", "__tmp3"));
    }

    #[test]
    fn late_binding_writes_and_secondary_results_keep_their_physical_slots() {
        let mut late_write = op("store_var", &["source"], None);
        late_write.var = Some("__tmp1".into());
        let storage = plan_with_params(
            vec![
                op("const", &[], Some("__tmp1")),
                op("sink", &["__tmp1"], None),
                op("unpack_sequence", &["sequence", "__tmp2", "__tmp3"], None),
                late_write,
                op("sink", &["__tmp2", "__tmp3"], None),
            ],
            &["sequence", "source"],
        )
        .1;
        assert!(!shares(&storage, "__tmp1", "__tmp2"));
        assert!(!shares(&storage, "__tmp2", "__tmp3"));
        assert!(storage.shared_slot("__tmp2").is_some() && storage.shared_slot("__tmp3").is_some());
    }

    #[test]
    fn resumable_state_machines_keep_one_local_per_name() {
        let storage = plan(vec![
            op("state_switch", &[], None),
            op("const", &[], Some("__tmp1")),
            op("sink", &["__tmp1"], None),
            labelled("state_label", 1),
            op("const", &[], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
        ]);
        assert_eq!(storage.shared_slot("__tmp1"), None);
        assert_eq!(storage.shared_slot("__tmp2"), None);
        assert!(storage.occupancy().is_none());
    }

    #[test]
    fn counted_loop_index_frames_keep_one_local_per_name() {
        let storage = plan_with_params(
            vec![
                op("loop_start", &[], None),
                op("loop_index_start", &["start"], Some("index")),
                op("sink", &["index"], None),
                op("loop_continue", &[], None),
                op("loop_end", &[], None),
            ],
            &["start"],
        )
        .1;
        assert_eq!(storage.shared_slot("index"), None);
        assert!(storage.occupancy().is_none());
    }

    #[test]
    fn label_only_functions_use_the_nonstateful_frame_contract() {
        let storage = plan(vec![
            labelled("state_label", 1),
            op("const", &[], Some("__tmp1")),
            op("sink", &["__tmp1"], None),
            op("const", &[], Some("__tmp2")),
            op("sink", &["__tmp2"], None),
        ]);
        assert!(shares(&storage, "__tmp1", "__tmp2"));
    }

    #[test]
    fn parameters_and_the_none_input_keep_their_reserved_locals() {
        let storage = plan_with_params(
            vec![
                op("use", &["param", "none"], Some("value")),
                op("use", &["param", "value"], Some("result")),
                op("ret", &["result"], None),
            ],
            &["param"],
        )
        .1;
        assert_eq!(storage.shared_slot("param"), None);
        assert_eq!(storage.shared_slot(WasmFrameLocals::NONE_NAME), None);
        assert!(storage.shared_slot("value").is_some());
        let occupancy = storage
            .occupancy()
            .expect("plain frames plan exact storage");
        assert!(occupancy.is_entry_live("param"));
        assert!(
            occupancy.occupies_operation("param", 0),
            "read again by the next operation"
        );
        assert!(!occupancy.occupies_operation("param", 1), "last read");
        assert!(
            occupancy.occupies_operation("result", 1),
            "an operation's own result"
        );
    }

    #[test]
    fn high_pressure_then_expiry_reuses_the_lowest_slot_deterministically() {
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
        let storage = plan(ops);
        let slots: BTreeSet<_> = names.iter().map(|name| storage.shared_slot(name)).collect();
        assert_eq!(slots.len(), names.len(), "simultaneously live values");
        assert_eq!(storage.shared_slot(&names[0]), Some(0));
        assert_eq!(storage.shared_slot("__tmpnext"), Some(0));
    }

    #[test]
    fn generated_control_flow_never_shares_a_slot_between_observable_values() {
        for seed in 0..512 {
            let (func, storage) = plan_with_params(generated_ops(seed), &["p"]);
            let ops = &func.ops;
            let occupancy = storage
                .occupancy()
                .expect("generated frames are not resumable");
            let live_after = exhaustive_live_after(ops);
            let names = names_of(ops);
            let entry = live_before(&ops[0], &live_after[0]);
            for name in &names {
                assert_eq!(
                    occupancy.is_entry_live(name),
                    entry.contains(name),
                    "seed {seed}, {name}: {ops:?}"
                );
            }
            let mut occupied_positions = vec![entry];
            for (op_idx, op) in ops.iter().enumerate() {
                let results = defined_names(op);
                let mut at_read = live_before(op, &live_after[op_idx]);
                at_read.extend(results.iter().cloned());
                let mut at_result = live_after[op_idx].clone();
                at_result.extend(results.iter().cloned());
                for name in &names {
                    assert_eq!(
                        occupancy.occupies_operation(name, op_idx),
                        at_read.contains(name) && at_result.contains(name),
                        "seed {seed}, op {op_idx}, {name}: {ops:?}"
                    );
                }
                occupied_positions.push(at_read);
                occupied_positions.push(at_result);
            }
            for occupied in &occupied_positions {
                let mut holders = BTreeMap::new();
                for name in occupied {
                    if let Some(slot) = storage.shared_slot(name)
                        && let Some(other) = holders.insert(slot, name)
                    {
                        panic!("seed {seed}: {other} and {name} both occupy slot {slot}: {ops:?}");
                    }
                }
            }
        }
    }

    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound
        }
    }

    const GENERATED_NAMES: [&str; 5] = ["p", "a", "b", "c", "d"];

    fn pick(rng: &mut Lcg) -> String {
        GENERATED_NAMES[rng.below(GENERATED_NAMES.len() as u64) as usize].to_string()
    }

    fn generated_op(rng: &mut Lcg) -> OpIR {
        let label = 1 + rng.below(3) as i64;
        let mut generated = OpIR::default();
        match rng.below(11) {
            0 | 1 => {
                generated.kind = "add".into();
                generated.args = Some(vec![pick(rng), pick(rng)]);
                generated.out = Some(pick(rng));
            }
            2 => {
                generated.kind = "store_var".into();
                generated.args = Some(vec![pick(rng)]);
                generated.var = Some(pick(rng));
            }
            3 => {
                generated.kind = "unpack_sequence".into();
                generated.args = Some(vec![pick(rng), pick(rng), pick(rng)]);
            }
            4 => {
                generated.kind = "iter_next_unboxed".into();
                generated.args = Some(vec![pick(rng)]);
                generated.var = Some(pick(rng));
                generated.out = Some(pick(rng));
            }
            5 => {
                generated.kind = "jump".into();
                generated.value = Some(label);
            }
            6 => {
                generated.kind = "br_if".into();
                generated.args = Some(vec![pick(rng)]);
                generated.value = Some(label);
            }
            7 => {
                generated.kind = "check_exception".into();
                generated.value = Some(label);
            }
            8 => {
                generated.kind = "ret".into();
                generated.args = Some(vec![pick(rng)]);
            }
            9 => {
                generated.kind = "print".into();
                generated.args = Some(vec![pick(rng)]);
            }
            _ => {
                generated.kind = "const".into();
                generated.out = Some(pick(rng));
            }
        }
        generated
    }

    /// Unstructured bodies with forward, backward and exception transfers,
    /// dead code, multi-result definitions, rebound parameters, and reads that
    /// precede every definition.
    fn generated_ops(seed: u64) -> Vec<OpIR> {
        let mut rng = Lcg(seed);
        let count = 6 + rng.below(20);
        let mut ops: Vec<OpIR> = (0..count).map(|_| generated_op(&mut rng)).collect();
        for label in 1..=3 {
            let at = rng.below(ops.len() as u64 + 1) as usize;
            ops.insert(at, labelled("label", label));
        }
        ops.push(op("ret_void", &[], None));
        ops
    }

    fn names_of(ops: &[OpIR]) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for op in ops {
            visit_simple_ir_reads(op, |read| {
                names.insert(read.name.to_string());
            });
            names.extend(defined_names(op));
        }
        names
    }

    fn defined_names(op: &OpIR) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        visit_simple_ir_defined_names(op, |name| {
            names.insert(name.to_string());
        });
        names
    }

    fn live_before(op: &OpIR, live_after: &BTreeSet<String>) -> BTreeSet<String> {
        let mut live = live_after.clone();
        for name in defined_names(op) {
            live.remove(&name);
        }
        visit_simple_ir_reads(op, |read| {
            live.insert(read.name.to_string());
        });
        live
    }

    /// Path semantics over the operation-level successor relation, without
    /// the block dataflow solver: a name is live after an operation when some
    /// successor path reads it before any redefinition.
    fn exhaustive_live_after(ops: &[OpIR]) -> Vec<BTreeSet<String>> {
        let cfg = CFG::build(ops);
        let block_successors = liveness_block_successors(&cfg);
        let mut successors = vec![Vec::new(); ops.len()];
        for block in &cfg.blocks {
            if block.start_op == block.end_op {
                continue; // Invocation has no source operation.
            }
            let tail = block.end_op - 1;
            for op_idx in block.start_op..tail {
                successors[op_idx].push(op_idx + 1);
            }
            for &successor in &block_successors[block.id] {
                successors[tail].push(cfg.blocks[successor].start_op);
            }
        }
        let names = names_of(ops);
        (0..ops.len())
            .map(|op_idx| {
                names
                    .iter()
                    .filter(|name| {
                        let mut pending = successors[op_idx].clone();
                        let mut visited = BTreeSet::new();
                        while let Some(next) = pending.pop() {
                            if !visited.insert(next) {
                                continue;
                            }
                            let mut reads = false;
                            visit_simple_ir_reads(&ops[next], |read| {
                                reads |= read.name == name.as_str();
                            });
                            if reads {
                                return true;
                            }
                            if !defined_names(&ops[next]).contains(name.as_str()) {
                                pending.extend(&successors[next]);
                            }
                        }
                        false
                    })
                    .cloned()
                    .collect()
            })
            .collect()
    }
}
