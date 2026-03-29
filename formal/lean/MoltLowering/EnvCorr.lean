/-
  MoltLowering.EnvCorr — helper lemmas for lowerEnv_corr.
-/
import MoltLowering.ASTtoTIR

set_option maxHeartbeats 800000
set_option autoImplicit false

namespace MoltLowering

/-- NameMap injectivity: distinct names map to distinct SSA vars. -/
def NameMap.Injective (nm : NameMap) : Prop :=
  ∀ x₁ x₂ n, nm.lookup x₁ = some n → nm.lookup x₂ = some n → x₁ = x₂

/-- If (x, v) is the first binding in scope and nm maps x to n, lowerScope sets n to lowerValue v. -/
theorem lowerScope_head (nm : NameMap) (x : MoltPython.Name) (v : MoltPython.PyValue)
    (rest : MoltPython.Scope) (ρ : MoltTIR.Env) (n : MoltTIR.Var) (tv : MoltTIR.Value)
    (hnm : nm.lookup x = some n) (htv : lowerValue v = some tv) :
    (lowerScope nm ((x, v) :: rest) ρ) n = some tv := by
  simp only [lowerScope, hnm, htv, MoltTIR.Env.set, ite_true]

/-- lowerScope on a binding that maps to a different SSA var doesn't affect n.
    Requires: the binding (y, w) maps to m ≠ n. -/
theorem lowerScope_ne (nm : NameMap) (y : MoltPython.Name) (w : MoltPython.PyValue)
    (rest : MoltPython.Scope) (ρ : MoltTIR.Env) (n : MoltTIR.Var)
    (hne : ∀ m, nm.lookup y = some m → m ≠ n) :
    (lowerScope nm ((y, w) :: rest) ρ) n = (lowerScope nm rest ρ) n := by
  simp only [lowerScope]
  match hnm_y : nm.lookup y, hlv_w : lowerValue w with
  | some m, some tv =>
    simp only [hnm_y, hlv_w, MoltTIR.Env.set]
    have : m ≠ n := hne m hnm_y
    simp [Ne.symm this]
  | some _, none => simp [hnm_y, hlv_w]
  | none, _ => simp [hnm_y]

/-- lowerScope preserves ρ n when no binding in scope maps to n via nm.
    Requires NameMap injectivity and that x (which maps to n) is not in scope. -/
theorem lowerScope_preserves (nm : NameMap) (scope : MoltPython.Scope) (ρ : MoltTIR.Env)
    (x : MoltPython.Name) (n : MoltTIR.Var)
    (hnm : nm.lookup x = some n)
    (hinj : NameMap.Injective nm)
    (habs : scope.lookup x = none) :
    (lowerScope nm scope ρ) n = ρ n := by
  induction scope with
  | nil => simp [lowerScope]
  | cons hd tl ih =>
    obtain ⟨y, w⟩ := hd
    simp only [MoltPython.Scope.lookup] at habs
    split at habs
    · -- y == x: contradiction with habs
      rename_i heq; simp at habs
    · -- y ≠ x
      rename_i hne_yx
      have hne_beq : ¬(y == x) = true := hne_yx
      have hne : y ≠ x := by intro h; simp [h] at hne_beq
      -- nm.lookup y = some m → m ≠ n (by injectivity + y ≠ x)
      have hne_var : ∀ m, nm.lookup y = some m → m ≠ n := by
        intro m hm hmn
        have := hinj y x n (hmn ▸ hm) hnm
        exact hne this
      -- Unfold one step: lowerScope ((y,w)::tl) ρ = match nm.lookup y, lowerValue w with ...
      simp only [lowerScope]
      match hnm_y : nm.lookup y, hlv_w : lowerValue w with
      | some m, some tv =>
        simp only [MoltTIR.Env.set]
        have : m ≠ n := hne_var m hnm_y
        simp only [Ne.symm this, ite_false]
        exact ih habs
      | some _, none => exact ih habs
      | none, _ => exact ih habs

/-- Key lemma: lowerScope correctly sets n when scope contains x first. -/
theorem lowerScope_lookup (nm : NameMap) (scope : MoltPython.Scope) (ρ : MoltTIR.Env)
    (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue) (tv : MoltTIR.Value)
    (hnm : nm.lookup x = some n)
    (hinj : NameMap.Injective nm)
    (hslookup : scope.lookup x = some v)
    (htv : lowerValue v = some tv) :
    (lowerScope nm scope ρ) n = some tv := by
  induction scope with
  | nil => simp [MoltPython.Scope.lookup] at hslookup
  | cons hd tl ih =>
    obtain ⟨y, w⟩ := hd
    simp only [MoltPython.Scope.lookup] at hslookup
    split at hslookup
    · -- y == x: this binding provides v
      rename_i heq
      have heq' : y = x := by simpa using heq
      subst heq'
      simp only [Option.some.injEq] at hslookup
      subst hslookup
      exact lowerScope_head nm _ _ tl ρ n tv hnm htv
    · -- y ≠ x: recurse
      rename_i hne_yx
      have hne : y ≠ x := by intro h; simp [h] at hne_yx
      have hne_var : ∀ m, nm.lookup y = some m → m ≠ n := by
        intro m hm hmn; exact hne (hinj y x n (hmn ▸ hm) hnm)
      -- Unfold one step of lowerScope
      simp only [lowerScope]
      match hnm_y : nm.lookup y, hlv_w : lowerValue w with
      | some m, some tv' =>
        simp only [MoltTIR.Env.set, Ne.symm (hne_var m hnm_y), ite_false]
        exact ih hslookup
      | some _, none => exact ih hslookup
      | none, _ => exact ih hslookup

/-- lowerScopes correctly maps names to SSA vars through the scope chain. -/
theorem lowerScopes_corr (nm : NameMap) (scopes : List MoltPython.Scope) (ρ : MoltTIR.Env)
    (x : MoltPython.Name) (n : MoltTIR.Var) (v : MoltPython.PyValue) (tv : MoltTIR.Value)
    (hnm : nm.lookup x = some n)
    (hinj : NameMap.Injective nm)
    (hlookup : MoltPython.lookupScopes scopes x = some v)
    (htv : lowerValue v = some tv) :
    (lowerScopes nm scopes ρ) n = some tv := by
  induction scopes with
  | nil => simp [MoltPython.lookupScopes] at hlookup
  | cons s rest ih =>
    simp only [MoltPython.lookupScopes] at hlookup
    simp only [lowerScopes]
    match hs : s.lookup x with
    | some v' =>
      -- Found in innermost scope
      simp [hs] at hlookup
      subst hlookup
      exact lowerScope_lookup nm s (lowerScopes nm rest ρ) x n v' tv hnm hinj hs htv
    | none =>
      -- Not in this scope, search deeper
      simp [hs] at hlookup
      have ih_result := ih hlookup
      exact lowerScope_preserves nm s (lowerScopes nm rest ρ) x n hnm hinj hs ▸ ih_result

end MoltLowering
