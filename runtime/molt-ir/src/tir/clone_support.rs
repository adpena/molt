//! Shared, fail-closed support for cloning TIR bodies across function scopes.
//!
//! Exception labels are function-local integers while ordinary CFG targets are
//! [`BlockId`]s. Any transform that clones a body must remap both namespaces and
//! every terminator value. Keeping that policy here prevents inlining, generator
//! fusion, and future whole-program transforms from growing subtly different
//! collision and missing-map behavior.

use std::collections::{BTreeSet, HashMap};

use crate::ir::FunctionIR;

use super::blocks::{BlockId, Terminator};
use super::function::TirFunction;
use super::op_kinds_generated::{
    opcode_has_exception_label_attr_table, simpleir_kind_uses_function_label_id,
};
use super::ops::{AttrDict, AttrValue, TirOp};
use super::values::ValueId;

/// Read an exception operation's function-local label, if present.
pub fn exception_label_of(op: &TirOp) -> Option<i64> {
    if !opcode_has_exception_label_attr_table(op.opcode) {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

/// Every exception label used or defined by `func`, in deterministic order.
pub fn function_label_ids(func: &TirFunction) -> BTreeSet<i64> {
    let mut labels: BTreeSet<i64> = func.label_id_map.values().copied().collect();
    labels.extend(
        func.blocks
            .values()
            .flat_map(|block| &block.ops)
            .filter_map(exception_label_of),
    );
    labels
}

/// Monotonic, overflow-checked allocator for a function-local label namespace.
#[derive(Debug, Clone)]
pub struct LabelAllocator {
    next: i64,
}

impl LabelAllocator {
    pub fn after_labels(labels: impl IntoIterator<Item = i64>) -> Self {
        let next = labels.into_iter().max().map_or(0, |label| {
            label
                .checked_add(1)
                .expect("TIR exhausted the exception-label domain")
        });
        Self { next }
    }

    pub fn for_function(func: &TirFunction) -> Self {
        Self::after_labels(function_label_ids(func))
    }

    pub fn for_simple_ir(func: &FunctionIR) -> Self {
        Self::after_labels(
            func.ops
                .iter()
                .filter(|op| simpleir_kind_uses_function_label_id(&op.kind))
                .filter_map(|op| op.value),
        )
    }

    pub fn fresh(&mut self) -> i64 {
        let label = self.next;
        self.next = self
            .next
            .checked_add(1)
            .expect("TIR exhausted the exception-label domain");
        label
    }
}

/// Build a deterministic source-to-fresh label map for cloning into `target`.
pub fn build_label_remap(source: &TirFunction, target: &TirFunction) -> HashMap<i64, i64> {
    let source_labels = function_label_ids(source);
    if source_labels.is_empty() {
        return HashMap::new();
    }
    let mut labels = LabelAllocator::for_function(target);
    let mut remap = HashMap::with_capacity(source_labels.len());
    for label in source_labels {
        remap.insert(label, labels.fresh());
    }
    remap
}

/// Rewrite an exception label attribute through the complete clone map.
pub fn remap_exception_label_attr(
    opcode: super::ops::OpCode,
    attrs: &mut AttrDict,
    label_remap: &HashMap<i64, i64>,
    context: &str,
) {
    if !opcode_has_exception_label_attr_table(opcode) {
        return;
    }
    let Some(AttrValue::Int(old_label)) = attrs.get("value") else {
        return;
    };
    let new_label = *label_remap
        .get(old_label)
        .unwrap_or_else(|| panic!("{context}: exception label {old_label} has no clone remap"));
    attrs.insert("value".into(), AttrValue::Int(new_label));
}

/// Transfer every label-bearing block through the same complete remap.
pub fn transfer_label_id_map(
    source: &TirFunction,
    target: &mut TirFunction,
    block_remap: &HashMap<BlockId, BlockId>,
    label_remap: &HashMap<i64, i64>,
    context: &str,
) {
    for (&old_block, &old_label) in &source.label_id_map {
        let old_block = BlockId(old_block);
        let new_block = *block_remap
            .get(&old_block)
            .unwrap_or_else(|| panic!("{context}: label block {old_block} has no clone remap"));
        let new_label = *label_remap
            .get(&old_label)
            .unwrap_or_else(|| panic!("{context}: defined label {old_label} has no clone remap"));
        assert!(
            target.label_id_map.insert(new_block.0, new_label).is_none(),
            "{context}: cloned label block {new_block} already has a label"
        );
    }
}

/// Clone a terminator with complete value and block remapping.
pub fn remap_terminator(
    term: &Terminator,
    value_remap: &HashMap<ValueId, ValueId>,
    block_remap: &HashMap<BlockId, BlockId>,
    context: &str,
) -> Terminator {
    let value = |old: ValueId| {
        *value_remap
            .get(&old)
            .unwrap_or_else(|| panic!("{context}: terminator value {old} has no clone remap"))
    };
    let block = |old: BlockId| {
        *block_remap
            .get(&old)
            .unwrap_or_else(|| panic!("{context}: terminator block {old} has no clone remap"))
    };
    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: block(*target),
            args: args.iter().map(|item| value(*item)).collect(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: value(*cond),
            then_block: block(*then_block),
            then_args: then_args.iter().map(|item| value(*item)).collect(),
            else_block: block(*else_block),
            else_args: else_args.iter().map(|item| value(*item)).collect(),
        },
        Terminator::Switch {
            value: selector,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: value(*selector),
            cases: cases
                .iter()
                .map(|(case, target, args)| {
                    (
                        *case,
                        block(*target),
                        args.iter().map(|item| value(*item)).collect(),
                    )
                })
                .collect(),
            default: block(*default),
            default_args: default_args.iter().map(|item| value(*item)).collect(),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(state, target, args)| {
                    (
                        *state,
                        block(*target),
                        args.iter().map(|item| value(*item)).collect(),
                    )
                })
                .collect(),
            default: block(*default),
            default_args: default_args.iter().map(|item| value(*item)).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|item| value(*item)).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "terminator value %0 has no clone remap")]
    fn terminator_remap_fails_closed_on_missing_value() {
        remap_terminator(
            &Terminator::Return {
                values: vec![ValueId(0)],
            },
            &HashMap::new(),
            &HashMap::new(),
            "clone-test",
        );
    }

    #[test]
    #[should_panic(expected = "terminator block bb7 has no clone remap")]
    fn terminator_remap_fails_closed_on_missing_block() {
        remap_terminator(
            &Terminator::Branch {
                target: BlockId(7),
                args: vec![],
            },
            &HashMap::new(),
            &HashMap::new(),
            "clone-test",
        );
    }

    #[test]
    fn simple_ir_allocator_reserves_definitions_references_and_resume_ids() {
        let function = FunctionIR {
            ops: vec![
                crate::ir::OpIR {
                    kind: "label".into(),
                    value: Some(3),
                    ..Default::default()
                },
                crate::ir::OpIR {
                    kind: "check_exception".into(),
                    value: Some(41),
                    ..Default::default()
                },
                crate::ir::OpIR {
                    kind: "state_yield".into(),
                    value: Some(53),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        assert_eq!(LabelAllocator::for_simple_ir(&function).fresh(), 54);
    }
}
