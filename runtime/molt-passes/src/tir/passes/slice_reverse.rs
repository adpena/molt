//! In-place reverse of a slice store whose value reverses the same range of
//! the same list (HF-837).
//!
//! ```text
//!   %s1 = SliceNew %start, %stop, -1        ; %start, %stop: None or ints
//!   %v  = Index %seq, %s1
//!   ...
//!   %s2 = SliceNew %lo, %hi, None
//!   StoreIndex %seq, %s2, %v
//! ```
//!
//! stores into `seq[lo:hi]` the items `seq[start]`, `seq[start - 1]`, ...,
//! `seq[stop + 1]`. When `hi` is proven `start + 1` (or both are None) and
//! `lo` is proven `stop + 1` (or both are None), and the two bounds of the
//! source slice are non-negative ints, CPython's slice normalization makes the
//! target range and the source range the same range `[lo, hi)` clamped to the
//! list, so the statement reverses that range in place. The proof is about SSA
//! values: each bound decomposes into a base value plus a constant offset
//! through copies, `Add` and `Sub` of int constants, so `seq[:k + 1] =
//! seq[k::-1]`, `n = k + 1; seq[:n] = seq[n - 1::-1]` and `seq[i:j] =
//! seq[j - 1:i - 1:-1]` all qualify, however they are spelled. The equation
//! is arithmetic only for an int base, so the kernel takes each bound as its
//! base and offset and checks the base at run time.
//!
//! The check after each raising op ends a block, so the statement is a
//! straight-line chain of blocks, each the only predecessor of the next. The
//! rewrite becomes
//!
//! ```text
//!   %done = ListReverseSlice %seq, %start_base, start_offset,
//!                            %stop_base, stop_offset, shape
//!   cond_br Bool(%done), ^join, ^slow
//! ^slow:  the original chain from %s1 through the StoreIndex ; br ^join
//! ^join:  the rest of the store's block
//! ```
//!
//! The kernel reverses in place only for an exact list without a C view and
//! bounds that are None or inline ints `>= 0`; otherwise it declines, having
//! done nothing, and the statement runs as written: a negative bound wraps
//! from the end, so the two ranges differ. The kernel runs no Python code and
//! reads only values the original ops read before the `Index`, so the order of
//! observable effects is unchanged. A shape that stores a reversed slice of
//! the same list but whose bounds the decomposition cannot prove equal is
//! reported under `MOLT_SLICE_REVERSE_REPORT=1`.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use super::value_identity::copy_value_source;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use molt_ir::OpKind;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

/// Where an op sits: its block and its index in the block.
type Site = (BlockId, usize);

/// Rewrite every proven reversed slice store of `func`. Requires: the module
/// phase is done and drops are not inserted.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "slice_reverse",
        ..Default::default()
    };
    let report = backend_env::var(BackendEnv::MOLT_SLICE_REVERSE_REPORT).as_deref() == Ok("1");
    // One rewrite at a time: each splits blocks, so sites are found afresh.
    // The slow path keeps each rewritten store, so its target slice value,
    // unique to the store, marks it done.
    let mut done: HashSet<ValueId> = HashSet::new();
    loop {
        let mut declined = Vec::new();
        let found = {
            let facts = Facts::new(func);
            let mut blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
            blocks.sort_unstable();
            blocks.into_iter().find_map(|block| {
                (0..func.blocks[&block].ops.len()).find_map(|index| {
                    let op = &func.blocks[&block].ops[index];
                    if op.opcode == OpCode::StoreIndex
                        && op.operands.get(1).is_some_and(|key| done.contains(key))
                    {
                        return None;
                    }
                    match facts.match_store(block, index) {
                        Match::Proven(rewrite) => Some(rewrite),
                        Match::Unproven(reason) => {
                            declined.push((block, index, reason));
                            None
                        }
                        Match::Other => None,
                    }
                })
            })
        };
        let Some(rewrite) = found else {
            // The last scan saw every remaining site.
            if report {
                for (block, index, reason) in declined {
                    eprintln!(
                        "[SLICE_REVERSE] fn={} block={} op={}: declined: {reason}",
                        func.name, block.0, index
                    );
                }
            }
            break;
        };
        let (tail, store) = rewrite.store;
        done.insert(func.blocks[&tail].ops[store].operands[1]);
        apply(func, rewrite);
        stats.values_changed += 1;
        stats.ops_added += 5;
    }
    stats
}

/// A proven site: the first and last op the slow path keeps, and the
/// kernel's operands.
struct Rewrite {
    first: Site,
    store: Site,
    seq: ValueId,
    start: Bound,
    stop: Bound,
    /// Bit 0: the source start is the constant None; bit 1: its stop is. A
    /// bound that is a value must be an int at run time, even if it is None.
    shape: i64,
}

/// A source slice bound as a base value plus a constant offset. The kernel
/// checks the base is an int and adds the offset, as the statement would.
#[derive(Clone, Copy)]
struct Bound {
    base: ValueId,
    offset: i64,
}

enum Match {
    Proven(Rewrite),
    /// The shape stores a reversed slice of the same list, but a proof is
    /// missing; the reason says which.
    Unproven(&'static str),
    Other,
}

/// Definitions and uses of `func`'s values.
struct Facts<'f> {
    func: &'f TirFunction,
    defs: HashMap<ValueId, Site>,
    /// Every site that uses a value; terminators use index `usize::MAX`.
    uses: HashMap<ValueId, Vec<Site>>,
    /// The number of CFG edges into each block.
    predecessors: HashMap<BlockId, usize>,
}

impl<'f> Facts<'f> {
    fn new(func: &'f TirFunction) -> Self {
        let mut defs = HashMap::new();
        let mut uses: HashMap<ValueId, Vec<Site>> = HashMap::new();
        let mut predecessors: HashMap<BlockId, usize> = HashMap::new();
        for (&block, body) in &func.blocks {
            for arg in &body.args {
                defs.insert(arg.id, (block, usize::MAX));
            }
            for (index, op) in body.ops.iter().enumerate() {
                for &result in &op.results {
                    defs.insert(result, (block, index));
                }
                for &operand in &op.operands {
                    uses.entry(operand).or_default().push((block, index));
                }
            }
            body.terminator
                .for_each_value(|value| uses.entry(value).or_default().push((block, usize::MAX)));
            body.terminator
                .for_each_edge(|target, _| *predecessors.entry(target).or_default() += 1);
        }
        Self {
            func,
            defs,
            uses,
            predecessors,
        }
    }

    fn op(&self, value: ValueId) -> Option<&'f TirOp> {
        let &(block, index) = self.defs.get(&value)?;
        self.func.blocks[&block].ops.get(index)
    }

    /// The value `value` copies, through every forwarding copy.
    fn canon(&self, mut value: ValueId) -> ValueId {
        for _ in 0..64 {
            match self.op(value).and_then(copy_value_source) {
                Some(source) => value = source,
                None => break,
            }
        }
        value
    }

    fn is_none(&self, value: ValueId) -> bool {
        self.op(self.canon(value))
            .is_some_and(|op| op.opcode == OpCode::ConstNone)
    }

    fn const_int(&self, value: ValueId) -> Option<i64> {
        let op = self.op(self.canon(value))?;
        if op.opcode == OpCode::ConstInt {
            return match op.attrs.get("value") {
                Some(AttrValue::Int(constant)) => Some(*constant),
                _ => None,
            };
        }
        if op.opcode == OpCode::Neg && op.operands.len() == 1 {
            return self.const_int(op.operands[0])?.checked_neg();
        }
        None
    }

    /// `value` as a base value plus a constant offset, through copies and
    /// `Add` and `Sub` of int constants. The equation holds for whatever the
    /// base is at run time only when the base is an int; callers check that.
    fn affine(&self, value: ValueId) -> (ValueId, i64) {
        let mut base = self.canon(value);
        let mut offset = 0i64;
        for _ in 0..64 {
            let Some(op) = self.op(base) else { break };
            let binary = op.operands.len() == 2;
            let adds = matches!(op.opcode, OpCode::Add | OpCode::InplaceAdd);
            let subtracts = matches!(op.opcode, OpCode::Sub | OpCode::InplaceSub);
            let step = if binary && adds {
                match (
                    self.const_int(op.operands[0]),
                    self.const_int(op.operands[1]),
                ) {
                    (_, Some(c)) => Some((op.operands[0], c)),
                    (Some(c), None) => Some((op.operands[1], c)),
                    (None, None) => None,
                }
            } else if binary && subtracts {
                self.const_int(op.operands[1])
                    .and_then(|c| c.checked_neg())
                    .map(|c| (op.operands[0], c))
            } else {
                None
            };
            let Some((next, c)) = step else { break };
            let Some(sum) = offset.checked_add(c) else {
                break;
            };
            offset = sum;
            base = self.canon(next);
        }
        (base, offset)
    }

    /// `target` is proven `source + 1`: both are one base plus constants, and
    /// the target's constant is one more. Both None also matches: the range
    /// then runs to that end. Returns the source bound as the kernel takes it.
    fn bound_follows(&self, source: ValueId, target: ValueId) -> Result<Bound, &'static str> {
        match (self.is_none(source), self.is_none(target)) {
            (true, true) => {
                return Ok(Bound {
                    base: source,
                    offset: 0,
                });
            }
            (true, false) | (false, true) => {
                return Err("one slice bound is None and its partner is not");
            }
            (false, false) => {}
        }
        let (source_base, source_offset) = self.affine(source);
        let (target_base, target_offset) = self.affine(target);
        if source_base != target_base || source_offset.checked_add(1) != Some(target_offset) {
            return Err("the bounds are not proven to name the same range");
        }
        // The equation is arithmetic only when the base is an int: the kernel
        // takes the base and checks it, so `n - 1` cannot be an int while
        // `n` is not.
        Ok(Bound {
            base: source_base,
            offset: source_offset,
        })
    }

    fn slice_operands(&self, value: ValueId) -> Option<[ValueId; 3]> {
        let op = self.op(self.canon(value))?;
        (op.opcode == OpCode::Copy && op.origin == Some(OpKind::SliceNew) && op.operands.len() == 3)
            .then(|| [op.operands[0], op.operands[1], op.operands[2]])
    }

    /// The straight-line chain from `from` to `to`: each block ends in an
    /// argument-free branch to the next, which has no other predecessor.
    /// Each check of a raising op ends a block, so one statement spans a chain.
    fn chain(&self, from: BlockId, to: BlockId) -> Option<Vec<BlockId>> {
        let mut chain = vec![from];
        let mut block = from;
        while block != to {
            let Terminator::Branch { target, args } = &self.func.blocks[&block].terminator else {
                return None;
            };
            if !args.is_empty() || self.predecessors.get(target).copied() != Some(1) {
                return None;
            }
            block = *target;
            if chain.contains(&block) || chain.len() > 64 {
                return None;
            }
            chain.push(block);
        }
        Some(chain)
    }

    fn match_store(&self, block: BlockId, index: usize) -> Match {
        let store = &self.func.blocks[&block].ops[index];
        if store.opcode != OpCode::StoreIndex || store.operands.len() != 3 {
            return Match::Other;
        }
        let [seq, key, value] = [store.operands[0], store.operands[1], store.operands[2]];
        let read_op = self.op(value).filter(|op| {
            op.opcode == OpCode::Index && op.operands.len() == 2 && op.results.len() == 1
        });
        let Some(read_op) = read_op else {
            return Match::Other;
        };
        if self.canon(read_op.operands[0]) != self.canon(seq) {
            return Match::Other;
        }
        let (Some([lo, hi, target_step]), Some([start, stop, source_step])) = (
            self.slice_operands(key),
            self.slice_operands(read_op.operands[1]),
        ) else {
            return Match::Other;
        };
        if self.const_int(source_step) != Some(-1) {
            return Match::Other;
        }
        if !(self.is_none(target_step) || self.const_int(target_step) == Some(1)) {
            return Match::Other;
        }
        // From here the statement stores a reversed slice of the same list.
        let (start_bound, stop_bound) =
            match (self.bound_follows(start, hi), self.bound_follows(stop, lo)) {
                (Ok(start), Ok(stop)) => (start, stop),
                (Err(reason), _) | (_, Err(reason)) => return Match::Unproven(reason),
            };
        // The slow path starts at the source slice when it is built for the
        // read alone, so the fast path builds no slice; else at the read.
        let source_slice = read_op.operands[1];
        let single_use = self
            .uses
            .get(&source_slice)
            .is_some_and(|uses| uses.len() == 1);
        let (Some(&read_site), Some(&slice_site)) =
            (self.defs.get(&value), self.defs.get(&source_slice))
        else {
            return Match::Other;
        };
        let first = if single_use && slice_site.1 != usize::MAX {
            slice_site
        } else {
            read_site
        };
        let Some(chain) = self.chain(first.0, block) else {
            return Match::Unproven("the statement is not one straight-line chain of blocks");
        };
        // The read lies in the chain, after the slow path's first op.
        let position = |site: Site| {
            chain
                .iter()
                .position(|&chain_block| chain_block == site.0)
                .map(|at| (at, site.1))
        };
        let (Some(first_at), Some(read_at), Some(store_at)) = (
            position(first),
            position(read_site),
            position((block, index)),
        ) else {
            return Match::Unproven("the read is outside the statement's chain");
        };
        if !(first_at <= read_at && read_at <= store_at) {
            return Match::Unproven("the read does not precede the store");
        }
        // Structured loop boundary blocks keep their shape for SimpleIR.
        if chain.iter().any(|chain_block| {
            self.func
                .loop_roles
                .get(chain_block)
                .is_some_and(|role| *role != crate::tir::blocks::LoopRole::None)
        }) {
            return Match::Unproven("the statement crosses a loop boundary block");
        }
        // The kernel's operands are defined before the slow path starts.
        let kernel = [self.canon(seq), start_bound.base, stop_bound.base];
        for operand in kernel {
            // A block argument is defined on entry, before every op.
            let def = self.defs.get(&operand).copied();
            match def.filter(|site| site.1 != usize::MAX).and_then(position) {
                Some(at) if at >= first_at => {
                    return Match::Unproven("a kernel operand is defined inside the statement");
                }
                _ if !self.defs.contains_key(&operand) => {
                    return Match::Unproven("a kernel operand has no definition");
                }
                _ => {}
            }
        }
        // Every value the slow path defines stays inside it.
        let inside = |site: Site| position(site).is_some_and(|at| at >= first_at && at <= store_at);
        for &chain_block in &chain {
            for (op_index, op) in self.func.blocks[&chain_block].ops.iter().enumerate() {
                if !inside((chain_block, op_index)) {
                    continue;
                }
                let escapes = op.results.iter().any(|result| {
                    self.uses
                        .get(result)
                        .is_some_and(|uses| uses.iter().any(|&site| !inside(site)))
                });
                if escapes {
                    return Match::Unproven("a value the statement defines is used after it");
                }
            }
        }
        let shape = i64::from(self.is_none(start)) | (i64::from(self.is_none(stop)) << 1);
        Match::Proven(Rewrite {
            first,
            store: (block, index),
            seq: kernel[0],
            start: start_bound,
            stop: stop_bound,
            shape,
        })
    }
}

/// Split the store's block after the store into a join, then the first
/// block at the slow path's first op: the kernel and its test end it, the
/// slow path holds the original ops through the store, and the join holds
/// the rest.
fn apply(func: &mut TirFunction, rewrite: Rewrite) {
    let Rewrite {
        first: (head, first),
        store: (tail, store),
        seq,
        start,
        stop,
        shape,
    } = rewrite;
    let span = func.blocks[&tail].ops[store].source_span;

    let join = func.fresh_block();
    let tail_block = func.blocks.get_mut(&tail).expect("store block exists");
    let rest = tail_block.ops.split_off(store + 1);
    let terminator = std::mem::replace(
        &mut tail_block.terminator,
        Terminator::Branch {
            target: join,
            args: Vec::new(),
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: Vec::<TirValue>::new(),
            ops: rest,
            terminator,
        },
    );
    // A loop whose break test ended the store's block finds it in the join.
    for cond_block in func.loop_cond_blocks.values_mut() {
        if *cond_block == tail {
            *cond_block = join;
        }
    }

    let mut constant = |value: i64| {
        let result = func.fresh_value();
        func.value_types.insert(result, TirType::I64);
        let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![result]);
        op.attrs.insert("value".into(), AttrValue::Int(value));
        op.source_span = span;
        (op, result)
    };
    let (start_offset_op, start_offset) = constant(start.offset);
    let (stop_offset_op, stop_offset) = constant(stop.offset);
    let (shape_op, shape_value) = constant(shape);
    let done = func.fresh_value();
    func.value_types.insert(done, TirType::DynBox);
    let truth = func.fresh_value();
    func.value_types.insert(truth, TirType::Bool);
    let mut kernel = TirOp::new(
        OpCode::Copy,
        vec![
            seq,
            start.base,
            start_offset,
            stop.base,
            stop_offset,
            shape_value,
        ],
        vec![done],
    );
    kernel.origin = Some(OpKind::ListReverseSlice);
    kernel.source_span = span;
    let mut test = TirOp::new(OpCode::Bool, vec![done], vec![truth]);
    test.source_span = span;

    let slow = func.fresh_block();
    let head_block = func.blocks.get_mut(&head).expect("first block exists");
    let slow_ops = head_block.ops.split_off(first);
    let slow_terminator = std::mem::replace(
        &mut head_block.terminator,
        Terminator::CondBranch {
            cond: truth,
            then_block: join,
            then_args: Vec::new(),
            else_block: slow,
            else_args: Vec::new(),
        },
    );
    head_block
        .ops
        .extend([start_offset_op, stop_offset_op, shape_op, kernel, test]);
    func.blocks.insert(
        slow,
        TirBlock {
            id: slow,
            args: Vec::new(),
            ops: slow_ops,
            terminator: slow_terminator,
        },
    );
}

#[cfg(test)]
#[path = "slice_reverse_tests.rs"]
mod tests;
