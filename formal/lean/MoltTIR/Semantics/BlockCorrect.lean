/-
  MoltTIR.Semantics.BlockCorrect — block-level execution correctness.

  Lifts expression-level and instruction-level correctness theorems to
  full block execution (instructions + terminator → TermResult).

  Key results:
  - evalArgs respects environment agreement.
  - Constant folding preserves block execution outcome.
  - Constant folding preserves evalTerminator.
-/
import MoltTIR.Passes.ConstFoldCorrect
import MoltTIR.Passes.SCCPCorrect
import MoltTIR.Passes.DCECorrect

namespace MoltTIR

/-- Execute a complete block: instructions then terminator. -/
def execBlock (f : Func) (ρ : Env) (b : Block) : Option TermResult :=
  match execInstrs ρ b.instrs with
  | none => none
  | some ρ' => evalTerminator f ρ' b.term

-- ══════════════════════════════════════════════════════════════════
-- Section 1: evalArgs agreement and constFold lemmas
-- ══════════════════════════════════════════════════════════════════

/-- If environments agree on all vars in an argument list, evalArgs agrees. -/
theorem evalArgs_agreeOn (ρ₁ ρ₂ : Env) (es : List Expr)
    (h : EnvAgreeOn (es.flatMap exprVars) ρ₁ ρ₂) :
    evalArgs ρ₁ es = evalArgs ρ₂ es := by
  induction es with
  | nil => rfl
  | cons e rest ih =>
    simp only [evalArgs]
    have he : EnvAgreeOn (exprVars e) ρ₁ ρ₂ :=
      fun x hx => h x (List.mem_flatMap.mpr ⟨e, List.mem_cons_self _ _, hx⟩)
    have hrest : EnvAgreeOn (rest.flatMap exprVars) ρ₁ ρ₂ :=
      fun x hx => h x (by
        simp only [List.flatMap_cons]
        exact List.mem_append_right _ hx)
    rw [evalExpr_agreeOn ρ₁ ρ₂ e he, ih hrest]

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Constant folding preserves execution at every level
-- ══════════════════════════════════════════════════════════════════

/-- Mapping constFoldInstr over instructions preserves execInstrs. -/
theorem constFoldInstrs_correct (ρ : Env) (instrs : List Instr) :
    execInstrs ρ (instrs.map constFoldInstr) = execInstrs ρ instrs := by
  induction instrs generalizing ρ with
  | nil => rfl
  | cons i rest ih =>
    simp only [List.map, execInstrs, constFoldInstr]
    rw [constFoldExpr_correct ρ i.rhs]
    match h : evalExpr ρ i.rhs with
    | none => rfl
    | some v => exact ih (ρ.set i.dst v)

/-- Mapping constFoldExpr over an expression list preserves evalArgs. -/
theorem constFoldArgs_correct (ρ : Env) (es : List Expr) :
    evalArgs ρ (es.map constFoldExpr) = evalArgs ρ es := by
  induction es with
  | nil => rfl
  | cons e rest ih =>
    simp only [List.map, evalArgs]
    rw [constFoldExpr_correct ρ e]
    match h : evalExpr ρ e with
    | none => rfl
    | some v => rw [ih]

/-- Constant folding a terminator preserves evalTerminator. -/
theorem constFoldTerminator_correct (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator f ρ (constFoldTerminator t) = evalTerminator f ρ t := by
  cases t with
  | ret e =>
    simp only [constFoldTerminator, evalTerminator]
    rw [constFoldExpr_correct ρ e]
  | jmp target args =>
    simp only [constFoldTerminator, evalTerminator]
    rw [constFoldArgs_correct ρ args]
  | br cond tl ta el ea =>
    simp only [constFoldTerminator, evalTerminator]
    rw [constFoldExpr_correct ρ cond]
    match h : evalExpr ρ cond with
    | some (.bool true) =>
      simp only [h]
      rw [constFoldArgs_correct ρ ta]
    | some (.bool false) =>
      simp only [h]
      rw [constFoldArgs_correct ρ ea]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

/-- Constant folding preserves full block execution. -/
theorem constFoldBlock_correct (f : Func) (ρ : Env) (b : Block) :
    execBlock f ρ (constFoldBlock b) = execBlock f ρ b := by
  simp only [execBlock, constFoldBlock]
  rw [constFoldInstrs_correct ρ b.instrs]
  match h : execInstrs ρ b.instrs with
  | none => rfl
  | some ρ' => exact constFoldTerminator_correct f ρ' b.term

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Block lookup lemma for function-level transforms
-- ══════════════════════════════════════════════════════════════════

/-- Mapping a transform over blockList preserves block lookup structure. -/
theorem blocks_map_some (f : Func) (g : Block → Block) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    ({ f with blockList := f.blockList.map fun (l, b) => (l, g b) } : Func).blocks lbl
    = some (g blk) := by
  simp only [Func.blocks] at *
  generalize f.blockList = xs at h ⊢
  induction xs generalizing blk with
  | nil => simp_all [List.find?]
  | cons p rest ih =>
    obtain ⟨l, b⟩ := p
    simp only [List.map, List.find?] at *
    cases hlbl : (l == lbl) <;> simp_all

/-- Mapping a transform over blockList preserves block lookup failure. -/
theorem blocks_map_none (f : Func) (g : Block → Block) (lbl : Label)
    (h : f.blocks lbl = none) :
    ({ f with blockList := f.blockList.map fun (l, b) => (l, g b) } : Func).blocks lbl
    = none := by
  simp only [Func.blocks] at *
  generalize f.blockList = xs at h ⊢
  induction xs with
  | nil => simp_all [List.find?]
  | cons p rest ih =>
    obtain ⟨l, b⟩ := p
    simp only [List.map, List.find?] at *
    cases hlbl : (l == lbl) <;> simp_all

end MoltTIR
