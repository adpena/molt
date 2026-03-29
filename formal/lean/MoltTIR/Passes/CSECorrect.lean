/-
  MoltTIR.Passes.CSECorrect — correctness proof for common subexpression elimination.

  Main theorem: if the availability map is sound (each entry's result variable
  holds the value of the corresponding expression), then CSE-transformed
  expressions evaluate identically to the originals.

  Key results:
  - cseExpr_correct: CSE preserves expression semantics under a sound avail map.
  - availMapSound_cons_fresh: SSA freshness maintains soundness through instructions.
  - cseInstr_correct: CSE instruction preserves expression semantics.
-/
import MoltTIR.Passes.CSE
import MoltTIR.Passes.DCECorrect

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Soundness predicate
-- ══════════════════════════════════════════════════════════════════

/-- An availability map is sound w.r.t. an environment if for every entry,
    the destination variable holds the same value as evaluating the expression. -/
def AvailMapSound (avail : AvailMap) (ρ : Env) : Prop :=
  ∀ e ∈ avail, ρ e.dst = evalExpr ρ (.bin e.op (.var e.lhs) (.var e.rhs))

/-- SSA freshness: variable x does not appear in any avail map entry. -/
def AvailFreshWrt (avail : AvailMap) (x : Var) : Prop :=
  ∀ e ∈ avail, e.dst ≠ x ∧ e.lhs ≠ x ∧ e.rhs ≠ x

/-- Empty availability map is trivially sound. -/
theorem availMapSound_empty (ρ : Env) : AvailMapSound [] ρ :=
  fun _ he => nomatch he

/-- Empty availability map is trivially fresh. -/
theorem availFreshWrt_empty (x : Var) : AvailFreshWrt [] x :=
  fun _ he => nomatch he

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Lookup soundness
-- ══════════════════════════════════════════════════════════════════

/-- If lookup finds v in a sound avail map, then ρ v equals the expression. -/
theorem lookup_sound (avail : AvailMap) (ρ : Env) (op : BinOp) (a b v : Var)
    (hsound : AvailMapSound avail ρ)
    (hlookup : availLookup avail op a b = some v) :
    ρ v = evalExpr ρ (.bin op (.var a) (.var b)) := by
  obtain ⟨entry, hmem, hop, hlhs, hrhs, hdst⟩ := availLookup_mem avail op a b v hlookup
  have h := hsound entry hmem
  rw [hop, hlhs, hrhs, hdst] at h
  exact h

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Expression-level correctness
-- ══════════════════════════════════════════════════════════════════

/-- CSE preserves expression semantics under a sound availability map. -/
theorem cseExpr_correct (avail : AvailMap) (ρ : Env) (e : Expr)
    (hsound : AvailMapSound avail ρ) :
    evalExpr ρ (cseExpr avail e) = evalExpr ρ e := by
  induction e with
  | val _ => rfl
  | var _ => rfl
  | bin op a b iha ihb =>
    match a, b with
    | .var xa, .var xb =>
      simp only [cseExpr]
      match hlook : availLookup avail op xa xb with
      | some v =>
        simp only [hlook, evalExpr]
        exact lookup_sound avail ρ op xa xb v hsound hlook
      | none => simp only [hlook]
    | .val _, _ =>
      show evalExpr ρ (.bin op (cseExpr avail (.val _)) (cseExpr avail _)) =
           evalExpr ρ (.bin op (.val _) _)
      rw [evalExpr_bin, evalExpr_bin, iha, ihb]
    | .bin _ _ _, _ =>
      show evalExpr ρ (.bin op (cseExpr avail (.bin _ _ _)) (cseExpr avail _)) =
           evalExpr ρ (.bin op (.bin _ _ _) _)
      rw [evalExpr_bin, evalExpr_bin, iha, ihb]
    | .un _ _, _ =>
      show evalExpr ρ (.bin op (cseExpr avail (.un _ _)) (cseExpr avail _)) =
           evalExpr ρ (.bin op (.un _ _) _)
      rw [evalExpr_bin, evalExpr_bin, iha, ihb]
    | .var _, .val _ =>
      show evalExpr ρ (.bin op (cseExpr avail (.var _)) (cseExpr avail (.val _))) =
           evalExpr ρ (.bin op (.var _) (.val _))
      rw [evalExpr_bin, evalExpr_bin, iha, ihb]
    | .var _, .bin _ _ _ =>
      show evalExpr ρ (.bin op (cseExpr avail (.var _)) (cseExpr avail (.bin _ _ _))) =
           evalExpr ρ (.bin op (.var _) (.bin _ _ _))
      rw [evalExpr_bin, evalExpr_bin, iha, ihb]
    | .var _, .un _ _ =>
      show evalExpr ρ (.bin op (cseExpr avail (.var _)) (cseExpr avail (.un _ _))) =
           evalExpr ρ (.bin op (.var _) (.un _ _))
      rw [evalExpr_bin, evalExpr_bin, iha, ihb]
  | un op a iha =>
    show evalExpr ρ (.un op (cseExpr avail a)) = evalExpr ρ (.un op a)
    rw [evalExpr_un, evalExpr_un, iha]

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Avail map maintenance under SSA freshness
-- ══════════════════════════════════════════════════════════════════

/-- Helper: x not in the expression vars of (bin op (var a) (var b)). -/
private theorem not_in_bin_var_var (op : BinOp) (a b x : Var)
    (ha : a ≠ x) (hb : b ≠ x) :
    x ∉ exprVars (.bin op (.var a) (.var b)) := by
  simp only [exprVars, List.mem_append, List.mem_cons, List.mem_nil_iff, or_false]
  exact fun h => h.elim (fun h => ha h.symm) (fun h => hb h.symm)

/-- Setting a fresh variable preserves avail map soundness. -/
theorem availMapSound_set_fresh (avail : AvailMap) (ρ : Env) (x : Var) (v : Value)
    (hsound : AvailMapSound avail ρ)
    (hfresh : AvailFreshWrt avail x) :
    AvailMapSound avail (ρ.set x v) := by
  intro entry hmem
  have ⟨hd, hl, hr⟩ := hfresh entry hmem
  have hold := hsound entry hmem
  have hni := not_in_bin_var_var entry.op entry.lhs entry.rhs x hl hr
  rw [Env.set_ne ρ x entry.dst v hd, evalExpr_set_irrelevant ρ x v _ hni]
  exact hold

/-- Adding a new entry after computing the expression, under SSA freshness. -/
theorem availMapSound_cons_fresh (avail : AvailMap) (ρ : Env)
    (op : BinOp) (a b dst : Var) (val : Value)
    (hsound : AvailMapSound avail ρ)
    (hfresh : AvailFreshWrt avail dst)
    (ha : a ≠ dst) (hb : b ≠ dst)
    (heval : evalExpr ρ (.bin op (.var a) (.var b)) = some val) :
    AvailMapSound
      ({ op := op, lhs := a, rhs := b, dst := dst } :: avail)
      (ρ.set dst val) := by
  intro entry hmem
  simp only [List.mem_cons] at hmem
  cases hmem with
  | inl heq =>
    subst heq
    simp only [AvailEntry.dst, AvailEntry.op, AvailEntry.lhs, AvailEntry.rhs]
    have hni := not_in_bin_var_var op a b dst ha hb
    rw [Env.set_eq, evalExpr_set_irrelevant ρ dst val _ hni]
    exact heval.symm
  | inr hmem' =>
    exact availMapSound_set_fresh avail ρ dst val hsound hfresh entry hmem'

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Instruction-level correctness
-- ══════════════════════════════════════════════════════════════════

/-- CSE instruction preserves expression semantics. -/
theorem cseInstr_correct (avail : AvailMap) (ρ : Env) (i : Instr)
    (hsound : AvailMapSound avail ρ) :
    evalExpr ρ (cseInstr avail i).1.rhs = evalExpr ρ i.rhs := by
  simp only [cseInstr]
  exact cseExpr_correct avail ρ i.rhs hsound

end MoltTIR
