# Proof Maintenance Guide

**Audience:** Future contributors to the Molt formal verification codebase.
**Prerequisite:** Basic familiarity with Lean 4 syntax and theorem proving.

---

## 1. Building and Verifying

### Running `lake build`

```bash
cd formal/lean
lake build
```

This type-checks every `.lean` file in the project. A successful build with no errors
means all proofs are valid and there are no sorry tactics.

### Interpreting Build Errors

| Error | Meaning | Fix |
|-------|---------|-----|
| `type mismatch` | A proof step produces the wrong type | Check the goal state; `simp` or `rw` may need different lemmas |
| `unsolved goals` | The proof is incomplete | Add more proof steps or check if the statement changed |
| `unknown identifier` | A name is not in scope | Check imports, namespaces, or spelling |
| `declaration uses 'sorry'` | A proof contains `sorry` (warning, not error) | Replace with a real proof |
| `deep recursion` | A tactic or definition is looping | Add `termination_by` or restructure the recursion |
| `deterministic timeout` | Proof search exceeded heartbeat limit | Add `set_option maxHeartbeats 400000` or simplify the goal before heavy tactics |

### Checking Sorry Count

To verify zero sorrys, search for tactic-level `sorry` usage:

```bash
grep -rn '^  sorry' --include='*.lean' formal/lean/ | grep -v '/--' | grep -v '-- '
```

The word "sorry" appears in comments and documentation (e.g., `SorryAudit.lean`).
These are not tactic invocations and do not affect soundness.

### Checking Axiom Count

```bash
grep -rn '^axiom ' --include='*.lean' formal/lean/ | wc -l
```

The current baseline is **68 axioms** across 6 files. See `AXIOM_INVENTORY.md` for the full list.

---

## 2. Adding a New Compiler Pass

When adding a new optimization pass (e.g., strength reduction):

### Step 1: Define the pass

Create `MoltTIR/Passes/StrengthReduce.lean`:

```lean
import MoltTIR.Syntax
import MoltTIR.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltTIR.Passes

/-- Replace `x * 2` with `x + x`. -/
def strengthReduceExpr (e : Expr) : Expr :=
  match e with
  | .binOp .mul lhs (.val (.int 2)) => .binOp .add lhs lhs
  | other => other

def strengthReduceInstr (i : Instr) : Instr :=
  { i with rhs := strengthReduceExpr i.rhs }

def strengthReduceBlock (b : Block) : Block :=
  { b with instrs := b.instrs.map strengthReduceInstr }

end MoltTIR.Passes
```

### Step 2: Prove correctness

Create `MoltTIR/Passes/StrengthReduceCorrect.lean`:

```lean
import MoltTIR.Passes.StrengthReduce
import MoltTIR.Semantics.EvalExpr

set_option autoImplicit false

namespace MoltTIR.Passes

theorem strengthReduceExpr_correct (ρ : Env) (e : Expr) (v : Value)
    (heval : evalExpr ρ e = some v) :
    evalExpr ρ (strengthReduceExpr e) = some v := by
  unfold strengthReduceExpr
  match e with
  | .binOp .mul lhs (.val (.int 2)) =>
    -- Show that x * 2 = x + x for the Molt value domain
    sorry  -- Replace with actual proof
  | _ => exact heval

end MoltTIR.Passes
```

### Step 3: Add to the pipeline

- Register the pass in `MoltTIR/Passes/Pipeline.lean`
- Add the pass simulation in `MoltTIR/Simulation/PassSimulation.lean`
- Prove SSA preservation in `MoltTIR/SSA/PassPreservesSSA.lean` (if the pass modifies
  instruction structure)

### Step 4: Add to lakefile

Add the new modules to `lakefile.lean` in the appropriate `lean_lib` section.

---

## 3. Adding a New Intrinsic and Its Axiom

When the compiler gains support for a new Python builtin (e.g., `chr`):

### Step 1: Declare the opaque function

In `MoltTIR/Runtime/IntrinsicContracts.lean`, add:

```lean
-- chr -------------------------------------------------------------

/-- `chr(n)` converts an integer to its Unicode character. -/
opaque intrinsic_chr : Int → String
```

### Step 2: Add axioms for the key properties

```lean
/-- `chr(n)` produces a single-character string for valid codepoints. -/
axiom chr_length : ∀ (n : Int), 0 ≤ n → n ≤ 0x10FFFF →
    (intrinsic_chr n).length = 1

/-- `chr(ord(c)) = c` for single characters. -/
axiom chr_ord_roundtrip : ∀ (c : String), c.length = 1 →
    intrinsic_chr (intrinsic_ord c) = c
```

### Step 3: Update the axiom inventory

Add the new axioms to `AXIOM_INVENTORY.md` in the appropriate category. Update the
total count.

### Step 4: Update the sorry baseline

If the new axioms introduced temporary sorrys in downstream proofs, track them in
`CERTIFICATION_STATUS.md` until they are closed.

---

## 4. Updating the Sorry Baseline

The sorry baseline is tracked in two places:

1. `docs/spec/areas/formal/CERTIFICATION_STATUS.md` — the authoritative status document
2. `formal/lean/BACKEND_PROOF_STATUS.md` — backend-specific status

When closing a sorry:

1. Remove the `sorry` tactic and replace with a real proof
2. Run `lake build` to verify the proof type-checks
3. Update both status documents
4. If the sorry was converted to an axiom instead of a proof, add it to
   `AXIOM_INVENTORY.md`

When adding a sorry (temporary, during development):

1. Add a TODO comment explaining what blocks the proof:
   ```lean
   sorry
   -- TODO(formal, owner:yourname, priority:P2, status:blocked):
   -- Requires formalizing the XYZ invariant from the frontend.
   ```
2. Add an entry to `CERTIFICATION_STATUS.md` in the "What is NOT Proven" section
3. Do not merge to main with new sorrys unless explicitly approved

---

## 5. Style Guide

### General Rules

- **`set_option autoImplicit false`** at the top of every file. No exceptions. This
  prevents Lean from silently introducing universe variables or type variables.

- **No Mathlib dependency.** The codebase is self-contained. If you need a Mathlib
  lemma, either prove it locally or find an alternative approach.

- **Section headers** use the following format:
  ```lean
  -- ══════════════════════════════════════════════════════════════════
  -- Section N: Title
  -- ══════════════════════════════════════════════════════════════════
  ```

- **Docstrings** on all public definitions and theorems:
  ```lean
  /-- One-line summary of what this theorem states. -/
  theorem my_theorem ...
  ```

### Naming Conventions

| Pattern | Example | Use for |
|---------|---------|---------|
| `pass_correct` | `constFold_correct` | Main correctness theorem for a pass |
| `pass_preserves_X` | `dce_preserves_ssa` | Pass preserves a structural property |
| `X_sound` | `absEvalExpr_sound` | Soundness of an abstract interpretation |
| `X_deterministic` | `evalExpr_deterministic` | Determinism property |
| `X_total` | `str_total` | Totality property |
| `X_nonneg` | `len_nonneg` | Non-negativity bound |

### Preferred Tactics

| Tactic | Use for |
|--------|---------|
| `simp` | Simplification with the default simp set |
| `simp only [...]` | Controlled simplification (preferred over bare `simp`) |
| `rfl` | Definitional equality |
| `rw [...]` | Rewriting with specific lemmas |
| `cases` / `match` | Case analysis on inductive types |
| `induction` | Structural induction |
| `omega` | Linear arithmetic over integers/naturals |
| `native_decide` | Decidable propositions (use sparingly; slow) |
| `exact` | Direct proof term |
| `bv_decide` | BitVec/Bool goals (requires Lean >= 4.17) |

### What to Avoid

- **`decide`** on large types (prefer `native_decide`)
- **`aesop`** (not available without Mathlib)
- **`norm_num`** (not available without Mathlib)
- **Recursive functions without `termination_by`** (causes `deep recursion`)
- **`sorry` on main branch** (the baseline is 0)

---

## 6. Axiom Policy

### When to Use `axiom`

Use `axiom` when the property is **true but unprovable within Lean**:

- **Hardware properties** (IEEE 754 conformance, cache behavior)
- **External toolchain properties** (Cranelift determinism, linker behavior)
- **Runtime behavior** (Python builtin semantics modeled as opaque functions)
- **Compiler invariants validated by other means** (SSA verifier, type checker)

Every axiom must have:
1. A docstring explaining what it asserts and why it is sound
2. An entry in `AXIOM_INVENTORY.md`
3. A validation mechanism (differential tests, runtime verifier, etc.)

### When to Use `sorry`

Use `sorry` **only during development**, when:
- The proof is in progress and will be completed before merge
- A blocking dependency needs to be resolved first

Every sorry must have:
1. A TODO comment with owner, priority, and blocking reason
2. An entry in `CERTIFICATION_STATUS.md`

**The main branch sorry baseline is 0. Do not merge new sorrys.**

### When to Write a Full Proof

The default. If a property is expressible and provable in Lean, prove it. Prefer
proof over axiom. Prefer axiom over sorry.

---

## 7. Trust Boundary Architecture

The Molt correctness argument spans three verification layers:

```
┌─────────────────────────────────────────────────────────┐
│                    Lean 4 Proofs                        │
│  ~1,331 theorems, 0 sorrys, 68 trust axioms            │
│  Covers: IR semantics, pass correctness, backend        │
│          equivalence, determinism, type safety           │
├─────────────────────────────────────────────────────────┤
│                    Rust Kani Proofs                      │
│  Covers: NaN-boxing bit layout, memory safety,          │
│          overflow checks, pointer validity               │
│  Validates: The Lean model's assumptions about the      │
│             runtime's bit-level behavior                 │
├─────────────────────────────────────────────────────────┤
│              Python Differential Tests                   │
│  ~3,500 test cases                                      │
│  Covers: End-to-end semantics preservation,             │
│          cross-backend output equivalence,               │
│          builtin behavior (validates intrinsic axioms)   │
└─────────────────────────────────────────────────────────┘
```

### How the Layers Connect

1. **Lean axioms about intrinsics** (e.g., `len_nonneg`) are validated by the Python
   differential test suite, which runs every builtin on thousands of inputs and
   verifies the axiom's stated property holds.

2. **Lean axioms about hardware** (e.g., `ieee754_basic_ops_deterministic`) are
   validated by cross-platform CI: the same test suite runs on x86-64, ARM64, and
   WASM, verifying identical float behavior.

3. **Lean axioms about the compiler** (e.g., `ssa_of_wellformed_tir`) are validated by
   the Rust SSA verifier that runs on every TIR function at compile time.

4. **Rust Kani proofs** validate bit-level properties (NaN-boxing, pointer tagging)
   that the Lean model abstracts over. The Lean proofs assume `IsInt bits` implies
   certain bit patterns; Kani exhaustively checks these patterns.

5. **Python differential tests** are the ultimate end-to-end check: compile the same
   Python program to all backends, run it, and verify identical output. This validates
   the entire chain from source to executable.

### Adding to the Trust Boundary

If you add a new axiom, ask:
- Is there a Kani proof that validates the underlying bit-level assumption?
- Is there a differential test that validates the semantic assumption?
- Is there a runtime verifier check that validates the structural assumption?

If none of these exist, create one before merging the axiom.
