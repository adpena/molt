//! Final placement of Python asynchronous-work/eval-breaker observations.
//!
//! The frontend's universal `CheckException` observations carry the exact
//! exceptional successor for their lexical region. This pass marks that
//! existing authority at every generated call-return site and every canonical
//! loop backedge. `check_exception_elim` must preserve marked observations.

use std::collections::{BTreeMap, BTreeSet};

use crate::tir::analysis::{AnalysisManager, LoopForest};
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::exception_regions::{
    ExceptionBoundaryHandler, ExceptionOpPosition, ExceptionRegionFacts, ExceptionRegions,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_requires_async_work_poll_after_table, simpleir_kind_is_call_graph_user_call,
};
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};

use super::PassStats;

fn is_call_return_poll(op: &TirOp) -> bool {
    opcode_requires_async_work_poll_after_table(op.opcode)
        || (op.opcode == OpCode::Copy
            && matches!(
                op.attrs.get("_original_kind"),
                Some(AttrValue::Str(kind)) if simpleir_kind_is_call_graph_user_call(kind)
            ))
}

fn mark_poll(op: &mut TirOp) -> bool {
    op.mark_async_work_poll()
}

fn check_exception(label: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(label));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    }
}

fn check_label(op: &TirOp) -> Option<i64> {
    if op.opcode != OpCode::CheckException {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

/// Resolve a reachable insertion boundary. The outer `Option` is reachability;
/// the inner `Option` is depth zero versus a labeled lexical handler.
fn reachable_lexical_handler(
    facts: &ExceptionRegionFacts,
    function_name: &str,
    position: ExceptionOpPosition,
) -> Option<Option<i64>> {
    match facts.lexical_handler_before(position) {
        Ok(ExceptionBoundaryHandler::Unreachable) => None,
        Ok(ExceptionBoundaryHandler::DepthZero) => Some(None),
        Ok(ExceptionBoundaryHandler::Labeled(label)) => Some(Some(label)),
        Ok(ExceptionBoundaryHandler::Anonymous { destination, .. }) => Some(Some(destination)),
        Err(error) => {
            panic!(
                "async-work poll boundary in function {function_name:?} has invalid lexical custody: {error:?}"
            )
        }
    }
}

/// Create the function-level propagation exit used when optimized/synthetic
/// TIR has no surviving lexical `CheckException` to clone. The exit is a real
/// labeled block in the same authority as frontend-created exception exits;
/// lower-to-SimpleIR therefore emits the ordinary return-with-pending path and
/// every backend receives an explicit branch target.
fn make_function_exception_exit(func: &mut TirFunction) -> TirOp {
    let mut used_labels: BTreeSet<i64> = func.label_id_map.values().copied().collect();
    used_labels.extend(func.blocks.values().flat_map(|block| {
        block
            .ops
            .iter()
            .filter_map(|op| match op.attrs.get("value") {
                Some(AttrValue::Int(label)) => Some(*label),
                _ => None,
            })
    }));
    let mut label = 0_i64;
    while used_labels.contains(&label) {
        label = label
            .checked_add(1)
            .expect("TIR exhausted the exception-label domain");
    }

    let exit = func.fresh_block();
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.label_id_map.insert(exit.0, label);

    check_exception(label)
}

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "async_work_poll",
        ..Default::default()
    };

    let loops = am.get::<LoopForest>(func).clone();
    let region_facts = am.get::<ExceptionRegions>(func).clone();
    let mut latches = BTreeSet::new();
    for header in loops.headers {
        let Some(body) = loops.bodies.get(&header) else {
            continue;
        };
        for &block_id in body {
            let Some(block) = func.blocks.get(&block_id) else {
                continue;
            };
            let mut reaches_header = false;
            block
                .terminator
                .for_each_edge(|target, _| reaches_header |= target == header);
            if reaches_header {
                latches.insert((block_id, header));
            }
        }
    }

    let call_sites: Vec<_> = func
        .blocks
        .iter()
        .flat_map(|(&block_id, block)| {
            block
                .ops
                .iter()
                .enumerate()
                .filter_map(|(index, op)| {
                    if !is_call_return_poll(op) {
                        return None;
                    }
                    let position = ExceptionOpPosition {
                        block: block_id,
                        op_index: index + 1,
                    };
                    let target = reachable_lexical_handler(&region_facts, &func.name, position)?;
                    Some((block_id, index, target))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut latch_sites_by_block: BTreeMap<_, (BTreeSet<_>, Option<i64>)> = BTreeMap::new();
    for (latch, header) in latches {
        let position = ExceptionOpPosition {
            block: latch,
            op_index: func.blocks[&latch].ops.len(),
        };
        let Some(target) = reachable_lexical_handler(&region_facts, &func.name, position) else {
            continue;
        };
        let entry = latch_sites_by_block
            .entry(latch)
            .or_insert_with(|| (BTreeSet::new(), target));
        assert_eq!(
            entry.1, target,
            "one latch boundary cannot have header-dependent exception custody"
        );
        entry.0.insert(header);
    }
    let latch_sites: Vec<_> = latch_sites_by_block
        .into_iter()
        .map(|(latch, (headers, target))| (latch, headers, target))
        .collect();

    let call_needs_function_exit = call_sites.iter().any(|(block, index, target)| {
        target.is_none()
            && func.blocks[block]
                .ops
                .get(index + 1)
                .and_then(check_label)
                .is_none()
    });
    let latch_needs_function_exit = latch_sites.iter().any(|(latch, _, target)| {
        target.is_none()
            && func.blocks[latch]
                .ops
                .last()
                .and_then(check_label)
                .is_none()
    });
    let function_exit_check = (call_needs_function_exit || latch_needs_function_exit)
        .then(|| make_function_exception_exit(func));

    // Prefer the frontend's lexical successor. Optimized and synthetic TIR may
    // legitimately lack one; those sites use the canonical function exit above
    // rather than silently dropping the poll or panicking in the pipeline.
    let mut call_blocks = BTreeSet::new();
    call_blocks.extend(call_sites.iter().map(|(block, _, _)| *block));
    for block_id in call_blocks {
        let mut sites: Vec<_> = call_sites
            .iter()
            .filter(|(block, _, _)| *block == block_id)
            .map(|(_, index, target)| (*index, *target))
            .collect();
        sites.sort_unstable_by_key(|(index, _)| *index);
        for (index, target) in sites.into_iter().rev() {
            let block = func.blocks.get_mut(&block_id).unwrap();
            // At this exact post-call boundary, a depth-zero adjacent
            // CheckException is the frontend's function-exit observation; it
            // is not a proximity search. In a lexical region, require the
            // fact-selected handler label exactly.
            let existing_matches = block
                .ops
                .get(index + 1)
                .and_then(check_label)
                .is_some_and(|label| target.is_none() || target == Some(label));
            if existing_matches {
                stats.attrs_changed += usize::from(mark_poll(&mut block.ops[index + 1]));
                continue;
            }
            let mut check = target.map(check_exception).unwrap_or_else(|| {
                function_exit_check
                    .clone()
                    .expect("depth-zero async-work poll requires a function exception exit")
            });
            mark_poll(&mut check);
            block.ops.insert(index + 1, check);
            stats.ops_added += 1;
        }
    }

    for (latch, _headers, target) in latch_sites {
        let existing = func.blocks[&latch]
            .ops
            .last()
            .and_then(check_label)
            .is_some_and(|label| target.is_none() || target == Some(label));
        if existing {
            let index = func.blocks[&latch].ops.len() - 1;
            let check = &mut func.blocks.get_mut(&latch).unwrap().ops[index];
            stats.attrs_changed += usize::from(mark_poll(check));
            continue;
        }

        let mut check = target.map(check_exception).unwrap_or_else(|| {
            function_exit_check
                .clone()
                .expect("depth-zero loop poll requires a function exception exit")
        });
        mark_poll(&mut check);
        func.blocks.get_mut(&latch).unwrap().ops.push(check);
        stats.ops_added += 1;
    }

    if stats.total_changes() != 0 {
        func.has_exception_handling = true;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::types::TirType;

    fn op(opcode: OpCode) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn labeled_op(opcode: OpCode, label: i64) -> TirOp {
        let mut op = op(opcode);
        op.attrs.insert("value".into(), AttrValue::Int(label));
        op
    }

    fn check(label: i64) -> TirOp {
        labeled_op(OpCode::CheckException, label)
    }

    fn exception_pop() -> TirOp {
        let mut op = op(OpCode::Copy);
        op.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("exception_pop".into()),
        );
        op
    }

    #[test]
    fn generated_call_returns_and_loop_backedges_share_one_poll_marker() {
        let mut func = TirFunction::new("polls".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = BlockId(1);
        func.next_block = 2;
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.get_mut(&header).unwrap().ops = vec![op(OpCode::Call), check(70)];
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![check(70)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );
        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.attrs_changed, 2);
        for block in func.blocks.values() {
            for check in block
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::CheckException)
            {
                assert!(check.is_async_work_poll());
            }
        }
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        assert_eq!(
            simple
                .iter()
                .filter(|op| op.kind == "async_work_poll")
                .count(),
            2,
            "the canonical wire spelling must preserve both generated sites"
        );
    }

    #[test]
    fn loop_without_lexical_check_gets_one_labeled_function_exit() {
        let mut func = TirFunction::new("synthetic_loop".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = BlockId(1);
        func.next_block = 2;
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.ops_added, 1);
        let check = func.blocks[&latch]
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::CheckException)
            .expect("latch poll");
        let label = match check.attrs.get("value") {
            Some(AttrValue::Int(label)) => *label,
            other => panic!("poll missing exception label: {other:?}"),
        };
        assert_eq!(
            func.label_id_map
                .values()
                .filter(|value| **value == label)
                .count(),
            1
        );
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        assert!(
            simple
                .iter()
                .any(|op| { op.kind == "async_work_poll" && op.value == Some(label) })
        );
        assert!(
            simple
                .iter()
                .any(|op| op.kind == "label" && op.value == Some(label))
        );
    }

    #[test]
    fn nested_try_call_uses_inner_lexical_handler_not_a_later_outer_check() {
        let mut func = TirFunction::new("nested_try".into(), vec![], TirType::None);
        let entry = func.entry_block;
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            labeled_op(OpCode::TryStart, 10),
            labeled_op(OpCode::TryStart, 20),
            op(OpCode::Call),
            check(10),
            check(20),
        ];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

        run(&mut func, &mut AnalysisManager::new());
        let checks: Vec<_> = func.blocks[&entry]
            .ops
            .iter()
            .skip(3)
            .filter_map(check_label)
            .collect();
        assert_eq!(checks[0], 20, "poll must target the inner lexical handler");
        assert_eq!(checks[1..], [10, 20]);
    }

    #[test]
    fn same_block_try_transition_routes_each_call_from_its_exact_boundary() {
        let mut func = TirFunction::new("try_transition".into(), vec![], TirType::None);
        let entry = func.entry_block;
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            labeled_op(OpCode::TryStart, 30),
            op(OpCode::Call),
            check(30),
            labeled_op(OpCode::TryEnd, 30),
            op(OpCode::Call),
        ];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

        run(&mut func, &mut AnalysisManager::new());
        let polls: Vec<_> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.is_async_work_poll())
            .filter_map(check_label)
            .collect();
        assert_eq!(polls.len(), 2);
        assert_eq!(polls[0], 30);
        assert_ne!(polls[1], 30, "depth-zero call must use the function exit");
    }

    #[test]
    fn anonymous_try_call_uses_its_recovered_handler_destination() {
        let mut func = TirFunction::new("anonymous_try_call".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 73);
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            op(OpCode::TryStart),
            op(OpCode::Call),
            check(73),
            op(OpCode::TryEnd),
        ];
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![op(OpCode::TryEnd)],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        let poll = func.blocks[&entry]
            .ops
            .iter()
            .find(|op| op.is_async_work_poll())
            .expect("anonymous-region call poll");
        assert_eq!(check_label(poll), Some(73));
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        assert!(crate::tir::lower_to_simple::validate_labels(&simple));
        assert!(
            simple
                .iter()
                .any(|op| { op.kind == "async_work_poll" && op.value == Some(73) })
        );
    }

    #[test]
    fn anonymous_try_loop_latch_uses_its_recovered_handler_destination() {
        let mut func = TirFunction::new("anonymous_try_loop".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = func.fresh_block();
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 74);
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.get_mut(&header).unwrap().ops = vec![op(OpCode::TryStart), check(74)];
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![op(OpCode::TryEnd)],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        assert_eq!(
            func.blocks[&latch].ops.last().and_then(check_label),
            Some(74)
        );
        assert!(func.blocks[&latch].ops.last().unwrap().is_async_work_poll());
    }

    #[test]
    fn nested_anonymous_try_calls_keep_inner_and_outer_destinations() {
        let mut func = TirFunction::new("nested_anonymous_try".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let outer_handler = func.fresh_block();
        let inner_handler = func.fresh_block();
        func.label_id_map.insert(outer_handler.0, 80);
        func.label_id_map.insert(inner_handler.0, 81);
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            op(OpCode::TryStart),
            check(80),
            op(OpCode::Call),
            op(OpCode::TryStart),
            check(81),
            op(OpCode::Call),
            op(OpCode::TryEnd),
            op(OpCode::Call),
            op(OpCode::TryEnd),
        ];
        for (handler, label) in [(outer_handler, 80), (inner_handler, 81)] {
            func.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![labeled_op(OpCode::TryEnd, label)],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }

        run(&mut func, &mut AnalysisManager::new());
        let polls: Vec<_> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.is_async_work_poll())
            .filter_map(check_label)
            .collect();
        assert_eq!(polls, [80, 81, 80]);
    }

    #[test]
    fn loop_backedge_inside_try_uses_the_active_handler() {
        let mut func = TirFunction::new("try_loop".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = func.fresh_block();
        func.blocks.get_mut(&header).unwrap().ops = vec![labeled_op(OpCode::TryStart, 40)];
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        assert_eq!(func.blocks[&latch].ops.len(), 1);
        assert_eq!(check_label(&func.blocks[&latch].ops[0]), Some(40));
    }

    #[test]
    fn one_latch_for_nested_loop_headers_gets_one_poll() {
        use crate::tir::values::ValueId;

        let mut func = TirFunction::new("multi_header_latch".into(), vec![], TirType::None);
        let outer = func.entry_block;
        let inner = func.fresh_block();
        let latch = func.fresh_block();
        func.blocks.get_mut(&outer).unwrap().ops = vec![labeled_op(OpCode::TryStart, 50)];
        func.blocks.get_mut(&outer).unwrap().terminator = Terminator::Branch {
            target: inner,
            args: vec![],
        };
        func.blocks.insert(
            inner,
            TirBlock {
                id: inner,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: latch,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: inner,
                    then_args: vec![],
                    else_block: outer,
                    else_args: vec![],
                },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        let polls = func.blocks[&latch]
            .ops
            .iter()
            .filter(|op| op.is_async_work_poll())
            .count();
        assert_eq!(polls, 1, "one insertion boundary must have one poll");
        assert_eq!(check_label(&func.blocks[&latch].ops[0]), Some(50));
    }

    #[test]
    fn nonlocal_inner_unwinds_preserve_one_outer_handler_at_loop_join() {
        use crate::tir::values::ValueId;

        let mut func = TirFunction::new("nonlocal_inner_unwinds".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let header = func.fresh_block();
        let first_try = func.fresh_block();
        let second_try = func.fresh_block();
        let cond = ValueId(0);
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.get_mut(&entry).unwrap().ops = vec![labeled_op(OpCode::TryStart, 22)];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![op(OpCode::Call)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: first_try,
                    then_args: vec![],
                    else_block: second_try,
                    else_args: vec![],
                },
            },
        );
        for (block, label) in [(first_try, 23), (second_try, 28)] {
            func.blocks.insert(
                block,
                TirBlock {
                    id: block,
                    args: vec![],
                    ops: vec![labeled_op(OpCode::TryStart, label), exception_pop()],
                    terminator: Terminator::Branch {
                        target: header,
                        args: vec![],
                    },
                },
            );
        }

        let facts = crate::tir::exception_regions::compute_exception_region_facts(&func);
        assert_eq!(
            facts.lexical_handler_before(ExceptionOpPosition {
                block: header,
                op_index: 1,
            }),
            Ok(ExceptionBoundaryHandler::Labeled(22)),
            "normal continue/break/return unwinds must not leak an inner try frame into the loop join"
        );
        run(&mut func, &mut AnalysisManager::new());
        let poll = func.blocks[&header]
            .ops
            .iter()
            .find(|op| op.is_async_work_poll())
            .expect("post-call poll");
        assert_eq!(check_label(poll), Some(22));
    }

    #[test]
    fn depth_zero_exit_lowers_as_value_return_for_non_none_function() {
        let mut func = TirFunction::new("value_function".into(), vec![], TirType::I64);
        let entry = func.entry_block;
        let value = func.fresh_value();
        func.value_types.insert(value, TirType::I64);
        let mut constant = labeled_op(OpCode::ConstInt, 7);
        constant.results.push(value);
        func.blocks.get_mut(&entry).unwrap().ops = vec![op(OpCode::Call), constant];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
            values: vec![value],
        };

        run(&mut func, &mut AnalysisManager::new());
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        assert!(simple.iter().any(|op| op.kind == "async_work_poll"));
        assert!(simple.iter().any(|op| op.kind == "label"));
        assert!(
            simple.iter().any(|op| op.kind == "ret"),
            "the ordinary value-return path must remain typed"
        );
        assert!(
            simple.iter().any(|op| op.kind == "ret_void"),
            "an empty TIR Return must lower to the existing return-with-pending backend sentinel"
        );
    }

    #[test]
    fn unreachable_block_call_is_not_a_poll_site() {
        let mut func = TirFunction::new("dead_call_block".into(), vec![], TirType::None);
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        let dead = func.fresh_block();
        func.blocks.insert(
            dead,
            TirBlock {
                id: dead,
                args: vec![],
                ops: vec![op(OpCode::Call)],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.total_changes(), 0);
        assert_eq!(func.blocks[&dead].ops.len(), 1);
        assert!(!func.has_exception_handling);
    }

    #[test]
    fn unreachable_in_block_post_call_boundary_is_not_a_poll_site() {
        let mut func = TirFunction::new("dead_post_transfer_call".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 91);
        func.blocks.get_mut(&entry).unwrap().ops =
            vec![op(OpCode::Raise), check(91), op(OpCode::Call)];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.total_changes(), 0);
        assert_eq!(func.blocks[&entry].ops.len(), 3);
        assert!(
            !func.blocks[&entry]
                .ops
                .iter()
                .any(TirOp::is_async_work_poll)
        );
    }

    #[test]
    fn unreachable_loop_latch_is_not_a_poll_site() {
        let mut func = TirFunction::new("dead_loop".into(), vec![], TirType::None);
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        let header = func.fresh_block();
        let latch = func.fresh_block();
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: latch,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.total_changes(), 0);
        assert!(func.blocks[&latch].ops.is_empty());
        assert!(!func.has_exception_handling);
    }
}
