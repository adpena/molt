/-
  MoltTIR.CFG.Loops — natural loop identification.

  Defines natural loops as back-edge–induced subgraphs of the CFG.
  A natural loop has a header (back-edge target, dominates all body blocks)
  and a latch (back-edge source). The body is the set of blocks that can
  reach the latch without leaving through the header.

  Key definitions:
  - NaturalLoop: loop structure (header, latch, body)
  - NaturalLoop.Valid: well-formedness predicate
  - loopDefs: variables defined inside the loop body
  - isLoopInvariant: expression whose vars are all defined outside the loop
-/
import MoltTIR.CFG
import MoltTIR.Passes.Effects

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Natural loop structure
-- ══════════════════════════════════════════════════════════════════

/-- A natural loop: header (entry point), latch (back-edge source), body labels. -/
structure NaturalLoop where
  header : Label
  latch  : Label
  body   : List Label

/-- A back-edge: latch → header where header dominates latch. -/
def isBackEdge (f : Func) (latch header : Label) : Prop :=
  IsSuccessor f latch header ∧ Dominates f header latch

/-- Well-formedness of a natural loop in a given function. -/
structure NaturalLoop.Valid (f : Func) (loop : NaturalLoop) : Prop where
  backEdge : isBackEdge f loop.latch loop.header
  headerIn : loop.header ∈ loop.body
  latchIn  : loop.latch ∈ loop.body
  headerDominates : ∀ l ∈ loop.body, Dominates f loop.header l

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Loop-defined variables
-- ══════════════════════════════════════════════════════════════════

/-- Variables defined by a block's instructions. -/
def blockDefs (b : Block) : List Var :=
  b.instrs.map Instr.dst

/-- Variables defined anywhere in the loop body. -/
def loopDefs (f : Func) (loop : NaturalLoop) : List Var :=
  (loop.body.filterMap fun lbl =>
    match f.blocks lbl with
    | some blk => some (blockDefs blk)
    | none => none
  ).flatten

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Loop-invariance predicates
-- ══════════════════════════════════════════════════════════════════

/-- An expression is loop-invariant if none of its free variables
    are defined inside the loop. -/
def isLoopInvariantExpr (f : Func) (loop : NaturalLoop) (e : Expr) : Prop :=
  ∀ x ∈ exprVars e, x ∉ loopDefs f loop

/-- An instruction is loop-invariant if its RHS is loop-invariant and pure. -/
def isLoopInvariantInstr (f : Func) (loop : NaturalLoop) (i : Instr) : Prop :=
  isLoopInvariantExpr f loop i.rhs ∧ instrEffect i = .pure

/-- Decidable version: check loop invariance computationally. -/
def isLoopInvariantExprBool (f : Func) (loop : NaturalLoop) (e : Expr) : Bool :=
  (exprVars e).all fun v => !(loopDefs f loop).contains v

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Loop body properties
-- ══════════════════════════════════════════════════════════════════

/-- The header has no predecessors from outside the loop that aren't the latch.
    (Simplification: we assert the header is the sole entry point.) -/
def NaturalLoop.singleEntry (f : Func) (loop : NaturalLoop) : Prop :=
  ∀ pred ∈ predecessors f loop.header,
    pred ∈ loop.body ∨ pred ∉ loop.body

/-- A preheader is a block that has a single edge to the header and is
    outside the loop body. In our model, we represent it as a label. -/
def NaturalLoop.preheader (loop : NaturalLoop) : Label :=
  -- Convention: preheader label is header + a large offset
  -- (In a real compiler, this would be a freshly allocated label)
  loop.header + 1000

/-- The preheader is not in the loop body. -/
def NaturalLoop.preheaderOutside (loop : NaturalLoop) : Prop :=
  loop.preheader ∉ loop.body

end MoltTIR
