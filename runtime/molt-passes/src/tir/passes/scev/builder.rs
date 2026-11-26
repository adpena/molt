use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::ScevExpr;
use crate::tir::op_kinds_generated::{ScevExprRule, opcode_scev_expr_rule_table};
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::index::{DefIndex, LoopContext};

/// SCEV builder. Owns the def index + loop context and memoizes results to
/// terminate on the recurrence cycle (a header arg's SCEV is computed while
/// computing the back-edge value's SCEV).
pub(super) struct ScevBuilder<'a> {
    pub(super) loops: &'a LoopContext,
    pub(super) guards: super::super::counted_loop::LoopGuardContext,
    defs: &'a DefIndex<'a>,
    /// header → its IV header-arg value (the recurrence "phi"), once recognized.
    pub(super) iv_of_header: HashMap<BlockId, ValueId>,
    /// Memoized SCEV per value.
    memo: HashMap<ValueId, ScevExpr>,
    /// Values currently on the evaluation stack (cycle guard).
    in_progress: HashSet<ValueId>,
}

impl<'a> ScevBuilder<'a> {
    pub(super) fn new(
        func: &'a TirFunction,
        loops: &'a LoopContext,
        defs: &'a DefIndex<'a>,
    ) -> Self {
        Self {
            loops,
            guards: super::super::counted_loop::LoopGuardContext::new(func),
            defs,
            iv_of_header: HashMap::default(),
            memo: HashMap::default(),
            in_progress: HashSet::default(),
        }
    }

    /// True if `v` is defined outside the loop headed by `header` (or is a
    /// constant), i.e. loop-invariant with respect to that loop.
    fn is_invariant_in(&self, v: ValueId, header: BlockId) -> bool {
        if self.defs.const_int.contains_key(&v) {
            return true;
        }
        match self.defs.def_block.get(&v) {
            Some(&db) => !self
                .loops
                .bodies
                .get(&header)
                .map(|body| body.contains(&db))
                .unwrap_or(false),
            // No known def site (parameter handled above via def_block) →
            // treat as defined at entry, hence invariant.
            None => true,
        }
    }

    /// Evaluate operands in depth-first, left-to-right order. Continuations and
    /// intermediate answers live on explicit stacks, including nested header
    /// starts/steps; memoization and cycle answers keep their original timing.
    pub(super) fn scev(&mut self, v: ValueId) -> ScevExpr {
        if let Some(expr) = self.memo.get(&v) {
            return expr.clone();
        }
        enum Work {
            Value(ValueId),
            Memoize(ValueId),
            Header(ValueId, BlockId, usize),
            Invariant(ValueId),
            InvariantOr(ValueId),
            Binary(ScevExprRule),
            Recurrence(BlockId),
        }
        let mut work = vec![Work::Value(v)];
        let mut answers = Vec::new();
        while let Some(task) = work.pop() {
            match task {
                Work::Value(value) => {
                    if let Some(expr) = self.memo.get(&value) {
                        answers.push(expr.clone());
                        continue;
                    }
                    if !self.in_progress.insert(value) {
                        answers.push(ScevExpr::Unknown);
                        continue;
                    }
                    work.push(Work::Memoize(value));
                    let source = self.defs.resolve(value);
                    if source != value {
                        work.push(Work::Value(source));
                    } else if let Some(&constant) = self.defs.const_int.get(&value) {
                        answers.push(ScevExpr::Constant(constant));
                    } else if let Some(&(header, index)) = self.defs.header_arg_sites.get(&value) {
                        work.push(Work::Header(value, header, index));
                    } else if let Some((opcode, operands, _)) = self.defs.def_op.get(&value) {
                        let rule = opcode_scev_expr_rule_table(*opcode);
                        if operands.len() == 2
                            && matches!(
                                rule,
                                ScevExprRule::Add | ScevExprRule::Sub | ScevExprRule::Mul
                            )
                        {
                            work.push(Work::Binary(rule));
                            work.push(Work::Value(operands[1]));
                            work.push(Work::Value(operands[0]));
                        } else {
                            answers.push(ScevExpr::Unknown);
                        }
                    } else {
                        answers.push(ScevExpr::Invariant(value));
                    }
                }
                Work::Memoize(value) => {
                    let result = answers.last().expect("value evaluation produces an answer");
                    self.in_progress.remove(&value);
                    self.memo.insert(value, result.clone());
                }
                Work::Header(iv, header, index) => {
                    match self.header_recurrence(iv, header, index) {
                        Ok((start, step)) => {
                            work.push(Work::Recurrence(header));
                            work.push(Work::Invariant(step));
                            work.push(Work::Invariant(start));
                        }
                        Err(result) => answers.push(result),
                    }
                }
                Work::Invariant(value) => {
                    if let Some(&constant) = self.defs.const_int.get(&value) {
                        answers.push(ScevExpr::Constant(constant));
                    } else if let Some(&(header, index)) = self.defs.header_arg_sites.get(&value) {
                        // This is a direct header query, not a memoized value
                        // query: preserve the recurrence recognizer's guard.
                        work.push(Work::InvariantOr(value));
                        work.push(Work::Header(value, header, index));
                    } else {
                        answers.push(ScevExpr::Invariant(value));
                    }
                }
                Work::InvariantOr(value) => {
                    let result = answers
                        .last_mut()
                        .expect("header evaluation produces an answer");
                    if matches!(result, ScevExpr::Unknown) {
                        *result = ScevExpr::Invariant(value);
                    }
                }
                Work::Binary(rule) => {
                    let b = answers.pop().expect("right operand");
                    let a = answers.pop().expect("left operand");
                    answers.push(match rule {
                        ScevExprRule::Add => fold_add(a, b),
                        ScevExprRule::Sub => fold_sub(a, b),
                        ScevExprRule::Mul => fold_mul(a, b),
                        ScevExprRule::None => {
                            unreachable!("only affine binary operations are scheduled")
                        }
                    });
                }
                Work::Recurrence(header) => {
                    let step = answers.pop().expect("recurrence step");
                    let start = answers.pop().expect("recurrence start");
                    answers.push(if matches!(step, ScevExpr::AddRec { .. }) {
                        ScevExpr::Unknown
                    } else {
                        ScevExpr::AddRec {
                            start: Box::new(start),
                            step: Box::new(step),
                            loop_header: header,
                        }
                    });
                }
            }
        }
        debug_assert_eq!(answers.len(), 1);
        answers.pop().expect("SCEV evaluation produces one answer")
    }

    /// Recognize a header block-argument as an induction-variable recurrence,
    /// or classify it as invariant / unknown.
    fn header_recurrence(
        &mut self,
        iv: ValueId,
        header: BlockId,
        arg_index: usize,
    ) -> Result<(ValueId, ValueId), ScevExpr> {
        if let Some(&known) = self.iv_of_header.get(&header)
            && known == iv
        {
            // Already recognized as this loop's IV (cycle re-entry): return the
            // recurrence shape placeholder. We model the start/step lazily, so
            // re-entry just yields Unknown for the nested computation; the outer
            // call assembles the AddRec. To avoid a partial AddRec here, signal
            // self-reference as Unknown (the caller building the AddRec uses the
            // start/step directly, not this recursive value).
            return Err(ScevExpr::Unknown);
        }

        let (Some(body), Some(edges)) = (
            self.loops.bodies.get(&header),
            self.defs.header_edges.get(&header),
        ) else {
            return Err(ScevExpr::Invariant(iv));
        };

        let mut start = None;
        let mut next = None;
        let mut agrees = true;
        for &(pred, args) in edges {
            let Some(&val) = args.get(arg_index) else {
                return Err(ScevExpr::Unknown);
            };
            let val = self.defs.resolve(val);
            let slot = if body.contains(&pred) {
                &mut next
            } else {
                &mut start
            };
            agrees &= slot.replace(val).is_none_or(|prior| prior == val);
        }
        let (Some(start_val), Some(next_val)) = (start, next) else {
            return Err(ScevExpr::Invariant(iv));
        };
        if !agrees {
            return Err(ScevExpr::Unknown);
        }

        // The back-edge value must be `Add(iv, step)` with `no_signed_wrap`,
        // and `step` loop-invariant. (Subtraction is normalized to Add of a
        // negative const upstream; we additionally accept Add(step, iv).)
        let (opcode, operands, nsw) = match self.defs.def_op.get(&next_val).copied() {
            Some(t) => t,
            None => return Err(ScevExpr::Invariant(iv)),
        };
        if opcode != OpCode::Add || operands.len() != 2 {
            return Err(ScevExpr::Unknown);
        }
        // Soundness rule 1: no AddRec without a non-wrap proof.
        if !nsw {
            return Err(ScevExpr::Unknown);
        }
        // Resolve operands through copies; the Add increments the IV phi via a
        // copy of it (`Add(Copy(iv), step)`).
        let (a, b) = (
            self.defs.resolve(operands[0]),
            self.defs.resolve(operands[1]),
        );
        let iv_resolved = self.defs.resolve(iv);
        let step_val = if a == iv_resolved {
            b
        } else if b == iv_resolved {
            a
        } else {
            // Not a self-increment of this iv.
            return Err(ScevExpr::Unknown);
        };
        // Soundness rule 2: step must be loop-invariant.
        if !self.is_invariant_in(step_val, header) {
            return Err(ScevExpr::Unknown);
        }

        // Mark before evaluating either dependency so nested header queries
        // observe exactly the same recurrence-cycle guard as a recursive walk.
        self.iv_of_header.insert(header, iv);
        Ok((start_val, step_val))
    }
}

/// Constant-fold / build an `Add` SCEV.
fn fold_add(a: ScevExpr, b: ScevExpr) -> ScevExpr {
    match (&a, &b) {
        (ScevExpr::Constant(x), ScevExpr::Constant(y)) => match x.checked_add(*y) {
            Some(s) => ScevExpr::Constant(s),
            None => ScevExpr::Unknown,
        },
        (ScevExpr::Unknown, _) | (_, ScevExpr::Unknown) => ScevExpr::Unknown,
        _ => ScevExpr::Add(Box::new(a), Box::new(b)),
    }
}

/// Constant-fold / build a `Sub` SCEV (represented as `Add(a, -b)` / direct
/// constant fold; non-constant subtraction is `Unknown` since the lattice has
/// no `Sub` node and `Add` of a negated symbol is not expressible safely).
fn fold_sub(a: ScevExpr, b: ScevExpr) -> ScevExpr {
    match (&a, &b) {
        (ScevExpr::Constant(x), ScevExpr::Constant(y)) => match x.checked_sub(*y) {
            Some(s) => ScevExpr::Constant(s),
            None => ScevExpr::Unknown,
        },
        _ => ScevExpr::Unknown,
    }
}

/// Constant-fold / build a `Mul` SCEV.
fn fold_mul(a: ScevExpr, b: ScevExpr) -> ScevExpr {
    match (&a, &b) {
        (ScevExpr::Constant(x), ScevExpr::Constant(y)) => match x.checked_mul(*y) {
            Some(s) => ScevExpr::Constant(s),
            None => ScevExpr::Unknown,
        },
        (ScevExpr::Unknown, _) | (_, ScevExpr::Unknown) => ScevExpr::Unknown,
        _ => ScevExpr::Mul(Box::new(a), Box::new(b)),
    }
}
