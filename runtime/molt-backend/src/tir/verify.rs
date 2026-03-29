//! TIR invariant checker.
//!
//! Verifies that a [`TirFunction`] is well-formed SSA. Call
//! [`verify_function`] to get a list of [`VerifyError`]s; an empty list
//! means the function is valid.

use std::collections::{HashMap, HashSet, VecDeque};

use super::blocks::{BlockId, Terminator};
use super::function::TirFunction;
use super::ops::OpCode;
use super::values::ValueId;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single verification error with location context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// Block where the error was detected (if applicable).
    pub block: Option<BlockId>,
    /// Op index within the block (if applicable).
    pub op_index: Option<usize>,
    /// Human-readable description.
    pub message: String,
}

impl VerifyError {
    fn func(msg: impl Into<String>) -> Self {
        Self {
            block: None,
            op_index: None,
            message: msg.into(),
        }
    }

    fn block(bid: BlockId, msg: impl Into<String>) -> Self {
        Self {
            block: Some(bid),
            op_index: None,
            message: msg.into(),
        }
    }

    fn op(bid: BlockId, op_idx: usize, msg: impl Into<String>) -> Self {
        Self {
            block: Some(bid),
            op_index: Some(op_idx),
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.block, self.op_index) {
            (None, _) => write!(f, "[func] {}", self.message),
            (Some(bid), None) => write!(f, "[^{}] {}", bid, self.message),
            (Some(bid), Some(idx)) => write!(f, "[^{} op#{}] {}", bid, idx, self.message),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Verify that `func` satisfies all TIR well-formedness invariants.
///
/// Returns `Ok(())` if the function is valid, or `Err(errors)` with a
/// non-empty list of all violations found.
pub fn verify_function(func: &TirFunction) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();
    verify_entry_block(func, &mut errors);
    verify_no_duplicate_values(func, &mut errors);
    verify_op_attributes(func, &mut errors);
    verify_terminators(func, &mut errors);
    verify_block_args(func, &mut errors);
    verify_ssa(func, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Check 1: entry block exists
// ---------------------------------------------------------------------------

fn verify_entry_block(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    if !func.blocks.contains_key(&func.entry_block) {
        errors.push(VerifyError::func(format!(
            "entry block ^{} does not exist in blocks map",
            func.entry_block
        )));
    }
}

// ---------------------------------------------------------------------------
// Check 2: no duplicate ValueIds
// ---------------------------------------------------------------------------

fn verify_no_duplicate_values(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    let mut defined: HashSet<ValueId> = HashSet::new();

    for (bid, block) in &func.blocks {
        // Block arguments count as definitions.
        for arg in &block.args {
            if !defined.insert(arg.id) {
                errors.push(VerifyError::block(
                    *bid,
                    format!("duplicate definition of {}", arg.id),
                ));
            }
        }
        // Op results count as definitions.
        for (op_idx, op) in block.ops.iter().enumerate() {
            for result in &op.results {
                if !defined.insert(*result) {
                    errors.push(VerifyError::op(
                        *bid,
                        op_idx,
                        format!("duplicate definition of {}", result),
                    ));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Check 3: op-level attribute and operand validation
// ---------------------------------------------------------------------------

fn verify_op_attributes(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    for (bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            // Check required attributes per opcode.
            // NOTE: Constant ops (ConstInt, ConstFloat, ConstStr, ConstBytes)
            // intentionally skip attribute checks because the lowering from
            // SimpleIR may produce placeholder constants (e.g. `const` with
            // no value) that are later consumed by type refinement. These
            // ops are structurally valid even without their value attribute.
            match op.opcode {
                OpCode::Call | OpCode::CallBuiltin => {
                    // Callee can be either an attribute or the first operand
                    // (SimpleIR encodes it as `var`, which becomes an operand).
                    if !op.attrs.contains_key("callee")
                        && !op.attrs.contains_key("s_value")
                        && op.operands.is_empty()
                    {
                        errors.push(VerifyError::op(
                            *bid,
                            op_idx,
                            format!(
                                "{:?} op has no callee (attr or operand)",
                                op.opcode
                            ),
                        ));
                    }
                }
                OpCode::CallMethod => {
                    if !op.attrs.contains_key("method")
                        && !op.attrs.contains_key("callee")
                        && !op.attrs.contains_key("s_value")
                        && op.operands.is_empty()
                    {
                        errors.push(VerifyError::op(
                            *bid,
                            op_idx,
                            "CallMethod op has no method (attr or operand)",
                        ));
                    }
                }
                _ => {}
            }

            // Check expected result counts for well-known opcodes.
            let expected_results = match op.opcode {
                // These produce exactly one result.
                OpCode::ConstInt
                | OpCode::ConstFloat
                | OpCode::ConstStr
                | OpCode::ConstBool
                | OpCode::ConstNone
                | OpCode::ConstBytes
                | OpCode::Add
                | OpCode::Sub
                | OpCode::Mul
                | OpCode::Div
                | OpCode::FloorDiv
                | OpCode::Mod
                | OpCode::Pow
                | OpCode::Neg
                | OpCode::Pos
                | OpCode::Eq
                | OpCode::Ne
                | OpCode::Lt
                | OpCode::Le
                | OpCode::Gt
                | OpCode::Ge
                | OpCode::Is
                | OpCode::IsNot
                | OpCode::In
                | OpCode::NotIn
                | OpCode::BitAnd
                | OpCode::BitOr
                | OpCode::BitXor
                | OpCode::BitNot
                | OpCode::Shl
                | OpCode::Shr
                | OpCode::And
                | OpCode::Or
                | OpCode::Not
                | OpCode::BoxVal
                | OpCode::UnboxVal
                | OpCode::TypeGuard
                | OpCode::Index
                | OpCode::LoadAttr
                | OpCode::GetIter
                | OpCode::IterNext
                | OpCode::ForIter
                | OpCode::Alloc
                | OpCode::StackAlloc
                | OpCode::BuildList
                | OpCode::BuildDict
                | OpCode::BuildTuple
                | OpCode::BuildSet
                | OpCode::BuildSlice
                | OpCode::CheckException
                | OpCode::Import
                | OpCode::ImportFrom => Some(1),
                // These produce zero results (side-effecting only).
                OpCode::IncRef
                | OpCode::DecRef
                | OpCode::StoreAttr
                | OpCode::DelAttr
                | OpCode::StoreIndex
                | OpCode::DelIndex
                | OpCode::Free
                | OpCode::Raise
                | OpCode::Deopt => Some(0),
                // Variable/unknown result count.
                _ => None,
            };

            if let Some(expected) = expected_results {
                if op.results.len() != expected {
                    errors.push(VerifyError::op(
                        *bid,
                        op_idx,
                        format!(
                            "{:?} op has {} results but expected {}",
                            op.opcode,
                            op.results.len(),
                            expected
                        ),
                    ));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Check 4: every block has a well-formed terminator (block exists in function)
// ---------------------------------------------------------------------------

fn verify_terminators(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    for (bid, block) in &func.blocks {
        match &block.terminator {
            Terminator::Branch { target, .. } => {
                if !func.blocks.contains_key(target) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("branch target ^{} does not exist", target),
                    ));
                }
            }
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                if !func.blocks.contains_key(then_block) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("cond_branch then_block ^{} does not exist", then_block),
                    ));
                }
                if !func.blocks.contains_key(else_block) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("cond_branch else_block ^{} does not exist", else_block),
                    ));
                }
            }
            Terminator::Switch { cases, default, .. } => {
                if !func.blocks.contains_key(default) {
                    errors.push(VerifyError::block(
                        *bid,
                        format!("switch default block ^{} does not exist", default),
                    ));
                }
                for (case_val, target, _) in cases {
                    if !func.blocks.contains_key(target) {
                        errors.push(VerifyError::block(
                            *bid,
                            format!("switch case {} target ^{} does not exist", case_val, target),
                        ));
                    }
                }
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Check 4: branch arg counts match target block param counts
// ---------------------------------------------------------------------------

fn verify_block_args(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    let arg_count = |bid: &BlockId| -> Option<usize> { func.blocks.get(bid).map(|b| b.args.len()) };

    for (bid, block) in &func.blocks {
        match &block.terminator {
            Terminator::Branch { target, args } => {
                if let Some(expected) = arg_count(target)
                    && args.len() != expected {
                        errors.push(VerifyError::block(
                            *bid,
                            format!(
                                "branch to ^{} passes {} args but block expects {}",
                                target,
                                args.len(),
                                expected
                            ),
                        ));
                    }
            }
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if let Some(expected) = arg_count(then_block)
                    && then_args.len() != expected {
                        errors.push(VerifyError::block(
                            *bid,
                            format!(
                                "cond_branch to ^{} passes {} then_args but block expects {}",
                                then_block,
                                then_args.len(),
                                expected
                            ),
                        ));
                    }
                if let Some(expected) = arg_count(else_block)
                    && else_args.len() != expected {
                        errors.push(VerifyError::block(
                            *bid,
                            format!(
                                "cond_branch to ^{} passes {} else_args but block expects {}",
                                else_block,
                                else_args.len(),
                                expected
                            ),
                        ));
                    }
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                if let Some(expected) = arg_count(default)
                    && default_args.len() != expected {
                        errors.push(VerifyError::block(
                            *bid,
                            format!(
                                "switch default ^{} passed {} args but block expects {}",
                                default,
                                default_args.len(),
                                expected
                            ),
                        ));
                    }
                for (case_val, target, args) in cases {
                    if let Some(expected) = arg_count(target)
                        && args.len() != expected {
                            errors.push(VerifyError::block(
                                *bid,
                                format!(
                                    "switch case {} to ^{} passes {} args but block expects {}",
                                    case_val,
                                    target,
                                    args.len(),
                                    expected
                                ),
                            ));
                        }
                }
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Check 5: SSA dominance — every use must be dominated by its definition
// ---------------------------------------------------------------------------

fn verify_ssa(func: &TirFunction, errors: &mut Vec<VerifyError>) {
    // Compute block ordering (BFS from entry) and build dominator tree.
    let dom = compute_dominators(func);

    // Build a map: ValueId → BlockId where it is defined.
    let mut def_block: HashMap<ValueId, BlockId> = HashMap::new();
    // Also track the op index within the block (for same-block use-before-def checks).
    let mut def_op_index: HashMap<ValueId, Option<usize>> = HashMap::new(); // None = block arg

    for (bid, block) in &func.blocks {
        for arg in &block.args {
            def_block.insert(arg.id, *bid);
            def_op_index.insert(arg.id, None);
        }
        for (op_idx, op) in block.ops.iter().enumerate() {
            for result in &op.results {
                def_block.insert(*result, *bid);
                def_op_index.insert(*result, Some(op_idx));
            }
        }
    }

    // Check every operand use.
    let check_use = |bid: BlockId,
                     op_idx: Option<usize>,
                     used: ValueId,
                     errors: &mut Vec<VerifyError>| {
        match def_block.get(&used) {
            None => {
                let msg = format!("{} used but never defined", used);
                match op_idx {
                    Some(i) => errors.push(VerifyError::op(bid, i, msg)),
                    None => errors.push(VerifyError::block(bid, msg)),
                }
            }
            Some(&def_bid) => {
                if def_bid == bid {
                    // Same block: ensure definition comes before use.
                    if let (Some(use_idx), Some(def_idx_opt)) = (op_idx, def_op_index.get(&used))
                        && let Some(def_idx) = def_idx_opt
                            && *def_idx >= use_idx {
                                errors.push(VerifyError::op(
                                    bid,
                                    use_idx,
                                    format!(
                                        "{} used at op#{} but defined later at op#{}",
                                        used, use_idx, def_idx
                                    ),
                                ));
                            }
                        // def_idx_opt == None means it's a block arg, always dominates.
                } else {
                    // Different block: def_bid must dominate bid.
                    if !dominates(&dom, def_bid, bid) {
                        let msg = format!(
                            "{} defined in ^{} does not dominate use in ^{}",
                            used, def_bid, bid
                        );
                        match op_idx {
                            Some(i) => errors.push(VerifyError::op(bid, i, msg)),
                            None => errors.push(VerifyError::block(bid, msg)),
                        }
                    }
                }
            }
        }
    };

    for (bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            for operand in &op.operands {
                check_use(*bid, Some(op_idx), *operand, errors);
            }
        }
        // Check terminator operands.
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for v in args {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                check_use(*bid, None, *cond, errors);
                for v in then_args {
                    check_use(*bid, None, *v, errors);
                }
                for v in else_args {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                check_use(*bid, None, *value, errors);
                for (_, _, args) in cases {
                    for v in args {
                        check_use(*bid, None, *v, errors);
                    }
                }
                for v in default_args {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::Return { values } => {
                for v in values {
                    check_use(*bid, None, *v, errors);
                }
            }
            Terminator::Unreachable => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Dominator helpers (simple BFS-based for correctness over performance)
// ---------------------------------------------------------------------------

/// Compute immediate dominator for each block, returning a map
/// `BlockId → Option<BlockId>` (None = entry block / no idom).
fn compute_dominators(func: &TirFunction) -> HashMap<BlockId, Option<BlockId>> {
    if func.blocks.is_empty() {
        return HashMap::new();
    }

    // BFS to find reachable blocks and RPO order.
    let rpo = bfs_order(func);
    let rpo_index: HashMap<BlockId, usize> = rpo.iter().enumerate().map(|(i, &b)| (b, i)).collect();

    // Build predecessor map.
    let mut pred: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for bid in func.blocks.keys() {
        pred.entry(*bid).or_default();
    }
    for (bid, block) in &func.blocks {
        for succ in successors_of(block) {
            pred.entry(succ).or_default().push(*bid);
        }
    }

    // Simple iterative dominator algorithm (Cooper et al.).
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
            // Find the first predecessor that has already been assigned a dominator.
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
            let new_val = new_idom;
            if !idom.contains_key(&b) || old != new_val {
                idom.insert(b, new_val);
                changed = true;
            }
        }
    }

    idom
}

fn intersect_dom(
    idom: &HashMap<BlockId, Option<BlockId>>,
    rpo: &HashMap<BlockId, usize>,
    mut a: BlockId,
    mut b: BlockId,
) -> BlockId {
    let rpo_of = |x: BlockId| rpo.get(&x).copied().unwrap_or(usize::MAX);
    // Safety bound: at most N iterations where N = number of blocks.
    // Prevents infinite loop on malformed CFG where idom chain has a cycle.
    let max_iters = rpo.len() * 2 + 1;
    let mut iters = 0;
    while a != b {
        iters += 1;
        if iters > max_iters {
            break; // Malformed CFG — stop rather than loop forever
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
        // If neither a nor b changed, we're stuck — break to prevent infinite loop
        let a_rpo = rpo_of(a);
        let b_rpo = rpo_of(b);
        if a_rpo == b_rpo && a != b {
            break;
        }
    }
    a
}

/// Returns true if `a` dominates `b` in the dominator tree.
fn dominates(idom: &HashMap<BlockId, Option<BlockId>>, a: BlockId, b: BlockId) -> bool {
    if a == b {
        return true;
    }
    let mut cur = b;
    let mut seen: HashSet<BlockId> = HashSet::new();
    loop {
        if !seen.insert(cur) {
            // Cycle guard.
            return false;
        }
        match idom.get(&cur).and_then(|x| *x) {
            Some(parent) => {
                if parent == a {
                    return true;
                }
                cur = parent;
            }
            None => return false,
        }
    }
}

/// BFS from entry block, returning blocks in BFS (roughly RPO) order.
fn bfs_order(func: &TirFunction) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut queue: VecDeque<BlockId> = VecDeque::new();
    let mut order: Vec<BlockId> = Vec::new();

    queue.push_back(func.entry_block);
    visited.insert(func.entry_block);

    while let Some(bid) = queue.pop_front() {
        order.push(bid);
        if let Some(block) = func.blocks.get(&bid) {
            for succ in successors_of(block) {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }

    order
}

/// Return the successor block IDs of a block based on its terminator.
fn successors_of(block: &super::blocks::TirBlock) -> Vec<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut succs = vec![*default];
            for (_, target, _) in cases {
                succs.push(*target);
            }
            succs
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    /// Build a minimal valid function: add(i64, i64) -> i64.
    fn valid_add_function() -> TirFunction {
        let mut func =
            TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let result = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        func
    }

    #[test]
    fn valid_function_passes_verification() {
        let func = valid_add_function();
        assert!(
            verify_function(&func).is_ok(),
            "valid add function should pass: {:?}",
            verify_function(&func).err()
        );
    }

    #[test]
    fn missing_entry_block_fails() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        // Set entry_block to a non-existent block id.
        func.entry_block = BlockId(99);
        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("entry block")),
            "expected entry block error, got: {:?}",
            errors
        );
    }

    #[test]
    fn branch_to_nonexistent_block_fails() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        // Point the entry block terminator to a block that doesn't exist.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: BlockId(99),
            args: vec![],
        };
        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("does not exist")),
            "expected 'does not exist' error, got: {:?}",
            errors
        );
    }

    #[test]
    fn wrong_branch_arg_count_fails() {
        // Entry branches to bb1 but passes 1 arg; bb1 expects 0.
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        // Add a const so we have ValueId(0) defined.
        let v0 = func.fresh_value();
        let bb1 = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v0],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Branch {
            target: bb1,
            args: vec![v0], // passing 1 arg
        };

        // bb1 expects no args.
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![], // expects 0
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("expects")),
            "expected arg-count error, got: {:?}",
            errors
        );
    }

    #[test]
    fn duplicate_value_definition_fails() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let v0 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        // Define v0 twice.
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v0],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v0], // duplicate!
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("duplicate")),
            "expected duplicate error, got: {:?}",
            errors
        );
    }

    #[test]
    fn use_of_undefined_value_fails() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let undefined = ValueId(999);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Neg,
            operands: vec![undefined], // never defined
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let result = verify_function(&func);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("never defined")),
            "expected undefined value error, got: {:?}",
            errors
        );
    }

    #[test]
    fn valid_multi_block_function_passes() {
        // Build: func @branch(bool) -> i64
        //   ^bb0(%0: bool):
        //     cond_br %0, ^bb1, ^bb2
        //   ^bb1:
        //     %2 = const_int {value: 1}
        //     return %2
        //   ^bb2:
        //     %3 = const_int {value: 0}
        //     return %3
        let mut func = TirFunction::new("branch".into(), vec![TirType::Bool], TirType::I64);

        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();

        let v1 = func.fresh_value();
        let v2 = func.fresh_value();

        // Entry.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: bb1,
            then_args: vec![],
            else_block: bb2,
            else_args: vec![],
        };

        // bb1.
        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), crate::tir::ops::AttrValue::Int(1));
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v1],
                    attrs: attrs1,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v1] },
            },
        );

        // bb2.
        let mut attrs2 = AttrDict::new();
        attrs2.insert("value".into(), crate::tir::ops::AttrValue::Int(0));
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v2],
                    attrs: attrs2,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v2] },
            },
        );

        assert!(
            verify_function(&func).is_ok(),
            "multi-block branch function should pass: {:?}",
            verify_function(&func).err()
        );
    }
}
