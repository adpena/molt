//! Machine-readable per-pass TIR fact deltas.
//!
//! This module is a diagnostic observer only. It samples a compact fact profile
//! before and after each pass when `MOLT_EMIT_PASS_DELTA=1` is set, then writes
//! one JSONL record per pass. Product IR, pass order, analysis invalidation, and
//! verification behavior are unchanged when the observer is disabled or enabled.

use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use super::blocks::Terminator;
use super::function::TirFunction;
use super::op_kinds_generated::{
    SccpConstantSeedRule, opcode_pass_delta_facts_table, opcode_sccp_constant_seed_rule_table,
};
use super::ops::{OpCode, TirOp};
use super::pass_manager::Mutates;
use super::passes::PassStats;
use super::passes::value_identity::{canonical_predicate, copy_value_source, peel_truth_wrappers};
use super::target_info::TargetInfo;
use super::translation_validator::TranslationValidation;
use super::types::TirType;
use super::values::ValueId;
use crate::debug_artifacts;
use crate::repr::Repr;

pub(crate) const PASS_DELTA_SCHEMA_VERSION: u32 = 2;

pub(crate) fn emit_enabled() -> bool {
    backend_env::var(BackendEnv::MOLT_EMIT_PASS_DELTA).as_deref() == Ok("1")
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FactProfile {
    pub blocks: usize,
    pub ops: usize,
    pub typed_values: usize,
    pub repr_counts: BTreeMap<String, usize>,
    pub value_reprs: BTreeMap<String, String>,
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
    pub branch_census: BranchCensus,
}

/// What the conditions of `CondBranch` terminators are made of (HF-773).
///
/// On the M6 programs a constant-only SCCP lattice decided no additional
/// branch (80/80, 262/262): Python branches on truthiness, identity, types
/// and comparisons of values that are not constants. This census names, per
/// undecided branch, the predicate that produced its condition and the class
/// of each predicate operand, so the next value-fact domain is chosen from
/// measured branch shapes. `Switch` terminators are not counted.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct BranchCensus {
    /// Branches whose condition is a constant (left for block removal).
    pub constant: usize,
    /// One row per distinct condition shape, in a stable order.
    pub undecided: Vec<BranchConditionRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct BranchConditionRow {
    /// The canonical predicate under every truth wrapper (`Bool`, `Not`,
    /// copies): `IsNot` counts as `Is` and `Ne` as `Eq`, negated.
    pub source: ConditionSource,
    /// The branch takes its then edge when the canonical predicate is false.
    pub negated: bool,
    /// The predicate's operands, constants last; for a block argument or an
    /// unbound value, the condition itself.
    pub operands: Vec<OperandFact>,
    pub count: usize,
}

/// The producer of a branch condition, after value-identity copies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub(crate) enum ConditionSource {
    Op(OpCode),
    BlockArg,
    Unbound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
pub(crate) struct OperandFact {
    pub class: ValueClass,
    /// Produced by an op with an SCCP constant-seed rule.
    pub constant: bool,
}

/// A closed projection of `TirType` for the census. A boxed value with a
/// known inner type takes the inner type's class.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
pub(crate) enum ValueClass {
    Int,
    Float,
    Bool,
    NoneType,
    Str,
    Bytes,
    List,
    Dict,
    Set,
    Tuple,
    Iterator,
    UserClass,
    Union,
    Dynamic,
    Other,
}

impl ValueClass {
    fn of(ty: Option<&TirType>) -> Self {
        match ty {
            None | Some(TirType::DynBox) => Self::Dynamic,
            Some(TirType::I64 | TirType::BigInt) => Self::Int,
            Some(TirType::F64) => Self::Float,
            Some(TirType::Bool) => Self::Bool,
            Some(TirType::None) => Self::NoneType,
            Some(TirType::Str) => Self::Str,
            Some(TirType::Bytes) => Self::Bytes,
            Some(TirType::List(_)) => Self::List,
            Some(TirType::Dict(..)) => Self::Dict,
            Some(TirType::Set(_)) => Self::Set,
            Some(TirType::Tuple(_)) => Self::Tuple,
            Some(TirType::Iterator(_)) => Self::Iterator,
            Some(TirType::UserClass(_)) => Self::UserClass,
            Some(TirType::Union(_)) => Self::Union,
            Some(TirType::Box(inner)) => Self::of(Some(inner)),
            Some(_) => Self::Other,
        }
    }
}

type ConditionShape = (ConditionSource, bool, Vec<OperandFact>);

impl BranchCensus {
    fn capture(func: &TirFunction) -> Self {
        let mut producers: HashMap<ValueId, &TirOp> = HashMap::new();
        let mut arg_types: HashMap<ValueId, &TirType> = HashMap::new();
        for block in func.blocks.values() {
            for arg in &block.args {
                arg_types.insert(arg.id, &arg.ty);
            }
            for op in &block.ops {
                for &result in &op.results {
                    producers.insert(result, op);
                }
            }
        }
        // Follow value-identity copies to the value's real producer. SSA has
        // no copy cycle; the bound keeps a malformed function finite.
        let resolve = |mut value: ValueId| {
            for _ in 0..=producers.len() {
                match producers.get(&value).and_then(|op| copy_value_source(op)) {
                    Some(source) => value = source,
                    None => break,
                }
            }
            value
        };
        let is_constant = |op: &TirOp| {
            opcode_sccp_constant_seed_rule_table(op.opcode) != SccpConstantSeedRule::None
        };
        let fact = |value: ValueId| OperandFact {
            class: ValueClass::of(
                func.value_types
                    .get(&value)
                    .or_else(|| arg_types.get(&value).copied()),
            ),
            constant: producers.get(&value).is_some_and(|&op| is_constant(op)),
        };

        let mut census = BranchCensus::default();
        let mut shapes: HashMap<ConditionShape, usize> = HashMap::new();
        for block in func.blocks.values() {
            let Terminator::CondBranch { cond, .. } = &block.terminator else {
                continue;
            };
            let truth = peel_truth_wrappers(*cond, producers.len(), |value| {
                producers.get(&value).copied()
            });
            let core = resolve(truth.core);
            let shape = match producers.get(&core) {
                Some(&op) if is_constant(op) => {
                    census.constant += 1;
                    continue;
                }
                Some(&op) => {
                    let (opcode, flips) = canonical_predicate(op.opcode);
                    let mut operands: Vec<OperandFact> = op
                        .operands
                        .iter()
                        .map(|&value| fact(resolve(value)))
                        .collect();
                    // Stable: constants after the rest, order otherwise kept.
                    operands.sort_by_key(|operand| operand.constant);
                    (
                        ConditionSource::Op(opcode),
                        truth.negated != flips,
                        operands,
                    )
                }
                None => {
                    let source = if arg_types.contains_key(&core) {
                        ConditionSource::BlockArg
                    } else {
                        ConditionSource::Unbound
                    };
                    (source, truth.negated, vec![fact(core)])
                }
            };
            *shapes.entry(shape).or_default() += 1;
        }
        census.undecided = shapes
            .into_iter()
            .map(|((source, negated, operands), count)| BranchConditionRow {
                source,
                negated,
                operands,
                count,
            })
            .collect();
        census.undecided.sort_by(|left, right| {
            (format!("{:?}", left.source), left.negated, &left.operands).cmp(&(
                format!("{:?}", right.source),
                right.negated,
                &right.operands,
            ))
        });
        census
    }
}

impl FactProfile {
    pub(crate) fn capture(func: &TirFunction) -> Self {
        let mut profile = FactProfile {
            blocks: func.blocks.len(),
            ops: 0,
            typed_values: 0,
            repr_counts: BTreeMap::new(),
            value_reprs: BTreeMap::new(),
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
            branch_census: BranchCensus::capture(func),
        };

        let value_range = crate::representation_facts::value_range_for(func);
        let repr_by_value =
            crate::representation_facts::repr_by_value_for(func, Some(&value_range));

        let mut seen_values = BTreeSet::new();
        for (&id, ty) in &func.value_types {
            profile.note_value(id, ty, &repr_by_value, &mut seen_values);
        }

        let mut block_ids: Vec<_> = func.blocks.keys().copied().collect();
        block_ids.sort_unstable();
        for block_id in block_ids {
            let Some(block) = func.blocks.get(&block_id) else {
                continue;
            };
            for arg in &block.args {
                profile.note_value(arg.id, &arg.ty, &repr_by_value, &mut seen_values);
            }
            for op in &block.ops {
                profile.note_op(op, &repr_by_value);
            }
        }

        profile
    }

    fn note_value(
        &mut self,
        id: ValueId,
        ty: &TirType,
        repr_by_value: &HashMap<ValueId, Repr>,
        seen_values: &mut BTreeSet<ValueId>,
    ) {
        if !seen_values.insert(id) {
            return;
        }
        self.typed_values += 1;
        let repr = repr_by_value
            .get(&id)
            .copied()
            .unwrap_or_else(|| Repr::default_for(ty));
        let repr_name = format!("{repr:?}");
        self.value_reprs.insert(id.0.to_string(), repr_name.clone());
        *self.repr_counts.entry(repr_name).or_default() += 1;
    }

    fn note_op(&mut self, op: &TirOp, repr_by_value: &HashMap<ValueId, Repr>) {
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
            self.note_call_results(op, repr_by_value);
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

    fn note_call_results(&mut self, op: &TirOp, repr_by_value: &HashMap<ValueId, Repr>) {
        for result in &op.results {
            let repr = repr_by_value.get(result).copied().unwrap_or(Repr::DynBox);
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
    translation_validation: &'a TranslationValidation,
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
    translation_validation: &TranslationValidation,
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
        translation_validation,
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
    if let Some(path) = backend_env::var_os(BackendEnv::MOLT_PASS_DELTA_PATH) {
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
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect};
    use crate::tir::target_info::TargetInfo;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            origin: None,
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
            molt_ir::FunctionReturnAbi::Value,
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

    fn const_i64_function() -> TirFunction {
        let mut func = TirFunction::new(
            "repr_const_i64".into(),
            vec![],
            TirType::I64,
            molt_ir::FunctionReturnAbi::Value,
        );
        let value = func.fresh_value();
        func.value_types.insert(value, TirType::I64);
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(1));
        let block = func.blocks.get_mut(&func.entry_block).unwrap();
        block.ops = vec![TirOp {
            origin: None,
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![value],
            attrs,
            source_span: None,
        }];
        block.terminator = Terminator::Return {
            values: vec![value],
        };
        func
    }

    fn opaque_i64_function() -> TirFunction {
        let mut func = TirFunction::new(
            "repr_opaque_i64".into(),
            vec![],
            TirType::I64,
            molt_ir::FunctionReturnAbi::Value,
        );
        let value = func.fresh_value();
        func.value_types.insert(value, TirType::I64);
        let block = func.blocks.get_mut(&func.entry_block).unwrap();
        block.ops = vec![TirOp {
            origin: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![value],
            attrs: AttrDict::new(),
            source_span: None,
        }];
        block.terminator = Terminator::Return {
            values: vec![value],
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
        assert_eq!(
            profile.value_reprs.get("0").map(String::as_str),
            Some("MaybeBigInt")
        );
        assert_eq!(
            profile.value_reprs.get("1").map(String::as_str),
            Some("DynBox")
        );
        assert_eq!(
            profile.value_reprs.get("3").map(String::as_str),
            Some("MaybeBigInt")
        );
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

    /// `if p is None` (identity against a constant), a constant condition, and
    /// `if not (p < p)` through a copy: three branch shapes with known answers.
    fn branch_census_function() -> TirFunction {
        let mut func = TirFunction::new(
            "branch_census".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let param = ValueId(0);
        let none = func.fresh_value();
        let is_none = func.fresh_value();
        let truth = func.fresh_value();
        let less = func.fresh_value();
        let negated = func.fresh_value();
        let copied = func.fresh_value();
        func.value_types.insert(none, TirType::None);
        func.value_types.insert(is_none, TirType::Bool);
        func.value_types.insert(truth, TirType::Bool);
        func.value_types.insert(less, TirType::DynBox);
        func.value_types.insert(negated, TirType::Bool);
        func.value_types.insert(copied, TirType::Bool);
        let constant_block = func.fresh_block();
        let compare_block = func.fresh_block();
        let exit = func.fresh_block();
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).expect("entry block exists");
        block.ops = vec![
            op(OpCode::ConstNone, vec![], vec![none]),
            op(OpCode::Is, vec![param, none], vec![is_none]),
        ];
        block.terminator = Terminator::CondBranch {
            cond: is_none,
            then_block: constant_block,
            then_args: vec![],
            else_block: compare_block,
            else_args: vec![],
        };
        let branch_to_exit = |cond| Terminator::CondBranch {
            cond,
            then_block: exit,
            then_args: vec![],
            else_block: exit,
            else_args: vec![],
        };
        func.blocks.insert(
            constant_block,
            TirBlock {
                id: constant_block,
                args: vec![],
                ops: vec![op(OpCode::ConstBool, vec![], vec![truth])],
                terminator: branch_to_exit(truth),
            },
        );
        func.blocks.insert(
            compare_block,
            TirBlock {
                id: compare_block,
                args: vec![],
                ops: vec![
                    op(OpCode::Lt, vec![param, param], vec![less]),
                    op(OpCode::Not, vec![less], vec![negated]),
                    op(OpCode::Copy, vec![negated], vec![copied]),
                ],
                terminator: branch_to_exit(copied),
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![param],
                },
            },
        );
        func
    }

    #[test]
    fn branch_census_names_each_undecided_predicate_and_its_operands() {
        let census = FactProfile::capture(&branch_census_function()).branch_census;
        let dynamic = OperandFact {
            class: ValueClass::Dynamic,
            constant: false,
        };
        let none_constant = OperandFact {
            class: ValueClass::NoneType,
            constant: true,
        };

        assert_eq!(census.constant, 1);
        assert_eq!(
            census.undecided,
            vec![
                BranchConditionRow {
                    source: ConditionSource::Op(OpCode::Is),
                    negated: false,
                    operands: vec![dynamic, none_constant],
                    count: 1,
                },
                BranchConditionRow {
                    source: ConditionSource::Op(OpCode::Lt),
                    negated: true,
                    operands: vec![dynamic, dynamic],
                    count: 1,
                },
            ]
        );
    }

    #[test]
    fn branch_census_counts_each_spelling_of_a_none_test_once() {
        // p is None; None is p; p is not None; not (p is None);
        // bool(not (p is None)): one predicate, two polarities.
        let mut func = TirFunction::new(
            "none_spellings".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let param = ValueId(0);
        let none = func.fresh_value();
        func.value_types.insert(none, TirType::None);
        let exit = func.fresh_block();
        let spellings: [&[(OpCode, bool)]; 5] = [
            &[(OpCode::Is, false)],
            &[(OpCode::Is, true)],
            &[(OpCode::IsNot, false)],
            &[(OpCode::Is, false), (OpCode::Not, false)],
            &[
                (OpCode::Is, false),
                (OpCode::Not, false),
                (OpCode::Bool, false),
            ],
        ];
        let mut blocks = Vec::new();
        for chain in spellings {
            let id = func.fresh_block();
            let mut ops = Vec::new();
            let mut cond = None;
            for &(opcode, none_first) in chain {
                let result = func.fresh_value();
                func.value_types.insert(result, TirType::Bool);
                let operands = match (opcode, cond) {
                    (OpCode::Is | OpCode::IsNot, _) if none_first => vec![none, param],
                    (OpCode::Is | OpCode::IsNot, _) => vec![param, none],
                    (_, Some(previous)) => vec![previous],
                    (_, None) => unreachable!("a wrapper follows its predicate"),
                };
                ops.push(op(opcode, operands, vec![result]));
                cond = Some(result);
            }
            blocks.push((id, ops, cond.expect("every spelling has a predicate")));
        }
        let entry = func.entry_block;
        let first = blocks[0].0;
        let block = func.blocks.get_mut(&entry).expect("entry block exists");
        block.ops = vec![op(OpCode::ConstNone, vec![], vec![none])];
        block.terminator = Terminator::Branch {
            target: first,
            args: vec![],
        };
        for (index, (id, ops, cond)) in blocks.iter().cloned().enumerate() {
            let next = blocks.get(index + 1).map_or(exit, |(next, _, _)| *next);
            func.blocks.insert(
                id,
                TirBlock {
                    id,
                    args: vec![],
                    ops,
                    terminator: Terminator::CondBranch {
                        cond,
                        then_block: next,
                        then_args: vec![],
                        else_block: exit,
                        else_args: vec![],
                    },
                },
            );
        }
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![param],
                },
            },
        );
        let census = FactProfile::capture(&func).branch_census;
        let dynamic = OperandFact {
            class: ValueClass::Dynamic,
            constant: false,
        };
        let none_constant = OperandFact {
            class: ValueClass::NoneType,
            constant: true,
        };
        let row = |negated, count| BranchConditionRow {
            source: ConditionSource::Op(OpCode::Is),
            negated,
            operands: vec![dynamic, none_constant],
            count,
        };
        assert_eq!(census.undecided, vec![row(false, 2), row(true, 3)]);
    }

    #[test]
    fn translation_validation_catches_carrier_widening_drift_from_ir_profiles() {
        let before = FactProfile::capture(&const_i64_function());
        let after = FactProfile::capture(&opaque_i64_function());

        assert_eq!(
            before.value_reprs.get("0").map(String::as_str),
            Some("RawI64Safe")
        );
        assert_eq!(
            after.value_reprs.get("0").map(String::as_str),
            Some("MaybeBigInt")
        );

        let validation = crate::tir::translation_validator::validate_repr_lattice_monotonic(
            &before.value_reprs,
            &after.value_reprs,
        );

        assert!(!validation.passed);
        assert_eq!(validation.violations.len(), 1);
        assert_eq!(validation.violations[0].value_id, "0");
        assert_eq!(validation.violations[0].before, "RawI64Safe");
        assert_eq!(validation.violations[0].after, "MaybeBigInt");
    }

    #[test]
    fn pass_delta_record_serializes_host_and_target_context() {
        let func = fixture_function();
        let before = FactProfile::capture(&func);
        let after = before.clone();
        let translation_validation =
            crate::tir::translation_validator::validate_repr_lattice_monotonic(
                &before.value_reprs,
                &after.value_reprs,
            );
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
            translation_validation: &translation_validation,
            delta: FactDelta::between(&before, &after),
        };

        let encoded = serde_json::to_value(record).expect("record serializes");
        assert_eq!(encoded["schema_version"], PASS_DELTA_SCHEMA_VERSION);
        assert_eq!(encoded["kind"], "molt_tir_pass_delta");
        assert_eq!(
            encoded["translation_validation"]["check"],
            "repr_lattice_monotonic"
        );
        assert_eq!(encoded["translation_validation"]["passed"], true);
        assert_eq!(encoded["host"]["os"], std::env::consts::OS);
        assert_eq!(encoded["host"]["arch"], std::env::consts::ARCH);
        assert_eq!(encoded["target"]["target"], "NativeCranelift");
        assert_eq!(encoded["target"]["profile"], "ReleaseFast");
    }
}
