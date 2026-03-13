/-
  MoltTIR.Validation.TranslationValidation — Alive2-style translation validation framework.

  Translation validation (Pnueli, Siegel, Singerman 1998) checks that a *specific*
  optimization step is correct, rather than proving the optimization correct for all
  inputs. Alive2 (Lee et al., PLDI 2021) demonstrated that this approach finds real
  compiler bugs (47 in LLVM) with 10-50x less effort than full proofs per optimization.

  This module provides the core framework:
  1. **Refinement**: f_out refines f_in iff every observable behavior of f_out is also
     a behavior of f_in (or f_in is undefined there). This is the semantic backbone.
  2. **Per-instruction equivalence**: two instructions produce the same result under a
     given abstract environment. This is the workhorse for peephole validation.
  3. **Transform validity**: a concrete optimization step (f_in, f_out) is valid iff
     f_out refines f_in. Validators return `Decidable` witnesses when possible.
  4. **Idempotency**: a pass applied twice equals the pass applied once.

  Design decisions:
  - We work at the expression, instruction, block, and function levels, matching the
    existing pass structure in MoltTIR.Passes.
  - Refinement is defined relationally (not computationally) to support both decidable
    checking and proof-based reasoning.
  - The framework composes with existing full proofs: a full proof of pass correctness
    implies translation validation succeeds for every concrete instance.
  - We use fuel-bounded execution (execFunc) to avoid divergence, consistent with
    the rest of the formalization.

  References:
  - Alive2: Lee, Menendez, Bruber, Regehr. "Alive2: Bounded Translation Validation
    for LLVM." PLDI 2021.
  - Translation Validation: Pnueli, Siegel, Singerman. "Translation Validation."
    TACAS 1998.
  - CompCert Validator: Tristan, Leroy. "Verified Validation of Lazy Code Motion."
    PLDI 2009.
-/
import MoltTIR.Semantics.ExecFunc
import MoltTIR.Passes.Lattice
import MoltTIR.Passes.Effects

set_option autoImplicit false

namespace MoltTIR

-- ══════════════════════════════════════════════════════════════════
-- Section 1: Refinement — the semantic foundation
-- ══════════════════════════════════════════════════════════════════

/-- An expression e_out refines e_in if, for every environment where e_out
    produces a value, e_in produces the same value.

    This captures the Alive2 notion: the optimized code must not introduce
    new defined behaviors — it may only remove undefined ones. -/
def ExprRefines (e_in e_out : Expr) : Prop :=
  ∀ (ρ : Env) (v : Value),
    evalExpr ρ e_out = some v → evalExpr ρ e_in = some v

/-- Semantic equivalence of expressions: mutual refinement.
    Stronger than refinement — both expressions produce exactly the same
    results (or both are undefined) for all environments. -/
def ExprEquiv (e1 e2 : Expr) : Prop :=
  ∀ (ρ : Env), evalExpr ρ e1 = evalExpr ρ e2

/-- Equivalence implies forward refinement. -/
theorem exprEquiv_implies_refines (e1 e2 : Expr) (h : ExprEquiv e1 e2) :
    ExprRefines e1 e2 := by
  intro ρ v hout
  rw [← h ρ]; exact hout

/-- Equivalence implies backward refinement. -/
theorem exprEquiv_implies_refines_rev (e1 e2 : Expr) (h : ExprEquiv e1 e2) :
    ExprRefines e2 e1 := by
  intro ρ v hin
  rw [h ρ]; exact hin

/-- Equivalence is symmetric. -/
theorem exprEquiv_symm (e1 e2 : Expr) (h : ExprEquiv e1 e2) :
    ExprEquiv e2 e1 :=
  fun ρ => (h ρ).symm

/-- Equivalence is transitive. -/
theorem exprEquiv_trans (e1 e2 e3 : Expr)
    (h12 : ExprEquiv e1 e2) (h23 : ExprEquiv e2 e3) :
    ExprEquiv e1 e3 :=
  fun ρ => (h12 ρ).trans (h23 ρ)

/-- Equivalence is reflexive. -/
theorem exprEquiv_refl (e : Expr) : ExprEquiv e e :=
  fun _ => rfl

/-- Refinement is reflexive. -/
theorem exprRefines_refl (e : Expr) : ExprRefines e e :=
  fun _ _ h => h

/-- Refinement is transitive. -/
theorem exprRefines_trans (e1 e2 e3 : Expr)
    (h12 : ExprRefines e1 e2) (h23 : ExprRefines e2 e3) :
    ExprRefines e1 e3 :=
  fun ρ v h => h12 ρ v (h23 ρ v h)

-- ══════════════════════════════════════════════════════════════════
-- Section 2: Instruction-level refinement
-- ══════════════════════════════════════════════════════════════════

/-- An instruction i_out refines i_in if they have the same destination and
    the RHS of i_out refines the RHS of i_in. The destination constraint
    ensures SSA variable identity is preserved. -/
def InstrRefines (i_in i_out : Instr) : Prop :=
  i_in.dst = i_out.dst ∧ ExprRefines i_in.rhs i_out.rhs

/-- Instruction equivalence: same destination, equivalent RHS. -/
def InstrEquiv (i1 i2 : Instr) : Prop :=
  i1.dst = i2.dst ∧ ExprEquiv i1.rhs i2.rhs

/-- Instruction equivalence implies instruction refinement. -/
theorem instrEquiv_implies_refines (i1 i2 : Instr) (h : InstrEquiv i1 i2) :
    InstrRefines i1 i2 :=
  ⟨h.1, exprEquiv_implies_refines i1.rhs i2.rhs h.2⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 3: Block-level refinement
-- ══════════════════════════════════════════════════════════════════

/-- Block equivalence: same parameters, pointwise instruction equivalence,
    and the terminator produces equivalent control-flow decisions.

    This is a structural check — it verifies that the optimized block is
    a valid replacement at each instruction position. -/
def BlockEquiv (b1 b2 : Block) : Prop :=
  b1.params = b2.params ∧
  b1.instrs.length = b2.instrs.length ∧
  (∀ (i : Nat) (i1 i2 : Instr),
    b1.instrs.get? i = some i1 →
    b2.instrs.get? i = some i2 →
    InstrEquiv i1 i2) ∧
  TermEquiv b1.term b2.term
where
  /-- Terminator equivalence: same structure with equivalent sub-expressions. -/
  TermEquiv : Terminator → Terminator → Prop
    | .ret e1, .ret e2 => ExprEquiv e1 e2
    | .jmp t1 a1, .jmp t2 a2 =>
        t1 = t2 ∧ a1.length = a2.length ∧
        ∀ (i : Nat) (e1 e2 : Expr),
          a1.get? i = some e1 → a2.get? i = some e2 → ExprEquiv e1 e2
    | .br c1 tl1 ta1 el1 ea1, .br c2 tl2 ta2 el2 ea2 =>
        ExprEquiv c1 c2 ∧ tl1 = tl2 ∧ el1 = el2 ∧
        ta1.length = ta2.length ∧ ea1.length = ea2.length ∧
        (∀ (i : Nat) (e1 e2 : Expr),
          ta1.get? i = some e1 → ta2.get? i = some e2 → ExprEquiv e1 e2) ∧
        (∀ (i : Nat) (e1 e2 : Expr),
          ea1.get? i = some e1 → ea2.get? i = some e2 → ExprEquiv e1 e2)
    | _, _ => False

-- ══════════════════════════════════════════════════════════════════
-- Section 4: Function-level refinement
-- ══════════════════════════════════════════════════════════════════

/-- A function f_out refines f_in if, for all fuel bounds and initial
    environments, every non-stuck outcome of f_out is also an outcome of f_in.

    This is the top-level correctness condition for an optimization pass:
    the optimized program must not produce results that the original could
    not produce. It may, however, resolve nondeterminism (stuck → defined)
    or remove stuck states. -/
def FuncRefines (f_in f_out : Func) : Prop :=
  f_in.entry = f_out.entry ∧
  ∀ (fuel : Nat) (ρ : Env) (lbl : Label) (result : Outcome),
    execFunc f_out fuel ρ lbl = some result →
    result ≠ .stuck →
    ∃ fuel' : Nat, execFunc f_in fuel' ρ lbl = some result

/-- Semantic function equivalence: mutual refinement with matching entry points. -/
def FuncEquiv (f1 f2 : Func) : Prop :=
  f1.entry = f2.entry ∧
  ∀ (fuel : Nat) (ρ : Env) (lbl : Label),
    execFunc f1 fuel ρ lbl = execFunc f2 fuel ρ lbl

/-- Function equivalence implies function refinement. -/
theorem funcEquiv_implies_refines (f1 f2 : Func) (h : FuncEquiv f1 f2) :
    FuncRefines f1 f2 := by
  constructor
  · exact h.1
  · intro fuel ρ lbl result hout _
    exact ⟨fuel, by rw [h.2]; exact hout⟩

-- ══════════════════════════════════════════════════════════════════
-- Section 5: Conditional refinement (under abstract environment)
-- ══════════════════════════════════════════════════════════════════

/-- Expression refinement conditioned on an abstract environment.
    The key insight from Alive2: we only need refinement to hold for inputs
    satisfying the precondition derived from abstract interpretation.

    This is strictly more useful than unconditional refinement — an optimization
    that is only valid under certain type/value constraints can be validated
    by providing the abstract environment as a witness. -/
def ExprRefinesUnder (σ : AbsEnv) (e_in e_out : Expr) : Prop :=
  ∀ (ρ : Env), AbsEnvSound σ ρ →
    ∀ (v : Value), evalExpr ρ e_out = some v → evalExpr ρ e_in = some v

/-- Conditional expression equivalence under abstract environment. -/
def ExprEquivUnder (σ : AbsEnv) (e1 e2 : Expr) : Prop :=
  ∀ (ρ : Env), AbsEnvSound σ ρ →
    evalExpr ρ e1 = evalExpr ρ e2

/-- Conditional equivalence implies conditional refinement. -/
theorem exprEquivUnder_implies_refinesUnder (σ : AbsEnv) (e1 e2 : Expr)
    (h : ExprEquivUnder σ e1 e2) :
    ExprRefinesUnder σ e1 e2 := by
  intro ρ hsound v hout
  rw [← h ρ hsound]; exact hout

/-- Unconditional equivalence implies conditional equivalence. -/
theorem exprEquiv_implies_equivUnder (σ : AbsEnv) (e1 e2 : Expr)
    (h : ExprEquiv e1 e2) :
    ExprEquivUnder σ e1 e2 :=
  fun ρ _ => h ρ

-- ══════════════════════════════════════════════════════════════════
-- Section 6: Transform validity — the validation judgment
-- ══════════════════════════════════════════════════════════════════

/-- A concrete transform (f_in ↦ f_out) is valid if f_out refines f_in.
    This is the top-level judgment that a translation validator must establish.

    Alive2-style: a validator examines one (f_in, f_out) pair and either
    confirms refinement or reports a counterexample. In our setting, the
    validator is a Lean function returning a proof or a `sorry`. -/
def ValidTransform (f_in f_out : Func) : Prop :=
  FuncRefines f_in f_out

/-- An expression-level transform is valid if it preserves semantics. -/
def ValidExprTransform (transform : Expr → Expr) : Prop :=
  ∀ (e : Expr), ExprEquiv e (transform e)

/-- A parameterized expression transform is valid under sound abstraction. -/
def ValidExprTransformAbs (transform : AbsEnv → Expr → Expr) : Prop :=
  ∀ (σ : AbsEnv) (e : Expr), ExprEquivUnder σ e (transform σ e)

/-- A function-level transform is valid if it preserves refinement. -/
def ValidFuncTransform (transform : Func → Func) : Prop :=
  ∀ (f : Func), FuncRefines f (transform f)

-- ══════════════════════════════════════════════════════════════════
-- Section 7: Connecting full proofs to translation validation
-- ══════════════════════════════════════════════════════════════════

/-- A semantics-preserving pass (from EndToEndProperties) yields a valid
    expression transform. This is the bridge: existing full proofs
    automatically satisfy translation validation. -/
theorem semanticsPreserving_implies_valid (pass : Expr → Expr)
    (h : ∀ (ρ : Env) (e : Expr), evalExpr ρ (pass e) = evalExpr ρ e) :
    ValidExprTransform pass :=
  fun e ρ => (h ρ e).symm

/-- Full proof of pass correctness implies validation succeeds for every
    concrete instance. This is the key composability theorem: we can mix
    fully-proved passes (which automatically validate) with translation-
    validated passes (which check specific instances). -/
theorem fullProof_validates_all (pass : Expr → Expr)
    (hproof : ValidExprTransform pass)
    (e : Expr) :
    ExprEquiv e (pass e) :=
  hproof e

-- ══════════════════════════════════════════════════════════════════
-- Section 8: Idempotency framework
-- ══════════════════════════════════════════════════════════════════

/-- Syntactic idempotency: applying the pass twice produces the same
    expression as applying it once. This is stronger than semantic
    idempotency and implies a fixed-point property. -/
def SyntacticIdempotent (pass : Expr → Expr) : Prop :=
  ∀ (e : Expr), pass (pass e) = pass e

/-- Semantic idempotency: applying the pass twice is semantically equivalent
    to applying it once. Weaker than syntactic but sufficient for correctness. -/
def SemanticIdempotent (pass : Expr → Expr) : Prop :=
  ∀ (e : Expr), ExprEquiv (pass (pass e)) (pass e)

/-- Syntactic idempotency implies semantic idempotency. -/
theorem syntacticIdempotent_implies_semantic (pass : Expr → Expr)
    (h : SyntacticIdempotent pass) :
    SemanticIdempotent pass :=
  fun e ρ => by rw [h e]

/-- Function-level syntactic idempotency. -/
def FuncSyntacticIdempotent (pass : Func → Func) : Prop :=
  ∀ (f : Func), pass (pass f) = pass f

/-- Function-level semantic idempotency: applying twice refines applying once
    and vice versa. -/
def FuncSemanticIdempotent (pass : Func → Func) : Prop :=
  ∀ (f : Func), FuncEquiv (pass (pass f)) (pass f)

-- ══════════════════════════════════════════════════════════════════
-- Section 9: Composition of validated transforms
-- ══════════════════════════════════════════════════════════════════

/-- Composing two valid expression transforms yields a valid transform. -/
theorem compose_valid_transforms (p1 p2 : Expr → Expr)
    (h1 : ValidExprTransform p1) (h2 : ValidExprTransform p2) :
    ValidExprTransform (p2 ∘ p1) := by
  intro e ρ
  simp [Function.comp]
  calc evalExpr ρ e
      = evalExpr ρ (p1 e) := h1 e ρ
    _ = evalExpr ρ (p2 (p1 e)) := h2 (p1 e) ρ

/-- Validation result: either a proof of validity or a counterexample witness. -/
inductive ValidationResult where
  | valid   : ValidationResult
  | invalid (witness_ρ : Env) (witness_v : Value) : ValidationResult

end MoltTIR
