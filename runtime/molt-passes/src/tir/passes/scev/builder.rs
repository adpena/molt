use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::ScevExpr;
use crate::tir::op_kinds_generated::{ScevExprRule, opcode_scev_expr_rule_table};
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::index::{DefIndex, LoopContext, collect_header_incoming};

/// SCEV builder. Owns the def index + loop context and memoizes results to
/// terminate on the recurrence cycle (a header arg's SCEV is computed while
/// computing the back-edge value's SCEV).
pub(super) struct ScevBuilder<'a> {
    func: &'a TirFunction,
    pub(super) loops: &'a LoopContext,
    defs: &'a DefIndex,
    /// header → its IV header-arg value (the recurrence "phi"), once recognized.
    pub(super) iv_of_header: HashMap<BlockId, ValueId>,
    /// Memoized SCEV per value.
    memo: HashMap<ValueId, ScevExpr>,
    /// Values currently on the recursion stack (cycle guard).
    in_progress: HashSet<ValueId>,
}

impl<'a> ScevBuilder<'a> {
    pub(super) fn new(func: &'a TirFunction, loops: &'a LoopContext, defs: &'a DefIndex) -> Self {
        Self {
            func,
            loops,
            defs,
            iv_of_header: HashMap::new(),
            memo: HashMap::new(),
            in_progress: HashSet::new(),
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

    /// Compute the SCEV expression for value `v`.
    pub(super) fn scev(&mut self, v: ValueId) -> ScevExpr {
        if let Some(e) = self.memo.get(&v) {
            return e.clone();
        }
        if self.in_progress.contains(&v) {
            // On the recurrence cycle without a resolved recurrence yet —
            // conservative.
            return ScevExpr::Unknown;
        }
        self.in_progress.insert(v);
        let result = self.compute_scev_of(v);
        self.in_progress.remove(&v);
        self.memo.insert(v, result.clone());
        result
    }

    fn compute_scev_of(&mut self, v: ValueId) -> ScevExpr {
        if let Some(&c) = self.defs.const_int.get(&v) {
            return ScevExpr::Constant(c);
        }

        // A header block-arg may be an induction-variable phi.
        if let Some(&header) = self.header_of_arg(v) {
            return self.scev_of_header_arg(v, header);
        }

        // Op-defined value: recognize affine combinations of invariants.
        if let Some((opcode, operands, _nsw)) = self.defs.def_op.get(&v).cloned() {
            return self.scev_of_op(v, opcode, &operands);
        }

        // Parameter or otherwise opaque definition → an invariant symbol.
        ScevExpr::Invariant(v)
    }

    /// If `v` is a header block-argument, return that header.
    fn header_of_arg(&self, v: ValueId) -> Option<&BlockId> {
        // header_args is small (one entry per loop header); linear scan is fine.
        self.defs
            .header_args
            .iter()
            .find_map(|(h, args)| if args.contains(&v) { Some(h) } else { None })
    }

    /// Recognize a header block-argument as an induction-variable recurrence,
    /// or classify it as invariant / unknown.
    fn scev_of_header_arg(&mut self, iv: ValueId, header: BlockId) -> ScevExpr {
        if let Some(&known) = self.iv_of_header.get(&header)
            && known == iv
        {
            // Already recognized as this loop's IV (cycle re-entry): return the
            // recurrence shape placeholder. We model the start/step lazily, so
            // re-entry just yields Unknown for the nested computation; the outer
            // call assembles the AddRec. To avoid a partial AddRec here, signal
            // self-reference as Unknown (the caller building the AddRec uses the
            // start/step directly, not this recursive value).
            return ScevExpr::Unknown;
        }

        let body = match self.loops.bodies.get(&header) {
            Some(b) => b.clone(),
            None => return ScevExpr::Invariant(iv),
        };
        let incoming = collect_header_incoming(self.func, header, &body);

        // Partition into entry (non-back-edge) and back-edge args for this iv.
        let arg_index = match self
            .defs
            .header_args
            .get(&header)
            .and_then(|args| args.iter().position(|&a| a == iv))
        {
            Some(i) => i,
            None => return ScevExpr::Invariant(iv),
        };

        let mut start_vals: Vec<ValueId> = Vec::new();
        let mut next_vals: Vec<ValueId> = Vec::new();
        for (_pred, args, is_back) in &incoming.edges {
            let Some(&val) = args.get(arg_index) else {
                // A predecessor that does not pass this arg → malformed for our
                // purposes; refuse to model.
                return ScevExpr::Unknown;
            };
            // Resolve through plain copies so the back-edge/start values name
            // their canonical sources (lowering wraps both in copies).
            let val = self.defs.resolve(val);
            if *is_back {
                next_vals.push(val);
            } else {
                start_vals.push(val);
            }
        }

        // Exactly one distinct start and one distinct back-edge value.
        if start_vals.is_empty() || next_vals.is_empty() {
            return ScevExpr::Invariant(iv);
        }
        let start_val = start_vals[0];
        if start_vals.iter().any(|&s| s != start_val) {
            return ScevExpr::Unknown;
        }
        let next_val = next_vals[0];
        if next_vals.iter().any(|&n| n != next_val) {
            return ScevExpr::Unknown;
        }

        // The back-edge value must be `Add(iv, step)` with `no_signed_wrap`,
        // and `step` loop-invariant. (Subtraction is normalized to Add of a
        // negative const upstream; we additionally accept Add(step, iv).)
        let (opcode, operands, nsw) = match self.defs.def_op.get(&next_val).cloned() {
            Some(t) => t,
            None => return ScevExpr::Invariant(iv),
        };
        if opcode != OpCode::Add || operands.len() != 2 {
            return ScevExpr::Unknown;
        }
        // Soundness rule 1: no AddRec without a non-wrap proof.
        if !nsw {
            return ScevExpr::Unknown;
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
            return ScevExpr::Unknown;
        };
        // Soundness rule 2: step must be loop-invariant.
        if !self.is_invariant_in(step_val, header) {
            return ScevExpr::Unknown;
        }

        // Mark this header's IV so the recursive `scev(start_val)` /
        // `scev(step_val)` calls (which can transitively reference the header
        // through invariants) terminate cleanly.
        self.iv_of_header.insert(header, iv);

        let start_scev = self.scev_invariant_expr(start_val, header);
        let step_scev = self.scev_invariant_expr(step_val, header);

        // Soundness rule 3: degree-2 recurrence. If the step is itself a
        // recurrence (an AddRec), the closed form is quadratic — refuse.
        if matches!(step_scev, ScevExpr::AddRec { .. }) {
            return ScevExpr::Unknown;
        }
        // The start, if a recurrence of an *outer* loop, is fine (a nested IV);
        // but a start that is an AddRec of THIS loop is impossible (it is
        // defined outside). So only the step's degree gates here.

        ScevExpr::AddRec {
            start: Box::new(start_scev),
            step: Box::new(step_scev),
            loop_header: header,
        }
    }

    /// Compute the SCEV of a value known to be loop-invariant w.r.t. `header`
    /// (start or step of an AddRec). Constants stay constants; everything else
    /// is an `Invariant` symbol unless it is itself an outer-loop recurrence.
    fn scev_invariant_expr(&mut self, v: ValueId, _header: BlockId) -> ScevExpr {
        if let Some(&c) = self.defs.const_int.get(&v) {
            return ScevExpr::Constant(c);
        }
        // It may be an induction variable of an enclosing loop.
        if let Some(&outer_header) = self.header_of_arg(v) {
            let e = self.scev_of_header_arg(v, outer_header);
            // Avoid returning a within-progress Unknown as Invariant noise.
            if !matches!(e, ScevExpr::Unknown) {
                return e;
            }
        }
        ScevExpr::Invariant(v)
    }

    /// SCEV of an op-defined value (affine combinations of invariants only).
    fn scev_of_op(&mut self, _v: ValueId, opcode: OpCode, operands: &[ValueId]) -> ScevExpr {
        match opcode_scev_expr_rule_table(opcode) {
            ScevExprRule::Add if operands.len() == 2 => {
                let a = self.scev(operands[0]);
                let b = self.scev(operands[1]);
                fold_add(a, b)
            }
            ScevExprRule::Sub if operands.len() == 2 => {
                let a = self.scev(operands[0]);
                let b = self.scev(operands[1]);
                fold_sub(a, b)
            }
            ScevExprRule::Mul if operands.len() == 2 => {
                let a = self.scev(operands[0]);
                let b = self.scev(operands[1]);
                fold_mul(a, b)
            }
            _ => ScevExpr::Unknown,
        }
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
