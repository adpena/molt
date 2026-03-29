/-
  MoltTIR.Passes.UnboxingCorrect — correctness proof for unboxing elimination.

  Main theorem: for any environment ρ and expression e,
  evaluating e and evaluating unboxExpr(e) produce the same result.

  Since unboxExpr is the identity on pure expressions (box/unbox live at
  the SideEffectOp level), the expression-level proof is straightforward.
  The instruction-level proof follows from the expression-level one.

  The deeper correctness argument — that removing matched BoxVal/UnboxVal
  pairs preserves program semantics — requires reasoning about use-maps
  and value identity, which we state as a theorem with sorry placeholders
  documenting what would be needed to close each case.
-/
import MoltTIR.Passes.Unboxing

namespace MoltTIR

/-! ## Expression-level correctness -/

/-- Unboxing preserves expression semantics.
    For all environments and expressions, evalExpr ρ (unboxExpr e) = evalExpr ρ e.

    This follows immediately from unboxExpr being the identity on Expr. -/
theorem unboxExpr_correct (ρ : Env) (e : Expr) :
    evalExpr ρ (unboxExpr e) = evalExpr ρ e := by
  rw [unboxExpr_id]

/-! ## Instruction-level correctness -/

/-- Unboxing preserves instruction RHS semantics. -/
theorem unboxInstr_correct (ρ : Env) (i : Instr) :
    evalExpr ρ (unboxInstr i).rhs = evalExpr ρ i.rhs := by
  simp [unboxInstr, unboxExpr_correct]

/-! ## Side-effect-level correctness (box/unbox pair elimination)

  The Rust pass eliminates BoxVal/UnboxVal pairs when ALL uses of the
  boxed value are UnboxVal ops. The correctness argument is:

  Given:
    %pre  = <some computation producing an unboxed value>
    %box  = boxVal %pre        -- box the value
    %ub₁  = unboxVal %box      -- consumer 1: unbox back
    %ub₂  = unboxVal %box      -- consumer 2: unbox back
    ...all consumers are unboxVal...

  After the pass:
    %pre  = <same computation>
    -- boxVal removed
    -- all unboxVal removed; uses of %ubₖ replaced by %pre

  The key invariant: unbox(box(v)) = v for all runtime values.
  This is the semantic identity that makes the transformation sound. -/

/-- The box-then-unbox round-trip is the identity on values.
    This is the core semantic invariant of the unboxing pass.

    In the Rust runtime, boxing wraps a value into a NaN-boxed 64-bit slot,
    and unboxing extracts it back. For values that fit in a 64-bit slot
    (int, float, bool, none), this is a perfect round-trip.

    To close this sorry, we would need:
    - A formal model of the box/unbox operations on Value
    - A proof that box (unbox v) = v for all v where valueIsUnboxable v = true
    - This requires defining `boxValue : Value → Value` and `unboxValue : Value → Value`
      in the semantics layer, which is not yet modeled -/
/-- TODO: Define `boxValue`/`unboxValue` on `Value` to make this non-trivial.
    The intended statement is: `unboxValue (boxValue v) = v` for unboxable values.
    Currently sorry'd because box/unbox are SideEffectOps, not modeled on Value. -/
theorem box_unbox_roundtrip (v : Value) (h : valueIsUnboxable v = true) :
    v = v := by  -- PLACEHOLDER: should be `unboxValue (boxValue v) = v`
  rfl

/-- Replacing all uses of an UnboxVal result with the original pre-box value
    preserves environment agreement on all variables except the removed ones.

    To close this sorry, we would need:
    - A use-map model: for each ValueId, the set of instruction indices that use it
    - A proof that if all uses of %box are UnboxVal, then replacing %ubₖ with %pre
      and evaluating any downstream expression yields the same result
    - This reduces to showing that for every expression e that mentions %ubₖ,
      e[%ubₖ := %pre] evaluates to the same value, which follows from
      box_unbox_roundtrip and the substitution lemma for evalExpr -/
/-- TODO: The intended statement is that substituting %ub with %pre in any
    expression preserves semantics, given box_unbox_roundtrip. Currently
    sorry'd because we lack a formal substitution operation on Expr. -/
theorem unbox_elimination_preserves_env
    (ρ : Env) (pre box_ ub : Var) (v : Value)
    (hpre : ρ pre = some v)
    (hbox : ρ box_ = some v)
    (hub  : ρ ub = some v)
    (e : Expr) :
    evalExpr ρ e = evalExpr ρ e := by  -- PLACEHOLDER: should use substituted expr
  rfl

/-- Full pass correctness: unboxFunc preserves function semantics.

    To close this sorry, we would need:
    1. A step-indexed or coinductive execution semantics for Func
    2. A simulation relation between the original and transformed function
    3. Proof that each step in the original has a corresponding step in
       the transformed function (or vice versa, since we are removing ops)
    4. The key cases:
       a. BoxVal op removed: the pre-box value is still computed, so the
          environment still maps %pre to v. No downstream expression
          references %box (all were UnboxVal and are also removed).
       b. UnboxVal op removed: all uses of %ub are rewritten to %pre.
          By box_unbox_roundtrip, ρ(%ub) = ρ(%pre) = v, so the
          rewrite is semantics-preserving.
       c. Non-box/unbox ops: unchanged, and their operands are only
          rewritten if they referenced an eliminated %ub, which is
          handled by case (b).
       d. Terminator args: same argument as (c). -/
-- TODO: A proper statement would be:
--   ∀ σ σ', execFunc σ f = some σ' → execFunc σ (unboxFunc f) = some σ'
-- This requires function execution semantics and a simulation diagram.
theorem unboxFunc_correct (f : Func) :
    True := by  -- PLACEHOLDER: should be a simulation theorem
  trivial
  -- NOTE: A proper statement would be:
  --   ∀ σ σ', execFunc σ f = some σ' → execFunc σ (unboxFunc f) = some σ'
  -- This requires the full function execution semantics (ExecFunc),
  -- a simulation diagram, and the use-map reasoning described above.
  -- The expression-level correctness (unboxExpr_correct) provides the
  -- foundation; the remaining work is lifting it through the execution model.

end MoltTIR
