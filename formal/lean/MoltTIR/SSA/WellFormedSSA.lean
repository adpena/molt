/-
  MoltTIR.SSA.WellFormedSSA — SSA well-formedness predicates and proofs.

  A function is in SSA form if:
  1. Every variable is defined at most once across all blocks
  2. Every use of a variable is dominated by its (unique) definition
  3. Block parameters (phi-node equivalents) appear only at block entry

  These properties guarantee that def-use chains are well-defined and
  that no variable is accessed before its definition. This is the core
  invariant that all midend passes must preserve.

  References:
  - Cytron et al., "Efficiently Computing Static Single Assignment Form
    and the Control Dependence Graph" (TOPLAS 1991)
  - Zhao et al., "Formalizing the LLVM Intermediate Representation for
    Verified Program Transformations" (POPL 2012)
-/
import MoltTIR.SSA.Dominance
import MoltTIR.WellFormed

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Definition and use sites
-- ══════════════════════════════════════════════════════════════════

/-- All variables defined in a block: block parameters + instruction destinations. -/
def blockAllDefs (b : Block) : List Var :=
  b.params ++ b.instrs.map Instr.dst

/-- All variables defined in a function across all blocks. -/
def funcAllDefs (f : Func) : List Var :=
  (f.blockList.map fun (_, blk) => blockAllDefs blk).flatten

/-- Variable v is defined in block labeled lbl. -/
def DefinedIn (f : Func) (v : Var) (lbl : Label) : Prop :=
  ∃ blk, f.blocks lbl = some blk ∧ v ∈ blockAllDefs blk

/-- Collect all (variable, label) definition pairs in a function. -/
def allDefSites (f : Func) : List (Var × Label) :=
  f.blockList.bind fun (lbl, blk) =>
    (blockAllDefs blk).map fun v => (v, lbl)

/-- All variables used in a block (in instruction RHS + terminator). -/
def blockAllUses (b : Block) : List Var :=
  b.instrs.bind (fun i => exprVars i.rhs) ++ termVars b.term

/-- Variable v is used in block labeled lbl. -/
def UsedIn (f : Func) (v : Var) (lbl : Label) : Prop :=
  ∃ blk, f.blocks lbl = some blk ∧ v ∈ blockAllUses blk

-- ══════════════════════════════════════════════════════════════════
-- Section 2: SSA well-formedness structure
-- ══════════════════════════════════════════════════════════════════

/-- A function is in well-formed SSA if it satisfies the three SSA properties. -/
structure SSAWellFormed (f : Func) : Prop where
  /-- Unique definitions: every variable is defined in at most one block.
      (Within a block, SSA requires at most one definition site —
      either a param or an instruction dst, never both.) -/
  unique_defs : ∀ v lbl₁ lbl₂,
    DefinedIn f v lbl₁ → DefinedIn f v lbl₂ → lbl₁ = lbl₂
  /-- Use-dominates-def: if variable v is used in block b_use and
      defined in block b_def, then b_def dominates b_use. -/
  use_dom_def : ∀ v b_use b_def,
    UsedIn f v b_use → DefinedIn f v b_def →
    Dom f b_def b_use
  /-- Entry block exists. -/
  entry_exists : (f.blocks f.entry).isSome

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Def-use chains are well-defined under SSA
-- ══════════════════════════════════════════════════════════════════

/-- Under SSA unique_defs, the defining block of any variable is unique
    (if it exists). This means def-use chains are functions, not relations. -/
theorem ssa_def_unique {f : Func} (hssa : SSAWellFormed f)
    (v : Var) (l₁ l₂ : Label)
    (h₁ : DefinedIn f v l₁) (h₂ : DefinedIn f v l₂) :
    l₁ = l₂ :=
  hssa.unique_defs v l₁ l₂ h₁ h₂

/-- Corollary: the definition site of a variable, if it exists, is a function. -/
def defSite (f : Func) (v : Var) : Option Label :=
  match (allDefSites f).find? (fun p => p.1 == v) with
  | some (_, lbl) => some lbl
  | none => none

-- ══════════════════════════════════════════════════════════════════
-- Section 4: No undefined variable access
-- ══════════════════════════════════════════════════════════════════

/-- Under SSA, if a variable is used and the function is well-formed,
    then the variable has a definition that dominates its use. -/
theorem ssa_no_undef_access {f : Func} (hssa : SSAWellFormed f)
    (v : Var) (b_use : Label)
    (hused : UsedIn f v b_use)
    (hdef_exists : ∃ b_def, DefinedIn f v b_def) :
    ∃ b_def, DefinedIn f v b_def ∧ Dom f b_def b_use := by
  obtain ⟨b_def, hdef⟩ := hdef_exists
  exact ⟨b_def, hdef, hssa.use_dom_def v b_use b_def hused hdef⟩

/-- Under SSA with unique defs and dominance, no variable can be used
    before it is defined along any execution path. This is the key
    safety property: SSA + dominance implies memory safety for locals. -/
theorem ssa_use_after_def {f : Func} (hssa : SSAWellFormed f)
    (v : Var) (b_use b_def : Label)
    (hused : UsedIn f v b_use)
    (hdef : DefinedIn f v b_def)
    (hreach : Reachable f f.entry b_use)
    (path : List Label)
    (hpath : CFGPath f f.entry b_use path) :
    b_def ∈ path := by
  have hdom := hssa.use_dom_def v b_use b_def hused hdef
  exact hdom hreach path hpath

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Block-level SSA refinement
-- ══════════════════════════════════════════════════════════════════

/-- Within a single block, instructions are in SSA if all dsts are distinct
    and no dst collides with a block parameter. -/
def blockSSA (b : Block) : Prop :=
  let dsts := b.instrs.map Instr.dst
  dsts.Nodup ∧
  ∀ v ∈ dsts, v ∉ b.params

/-- Block-level SSA implies no duplicate definitions within a block. -/
theorem blockSSA_no_dup_defs {b : Block} (h : blockSSA b) :
    (blockAllDefs b).Nodup := by
  sorry

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Connecting to WellFormed.lean
-- ══════════════════════════════════════════════════════════════════

/-- If a function is SSA well-formed and each block passes blockWellFormed,
    then every variable use is in scope (the original WellFormed predicate
    from WellFormed.lean is implied). -/
theorem ssa_implies_wellformed {f : Func} (hssa : SSAWellFormed f) :
    ∀ (lbl : Label) (blk : Block),
      f.blocks lbl = some blk →
      blockWellFormed blk = true := by
  sorry

end MoltTIR
