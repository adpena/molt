/-
  MoltTIR.Passes.EdgeThreadCorrect — correctness proof for edge threading.

  Main theorem: if the abstract environment soundly approximates the
  concrete environment, then edge-threaded terminators produce the same
  execution result as the original branch terminators.

  Key insight: edge threading replaces `br cond L_then L_else` with
  `jmp L_then args` (or `jmp L_else args`) when absEvalExpr says the
  condition is a known boolean. If the abstract evaluation is sound,
  the concrete condition evaluates to the same boolean, so the branch
  would have taken the same direction anyway.

  Proof strategy:
  - Use SCCP soundness (absEvalExpr_sound from SCCPCorrect.lean):
    if absEvalExpr σ cond = .known (.bool b) and σ is sound w.r.t. ρ,
    then evalExpr ρ cond = some (.bool b).
  - Show that the threaded terminator (jmp) evaluates to the same
    TermResult as the original branch when the condition is known.
-/
import MoltTIR.Passes.EdgeThread
import MoltTIR.Passes.SCCPCorrect
import MoltTIR.Semantics.ExecBlock

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Edge threading preserves non-branch terminators
-- ══════════════════════════════════════════════════════════════════

/-- Edge threading on a return is identity. -/
theorem edgeThreadTerminator_ret (σ : AbsEnv) (e : Expr) :
    edgeThreadTerminator σ (.ret e) = .ret e := rfl

/-- Edge threading on a jump is identity. -/
theorem edgeThreadTerminator_jmp (σ : AbsEnv) (target : Label) (args : List Expr) :
    edgeThreadTerminator σ (.jmp target args) = .jmp target args := rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Edge threading on known-true branches
-- ══════════════════════════════════════════════════════════════════

/-- If the condition is known-true, edge threading produces a jump
    to the then-branch. -/
theorem edgeThreadTerminator_known_true (σ : AbsEnv) (cond : Expr)
    (tl : Label) (ta : List Expr) (el : Label) (ea : List Expr)
    (hknown : absEvalExpr σ cond = .known (.bool true)) :
    edgeThreadTerminator σ (.br cond tl ta el ea) = .jmp tl ta := by
  simp [edgeThreadTerminator, hknown]

/-- If the condition is known-false, edge threading produces a jump
    to the else-branch. -/
theorem edgeThreadTerminator_known_false (σ : AbsEnv) (cond : Expr)
    (tl : Label) (ta : List Expr) (el : Label) (ea : List Expr)
    (hknown : absEvalExpr σ cond = .known (.bool false)) :
    edgeThreadTerminator σ (.br cond tl ta el ea) = .jmp el ea := by
  simp [edgeThreadTerminator, hknown]

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Semantic correctness for known-true threading
-- ══════════════════════════════════════════════════════════════════

/-- When the abstract env says the condition is known-true and the
    abstract env is sound, the concrete branch would take the then-path.
    So threading to `jmp tl ta` is correct.

    This is the core correctness argument: the threaded terminator
    evaluates to the same TermResult as the original branch. -/
theorem edgeThread_branch_true_correct (f : Func) (σ : AbsEnv) (ρ : Env)
    (cond : Expr) (tl : Label) (ta : List Expr) (el : Label) (ea : List Expr)
    (hsound : AbsEnvSound σ ρ)
    (hknown : absEvalExpr σ cond = .known (.bool true)) :
    evalTerminator f ρ (edgeThreadTerminator σ (.br cond tl ta el ea)) =
    evalTerminator f ρ (.br cond tl ta el ea) := by
  rw [edgeThreadTerminator_known_true σ cond tl ta el ea hknown]
  -- Need to show: evalTerminator f ρ (.jmp tl ta) = evalTerminator f ρ (.br cond tl ta el ea)
  -- From SCCP soundness: evalExpr ρ cond = some (.bool true)
  have hcond := absEvalExpr_sound σ ρ cond hsound (.bool true) hknown
  simp [evalTerminator, hcond]

/-- Symmetric case for known-false threading. -/
theorem edgeThread_branch_false_correct (f : Func) (σ : AbsEnv) (ρ : Env)
    (cond : Expr) (tl : Label) (ta : List Expr) (el : Label) (ea : List Expr)
    (hsound : AbsEnvSound σ ρ)
    (hknown : absEvalExpr σ cond = .known (.bool false)) :
    evalTerminator f ρ (edgeThreadTerminator σ (.br cond tl ta el ea)) =
    evalTerminator f ρ (.br cond tl ta el ea) := by
  rw [edgeThreadTerminator_known_false σ cond tl ta el ea hknown]
  have hcond := absEvalExpr_sound σ ρ cond hsound (.bool false) hknown
  simp [evalTerminator, hcond]

-- ══════════════════════════════════════════════════════════════════
-- Section 4: General edge threading correctness
-- ══════════════════════════════════════════════════════════════════

/-- Edge threading preserves terminator semantics for all terminators,
    under a sound abstract environment.

    Case analysis:
    - ret: identity, trivially correct.
    - jmp: identity, trivially correct.
    - br with known-true condition: reduces to known_true case.
    - br with known-false condition: reduces to known_false case.
    - br with unknown/overdefined condition: identity, trivially correct.

    NOTE: this theorem inherits the `sorry` from absEvalExpr_sound
    (the var-case definedness gap documented in SCCPCorrect.lean). -/
theorem edgeThreadTerminator_correct (f : Func) (σ : AbsEnv) (ρ : Env)
    (t : Terminator) (hsound : AbsEnvSound σ ρ) :
    evalTerminator f ρ (edgeThreadTerminator σ t) =
    evalTerminator f ρ t := by
  cases t with
  | ret e => rfl
  | jmp target args => rfl
  | br cond tl ta el ea =>
    simp only [edgeThreadTerminator]
    match habseval : absEvalExpr σ cond with
    | .known (.bool true) =>
      simp [habseval]
      have hcond := absEvalExpr_sound σ ρ cond hsound (.bool true) habseval
      simp [evalTerminator, hcond]
    | .known (.bool false) =>
      simp [habseval]
      have hcond := absEvalExpr_sound σ ρ cond hsound (.bool false) habseval
      simp [evalTerminator, hcond]
    | .known (.int _) => simp [habseval]
    | .known (.float _) => simp [habseval]
    | .known (.str _) => simp [habseval]
    | .known .none => simp [habseval]
    | .unknown => simp [habseval]
    | .overdefined => simp [habseval]
  | yield val resume resumeArgs => rfl
  | switch scrutinee cases default_ => rfl
  | unreachable => rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Block-level correctness
-- ══════════════════════════════════════════════════════════════════

/-- Edge threading preserves block instructions (it only modifies
    the terminator). -/
theorem edgeThreadBlock_instrs (σ : AbsEnv) (b : Block) :
    (edgeThreadBlock σ b).instrs = b.instrs := rfl

/-- Edge threading preserves block params. -/
theorem edgeThreadBlock_params (σ : AbsEnv) (b : Block) :
    (edgeThreadBlock σ b).params = b.params := rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Function-level structural preservation
-- ══════════════════════════════════════════════════════════════════

/-- Edge threading preserves the number of blocks. -/
theorem edgeThreadFunc_blockCount (f : Func) (st : SCCPState) :
    (edgeThreadFunc f st).blockList.length = f.blockList.length := by
  simp [edgeThreadFunc, List.length_map]

/-- Edge threading preserves block labels. -/
theorem edgeThreadFunc_labels (f : Func) (st : SCCPState) :
    (edgeThreadFunc f st).blockList.map Prod.fst =
    f.blockList.map Prod.fst := by
  simp [edgeThreadFunc, List.map_map]

end MoltTIR
