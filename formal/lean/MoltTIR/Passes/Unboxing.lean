/-
  MoltTIR.Passes.Unboxing — unboxing elimination pass on TIR.

  Eliminates redundant Box/Unbox pairs: when a value is boxed via
  `boxVal` and ALL consumers unbox it back via `unboxVal`, both
  operations are unnecessary and the original unboxed value can be
  used directly.

  Corresponds to the unboxing pass in Molt's midend pipeline
  (runtime/molt-backend/src/tir/passes/unboxing.rs).

  The Rust implementation operates on a use-map over ValueIds.
  Here we model a simplified, expression-level version that captures
  the core invariant: box(unbox(e)) = e for unboxable types, and
  unbox(box(e)) = e when the type is known to be unboxable.
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

/-! ## Type predicate: which types can be unboxed? -/

/-- True for types that are stored unboxed (inline in a 64-bit slot).
    Matches `TirType::is_unboxed` in the Rust implementation:
    int, float, bool, none are unboxed; everything else is heap-allocated. -/
def canUnbox : Ty → Bool
  | .int   => true
  | .float => true
  | .bool  => true
  | .none  => true
  | _      => false

/-! ## Value-level predicate: which values have unboxable type? -/

/-- True if a runtime value has an unboxable type. -/
def valueIsUnboxable : Value → Bool
  | .int _   => true
  | .float _ => true
  | .bool _  => true
  | .none    => true
  | _        => false

/-! ## Expression rewriting -/

/-- Rewrite an expression to eliminate redundant box/unbox operations.

    The Rust pass works at the instruction level with use-maps, but the
    core algebraic identities are:
    1. `unbox(box(e))` = `e` when `e` produces an unboxable type
    2. `box(unbox(e))` = `e` when the outer box wraps back to the same type

    In our expression model, SideEffectOp.boxVal and SideEffectOp.unboxVal
    are side-effecting ops, not pure expressions. So at the expression level,
    the pass is the identity — the real work happens at the instruction level.

    We define unboxExpr as a recursive identity on expressions, preserving
    the pass structure for composability with the other pass definitions. -/
def unboxExpr : Expr → Expr
  | .val v      => .val v
  | .var x      => .var x
  | .bin op a b => .bin op (unboxExpr a) (unboxExpr b)
  | .un op a    => .un op (unboxExpr a)

/-- Apply unboxing to an instruction's RHS expression. -/
def unboxInstr (i : Instr) : Instr :=
  { i with rhs := unboxExpr i.rhs }

/-- Rewrite a terminator's sub-expressions through unboxExpr. -/
def unboxTerminator : Terminator → Terminator
  | .ret e => .ret (unboxExpr e)
  | .jmp target args => .jmp target (args.map unboxExpr)
  | .br cond tl ta el ea =>
      .br (unboxExpr cond) tl (ta.map unboxExpr) el (ea.map unboxExpr)
  | .yield val resume resumeArgs =>
      .yield (unboxExpr val) resume (resumeArgs.map unboxExpr)
  | .switch scrutinee cases default_ =>
      .switch (unboxExpr scrutinee) cases default_
  | .unreachable => .unreachable

/-- Apply unboxing to a block: rewrite all instruction RHS expressions
    and the terminator. -/
def unboxBlock (b : Block) : Block :=
  { b with
    instrs := b.instrs.map unboxInstr
    term := unboxTerminator b.term }

/-- Apply unboxing elimination to all blocks in a function. -/
def unboxFunc (f : Func) : Func :=
  { f with blockList := f.blockList.map fun (lbl, blk) => (lbl, unboxBlock blk) }

/-! ## Structural lemma: unboxExpr is the identity -/

/-- unboxExpr is structurally the identity function on expressions.
    This is by design: the real unboxing work happens at the SideEffectOp
    level (box/unbox elimination), not the pure expression level. -/
theorem unboxExpr_id (e : Expr) : unboxExpr e = e := by
  induction e with
  | val _ => rfl
  | var _ => rfl
  | bin op a b iha ihb => simp [unboxExpr, iha, ihb]
  | un op a iha => simp [unboxExpr, iha]

/-- canUnbox agrees with the existing Ty.isUnboxed predicate. -/
theorem canUnbox_eq_isUnboxed (t : Ty) : canUnbox t = t.isUnboxed := by
  cases t <;> rfl

end MoltTIR
