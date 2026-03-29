/-
  MoltTIR.Passes.JoinCanonCorrect — correctness proof for join canonicalization.

  Key insight: buildJoinMap maps each (target, args) signature to the
  *original* target label. Therefore canonicalizeJump is always identity,
  joinCanonTerminator is identity, and joinCanonFunc preserves execFunc.
-/
import MoltTIR.Passes.JoinCanon
import MoltTIR.Semantics.ExecBlock
import MoltTIR.Semantics.ExecFunc
import MoltTIR.Semantics.BlockCorrect

set_option autoImplicit false

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Join map soundness
-- ══════════════════════════════════════════════════════════════════

/-- A join map is sound w.r.t. a function if every canonical label
    points to a block with the same params as the original target. -/
def JoinMapSound (jmap : JoinMap) (f : Func) : Prop :=
  ∀ sig canonical, (sig, canonical) ∈ jmap →
    ∀ origBlk canonBlk,
      f.blocks sig.target = some origBlk →
      f.blocks canonical = some canonBlk →
      origBlk.params = canonBlk.params

/-- Empty join map is trivially sound. -/
theorem joinMapSound_empty (f : Func) : JoinMapSound [] f :=
  fun _ _ he => absurd he (List.not_mem_nil _)

-- ══════════════════════════════════════════════════════════════════
-- Section 2: canonicalizeJump preserves arguments
-- ══════════════════════════════════════════════════════════════════

/-- canonicalizeJump does not modify the argument expressions. -/
theorem canonicalizeJump_args (jmap : JoinMap) (target : Label) (args : List Expr) :
    (canonicalizeJump jmap target args).2 = args := by
  simp [canonicalizeJump]
  split
  · rfl
  · rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Join lookup membership
-- ══════════════════════════════════════════════════════════════════

/-- If joinLookup finds a canonical label, the entry is in the map. -/
theorem joinLookup_mem (jmap : JoinMap) (target : Label) (args : List Expr)
    (canonical : Label)
    (h : joinLookup jmap { target := target, args := args } = some canonical) :
    ({ target := target, args := args : JoinSig }, canonical) ∈ jmap := by
  induction jmap with
  | nil => simp [joinLookup] at h
  | cons entry rest ih =>
    simp only [joinLookup] at h
    split at h
    · case isTrue heq =>
      simp at h
      have heq' : entry.1 = { target := target, args := args : JoinSig } :=
        beq_iff_eq.mp heq
      rw [← heq', ← h]
      exact List.mem_cons_self _ _
    · case isFalse _ =>
      exact List.mem_cons_of_mem _ (ih h)

-- ══════════════════════════════════════════════════════════════════
-- Section 4: canonicalizeJump soundness
-- ══════════════════════════════════════════════════════════════════

/-- If canonicalizeJump rewrites the label, the original and canonical
    blocks have the same params (under a sound join map). -/
theorem canonicalizeJump_sound (jmap : JoinMap) (f : Func)
    (target : Label) (args : List Expr)
    (hsound : JoinMapSound jmap f) :
    let (target', _) := canonicalizeJump jmap target args
    ∀ origBlk canonBlk,
      f.blocks target = some origBlk →
      f.blocks target' = some canonBlk →
      origBlk.params = canonBlk.params := by
  simp [canonicalizeJump]
  split
  · case h_1 canonical hlookup =>
      intro origBlk canonBlk hOrig hCanon
      have hmem := joinLookup_mem jmap target args canonical hlookup
      exact hsound { target := target, args := args } canonical hmem origBlk canonBlk hOrig hCanon
  · intro origBlk canonBlk hOrig hCanon
    simp_all

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Structural preservation
-- ══════════════════════════════════════════════════════════════════

/-- Join canonicalization on a return terminator is identity. -/
theorem joinCanonTerminator_ret (jmap : JoinMap) (e : Expr) :
    joinCanonTerminator jmap (.ret e) = .ret e := rfl

/-- Join canonicalization preserves terminator variable references. -/
theorem joinCanonTerminator_preserves_vars (jmap : JoinMap) (t : Terminator) :
    termVars (joinCanonTerminator jmap t) = termVars t := by
  cases t with
  | ret e => rfl
  | jmp target args =>
    simp only [joinCanonTerminator, termVars]
    have h := canonicalizeJump_args jmap target args
    generalize canonicalizeJump jmap target args = p at h
    obtain ⟨target', args'⟩ := p
    simp at h; simp [h]
  | br cond tl ta el ea =>
    simp only [joinCanonTerminator, termVars]
    have h1 := canonicalizeJump_args jmap tl ta
    have h2 := canonicalizeJump_args jmap el ea
    generalize canonicalizeJump jmap tl ta = p1 at h1
    generalize canonicalizeJump jmap el ea = p2 at h2
    obtain ⟨tl', ta'⟩ := p1; obtain ⟨el', ea'⟩ := p2
    simp at h1 h2; simp [h1, h2]
  | yield val resume resumeArgs =>
    simp only [joinCanonTerminator, termVars]
    have h := canonicalizeJump_args jmap resume resumeArgs
    generalize canonicalizeJump jmap resume resumeArgs = p at h
    obtain ⟨resume', args'⟩ := p
    simp at h; simp [h]
  | switch scrutinee cases default_ => rfl
  | unreachable => rfl

/-- Join canonicalization preserves block instructions. -/
theorem joinCanonBlock_instrs (jmap : JoinMap) (b : Block) :
    (joinCanonBlock jmap b).instrs = b.instrs := rfl

/-- Join canonicalization preserves block params. -/
theorem joinCanonBlock_params (jmap : JoinMap) (b : Block) :
    (joinCanonBlock jmap b).params = b.params := rfl

/-- Join canonicalization preserves the number of blocks. -/
theorem joinCanonFunc_blockCount (f : Func) :
    (joinCanonFunc f).blockList.length = f.blockList.length := by
  simp [joinCanonFunc, List.length_map]

-- ══════════════════════════════════════════════════════════════════
-- Section 6: buildJoinMap produces identity mappings
-- ══════════════════════════════════════════════════════════════════

/-- Key invariant: every entry in the join map maps a signature to its
    own target. For every (sig, lbl) in the map, lbl = sig.target. -/
def JoinMapIdentity (jmap : JoinMap) : Prop :=
  ∀ sig lbl, (sig, lbl) ∈ jmap → lbl = sig.target

/-- The fold accumulator in buildJoinMap maintains JoinMapIdentity. -/
private theorem buildJoinMap_fold_identity
    (blocks : List (Label × Block)) (acc : JoinMap)
    (hacc : JoinMapIdentity acc) :
    JoinMapIdentity (blocks.foldl (fun jmap (_, blk) =>
      match blk.term with
      | .jmp target args =>
          let sig := { target := target, args := args : JoinSig }
          match joinLookup jmap sig with
          | some _ => jmap
          | none => (sig, target) :: jmap
      | _ => jmap) acc) := by
  induction blocks generalizing acc with
  | nil => exact hacc
  | cons p rest ih =>
    obtain ⟨_, blk⟩ := p
    simp only [List.foldl]
    apply ih
    cases blk.term with
    | ret _ => exact hacc
    | jmp target args =>
      simp only
      cases joinLookup acc { target := target, args := args : JoinSig } with
      | some _ => exact hacc
      | none =>
        intro sig' lbl' hmem
        simp only [List.mem_cons] at hmem
        cases hmem with
        | inl heq =>
          have hpair := Prod.mk.inj heq
          rw [hpair.2, hpair.1]
        | inr hmem => exact hacc sig' lbl' hmem
    | br _ _ _ _ _ => exact hacc
    | yield _ _ _ => exact hacc
    | switch _ _ _ => exact hacc
    | unreachable => exact hacc

/-- buildJoinMap produces a join map where every entry maps to the original target. -/
theorem buildJoinMap_identity (f : Func) : JoinMapIdentity (buildJoinMap f) := by
  simp only [buildJoinMap]
  exact buildJoinMap_fold_identity f.blockList [] (fun _ _ h => absurd h (List.not_mem_nil _))

/-- If all map entries are identity, joinLookup returns the original target. -/
theorem joinLookup_identity (jmap : JoinMap) (sig : JoinSig)
    (hid : JoinMapIdentity jmap) (canonical : Label)
    (h : joinLookup jmap sig = some canonical) :
    canonical = sig.target := by
  have hmem := joinLookup_mem jmap sig.target sig.args canonical (by
    cases sig with | mk t a => exact h)
  cases sig with | mk t a =>
    exact hid { target := t, args := a } canonical hmem

/-- canonicalizeJump is identity when the join map has the identity property. -/
theorem canonicalizeJump_identity (jmap : JoinMap) (target : Label) (args : List Expr)
    (hid : JoinMapIdentity jmap) :
    canonicalizeJump jmap target args = (target, args) := by
  simp only [canonicalizeJump]
  split
  · case h_1 canonical hlookup =>
    have := joinLookup_identity jmap { target := target, args := args } hid canonical hlookup
    simp [this]
  · rfl

/-- joinCanonTerminator is identity when the join map has the identity property. -/
theorem joinCanonTerminator_identity (jmap : JoinMap) (t : Terminator)
    (hid : JoinMapIdentity jmap) :
    joinCanonTerminator jmap t = t := by
  cases t with
  | ret _ => rfl
  | jmp target args =>
    simp only [joinCanonTerminator, canonicalizeJump_identity jmap target args hid]
  | br cond tl ta el ea =>
    simp only [joinCanonTerminator,
               canonicalizeJump_identity jmap tl ta hid,
               canonicalizeJump_identity jmap el ea hid]
  | yield val resume resumeArgs =>
    simp only [joinCanonTerminator, canonicalizeJump_identity jmap resume resumeArgs hid]
  | switch scrutinee cases default_ => rfl
  | unreachable => rfl

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Function-level correctness
-- ══════════════════════════════════════════════════════════════════

/-- joinCanonFunc preserves block lookup (found blocks). -/
theorem joinCanonFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (joinCanonFunc f).blocks lbl = some (joinCanonBlock (buildJoinMap f) blk) :=
  blocks_map_some f (joinCanonBlock (buildJoinMap f)) lbl blk h

/-- joinCanonFunc preserves block lookup failure. -/
theorem joinCanonFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (joinCanonFunc f).blocks lbl = none :=
  blocks_map_none f (joinCanonBlock (buildJoinMap f)) lbl h

/-- joinCanonFunc does not change block parameters. -/
theorem joinCanonFunc_block_params (f : Func) (b : Block) :
    (joinCanonBlock (buildJoinMap f) b).params = b.params := rfl

/-- evalTerminator is preserved by joinCanonFunc. -/
theorem joinCanon_evalTerminator (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator (joinCanonFunc f) ρ t = evalTerminator f ρ t := by
  cases t with
  | ret _ => rfl
  | jmp target args =>
    simp only [evalTerminator]
    match evalArgs ρ args with
    | none => rfl
    | some _ =>
      match hblk : f.blocks target with
      | none => simp [joinCanonFunc_blocks_none f target hblk]
      | some blk => simp [joinCanonFunc_blocks_some f target blk hblk,
                           joinCanonFunc_block_params]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    match evalExpr ρ cond with
    | some (.bool true) =>
      match evalArgs ρ ta with
      | none => rfl
      | some _ =>
        match hblk : f.blocks tl with
        | none => simp [joinCanonFunc_blocks_none f tl hblk]
        | some blk => simp [joinCanonFunc_blocks_some f tl blk hblk,
                             joinCanonFunc_block_params]
    | some (.bool false) =>
      match evalArgs ρ ea with
      | none => rfl
      | some _ =>
        match hblk : f.blocks el with
        | none => simp [joinCanonFunc_blocks_none f el hblk]
        | some blk => simp [joinCanonFunc_blocks_some f el blk hblk,
                             joinCanonFunc_block_params]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl
  | yield _ _ _ => rfl
  | switch scrutinee cases default_ =>
    simp only [evalTerminator]
    match evalExpr ρ scrutinee with
    | some (.int n) =>
      simp only []
      match hfind : (cases.find? (fun p => p.1 == n)) with
      | some (_, lbl) =>
        simp only [hfind]
        match hblk : f.blocks lbl with
        | none => simp [joinCanonFunc_blocks_none f lbl hblk]
        | some blk => simp [joinCanonFunc_blocks_some f lbl blk hblk,
                             joinCanonFunc_block_params]
      | none =>
        simp only [hfind]
        match hblk : f.blocks default_ with
        | none => simp [joinCanonFunc_blocks_none f default_ hblk]
        | some blk => simp [joinCanonFunc_blocks_some f default_ blk hblk,
                             joinCanonFunc_block_params]
    | some (.bool _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl
  | unreachable => rfl

/-- joinCanonFunc preserves execFunc for all inputs.
    Proof by fuel induction, using the identity property of buildJoinMap. -/
theorem joinCanonFunc_correct (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (joinCanonFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  have hid := buildJoinMap_identity f
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none =>
      simp [joinCanonFunc_blocks_none f lbl hblk]
    | some blk =>
      have hblk_id : joinCanonBlock (buildJoinMap f) blk = blk := by
        simp only [joinCanonBlock, joinCanonTerminator_identity (buildJoinMap f) blk.term hid]
      simp only [joinCanonFunc_blocks_some f lbl blk hblk, hblk_id]
      match hei : execInstrs ρ blk.instrs with
      | none => simp [hei]
      | some ρ' =>
        simp only [hei]
        rw [joinCanon_evalTerminator f ρ' blk.term]
        match evalTerminator f ρ' blk.term with
        | none => rfl
        | some (.ret v) => rfl
        | some (.jump target env') => exact ih env' target

end MoltTIR
