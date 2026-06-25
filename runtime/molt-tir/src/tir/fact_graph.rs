//! Value-level fact provenance for the semantic control plane.
//!
//! The fact graph is a read-only projection over live TIR state. It does not
//! decide type, carrier, call-target, or call-site facts itself; it records where
//! those facts came from and who consumes the value. This keeps doc 46's
//! discovery-vs-authority rule intact: `TirFunction::value_types` owns semantic
//! type facts, `Repr::default_for` owns the conservative carrier floor, and
//! `CallFactsTable` owns call-site facts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::blocks::{BlockId, Terminator};
use super::call_facts::{
    CallFacts, CallFactsTable, CallTargetFact, FactValue, InlineEligibility, InlineWhyNot,
};
use super::function::TirFunction;
use super::op_kinds_generated::{
    ExplicitReleaseOperands, RefcountBalanceRole, opcode_explicit_release_operands_table,
    opcode_is_escape_alloc_site_table, opcode_is_refcount_heap_exposure_table,
    opcode_refcount_balance_role_table,
};
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;
use crate::repr::Repr;

pub const FACT_GRAPH_SCHEMA_VERSION: u32 = 2;
pub const FACT_GRAPH_KIND: &str = "molt_tir_fact_graph";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactGraph {
    pub schema_version: u32,
    pub kind: String,
    pub function: String,
    pub values: Vec<ValueFactNode>,
    pub edges: Vec<FactEdge>,
    pub summary: FactGraphSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueFactNode {
    pub value: u32,
    pub producer: Option<FactProducer>,
    pub facts: Vec<FactEntry>,
    pub consumers: Vec<FactConsumer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactProducer {
    pub kind: String,
    pub block: Option<u32>,
    pub op_index: Option<usize>,
    pub opcode: Option<String>,
    pub result_index: Option<usize>,
    pub param_index: Option<usize>,
    pub source_site: Option<FactSourceSite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactConsumer {
    pub kind: String,
    pub block: Option<u32>,
    pub op_index: Option<usize>,
    pub opcode: Option<String>,
    pub operand_index: Option<usize>,
    pub role: String,
    pub source_site: Option<FactSourceSite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactEntry {
    pub kind: String,
    pub value: String,
    pub confidence: String,
    pub producer: String,
    pub event_id: Option<String>,
    pub source_site: Option<FactSourceSite>,
    pub guards: Vec<String>,
    pub invalidators: Vec<String>,
    pub backend_lowering_status: String,
    pub test_coverage: String,
    pub perf_relevance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactEdge {
    pub from_value: u32,
    pub to_value: Option<u32>,
    pub kind: String,
    pub consumer: FactConsumer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactGraphSummary {
    pub value_count: usize,
    pub fact_count: usize,
    pub edge_count: usize,
    pub call_fact_count: usize,
    pub source_site_value_count: usize,
    pub allocation_ownership_fact_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FactSourceSite {
    pub line: u32,
    pub col: Option<u32>,
    pub end_col: Option<u32>,
}

#[derive(Default)]
struct NodeBuilder {
    producer: Option<FactProducer>,
    facts: Vec<FactEntry>,
    consumers: Vec<FactConsumer>,
}

impl FactGraph {
    /// Build a graph from the fail-closed local `CallFactsTable` floor.
    pub fn build_local(func: &TirFunction) -> FactGraph {
        let call_facts = CallFactsTable::build_local(func);
        Self::build_with_call_facts(func, &call_facts, "CallFactsTable::build_local")
    }

    /// Build a graph using the caller-supplied call-fact table.
    ///
    /// Module phase code can pass the precise interprocedural table and name the
    /// source accordingly; local/debug callers can pass `build_local`. This keeps
    /// graph construction from recomputing or weakening call-fact authority.
    pub fn build_with_call_facts(
        func: &TirFunction,
        call_facts: &CallFactsTable,
        call_fact_source: &str,
    ) -> FactGraph {
        let mut nodes: BTreeMap<u32, NodeBuilder> = BTreeMap::new();
        let mut edges = Vec::new();

        for (block_id, block) in sorted_blocks(func) {
            for (arg_index, arg) in block.args.iter().enumerate() {
                let kind = if block_id == func.entry_block && arg_index < func.param_types.len() {
                    "parameter"
                } else {
                    "block_arg"
                };
                let producer = FactProducer {
                    kind: kind.to_string(),
                    block: Some(block_id.0),
                    op_index: None,
                    opcode: None,
                    result_index: None,
                    param_index: (kind == "parameter").then_some(arg_index),
                    source_site: None,
                };
                ensure_node(&mut nodes, arg.id)
                    .producer
                    .get_or_insert(producer);
                add_value_type_facts(&mut nodes, arg.id, &arg.ty, "block_arg");
            }

            for (op_index, op) in block.ops.iter().enumerate() {
                add_op_consumers(&mut nodes, &mut edges, block_id, op_index, op);
                add_op_ownership_facts(&mut nodes, &func.name, block_id, op_index, op);
                for (result_index, result) in op.results.iter().copied().enumerate() {
                    let producer = FactProducer {
                        kind: "op_result".to_string(),
                        block: Some(block_id.0),
                        op_index: Some(op_index),
                        opcode: Some(format!("{:?}", op.opcode)),
                        result_index: Some(result_index),
                        param_index: None,
                        source_site: fact_source_site(op),
                    };
                    ensure_node(&mut nodes, result).producer = Some(producer);
                    if let Some(ty) = func.value_types.get(&result) {
                        add_value_type_facts(&mut nodes, result, ty, "function.value_types");
                    }
                    add_result_ownership_facts(
                        &mut nodes,
                        &func.name,
                        block_id,
                        op_index,
                        result_index,
                        result,
                        op,
                    );
                    if result_index == 0
                        && let Some(facts) = call_facts.get(result)
                    {
                        let event_prefix = op_event_prefix(
                            &func.name,
                            block_id,
                            op_index,
                            op,
                            &format!("result{result_index}"),
                        );
                        add_call_facts(
                            &mut nodes,
                            result,
                            facts,
                            call_fact_source,
                            fact_source_site(op),
                            &event_prefix,
                        );
                    }
                }
            }

            add_terminator_consumers(&mut nodes, &mut edges, block_id, &block.terminator);
        }

        for (value, ty) in sorted_value_types(func) {
            add_value_type_facts(&mut nodes, value, ty, "function.value_types");
        }

        let values: Vec<ValueFactNode> = nodes
            .into_iter()
            .map(|(value, node)| ValueFactNode {
                value,
                producer: node.producer,
                facts: dedupe_facts(node.facts),
                consumers: dedupe_consumers(node.consumers),
            })
            .collect();
        let fact_count = values.iter().map(|v| v.facts.len()).sum();
        let call_fact_count = values
            .iter()
            .flat_map(|v| &v.facts)
            .filter(|f| f.kind.starts_with("call."))
            .count();
        let source_site_value_count = values
            .iter()
            .filter(|v| {
                v.producer
                    .as_ref()
                    .and_then(|producer| producer.source_site)
                    .is_some()
                    || v.consumers
                        .iter()
                        .any(|consumer| consumer.source_site.is_some())
                    || v.facts.iter().any(|fact| fact.source_site.is_some())
            })
            .count();
        let allocation_ownership_fact_count = values
            .iter()
            .flat_map(|v| &v.facts)
            .filter(|f| f.kind.starts_with("allocation.") || f.kind.starts_with("ownership."))
            .count();
        let edge_count = edges.len();
        let value_count = values.len();
        FactGraph {
            schema_version: FACT_GRAPH_SCHEMA_VERSION,
            kind: FACT_GRAPH_KIND.to_string(),
            function: func.name.clone(),
            values,
            edges,
            summary: FactGraphSummary {
                value_count,
                fact_count,
                edge_count,
                call_fact_count,
                source_site_value_count,
                allocation_ownership_fact_count,
            },
        }
    }

    pub fn to_pretty_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

fn sorted_blocks(func: &TirFunction) -> Vec<(BlockId, &super::blocks::TirBlock)> {
    let mut blocks: Vec<_> = func.blocks.iter().map(|(&id, block)| (id, block)).collect();
    blocks.sort_by_key(|(id, _)| id.0);
    blocks
}

fn sorted_value_types(func: &TirFunction) -> Vec<(ValueId, &TirType)> {
    let mut values: Vec<_> = func.value_types.iter().map(|(&id, ty)| (id, ty)).collect();
    values.sort_by_key(|(id, _)| id.0);
    values
}

fn ensure_node(nodes: &mut BTreeMap<u32, NodeBuilder>, value: ValueId) -> &mut NodeBuilder {
    nodes.entry(value.0).or_default()
}

fn fact_source_site(op: &TirOp) -> Option<FactSourceSite> {
    op.source_site().map(|site| FactSourceSite {
        line: site.line,
        col: site.col,
        end_col: site.end_col,
    })
}

fn op_event_prefix(
    function_name: &str,
    block_id: BlockId,
    op_index: usize,
    op: &TirOp,
    subject: &str,
) -> String {
    format!(
        "{function_name}:bb{}:op{}:{:?}:{subject}",
        block_id.0, op_index, op.opcode
    )
}

fn fact_event_id(event_prefix: &str, kind: &str) -> String {
    format!("{event_prefix}:{kind}")
}

fn add_value_type_facts(
    nodes: &mut BTreeMap<u32, NodeBuilder>,
    value: ValueId,
    ty: &TirType,
    producer: &str,
) {
    let node = ensure_node(nodes, value);
    node.facts.push(FactEntry {
        kind: "tir_type".to_string(),
        value: format!("{ty:?}"),
        confidence: "proven".to_string(),
        producer: producer.to_string(),
        event_id: None,
        source_site: None,
        guards: Vec::new(),
        invalidators: vec!["value_types".to_string(), "ops".to_string()],
        backend_lowering_status: "type-guides-representation".to_string(),
        test_coverage: "tir::function value_types tests + fact_graph tests".to_string(),
        perf_relevance: "typed values are the root of raw-vs-boxed decisions".to_string(),
    });
    node.facts.push(FactEntry {
        kind: "repr_floor".to_string(),
        value: format!("{:?}", Repr::default_for(ty)),
        confidence: "proven_floor".to_string(),
        producer: "Repr::default_for(TirType)".to_string(),
        event_id: None,
        source_site: None,
        guards: Vec::new(),
        invalidators: vec!["value_types".to_string(), "repr_lattice".to_string()],
        backend_lowering_status: "conservative-carrier-floor".to_string(),
        test_coverage: "repr::default_for + fact_graph tests".to_string(),
        perf_relevance: "explains why a typed value still lowers as boxed or BigInt-safe"
            .to_string(),
    });
}

fn add_op_consumers(
    nodes: &mut BTreeMap<u32, NodeBuilder>,
    edges: &mut Vec<FactEdge>,
    block_id: BlockId,
    op_index: usize,
    op: &TirOp,
) {
    for (operand_index, operand) in op.operands.iter().copied().enumerate() {
        let consumer = FactConsumer {
            kind: "op_operand".to_string(),
            block: Some(block_id.0),
            op_index: Some(op_index),
            opcode: Some(format!("{:?}", op.opcode)),
            operand_index: Some(operand_index),
            role: format!("operand[{operand_index}]"),
            source_site: fact_source_site(op),
        };
        ensure_node(nodes, operand).consumers.push(consumer.clone());
        for result in &op.results {
            edges.push(FactEdge {
                from_value: operand.0,
                to_value: Some(result.0),
                kind: "op_operand_to_result".to_string(),
                consumer: consumer.clone(),
            });
        }
    }
}

fn add_result_ownership_facts(
    nodes: &mut BTreeMap<u32, NodeBuilder>,
    function_name: &str,
    block_id: BlockId,
    op_index: usize,
    result_index: usize,
    result: ValueId,
    op: &TirOp,
) {
    let source_site = fact_source_site(op);
    let event_prefix = op_event_prefix(
        function_name,
        block_id,
        op_index,
        op,
        &format!("result{result_index}"),
    );
    if opcode_is_escape_alloc_site_table(op.opcode) {
        add_allocation_fact(
            nodes,
            result,
            "allocation.heap_root",
            "escape_alloc_site",
            fact_event_id(&event_prefix, "allocation.heap_root"),
            source_site,
            "op_kinds.escape_alloc_site_opcodes",
            "fresh heap roots drive escape analysis and stack promotion",
        );
    }
    if matches!(op.opcode, OpCode::StackAlloc | OpCode::ObjectNewBoundStack) {
        add_allocation_fact(
            nodes,
            result,
            "allocation.stack_root",
            "stack_alloc_site",
            fact_event_id(&event_prefix, "allocation.stack_root"),
            source_site,
            "op_kind + escape_analysis stack promotion",
            "stack roots remove heap allocation and refcount traffic",
        );
    }
    if matches!(op.attrs.get("arena_eligible"), Some(AttrValue::Bool(true))) {
        add_allocation_fact(
            nodes,
            result,
            "allocation.arena_eligible",
            "true",
            fact_event_id(&event_prefix, "allocation.arena_eligible"),
            source_site,
            "escape_analysis",
            "arena placement amortizes allocation/free overhead",
        );
    }
    if matches!(op.attrs.get("defines_del"), Some(AttrValue::Bool(true))) {
        add_allocation_fact(
            nodes,
            result,
            "ownership.finalizer_sensitive",
            "true",
            fact_event_id(&event_prefix, "ownership.finalizer_sensitive"),
            source_site,
            "frontend class MRO + escape_analysis",
            "finalizer-sensitive roots constrain stack promotion and drop order",
        );
    }
}

fn add_op_ownership_facts(
    nodes: &mut BTreeMap<u32, NodeBuilder>,
    function_name: &str,
    block_id: BlockId,
    op_index: usize,
    op: &TirOp,
) {
    let source_site = fact_source_site(op);
    let balance = opcode_refcount_balance_role_table(op.opcode);
    if balance.is_refcount_balance() {
        let value = match balance {
            RefcountBalanceRole::Increment => "increment",
            RefcountBalanceRole::Decrement => "decrement",
            RefcountBalanceRole::NotRefcountBalance => unreachable!(),
        };
        for (operand_index, operand) in op.operands.iter().copied().enumerate() {
            let event_prefix = op_event_prefix(
                function_name,
                block_id,
                op_index,
                op,
                &format!("operand{operand_index}"),
            );
            add_allocation_fact(
                nodes,
                operand,
                "ownership.refcount_balance",
                value,
                fact_event_id(&event_prefix, "ownership.refcount_balance"),
                source_site,
                "op_kinds.refcount_balance_*_opcodes",
                "refcount balance events explain retained and released ownership",
            );
        }
    }

    if opcode_is_refcount_heap_exposure_table(op.opcode) {
        for (operand_index, operand) in op.operands.iter().copied().enumerate() {
            let event_prefix = op_event_prefix(
                function_name,
                block_id,
                op_index,
                op,
                &format!("operand{operand_index}"),
            );
            add_allocation_fact(
                nodes,
                operand,
                "ownership.heap_exposure",
                "true",
                fact_event_id(&event_prefix, "ownership.heap_exposure"),
                source_site,
                "op_kinds.refcount_heap_exposure_opcodes",
                "heap exposure blocks deferred RC elimination and stack-only assumptions",
            );
        }
    }

    match opcode_explicit_release_operands_table(op.opcode, op.operands.len()) {
        ExplicitReleaseOperands::None => {}
        ExplicitReleaseOperands::All => {
            for (operand_index, operand) in op.operands.iter().copied().enumerate() {
                let event_prefix = op_event_prefix(
                    function_name,
                    block_id,
                    op_index,
                    op,
                    &format!("operand{operand_index}"),
                );
                add_allocation_fact(
                    nodes,
                    operand,
                    "ownership.explicit_release",
                    format!("operand[{operand_index}]"),
                    fact_event_id(&event_prefix, "ownership.explicit_release"),
                    source_site,
                    "op_kinds.explicit_release_operands",
                    "explicit releases are terminal ownership events",
                );
            }
        }
        ExplicitReleaseOperands::One(index) => {
            if let Some(operand) = op.operands.get(index) {
                let event_prefix = op_event_prefix(
                    function_name,
                    block_id,
                    op_index,
                    op,
                    &format!("operand{index}"),
                );
                add_allocation_fact(
                    nodes,
                    *operand,
                    "ownership.explicit_release",
                    format!("operand[{index}]"),
                    fact_event_id(&event_prefix, "ownership.explicit_release"),
                    source_site,
                    "op_kinds.explicit_release_operands",
                    "explicit releases are terminal ownership events",
                );
            }
        }
    }
}

fn add_allocation_fact(
    nodes: &mut BTreeMap<u32, NodeBuilder>,
    value: ValueId,
    kind: &str,
    value_text: impl Into<String>,
    event_id: String,
    source_site: Option<FactSourceSite>,
    producer: &str,
    perf_relevance: &str,
) {
    ensure_node(nodes, value).facts.push(FactEntry {
        kind: kind.to_string(),
        value: value_text.into(),
        confidence: "proven".to_string(),
        producer: producer.to_string(),
        event_id: Some(event_id),
        source_site,
        guards: Vec::new(),
        invalidators: vec![
            "op_kinds.toml".to_string(),
            "ops".to_string(),
            "ownership_lattice".to_string(),
        ],
        backend_lowering_status: "diagnostic-projection-of-existing-tir-facts".to_string(),
        test_coverage: "fact_graph allocation/source-site tests".to_string(),
        perf_relevance: perf_relevance.to_string(),
    });
}

fn add_terminator_consumers(
    nodes: &mut BTreeMap<u32, NodeBuilder>,
    edges: &mut Vec<FactEdge>,
    block_id: BlockId,
    term: &Terminator,
) {
    for (role, value) in terminator_values(term) {
        let consumer = FactConsumer {
            kind: "terminator_operand".to_string(),
            block: Some(block_id.0),
            op_index: None,
            opcode: None,
            operand_index: None,
            role,
            source_site: None,
        };
        ensure_node(nodes, value).consumers.push(consumer.clone());
        edges.push(FactEdge {
            from_value: value.0,
            to_value: None,
            kind: "terminator_use".to_string(),
            consumer,
        });
    }
}

fn terminator_values(term: &Terminator) -> Vec<(String, ValueId)> {
    let mut out = Vec::new();
    match term {
        Terminator::Branch { args, .. } => {
            for (idx, value) in args.iter().copied().enumerate() {
                out.push((format!("branch_arg[{idx}]"), value));
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            out.push(("cond".to_string(), *cond));
            for (idx, value) in then_args.iter().copied().enumerate() {
                out.push((format!("then_arg[{idx}]"), value));
            }
            for (idx, value) in else_args.iter().copied().enumerate() {
                out.push((format!("else_arg[{idx}]"), value));
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            out.push(("switch_value".to_string(), *value));
            for (case_idx, (_case, _target, args)) in cases.iter().enumerate() {
                for (arg_idx, value) in args.iter().copied().enumerate() {
                    out.push((format!("case[{case_idx}].arg[{arg_idx}]"), value));
                }
            }
            for (idx, value) in default_args.iter().copied().enumerate() {
                out.push((format!("default_arg[{idx}]"), value));
            }
        }
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (case_idx, (_state, _target, args)) in cases.iter().enumerate() {
                for (arg_idx, value) in args.iter().copied().enumerate() {
                    out.push((format!("state_case[{case_idx}].arg[{arg_idx}]"), value));
                }
            }
            for (idx, value) in default_args.iter().copied().enumerate() {
                out.push((format!("state_default_arg[{idx}]"), value));
            }
        }
        Terminator::Return { values } => {
            for (idx, value) in values.iter().copied().enumerate() {
                out.push((format!("return[{idx}]"), value));
            }
        }
        Terminator::Unreachable => {}
    }
    out
}

fn add_call_facts(
    nodes: &mut BTreeMap<u32, NodeBuilder>,
    value: ValueId,
    facts: &CallFacts,
    producer: &str,
    source_site: Option<FactSourceSite>,
    event_prefix: &str,
) {
    let node = ensure_node(nodes, value);
    node.facts.push(call_fact_entry(
        "call.target",
        call_target_value(&facts.target),
        call_target_confidence(&facts.target),
        producer,
        source_site,
        event_prefix,
        "direct target enables devirtualized calls and removes generic helper traffic",
    ));
    node.facts.push(call_fact_entry(
        "call.typed_return",
        facts
            .typed_return
            .map(|repr| format!("{repr:?}"))
            .unwrap_or_else(|| "unknown_or_dynbox".to_string()),
        if facts.typed_return.is_some() {
            "proven"
        } else {
            "unknown"
        },
        producer,
        source_site,
        event_prefix,
        "typed returns keep call results out of DynBox lanes",
    ));
    node.facts.push(call_fact_entry(
        "call.leaf",
        fact_value_string(facts.leaf),
        fact_value_confidence(facts.leaf),
        producer,
        source_site,
        event_prefix,
        "leaf calls unlock frame and spill elision",
    ));
    node.facts.push(call_fact_entry(
        "call.no_throw",
        fact_value_string(facts.no_throw),
        fact_value_confidence(facts.no_throw),
        producer,
        source_site,
        event_prefix,
        "no-throw calls remove exception-region churn on normal edges",
    ));
    node.facts.push(call_fact_entry(
        "call.no_alloc",
        fact_value_string(facts.no_alloc),
        fact_value_confidence(facts.no_alloc),
        producer,
        source_site,
        event_prefix,
        "no-alloc calls allow borrowed args and RC elision across calls",
    ));
    node.facts.push(call_fact_entry(
        "call.inlinable",
        inline_eligibility_value(facts.inlinable),
        inline_eligibility_confidence(facts.inlinable),
        producer,
        source_site,
        event_prefix,
        "inline eligibility records why a call stayed generic",
    ));
}

fn call_fact_entry(
    kind: &str,
    value: String,
    confidence: &str,
    producer: &str,
    source_site: Option<FactSourceSite>,
    event_prefix: &str,
    perf_relevance: &str,
) -> FactEntry {
    FactEntry {
        kind: kind.to_string(),
        value,
        confidence: confidence.to_string(),
        producer: producer.to_string(),
        event_id: Some(fact_event_id(event_prefix, kind)),
        source_site,
        guards: Vec::new(),
        invalidators: vec![
            "AnalysisId::CallFacts:cfg_sensitive".to_string(),
            "AnalysisId::CallFacts:ops_sensitive".to_string(),
        ],
        backend_lowering_status: "advisory-until-consumed-by-backends".to_string(),
        test_coverage: "call_facts tests + fact_graph tests".to_string(),
        perf_relevance: perf_relevance.to_string(),
    }
}

fn call_target_value(target: &CallTargetFact) -> String {
    match target {
        CallTargetFact::StaticDirect { callee } => format!("StaticDirect({callee})"),
        CallTargetFact::Opaque => "Opaque".to_string(),
    }
}

fn call_target_confidence(target: &CallTargetFact) -> &'static str {
    match target {
        CallTargetFact::StaticDirect { .. } => "proven",
        CallTargetFact::Opaque => "unknown",
    }
}

fn fact_value_string(value: FactValue) -> String {
    match value {
        FactValue::Proven => "Proven".to_string(),
        FactValue::Guarded(id) => format!("Guarded({})", id.0),
        FactValue::Profiled(confidence) => format!("Profiled({})", confidence.0),
        FactValue::Unknown => "Unknown".to_string(),
        FactValue::False => "False".to_string(),
    }
}

fn fact_value_confidence(value: FactValue) -> &'static str {
    match value {
        FactValue::Proven => "proven",
        FactValue::Guarded(_) => "guarded",
        FactValue::Profiled(_) => "profiled",
        FactValue::Unknown => "unknown",
        FactValue::False => "proven_false",
    }
}

fn inline_eligibility_value(value: InlineEligibility) -> String {
    match value {
        InlineEligibility::Eligible => "Eligible".to_string(),
        InlineEligibility::WhyNot(reason) => format!("WhyNot({})", inline_why_not(reason)),
        InlineEligibility::Unknown => "Unknown".to_string(),
    }
}

fn inline_eligibility_confidence(value: InlineEligibility) -> &'static str {
    match value {
        InlineEligibility::Eligible => "proven",
        InlineEligibility::WhyNot(_) => "proven_false",
        InlineEligibility::Unknown => "unknown",
    }
}

fn inline_why_not(reason: InlineWhyNot) -> &'static str {
    match reason {
        InlineWhyNot::Recursive => "Recursive",
        InlineWhyNot::HasHandlers => "HasHandlers",
        InlineWhyNot::Generator => "Generator",
        InlineWhyNot::EntryHasPredecessor => "EntryHasPredecessor",
        InlineWhyNot::Closure => "Closure",
        InlineWhyNot::OverBudget => "OverBudget",
    }
}

fn dedupe_facts(mut facts: Vec<FactEntry>) -> Vec<FactEntry> {
    facts.sort_by(|a, b| {
        (
            &a.kind,
            &a.value,
            &a.confidence,
            &a.producer,
            &a.event_id,
            a.source_site,
        )
            .cmp(&(
                &b.kind,
                &b.value,
                &b.confidence,
                &b.producer,
                &b.event_id,
                b.source_site,
            ))
    });
    facts.dedup_by(|a, b| {
        a.kind == b.kind
            && a.value == b.value
            && a.confidence == b.confidence
            && a.producer == b.producer
            && a.event_id == b.event_id
            && a.source_site == b.source_site
    });
    facts
}

fn dedupe_consumers(mut consumers: Vec<FactConsumer>) -> Vec<FactConsumer> {
    consumers.sort_by(|a, b| {
        (
            &a.kind,
            a.block,
            a.op_index,
            &a.opcode,
            a.operand_index,
            &a.role,
        )
            .cmp(&(
                &b.kind,
                b.block,
                b.op_index,
                &b.opcode,
                b.operand_index,
                &b.role,
            ))
    });
    consumers.dedup();
    consumers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, TirBlock};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, SourceSite};
    use crate::tir::values::TirValue;

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

    #[test]
    fn graph_records_producers_consumers_and_repr_floor() {
        let mut func = TirFunction::new("add".into(), vec![TirType::I64], TirType::I64);
        let entry = func.entry_block;
        let c = func.fresh_value();
        let sum = func.fresh_value();
        func.value_types.insert(c, TirType::I64);
        func.value_types.insert(sum, TirType::I64);
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops.push(op(OpCode::ConstInt, vec![], vec![c]));
        block
            .ops
            .push(op(OpCode::Add, vec![ValueId(0), c], vec![sum]));
        block.terminator = Terminator::Return { values: vec![sum] };

        let graph = FactGraph::build_local(&func);
        assert_eq!(graph.kind, FACT_GRAPH_KIND);
        assert_eq!(graph.summary.value_count, 3);
        assert!(graph.summary.fact_count >= 6);
        assert!(graph.summary.edge_count >= 3);

        let param = graph.values.iter().find(|n| n.value == 0).unwrap();
        assert_eq!(param.producer.as_ref().unwrap().kind, "parameter");
        assert!(
            param
                .facts
                .iter()
                .any(|f| f.kind == "repr_floor" && f.value == "MaybeBigInt")
        );
        assert!(
            param
                .consumers
                .iter()
                .any(|c| c.kind == "op_operand" && c.opcode.as_deref() == Some("Add"))
        );

        let sum_node = graph.values.iter().find(|n| n.value == sum.0).unwrap();
        assert_eq!(
            sum_node.producer.as_ref().unwrap().opcode.as_deref(),
            Some("Add")
        );
        assert!(
            sum_node
                .consumers
                .iter()
                .any(|c| c.kind == "terminator_operand" && c.role == "return[0]")
        );
    }

    #[test]
    fn graph_records_call_facts_from_authoritative_table() {
        let mut func = TirFunction::new("caller".into(), vec![], TirType::None);
        let call_result = func.fresh_value();
        func.value_types.insert(call_result, TirType::I64);
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str("callee".into()));
        let block = func.blocks.get_mut(&func.entry_block).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![call_result],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return {
            values: vec![call_result],
        };

        let facts = CallFactsTable::build_local(&func);
        let graph = FactGraph::build_with_call_facts(&func, &facts, "CallFactsTable::build_local");
        let node = graph
            .values
            .iter()
            .find(|n| n.value == call_result.0)
            .unwrap();
        assert!(node.facts.iter().any(|f| {
            f.kind == "call.target"
                && f.value == "Opaque"
                && f.producer == "CallFactsTable::build_local"
        }));
        assert!(
            node.facts
                .iter()
                .any(|f| f.kind == "call.typed_return" && f.value == "MaybeBigInt")
        );
        assert_eq!(graph.summary.call_fact_count, 6);
    }

    #[test]
    fn graph_records_sourced_allocation_and_ownership_events() {
        let mut func = TirFunction::new("own".into(), vec![], TirType::None);
        let root = func.fresh_value();
        func.value_types.insert(root, TirType::DynBox);
        let mut alloc = op(OpCode::Alloc, vec![], vec![root]);
        alloc.set_source_site(SourceSite {
            line: 11,
            col: Some(4),
            end_col: Some(14),
        });
        let mut release = op(OpCode::DecRef, vec![root], vec![]);
        release.set_source_site(SourceSite {
            line: 12,
            col: Some(8),
            end_col: Some(15),
        });
        let block = func.blocks.get_mut(&func.entry_block).unwrap();
        block.ops.push(alloc);
        block.ops.push(release);
        block.terminator = Terminator::Return { values: Vec::new() };

        let graph = FactGraph::build_local(&func);
        let node = graph.values.iter().find(|n| n.value == root.0).unwrap();
        assert_eq!(
            node.producer.as_ref().unwrap().source_site,
            Some(FactSourceSite {
                line: 11,
                col: Some(4),
                end_col: Some(14),
            })
        );
        assert!(
            node.consumers
                .iter()
                .any(|c| c.opcode.as_deref() == Some("DecRef")
                    && c.source_site
                        == Some(FactSourceSite {
                            line: 12,
                            col: Some(8),
                            end_col: Some(15),
                        }))
        );
        assert!(node.facts.iter().any(|f| {
            f.kind == "allocation.heap_root"
                && f.event_id.as_deref() == Some("own:bb0:op0:Alloc:result0:allocation.heap_root")
                && f.source_site
                    == Some(FactSourceSite {
                        line: 11,
                        col: Some(4),
                        end_col: Some(14),
                    })
        }));
        assert!(node.facts.iter().any(|f| {
            f.kind == "ownership.explicit_release"
                && f.event_id.as_deref()
                    == Some("own:bb0:op1:DecRef:operand0:ownership.explicit_release")
                && f.source_site
                    == Some(FactSourceSite {
                        line: 12,
                        col: Some(8),
                        end_col: Some(15),
                    })
        }));
        assert_eq!(graph.summary.source_site_value_count, 1);
        assert_eq!(graph.summary.allocation_ownership_fact_count, 3);
    }

    #[test]
    fn graph_json_roundtrips_with_stable_schema_header() {
        let mut func = TirFunction::new("branch".into(), vec![TirType::Bool], TirType::None);
        let target = BlockId(1);
        func.blocks.insert(
            target,
            TirBlock {
                id: target,
                args: vec![TirValue {
                    id: ValueId(1),
                    ty: TirType::Bool,
                }],
                ops: Vec::new(),
                terminator: Terminator::Return {
                    values: vec![ValueId(1)],
                },
            },
        );
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target,
            args: vec![ValueId(0)],
        };

        let graph = FactGraph::build_local(&func);
        let json = graph.to_pretty_json().unwrap();
        let decoded: FactGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, FACT_GRAPH_SCHEMA_VERSION);
        assert_eq!(decoded.kind, FACT_GRAPH_KIND);
        assert_eq!(decoded.values[0].value, 0);
        assert_eq!(decoded.values[1].value, 1);
    }
}
