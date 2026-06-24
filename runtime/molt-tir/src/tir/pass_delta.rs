//! Machine-readable per-pass TIR fact deltas.
//!
//! This module is a diagnostic observer only. It samples a compact fact profile
//! before and after each pass when `MOLT_EMIT_PASS_DELTA=1` is set, then writes
//! one JSONL record per pass. Product IR, pass order, analysis invalidation, and
//! verification behavior are unchanged when the observer is disabled or enabled.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use super::function::TirFunction;
use super::op_kinds_generated::opcode_pass_delta_facts_table;
use super::ops::TirOp;
use super::pass_manager::Mutates;
use super::passes::PassStats;
use super::target_info::TargetInfo;
use super::types::TirType;
use super::values::ValueId;
use crate::debug_artifacts;
use crate::representation_plan::Repr;

pub(crate) const PASS_DELTA_SCHEMA_VERSION: u32 = 1;

pub(crate) fn emit_enabled() -> bool {
    std::env::var("MOLT_EMIT_PASS_DELTA").as_deref() == Ok("1")
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FactProfile {
    pub blocks: usize,
    pub ops: usize,
    pub typed_values: usize,
    pub repr_counts: BTreeMap<String, usize>,
    pub op_counts: BTreeMap<String, usize>,
    pub box_ops: usize,
    pub unbox_ops: usize,
    pub generic_calls: usize,
    pub direct_calls: usize,
    pub method_calls: usize,
    pub runtime_helper_calls: usize,
    pub call_results_typed_repr: usize,
    pub call_results_dynbox: usize,
    pub rc_events: usize,
    pub inc_ref_ops: usize,
    pub dec_ref_ops: usize,
    pub del_boundary_ops: usize,
    pub exception_events: usize,
    pub type_guard_ops: usize,
    pub heap_alloc_ops: usize,
}

impl FactProfile {
    pub(crate) fn capture(func: &TirFunction) -> Self {
        let mut profile = FactProfile {
            blocks: func.blocks.len(),
            ops: 0,
            typed_values: 0,
            repr_counts: BTreeMap::new(),
            op_counts: BTreeMap::new(),
            box_ops: 0,
            unbox_ops: 0,
            generic_calls: 0,
            direct_calls: 0,
            method_calls: 0,
            runtime_helper_calls: 0,
            call_results_typed_repr: 0,
            call_results_dynbox: 0,
            rc_events: 0,
            inc_ref_ops: 0,
            dec_ref_ops: 0,
            del_boundary_ops: 0,
            exception_events: 0,
            type_guard_ops: 0,
            heap_alloc_ops: 0,
        };

        let mut seen_values = BTreeSet::new();
        for (&id, ty) in &func.value_types {
            profile.note_value(id, ty, &mut seen_values);
        }

        let mut block_ids: Vec<_> = func.blocks.keys().copied().collect();
        block_ids.sort_unstable();
        for block_id in block_ids {
            let Some(block) = func.blocks.get(&block_id) else {
                continue;
            };
            for arg in &block.args {
                profile.note_value(arg.id, &arg.ty, &mut seen_values);
            }
            for op in &block.ops {
                profile.note_op(func, op);
            }
        }

        profile
    }

    fn note_value(&mut self, id: ValueId, ty: &TirType, seen_values: &mut BTreeSet<ValueId>) {
        if !seen_values.insert(id) {
            return;
        }
        self.typed_values += 1;
        let repr = format!("{:?}", Repr::default_for(ty));
        *self.repr_counts.entry(repr).or_default() += 1;
    }

    fn note_op(&mut self, func: &TirFunction, op: &TirOp) {
        self.ops += 1;
        *self
            .op_counts
            .entry(format!("{:?}", op.opcode))
            .or_default() += 1;

        let facts = opcode_pass_delta_facts_table(op.opcode);
        if facts.box_op {
            self.box_ops += 1;
        }
        if facts.unbox_op {
            self.unbox_ops += 1;
        }
        if facts.generic_call {
            self.generic_calls += 1;
            self.note_call_results(func, op);
        }
        if facts.direct_call {
            self.direct_calls += 1;
        }
        if facts.method_call {
            self.method_calls += 1;
        }
        if facts.runtime_helper_call {
            self.runtime_helper_calls += 1;
        }
        if facts.rc_event {
            self.rc_events += 1;
        }
        if facts.inc_ref {
            self.inc_ref_ops += 1;
        }
        if facts.dec_ref {
            self.dec_ref_ops += 1;
        }
        if facts.del_boundary {
            self.del_boundary_ops += 1;
        }
        if facts.exception_event {
            self.exception_events += 1;
        }
        if facts.type_guard {
            self.type_guard_ops += 1;
        }
        if facts.heap_alloc {
            self.heap_alloc_ops += 1;
        }
    }

    fn note_call_results(&mut self, func: &TirFunction, op: &TirOp) {
        for result in &op.results {
            let repr = func
                .value_types
                .get(result)
                .map(Repr::default_for)
                .unwrap_or(Repr::DynBox);
            if repr == Repr::DynBox {
                self.call_results_dynbox += 1;
            } else {
                self.call_results_typed_repr += 1;
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct PassStatsRecord<'a> {
    name: &'a str,
    values_changed: usize,
    attrs_changed: usize,
    ops_removed: usize,
    ops_added: usize,
    facts_changed: usize,
    total_changes: usize,
}

impl<'a> PassStatsRecord<'a> {
    fn from_stats(stats: &'a PassStats) -> Self {
        Self {
            name: stats.name,
            values_changed: stats.values_changed,
            attrs_changed: stats.attrs_changed,
            ops_removed: stats.ops_removed,
            ops_added: stats.ops_added,
            facts_changed: stats.facts_changed,
            total_changes: stats.total_changes(),
        }
    }
}

#[derive(Debug, Serialize)]
struct HostRecord {
    os: &'static str,
    arch: &'static str,
    family: &'static str,
    pointer_width: usize,
}

impl HostRecord {
    fn current() -> Self {
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            family: std::env::consts::FAMILY,
            pointer_width: std::mem::size_of::<usize>() * 8,
        }
    }
}

#[derive(Debug, Serialize)]
struct TargetRecord {
    target: String,
    profile: String,
}

impl TargetRecord {
    fn from_target_info(tti: &TargetInfo) -> Self {
        Self {
            target: format!("{:?}", tti.target),
            profile: format!("{:?}", tti.profile),
        }
    }
}

#[derive(Debug, Serialize)]
struct FactDelta {
    blocks_delta: isize,
    ops_delta: isize,
    typed_values_delta: isize,
    repr_delta: BTreeMap<String, isize>,
    lost_repr_values: BTreeMap<String, usize>,
    gained_repr_values: BTreeMap<String, usize>,
    op_delta: BTreeMap<String, isize>,
    added_box_ops: usize,
    removed_box_ops: usize,
    added_unbox_ops: usize,
    removed_unbox_ops: usize,
    added_generic_calls: usize,
    removed_generic_calls: usize,
    added_runtime_helper_calls: usize,
    removed_runtime_helper_calls: usize,
    added_rc_events: usize,
    removed_rc_events: usize,
    added_exception_events: usize,
    removed_exception_events: usize,
    added_type_guard_ops: usize,
    removed_type_guard_ops: usize,
    added_heap_alloc_ops: usize,
    removed_heap_alloc_ops: usize,
    call_results_typed_repr_delta: isize,
    call_results_dynbox_delta: isize,
}

impl FactDelta {
    fn between(before: &FactProfile, after: &FactProfile) -> Self {
        let repr_delta = count_delta(&before.repr_counts, &after.repr_counts);
        let op_delta = count_delta(&before.op_counts, &after.op_counts);
        Self {
            blocks_delta: signed_delta(before.blocks, after.blocks),
            ops_delta: signed_delta(before.ops, after.ops),
            typed_values_delta: signed_delta(before.typed_values, after.typed_values),
            lost_repr_values: negative_counts(&repr_delta),
            gained_repr_values: positive_counts(&repr_delta),
            repr_delta,
            op_delta,
            added_box_ops: positive_delta(before.box_ops, after.box_ops),
            removed_box_ops: positive_delta(after.box_ops, before.box_ops),
            added_unbox_ops: positive_delta(before.unbox_ops, after.unbox_ops),
            removed_unbox_ops: positive_delta(after.unbox_ops, before.unbox_ops),
            added_generic_calls: positive_delta(before.generic_calls, after.generic_calls),
            removed_generic_calls: positive_delta(after.generic_calls, before.generic_calls),
            added_runtime_helper_calls: positive_delta(
                before.runtime_helper_calls,
                after.runtime_helper_calls,
            ),
            removed_runtime_helper_calls: positive_delta(
                after.runtime_helper_calls,
                before.runtime_helper_calls,
            ),
            added_rc_events: positive_delta(before.rc_events, after.rc_events),
            removed_rc_events: positive_delta(after.rc_events, before.rc_events),
            added_exception_events: positive_delta(before.exception_events, after.exception_events),
            removed_exception_events: positive_delta(
                after.exception_events,
                before.exception_events,
            ),
            added_type_guard_ops: positive_delta(before.type_guard_ops, after.type_guard_ops),
            removed_type_guard_ops: positive_delta(after.type_guard_ops, before.type_guard_ops),
            added_heap_alloc_ops: positive_delta(before.heap_alloc_ops, after.heap_alloc_ops),
            removed_heap_alloc_ops: positive_delta(after.heap_alloc_ops, before.heap_alloc_ops),
            call_results_typed_repr_delta: signed_delta(
                before.call_results_typed_repr,
                after.call_results_typed_repr,
            ),
            call_results_dynbox_delta: signed_delta(
                before.call_results_dynbox,
                after.call_results_dynbox,
            ),
        }
    }
}

#[derive(Debug, Serialize)]
struct PassDeltaRecord<'a> {
    schema_version: u32,
    kind: &'static str,
    function: &'a str,
    pass: &'a str,
    mutation_class: &'static str,
    host: HostRecord,
    target: TargetRecord,
    stats: PassStatsRecord<'a>,
    before: &'a FactProfile,
    after: &'a FactProfile,
    delta: FactDelta,
}

pub(crate) fn emit_pass_delta(
    func: &TirFunction,
    pass_name: &'static str,
    mutation_class: Mutates,
    tti: &TargetInfo,
    stats: &PassStats,
    before: &FactProfile,
    after: &FactProfile,
) {
    let record = PassDeltaRecord {
        schema_version: PASS_DELTA_SCHEMA_VERSION,
        kind: "molt_tir_pass_delta",
        function: &func.name,
        pass: pass_name,
        mutation_class: mutation_class_name(mutation_class),
        host: HostRecord::current(),
        target: TargetRecord::from_target_info(tti),
        stats: PassStatsRecord::from_stats(stats),
        before,
        after,
        delta: FactDelta::between(before, after),
    };

    match serde_json::to_vec(&record) {
        Ok(mut line) => {
            line.push(b'\n');
            if let Err(err) = append_delta_line(&line) {
                eprintln!("[MOLT_EMIT_PASS_DELTA] failed to write pass delta: {err}");
            }
        }
        Err(err) => {
            eprintln!("[MOLT_EMIT_PASS_DELTA] failed to encode pass delta: {err}");
        }
    }
}

fn append_delta_line(line: &[u8]) -> io::Result<()> {
    if let Some(path) = std::env::var_os("MOLT_PASS_DELTA_PATH") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(line)?;
        return Ok(());
    }
    debug_artifacts::append_debug_artifact("tir/pass_delta.jsonl", line).map(|_| ())
}

fn mutation_class_name(mutation_class: Mutates) -> &'static str {
    match mutation_class {
        Mutates::ReadOnly => "ReadOnly",
        Mutates::OpsOnly => "OpsOnly",
        Mutates::Cfg => "Cfg",
    }
}

fn signed_delta(before: usize, after: usize) -> isize {
    after as isize - before as isize
}

fn positive_delta(before: usize, after: usize) -> usize {
    after.saturating_sub(before)
}

fn count_delta(
    before: &BTreeMap<String, usize>,
    after: &BTreeMap<String, usize>,
) -> BTreeMap<String, isize> {
    let keys: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    let mut out = BTreeMap::new();
    for key in keys {
        let delta = signed_delta(
            *before.get(key).unwrap_or(&0),
            *after.get(key).unwrap_or(&0),
        );
        if delta != 0 {
            out.insert(key.clone(), delta);
        }
    }
    out
}

fn positive_counts(delta: &BTreeMap<String, isize>) -> BTreeMap<String, usize> {
    delta
        .iter()
        .filter_map(|(key, value)| {
            if *value > 0 {
                Some((key.clone(), *value as usize))
            } else {
                None
            }
        })
        .collect()
}

fn negative_counts(delta: &BTreeMap<String, isize>) -> BTreeMap<String, usize> {
    delta
        .iter()
        .filter_map(|(key, value)| {
            if *value < 0 {
                Some((key.clone(), value.unsigned_abs()))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use crate::tir::target_info::TargetInfo;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn fixture_function() -> TirFunction {
        let mut func = TirFunction::new(
            "pass_delta_fixture".into(),
            vec![TirType::I64],
            TirType::I64,
        );
        let entry = func.entry_block;
        let boxed = func.fresh_value();
        let call_result = func.fresh_value();
        let unboxed = func.fresh_value();
        func.value_types.insert(boxed, TirType::DynBox);
        func.value_types.insert(call_result, TirType::Str);
        func.value_types.insert(unboxed, TirType::I64);
        let block = func.blocks.get_mut(&entry).expect("entry block exists");
        block.ops = vec![
            op(OpCode::BoxVal, vec![ValueId(0)], vec![boxed]),
            op(OpCode::CallBuiltin, vec![boxed], vec![call_result]),
            op(OpCode::UnboxVal, vec![call_result], vec![unboxed]),
            op(OpCode::IncRef, vec![boxed], vec![]),
            op(OpCode::CheckException, vec![], vec![]),
        ];
        block.terminator = Terminator::Return {
            values: vec![unboxed],
        };
        func
    }

    #[test]
    fn fact_profile_captures_pass_delta_categories() {
        let func = fixture_function();
        let profile = FactProfile::capture(&func);

        assert_eq!(profile.blocks, 1);
        assert_eq!(profile.ops, 5);
        assert_eq!(profile.box_ops, 1);
        assert_eq!(profile.unbox_ops, 1);
        assert_eq!(profile.generic_calls, 1);
        assert_eq!(profile.runtime_helper_calls, 1);
        assert_eq!(profile.call_results_dynbox, 1);
        assert_eq!(profile.rc_events, 1);
        assert_eq!(profile.exception_events, 1);
        assert_eq!(profile.repr_counts.get("DynBox"), Some(&2));
        assert_eq!(profile.repr_counts.get("MaybeBigInt"), Some(&2));
    }

    #[test]
    fn fact_delta_reports_lost_repr_and_added_ops() {
        let before = FactProfile::capture(&fixture_function());
        let mut after_func = fixture_function();
        let result = after_func.fresh_value();
        after_func.value_types.insert(result, TirType::DynBox);
        after_func
            .blocks
            .get_mut(&after_func.entry_block)
            .expect("entry block exists")
            .ops
            .push(op(OpCode::DecRef, vec![result], vec![]));
        after_func.value_types.remove(&ValueId(3));
        let after = FactProfile::capture(&after_func);

        let delta = FactDelta::between(&before, &after);

        assert_eq!(delta.added_rc_events, 1);
        assert_eq!(delta.lost_repr_values.get("MaybeBigInt"), Some(&1));
        assert_eq!(delta.gained_repr_values.get("DynBox"), Some(&1));
        assert_eq!(delta.op_delta.get("DecRef"), Some(&1));
    }

    #[test]
    fn pass_delta_record_serializes_host_and_target_context() {
        let func = fixture_function();
        let before = FactProfile::capture(&func);
        let after = before.clone();
        let stats = PassStats {
            name: "noop",
            ..PassStats::default()
        };
        let record = PassDeltaRecord {
            schema_version: PASS_DELTA_SCHEMA_VERSION,
            kind: "molt_tir_pass_delta",
            function: &func.name,
            pass: "noop",
            mutation_class: mutation_class_name(Mutates::ReadOnly),
            host: HostRecord::current(),
            target: TargetRecord::from_target_info(&TargetInfo::native_release_fast()),
            stats: PassStatsRecord::from_stats(&stats),
            before: &before,
            after: &after,
            delta: FactDelta::between(&before, &after),
        };

        let encoded = serde_json::to_value(record).expect("record serializes");
        assert_eq!(encoded["schema_version"], PASS_DELTA_SCHEMA_VERSION);
        assert_eq!(encoded["kind"], "molt_tir_pass_delta");
        assert_eq!(encoded["host"]["os"], std::env::consts::OS);
        assert_eq!(encoded["host"]["arch"], std::env::consts::ARCH);
        assert_eq!(encoded["target"]["target"], "NativeCranelift");
        assert_eq!(encoded["target"]["profile"], "ReleaseFast");
    }
}
