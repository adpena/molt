/-
  CSE helper lemmas for PassPreservesSSA.
-/
import MoltTIR.Passes.CSE

set_option maxHeartbeats 1600000

namespace MoltTIR

-- Key property: cseExpr only introduces avail entry dsts as new vars
theorem cseExpr_vars (avail : AvailMap) (e : Expr) :
    ∀ w ∈ exprVars (cseExpr avail e),
    w ∈ exprVars e ∨ ∃ entry ∈ avail, entry.dst = w := by
  induction e with
  | val _ => simp [cseExpr, exprVars]
  | var _ => intro w hw; left; unfold cseExpr at hw; exact hw
  | un _ a iha => intro w hw; unfold cseExpr at hw; exact iha w hw
  | bin op a b iha ihb =>
    intro w hw
    simp only [cseExpr] at hw
    split at hw
    · -- Case: a = .var xa, b = .var xb
      rename_i xa xb
      split at hw
      · -- availLookup = some v
        rename_i v hlook
        simp only [exprVars, List.mem_singleton] at hw
        right
        obtain ⟨entry, hmem, _, _, _, hdst⟩ := availLookup_mem avail op xa xb v hlook
        exact ⟨entry, hmem, hw ▸ hdst⟩
      · -- availLookup = none
        left; exact hw
    · -- Case: not (var, var) — recursive
      simp only [exprVars, List.mem_append] at hw ⊢
      rcases hw with h1 | h2
      · rcases iha w h1 with h | h
        · exact Or.inl (Or.inl h)
        · exact Or.inr h
      · rcases ihb w h2 with h | h
        · exact Or.inl (Or.inr h)
        · exact Or.inr h

-- cseInstrs: uses are from original uses or fixed dst set
theorem cseInstrs_vars (avail : AvailMap) (instrs : List Instr) (allDsts : List Var)
    (havail : ∀ e, e ∈ avail → e.dst ∈ allDsts)
    (hdsts : ∀ d ∈ instrs.map Instr.dst, d ∈ allDsts) :
    ∀ w ∈ (cseInstrs avail instrs).flatMap (fun i => exprVars i.rhs),
    w ∈ instrs.flatMap (fun i => exprVars i.rhs) ∨ w ∈ allDsts := by
  induction instrs generalizing avail with
  | nil => simp [cseInstrs]
  | cons i rest ih =>
    intro w hw
    simp only [cseInstrs] at hw
    have hdst_i : i.dst ∈ allDsts := hdsts i.dst (List.mem_cons_self ..)
    have hdsts_rest : ∀ d ∈ rest.map Instr.dst, d ∈ allDsts :=
      fun d hd => hdsts d (List.mem_cons_of_mem _ hd)
    simp only [List.flatMap_cons, List.mem_append] at hw ⊢
    rcases hw with hw_hd | hw_tl
    · -- w in first transformed instruction
      have hw_cse : w ∈ exprVars (cseExpr avail i.rhs) := by
        simp only [cseInstr] at hw_hd; exact hw_hd
      rcases cseExpr_vars avail i.rhs w hw_cse with h | ⟨entry, hmem, hdst⟩
      · left; exact Or.inl h
      · right; exact hdst ▸ havail entry hmem
    · -- w in rest
      have havail' : ∀ e, e ∈ (cseInstr avail i).2 → e.dst ∈ allDsts := by
        intro e he; simp only [cseInstr] at he
        split at he
        · simp only [List.mem_cons] at he
          rcases he with rfl | h; exact hdst_i; exact havail e h
        · exact havail e he
      rcases ih _ havail' hdsts_rest w hw_tl with h | h
      · left; exact Or.inr h
      · right; exact h

-- buildAvail entries' dsts are instruction dsts
theorem buildAvail_dsts (avail : AvailMap) (instrs : List Instr) (allDsts : List Var)
    (havail : ∀ e, e ∈ avail → e.dst ∈ allDsts)
    (hdsts : ∀ d ∈ instrs.map Instr.dst, d ∈ allDsts) :
    ∀ e, e ∈ buildAvail avail instrs → e.dst ∈ allDsts := by
  induction instrs generalizing avail with
  | nil => simp [buildAvail]; exact havail
  | cons i rest ih =>
    intro e he
    simp only [buildAvail] at he
    have hdst_i : i.dst ∈ allDsts := hdsts i.dst (List.mem_cons_self ..)
    have hdsts_rest : ∀ d ∈ rest.map Instr.dst, d ∈ allDsts :=
      fun d hd => hdsts d (List.mem_cons_of_mem _ hd)
    -- The avail extends with the current instruction's entry
    -- We need to show the extended avail preserves the dst invariant
    have havail' : ∀ e', e' ∈ (let avail' := match i.rhs with
        | .bin op (.var a) (.var b) =>
            { op := op, lhs := a, rhs := b, dst := i.dst : AvailEntry } :: avail
        | _ => avail; avail') → e'.dst ∈ allDsts := by
      intro e' he'
      simp only [] at he'
      split at he'
      · simp only [List.mem_cons] at he'
        rcases he' with rfl | h; exact hdst_i; exact havail e' h
      · exact havail e' he'
    exact ih _ havail' hdsts_rest e he

-- cseTerminator: vars are original or avail dsts
-- Helper for mapped expression lists
private theorem map_cseExpr_vars (avail : AvailMap) (es : List Expr) :
    ∀ w ∈ (es.map (cseExpr avail)).flatMap exprVars,
    w ∈ es.flatMap exprVars ∨ ∃ entry ∈ avail, entry.dst = w := by
  intro w hw
  simp only [List.mem_flatMap, List.mem_map] at hw
  obtain ⟨e', ⟨e, he_mem, rfl⟩, hw'⟩ := hw
  rcases cseExpr_vars avail e w hw' with h | h
  · left; simp only [List.mem_flatMap]; exact ⟨e, he_mem, h⟩
  · exact Or.inr h

theorem cseTerminator_vars (avail : AvailMap) (t : Terminator) :
    ∀ w ∈ termVars (cseTerminator avail t),
    w ∈ termVars t ∨ ∃ entry ∈ avail, entry.dst = w := by
  intro w hw
  cases t with
  | ret e => simp only [cseTerminator, termVars] at hw ⊢; exact cseExpr_vars avail e w hw
  | jmp target args =>
    simp only [cseTerminator, termVars] at hw ⊢
    exact map_cseExpr_vars avail args w hw
  | br cond tl ta el ea =>
    simp only [cseTerminator, termVars] at hw ⊢
    -- hw : w ∈ exprVars (cseExpr avail cond) ++ (ta.map (cseExpr avail)).flatMap exprVars ++ (ea.map (cseExpr avail)).flatMap exprVars
    rcases List.mem_append.mp hw with h12 | h3
    · rcases List.mem_append.mp h12 with h1 | h2
      · rcases cseExpr_vars avail cond w h1 with h' | h'
        · left; exact List.mem_append_left _ (List.mem_append_left _ h')
        · exact Or.inr h'
      · rcases map_cseExpr_vars avail ta w h2 with h' | h'
        · left; exact List.mem_append_left _ (List.mem_append_right _ h')
        · exact Or.inr h'
    · rcases map_cseExpr_vars avail ea w h3 with h' | h'
      · left; exact List.mem_append_right _ h'
      · exact Or.inr h'
  | yield val resume resumeArgs =>
    simp only [cseTerminator, termVars, List.mem_append] at hw ⊢
    rcases hw with h | h
    · rcases cseExpr_vars avail val w h with h' | h'
      · exact Or.inl (Or.inl h')
      · exact Or.inr h'
    · rcases map_cseExpr_vars avail resumeArgs w h with h' | h'
      · exact Or.inl (Or.inr h')
      · exact Or.inr h'
  | switch scrutinee _ _ =>
    simp only [cseTerminator, termVars] at hw ⊢; exact cseExpr_vars avail scrutinee w hw
  | unreachable => simp only [cseTerminator, termVars] at hw; exact nomatch hw

end MoltTIR
