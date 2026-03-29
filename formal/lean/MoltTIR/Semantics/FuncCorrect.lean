/-
  MoltTIR.Semantics.FuncCorrect — function-level execution correctness.

  Proves that constant folding preserves function execution semantics
  (execFunc). This lifts block-level correctness to the full function
  level, handling block lookup, parameter preservation, and recursive
  jump chains.

  Key result:
  - constFoldFunc_correct: execFunc (constFoldFunc f) fuel ρ lbl = execFunc f fuel ρ lbl
-/
import MoltTIR.Semantics.BlockCorrect
import MoltTIR.Semantics.ExecFunc

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: constFoldFunc preserves block lookup
-- ══════════════════════════════════════════════════════════════════

/-- constFoldFunc preserves block lookup for found blocks. -/
theorem constFoldFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (constFoldFunc f).blocks lbl = some (constFoldBlock blk) :=
  blocks_map_some f constFoldBlock lbl blk h

/-- constFoldFunc preserves block lookup failure. -/
theorem constFoldFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (constFoldFunc f).blocks lbl = none :=
  blocks_map_none f constFoldBlock lbl h

/-- constFoldBlock does not change block parameters. -/
theorem constFoldBlock_params (b : Block) : (constFoldBlock b).params = b.params := rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 2: constFold preserves evalTerminator with folded function
-- ══════════════════════════════════════════════════════════════════

/-- Constant folding preserves evalTerminator even when the function is
    also folded. This is stronger than constFoldTerminator_correct
    (which keeps the same function) because evalTerminator uses f.blocks
    to look up target block params for jmp/br. -/
theorem constFold_evalTerminator (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator (constFoldFunc f) ρ (constFoldTerminator t)
    = evalTerminator f ρ t := by
  cases t with
  | ret e =>
    simp only [constFoldTerminator, evalTerminator]
    rw [constFoldExpr_correct ρ e]
  | jmp target args =>
    simp only [constFoldTerminator, evalTerminator]
    rw [constFoldArgs_correct ρ args]
    match evalArgs ρ args with
    | none => rfl
    | some vals =>
      match hblk : f.blocks target with
      | none => simp [constFoldFunc_blocks_none f target hblk]
      | some blk => simp [constFoldFunc_blocks_some f target blk hblk, constFoldBlock_params]
  | br cond tl ta el ea =>
    simp only [constFoldTerminator, evalTerminator]
    rw [constFoldExpr_correct ρ cond]
    match evalExpr ρ cond with
    | some (.bool true) =>
      rw [constFoldArgs_correct ρ ta]
      match evalArgs ρ ta with
      | none => rfl
      | some vals =>
        match hblk : f.blocks tl with
        | none => simp [constFoldFunc_blocks_none f tl hblk]
        | some blk => simp [constFoldFunc_blocks_some f tl blk hblk, constFoldBlock_params]
    | some (.bool false) =>
      rw [constFoldArgs_correct ρ ea]
      match evalArgs ρ ea with
      | none => rfl
      | some vals =>
        match hblk : f.blocks el with
        | none => simp [constFoldFunc_blocks_none f el hblk]
        | some blk => simp [constFoldFunc_blocks_some f el blk hblk, constFoldBlock_params]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl
  | yield val resume resumeArgs =>
    -- Both sides evaluate to none (generators not modeled)
    rfl
  | switch scrutinee cases default_ =>
    simp only [constFoldTerminator, evalTerminator]
    rw [constFoldExpr_correct ρ scrutinee]
    match evalExpr ρ scrutinee with
    | some (.int n) =>
      simp only [switchTarget]
      match hfind : cases.find? (fun p => p.fst == n) with
      | some (_, lbl) =>
        simp only [hfind]
        match hblk : f.blocks lbl with
        | none => simp [constFoldFunc_blocks_none f lbl hblk]
        | some blk => simp [constFoldFunc_blocks_some f lbl blk hblk, constFoldBlock_params]
      | none =>
        simp only [hfind]
        match hblk : f.blocks default_ with
        | none => simp [constFoldFunc_blocks_none f default_ hblk]
        | some blk => simp [constFoldFunc_blocks_some f default_ blk hblk, constFoldBlock_params]
    | some (.bool _) | some (.float _) | some (.str _) | some .none | none => rfl
  | unreachable =>
    -- Both sides evaluate to none
    rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Main theorem — constFold preserves execFunc
-- ══════════════════════════════════════════════════════════════════

/-- Constant folding preserves function execution semantics.
    Proof by induction on fuel. At each step: look up block (preserved by
    blocks_map_some/none), execute instructions (by constFoldInstrs_correct),
    evaluate terminator (by constFold_evalTerminator), recurse (by IH). -/
theorem constFoldFunc_correct (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (constFoldFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none =>
      simp [constFoldFunc_blocks_none f lbl hblk]
    | some blk =>
      simp only [constFoldFunc_blocks_some f lbl blk hblk, constFoldBlock]
      rw [constFoldInstrs_correct ρ blk.instrs]
      match execInstrs ρ blk.instrs with
      | none => rfl
      | some ρ' =>
        simp only [constFold_evalTerminator, ih]

end MoltTIR
