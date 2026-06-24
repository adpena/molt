//! Dead block-argument pruning.
//!
//! TIR uses MLIR-style block arguments as its phi representation. Earlier
//! passes can shrink executable use sites while leaving join/handler/resume
//! block signatures wide. Those dead payload lanes are semantically inert, but
//! they are still lowered to SimpleIR `store_var`/`load_var` traffic on every
//! predecessor edge, which can dominate native codegen memory for large
//! ecosystem functions. This pass removes those lanes at the TIR authority
//! layer and rewrites every edge payload surface that can bind block args:
//! explicit terminators, state-dispatch edges, and implicit exception-transfer
//! op operands.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::PassStats;
use crate::tir::values::ValueId;

fn real_uses_in_terminator(term: &Terminator, uses: &mut HashSet<ValueId>) {
    match term {
        Terminator::Branch { .. } => {}
        Terminator::CondBranch { cond, .. } => {
            uses.insert(*cond);
        }
        Terminator::Switch { value, .. } => {
            uses.insert(*value);
        }
        Terminator::StateDispatch { .. } => {}
        Terminator::Return { values } => {
            uses.extend(values.iter().copied());
        }
        Terminator::Unreachable => {}
    }
}

fn block_arg_ids(func: &TirFunction) -> HashSet<ValueId> {
    func.blocks
        .values()
        .flat_map(|block| block.args.iter().map(|arg| arg.id))
        .collect()
}

fn real_used_values(func: &TirFunction) -> HashSet<ValueId> {
    let mut uses = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if !dominators::is_exception_transfer_edge(op.opcode) {
                uses.extend(op.operands.iter().copied());
            }
        }
        real_uses_in_terminator(&block.terminator, &mut uses);
    }
    uses
}

fn target_arg_is_live(
    func: &TirFunction,
    live: &HashSet<ValueId>,
    target: BlockId,
    arg_index: usize,
) -> bool {
    func.blocks
        .get(&target)
        .and_then(|block| block.args.get(arg_index))
        .is_some_and(|arg| live.contains(&arg.id))
}

fn propagate_edge_payload_liveness(
    func: &TirFunction,
    live: &mut HashSet<ValueId>,
    target: BlockId,
    args: &[ValueId],
) -> bool {
    let mut changed = false;
    for (idx, &arg) in args.iter().enumerate() {
        if target_arg_is_live(func, live, target, idx) {
            changed |= live.insert(arg);
        }
    }
    changed
}

fn propagate_terminator_liveness(
    func: &TirFunction,
    live: &mut HashSet<ValueId>,
    term: &Terminator,
) -> bool {
    match term {
        Terminator::Branch { target, args } => {
            propagate_edge_payload_liveness(func, live, *target, args)
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            propagate_edge_payload_liveness(func, live, *then_block, then_args)
                | propagate_edge_payload_liveness(func, live, *else_block, else_args)
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut changed = propagate_edge_payload_liveness(func, live, *default, default_args);
            for (_, target, args) in cases {
                changed |= propagate_edge_payload_liveness(func, live, *target, args);
            }
            changed
        }
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut changed = propagate_edge_payload_liveness(func, live, *default, default_args);
            for (_, target, args) in cases {
                changed |= propagate_edge_payload_liveness(func, live, *target, args);
            }
            changed
        }
        Terminator::Return { .. } | Terminator::Unreachable => false,
    }
}

fn propagate_exception_liveness(
    func: &TirFunction,
    live: &mut HashSet<ValueId>,
    label_to_block: &HashMap<i64, BlockId>,
) -> bool {
    let mut changed = false;
    for block in func.blocks.values() {
        for op in &block.ops {
            let Some(target) = exception_transfer_target(op.opcode, &op.attrs, label_to_block)
            else {
                continue;
            };
            changed |= propagate_edge_payload_liveness(func, live, target, &op.operands);
        }
    }
    changed
}

fn live_values(func: &TirFunction) -> HashSet<ValueId> {
    let block_args = block_arg_ids(func);
    let mut live: HashSet<ValueId> = real_used_values(func)
        .into_iter()
        .filter(|value| block_args.contains(value))
        .collect();
    let label_to_block = dominators::exception_label_to_block(func);

    loop {
        let mut changed = false;
        for block in func.blocks.values() {
            changed |= propagate_terminator_liveness(func, &mut live, &block.terminator);
        }
        changed |= propagate_exception_liveness(func, &mut live, &label_to_block);
        if !changed {
            return live;
        }
    }
}

fn dead_arg_positions(func: &TirFunction) -> BTreeMap<BlockId, Vec<usize>> {
    let live = live_values(func);
    let mut prune = BTreeMap::new();
    for (&bid, block) in &func.blocks {
        if bid == func.entry_block || block.args.is_empty() {
            continue;
        }
        let dead: Vec<usize> = block
            .args
            .iter()
            .enumerate()
            .filter_map(|(idx, arg)| (!live.contains(&arg.id)).then_some(idx))
            .collect();
        if !dead.is_empty() {
            prune.insert(bid, dead);
        }
    }
    prune
}

fn retain_positions_not_in<T>(values: &mut Vec<T>, dead_positions: &[usize]) -> usize {
    if dead_positions.is_empty() || values.is_empty() {
        return 0;
    }
    let mut idx = 0usize;
    let before = values.len();
    values.retain(|_| {
        let keep = dead_positions.binary_search(&idx).is_err();
        idx += 1;
        keep
    });
    before - values.len()
}

fn prune_edge_args(
    target: BlockId,
    args: &mut Vec<ValueId>,
    prune: &BTreeMap<BlockId, Vec<usize>>,
) -> usize {
    prune
        .get(&target)
        .map(|dead| retain_positions_not_in(args, dead))
        .unwrap_or(0)
}

fn prune_terminator_payloads(
    term: &mut Terminator,
    prune: &BTreeMap<BlockId, Vec<usize>>,
) -> usize {
    match term {
        Terminator::Branch { target, args } => prune_edge_args(*target, args, prune),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            prune_edge_args(*then_block, then_args, prune)
                + prune_edge_args(*else_block, else_args, prune)
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut removed = prune_edge_args(*default, default_args, prune);
            for (_, target, args) in cases {
                removed += prune_edge_args(*target, args, prune);
            }
            removed
        }
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut removed = prune_edge_args(*default, default_args, prune);
            for (_, target, args) in cases {
                removed += prune_edge_args(*target, args, prune);
            }
            removed
        }
        Terminator::Return { .. } | Terminator::Unreachable => 0,
    }
}

fn exception_transfer_target(
    op_opcode: OpCode,
    attrs: &crate::tir::ops::AttrDict,
    label_to_block: &std::collections::HashMap<i64, BlockId>,
) -> Option<BlockId> {
    if !dominators::is_exception_transfer_edge(op_opcode) {
        return None;
    }
    let Some(AttrValue::Int(label)) = attrs.get("value") else {
        return None;
    };
    label_to_block.get(label).copied()
}

fn prune_exception_payloads(
    func: &mut TirFunction,
    prune: &BTreeMap<BlockId, Vec<usize>>,
) -> usize {
    let label_to_block = dominators::exception_label_to_block(func);
    let mut removed = 0usize;
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            let Some(target) = exception_transfer_target(op.opcode, &op.attrs, &label_to_block)
            else {
                continue;
            };
            removed += prune_edge_args(target, &mut op.operands, prune);
        }
    }
    removed
}

/// Remove block-argument lanes whose SSA value is unused inside the target
/// block, updating all edge payloads that bind the target's remaining args.
///
/// The pass iterates to a fixed point because pruning one block's arg can make a
/// predecessor block arg dead when that value was only forwarded through the
/// removed lane.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "block_arg_prune",
        ..Default::default()
    };

    loop {
        let prune = dead_arg_positions(func);
        if prune.is_empty() {
            break;
        }

        let mut removed_block_args = 0usize;
        for (bid, dead_positions) in &prune {
            let Some(block) = func.blocks.get_mut(bid) else {
                continue;
            };
            let dead_values: Vec<ValueId> = dead_positions
                .iter()
                .filter_map(|idx| block.args.get(*idx).map(|arg| arg.id))
                .collect();
            removed_block_args += retain_positions_not_in(&mut block.args, dead_positions);
            for value in dead_values {
                func.value_types.remove(&value);
            }
        }

        let mut removed_edge_args = 0usize;
        for block in func.blocks.values_mut() {
            removed_edge_args += prune_terminator_payloads(&mut block.terminator, &prune);
        }
        removed_edge_args += prune_exception_payloads(func, &prune);

        stats.values_changed += removed_block_args + removed_edge_args;
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, Dialect, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::TirValue;

    fn arg(id: ValueId, ty: TirType) -> TirValue {
        TirValue { id, ty }
    }

    fn copy(operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn check_exception(label: i64, operands: Vec<ValueId>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(label));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands,
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    fn try_start(label: i64, operands: Vec<ValueId>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(label));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryStart,
            operands,
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    fn add_type(func: &mut TirFunction, id: ValueId, ty: TirType) {
        func.value_types.insert(id, ty);
    }

    #[test]
    fn prunes_unused_branch_block_arg_and_incoming_payload() {
        let mut func = TirFunction::new("branch_prune".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let join = func.fresh_block();
        let used = func.fresh_value();
        let dead = func.fresh_value();
        let out = func.fresh_value();
        for value in [used, dead, out] {
            add_type(&mut func, value, TirType::I64);
        }

        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: join,
            args: vec![used, dead],
        };
        func.blocks.insert(
            join,
            TirBlock {
                id: join,
                args: vec![arg(used, TirType::I64), arg(dead, TirType::I64)],
                ops: vec![copy(vec![used], vec![out])],
                terminator: Terminator::Return { values: vec![out] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 2);
        assert_eq!(func.blocks[&join].args.len(), 1);
        assert_eq!(func.blocks[&join].args[0].id, used);
        assert!(!func.value_types.contains_key(&dead));
        let Terminator::Branch { args, .. } = &func.blocks[&entry].terminator else {
            panic!("expected entry branch");
        };
        assert_eq!(args, &vec![used]);
    }

    #[test]
    fn prunes_cond_branch_payloads_on_each_matching_edge() {
        let mut func = TirFunction::new("cond_prune".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let then_block = func.fresh_block();
        let else_block = func.fresh_block();
        let cond = func.fresh_value();
        let then_used = func.fresh_value();
        let then_dead = func.fresh_value();
        let else_dead = func.fresh_value();
        let out = func.fresh_value();
        for value in [cond, then_used, then_dead, else_dead, out] {
            add_type(&mut func, value, TirType::I64);
        }
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block,
            then_args: vec![then_used, then_dead],
            else_block,
            else_args: vec![else_dead],
        };
        func.blocks.insert(
            then_block,
            TirBlock {
                id: then_block,
                args: vec![arg(then_used, TirType::I64), arg(then_dead, TirType::I64)],
                ops: vec![copy(vec![then_used], vec![out])],
                terminator: Terminator::Return { values: vec![out] },
            },
        );
        func.blocks.insert(
            else_block,
            TirBlock {
                id: else_block,
                args: vec![arg(else_dead, TirType::I64)],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 4);
        let Terminator::CondBranch {
            then_args,
            else_args,
            ..
        } = &func.blocks[&entry].terminator
        else {
            panic!("expected cond branch");
        };
        assert_eq!(then_args, &vec![then_used]);
        assert!(else_args.is_empty());
        assert_eq!(func.blocks[&then_block].args.len(), 1);
        assert!(func.blocks[&else_block].args.is_empty());
    }

    #[test]
    fn prunes_switch_payloads_on_cases_and_default() {
        let mut func = TirFunction::new("switch_prune".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let case_block = func.fresh_block();
        let default_block = func.fresh_block();
        let selector = func.fresh_value();
        let case_used = func.fresh_value();
        let case_dead = func.fresh_value();
        let default_dead = func.fresh_value();
        let out = func.fresh_value();
        for value in [selector, case_used, case_dead, default_dead, out] {
            add_type(&mut func, value, TirType::I64);
        }
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Switch {
            value: selector,
            cases: vec![(0, case_block, vec![case_used, case_dead])],
            default: default_block,
            default_args: vec![default_dead],
        };
        func.blocks.insert(
            case_block,
            TirBlock {
                id: case_block,
                args: vec![arg(case_used, TirType::I64), arg(case_dead, TirType::I64)],
                ops: vec![copy(vec![case_used], vec![out])],
                terminator: Terminator::Return { values: vec![out] },
            },
        );
        func.blocks.insert(
            default_block,
            TirBlock {
                id: default_block,
                args: vec![arg(default_dead, TirType::I64)],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 4);
        assert_eq!(func.blocks[&case_block].args.len(), 1);
        assert_eq!(func.blocks[&case_block].args[0].id, case_used);
        assert!(func.blocks[&default_block].args.is_empty());
        let Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } = &func.blocks[&entry].terminator
        else {
            panic!("expected switch");
        };
        assert_eq!(*value, selector);
        assert_eq!(cases[0].2, vec![case_used]);
        assert!(default_args.is_empty());
    }

    #[test]
    fn prunes_unused_check_exception_handler_payloads() {
        let mut func = TirFunction::new("exception_prune".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let handler = func.fresh_block();
        let used = func.fresh_value();
        let dead = func.fresh_value();
        let out = func.fresh_value();
        for value in [used, dead, out] {
            add_type(&mut func, value, TirType::DynBox);
        }
        func.label_id_map.insert(handler.0, 99);
        func.blocks.get_mut(&entry).unwrap().ops = vec![check_exception(99, vec![used, dead])];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![arg(used, TirType::DynBox), arg(dead, TirType::DynBox)],
                ops: vec![copy(vec![used], vec![out])],
                terminator: Terminator::Return { values: vec![out] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 2);
        assert_eq!(func.blocks[&handler].args.len(), 1);
        assert_eq!(func.blocks[&entry].ops[0].operands, vec![used]);
    }

    #[test]
    fn prunes_unused_try_start_handler_payloads() {
        let mut func = TirFunction::new("try_start_prune".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let handler = func.fresh_block();
        let used = func.fresh_value();
        let dead = func.fresh_value();
        let out = func.fresh_value();
        for value in [used, dead, out] {
            add_type(&mut func, value, TirType::DynBox);
        }
        func.label_id_map.insert(handler.0, 77);
        func.blocks.get_mut(&entry).unwrap().ops = vec![try_start(77, vec![used, dead])];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![arg(used, TirType::DynBox), arg(dead, TirType::DynBox)],
                ops: vec![copy(vec![used], vec![out])],
                terminator: Terminator::Return { values: vec![out] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 2);
        assert_eq!(func.blocks[&handler].args.len(), 1);
        assert_eq!(func.blocks[&handler].args[0].id, used);
        assert_eq!(func.blocks[&entry].ops[0].operands, vec![used]);
    }

    #[test]
    fn prunes_state_dispatch_payloads() {
        let mut func = TirFunction::new("state_prune".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let resume = func.fresh_block();
        let default = func.fresh_block();
        let used = func.fresh_value();
        let dead = func.fresh_value();
        for value in [used, dead] {
            add_type(&mut func, value, TirType::DynBox);
        }
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::StateDispatch {
            cases: vec![(1, resume, vec![used, dead])],
            default,
            default_args: vec![dead],
        };
        func.blocks.insert(
            resume,
            TirBlock {
                id: resume,
                args: vec![arg(used, TirType::DynBox), arg(dead, TirType::DynBox)],
                ops: vec![],
                terminator: Terminator::Return { values: vec![used] },
            },
        );
        func.blocks.insert(
            default,
            TirBlock {
                id: default,
                args: vec![arg(dead, TirType::DynBox)],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 4);
        let Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } = &func.blocks[&entry].terminator
        else {
            panic!("expected state dispatch");
        };
        assert_eq!(cases[0].2, vec![used]);
        assert!(default_args.is_empty());
        assert_eq!(func.blocks[&resume].args.len(), 1);
        assert!(func.blocks[&default].args.is_empty());
    }

    #[test]
    fn never_prunes_entry_parameters() {
        let mut func = TirFunction::new(
            "entry_params".into(),
            vec![TirType::I64, TirType::I64],
            TirType::None,
        );
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
        assert_eq!(func.blocks[&func.entry_block].args.len(), 2);
    }

    #[test]
    fn fixed_point_prunes_forwarded_dead_arg_chain() {
        let mut func = TirFunction::new("chain_prune".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let middle = func.fresh_block();
        let exit = func.fresh_block();
        let forwarded = func.fresh_value();
        for value in [forwarded] {
            add_type(&mut func, value, TirType::DynBox);
        }
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: middle,
            args: vec![forwarded],
        };
        func.blocks.insert(
            middle,
            TirBlock {
                id: middle,
                args: vec![arg(forwarded, TirType::DynBox)],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: exit,
                    args: vec![forwarded],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![arg(forwarded, TirType::DynBox)],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 4);
        assert!(func.blocks[&middle].args.is_empty());
        assert!(func.blocks[&exit].args.is_empty());
        let Terminator::Branch { args, .. } = &func.blocks[&entry].terminator else {
            panic!("expected branch");
        };
        assert!(args.is_empty());
        let Terminator::Branch { args, .. } = &func.blocks[&middle].terminator else {
            panic!("expected branch");
        };
        assert!(args.is_empty());
    }

    #[test]
    fn keeps_arg_used_only_in_dominated_descendant() {
        let mut func = TirFunction::new("descendant_use".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let carrier = func.fresh_block();
        let use_block = func.fresh_block();
        let src = func.fresh_value();
        let phi = func.fresh_value();
        let out = func.fresh_value();
        for value in [src, phi, out] {
            add_type(&mut func, value, TirType::DynBox);
        }

        func.blocks.get_mut(&entry).unwrap().ops = vec![copy(vec![], vec![src])];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: carrier,
            args: vec![src],
        };
        func.blocks.insert(
            carrier,
            TirBlock {
                id: carrier,
                args: vec![arg(phi, TirType::DynBox)],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: use_block,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            use_block,
            TirBlock {
                id: use_block,
                args: vec![],
                ops: vec![copy(vec![phi], vec![out])],
                terminator: Terminator::Return { values: vec![out] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
        assert_eq!(func.blocks[&carrier].args.len(), 1);
        let Terminator::Branch { args, .. } = &func.blocks[&entry].terminator else {
            panic!("expected entry branch");
        };
        assert_eq!(args, &vec![src]);
    }
}
