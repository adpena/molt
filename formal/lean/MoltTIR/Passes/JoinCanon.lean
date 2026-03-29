/-
  MoltTIR.Passes.JoinCanon — join label canonicalization on TIR.

  Normalizes join points in the CFG: when multiple blocks jump to the
  same target with identical arguments, they can share a single canonical
  join block. This reduces phi-node complexity and exposes more
  optimization opportunities for subsequent passes.

  In Molt's midend pipeline, this corresponds to
  `_normalize_try_except_join_labels`: it identifies blocks that branch
  to the same join point with identical argument lists and rewires them
  to a single canonical copy.

  Model: for each block's terminator, if it jumps to a target that has
  already been seen with the same arguments, rewrite the jump to use
  the canonical copy. This is a purely structural CFG transformation
  that preserves semantics by construction — it only renames labels
  and does not change the values flowing through edges.
-/
import MoltTIR.Semantics.EvalExpr

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Canonical join map
-- ══════════════════════════════════════════════════════════════════

/-- A join signature: target label + argument expressions.
    Two jumps with the same signature are join-equivalent. -/
structure JoinSig where
  target : Label
  args   : List Expr
  deriving DecidableEq, Repr

/-- Map from join signatures to canonical labels. -/
abbrev JoinMap := List (JoinSig × Label)

/-- Look up a canonical label for a join signature. -/
def joinLookup : JoinMap → JoinSig → Option Label
  | [], _ => none
  | (sig, lbl) :: rest, s =>
      if sig == s then some lbl else joinLookup rest s

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Terminator rewriting
-- ══════════════════════════════════════════════════════════════════

/-- Rewrite a label in a jump using the join map.
    If the (target, args) pair has a canonical entry, redirect to it. -/
def canonicalizeJump (jmap : JoinMap) (target : Label) (args : List Expr) :
    Label × List Expr :=
  let sig := { target := target, args := args : JoinSig }
  match joinLookup jmap sig with
  | some canonical => (canonical, args)
  | none => (target, args)

/-- Rewrite terminators to use canonical join labels. -/
def joinCanonTerminator (jmap : JoinMap) : Terminator → Terminator
  | .ret e => .ret e
  | .jmp target args =>
      let (target', args') := canonicalizeJump jmap target args
      .jmp target' args'
  | .br cond tl ta el ea =>
      let (tl', ta') := canonicalizeJump jmap tl ta
      let (el', ea') := canonicalizeJump jmap el ea
      .br cond tl' ta' el' ea'
  | .yield val resume resumeArgs =>
      let (resume', args') := canonicalizeJump jmap resume resumeArgs
      .yield val resume' args'
  | .switch scrutinee cases default_ =>
      .switch scrutinee cases default_  -- switch targets not canonicalized (no args)
  | .unreachable => .unreachable

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Block and function-level canonicalization
-- ══════════════════════════════════════════════════════════════════

/-- Apply join canonicalization to a block's terminator. -/
def joinCanonBlock (jmap : JoinMap) (b : Block) : Block :=
  { b with term := joinCanonTerminator jmap b.term }

/-- Build a join map from a function: collect all (target, args) pairs
    from jump terminators and assign canonical labels.
    First occurrence of each signature becomes the canonical one. -/
def buildJoinMap (f : Func) : JoinMap :=
  f.blockList.foldl (fun jmap (_, blk) =>
    match blk.term with
    | .jmp target args =>
        let sig := { target := target, args := args : JoinSig }
        match joinLookup jmap sig with
        | some _ => jmap  -- already have a canonical entry
        | none => (sig, target) :: jmap  -- first occurrence
    | _ => jmap
  ) []

/-- Apply join canonicalization to a function. -/
def joinCanonFunc (f : Func) : Func :=
  let jmap := buildJoinMap f
  { f with blockList := f.blockList.map fun (lbl, blk) =>
      (lbl, joinCanonBlock jmap blk) }

end MoltTIR
