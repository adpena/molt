/-
  MoltTIR.CFG — control-flow graph abstractions.

  Definitions for reasoning about control flow at the function level:
  successor/predecessor relations, reachability, and dominance.
  These support DCE (dead block elimination) and SCCP (worklist propagation).
-/
import MoltTIR.Syntax

namespace MoltTIR

/-- Labels that a terminator can transfer control to. -/
def termSuccessors : Terminator → List Label
  | .ret _ => []
  | .jmp target _ => [target]
  | .br _ thenLbl _ elseLbl _ => [thenLbl, elseLbl]
  | .yield _ resume _ => [resume]   -- STATE_YIELD resumes at one target
  | .switch _ cases default_ => default_ :: cases.map (·.2)
  | .unreachable => []

/-- All successor labels reachable from a block (via its terminator). -/
def blockSuccessors (b : Block) : List Label :=
  termSuccessors b.term

/-- Direct successor relation: lbl₁ can transfer to lbl₂ in one step. -/
def IsSuccessor (f : Func) (l1 l2 : Label) : Prop :=
  ∃ blk, f.blocks l1 = some blk ∧ l2 ∈ termSuccessors blk.term

/-- Predecessor list: all labels whose terminator names `lbl` as a target. -/
def predecessors (f : Func) (lbl : Label) : List Label :=
  f.blockList.filterMap fun (l, blk) =>
    if lbl ∈ termSuccessors blk.term then some l else none

/-- Reachability: reflexive-transitive closure of the successor relation. -/
inductive Reachable (f : Func) : Label → Label → Prop where
  | refl (l : Label) : Reachable f l l
  | step (l1 l2 l3 : Label) :
      IsSuccessor f l1 l2 → Reachable f l2 l3 → Reachable f l1 l3

/-- Dominance: d dominates l if every path from f.entry to l passes through d.
    Defined classically: if l is reachable and removing d makes l unreachable. -/
def Dominates (f : Func) (d l : Label) : Prop :=
  Reachable f f.entry l →
  ∀ (path : List Label),
    pathFromTo f f.entry l path → d ∈ path
where
  /-- A valid path is a sequence of labels connected by successor edges. -/
  pathFromTo (f : Func) : Label → Label → List Label → Prop
    | src, dst, [x] => src = x ∧ x = dst
    | src, dst, x :: y :: rest =>
        src = x ∧ IsSuccessor f x y ∧ pathFromTo f y dst (y :: rest)
    | _, _, [] => False

/-- The entry block trivially dominates itself. -/
theorem entry_dominates_self (f : Func) : Dominates f f.entry f.entry := by
  intro _hr path hpath
  cases path with
  | nil => exact absurd hpath (by simp [Dominates.pathFromTo])
  | cons x rest =>
    cases rest with
    | nil =>
      simp only [Dominates.pathFromTo] at hpath
      obtain ⟨h1, _⟩ := hpath
      subst h1
      exact List.Mem.head _
    | cons y rest' =>
      simp only [Dominates.pathFromTo] at hpath
      obtain ⟨h1, _, _⟩ := hpath
      subst h1
      exact List.Mem.head _

/-- Reachability is transitive. -/
theorem Reachable.trans {f : Func} {a b c : Label}
    (h1 : Reachable f a b) (h2 : Reachable f b c) : Reachable f a c := by
  induction h1 with
  | refl _ => exact h2
  | step l1 l2 _ hs _ ih => exact .step l1 l2 c hs (ih h2)

/-- A terminator with no successors yields a return-only block. -/
theorem ret_no_successors (e : Expr) : termSuccessors (.ret e) = [] := rfl

/-- Jump has exactly one successor. -/
theorem jmp_successors (target : Label) (args : List Expr) :
    termSuccessors (.jmp target args) = [target] := rfl

/-- Branch has exactly two successors. -/
theorem br_successors (c : Expr) (tl : Label) (ta : List Expr) (el : Label) (ea : List Expr) :
    termSuccessors (.br c tl ta el ea) = [tl, el] := rfl

/-- Yield (STATE_YIELD) has exactly one successor: the resume label.
    Models generator suspension in the CFG — control transfers to the resume
    block when the generator's __next__ is called. -/
theorem yield_successors (val : Expr) (resume : Label) (args : List Expr) :
    termSuccessors (.yield val resume args) = [resume] := rfl

end MoltTIR
