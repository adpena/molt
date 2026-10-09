//! Canonical structural verifier for the flat SimpleIR transport.
//!
//! This module owns logical CFG edges, definite-definition dataflow, and PHI
//! predecessor ordering. Tooling may transport reports, but must not rebuild a
//! second control-flow model outside `molt-ir`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::ir::ExecutionContextPolicy;
use crate::ir::{FunctionIR, OpIR, SimpleIR};
use crate::tir::cfg_liveness::{SimpleNameSet, SimpleNameTable};
use crate::tir::dominators::{
    exception_edge_binds_handler_arguments, is_simple_exception_transfer_kind,
};
use crate::tir::op_kinds_generated::{
    SimpleIrCallTargetRole, SimpleIrVerifierRegionRole, kind_to_opcode_table,
    simpleir_call_target_role, simpleir_kind_is_repoll, simpleir_kind_is_suspend,
    simpleir_kind_is_terminator, simpleir_kind_is_verifier_label_definition,
    simpleir_kind_is_verifier_label_reference, simpleir_kind_is_verifier_loop_scoped,
    simpleir_kind_is_verifier_phi, simpleir_verifier_region_role,
};
use crate::tir::simple_def_use::{visit_simple_ir_defined_names, visit_simple_ir_reads};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimpleIrDiagnostic {
    pub function: String,
    pub op_index: isize,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimpleIrVerificationReport {
    pub errors: Vec<SimpleIrDiagnostic>,
    pub warnings: Vec<SimpleIrDiagnostic>,
    pub functions_checked: usize,
    pub ops_checked: usize,
}

impl SimpleIrVerificationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalEdge {
    pub source: usize,
    pub target: usize,
    pub role: EdgeRole,
    pub ordinal: usize,
    /// False for verifier-only reachability, never a runtime transfer.
    pub executable: bool,
    /// Transfer polarity before PHI predecessor-order annotation.
    pub execution_role: EdgeRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeRole {
    BranchTrue,
    BranchFalse,
    LoopEntry,
    LoopLatch,
    LoopExit,
    Normal,
    Exception,
    DispatchDefault,
    Resume,
    Fallthrough,
    Taken,
}

impl EdgeRole {
    fn phi_order(self) -> usize {
        match self {
            Self::BranchTrue
            | Self::LoopEntry
            | Self::Normal
            | Self::DispatchDefault
            | Self::Fallthrough => 0,
            Self::BranchFalse | Self::LoopLatch | Self::Exception | Self::Resume | Self::Taken => 1,
            Self::LoopExit => 99,
        }
    }
}

#[derive(Debug, Clone)]
struct StructuredTargets {
    if_regions: BTreeMap<usize, (Option<usize>, usize)>,
    loop_ends: BTreeMap<usize, usize>,
    loop_for_op: BTreeMap<usize, usize>,
}

#[derive(Debug, Clone)]
struct OpFlow {
    edges: Vec<Vec<LogicalEdge>>,
    /// Sources of executable transfers beyond the operation stream. Keep these
    /// outside the real-index graph so block/liveness consumers cannot mistake
    /// an implicit exit for an operation, or lose it while clipping successors.
    implicit_exits: BTreeSet<usize>,
}

impl OpFlow {
    fn executable_reachable(&self) -> Vec<bool> {
        let mut reachable = vec![false; self.edges.len()];
        let mut pending: Vec<_> = (!self.edges.is_empty()).then_some(0).into_iter().collect();
        while let Some(index) = pending.pop() {
            if std::mem::replace(&mut reachable[index], true) {
                continue;
            }
            pending.extend(
                self.edges[index]
                    .iter()
                    .filter(|edge| edge.executable)
                    .map(|edge| edge.target),
            );
        }
        reachable
    }
}

#[derive(Debug, Clone)]
struct BasicBlocks {
    ranges: Vec<(usize, usize)>,
    op_to_block: Vec<usize>,
}

/// The executable projection of the verifier's operation-level control flow, shared with
/// targets that cannot express labelled transfers directly. Block ranges are
/// inclusive. Exception observers remain exact operation boundaries; TRY
/// metadata must not be reinterpreted as a lexical protected interval.
pub struct SimpleIrLogicalFlow {
    pub edges: Vec<Vec<LogicalEdge>>,
    pub blocks: Vec<(usize, usize)>,
    pub op_to_block: Vec<usize>,
    pub cross_block_values: BTreeSet<String>,
}

/// Project operations after [`validate_simple_ir_control_flow`] has admitted
/// their function's label namespace and structured regions.
pub fn simple_ir_logical_flow(ops: &[OpIR]) -> SimpleIrLogicalFlow {
    if ops.is_empty() {
        return SimpleIrLogicalFlow {
            edges: Vec::new(),
            blocks: Vec::new(),
            op_to_block: Vec::new(),
            cross_block_values: BTreeSet::new(),
        };
    }
    let edges: Vec<Vec<_>> = op_flow(ops)
        .edges
        .into_iter()
        .map(|edges| {
            edges
                .into_iter()
                .filter(|edge| edge.executable)
                .map(|mut edge| {
                    edge.role = edge.execution_role;
                    edge
                })
                .collect()
        })
        .collect();
    let blocks = basic_blocks(ops, &edges);
    let count = blocks.ranges.len();
    let mut definitions = vec![BTreeSet::new(); count];
    let mut uses = vec![BTreeSet::new(); count];
    let mut successors = vec![BTreeSet::new(); count];
    for (block, &(start, end)) in blocks.ranges.iter().enumerate() {
        for index in start..=end {
            visit_simple_ir_reads(&ops[index], |read| {
                if !definitions[block].contains(read.name) {
                    uses[block].insert(read.name.to_string());
                }
            });
            visit_simple_ir_defined_names(&ops[index], |name| {
                definitions[block].insert(name.to_string());
            });
        }
        // Only the tail transfers between executable blocks. Retain a real
        // backedge to this same block; internal fallthroughs are not backedges.
        for edge in &edges[end] {
            successors[block].insert(blocks.op_to_block[edge.target]);
        }
    }
    let mut live_in = uses.clone();
    let mut live_out = vec![BTreeSet::new(); count];
    loop {
        let mut changed = false;
        for block in (0..count).rev() {
            let outgoing: BTreeSet<String> = successors[block]
                .iter()
                .flat_map(|target| live_in[*target].iter().cloned())
                .collect();
            let incoming = uses[block]
                .union(&outgoing.difference(&definitions[block]).cloned().collect())
                .cloned()
                .collect();
            changed |= outgoing != live_out[block] || incoming != live_in[block];
            live_out[block] = outgoing;
            live_in[block] = incoming;
        }
        if !changed {
            break;
        }
    }
    let all_defined: BTreeSet<String> = definitions.into_iter().flatten().collect();
    let cross_block_values = live_out
        .into_iter()
        .flatten()
        .filter(|name| all_defined.contains(name))
        .collect();
    SimpleIrLogicalFlow {
        edges,
        blocks: blocks.ranges,
        op_to_block: blocks.op_to_block,
        cross_block_values,
    }
}

pub fn verify_simple_ir(ir: &SimpleIR) -> SimpleIrVerificationReport {
    let mut report = SimpleIrVerificationReport::default();
    let function_names: BTreeSet<&str> = ir.functions.iter().map(|f| f.name.as_str()).collect();
    for function in &ir.functions {
        report.functions_checked += 1;
        report.ops_checked += function.ops.len();
        verify_function(function, &function_names, &mut report.errors);
    }
    if !ir.functions.is_empty() && !function_names.contains("molt_main") {
        report.warnings.push(diagnostic(
            "<top-level>",
            -1,
            "missing-entry",
            "no 'molt_main' function found in SimpleIR",
        ));
    }
    report
}

/// Admit the control-flow graph consumed by lowering and target planning.
///
/// Reuse the full verifier's structural and label checks without imposing its
/// whole-function completion or dataflow diagnostics on partial IR fragments.
/// In particular, implicit falloff is an executable exit, not an undefined
/// label, and path-local TRY markers are never lexical brackets.
pub fn validate_simple_ir_control_flow(ir: &SimpleIR) -> Result<(), String> {
    let mut errors = Vec::new();
    for function in &ir.functions {
        verify_control_flow(function, &mut errors);
    }
    if let Some(error) = errors.first() {
        return Err(format!(
            "function `{}` op#{} [{}]: {}",
            error.function, error.op_index, error.kind, error.message
        ));
    }
    Ok(())
}

fn diagnostic(
    function: &str,
    op_index: isize,
    kind: &str,
    message: impl Into<String>,
) -> SimpleIrDiagnostic {
    SimpleIrDiagnostic {
        function: function.to_string(),
        op_index,
        kind: kind.to_string(),
        message: message.into(),
    }
}

fn verify_function(
    function: &FunctionIR,
    function_names: &BTreeSet<&str>,
    errors: &mut Vec<SimpleIrDiagnostic>,
) {
    let ops = &function.ops;
    if ops.is_empty() {
        errors.push(diagnostic(
            &function.name,
            -1,
            "empty-function",
            "function has no ops (no entry point)",
        ));
        return;
    }
    let flow = op_flow(ops);
    verify_definite_definitions(function, &flow.edges, errors);
    verify_function_references(function, function_names, errors);
    verify_control_flow(function, errors);
    verify_completion(function, &flow, errors);
}

fn verify_completion(function: &FunctionIR, flow: &OpFlow, errors: &mut Vec<SimpleIrDiagnostic>) {
    let reachable = flow.executable_reachable();
    for &index in &flow.implicit_exits {
        if reachable[index] {
            errors.push(diagnostic(
                &function.name,
                index as isize,
                "missing-return",
                format!(
                    "executable control flow leaves the function without a terminator at {:?}",
                    function.ops[index].kind
                ),
            ));
        }
    }
}

fn verify_control_flow(function: &FunctionIR, errors: &mut Vec<SimpleIrDiagnostic>) {
    verify_block_structure(function, errors);
    verify_labels(function, errors);
    if let Err(message) = crate::ir_schema::validate_state_dispatch(&function.ops) {
        errors.push(diagnostic(
            &function.name,
            -1,
            "invalid-state-dispatch",
            message,
        ));
    }
}

fn verify_function_references(
    function: &FunctionIR,
    function_names: &BTreeSet<&str>,
    errors: &mut Vec<SimpleIrDiagnostic>,
) {
    for (index, op) in function.ops.iter().enumerate() {
        let Some(role) = simpleir_call_target_role(&op.kind) else {
            continue;
        };
        if role == SimpleIrCallTargetRole::Opaque {
            continue;
        }
        let Some(target) = op.s_value.as_deref().filter(|value| !value.is_empty()) else {
            errors.push(diagnostic(
                &function.name,
                index as isize,
                "invalid-call-target",
                format!("{:?} direct call has no string target", op.kind),
            ));
            continue;
        };
        if role == SimpleIrCallTargetRole::InternalRequired
            && !function_names.contains(target)
            && !target.starts_with("molt_")
        {
            errors.push(diagnostic(
                &function.name,
                index as isize,
                "invalid-call-target",
                format!(
                    "{:?} op references internal function {:?} which is not in the function list",
                    op.kind, target
                ),
            ));
        }
    }
}

fn verify_block_structure(function: &FunctionIR, errors: &mut Vec<SimpleIrDiagnostic>) {
    let mut stack: Vec<(&str, usize, bool)> = Vec::new();
    for (index, op) in function.ops.iter().enumerate() {
        if let Some((region, role)) = simpleir_verifier_region_role(&op.kind) {
            match role {
                SimpleIrVerifierRegionRole::Start => stack.push((region, index, false)),
                SimpleIrVerifierRegionRole::Alternate => match stack.last_mut() {
                    Some((active, _, seen)) if *active == region && !*seen => *seen = true,
                    Some((active, _, true)) if *active == region => errors.push(diagnostic(
                        &function.name,
                        index as isize,
                        "duplicate-control-alternate",
                        format!(
                            "{:?} appears more than once for region {:?}",
                            op.kind, region
                        ),
                    )),
                    _ => errors.push(diagnostic(
                        &function.name,
                        index as isize,
                        "unbalanced-control-flow",
                        format!("{:?} has no active {:?} region", op.kind, region),
                    )),
                },
                SimpleIrVerifierRegionRole::End => match stack.last() {
                    Some((active, _, _)) if *active == region => {
                        stack.pop();
                    }
                    _ => errors.push(diagnostic(
                        &function.name,
                        index as isize,
                        "unbalanced-control-flow",
                        format!("{:?} has no active {:?} region", op.kind, region),
                    )),
                },
            }
        } else if simpleir_kind_is_verifier_loop_scoped(&op.kind)
            && !stack.iter().any(|(region, _, _)| *region == "loop")
        {
            errors.push(diagnostic(
                &function.name,
                index as isize,
                "break-outside-loop",
                format!("{:?} appears outside a loop region", op.kind),
            ));
        }
    }
    for (region, index, _) in stack {
        errors.push(diagnostic(
            &function.name,
            index as isize,
            "unbalanced-control-flow",
            format!("region {:?} has no matching end", region),
        ));
    }
}

fn verify_labels(function: &FunctionIR, errors: &mut Vec<SimpleIrDiagnostic>) {
    let mut definitions = BTreeMap::new();
    let mut references = Vec::new();
    for (index, op) in function.ops.iter().enumerate() {
        if simpleir_kind_is_verifier_label_definition(&op.kind) {
            match op.value {
                None => errors.push(diagnostic(
                    &function.name,
                    index as isize,
                    "malformed-label-definition",
                    format!("{:?} label definition must have an integer value", op.kind),
                )),
                Some(value) => {
                    if let Some(previous) = definitions.insert(value, index) {
                        errors.push(diagnostic(
                            &function.name,
                            index as isize,
                            "duplicate-label-definition",
                            format!("label {value} was already defined at op #{previous}"),
                        ));
                    }
                }
            }
        } else if simpleir_kind_is_verifier_label_reference(&op.kind) {
            match op.value {
                Some(value) => references.push((index, value)),
                None => errors.push(diagnostic(
                    &function.name,
                    index as isize,
                    "malformed-label-reference",
                    format!("{:?} label reference must have an integer value", op.kind),
                )),
            }
        }
    }
    for (index, target) in references {
        if !definitions.contains_key(&target) {
            errors.push(diagnostic(
                &function.name,
                index as isize,
                "invalid-jump-target",
                format!(
                    "{:?} references undefined label {target}",
                    function.ops[index].kind
                ),
            ));
        }
    }
}

fn structured_targets(ops: &[OpIR]) -> StructuredTargets {
    let mut if_regions = BTreeMap::new();
    let mut loop_ends = BTreeMap::new();
    let mut stack: Vec<(&str, usize, Option<usize>)> = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let Some((region, role)) = simpleir_verifier_region_role(&op.kind) else {
            continue;
        };
        match role {
            SimpleIrVerifierRegionRole::Start => stack.push((region, index, None)),
            SimpleIrVerifierRegionRole::Alternate => {
                if let Some((active, _, alternate)) = stack.last_mut()
                    && *active == region
                {
                    *alternate = Some(index);
                }
            }
            SimpleIrVerifierRegionRole::End => {
                if stack.last().is_some_and(|(active, _, _)| *active == region) {
                    let (_, start, alternate) = stack.pop().expect("checked stack");
                    if region == "if" {
                        if_regions.insert(start, (alternate, index));
                    } else if region == "loop" {
                        loop_ends.insert(start, index);
                    }
                }
            }
        }
    }
    let mut loop_for_op = BTreeMap::new();
    let mut active_loops = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        if simpleir_verifier_region_role(&op.kind)
            == Some(("loop", SimpleIrVerifierRegionRole::Start))
            && loop_ends.contains_key(&index)
        {
            active_loops.push(index);
        }
        if let Some(active) = active_loops.last() {
            loop_for_op.insert(index, *active);
        }
        if simpleir_verifier_region_role(&op.kind)
            == Some(("loop", SimpleIrVerifierRegionRole::End))
        {
            active_loops.pop();
        }
    }
    StructuredTargets {
        if_regions,
        loop_ends,
        loop_for_op,
    }
}

/// Operation-index resume transfers shared by verification and CFG/SSA lifting.
/// Explicit maps are authoritative, including an empty map. Source IR without
/// a map establishes resume identity at suspension sites before CFG lifting;
/// terminal lowering serializes that identity and never infers it again.
/// The target may be `ops.len()`: that is a real missing continuation, which
/// whole-function verification must observe before block projection clips it.
pub(crate) fn state_resume_op_edges(ops: &[OpIR]) -> Vec<(usize, usize, i64)> {
    let Some(switch) = ops.iter().position(|op| op.kind == "state_switch") else {
        return Vec::new();
    };
    let labels: BTreeMap<i64, usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            simpleir_kind_is_verifier_label_definition(&op.kind)
                .then_some(op.value.map(|label| (label, index)))
                .flatten()
        })
        .collect();
    if let Some(targets) = &ops[switch].state_targets {
        // Missing and duplicate labels/states are rejected by the shared
        // dispatch validator. Do not turn malformed maps into inferred edges.
        return targets
            .iter()
            .filter_map(|&(state, label)| labels.get(&label).map(|&target| (switch, target, state)))
            .collect();
    }
    let state_labels: BTreeMap<i64, usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            (op.kind == "state_label")
                .then_some(op.value.map(|state| (state, index)))
                .flatten()
        })
        .collect();
    let mut edges = Vec::new();
    for (index, state) in suspension_saved_states(ops) {
        let Some(state) = state else {
            continue;
        };
        let target = state_labels
            .get(&state)
            .copied()
            .or_else(|| (!simpleir_kind_is_repoll(&ops[index].kind)).then_some(index + 1));
        if let Some(target) = target {
            edges.push((switch, target, state));
        }
    }
    edges.sort_unstable();
    edges.dedup();
    edges
}

/// Decode the state actually saved when a suspension ends its invocation.
/// A repoll's `value` is the running/ready state, not its pending resume state.
/// Both source inference and explicit-map admission must use this same fact.
fn suspension_saved_states(ops: &[OpIR]) -> Vec<(usize, Option<i64>)> {
    let const_values: BTreeMap<&str, i64> = ops
        .iter()
        .filter_map(|op| {
            (op.kind == "const")
                .then_some(op.out.as_deref().zip(op.value))
                .flatten()
        })
        .collect();
    ops.iter()
        .enumerate()
        .filter(|(_, op)| simpleir_kind_is_suspend(&op.kind))
        .map(|(index, op)| {
            let state = if simpleir_kind_is_repoll(&op.kind) {
                op.args
                    .as_deref()
                    .and_then(|args| args.last())
                    .and_then(|name| const_values.get(name.as_str()).copied())
            } else {
                op.value
            };
            (index, state)
        })
        .collect()
}

/// Complete the shared schema check after explicit map shape/labels have been
/// admitted. This reads the canonical executable graph directly; it does not
/// call a validator, infer replacement cases, or recurse through CFG lifting.
pub(crate) fn validate_explicit_state_resume_coverage(ops: &[OpIR]) -> Result<(), String> {
    let Some((switch, targets)) = ops.iter().enumerate().find_map(|(index, op)| {
        (op.kind == "state_switch")
            .then_some(op.state_targets.as_ref().map(|targets| (index, targets)))
            .flatten()
    }) else {
        return Ok(());
    };
    // Terminal ownership removes suspension operations. StateSet alone also
    // records running/ready states, so it cannot require a resume case.
    if !ops.iter().any(|op| simpleir_kind_is_suspend(&op.kind)) {
        return Ok(());
    }
    let states: BTreeSet<_> = targets.iter().map(|&(state, _)| state).collect();
    let reachable = op_flow(ops).executable_reachable();
    for (index, state) in suspension_saved_states(ops) {
        if !reachable[index] {
            continue;
        }
        let state = state.ok_or_else(|| {
            format!(
                "op#{index}: reachable {} requires a statically known saved resume state for state_switch op#{switch}",
                ops[index].kind
            )
        })?;
        if !states.contains(&state) {
            return Err(format!(
                "op#{index}: reachable {} saves state {state} absent from state_switch op#{switch} state_targets",
                ops[index].kind
            ));
        }
    }
    Ok(())
}

fn op_flow(ops: &[OpIR]) -> OpFlow {
    let count = ops.len();
    let labels: BTreeMap<i64, usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            (simpleir_kind_is_verifier_label_definition(&op.kind))
                .then_some(op.value.map(|value| (value, index)))
                .flatten()
        })
        .collect();
    let targets = structured_targets(ops);
    let resume_edges = state_resume_op_edges(ops);
    let if_by_end: BTreeMap<usize, (usize, Option<usize>)> = targets
        .if_regions
        .iter()
        .map(|(start, (alternate, end))| (*end, (*start, *alternate)))
        .collect();
    let mut edges = vec![Vec::new(); count];
    let mut implicit_exits = BTreeSet::new();
    let mut add = |source: usize, target: usize, mut role: EdgeRole| {
        // TRY_START describes handler reachability for verification, not a
        // runtime pending-state observation. LOOP_END's exit is likewise a
        // conservative verifier join; actual execution takes its latch.
        let executable = match role {
            EdgeRole::Exception => kind_to_opcode_table(&ops[source].kind)
                .is_some_and(exception_edge_binds_handler_arguments),
            EdgeRole::LoopExit => ops[source].kind != "loop_end",
            _ => true,
        };
        if target >= count {
            if executable {
                implicit_exits.insert(source);
            }
            return;
        }
        let execution_role = role;
        if let Some((start, alternate)) = if_by_end.get(&target) {
            role = match alternate {
                None if *start < source && source < target => EdgeRole::BranchTrue,
                Some(alternate) if *start < source && source <= *alternate => EdgeRole::BranchTrue,
                Some(alternate) if *alternate < source && source < target => EdgeRole::BranchFalse,
                _ => role,
            };
        }
        let ordinal = edges[source].len();
        edges[source].push(LogicalEdge {
            source,
            target,
            role,
            ordinal,
            executable,
            execution_role,
        });
    };
    for (index, op) in ops.iter().enumerate() {
        let next = index + 1;
        match op.kind.as_str() {
            "if" => {
                if let Some((alternate, end)) = targets.if_regions.get(&index) {
                    add(
                        index,
                        if *alternate == Some(next) { *end } else { next },
                        EdgeRole::BranchTrue,
                    );
                    add(
                        index,
                        alternate.map_or(*end, |value| value + 1),
                        EdgeRole::BranchFalse,
                    );
                }
            }
            "else" => {
                if let Some(end) = targets
                    .if_regions
                    .values()
                    .find_map(|(alternate, end)| (*alternate == Some(index)).then_some(*end))
                {
                    add(index, end, EdgeRole::Taken);
                }
            }
            "loop_end" => {
                if let Some(start) = targets.loop_for_op.get(&index) {
                    add(index, next, EdgeRole::LoopExit);
                    add(index, *start, EdgeRole::LoopLatch);
                }
            }
            kind if simpleir_kind_is_verifier_loop_scoped(kind) => {
                if let Some(start) = targets.loop_for_op.get(&index)
                    && let Some(end) = targets.loop_ends.get(start)
                {
                    match kind {
                        "loop_continue" => add(index, *start, EdgeRole::LoopLatch),
                        "loop_break" => add(index, end + 1, EdgeRole::LoopExit),
                        _ => {
                            let true_break = kind != "loop_break_if_false";
                            add(
                                index,
                                end + 1,
                                if true_break {
                                    EdgeRole::BranchTrue
                                } else {
                                    EdgeRole::BranchFalse
                                },
                            );
                            add(
                                index,
                                next,
                                if true_break {
                                    EdgeRole::BranchFalse
                                } else {
                                    EdgeRole::BranchTrue
                                },
                            );
                        }
                    }
                }
            }
            kind if is_simple_exception_transfer_kind(kind) => {
                add(index, next, EdgeRole::Normal);
                if let Some(target) = op.value.and_then(|value| labels.get(&value).copied()) {
                    add(index, target, EdgeRole::Exception);
                }
            }
            "jump" | "goto" => {
                if let Some(target) = op.value.and_then(|value| labels.get(&value).copied()) {
                    add(index, target, EdgeRole::Taken);
                }
            }
            "br_if" => {
                if let Some(target) = op.value.and_then(|value| labels.get(&value).copied()) {
                    add(index, target, EdgeRole::BranchTrue);
                }
                add(index, next, EdgeRole::BranchFalse);
            }
            "state_switch" => {
                add(index, next, EdgeRole::DispatchDefault);
                for &(source, target, _) in &resume_edges {
                    if source == index {
                        add(index, target, EdgeRole::Resume);
                    }
                }
            }
            kind if simpleir_kind_is_suspend(kind) => {
                if simpleir_kind_is_repoll(kind) {
                    add(index, next, EdgeRole::Fallthrough);
                }
            }
            kind if simpleir_kind_is_terminator(kind) => {}
            _ => {
                let role = if targets.loop_ends.contains_key(&next) {
                    EdgeRole::LoopEntry
                } else {
                    EdgeRole::Fallthrough
                };
                add(index, next, role);
            }
        }
    }
    OpFlow {
        edges,
        implicit_exits,
    }
}

fn basic_blocks(ops: &[OpIR], edges: &[Vec<LogicalEdge>]) -> BasicBlocks {
    let mut leaders = BTreeSet::from([0]);
    for (index, outgoing) in edges.iter().enumerate() {
        let targets: BTreeSet<usize> = outgoing.iter().map(|edge| edge.target).collect();
        let next = index + 1;
        let expected = if next < ops.len() {
            BTreeSet::from([next])
        } else {
            BTreeSet::new()
        };
        if targets != expected {
            if next < ops.len() {
                leaders.insert(next);
            }
            leaders.extend(targets);
        }
    }
    let starts: Vec<usize> = leaders.into_iter().collect();
    let ranges: Vec<(usize, usize)> = starts
        .iter()
        .enumerate()
        .map(|(position, start)| {
            (
                *start,
                starts
                    .get(position + 1)
                    .map_or(ops.len() - 1, |next| next - 1),
            )
        })
        .collect();
    let mut op_to_block = vec![0; ops.len()];
    for (block, (start, end)) in ranges.iter().enumerate() {
        op_to_block[*start..=*end].fill(block);
    }
    BasicBlocks {
        ranges,
        op_to_block,
    }
}

fn canonical_phi_edges(
    block: usize,
    incoming: &[Vec<LogicalEdge>],
    reachable: &BTreeSet<usize>,
) -> Vec<LogicalEdge> {
    let mut target = block;
    let mut edges: Vec<LogicalEdge> = incoming[target]
        .iter()
        .filter(|edge| reachable.contains(&edge.source))
        .cloned()
        .collect();
    let mut visited = BTreeSet::from([target]);
    while edges.len() == 1 {
        let predecessor = edges[0].source;
        if visited.contains(&predecessor) {
            break;
        }
        let upstream: Vec<LogicalEdge> = incoming[predecessor]
            .iter()
            .filter(|edge| reachable.contains(&edge.source))
            .cloned()
            .collect();
        if upstream.len() <= 1 {
            break;
        }
        visited.insert(predecessor);
        target = predecessor;
        edges = upstream;
    }
    let semantic_role = |edge: &LogicalEdge| {
        let mut role = edge.role;
        let mut source = edge.source;
        let mut seen = BTreeSet::from([target]);
        while matches!(role, EdgeRole::Fallthrough | EdgeRole::Taken) && !seen.contains(&source) {
            seen.insert(source);
            let upstream: Vec<&LogicalEdge> = incoming[source]
                .iter()
                .filter(|candidate| reachable.contains(&candidate.source))
                .collect();
            if upstream.len() != 1 {
                break;
            }
            role = upstream[0].role;
            source = upstream[0].source;
        }
        role
    };
    edges.sort_by_key(|edge| (semantic_role(edge).phi_order(), edge.ordinal));
    edges
}

fn verify_definite_definitions(
    function: &FunctionIR,
    edges: &[Vec<LogicalEdge>],
    errors: &mut Vec<SimpleIrDiagnostic>,
) {
    let ops = &function.ops;
    let blocks = basic_blocks(ops, edges);
    let mut successors = vec![BTreeSet::new(); blocks.ranges.len()];
    let mut predecessors = vec![BTreeSet::new(); blocks.ranges.len()];
    let mut incoming = vec![Vec::new(); blocks.ranges.len()];
    for (block, (_, end)) in blocks.ranges.iter().enumerate() {
        for edge in &edges[*end] {
            let successor = blocks.op_to_block[edge.target];
            successors[block].insert(successor);
            predecessors[successor].insert(block);
            incoming[successor].push(LogicalEdge {
                source: block,
                target: successor,
                role: edge.role,
                ordinal: edge.ordinal,
                executable: edge.executable,
                execution_role: edge.execution_role,
            });
        }
    }
    let mut reachable = BTreeSet::new();
    let mut pending = vec![0];
    while let Some(block) = pending.pop() {
        if reachable.insert(block) {
            pending.extend(successors[block].iter().copied());
        }
    }
    // The canonical dense name table and bitsets: each block boundary set
    // takes `names / 8` bytes. Name-keyed sets cost a heap string per name per
    // block, which drove the verifier past 7 GB on large functions (HF-96).
    let mut names = SimpleNameTable::for_ops(ops);
    for param in &function.params {
        names.intern(param);
    }
    let name_id = |name: &str| {
        names
            .id(name)
            .expect("SimpleIR name table holds every name")
    };
    let mut params = SimpleNameSet::empty(names.len());
    for param in &function.params {
        params.insert(name_id(param));
    }
    let generated: Vec<SimpleNameSet> = blocks
        .ranges
        .iter()
        .map(|(start, end)| {
            let mut set = SimpleNameSet::empty(names.len());
            for op in &ops[*start..=*end] {
                visit_simple_ir_defined_names(op, |name| set.insert(name_id(name)));
            }
            set
        })
        .collect();
    let mut universe = params.clone();
    for set in &generated {
        universe.union_with(set);
    }
    let mut definite_in = vec![universe.clone(); blocks.ranges.len()];
    let mut definite_out = vec![universe.clone(); blocks.ranges.len()];
    definite_in[0] = params.clone();
    definite_out[0] = params.clone();
    definite_out[0].union_with(&generated[0]);
    loop {
        let mut changed = false;
        for block in &reachable {
            let new_in = if *block == 0 {
                params.clone()
            } else {
                let mut pred_iter = predecessors[*block].intersection(&reachable);
                match pred_iter.next() {
                    None => SimpleNameSet::empty(names.len()),
                    Some(first) => {
                        let mut acc = definite_out[*first].clone();
                        for pred in pred_iter {
                            acc.intersect_with(&definite_out[*pred]);
                        }
                        acc
                    }
                }
            };
            let mut new_out = new_in.clone();
            new_out.union_with(&generated[*block]);
            if new_in != definite_in[*block] || new_out != definite_out[*block] {
                definite_in[*block] = new_in;
                definite_out[*block] = new_out;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for block in &reachable {
        let (start, end) = blocks.ranges[*block];
        let mut available = definite_in[*block].clone();
        let phi_edges = canonical_phi_edges(*block, &incoming, &reachable);
        for (index, op) in ops.iter().enumerate().take(end + 1).skip(start) {
            if simpleir_kind_is_verifier_phi(&op.kind) {
                let args = op.args.as_deref().unwrap_or_default();
                let collapsed = args.len() == 1 && !phi_edges.is_empty();
                if args.len() != phi_edges.len() && !collapsed {
                    errors.push(diagnostic(
                        &function.name,
                        index as isize,
                        "invalid-phi-arity",
                        format!(
                            "phi has {} inputs for {} canonical predecessors",
                            args.len(),
                            phi_edges.len()
                        ),
                    ));
                }
                for (edge_index, edge) in phi_edges.iter().enumerate() {
                    let Some(value) = args.get(if collapsed { 0 } else { edge_index }) else {
                        continue;
                    };
                    if !names
                        .id(value)
                        .is_some_and(|id| definite_out[edge.source].contains(id))
                    {
                        errors.push(diagnostic(
                            &function.name,
                            index as isize,
                            "non-dominating-phi-input",
                            format!(
                                "phi input {edge_index} value {value:?} is not defined on predecessor block starting at op #{}",
                                blocks.ranges[edge.source].0
                            ),
                        ));
                    }
                }
            } else {
                visit_simple_ir_reads(op, |read| {
                    if read.name == "none" {
                        return;
                    }
                    let id = name_id(read.name);
                    if available.contains(id) {
                        return;
                    }
                    let kind = if universe.contains(id) {
                        "non-dominating-definition"
                    } else {
                        "use-before-def"
                    };
                    errors.push(diagnostic(
                        &function.name,
                        index as isize,
                        kind,
                        format!(
                            "variable {:?} used by {:?} op has no definition on every reachable predecessor path",
                            read.name, op.kind
                        ),
                    ));
                });
            }
            visit_simple_ir_defined_names(op, |name| available.insert(name_id(name)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    fn verify(params: &[&str], ops: Vec<OpIR>) -> SimpleIrVerificationReport {
        verify_simple_ir(&SimpleIR {
            functions: vec![FunctionIR {
                return_abi: crate::FunctionReturnAbi::Value,
                name: "molt_main".to_string(),
                params: params.iter().map(|name| (*name).to_string()).collect(),
                ops,
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: ExecutionContextPolicy::None,
            }],
            profile: None,
        })
    }

    #[test]
    fn control_flow_admission_reuses_every_label_reference_family() {
        for kind in [
            "jump",
            "goto",
            "br_if",
            "try_start",
            "try_end",
            "check_exception",
            "async_work_poll",
        ] {
            for reference in [
                OpIR {
                    value: Some(19),
                    ..op(kind)
                },
                OpIR {
                    s_value: Some("alias".into()),
                    ..op(kind)
                },
            ] {
                let expected = if reference.value.is_some() {
                    "invalid-jump-target"
                } else {
                    "malformed-label-reference"
                };
                let ir = SimpleIR {
                    functions: vec![FunctionIR {
                        return_abi: crate::FunctionReturnAbi::Void,
                        name: "bad_graph".into(),
                        ops: vec![reference],
                        ..FunctionIR::default()
                    }],
                    profile: None,
                };
                let error = validate_simple_ir_control_flow(&ir).unwrap_err();
                assert!(error.contains(expected), "{kind}: {error}");
                assert!(
                    verify_simple_ir(&ir)
                        .errors
                        .iter()
                        .any(|error| error.kind == expected)
                );
            }
        }
    }

    #[test]
    fn control_flow_admission_keeps_valid_falloff_and_path_local_metadata() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                return_abi: crate::FunctionReturnAbi::Void,
                name: "falloff".into(),
                ops: vec![
                    OpIR {
                        value: Some(19),
                        ..op("try_start")
                    },
                    OpIR {
                        value: Some(19),
                        ..op("try_end")
                    },
                    OpIR {
                        value: Some(19),
                        ..op("try_end")
                    },
                    OpIR {
                        value: Some(19),
                        ..op("label")
                    },
                    OpIR {
                        value: Some(19),
                        ..op("check_exception")
                    },
                ],
                ..FunctionIR::default()
            }],
            profile: None,
        };
        validate_simple_ir_control_flow(&ir)
            .expect("graph admission is not a lexical TRY or module-completion check");
        assert!(
            verify_simple_ir(&ir)
                .errors
                .iter()
                .any(|error| error.kind == "missing-return"),
            "full module verification retains its stronger completion contract"
        );
    }

    #[test]
    fn completion_follows_executable_paths_not_the_last_lexical_op() {
        let branch = OpIR {
            args: Some(vec!["condition".into()]),
            ..op("if")
        };
        for (ops, missing) in [
            (
                vec![
                    branch.clone(),
                    op("ret_void"),
                    op("else"),
                    op("ret_void"),
                    op("end_if"),
                ],
                vec![],
            ),
            (vec![branch, op("ret_void"), op("end_if")], vec![2]),
            (vec![op("ret_void"), op("nop")], vec![]),
            (vec![op("loop_start"), op("loop_end")], vec![]),
            (
                vec![op("loop_start"), op("loop_continue"), op("loop_end")],
                vec![],
            ),
        ] {
            let report = verify(&["condition"], ops.clone());
            assert_eq!(
                report
                    .errors
                    .iter()
                    .map(|error| (error.kind.as_str(), error.op_index))
                    .collect::<Vec<_>>(),
                missing
                    .into_iter()
                    .map(|index| ("missing-return", index))
                    .collect::<Vec<_>>(),
                "{ops:?}: {report:?}",
            );
        }
    }

    #[test]
    fn completion_keeps_clipped_conditional_loop_and_exception_exits() {
        for kind in [
            "loop_break",
            "loop_break_if_true",
            "loop_break_if_false",
            "loop_break_if_exception",
        ] {
            let ops = vec![
                op("loop_start"),
                OpIR {
                    args: Some(vec!["condition".into()]),
                    ..op(kind)
                },
                op("loop_end"),
            ];
            let report = verify(&["condition"], ops);
            assert_eq!(report.errors.len(), 1, "{kind}: {report:?}");
            assert_eq!(report.errors[0].kind, "missing-return");
            assert_eq!(
                report.errors[0].op_index, 1,
                "break falls off before the lexical tail"
            );
        }
        for kind in ["br_if", "check_exception", "async_work_poll"] {
            let ops = vec![
                OpIR {
                    value: Some(5),
                    ..op("label")
                },
                OpIR {
                    value: Some(5),
                    args: Some(vec!["condition".into()]),
                    ..op(kind)
                },
            ];
            let flow = simple_ir_logical_flow(&ops);
            assert!(flow.edges[1].iter().any(|edge| edge.target == 0), "{kind}");
            assert!(
                flow.edges
                    .iter()
                    .flatten()
                    .all(|edge| edge.target < ops.len())
            );
            let report = verify(&["condition"], ops);
            assert_eq!(report.errors.len(), 1, "{kind}: {report:?}");
            assert_eq!(report.errors[0].kind, "missing-return");
            assert_eq!(report.errors[0].op_index, 1);
        }
    }

    #[test]
    fn completion_does_not_execute_try_registration_edges() {
        for (kind, missing) in [
            ("try_start", false),
            ("check_exception", true),
            ("async_work_poll", true),
        ] {
            let report = verify(
                &[],
                vec![
                    OpIR {
                        value: Some(9),
                        ..op(kind)
                    },
                    op("ret_void"),
                    OpIR {
                        value: Some(9),
                        ..op("label")
                    },
                ],
            );
            assert_eq!(
                report.errors.len(),
                usize::from(missing),
                "{kind}: {report:?}"
            );
            if missing {
                assert_eq!(report.errors[0].kind, "missing-return");
                assert_eq!(report.errors[0].op_index, 2);
            }
        }
    }

    #[test]
    fn suspension_ends_or_repolls_the_activation_without_dispatching() {
        for (kind, continues) in [("state_yield", false), ("state_transition", true)] {
            let suspend = OpIR {
                args: Some(vec!["value".into()]),
                value: Some(7),
                ..op(kind)
            };
            let ops = vec![suspend.clone(), op("ret_void")];
            let flow = simple_ir_logical_flow(&ops);
            assert_eq!(flow.edges[0].len(), usize::from(continues), "{kind}");
            if continues {
                assert_eq!(flow.edges[0][0].role, EdgeRole::Fallthrough);
                assert_eq!(flow.edges[0][0].target, 1);
            }
            let report = verify(&["value"], vec![suspend]);
            assert_eq!(
                report.errors.len(),
                usize::from(continues),
                "{kind}: {report:?}"
            );
            if continues {
                assert_eq!(report.errors[0].kind, "missing-return");
            }
        }
    }

    #[test]
    fn explicit_resume_maps_own_reachability_and_cfg_projection() {
        for (targets, expected) in [
            (Some(vec![(7, 200), (9, 200)]), vec![(4, 7), (4, 9)]),
            (Some(vec![]), vec![]),
            (None, vec![(2, 7)]),
        ] {
            let ops = vec![
                OpIR {
                    state_targets: targets,
                    ..op("state_switch")
                },
                op("ret_void"),
                OpIR {
                    value: Some(7),
                    ..op("state_label")
                },
                OpIR {
                    value: Some(7),
                    args: Some(vec!["value".into()]),
                    ..op("state_yield")
                },
                OpIR {
                    value: Some(200),
                    ..op("label")
                },
                op("nop"),
            ];
            let flow = simple_ir_logical_flow(&ops);
            assert_eq!(flow.edges[0][0].role, EdgeRole::DispatchDefault);
            assert_eq!(flow.edges[0][0].target, 1);
            assert_eq!(
                flow.edges[0]
                    .iter()
                    .filter(|edge| edge.role == EdgeRole::Resume)
                    .map(|edge| edge.target)
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|&(target, _)| target)
                    .collect::<Vec<_>>(),
            );
            assert!(
                flow.edges[3].is_empty(),
                "a yield does not dispatch or fall through"
            );
            let cfg = crate::tir::cfg::CFG::build(&ops);
            let projected: Vec<_> = cfg
                .state_resume_edges
                .iter()
                .map(|&(_, block, state)| (cfg.blocks[block].start_op, state))
                .collect();
            assert_eq!(
                projected, expected,
                "CFG must consume the same declared resume identity"
            );
            let report = verify(&["value"], ops);
            let has_resume_falloff = expected.iter().any(|&(target, _)| target == 4);
            assert_eq!(
                report.errors.len(),
                usize::from(has_resume_falloff),
                "{report:?}"
            );
            if has_resume_falloff {
                assert_eq!(report.errors[0].kind, "missing-return");
                assert_eq!(report.errors[0].op_index, 5);
            }
        }
    }

    #[test]
    fn dispatch_keeps_missing_default_and_source_resume_continuations() {
        for ops in [
            vec![OpIR {
                state_targets: Some(vec![]),
                ..op("state_switch")
            }],
            vec![
                op("state_switch"),
                OpIR {
                    value: Some(7),
                    args: Some(vec!["value".into()]),
                    ..op("state_yield")
                },
            ],
        ] {
            let flow = simple_ir_logical_flow(&ops);
            assert!(
                flow.edges
                    .iter()
                    .flatten()
                    .all(|edge| edge.target < ops.len())
            );
            let report = verify(&["value"], ops);
            assert_eq!(report.errors.len(), 1, "{report:?}");
            assert_eq!(report.errors[0].kind, "missing-return");
            assert_eq!(
                report.errors[0].op_index, 0,
                "dispatch exposes the missing invocation path"
            );
        }
    }

    #[test]
    fn source_resume_reentry_keeps_its_real_self_edge() {
        let ops = vec![
            OpIR {
                value: Some(7),
                ..op("state_label")
            },
            op("state_switch"),
            OpIR {
                value: Some(7),
                args: Some(vec!["value".into()]),
                ..op("state_yield")
            },
        ];
        let flow = simple_ir_logical_flow(&ops);
        assert!(
            flow.edges[1]
                .iter()
                .any(|edge| edge.role == EdgeRole::Resume && edge.target == 0)
        );
        let cfg = crate::tir::cfg::CFG::build(&ops);
        assert_eq!(cfg.state_resume_edges.len(), 1);
        let (dispatch, resume, state) = cfg.state_resume_edges[0];
        assert_eq!(
            dispatch, resume,
            "resumption can reenter the dispatch block"
        );
        assert_ne!(
            dispatch, cfg.entry,
            "invocation must remain a distinct predecessor"
        );
        assert_eq!(state, 7);
        let report = verify(&["value"], ops);
        assert!(
            report.is_ok(),
            "resumption loops without falling off: {report:?}"
        );
    }

    #[test]
    fn explicit_maps_reject_reachable_omitted_suspension_states() {
        for targets in [vec![], vec![(8, 70)], vec![(7, 70)]] {
            for slot in [None, Some("slot")] {
                let mut args = vec!["future".into()];
                args.extend(slot.map(str::to_string));
                args.push("pending".into());
                let ops = vec![
                    OpIR {
                        value: Some(7),
                        out: Some("pending".into()),
                        ..op("const")
                    },
                    OpIR {
                        state_targets: Some(targets.clone()),
                        ..op("state_switch")
                    },
                    OpIR {
                        value: Some(70),
                        ..op("label")
                    },
                    OpIR {
                        value: Some(8),
                        args: Some(args),
                        out: Some("polled".into()),
                        ..op("state_transition")
                    },
                    OpIR {
                        args: Some(vec!["polled".into()]),
                        ..op("ret")
                    },
                ];
                let admitted = targets.iter().any(|&(state, _)| state == 7);
                let validation = crate::ir_schema::validate_state_dispatch(&ops);
                assert_eq!(validation.is_ok(), admitted, "{validation:?}");
                if !admitted {
                    assert!(validation.unwrap_err().contains("saves state 7 absent"));
                }
                let report = verify(&["future", "slot"], ops);
                assert_eq!(report.is_ok(), admitted, "{report:?}");
                if !admitted {
                    assert_eq!(report.errors.len(), 1);
                    assert_eq!(report.errors[0].kind, "invalid-state-dispatch");
                }
            }
        }
        // A declared first resume case exposes a second suspension whose state
        // is missing. Checking only the initial invocation would miss this.
        let mut ops = vec![
            OpIR {
                state_targets: Some(vec![(1, 10)]),
                ..op("state_switch")
            },
            OpIR {
                value: Some(1),
                args: Some(vec!["value".into()]),
                ..op("state_yield")
            },
            OpIR {
                value: Some(10),
                ..op("state_label")
            },
            OpIR {
                value: Some(2),
                args: Some(vec!["value".into()]),
                ..op("state_yield")
            },
            OpIR {
                value: Some(20),
                ..op("label")
            },
            op("ret_void"),
        ];
        let error = crate::ir_schema::validate_state_dispatch(&ops).unwrap_err();
        assert!(
            error.contains("op#3: reachable state_yield saves state 2 absent"),
            "{error}"
        );
        ops[0].state_targets.as_mut().unwrap().push((2, 20));
        assert!(crate::ir_schema::validate_state_dispatch(&ops).is_ok());
        assert!(verify(&["value"], ops).is_ok());
    }

    #[test]
    fn empty_resume_maps_do_not_infer_cases_from_dead_or_running_state() {
        let switch = OpIR {
            state_targets: Some(vec![]),
            ..op("state_switch")
        };
        for ops in [
            vec![switch.clone(), op("ret_void")],
            vec![
                switch.clone(),
                OpIR {
                    value: Some(99),
                    ..op("state_set")
                },
                op("ret_void"),
            ],
            vec![
                switch.clone(),
                op("ret_void"),
                OpIR {
                    value: Some(7),
                    args: Some(vec!["value".into()]),
                    ..op("state_yield")
                },
            ],
        ] {
            assert!(crate::ir_schema::validate_state_dispatch(&ops).is_ok());
            let flow = simple_ir_logical_flow(&ops);
            assert!(
                !flow.edges[0]
                    .iter()
                    .any(|edge| edge.role == EdgeRole::Resume)
            );
            assert!(verify(&["value"], ops).is_ok());
        }
        for (kind, executable) in [("try_start", false), ("check_exception", true)] {
            let ops = vec![
                switch.clone(),
                OpIR {
                    value: Some(70),
                    ..op(kind)
                },
                op("ret_void"),
                OpIR {
                    value: Some(70),
                    ..op("label")
                },
                OpIR {
                    value: Some(7),
                    args: Some(vec!["value".into()]),
                    ..op("state_yield")
                },
            ];
            assert_eq!(
                crate::ir_schema::validate_state_dispatch(&ops).is_err(),
                executable
            );
        }
    }

    #[test]
    fn explicit_maps_require_known_saved_states_only_on_executable_suspensions() {
        for suspend in [
            OpIR {
                args: Some(vec!["value".into()]),
                ..op("state_yield")
            },
            OpIR {
                value: Some(8),
                args: Some(vec!["future".into(), "unknown_pending".into()]),
                out: Some("polled".into()),
                ..op("state_transition")
            },
        ] {
            for reachable in [false, true] {
                let mut ops = vec![OpIR {
                    state_targets: Some(vec![]),
                    ..op("state_switch")
                }];
                if !reachable {
                    ops.push(op("ret_void"));
                }
                ops.extend([suspend.clone(), op("ret_void")]);
                let result = crate::ir_schema::validate_state_dispatch(&ops);
                assert_eq!(result.is_err(), reachable);
                if reachable {
                    assert!(
                        result
                            .unwrap_err()
                            .contains("requires a statically known saved resume state")
                    );
                }
            }
        }
    }

    #[test]
    fn control_flow_admission_reuses_state_map_validation() {
        for (kind, targets) in [
            ("state_switch", vec![(7, 404)]),
            ("state_switch", vec![(7, 9), (7, 9)]),
            ("nop", vec![(7, 9)]),
        ] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "molt_main".into(),
                    ops: vec![
                        OpIR {
                            state_targets: Some(targets),
                            ..op(kind)
                        },
                        op("ret_void"),
                        OpIR {
                            value: Some(9),
                            ..op("label")
                        },
                        op("ret_void"),
                    ],
                    ..FunctionIR::default()
                }],
                profile: None,
            };
            assert!(
                validate_simple_ir_control_flow(&ir)
                    .unwrap_err()
                    .contains("invalid-state-dispatch")
            );
            assert!(
                verify_simple_ir(&ir)
                    .errors
                    .iter()
                    .any(|error| error.kind == "invalid-state-dispatch")
            );
        }
    }

    #[test]
    fn exception_phi_predecessors_are_explicit_transfers_not_try_region_blocks() {
        let mut try_start = op("try_start");
        try_start.value = Some(100);
        let mut branch = op("if");
        branch.args = Some(vec!["cond".to_string()]);
        let mut then_value = op("copy_var");
        then_value.var = Some("seed".to_string());
        then_value.out = Some("then_value".to_string());
        let mut else_value = op("copy_var");
        else_value.var = Some("seed".to_string());
        else_value.out = Some("else_value".to_string());
        let mut try_end = op("try_end");
        try_end.value = Some(100);
        let mut check = op("check_exception");
        check.value = Some(100);
        let mut poll = op("async_work_poll");
        poll.value = Some(100);
        let mut label = op("label");
        label.value = Some(100);
        let mut phi = op("phi");
        phi.args = Some(vec![
            "seed".to_string(),
            "seed".to_string(),
            "seed".to_string(),
        ]);
        phi.out = Some("handler_value".to_string());
        let mut handler_return = op("ret");
        handler_return.args = Some(vec!["handler_value".to_string()]);

        let report = verify(
            &["cond", "seed"],
            vec![
                try_start,
                branch,
                then_value,
                op("else"),
                else_value,
                op("end_if"),
                try_end,
                check,
                poll,
                op("ret_void"),
                label,
                phi,
                handler_return,
            ],
        );
        assert!(
            report.is_ok(),
            "only try_start, check_exception, and its async_work_poll alias may feed the handler phi: {report:?}"
        );
    }

    #[test]
    fn path_local_try_closes_do_not_pair_with_lexical_if_or_loop_regions() {
        let labelled = |kind: &str| OpIR {
            value: Some(100),
            ..op(kind)
        };
        let mut branch = op("if");
        branch.args = Some(vec!["condition".into()]);
        let report = verify(
            &["condition"],
            vec![
                labelled("try_start"),
                op("loop_start"),
                branch,
                labelled("try_end"),
                op("ret_void"),
                op("end_if"),
                labelled("try_end"),
                op("loop_break"),
                op("loop_end"),
                op("ret_void"),
                labelled("label"),
                labelled("try_end"),
                op("ret_void"),
            ],
        );
        assert!(
            report.is_ok(),
            "path-local metadata is not a lexical bracket: {report:?}"
        );
    }

    #[test]
    fn executable_flow_separates_metadata_and_phi_order_from_transfer_polarity() {
        let labelled = |kind: &str| OpIR {
            value: Some(100),
            ..op(kind)
        };
        let branch = OpIR {
            args: Some(vec!["condition".into()]),
            ..op("if")
        };
        let ops = vec![
            labelled("try_start"),
            op("loop_start"),
            branch,
            labelled("check_exception"),
            op("end_if"),
            op("loop_end"),
            labelled("label"),
            op("ret_void"),
        ];
        let analysis = op_flow(&ops).edges;
        assert!(
            analysis[0]
                .iter()
                .any(|edge| edge.role == EdgeRole::Exception && !edge.executable)
        );
        assert!(
            analysis[5]
                .iter()
                .any(|edge| edge.role == EdgeRole::LoopExit && !edge.executable)
        );
        let flow = simple_ir_logical_flow(&ops);
        assert_eq!(
            flow.edges[0].len(),
            1,
            "TRY metadata cannot observe pending state"
        );
        assert_eq!(flow.edges[5].len(), 1, "LOOP_END executes only its latch");
        assert_eq!(flow.edges[5][0].target, 1);
        assert!(
            flow.edges[3]
                .iter()
                .any(|edge| edge.role == EdgeRole::Normal && edge.target == 4)
        );
        assert!(
            flow.edges[3]
                .iter()
                .any(|edge| edge.role == EdgeRole::Exception && edge.target == 6)
        );
    }

    #[test]
    fn logical_flow_liveness_keeps_only_values_crossing_executable_blocks() {
        let ops = vec![
            OpIR {
                value: Some(1),
                out: Some("live".into()),
                ..op("const_int")
            },
            OpIR {
                value: Some(2),
                out: Some("local_only".into()),
                ..op("const_int")
            },
            OpIR {
                value: Some(100),
                ..op("check_exception")
            },
            OpIR {
                args: Some(vec!["live".into()]),
                ..op("ret")
            },
            OpIR {
                value: Some(100),
                ..op("label")
            },
            OpIR {
                args: Some(vec!["live".into()]),
                ..op("ret")
            },
        ];
        let flow = simple_ir_logical_flow(&ops);
        assert_eq!(
            flow.cross_block_values,
            BTreeSet::from(["live".to_string()])
        );
    }

    #[test]
    fn logical_flow_liveness_retains_values_across_a_same_block_backedge() {
        let ops = vec![
            OpIR {
                value: Some(10),
                ..op("label")
            },
            OpIR {
                var: Some("carried".into()),
                out: Some("local".into()),
                ..op("load_var")
            },
            OpIR {
                var: Some("carried".into()),
                args: Some(vec!["local".into()]),
                ..op("store_var")
            },
            OpIR {
                value: Some(10),
                ..op("jump")
            },
        ];
        let flow = simple_ir_logical_flow(&ops);
        assert_eq!(flow.blocks, vec![(0, 3)]);
        assert_eq!(
            flow.cross_block_values,
            BTreeSet::from(["carried".to_string()])
        );
    }

    #[test]
    fn path_local_try_metadata_does_not_relax_lexical_bracket_validation() {
        for (open, close) in [("if", "loop_end"), ("loop_start", "end_if")] {
            let mut opener = op(open);
            if open == "if" {
                opener.args = Some(vec!["condition".into()]);
            }
            let report = verify(
                &["condition"],
                vec![
                    OpIR {
                        value: Some(100),
                        ..op("try_start")
                    },
                    opener,
                    OpIR {
                        value: Some(100),
                        ..op("try_end")
                    },
                    op(close),
                    OpIR {
                        value: Some(100),
                        ..op("label")
                    },
                    op("ret_void"),
                ],
            );
            assert!(
                report
                    .errors
                    .iter()
                    .any(|error| error.kind == "unbalanced-control-flow"),
                "removing metadata brackets must still reject mismatched {open}/{close}: {report:?}"
            );
        }
    }

    #[test]
    fn path_local_try_metadata_still_requires_a_defined_handler_label() {
        for kind in ["try_start", "try_end"] {
            for (value, diagnostic_kind) in [
                (None, "malformed-label-reference"),
                (Some(100), "invalid-jump-target"),
            ] {
                let report = verify(&[], vec![OpIR { value, ..op(kind) }, op("ret_void")]);
                assert!(
                    report
                        .errors
                        .iter()
                        .any(|error| error.kind == diagnostic_kind),
                    "{report:?}"
                );
                assert!(
                    !report
                        .errors
                        .iter()
                        .any(|error| error.kind == "unbalanced-control-flow"),
                    "{report:?}"
                );
            }
        }
    }

    #[test]
    fn branch_local_definition_does_not_dominate_join() {
        let mut branch = op("if");
        branch.args = Some(vec!["condition".to_string()]);
        let mut local = op("const");
        local.value = Some(1);
        local.out = Some("branch_local".to_string());
        let mut ret = op("ret");
        ret.args = Some(vec!["branch_local".to_string()]);

        let report = verify(&["condition"], vec![branch, local, op("end_if"), ret]);
        assert_eq!(
            report
                .errors
                .iter()
                .map(|diagnostic| diagnostic.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["non-dominating-definition"]
        );
    }

    fn error_kinds(report: &SimpleIrVerificationReport) -> Vec<&str> {
        report
            .errors
            .iter()
            .map(|diagnostic| diagnostic.kind.as_str())
            .collect()
    }

    #[test]
    fn read_of_a_name_nothing_defines_is_use_before_def() {
        let mut ret = op("ret");
        ret.args = Some(vec!["ghost".to_string()]);

        let report = verify(&[], vec![ret]);
        assert_eq!(error_kinds(&report), vec!["use-before-def"]);
    }

    #[test]
    fn definitions_on_every_branch_reach_the_join() {
        let mut branch = op("if");
        branch.args = Some(vec!["condition".to_string()]);
        let mut left = op("const");
        left.value = Some(1);
        left.out = Some("joined".to_string());
        let mut right = op("const");
        right.value = Some(2);
        right.out = Some("joined".to_string());
        let mut ret = op("ret");
        ret.args = Some(vec!["joined".to_string()]);

        let report = verify(
            &["condition", "unused_param"],
            vec![branch, left, op("else"), right, op("end_if"), ret],
        );
        assert_eq!(error_kinds(&report), Vec::<&str>::new());
    }

    #[test]
    fn parameter_reads_need_no_defining_op() {
        let mut ret = op("ret");
        ret.args = Some(vec!["param".to_string()]);

        let report = verify(&["param"], vec![ret]);
        assert_eq!(error_kinds(&report), Vec::<&str>::new());
    }

    #[test]
    fn phi_inputs_follow_semantic_branch_order() {
        let mut branch = op("if");
        branch.args = Some(vec!["condition".to_string()]);
        let mut left = op("const");
        left.value = Some(1);
        left.out = Some("left".to_string());
        let mut right = op("const");
        right.value = Some(2);
        right.out = Some("right".to_string());
        let mut phi = op("phi");
        phi.args = Some(vec!["left".to_string(), "right".to_string()]);
        phi.out = Some("merged".to_string());
        let mut ret = op("ret");
        ret.args = Some(vec!["merged".to_string()]);
        let prefix = vec![branch, left, op("else"), right, op("end_if")];

        let mut valid = prefix.clone();
        valid.extend([phi.clone(), ret.clone()]);
        assert!(verify(&["condition"], valid).is_ok());

        phi.args = Some(vec!["right".to_string(), "left".to_string()]);
        let mut invalid = prefix;
        invalid.extend([phi, ret]);
        assert_eq!(
            verify(&["condition"], invalid)
                .errors
                .iter()
                .filter(|diagnostic| diagnostic.kind == "non-dominating-phi-input")
                .count(),
            2
        );
    }
}
