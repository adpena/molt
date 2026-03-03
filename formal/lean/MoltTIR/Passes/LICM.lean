/-
  MoltTIR.Passes.LICM — Loop Invariant Code Motion.

  Hoists pure, loop-invariant instructions from inside a loop to its
  preheader. An instruction is eligible for hoisting if:
  1. Its effect is pure (no side effects)
  2. All operand variables are defined outside the loop body

  Key definitions:
  - partitionInstrs: split block instructions into hoistable and remaining
  - licmBlock: transform a single block, extracting loop-invariant instructions
  - licmFunc: apply LICM to all blocks in a natural loop
-/
import MoltTIR.CFG.Loops

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Partition instructions into hoistable / remaining
-- ══════════════════════════════════════════════════════════════════

/-- Partition a list of instructions: those that are loop-invariant (hoistable)
    and those that are not (remaining). Preserves order within each group. -/
def partitionInstrs (f : Func) (loop : NaturalLoop) :
    List Instr → List Instr × List Instr
  | [] => ([], [])
  | i :: rest =>
    let (hoisted, remaining) := partitionInstrs f loop rest
    if isLoopInvariantExprBool f loop i.rhs then
      (i :: hoisted, remaining)
    else
      (hoisted, i :: remaining)

/-- The hoisted instructions are all loop-invariant. -/
theorem partitionInstrs_hoisted_invariant (f : Func) (loop : NaturalLoop)
    (instrs : List Instr) :
    let (hoisted, _) := partitionInstrs f loop instrs
    ∀ i ∈ hoisted, isLoopInvariantExprBool f loop i.rhs = true := by
  induction instrs with
  | nil => simp [partitionInstrs]
  | cons i rest ih =>
    simp only [partitionInstrs]
    split
    case isTrue h =>
      intro j hj
      simp at hj
      cases hj with
      | inl heq => subst heq; exact h
      | inr hrest => exact ih j hrest
    case isFalse _ =>
      intro j hj
      exact ih j hj

/-- The remaining instructions are all non-loop-invariant. -/
theorem partitionInstrs_remaining_not_invariant (f : Func) (loop : NaturalLoop)
    (instrs : List Instr) :
    let (_, remaining) := partitionInstrs f loop instrs
    ∀ i ∈ remaining, isLoopInvariantExprBool f loop i.rhs = false := by
  induction instrs with
  | nil => simp [partitionInstrs]
  | cons i rest ih =>
    simp only [partitionInstrs]
    split
    case isTrue _ =>
      intro j hj
      exact ih j hj
    case isFalse h =>
      intro j hj
      simp at hj
      cases hj with
      | inl heq => subst heq; simp at h; exact h
      | inr hrest => exact ih j hrest

-- ══════════════════════════════════════════════════════════════════
-- Section 2: LICM block transformation
-- ══════════════════════════════════════════════════════════════════

/-- Transform a block by removing loop-invariant instructions.
    Returns (hoisted instructions, modified block). -/
def licmBlock (f : Func) (loop : NaturalLoop) (b : Block) :
    List Instr × Block :=
  let (hoisted, remaining) := partitionInstrs f loop b.instrs
  (hoisted, { b with instrs := remaining })

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Function-level LICM
-- ══════════════════════════════════════════════════════════════════

/-- Collect all hoisted instructions from loop body blocks. -/
def collectHoisted (f : Func) (loop : NaturalLoop) : List Instr :=
  (loop.body.filterMap fun lbl =>
    match f.blocks lbl with
    | some blk => some (licmBlock f loop blk).1
    | none => none
  ).flatten

/-- Apply LICM to a function: for each block in the loop body,
    remove loop-invariant instructions; prepend them to the preheader. -/
def licmFunc (f : Func) (loop : NaturalLoop) (preheaderBlk : Block) : Func :=
  let hoisted := collectHoisted f loop
  let newPreheader := { preheaderBlk with
    instrs := preheaderBlk.instrs ++ hoisted }
  { f with blockList := f.blockList.map fun (lbl, blk) =>
      if lbl ∈ loop.body then
        let (_, blk') := licmBlock f loop blk
        (lbl, blk')
      else if lbl = loop.preheader then
        (lbl, newPreheader)
      else
        (lbl, blk) }

end MoltTIR
