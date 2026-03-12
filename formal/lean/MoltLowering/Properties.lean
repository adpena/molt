/-
  MoltLowering.Properties — Structural properties of the AST→TIR lowering.

  Key properties:
  1. lowerValue is injective on its domain (distinct Python scalar values →
     distinct TIR values)
  2. lowerExpr preserves expression structure (structural faithfulness)
  3. lowerEnv preserves variable bindings (lookup correspondence)
  4. Operator correspondence: lowerBinOp and lowerUnaryOp are injective

  These properties are prerequisites for the main correctness theorem
  in MoltLowering.Correct.
-/
import MoltLowering.ASTtoTIR

set_option autoImplicit false

namespace MoltLowering

-- ═══════════════════════════════════════════════════════════════════════════
-- lowerValue injectivity
-- ═══════════════════════════════════════════════════════════════════════════

/-- lowerValue is injective: if two Python values lower to the same TIR value,
    the original values are equal.

    This ensures no information loss during lowering — the compiler cannot
    conflate distinct source values. Only covers the scalar domain where
    lowerValue produces some. -/
theorem lowerValue_injective (a b : MoltPython.PyValue)
    (ta tb : MoltTIR.Value)
    (ha : lowerValue a = some ta)
    (hb : lowerValue b = some tb)
    (heq : ta = tb) :
    a = b := by
  subst heq
  cases a <;> cases b <;> simp [lowerValue] at ha hb <;> try (first | rfl | contradiction)
  · -- intVal, intVal
    obtain ⟨rfl⟩ := ha; obtain ⟨rfl⟩ := hb; rfl
  · -- intVal, floatVal — discriminant mismatch
    obtain ⟨rfl⟩ := ha; simp [MoltTIR.Value.int.injEq] at hb
  · -- intVal, boolVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- intVal, strVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- intVal, noneVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- floatVal, intVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- floatVal, floatVal
    obtain ⟨rfl⟩ := ha; obtain ⟨rfl⟩ := hb; rfl
  · -- floatVal, boolVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- floatVal, strVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- floatVal, noneVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- boolVal, intVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- boolVal, floatVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- boolVal, boolVal
    obtain ⟨rfl⟩ := ha; obtain ⟨rfl⟩ := hb; rfl
  · -- boolVal, strVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- boolVal, noneVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- strVal, intVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- strVal, floatVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- strVal, boolVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- strVal, strVal
    obtain ⟨rfl⟩ := ha; obtain ⟨rfl⟩ := hb; rfl
  · -- strVal, noneVal
    obtain ⟨rfl⟩ := ha; simp at hb
  · -- noneVal, intVal
    simp at ha
  · -- noneVal, floatVal
    simp at ha
  · -- noneVal, boolVal
    simp at ha
  · -- noneVal, strVal
    simp at ha
  · -- noneVal, noneVal
    rfl

/-- lowerValue roundtrips: lowering then comparing equals comparing directly
    for scalar types. -/
theorem lowerValue_some_scalar (v : MoltPython.PyValue) (tv : MoltTIR.Value)
    (h : lowerValue v = some tv) :
    match v with
    | .intVal n   => tv = .int n
    | .floatVal f => tv = .float f
    | .boolVal b  => tv = .bool b
    | .strVal s   => tv = .str s
    | .noneVal    => tv = .none
    | _           => False := by
  cases v <;> simp [lowerValue] at h <;> exact h

-- ═══════════════════════════════════════════════════════════════════════════
-- Operator lowering injectivity
-- ═══════════════════════════════════════════════════════════════════════════

/-- lowerBinOp is injective: distinct Python operators map to distinct TIR operators. -/
theorem lowerBinOp_injective (a b : MoltPython.BinOp)
    (h : lowerBinOp a = lowerBinOp b) :
    a = b := by
  cases a <;> cases b <;> simp [lowerBinOp] at h <;> rfl

/-- lowerUnaryOp is injective. -/
theorem lowerUnaryOp_injective (a b : MoltPython.UnaryOp)
    (h : lowerUnaryOp a = lowerUnaryOp b) :
    a = b := by
  cases a <;> cases b <;> simp [lowerUnaryOp] at h <;> rfl

-- ═══════════════════════════════════════════════════════════════════════════
-- Expression structure preservation
-- ═══════════════════════════════════════════════════════════════════════════

/-- Lowering a literal expression produces a TIR value expression. -/
theorem lowerExpr_intLit (nm : NameMap) (n : Int) :
    lowerExpr nm (.intLit n) = some (.val (.int n)) := by
  simp [lowerExpr]

theorem lowerExpr_floatLit (nm : NameMap) (f : Int) :
    lowerExpr nm (.floatLit f) = some (.val (.float f)) := by
  simp [lowerExpr]

theorem lowerExpr_boolLit (nm : NameMap) (b : Bool) :
    lowerExpr nm (.boolLit b) = some (.val (.bool b)) := by
  simp [lowerExpr]

theorem lowerExpr_strLit (nm : NameMap) (s : String) :
    lowerExpr nm (.strLit s) = some (.val (.str s)) := by
  simp [lowerExpr]

theorem lowerExpr_noneLit (nm : NameMap) :
    lowerExpr nm .noneLit = some (.val .none) := by
  simp [lowerExpr]

/-- Lowering a variable produces a TIR variable when the name is mapped. -/
theorem lowerExpr_name (nm : NameMap) (x : MoltPython.Name) (v : MoltTIR.Var)
    (h : nm.lookup x = some v) :
    lowerExpr nm (.name x) = some (.var v) := by
  simp [lowerExpr, h]

/-- Lowering a binop expression preserves the binary structure. -/
theorem lowerExpr_binOp (nm : NameMap) (op : MoltPython.BinOp)
    (left right : MoltPython.PyExpr)
    (tl tr : MoltTIR.Expr)
    (hl : lowerExpr nm left = some tl)
    (hr : lowerExpr nm right = some tr) :
    lowerExpr nm (.binOp op left right) = some (.bin (lowerBinOp op) tl tr) := by
  simp [lowerExpr, hl, hr]

/-- Lowering a unaryop expression preserves the unary structure. -/
theorem lowerExpr_unaryOp (nm : NameMap) (op : MoltPython.UnaryOp)
    (operand : MoltPython.PyExpr)
    (ta : MoltTIR.Expr)
    (ha : lowerExpr nm operand = some ta) :
    lowerExpr nm (.unaryOp op operand) = some (.un (lowerUnaryOp op) ta) := by
  simp [lowerExpr, ha]

-- ═══════════════════════════════════════════════════════════════════════════
-- Environment correspondence
-- ═══════════════════════════════════════════════════════════════════════════

/-- If a Python variable x is bound to a scalar value v in the innermost scope,
    and the NameMap maps x to SSA var n, then the lowered TIR environment
    maps n to lowerValue v. -/
theorem lowerScope_preserves_binding
    (nm : NameMap) (x : MoltPython.Name) (v : MoltPython.PyValue)
    (n : MoltTIR.Var) (tv : MoltTIR.Value)
    (hnm : nm.lookup x = some n)
    (hlv : lowerValue v = some tv)
    (scope : MoltPython.Scope)
    (ρ : MoltTIR.Env)
    (hscope : scope = [(x, v)]) :
    lowerScope nm scope ρ n = some tv := by
  subst hscope
  simp [lowerScope, hnm, hlv, MoltTIR.Env.set]

/-- The lowered environment respects the name map: if a Python environment
    has x → v (scalar), and the NameMap has x → n, the TIR environment has n → lowerValue v.

    This is the environment correspondence invariant that the correctness
    theorem depends on. Stated for a single-scope, single-binding environment
    as the base case. -/
theorem lowerEnv_single_binding
    (nm : NameMap) (x : MoltPython.Name) (v : MoltPython.PyValue)
    (n : MoltTIR.Var) (tv : MoltTIR.Value)
    (hnm : nm.lookup x = some n)
    (hlv : lowerValue v = some tv) :
    lowerEnv nm { scopes := [[(x, v)]] } n = some tv := by
  simp [lowerEnv, lowerScopes, lowerScope, hnm, hlv, MoltTIR.Env.set, MoltTIR.Env.empty]

end MoltLowering
