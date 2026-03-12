/-
  MoltTIR.Backend.LuauEnvCorr -- Environment correspondence between MoltTIR and Luau.

  Defines the key invariant connecting the MoltTIR SSA environment (Var → Option Value)
  to the Luau string-named environment (String → Option LuauValue) via a naming
  context (VarNames : Var → String).

  The correspondence states: for every IR variable x, if ρ maps x to some value v,
  then the Luau environment maps names(x) to valueToLuau(v). Variables not bound in
  ρ are unconstrained in the Luau environment.

  This relation is the bridge needed for the semantic correctness proofs in
  LuauCorrect.lean.
-/
import MoltTIR.Backend.LuauSemantics
import MoltTIR.Backend.LuauEmit

set_option autoImplicit false

namespace MoltTIR.Backend

-- ======================================================================
-- Section 1: Environment correspondence
-- ======================================================================

/-- Environment correspondence: the Luau environment faithfully represents
    the MoltTIR environment through the naming context.

    For every IR variable x:
    - If ρ(x) = none, no constraint on lenv(names(x))
    - If ρ(x) = some v, then lenv(names(x)) = some (valueToLuau v)

    The `injective` field ensures the naming context is injective on the
    domain of ρ, preventing aliasing bugs where two different IR variables
    map to the same Luau name. -/
structure LuauEnvCorresponds (names : VarNames) (ρ : MoltTIR.Env) (lenv : LuauEnv) : Prop where
  var_corr : ∀ (x : MoltTIR.Var),
    ρ x = none ∨ (∃ v, ρ x = some v ∧ lenv (names x) = some (valueToLuau v))
  injective : ∀ (x y : MoltTIR.Var),
    ρ x ≠ none → ρ y ≠ none → names x = names y → x = y

-- ======================================================================
-- Section 2: Correspondence preservation lemmas
-- ======================================================================

/-- Empty environments correspond. -/
theorem envCorr_empty (names : VarNames) :
    LuauEnvCorresponds names MoltTIR.Env.empty LuauEnv.empty := by
  exact ⟨fun _ => Or.inl rfl, fun _ _ h => absurd rfl h⟩

/-- Setting a fresh SSA variable in both environments preserves correspondence.
    This is the case that arises in SSA instruction emission, where each dst is
    fresh (not previously bound in ρ). The freshness constraint (ρ x = none) is
    the SSA invariant. -/
theorem envCorr_set (names : VarNames) (ρ : MoltTIR.Env) (lenv : LuauEnv)
    (x : MoltTIR.Var) (v : MoltTIR.Value)
    (hcorr : LuauEnvCorresponds names ρ lenv)
    (_hfresh : ρ x = none)
    (hinj_names : ∀ (y : MoltTIR.Var), ρ y ≠ none → names y ≠ names x) :
    LuauEnvCorresponds names (ρ.set x v) (lenv.set (names x) (valueToLuau v)) := by
  constructor
  · intro y
    simp only [MoltTIR.Env.set]
    split
    · -- y = x case
      rename_i heq
      right
      exact ⟨v, rfl, by simp [LuauEnv.set, heq]⟩
    · -- y ≠ x case
      rename_i hne
      rcases hcorr.var_corr y with hnil | ⟨w, hw, hlenv⟩
      · left; exact hnil
      · right
        refine ⟨w, hw, ?_⟩
        have hne_name : names y ≠ names x := by
          apply hinj_names y
          rw [hw]; exact Option.noConfusion
        simp only [LuauEnv.set, hne_name, ite_false]
        exact hlenv
  · intro a b ha hb hab
    simp only [MoltTIR.Env.set] at ha hb
    split at ha
    · -- a = x
      rename_i heq_a
      split at hb
      · -- b = x too
        rename_i heq_b
        exact heq_a.trans heq_b.symm
      · -- b ≠ x
        rename_i hne_b
        exfalso
        exact hinj_names b hb (by rw [heq_a] at hab; exact hab.symm)
    · -- a ≠ x
      rename_i hne_a
      split at hb
      · -- b = x
        rename_i heq_b
        exfalso
        exact hinj_names a ha (by rw [heq_b] at hab; exact hab)
      · -- b ≠ x
        exact hcorr.injective a b ha hb hab

-- ======================================================================
-- Section 3: Expression evaluation correspondence
-- ======================================================================

/-- If environments correspond and a value literal evaluates in MoltTIR,
    then the emitted Luau literal evaluates to the corresponding Luau value. -/
theorem evalLuauExpr_val_corr (names : VarNames) (v : MoltTIR.Value) (env : LuauEnv) :
    evalLuauExpr env (emitExpr names (.val v)) = some (valueToLuau v) := by
  cases v <;> rfl

/-- If environments correspond, then a variable reference evaluates consistently:
    if ρ(x) = some v, then evalLuauExpr lenv (varRef (names x)) = some (valueToLuau v). -/
theorem evalLuauExpr_var_corr (names : VarNames) (ρ : MoltTIR.Env) (lenv : LuauEnv)
    (x : MoltTIR.Var) (v : MoltTIR.Value)
    (hcorr : LuauEnvCorresponds names ρ lenv)
    (hbound : ρ x = some v) :
    evalLuauExpr lenv (.varRef (names x)) = some (valueToLuau v) := by
  simp [evalLuauExpr]
  rcases hcorr.var_corr x with hnil | ⟨w, hw, hlenv⟩
  · simp [hbound] at hnil
  · rw [hbound] at hw
    cases hw
    exact hlenv

end MoltTIR.Backend
