/-
  MoltTIR.Optimization.Specialization — formal model of type specialization.

  Models type specialization: replacing generic operations with type-specific
  fast paths guarded by runtime type checks. When the type guard holds,
  the specialized code produces the same result as the generic code.
  When the guard fails, deoptimization falls back to the generic path.

  Proves:
  - Specialized code is semantically equivalent to generic code when the
    type guard holds.
  - Deoptimization correctly falls back to the generic path.
  - The full specialization + deopt scheme is a refinement of the generic code.

  References:
  - compiler/molt/codegen/ (specialization in Cranelift codegen)
  - MoltTIR.Syntax (IR syntax: Expr, Value, BinOp)
  - MoltTIR.Semantics.EvalExpr (expression evaluation)
-/
import MoltTIR.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltTIR.Optimization.Specialization

open MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Type tags and type guards
-- ══════════════════════════════════════════════════════════════════

/-- Type tag for runtime values. Corresponds to the NaN-box tag bits. -/
inductive TypeTag where
  | intTag
  | boolTag
  | floatTag
  | strTag
  | noneTag
  deriving DecidableEq, Repr

/-- Extract the type tag from a runtime value. -/
def typeOf : Value → TypeTag
  | .int _ => .intTag
  | .bool _ => .boolTag
  | .float _ => .floatTag
  | .str _ => .strTag
  | .none => .noneTag

/-- A type guard: a runtime check that a value has a specific type tag. -/
structure TypeGuard where
  /-- Variable being checked. -/
  var : Var
  /-- Expected type tag. -/
  expectedTag : TypeTag
  deriving DecidableEq, Repr

/-- Evaluate a type guard in an environment.
    Returns true if the variable has the expected type. -/
def evalGuard (ρ : Env) (g : TypeGuard) : Option Bool :=
  match ρ g.var with
  | some v => some (typeOf v == g.expectedTag)
  | none => none

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Specialized operations
-- ══════════════════════════════════════════════════════════════════

/-- A specialized operation: a type-specific fast path paired with
    the generic fallback and the guard condition. -/
structure SpecializedOp where
  /-- Type guards that must all pass for specialization. -/
  guards : List TypeGuard
  /-- The specialized expression (type-specific fast path). -/
  specialized : Expr
  /-- The generic expression (fallback). -/
  generic : Expr
  deriving Repr

/-- Evaluate all guards; return true iff all pass. -/
def evalGuards (ρ : Env) : List TypeGuard → Option Bool
  | [] => some true
  | g :: gs =>
    match evalGuard ρ g with
    | some true =>
      match evalGuards ρ gs with
      | some true => some true
      | some false => some false
      | none => none
    | some false => some false
    | none => none

/-- Execute a specialized operation: check guards, then run the
    specialized path if all pass, or the generic path if any fails. -/
def execSpecialized (ρ : Env) (op : SpecializedOp) : Option Value :=
  match evalGuards ρ op.guards with
  | some true => evalExpr ρ op.specialized
  | some false => evalExpr ρ op.generic
  | none => none

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Specialization correctness — guards-hold case
-- ══════════════════════════════════════════════════════════════════

/-- A specialization is correct if, when all guards hold, the specialized
    expression produces the same result as the generic expression. -/
def SpecializationCorrect (op : SpecializedOp) : Prop :=
  ∀ (ρ : Env),
    evalGuards ρ op.guards = some true →
    evalExpr ρ op.specialized = evalExpr ρ op.generic

/-- When a specialization is correct and guards pass, execSpecialized
    produces the same result as the generic expression. -/
theorem specialization_equiv_when_correct
    (ρ : Env) (op : SpecializedOp)
    (hcorrect : SpecializationCorrect op)
    (hguards : evalGuards ρ op.guards = some true) :
    execSpecialized ρ op = evalExpr ρ op.generic := by
  simp [execSpecialized, hguards]
  exact hcorrect ρ hguards

/-- When guards fail, execSpecialized falls back to the generic expression. -/
theorem deopt_falls_back_to_generic
    (ρ : Env) (op : SpecializedOp)
    (hguards : evalGuards ρ op.guards = some false) :
    execSpecialized ρ op = evalExpr ρ op.generic := by
  simp [execSpecialized, hguards]

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Full specialization theorem — always equivalent to generic
-- ══════════════════════════════════════════════════════════════════

/-- The main theorem: a correct specialization always produces the same
    observable result as the generic expression, regardless of whether
    guards pass or fail.
    This is the key correctness property: specialization is a refinement
    of the generic code. -/
theorem specialization_is_refinement
    (ρ : Env) (op : SpecializedOp)
    (hcorrect : SpecializationCorrect op) :
    execSpecialized ρ op = evalExpr ρ op.generic
    ∨ execSpecialized ρ op = none := by
  simp [execSpecialized]
  match hg : evalGuards ρ op.guards with
  | some true => exact Or.inl (hcorrect ρ hg)
  | some false => exact Or.inl rfl
  | none => exact Or.inr rfl

/-- Stronger version: when the environment is well-formed (all guard
    variables are defined), the specialization always produces the same
    result as generic. -/
theorem specialization_always_equiv
    (ρ : Env) (op : SpecializedOp)
    (hcorrect : SpecializationCorrect op)
    (hguards_defined : ∀ g ∈ op.guards, (ρ g.var).isSome = true) :
    execSpecialized ρ op = evalExpr ρ op.generic := by
  simp [execSpecialized]
  -- All guards evaluate to some bool (since all vars are defined)
  -- Split on whether they pass or fail
  match hg : evalGuards ρ op.guards with
  | some true => exact hcorrect ρ hg
  | some false => rfl
  | none =>
    -- This case is impossible: all guard vars are defined, so evalGuards
    -- cannot return none.
    exfalso
    -- Prove by induction on op.guards that evalGuards returns some when
    -- all guard vars are defined.
    have : ∀ (gs : List TypeGuard),
        (∀ g ∈ gs, (ρ g.var).isSome = true) →
        ∃ b, evalGuards ρ gs = some b := by
      intro gs hgs
      induction gs with
      | nil => exact ⟨true, rfl⟩
      | cons g rest ih =>
        have hg_def := hgs g (List.mem_cons_self _ _)
        have hrest := fun g' hg' => hgs g' (List.mem_cons_of_mem _ hg')
        obtain ⟨b_rest, hb_rest⟩ := ih hrest
        -- evalGuard returns some when the var is defined
        have hg_some : ∃ b, evalGuard ρ g = some b := by
          simp [evalGuard]
          cases hv : ρ g.var with
          | none => simp [hv] at hg_def
          | some v => exact ⟨_, rfl⟩
        obtain ⟨bg, hbg⟩ := hg_some
        cases bg with
        | true =>
          simp [evalGuards, hbg]
          exact ⟨b_rest, hb_rest⟩
        | false =>
          exact ⟨false, by simp [evalGuards, hbg]⟩
    obtain ⟨b, hb⟩ := this op.guards hguards_defined
    rw [hb] at hg
    exact Option.noConfusion hg

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Concrete specialization examples
-- ══════════════════════════════════════════════════════════════════

/-- Integer addition specialization: when both operands are ints,
    use direct int addition instead of generic dispatch. -/
def intAddSpec (x y : Var) : SpecializedOp :=
  { guards := [⟨x, .intTag⟩, ⟨y, .intTag⟩]
    specialized := .bin .add (.var x) (.var y)
    generic := .bin .add (.var x) (.var y) }

/-- The int-add specialization is trivially correct because the specialized
    and generic expressions are identical. In the real compiler, the
    specialized path uses a different (faster) code sequence, but at the
    IR semantics level they are the same operation. -/
theorem intAddSpec_correct (x y : Var) :
    SpecializationCorrect (intAddSpec x y) := by
  unfold SpecializationCorrect intAddSpec
  intro ρ _
  rfl

/-- Example: type guard passes when the value has the expected tag. -/
example : evalGuard (fun v => if v = 0 then some (.int 42) else none) ⟨0, .intTag⟩
    = some true := by native_decide

/-- Example: type guard fails when the value has a different tag. -/
example : evalGuard (fun v => if v = 0 then some (.bool true) else none) ⟨0, .intTag⟩
    = some false := by native_decide

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Guard composition and multi-operand specialization
-- ══════════════════════════════════════════════════════════════════

/-- evalGuards on a single guard reduces to evalGuard. -/
theorem evalGuards_singleton (ρ : Env) (g : TypeGuard) :
    evalGuards ρ [g] = evalGuard ρ g := by
  simp [evalGuards]
  cases evalGuard ρ g with
  | none => rfl
  | some b =>
    cases b <;> simp [evalGuards]

/-- If the first guard fails, the overall result is false regardless
    of remaining guards. -/
theorem evalGuards_short_circuit (ρ : Env) (g : TypeGuard) (gs : List TypeGuard)
    (hfail : evalGuard ρ g = some false) :
    evalGuards ρ (g :: gs) = some false := by
  simp [evalGuards, hfail]

/-- If the first guard passes, the result depends on the remaining guards. -/
theorem evalGuards_pass_first (ρ : Env) (g : TypeGuard) (gs : List TypeGuard)
    (hpass : evalGuard ρ g = some true) :
    evalGuards ρ (g :: gs) = evalGuards ρ gs := by
  simp [evalGuards, hpass]

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Deoptimization chain correctness
-- ══════════════════════════════════════════════════════════════════

/-- A deoptimization chain: a sequence of specializations tried in order.
    Each has its own guards; the first one whose guards pass is used.
    If none pass, the generic fallback is used. -/
def execDeoptChain (ρ : Env) (generic : Expr) : List SpecializedOp → Option Value
  | [] => evalExpr ρ generic
  | op :: rest =>
    match evalGuards ρ op.guards with
    | some true => evalExpr ρ op.specialized
    | some false => execDeoptChain ρ generic rest
    | none => none

/-- A deopt chain is correct if every specialization in it is correct
    with respect to the same generic expression. -/
def DeoptChainCorrect (generic : Expr) (chain : List SpecializedOp) : Prop :=
  ∀ op ∈ chain,
    op.generic = generic ∧
    ∀ (ρ : Env), evalGuards ρ op.guards = some true →
      evalExpr ρ op.specialized = evalExpr ρ generic

/-- A correct deopt chain always produces the same result as generic. -/
theorem deopt_chain_equiv
    (ρ : Env) (generic : Expr) (chain : List SpecializedOp)
    (hcorrect : DeoptChainCorrect generic chain) :
    execDeoptChain ρ generic chain = evalExpr ρ generic
    ∨ execDeoptChain ρ generic chain = none := by
  induction chain with
  | nil => exact Or.inl rfl
  | cons op rest ih =>
    simp [execDeoptChain]
    match hg : evalGuards ρ op.guards with
    | some true =>
      simp [hg]
      have ⟨_, hspec⟩ := hcorrect op (List.mem_cons_self _ _)
      exact Or.inl (hspec ρ hg)
    | some false =>
      simp [hg]
      exact ih (fun o ho => hcorrect o (List.mem_cons_of_mem _ ho))
    | none =>
      simp [hg]
      exact Or.inr rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Type guard correctness — guards reflect actual types
-- ══════════════════════════════════════════════════════════════════

/-- A type guard is sound: when it passes, the value truly has the
    expected type tag. -/
theorem guard_soundness (ρ : Env) (g : TypeGuard) (v : Value)
    (hdef : ρ g.var = some v)
    (hpass : evalGuard ρ g = some true) :
    typeOf v = g.expectedTag := by
  simp [evalGuard, hdef] at hpass
  exact beq_iff_eq.mp hpass

/-- A type guard is complete: when the value has the expected type,
    the guard passes. -/
theorem guard_completeness (ρ : Env) (g : TypeGuard) (v : Value)
    (hdef : ρ g.var = some v)
    (htype : typeOf v = g.expectedTag) :
    evalGuard ρ g = some true := by
  simp [evalGuard, hdef, htype]

-- ══════════════════════════════════════════════════════════════════
-- Section 9: Specialization preserves determinism
-- ══════════════════════════════════════════════════════════════════

/-- Type guards are deterministic: same environment → same result. -/
theorem guard_deterministic (ρ : Env) (g : TypeGuard) :
    ∀ b₁ b₂, evalGuard ρ g = some b₁ → evalGuard ρ g = some b₂ → b₁ = b₂ := by
  intro b₁ b₂ h₁ h₂
  simp [h₁] at h₂
  exact h₂

/-- Specialization is deterministic: same environment → same result. -/
theorem specialization_deterministic (ρ : Env) (op : SpecializedOp) :
    ∀ v₁ v₂, execSpecialized ρ op = some v₁ →
      execSpecialized ρ op = some v₂ → v₁ = v₂ := by
  intro v₁ v₂ h₁ h₂
  simp [h₁] at h₂
  exact h₂

-- ══════════════════════════════════════════════════════════════════
-- Section 10: Concrete witness — integer comparison specialization
-- ══════════════════════════════════════════════════════════════════

/-- Concrete witness: int-lt specialization on known values. -/
example :
    let ρ : Env := fun v => if v = 0 then some (.int 3) else
                             if v = 1 then some (.int 5) else none
    let op : SpecializedOp :=
      { guards := [⟨0, .intTag⟩, ⟨1, .intTag⟩]
        specialized := .bin .lt (.var 0) (.var 1)
        generic := .bin .lt (.var 0) (.var 1) }
    execSpecialized ρ op = some (.bool true) := by
  native_decide

/-- Concrete witness: guard failure triggers generic path. -/
example :
    let ρ : Env := fun v => if v = 0 then some (.bool true) else
                             if v = 1 then some (.int 5) else none
    let op : SpecializedOp :=
      { guards := [⟨0, .intTag⟩, ⟨1, .intTag⟩]
        specialized := .bin .lt (.var 0) (.var 1)
        generic := .bin .lt (.var 0) (.var 1) }
    -- Guard fails (var 0 is bool, not int), so generic path is taken
    execSpecialized ρ op = evalExpr ρ (.bin .lt (.var 0) (.var 1)) := by
  native_decide

end MoltTIR.Optimization.Specialization
