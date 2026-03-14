/-
  MoltTIR.Semantics.EvalExpr — expression evaluation (pure, deterministic).

  Covers Molt's core arithmetic, comparison, and bitwise opcodes.
  Additional opcodes (string ops, collection ops, etc.) are modeled
  as opaque intrinsics that don't need expression-level semantics.
-/
import MoltTIR.Semantics.State

namespace MoltTIR

/-- Evaluate a binary operator on two values. Returns none on type mismatch. -/
def evalBinOp (op : BinOp) (a b : Value) : Option Value :=
  match op, a, b with
  -- arithmetic (int × int → int)
  | .add, .int x, .int y => some (.int (x + y))
  | .sub, .int x, .int y => some (.int (x - y))
  | .mul, .int x, .int y => some (.int (x * y))
  | .mod, .int x, .int y => if y == 0 then none else some (.int (x % y))
  | .floordiv, .int x, .int y => if y == 0 then none else some (.int (x / y))
  | .pow, .int x, .int y =>
      if y < 0 then none
      else some (.int (x ^ y.toNat))
  -- string concatenation
  | .add, .str x, .str y => some (.str (x ++ y))
  -- string repetition (str * int)
  | .mul, .str s, .int n =>
      if n ≤ 0 then some (.str "")
      else some (.str (String.join (List.replicate n.toNat s)))
  | .mul, .int n, .str s =>
      if n ≤ 0 then some (.str "")
      else some (.str (String.join (List.replicate n.toNat s)))
  -- int * float promotion
  | .add, .int x, .float y => some (.float (x + y))
  | .sub, .int x, .float y => some (.float (x - y))
  | .mul, .int x, .float y => some (.float (x * y))
  | .add, .float x, .int y => some (.float (x + y))
  | .sub, .float x, .int y => some (.float (x - y))
  | .mul, .float x, .int y => some (.float (x * y))
  -- float * float arithmetic
  | .add, .float x, .float y => some (.float (x + y))
  | .sub, .float x, .float y => some (.float (x - y))
  | .mul, .float x, .float y => some (.float (x * y))
  -- comparison (int × int → bool)
  | .eq,  .int x, .int y => some (.bool (x == y))
  | .ne,  .int x, .int y => some (.bool (x != y))
  | .lt,  .int x, .int y => some (.bool (x < y))
  | .le,  .int x, .int y => some (.bool (x ≤ y))
  | .gt,  .int x, .int y => some (.bool (x > y))
  | .ge,  .int x, .int y => some (.bool (x ≥ y))
  -- comparison (bool × bool → bool)
  | .eq,  .bool x, .bool y => some (.bool (x == y))
  | .ne,  .bool x, .bool y => some (.bool (x != y))
  -- bitwise ops (bit_and, bit_or, bit_xor, lshift, rshift) are defined in
  -- the syntax but not evaluated here — they fall to the catch-all.
  -- Lean's Int lacks HAnd/HOr/HXor; add implementations when needed.
  -- catch-all for type mismatches, unmodeled ops, and bitwise
  | _, _, _ => none

/-- Evaluate a unary operator. -/
def evalUnOp (op : UnOp) (a : Value) : Option Value :=
  match op, a with
  | .neg, .int x => some (.int (-x))
  | .not, .bool x => some (.bool (!x))
  | .abs, .int x => some (.int (if x < 0 then -x else x))
  | _, _ => none

/-- Evaluate an expression in an environment. Total, deterministic. -/
def evalExpr (ρ : Env) : Expr → Option Value
  | .val v => some v
  | .var x => ρ x
  | .bin op a b =>
      match evalExpr ρ a, evalExpr ρ b with
      | some va, some vb => evalBinOp op va vb
      | _, _ => none
  | .un op a =>
      match evalExpr ρ a with
      | some va => evalUnOp op va
      | none => none

/-- Evaluating an expression in an environment extended with an irrelevant
    binding produces the same result. This is the key lemma for DCE correctness:
    if x does not appear in e, then setting x in ρ does not affect evalExpr ρ e. -/
theorem evalExpr_set_irrelevant (ρ : Env) (x : Var) (v : Value) (e : Expr)
    (h : x ∉ exprVars e) : evalExpr (ρ.set x v) e = evalExpr ρ e := by
  induction e with
  | val _ => rfl
  | var y =>
    have hne : y ≠ x := by
      intro heq; apply h; simp [exprVars]; exact heq.symm
    exact Env.set_ne ρ x y v hne
  | bin op a b iha ihb =>
    have ha : x ∉ exprVars a := fun hm => h (by simp [exprVars]; exact Or.inl hm)
    have hb : x ∉ exprVars b := fun hm => h (by simp [exprVars]; exact Or.inr hm)
    simp only [evalExpr, iha ha, ihb hb]
  | un op a iha =>
    simp only [evalExpr, iha h]

/-- evalExpr is a function, so it is trivially deterministic. -/
theorem evalExpr_deterministic (ρ : Env) (e : Expr) :
    ∀ v1 v2, evalExpr ρ e = some v1 → evalExpr ρ e = some v2 → v1 = v2 := by
  intro v1 v2 h1 h2
  simp [h1] at h2
  exact h2

end MoltTIR
