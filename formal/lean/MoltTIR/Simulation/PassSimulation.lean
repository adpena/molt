/-
  MoltTIR.Simulation.PassSimulation — simulation diagram instances for each midend pass.

  Instantiates the generic simulation framework from Diagram.lean for
  each verified compiler pass:
  - constFoldSim  — constant folding (1-to-1 step correspondence)
  - dceSim        — dead code elimination (source step → 0 or 1 target steps)
  - sccpSim       — SCCP (1-to-1 with abstract env soundness)
  - cseSim        — CSE (n-to-1 expression merging, 1-to-1 block steps)

  Each instantiation defines match_states and proves (or stubs with sorry)
  the simulation property. The proofs leverage the existing per-pass
  correctness theorems (constFoldFunc_correct, etc.) to establish the
  simulation diagrams.
-/
import MoltTIR.Simulation.Diagram
import MoltTIR.Passes.ConstFold
import MoltTIR.Passes.ConstFoldCorrect
import MoltTIR.Passes.DCE
import MoltTIR.Passes.DCECorrect
import MoltTIR.Passes.SCCP
import MoltTIR.Passes.SCCPCorrect
import MoltTIR.Passes.SCCPMultiCorrect
import MoltTIR.Passes.CSE
import MoltTIR.Passes.CSECorrect
import MoltTIR.Semantics.FuncCorrect

set_option autoImplicit false

namespace MoltTIR


-- ══════════════════════════════════════════════════════════════════
-- Section 1: Constant Folding — FuncSimulation
-- ══════════════════════════════════════════════════════════════════

/-- Constant folding has a direct FuncSimulation: it preserves execFunc
    exactly (proven in FuncCorrect.lean). The match_states is identity
    on env and label — constant folding doesn't change the control flow
    or the values computed, only the syntactic form of expressions. -/
def constFoldSim : FuncSimulation constFoldFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => constFoldFunc_correct f fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨constFoldBlock blk, constFoldFunc_blocks_some f f.entry blk h, constFoldBlock_params blk⟩
  entry_block_none := fun f h => constFoldFunc_blocks_none f f.entry h

/-- Constant folding preserves behavioral equivalence. -/
theorem constFold_behavioralEquiv (f : Func) :
    BehavioralEquivalence (constFoldFunc f) f :=
  constFoldSim.toBehavioralEquiv f

-- ══════════════════════════════════════════════════════════════════
-- Section 2: DCE — block helpers then FuncSimulation
-- ══════════════════════════════════════════════════════════════════

/-- DCE match_states: the target environment agrees with the source
    on all used variables. DCE removes dead instructions, so the target
    environment may lack bindings for dead variables, but agrees on
    all variables that are actually referenced. -/
structure DCEMatchState (used : List Var) where
  src_env : Env
  tgt_env : Env
  agree : EnvAgreeOn used src_env tgt_env

/-- DCE preserves block lookup for found blocks. -/
theorem dceFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (dceFunc f).blocks lbl = some (dceBlock blk) :=
  blocks_map_some f dceBlock lbl blk h

/-- DCE preserves block lookup failure. -/
theorem dceFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (dceFunc f).blocks lbl = none :=
  blocks_map_none f dceBlock lbl h

/-- DCE does not change block parameters. -/
theorem dceBlock_params (b : Block) : (dceBlock b).params = b.params := rfl

/-- DCE does not change the terminator. -/
theorem dceBlock_term (b : Block) : (dceBlock b).term = b.term := rfl

/-- DCE preserves evalTerminator when the function is also DCE-transformed.
    The terminator is unchanged by DCE, and block params are preserved,
    so the block lookup in jmp/br gives the same params → same bindParams. -/
theorem dce_evalTerminator (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator (dceFunc f) ρ t = evalTerminator f ρ t := by
  cases t with
  | ret e => rfl
  | jmp target args =>
    simp only [evalTerminator]
    match evalArgs ρ args with
    | none => rfl
    | some vals =>
      match hblk : f.blocks target with
      | none => simp [dceFunc_blocks_none f target hblk]
      | some blk => simp [dceFunc_blocks_some f target blk hblk, dceBlock_params]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    match evalExpr ρ cond with
    | some (.bool true) =>
      match evalArgs ρ ta with
      | none => rfl
      | some vals =>
        match hblk : f.blocks tl with
        | none => simp [dceFunc_blocks_none f tl hblk]
        | some blk => simp [dceFunc_blocks_some f tl blk hblk, dceBlock_params]
    | some (.bool false) =>
      match evalArgs ρ ea with
      | none => rfl
      | some vals =>
        match hblk : f.blocks el with
        | none => simp [dceFunc_blocks_none f el hblk]
        | some blk => simp [dceFunc_blocks_some f el blk hblk, dceBlock_params]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

/-- The preconditions for dce_instrs_agreeOn hold structurally when
    `used = usedVarsSuffix instrs term`. Dead instructions have dst ∉ used
    (tautological from ¬isLive), and all RHS vars are in used
    (since usedVarsSuffix collects all RHS vars). -/
private theorem dce_instrs_agreeOn_precond_dead (instrs : List Instr) (term : Terminator) :
    ∀ i ∈ instrs, ¬isLive (usedVarsSuffix instrs term) i → i.dst ∉ usedVarsSuffix instrs term := by
  intro i _hi hlive hmem
  -- isLive used i = used.contains i.dst, which unfolds to List.elem
  -- ¬isLive means ¬(used.contains i.dst = true)
  -- but hmem : i.dst ∈ used, so used.contains i.dst = true, contradiction
  apply hlive
  simp only [isLive]
  unfold List.contains
  exact List.elem_iff.mpr hmem

private theorem dce_instrs_agreeOn_precond_rhs (instrs : List Instr) (term : Terminator) :
    ∀ i ∈ instrs, ∀ x ∈ exprVars i.rhs, x ∈ usedVarsSuffix instrs term := by
  intro i hi x hx
  simp only [usedVarsSuffix]
  exact List.mem_append_left _ (List.mem_bind.mpr ⟨i, hi, hx⟩)

/-- termVars are a subset of usedVarsSuffix. -/
private theorem termVars_sub_usedVarsSuffix (instrs : List Instr) (term : Terminator) :
    ∀ x ∈ termVars term, x ∈ usedVarsSuffix instrs term := by
  intro x hx
  simp only [usedVarsSuffix]
  exact List.mem_append_right _ hx

/-- If environments agree on termVars, evalTerminator gives the same result.
    The terminator only reads variables through evalExpr/evalArgs, so
    agreement on the referenced variables suffices. -/
private theorem evalTerminator_agreeOn (f : Func) (ρ₁ ρ₂ : Env) (t : Terminator)
    (h : EnvAgreeOn (termVars t) ρ₁ ρ₂) :
    evalTerminator f ρ₁ t = evalTerminator f ρ₂ t := by
  cases t with
  | ret e =>
    simp only [evalTerminator]
    rw [evalExpr_agreeOn ρ₁ ρ₂ e h]
  | jmp target args =>
    simp only [evalTerminator]
    have hargs : EnvAgreeOn (args.flatMap exprVars) ρ₁ ρ₂ :=
      fun x hx => h x (by simp only [termVars]; exact hx)
    rw [evalArgs_agreeOn ρ₁ ρ₂ args hargs]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    have hcond : EnvAgreeOn (exprVars cond) ρ₁ ρ₂ :=
      fun x hx => h x (by
        simp only [termVars]
        exact List.mem_append_left _ (List.mem_append_left _ hx))
    rw [evalExpr_agreeOn ρ₁ ρ₂ cond hcond]
    match evalExpr ρ₂ cond with
    | some (.bool true) =>
      have hta : EnvAgreeOn (ta.flatMap exprVars) ρ₁ ρ₂ :=
        fun x hx => h x (by
          simp only [termVars]
          exact List.mem_append_left _ (List.mem_append_right _ hx))
      rw [evalArgs_agreeOn ρ₁ ρ₂ ta hta]
    | some (.bool false) =>
      have hea : EnvAgreeOn (ea.flatMap exprVars) ρ₁ ρ₂ :=
        fun x hx => h x (by
          simp only [termVars]
          exact List.mem_append_right _ hx)
      rw [evalArgs_agreeOn ρ₁ ρ₂ ea hea]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

/-- DCE preserves InstrTotal: if all instructions evaluate in the original,
    a filtered subset also evaluates (fewer instructions, same env flow). -/
theorem dce_preserves_total (f : Func) (ht : InstrTotal f) : InstrTotal (dceFunc f) := by
  intro lbl blk' ρ hblk'
  -- blk' = dceBlock blk for some original block blk
  simp only [dceFunc, Func.blocks] at hblk'
  -- We need to show execInstrs ρ (dceBlock blk).instrs evaluates.
  -- dceBlock removes some instructions. If the original is total, the subset is too.
  -- This requires showing that filtering preserves totality.
  sorry
  -- TODO(formal, owner:compiler, milestone:M3, priority:P1, status:partial):
  -- Prove by showing that execInstrs on a filtered subset of a total instruction
  -- list also succeeds. The key: each kept instruction's RHS vars are defined
  -- (they were defined in the original), and removing dead instructions only
  -- removes bindings that aren't referenced.

/-- DCE preserves function execution for well-typed (InstrTotal) functions.
    Proof by fuel induction:
    - Base: both return none
    - Step: InstrTotal guarantees execInstrs succeeds for both original and DCE'd,
      dce_instrs_agreeOn gives environment agreement on termVars,
      dce_evalTerminator shows terminator evaluates the same,
      IH closes the recursive case. -/
theorem dceFunc_correct_wt (f : Func) (ht : InstrTotal f) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (dceFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none =>
      simp [dceFunc_blocks_none f lbl hblk]
    | some blk =>
      simp only [dceFunc_blocks_some f lbl blk hblk]
      -- InstrTotal gives us that the original instructions evaluate
      have htotal_orig := ht lbl blk ρ hblk
      obtain ⟨ρ_orig, hρ_orig⟩ := Option.isSome_iff_exists.mp htotal_orig
      -- InstrTotal (dceFunc f) gives us that the DCE'd instructions evaluate
      have ht_dce := dce_preserves_total f ht
      have hblk_dce := dceFunc_blocks_some f lbl blk hblk
      have htotal_dce := ht_dce lbl (dceBlock blk) ρ hblk_dce
      obtain ⟨ρ_dce, hρ_dce⟩ := Option.isSome_iff_exists.mp htotal_dce
      -- DCE'd block instructions
      simp only [dceBlock]
      -- Rewrite both sides with known execInstrs results
      simp only [dceBlock] at hρ_dce
      simp only [hρ_dce, hρ_orig]
      -- Now apply dce_instrs_agreeOn to get environment agreement
      have hdead := dce_instrs_agreeOn_precond_dead blk.instrs blk.term
      have hrhs := dce_instrs_agreeOn_precond_rhs blk.instrs blk.term
      have hagree_init : EnvAgreeOn (usedVarsSuffix blk.instrs blk.term) ρ ρ :=
        envAgreeOn_refl (usedVarsSuffix blk.instrs blk.term) ρ
      have hagree_final : EnvAgreeOn (usedVarsSuffix blk.instrs blk.term) ρ_dce ρ_orig :=
        dce_instrs_agreeOn (usedVarsSuffix blk.instrs blk.term) blk.instrs
          hdead hrhs ρ ρ hagree_init ρ_dce ρ_orig hρ_dce hρ_orig
      -- termVars ⊆ used, so agreement on used implies agreement on termVars
      have hagree_term : EnvAgreeOn (termVars blk.term) ρ_dce ρ_orig :=
        fun x hx => hagree_final x (termVars_sub_usedVarsSuffix blk.instrs blk.term x hx)
      -- Terminators agree: first swap dceFunc↔f, then swap ρ_dce↔ρ_orig
      rw [dce_evalTerminator f ρ_dce blk.term]
      rw [evalTerminator_agreeOn f ρ_dce ρ_orig blk.term hagree_term]
      -- Now both sides match on evalTerminator f ρ_orig blk.term
      match evalTerminator f ρ_orig blk.term with
      | none => rfl
      | some (.ret v) => rfl
      | some (.jump target env') => exact ih env' target

/-- DCE simulation at the function level (well-typed variant).
    Unlike FuncSimulation, FuncSimulationWT adds an InstrTotal precondition,
    which is necessary because DCE can change stuck behavior: removing a
    dead instruction with a type error turns .stuck into .ret v.
    Under InstrTotal, no instruction has type errors, so this cannot happen. -/
def dceSim : FuncSimulationWT dceFunc where
  simulation := fun f ht fuel ρ lbl => dceFunc_correct_wt f ht fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨dceBlock blk, dceFunc_blocks_some f f.entry blk h, dceBlock_params blk⟩
  entry_block_none := fun f h => dceFunc_blocks_none f f.entry h
  preserves_total := fun f ht => dce_preserves_total f ht

-- ══════════════════════════════════════════════════════════════════
-- Section 3: SCCP — block helpers, instruction correctness, FuncSimulation
-- ══════════════════════════════════════════════════════════════════

/-- SCCP preserves block lookup for found blocks. -/
theorem sccpFunc_blocks_some' (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (sccpFunc f).blocks lbl = some (sccpBlock AbsEnv.top blk).2 :=
  blocks_map_some f (fun b => (sccpBlock AbsEnv.top b).2) lbl blk h

/-- SCCP preserves block lookup failure. -/
theorem sccpFunc_blocks_none' (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (sccpFunc f).blocks lbl = none :=
  blocks_map_none f (fun b => (sccpBlock AbsEnv.top b).2) lbl h

/-- SCCP does not change block parameters. -/
theorem sccpBlock_params (σ : AbsEnv) (b : Block) :
    (sccpBlock σ b).2.params = b.params := rfl

/-- SCCP does not change the terminator. -/
theorem sccpBlock_term (σ : AbsEnv) (b : Block) :
    (sccpBlock σ b).2.term = b.term := rfl

/-- SCCP-transformed instructions preserve execInstrs when the abstract
    environment is sound. Proof by induction on the instruction list,
    using sccpExpr_correct at each step and absEnvSound_set +
    absEvalExpr_concretizes to maintain soundness. -/
theorem sccpInstrs_correct (σ : AbsEnv) (ρ : Env) (instrs : List Instr)
    (hsound : AbsEnvSound σ ρ) :
    execInstrs ρ (sccpInstrs σ instrs).2 = execInstrs ρ instrs := by
  induction instrs generalizing σ ρ with
  | nil => rfl
  | cons i rest ih =>
    simp only [sccpInstrs, execInstrs]
    -- Case split on abstract evaluation of i.rhs
    cases hab : absEvalExpr σ i.rhs with
    | known v =>
      -- sccpInstrs replaces i.rhs with Expr.val v
      simp only [hab]
      -- absEvalExpr_sound tells us evalExpr ρ i.rhs = some v
      have heval := absEvalExpr_sound σ ρ i.rhs hsound v hab
      simp only [evalExpr, heval]
      exact ih _ _ (absEnvSound_set σ ρ i.dst v (.known v) hsound
        (by rw [← hab]; exact absEvalExpr_concretizes σ ρ i.rhs v hsound heval))
    | unknown =>
      simp only [hab]
      match hm : evalExpr ρ i.rhs with
      | none => rfl
      | some w =>
        exact ih _ _ (absEnvSound_set σ ρ i.dst w .unknown hsound
          (by rw [← hab]; exact absEvalExpr_concretizes σ ρ i.rhs w hsound hm))
    | overdefined =>
      simp only [hab]
      match hm : evalExpr ρ i.rhs with
      | none => rfl
      | some w =>
        exact ih _ _ (absEnvSound_set σ ρ i.dst w .overdefined hsound
          (by rw [← hab]; exact absEvalExpr_concretizes σ ρ i.rhs w hsound hm))

/-- SCCP preserves evalTerminator even when the function is also
    SCCP-transformed. The terminator expression is unchanged and the
    block params used by jmp/br target lookup are preserved. -/
theorem sccp_evalTerminator (f : Func) (ρ : Env) (t : Terminator) :
    evalTerminator (sccpFunc f) ρ t = evalTerminator f ρ t := by
  cases t with
  | ret e => rfl
  | jmp target args =>
    simp only [evalTerminator]
    match evalArgs ρ args with
    | none => rfl
    | some vals =>
      match hblk : f.blocks target with
      | none => simp [sccpFunc_blocks_none' f target hblk]
      | some blk => simp [sccpFunc_blocks_some' f target blk hblk, sccpBlock_params]
  | br cond tl ta el ea =>
    simp only [evalTerminator]
    match evalExpr ρ cond with
    | some (.bool true) =>
      match evalArgs ρ ta with
      | none => rfl
      | some vals =>
        match hblk : f.blocks tl with
        | none => simp [sccpFunc_blocks_none' f tl hblk]
        | some blk => simp [sccpFunc_blocks_some' f tl blk hblk, sccpBlock_params]
    | some (.bool false) =>
      match evalArgs ρ ea with
      | none => rfl
      | some vals =>
        match hblk : f.blocks el with
        | none => simp [sccpFunc_blocks_none' f el hblk]
        | some blk => simp [sccpFunc_blocks_some' f el blk hblk, sccpBlock_params]
    | some (.int _) => rfl
    | some (.float _) => rfl
    | some (.str _) => rfl
    | some .none => rfl
    | none => rfl

/-- SCCP preserves function execution semantics.
    Proof by induction on fuel, following the constFoldFunc_correct pattern. -/
theorem sccpFunc_correct (f : Func) (fuel : Nat) (ρ : Env) (lbl : Label) :
    execFunc (sccpFunc f) fuel ρ lbl = execFunc f fuel ρ lbl := by
  induction fuel generalizing ρ lbl with
  | zero => rfl
  | succ n ih =>
    simp only [execFunc]
    match hblk : f.blocks lbl with
    | none =>
      simp [sccpFunc_blocks_none' f lbl hblk]
    | some blk =>
      simp only [sccpFunc_blocks_some' f lbl blk hblk, sccpBlock]
      rw [sccpInstrs_correct AbsEnv.top ρ blk.instrs (absEnvTop_sound ρ)]
      match execInstrs ρ blk.instrs with
      | none => rfl
      | some ρ' =>
        simp only [sccp_evalTerminator, ih]

/-- SCCP simulation. -/
def sccpSim : FuncSimulation sccpFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => sccpFunc_correct f fuel ρ lbl
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨(sccpBlock AbsEnv.top blk).2, sccpFunc_blocks_some' f f.entry blk h,
     sccpBlock_params AbsEnv.top blk⟩
  entry_block_none := fun f h => sccpFunc_blocks_none' f f.entry h

/-- SCCP preserves block lookup for found blocks (existential form). -/
theorem sccpFunc_blocks_some (f : Func) (lbl : Label) (blk : Block) :
    f.blocks lbl = some blk →
    ∃ blk', (sccpFunc f).blocks lbl = some blk' := by
  intro h
  exact ⟨(sccpBlock AbsEnv.top blk).2, sccpFunc_blocks_some' f lbl blk h⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 4: CSE — block helpers then FuncSimulation
-- ══════════════════════════════════════════════════════════════════

/-- CSE preserves block lookup for found blocks. -/
theorem cseFunc_blocks_some (f : Func) (lbl : Label) (blk : Block)
    (h : f.blocks lbl = some blk) :
    (cseFunc f).blocks lbl = some (cseBlock blk) :=
  blocks_map_some f cseBlock lbl blk h

/-- CSE preserves block lookup failure. -/
theorem cseFunc_blocks_none (f : Func) (lbl : Label)
    (h : f.blocks lbl = none) :
    (cseFunc f).blocks lbl = none :=
  blocks_map_none f cseBlock lbl h

/-- CSE does not change block parameters. -/
theorem cseBlock_params (b : Block) : (cseBlock b).params = b.params := rfl

/-- CSE simulation. -/
def cseSim : FuncSimulation cseFunc where
  match_env := fun _f ρ lbl ρ' lbl' => ρ = ρ' ∧ lbl = lbl'
  simulation := fun f fuel ρ lbl => by
    -- TODO(formal, owner:compiler, milestone:M3, priority:P2, status:partial):
    -- Unlike DCE, CSE simulation IS provable without well-typedness:
    -- CSE replaces e with .var x where x was defined by an earlier instruction
    -- with the same RHS. Under SSA, the env at the use point has x = evalExpr ρ e,
    -- so .var x evaluates to the same value. Requires:
    -- 1. AvailMapSound threading through sccpInstrs
    -- 2. SSA freshness (x not redefined between def and use)
    -- 3. Fuel induction with block-level agreement
    sorry
  entry_preserved := fun _ => rfl
  entry_block_some := fun f blk h =>
    ⟨cseBlock blk, cseFunc_blocks_some f f.entry blk h, cseBlock_params blk⟩
  entry_block_none := fun f h => cseFunc_blocks_none f f.entry h

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Summary of simulation status
-- ══════════════════════════════════════════════════════════════════

/-
  Pass simulation status:

  | Pass          | Simulation type   | execFunc preserved         | Behavioral equiv | blocks_some/none |
  |---------------|:-----------------:|:--------------------------:|:----------------:|:----------------:|
  | ConstFold     | FuncSimulation    |             ✓              |        ✓         |        ✓         |
  | DCE           | FuncSimulationWT  | ✓ (modulo preserves_total) | via WT           |        ✓         |
  | SCCP          | FuncSimulation    |             ✓              |        ✓         |        ✓         |
  | CSE           | FuncSimulation    |         sorry (P2)         |    sorry (P2)    |        ✓         |

  ConstFold has a complete end-to-end proof chain: FuncSimulation (via
  constFoldFunc_correct from Semantics/FuncCorrect.lean) and BehavioralEquivalence
  (via FuncSimulation.toBehavioralEquiv).

  SCCP now has a complete end-to-end proof chain: FuncSimulation (via
  sccpFunc_correct proved here using sccpInstrs_correct + sccp_evalTerminator)
  and BehavioralEquivalence (via FuncSimulation.toBehavioralEquiv).

  DCE now uses FuncSimulationWT (well-typed simulation) with InstrTotal f
  as precondition. The fuel induction step is fully proven: dce_instrs_agreeOn
  gives env agreement, evalTerminator_agreeOn bridges envs for the terminator,
  dce_evalTerminator handles the function difference, and the IH closes the
  recursive case. The only remaining sorry is dce_preserves_total (showing
  InstrTotal is preserved by DCE), which requires proving that filtering
  instructions preserves totality.

  CSE block lookup lemmas are proven via blocks_map_some/none from BlockCorrect.
-/

end MoltTIR
