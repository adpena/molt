//! Representation-aware LIR verifier.
//!
//! This checker is intentionally narrow in the first Task 1 slice. It proves
//! the core invariants required before any backend starts consuming LIR:
//! - entry block exists;
//! - every branch passes the right number of arguments;
//! - branch arguments match the target block parameters in semantic type and
//!   low-level representation;
//! - conditional branches consume `Bool1`;
//! - return values match the declared function return arity and representation.

use std::collections::{HashMap, HashSet};

use super::blocks::BlockId;
use super::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use super::ops::{AttrValue, OpCode};
use super::types::TirType;
use super::values::ValueId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LirVerifyError {
    pub block: Option<BlockId>,
    pub op_index: Option<usize>,
    pub message: String,
}

impl LirVerifyError {
    fn func(message: impl Into<String>) -> Self {
        Self {
            block: None,
            op_index: None,
            message: message.into(),
        }
    }

    fn block(block: BlockId, message: impl Into<String>) -> Self {
        Self {
            block: Some(block),
            op_index: None,
            message: message.into(),
        }
    }
}

pub fn verify_lir_function(func: &LirFunction) -> Result<(), Vec<LirVerifyError>> {
    let mut errors = Vec::new();
    if !func.blocks.contains_key(&func.entry_block) {
        errors.push(LirVerifyError::func(format!(
            "entry block ^{} does not exist in blocks map",
            func.entry_block
        )));
        return Err(errors);
    }

    verify_entry_block_signature(func, &mut errors);
    let values = build_value_table(func, &mut errors);
    let dominators = compute_dominator_tree(func);
    verify_ops(func, &values, &dominators, &mut errors);
    verify_terminators(func, &values, &dominators, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[derive(Debug, Clone)]
struct ValueDef {
    value: LirValue,
    block: BlockId,
    op_index: Option<usize>,
}

#[derive(Debug, Default)]
struct DominatorInfo {
    preorder: HashMap<BlockId, usize>,
    postorder: HashMap<BlockId, usize>,
}

impl DominatorInfo {
    fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        if a == b {
            return true;
        }
        match (
            self.preorder.get(&a),
            self.preorder.get(&b),
            self.postorder.get(&a),
            self.postorder.get(&b),
        ) {
            (Some(&a_pre), Some(&b_pre), Some(&a_post), Some(&b_post)) => {
                a_pre <= b_pre && b_post <= a_post
            }
            _ => false,
        }
    }
}

fn verify_entry_block_signature(func: &LirFunction, errors: &mut Vec<LirVerifyError>) {
    if func.param_names.len() != func.param_types.len() {
        errors.push(LirVerifyError::func(format!(
            "function {} declares {} param names but {} param types",
            func.name,
            func.param_names.len(),
            func.param_types.len()
        )));
    }

    let Some(entry_block) = func.blocks.get(&func.entry_block) else {
        return;
    };

    if entry_block.args.len() != func.param_types.len() {
        errors.push(LirVerifyError::block(
            func.entry_block,
            format!(
                "entry block ^{} expects {} params but function signature declares {}",
                func.entry_block,
                entry_block.args.len(),
                func.param_types.len()
            ),
        ));
        return;
    }

    for (idx, (actual, expected_ty)) in entry_block
        .args
        .iter()
        .zip(func.param_types.iter())
        .enumerate()
    {
        let expected_repr = LirRepr::for_type(expected_ty);
        if expected_repr != LirRepr::DynBox && actual.ty != *expected_ty {
            errors.push(LirVerifyError::block(
                func.entry_block,
                format!(
                    "entry block ^{} type mismatch for param {}: expected {:?}, found {:?}",
                    func.entry_block, idx, expected_ty, actual.ty
                ),
            ));
        }
        if actual.repr != expected_repr {
            errors.push(LirVerifyError::block(
                func.entry_block,
                format!(
                    "entry block ^{} representation mismatch for param {}: expected {:?}, found {:?}",
                    func.entry_block, idx, expected_repr, actual.repr
                ),
            ));
        }
    }
}

fn build_value_table(
    func: &LirFunction,
    errors: &mut Vec<LirVerifyError>,
) -> HashMap<ValueId, ValueDef> {
    let mut table = HashMap::new();
    for (bid, block) in &func.blocks {
        if block.id != *bid {
            errors.push(LirVerifyError::block(
                *bid,
                format!(
                    "block map key ^{} does not match embedded id ^{}",
                    bid, block.id
                ),
            ));
        }
        for arg in &block.args {
            if table
                .insert(
                    arg.id,
                    ValueDef {
                        value: arg.clone(),
                        block: *bid,
                        op_index: None,
                    },
                )
                .is_some()
            {
                errors.push(LirVerifyError::block(
                    *bid,
                    format!("duplicate definition of {}", arg.id),
                ));
            }
        }
        for (op_index, op) in block.ops.iter().enumerate() {
            insert_op_results(*bid, op_index, op, &mut table, errors);
        }
    }
    table
}

fn insert_op_results(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    table: &mut HashMap<ValueId, ValueDef>,
    errors: &mut Vec<LirVerifyError>,
) {
    for value in &op.result_values {
        if table
            .insert(
                value.id,
                ValueDef {
                    value: value.clone(),
                    block: bid,
                    op_index: Some(op_index),
                },
            )
            .is_some()
        {
            errors.push(LirVerifyError::block(
                bid,
                format!("duplicate definition of {}", value.id),
            ));
        }
    }
}

fn compute_dominators(func: &LirFunction) -> HashMap<BlockId, Option<BlockId>> {
    if func.blocks.is_empty() {
        return HashMap::new();
    }

    let rpo = bfs_order(func);
    let rpo_index: HashMap<BlockId, usize> = rpo.iter().enumerate().map(|(i, &b)| (b, i)).collect();

    let mut pred: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for bid in func.blocks.keys() {
        pred.entry(*bid).or_default();
    }
    let label_to_block = exception_label_to_block(func);
    for (bid, block) in &func.blocks {
        for succ in terminator_successors(&block.terminator) {
            pred.entry(succ).or_default().push(*bid);
        }
        for succ in exception_successors(block, &label_to_block) {
            pred.entry(succ).or_default().push(*bid);
        }
    }

    let mut idom: HashMap<BlockId, Option<BlockId>> = HashMap::new();
    let entry = func.entry_block;
    idom.insert(entry, None);

    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo {
            if b == entry {
                continue;
            }
            let preds = pred.get(&b).cloned().unwrap_or_default();
            let mut new_idom: Option<BlockId> = None;
            for &p in &preds {
                if idom.contains_key(&p) {
                    new_idom = Some(match new_idom {
                        None => p,
                        Some(cur) => intersect_dom(&idom, &rpo_index, cur, p),
                    });
                }
            }
            let old = idom.get(&b).copied().flatten();
            if !idom.contains_key(&b) || old != new_idom {
                idom.insert(b, new_idom);
                changed = true;
            }
        }
    }

    idom
}

fn compute_dominator_tree(func: &LirFunction) -> DominatorInfo {
    let idom = compute_dominators(func);
    if idom.is_empty() {
        return DominatorInfo::default();
    }

    let mut children: HashMap<BlockId, Vec<BlockId>> = HashMap::with_capacity(idom.len());
    for &block in idom.keys() {
        children.entry(block).or_default();
    }
    for (&block, parent) in &idom {
        if let Some(parent) = *parent {
            children.entry(parent).or_default().push(block);
        }
    }

    let mut preorder: HashMap<BlockId, usize> = HashMap::with_capacity(idom.len());
    let mut postorder: HashMap<BlockId, usize> = HashMap::with_capacity(idom.len());
    let mut tick = 0usize;
    let entry = func.entry_block;

    if idom.contains_key(&entry) {
        preorder.insert(entry, tick);
        tick += 1;
        let mut stack: Vec<(BlockId, usize)> = vec![(entry, 0)];
        while let Some((node, child_idx)) = stack.last_mut() {
            let next_child = children
                .get(node)
                .and_then(|child_list| child_list.get(*child_idx))
                .copied();
            if let Some(child) = next_child {
                *child_idx += 1;
                if preorder.contains_key(&child) {
                    continue;
                }
                preorder.insert(child, tick);
                tick += 1;
                stack.push((child, 0));
            } else {
                postorder.insert(*node, tick);
                tick += 1;
                stack.pop();
            }
        }
    }

    DominatorInfo {
        preorder,
        postorder,
    }
}

fn intersect_dom(
    idom: &HashMap<BlockId, Option<BlockId>>,
    rpo: &HashMap<BlockId, usize>,
    mut a: BlockId,
    mut b: BlockId,
) -> BlockId {
    let rpo_of = |x: BlockId| rpo.get(&x).copied().unwrap_or(usize::MAX);
    let max_iters = rpo.len() * 2 + 1;
    let mut iters = 0usize;
    while a != b {
        iters += 1;
        if iters > max_iters {
            break;
        }
        while rpo_of(a) > rpo_of(b) {
            match idom.get(&a).and_then(|x| *x) {
                Some(p) if p != a => a = p,
                _ => break,
            }
        }
        while rpo_of(b) > rpo_of(a) {
            match idom.get(&b).and_then(|x| *x) {
                Some(p) if p != b => b = p,
                _ => break,
            }
        }
        let a_rpo = rpo_of(a);
        let b_rpo = rpo_of(b);
        if a_rpo == b_rpo && a != b {
            break;
        }
    }
    a
}

fn bfs_order(func: &LirFunction) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut order = Vec::new();

    queue.push_back(func.entry_block);
    visited.insert(func.entry_block);

    let label_to_block = exception_label_to_block(func);

    while let Some(bid) = queue.pop_front() {
        order.push(bid);
        if let Some(block) = func.blocks.get(&bid) {
            for succ in terminator_successors(&block.terminator) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
            for succ in exception_successors(block, &label_to_block) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }

    order
}

fn terminator_successors(terminator: &LirTerminator) -> Vec<BlockId> {
    match terminator {
        LirTerminator::Branch { target, .. } => vec![*target],
        LirTerminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        LirTerminator::Switch { cases, default, .. } => {
            let mut targets = cases.iter().map(|(_, block, _)| *block).collect::<Vec<_>>();
            targets.push(*default);
            targets
        }
        LirTerminator::Return { .. } | LirTerminator::Unreachable => Vec::new(),
    }
}

/// Build the inverse of `LirFunction::label_id_map` for resolving exception
/// edges encoded as op `value` attrs.
fn exception_label_to_block(func: &LirFunction) -> HashMap<i64, BlockId> {
    func.label_id_map
        .iter()
        .map(|(&bid, &label_id)| (label_id, BlockId(bid)))
        .collect()
}

/// Return the implicit successors of `block` that are reached only via
/// exception flow — encoded by `CheckException`/`TryStart`/`TryEnd` ops
/// with a `value` attr giving the target label_id. The LIR verifier needs
/// to follow these edges so that exception handler blocks are considered
/// reachable from the function entry; otherwise their value uses appear to
/// violate dominance even though at runtime control flow correctly reaches
/// them via the runtime exception path.
fn exception_successors(
    block: &LirBlock,
    label_to_block: &HashMap<i64, BlockId>,
) -> Vec<BlockId> {
    let mut successors = Vec::new();
    for op in &block.ops {
        if matches!(
            op.tir_op.opcode,
            OpCode::CheckException | OpCode::TryStart | OpCode::TryEnd
        ) && let Some(AttrValue::Int(target_label)) = op.tir_op.attrs.get("value")
            && let Some(&target) = label_to_block.get(target_label)
        {
            successors.push(target);
        }
    }
    successors
}

fn verify_ops(
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &DominatorInfo,
    errors: &mut Vec<LirVerifyError>,
) {
    for (bid, block) in &func.blocks {
        for (op_index, op) in block.ops.iter().enumerate() {
            verify_op_surface(*bid, op_index, op, errors);
            for operand in &op.tir_op.operands {
                verify_use_dominates(
                    *bid,
                    op_index,
                    *operand,
                    values,
                    dominators,
                    errors,
                    "op operand",
                );
            }
            match op.tir_op.opcode {
                OpCode::BoxVal => verify_box_op(*bid, op_index, op, values, errors),
                OpCode::UnboxVal => verify_unbox_op(*bid, op_index, op, values, errors),
                OpCode::Add | OpCode::Sub | OpCode::Mul => {
                    verify_checked_i64_arithmetic(*bid, op_index, op, errors)
                }
                OpCode::CallBuiltin => verify_truthy_materialization(*bid, op_index, op, errors),
                _ => {}
            }
        }
    }
}

fn verify_op_surface(bid: BlockId, op_index: usize, op: &LirOp, errors: &mut Vec<LirVerifyError>) {
    if op.tir_op.results.len() != op.result_values.len() {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "op result arity drift: tir has {} results but lir has {}",
                op.tir_op.results.len(),
                op.result_values.len()
            ),
        });
        return;
    }

    for (slot, (tir_id, lir_value)) in op
        .tir_op
        .results
        .iter()
        .zip(op.result_values.iter())
        .enumerate()
    {
        if *tir_id != lir_value.id {
            errors.push(LirVerifyError {
                block: Some(bid),
                op_index: Some(op_index),
                message: format!(
                    "result id drift at slot {}: tir uses {} but lir uses {}",
                    slot, tir_id, lir_value.id
                ),
            });
        }
    }
}

fn verify_checked_i64_arithmetic(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    errors: &mut Vec<LirVerifyError>,
) {
    let checked = matches!(
        op.tir_op.attrs.get("lir.checked_overflow"),
        Some(AttrValue::Bool(true))
    );
    if !checked {
        return;
    }
    if op.result_values.len() != 3 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic requires 3 results, found {}",
                op.result_values.len()
            ),
        });
        return;
    }
    let main = &op.result_values[0];
    let overflow_box = &op.result_values[1];
    let overflow_flag = &op.result_values[2];
    if main.ty != TirType::I64 || main.repr != LirRepr::I64 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic main result must be I64/I64, found {:?}/{:?}",
                main.ty, main.repr
            ),
        });
    }
    if overflow_box.ty != TirType::DynBox || overflow_box.repr != LirRepr::DynBox {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic overflow box must be DynBox/DynBox, found {:?}/{:?}",
                overflow_box.ty, overflow_box.repr
            ),
        });
    }
    if overflow_flag.ty != TirType::Bool || overflow_flag.repr != LirRepr::Bool1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "checked i64 arithmetic overflow flag must be Bool/Bool1, found {:?}/{:?}",
                overflow_flag.ty, overflow_flag.repr
            ),
        });
    }
}

fn verify_truthy_materialization(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    errors: &mut Vec<LirVerifyError>,
) {
    let truthy = matches!(
        op.tir_op.attrs.get("lir.truthy_cond"),
        Some(AttrValue::Bool(true))
    );
    if !truthy {
        return;
    }
    if op.tir_op.operands.len() != 1 || op.result_values.len() != 1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "truthiness materialization requires one operand and one result".to_string(),
        });
        return;
    }
    let result = &op.result_values[0];
    if result.ty != TirType::Bool || result.repr != LirRepr::Bool1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "truthiness materialization must produce Bool/Bool1, found {:?}/{:?}",
                result.ty, result.repr
            ),
        });
    }
}

fn verify_box_op(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    _values: &HashMap<ValueId, ValueDef>,
    errors: &mut Vec<LirVerifyError>,
) {
    if op.tir_op.operands.len() != 1 || op.result_values.len() != 1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "box op requires exactly one operand and one result".to_string(),
        });
        return;
    }
    let result = &op.result_values[0];
    if result.repr != LirRepr::DynBox {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "box op must produce a DynBox-lane result, found {:?}/{:?}",
                result.ty, result.repr,
            ),
        });
    }
}

fn verify_unbox_op(
    bid: BlockId,
    op_index: usize,
    op: &LirOp,
    values: &HashMap<ValueId, ValueDef>,
    errors: &mut Vec<LirVerifyError>,
) {
    if op.tir_op.operands.len() != 1 || op.result_values.len() != 1 {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "unbox op requires exactly one operand and one result".to_string(),
        });
        return;
    }
    let result = &op.result_values[0];
    if result.repr == LirRepr::DynBox {
        errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: "unbox op must produce a non-DynBox result".to_string(),
        });
    }
    match values.get(&op.tir_op.operands[0]) {
        Some(def)
            if def.value.repr == LirRepr::DynBox
                && matches!(def.value.ty, TirType::DynBox | TirType::Box(_)) => {}
        Some(def) => errors.push(LirVerifyError {
            block: Some(bid),
            op_index: Some(op_index),
            message: format!(
                "unbox op requires Box(_) or DynBox operand with DynBox repr, found {:?}/{:?}",
                def.value.ty, def.value.repr
            ),
        }),
        None => {}
    }
}

fn verify_terminators(
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &DominatorInfo,
    errors: &mut Vec<LirVerifyError>,
) {
    for (bid, block) in &func.blocks {
        let use_index = block.ops.len();
        match &block.terminator {
            LirTerminator::Branch { target, args } => {
                verify_branch_args(
                    *bid, use_index, *target, args, func, values, dominators, errors,
                );
            }
            LirTerminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                verify_use_dominates(
                    *bid,
                    use_index,
                    *cond,
                    values,
                    dominators,
                    errors,
                    "conditional branch condition",
                );
                match values.get(cond) {
                    Some(def) if def.value.repr == LirRepr::Bool1 && def.value.ty == TirType::Bool => {}
                    Some(def) if def.value.repr != LirRepr::Bool1 => errors.push(LirVerifyError::block(
                        *bid,
                        format!(
                            "conditional branch requires Bool1 condition, found {:?} for {}",
                            def.value.repr, def.value.id
                        ),
                    )),
                    Some(def) => errors.push(LirVerifyError::block(
                        *bid,
                        format!(
                            "conditional branch requires semantic Bool condition, found {:?} for {}",
                            def.value.ty, def.value.id
                        ),
                    )),
                    None => {}
                }
                verify_branch_args(
                    *bid,
                    use_index,
                    *then_block,
                    then_args,
                    func,
                    values,
                    dominators,
                    errors,
                );
                verify_branch_args(
                    *bid,
                    use_index,
                    *else_block,
                    else_args,
                    func,
                    values,
                    dominators,
                    errors,
                );
            }
            LirTerminator::Return {
                values: return_values,
            } => {
                if return_values.len() != func.return_types.len() {
                    errors.push(LirVerifyError::block(
                        *bid,
                        format!(
                            "return arity mismatch: expected {}, found {}",
                            func.return_types.len(),
                            return_values.len()
                        ),
                    ));
                    continue;
                }
                for (idx, (value_id, expected_ty)) in return_values
                    .iter()
                    .zip(func.return_types.iter())
                    .enumerate()
                {
                    verify_use_dominates(
                        *bid,
                        use_index,
                        *value_id,
                        values,
                        dominators,
                        errors,
                        "return value",
                    );
                    let expected_repr = LirRepr::for_type(expected_ty);
                    if let Some(def) = values.get(value_id) {
                        if expected_repr != LirRepr::DynBox && def.value.ty != *expected_ty {
                            errors.push(LirVerifyError::block(
                                *bid,
                                format!(
                                    "return value {} type mismatch at slot {}: expected {:?}, found {:?}",
                                    def.value.id, idx, expected_ty, def.value.ty
                                ),
                            ));
                        }
                        if def.value.repr != expected_repr {
                            errors.push(LirVerifyError::block(
                                *bid,
                                format!(
                                    "return value {} representation mismatch at slot {}: expected {:?}, found {:?}",
                                    def.value.id, idx, expected_repr, def.value.repr
                                ),
                            ));
                        }
                    }
                }
            }
            LirTerminator::Switch {
                value,
                cases,
                default,
                default_args,
            } => {
                verify_use_dominates(
                    *bid,
                    use_index,
                    *value,
                    values,
                    dominators,
                    errors,
                    "switch value",
                );
                for (_, target, args) in cases {
                    verify_branch_args(
                        *bid, use_index, *target, args, func, values, dominators, errors,
                    );
                }
                verify_branch_args(
                    *bid,
                    use_index,
                    *default,
                    default_args,
                    func,
                    values,
                    dominators,
                    errors,
                );
            }
            LirTerminator::Unreachable => {}
        }
    }
}

fn verify_branch_args(
    source: BlockId,
    use_index: usize,
    target: BlockId,
    args: &[ValueId],
    func: &LirFunction,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &DominatorInfo,
    errors: &mut Vec<LirVerifyError>,
) {
    let Some(target_block) = func.blocks.get(&target) else {
        errors.push(LirVerifyError::block(
            source,
            format!("branch targets missing block ^{}", target),
        ));
        return;
    };

    if args.len() != target_block.args.len() {
        errors.push(LirVerifyError::block(
            source,
            format!(
                "branch to ^{} passes {} args but target expects {}",
                target,
                args.len(),
                target_block.args.len()
            ),
        ));
        return;
    }

    for (idx, (arg_id, expected)) in args.iter().zip(target_block.args.iter()).enumerate() {
        verify_use_dominates(
            source,
            use_index,
            *arg_id,
            values,
            dominators,
            errors,
            "branch argument",
        );
        if let Some(actual) = values.get(arg_id) {
            if expected.repr != LirRepr::DynBox && actual.value.ty != expected.ty {
                errors.push(LirVerifyError::block(
                    source,
                    format!(
                        "branch type mismatch for target ^{} arg {}: expected {:?}, found {:?}",
                        target, idx, expected.ty, actual.value.ty
                    ),
                ));
            }
            if actual.value.repr != expected.repr {
                errors.push(LirVerifyError::block(
                    source,
                    format!(
                        "branch representation mismatch for target ^{} arg {}: expected {:?}, found {:?}",
                        target, idx, expected.repr, actual.value.repr
                    ),
                ));
            }
        }
    }
}

fn verify_use_dominates(
    use_block: BlockId,
    use_index: usize,
    value_id: ValueId,
    values: &HashMap<ValueId, ValueDef>,
    dominators: &DominatorInfo,
    errors: &mut Vec<LirVerifyError>,
    context: &str,
) {
    match values.get(&value_id) {
        Some(def) if definition_dominates(def, use_block, use_index, dominators) => {}
        Some(def) => errors.push(LirVerifyError::block(
            use_block,
            format!(
                "{context} {} defined in ^{} does not dominate use in ^{}",
                value_id, def.block, use_block
            ),
        )),
        None => errors.push(LirVerifyError::block(
            use_block,
            format!("{context} uses undefined value {}", value_id),
        )),
    }
}

fn definition_dominates(
    def: &ValueDef,
    use_block: BlockId,
    use_index: usize,
    dominators: &DominatorInfo,
) -> bool {
    if def.block == use_block {
        match def.op_index {
            None => true,
            Some(def_index) => def_index < use_index,
        }
    } else {
        dominators.dominates(def.block, use_block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::BlockId;
    use crate::tir::lir::{LirBlock, LirFunction, LirTerminator};

    fn value(id: u32, ty: TirType, repr: LirRepr) -> LirValue {
        LirValue {
            id: ValueId(id),
            ty,
            repr,
        }
    }

    #[test]
    fn repr_for_bool_return_must_match_bool1() {
        let entry = LirBlock {
            id: BlockId(0),
            args: vec![value(0, TirType::Bool, LirRepr::Bool1)],
            ops: vec![],
            terminator: LirTerminator::Return {
                values: vec![ValueId(0)],
            },
        };
        let mut blocks = HashMap::new();
        blocks.insert(BlockId(0), entry);
        let func = LirFunction {
            name: "bool_return".to_string(),
            param_names: vec!["flag".to_string()],
            param_types: vec![TirType::Bool],
            return_types: vec![TirType::Bool],
            blocks,
            entry_block: BlockId(0),
            label_id_map: HashMap::new(),
        };
        assert!(verify_lir_function(&func).is_ok());
    }
}
