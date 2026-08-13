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
use crate::tir::op_kinds_generated::{
    SimpleIrCallTargetRole, SimpleIrVerifierRegionRole, simpleir_call_target_role,
    simpleir_kind_is_repoll, simpleir_kind_is_return_terminator, simpleir_kind_is_suspend,
    simpleir_kind_is_terminator, simpleir_kind_is_verifier_label_definition,
    simpleir_kind_is_verifier_label_reference, simpleir_kind_is_verifier_loop_scoped,
    simpleir_kind_is_verifier_phi, simpleir_kind_is_wasm_stateful_dispatch,
    simpleir_verifier_region_role,
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
struct LogicalEdge {
    source: usize,
    target: usize,
    role: EdgeRole,
    ordinal: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EdgeRole {
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
struct BasicBlocks {
    ranges: Vec<(usize, usize)>,
    op_to_block: Vec<usize>,
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
    verify_definite_definitions(function, errors);
    verify_function_references(function, function_names, errors);
    verify_block_structure(function, errors);
    verify_labels(function, errors);
    if !simpleir_kind_is_return_terminator(&ops[ops.len() - 1].kind) {
        errors.push(diagnostic(
            &function.name,
            (ops.len() - 1) as isize,
            "missing-return",
            format!(
                "function does not end with ret/ret_void (last op is {:?})",
                ops[ops.len() - 1].kind
            ),
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
                format!("jump/check_exception targets undefined label {target}"),
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

fn op_edges(ops: &[OpIR]) -> Vec<Vec<LogicalEdge>> {
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
    let state_labels: BTreeMap<i64, usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            (op.kind == "state_label")
                .then_some(op.value.map(|v| (v, index)))
                .flatten()
        })
        .collect();
    let const_values: BTreeMap<&str, i64> = ops
        .iter()
        .filter_map(|op| {
            (op.kind == "const")
                .then_some(op.out.as_deref().zip(op.value))
                .flatten()
        })
        .collect();
    let mut state_entries = BTreeSet::new();
    for (index, op) in ops.iter().enumerate() {
        if simpleir_kind_is_suspend(&op.kind) && !simpleir_kind_is_repoll(&op.kind) {
            if let Some(state) = op.value {
                state_entries.insert(*state_labels.get(&state).unwrap_or(&(index + 1)));
            }
        } else if simpleir_kind_is_repoll(&op.kind)
            && let Some(pending_name) = op.args.as_deref().and_then(|args| args.last())
            && let Some(state) = const_values.get(pending_name.as_str())
            && let Some(entry) = state_labels.get(state)
        {
            state_entries.insert(*entry);
        }
    }
    let if_by_end: BTreeMap<usize, (usize, Option<usize>)> = targets
        .if_regions
        .iter()
        .map(|(start, (alternate, end))| (*end, (*start, *alternate)))
        .collect();
    let mut edges = vec![Vec::new(); count];
    let mut add = |source: usize, target: usize, mut role: EdgeRole| {
        if target >= count {
            return;
        }
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
                    add(index, end, EdgeRole::BranchTrue);
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
            "check_exception" | "async_work_poll" => {
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
            kind if simpleir_kind_is_wasm_stateful_dispatch(kind) => {
                add(index, next, EdgeRole::DispatchDefault);
                for entry in &state_entries {
                    add(index, *entry, EdgeRole::Resume);
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
    edges
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

fn exception_block_edges(ops: &[OpIR], blocks: &BasicBlocks) -> BTreeSet<(usize, usize)> {
    let labels: BTreeMap<i64, usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            (simpleir_kind_is_verifier_label_definition(&op.kind))
                .then_some(op.value.map(|v| (v, index)))
                .flatten()
        })
        .collect();
    let mut active = Vec::new();
    let mut regions = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        match simpleir_verifier_region_role(&op.kind) {
            Some(("try", SimpleIrVerifierRegionRole::Start)) => {
                if let Some(handler) = op.value.and_then(|value| labels.get(&value).copied()) {
                    active.push((index, handler));
                }
            }
            Some(("try", SimpleIrVerifierRegionRole::End)) => {
                if let Some((start, handler)) = active.pop() {
                    regions.push((start, index, handler));
                }
            }
            _ => {}
        }
    }
    regions.extend(
        active
            .into_iter()
            .map(|(start, handler)| (start, ops.len() - 1, handler)),
    );
    let mut result = BTreeSet::new();
    for (start, end, handler_op) in regions {
        let handler_block = blocks.op_to_block[handler_op];
        for (block, (block_start, block_end)) in blocks.ranges.iter().enumerate() {
            if *block_end >= start && *block_start <= end && block != handler_block {
                result.insert((block, handler_block));
            }
        }
    }
    result
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

fn definitions(op: &OpIR) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    visit_simple_ir_defined_names(op, |name| {
        result.insert(name.to_string());
    });
    result
}

fn verify_definite_definitions(function: &FunctionIR, errors: &mut Vec<SimpleIrDiagnostic>) {
    let ops = &function.ops;
    let edges = op_edges(ops);
    let blocks = basic_blocks(ops, &edges);
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
            });
        }
    }
    for (source, handler) in exception_block_edges(ops, &blocks) {
        successors[source].insert(handler);
        predecessors[handler].insert(source);
        let ordinal = incoming[handler].len();
        incoming[handler].push(LogicalEdge {
            source,
            target: handler,
            role: EdgeRole::Exception,
            ordinal,
        });
    }
    let mut reachable = BTreeSet::new();
    let mut pending = vec![0];
    while let Some(block) = pending.pop() {
        if reachable.insert(block) {
            pending.extend(successors[block].iter().copied());
        }
    }
    let params: BTreeSet<String> = function.params.iter().cloned().collect();
    let mut universe = params.clone();
    for op in ops {
        universe.extend(definitions(op));
    }
    let generated: Vec<BTreeSet<String>> = blocks
        .ranges
        .iter()
        .map(|(start, end)| ops[*start..=*end].iter().flat_map(definitions).collect())
        .collect();
    let mut definite_in = vec![universe.clone(); blocks.ranges.len()];
    let mut definite_out = vec![universe.clone(); blocks.ranges.len()];
    definite_in[0] = params.clone();
    definite_out[0] = params.union(&generated[0]).cloned().collect();
    loop {
        let mut changed = false;
        for block in &reachable {
            let new_in = if *block == 0 {
                params.clone()
            } else {
                let mut pred_iter = predecessors[*block].intersection(&reachable);
                match pred_iter.next() {
                    None => BTreeSet::new(),
                    Some(first) => pred_iter.fold(definite_out[*first].clone(), |acc, pred| {
                        acc.intersection(&definite_out[*pred]).cloned().collect()
                    }),
                }
            };
            let new_out = new_in.union(&generated[*block]).cloned().collect();
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
                    if !definite_out[edge.source].contains(value) {
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
                    if read.name == "none" || available.contains(read.name) {
                        return;
                    }
                    let kind = if universe.contains(read.name) {
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
            available.extend(definitions(op));
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
                name: "molt_main".to_string(),
                params: params.iter().map(|name| (*name).to_string()).collect(),
                ops,
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: ExecutionContextPolicy::None,
            }],
            profile: None,
        })
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
