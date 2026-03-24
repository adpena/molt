# Project TITAN Phase 2: Optimization Core (TIR Passes)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement TIR optimization passes that transform typed IR for maximum performance — unboxing, escape analysis, dead code elimination, loop optimization, and iterator fusion.

**Architecture:** Each pass operates on `TirFunction` in-place, transforming ops/types/blocks. Passes run in order after `lower_to_tir()` and before `lower_to_simple_ir()`. Every pass is verified by the TIR verifier after execution. Every pass must preserve CPython parity.

**Tech Stack:** Rust (molt-backend TIR module)

**Spec Reference:** `docs/superpowers/specs/2026-03-23-project-titan-optimization-design.md` — Section 4.5 (Passes 2-17), Section 7

**Prerequisites:** Phase 1 Track A complete (TIR pipeline: lower_from_simple → type_refine → lower_to_simple)

---

## Pass Pipeline (execution order)

The spec defines 17 passes. Phase 2 implements the highest-impact subset that operates on the existing TIR infrastructure. Each pass is a function `fn run(func: &mut TirFunction) -> PassStats`.

| Priority | Pass | Spec Ref | Impact | Complexity |
|----------|------|----------|--------|------------|
| P0 | Unboxing | Pass 2 | Very High | Medium |
| P0 | Escape Analysis | Pass 3 | Very High | High |
| P0 | Dead Code Elimination | Pass 17 | High | Low |
| P1 | SCCP (Constant Propagation) | Pass 5 | High | Medium |
| P1 | Strength Reduction | Pass 16 | Medium | Low |
| P1 | Bounds Check Elimination | Pass 8 | High | Medium |
| P2 | Refcount Elimination | Pass 13 | High | Medium |
| P2 | Deforestation / Iterator Fusion | Pass 9 | Very High | High |
| P3 | Monomorphization | Pass 12 | Very High | Very High |
| P3 | Closure/Lambda Specialization | Pass 10 | High | High |

Phase 2 implements P0 and P1 passes (6 passes). P2 and P3 are deferred to Phase 3 integration.

---

## Task D1: Unboxing Pass

**File:** `runtime/molt-backend/src/tir/passes/unboxing.rs`

The highest-value optimization. Eliminates NaN-boxing overhead for values with known types.

- [ ] **Step 1: Create passes module structure**

```bash
mkdir -p runtime/molt-backend/src/tir/passes
```

Create `runtime/molt-backend/src/tir/passes/mod.rs`:
```rust
pub mod unboxing;
pub mod escape_analysis;
pub mod dce;
pub mod sccp;
pub mod strength_reduction;
pub mod bce;

/// Statistics returned by each optimization pass.
#[derive(Debug, Default)]
pub struct PassStats {
    pub name: &'static str,
    pub values_changed: usize,
    pub ops_removed: usize,
    pub ops_added: usize,
}
```

Register in `runtime/molt-backend/src/tir/mod.rs`: `pub mod passes;`

- [ ] **Step 2: Implement unboxing pass**

```rust
/// Unboxing: eliminate Box/Unbox pairs when all consumers of a boxed value
/// eventually unbox to the same type.
///
/// Pattern: %boxed = molt.box %val : i64 → dynbox
///          %unboxed = molt.unbox %boxed : dynbox → i64
///          use %unboxed
/// After:   use %val (box and unbox eliminated)
///
/// Only fires when ALL uses of %boxed are molt.unbox to the same type.
pub fn run(func: &mut TirFunction) -> PassStats
```

Algorithm:
1. For each `BoxVal` op, find all uses of its result ValueId
2. If ALL uses are `UnboxVal` to the same type, replace all uses of the unboxed ValueIds with the original pre-box ValueId
3. Mark the BoxVal and all UnboxVal ops as dead (remove in DCE)

- [ ] **Step 3: Tests**

- Box/unbox pair eliminated when only consumer is unbox
- Box kept when some consumers use it as DynBox
- Multiple unbox consumers all eliminated
- Box with no consumers → eliminated (dead code)

- [ ] **Step 4: Verify and commit**

`cargo test -p molt-backend --lib -- tir::passes::unboxing`
Commit: `git commit -m "feat(titan-p2): add unboxing pass (eliminate box/unbox pairs)"`

---

## Task D2: Escape Analysis

**File:** `runtime/molt-backend/src/tir/passes/escape_analysis.rs`

Determines which allocations can be stack-allocated (NoEscape) vs must remain on heap (GlobalEscape).

- [ ] **Step 1: Implement escape analysis**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeState {
    NoEscape,      // never leaves function → stack allocate
    ArgEscape,     // passed to callee but not stored → stack + callee lifetime
    GlobalEscape,  // stored in heap/global → must heap allocate
}

/// Analyze escape state of all Alloc operations in a function.
/// Returns a map from the Alloc result ValueId to its escape state.
pub fn analyze_escapes(func: &TirFunction) -> HashMap<ValueId, EscapeState>
```

Algorithm:
1. Find all `Alloc` / `StackAlloc` ops → start as `NoEscape`
2. For each use of an allocated value:
   - Stored to a field of another object → `GlobalEscape`
   - Returned from function → `GlobalEscape`
   - Passed to unknown/external function → `GlobalEscape`
   - Passed to known callee that doesn't store it → `ArgEscape`
   - Used only for reads, local operations → stays `NoEscape`
3. Propagate: if value A escapes and value B is stored into A, then B escapes too

Conservative default: anything not provably NoEscape is GlobalEscape.

- [ ] **Step 2: Rewrite pass — convert NoEscape Alloc to StackAlloc**

```rust
/// Rewrite Alloc ops to StackAlloc when escape analysis proves NoEscape.
/// Also removes corresponding IncRef/DecRef ops on stack-allocated values.
pub fn apply_escape_results(func: &mut TirFunction, escapes: &HashMap<ValueId, EscapeState>) -> PassStats
```

- [ ] **Step 3: Tests**

- Local-only allocation → NoEscape
- Returned value → GlobalEscape
- Stored into list → GlobalEscape
- Passed to known pure function → ArgEscape
- Transitive: stored into NoEscape container → stays NoEscape

- [ ] **Step 4: Verify and commit**

Commit: `git commit -m "feat(titan-p2): add escape analysis pass (NoEscape/ArgEscape/GlobalEscape)"`

---

## Task D3: Dead Code Elimination

**File:** `runtime/molt-backend/src/tir/passes/dce.rs`

Remove ops whose results are never used.

- [ ] **Step 1: Implement DCE**

```rust
/// Remove ops whose results are never used by any other op or terminator.
/// Preserves ops with side effects (Call, StoreAttr, StoreIndex, Raise, IncRef, DecRef, Free).
pub fn run(func: &mut TirFunction) -> PassStats
```

Algorithm:
1. Build use-count map: for each ValueId, count how many times it appears as an operand
2. Walk ops in reverse order per block
3. If an op's results all have use-count 0 AND the op has no side effects → remove it
4. Decrement use-counts of the removed op's operands (may enable more removals)
5. Iterate until no more removals

Side-effecting ops (never remove): Call, CallMethod, CallBuiltin, StoreAttr, StoreIndex, DelAttr, DelIndex, Raise, Yield, YieldFrom, IncRef, DecRef, Free, Alloc (may have finalizer).

- [ ] **Step 2: Tests**

- Unused constant → removed
- Unused arithmetic → removed
- Used value → kept
- Side-effecting call with unused result → kept
- Chain: A used by B, B unused → both removed

- [ ] **Step 3: Verify and commit**

Commit: `git commit -m "feat(titan-p2): add dead code elimination pass"`

---

## Task D4: SCCP (Sparse Conditional Constant Propagation)

**File:** `runtime/molt-backend/src/tir/passes/sccp.rs`

Propagate constants through the SSA graph and eliminate dead branches.

- [ ] **Step 1: Implement SCCP**

```rust
/// Sparse Conditional Constant Propagation on typed SSA.
/// Folds operations with constant operands and eliminates unreachable branches.
pub fn run(func: &mut TirFunction) -> PassStats
```

Algorithm (standard SCCP):
1. Lattice values: `Top` (unknown), `Constant(i64/f64/bool)`, `Bottom` (overdefined)
2. Initialize: constants → Constant, everything else → Top
3. Worklist: process ops in RPO, propagate constants forward
4. For CondBranch: if condition is constant, only one successor is executable
5. For arithmetic: if both operands are Constant, fold to Constant
6. Iterate until worklist empty
7. Replace Constant-valued ops with ConstInt/ConstFloat/ConstBool
8. Replace CondBranch with known-constant condition with Branch to the taken side

- [ ] **Step 2: Tests**

- Constant arithmetic: `1 + 2` → `3`
- Constant comparison: `5 > 3` → `true`
- Dead branch elimination: `if true: A else: B` → only A
- Non-constant operand: stays unchanged
- Constant through block argument: propagates across blocks

- [ ] **Step 3: Verify and commit**

Commit: `git commit -m "feat(titan-p2): add SCCP pass (constant propagation + dead branch elimination)"`

---

## Task D5: Strength Reduction

**File:** `runtime/molt-backend/src/tir/passes/strength_reduction.rs`

Replace expensive operations with cheaper equivalents.

- [ ] **Step 1: Implement strength reduction**

```rust
/// Replace expensive operations with cheaper equivalents.
pub fn run(func: &mut TirFunction) -> PassStats
```

Rewrite rules (I64 operands only):
- `x ** 2` → `x * x`
- `x * 2` → `x + x`
- `x * power_of_2` → `x << k` (where k = log2)
- `x // power_of_2` → `x >> k` (for non-negative x, checked via type)
- `x % power_of_2` → `x & (power_of_2 - 1)` (for non-negative x)

Check the `attrs` dict for constant values in the second operand.

- [ ] **Step 2: Tests**

- Each rewrite rule fires correctly
- Non-power-of-2 is not rewritten
- F64 operands are not rewritten (FP semantics differ)
- DynBox operands are not rewritten

- [ ] **Step 3: Verify and commit**

Commit: `git commit -m "feat(titan-p2): add strength reduction pass"`

---

## Task D6: Bounds Check Elimination

**File:** `runtime/molt-backend/src/tir/passes/bce.rs`

Remove redundant bounds checks on list/tuple indexing.

- [ ] **Step 1: Implement BCE**

```rust
/// Eliminate bounds checks that are provably safe.
pub fn run(func: &mut TirFunction) -> PassStats
```

Patterns to detect:
1. **Range-based:** `for i in range(len(lst)): lst[i]` — i is always in bounds
   - Detect: loop induction variable, upper bound = len(container), index = induction var
   - Action: mark the Index op as "bounds-check-eliminated"

2. **Dominator-based:** if `lst[i+1]` succeeded earlier in a dominating block, then `lst[i]` is safe
   - Detect: successful index at larger offset dominates current index
   - Action: mark current Index as safe

3. **Constant index:** `lst[0]` when len is known > 0
   - Detect: constant index, known-nonempty container
   - Action: mark as safe

For Phase 2, implement pattern 1 (range-based) only — it covers the most common case.

- [ ] **Step 2: Tests**

- Range(len) pattern → bounds check eliminated
- Unknown upper bound → bounds check kept
- Negative index → bounds check kept (could be valid but can't prove)
- Constant index on known-length → eliminated

- [ ] **Step 3: Verify and commit**

Commit: `git commit -m "feat(titan-p2): add bounds check elimination pass (range-based)"`

---

## Task D7: Pass Pipeline Integration

**File:** Modify `runtime/molt-backend/src/tir/passes/mod.rs`

Wire all passes into a single pipeline function.

- [ ] **Step 1: Implement pipeline runner**

```rust
/// Run the full TIR optimization pipeline on a function.
/// Passes run in the order specified by the spec.
/// Each pass is verified by the TIR verifier after execution.
/// Returns aggregate stats.
pub fn run_optimization_pipeline(func: &mut TirFunction) -> Vec<PassStats> {
    let mut all_stats = Vec::new();

    // Pass order matters:
    // 1. Type refinement (already done in lower_from_simple)
    // 2. Unboxing (needs types)
    // 3. Escape analysis (needs unboxed info)
    // 4. SCCP (can fold after unboxing reveals constants)
    // 5. Strength reduction (after SCCP reveals constant operands)
    // 6. BCE (after SCCP/SR simplify loop bounds)
    // 7. DCE (clean up after all other passes)

    all_stats.push(unboxing::run(func));
    all_stats.push(escape_analysis::run(func));
    all_stats.push(sccp::run(func));
    all_stats.push(strength_reduction::run(func));
    all_stats.push(bce::run(func));
    all_stats.push(dce::run(func));

    // Verify TIR invariants after all passes
    if let Err(errors) = super::verify::verify_function(func) {
        panic!("TIR verification failed after optimization: {:?}", errors);
    }

    all_stats
}
```

- [ ] **Step 2: Add TIR_OPT_STATS environment variable**

When `TIR_OPT_STATS=1`, print per-pass statistics to stderr.

- [ ] **Step 3: Integration test**

Create a test that:
1. Constructs a FunctionIR with known optimizable patterns
2. Runs `lower_to_tir()` → `run_optimization_pipeline()` → `lower_to_simple_ir()`
3. Verifies the output has fewer ops than the input
4. Verifies correctness (same semantics)

- [ ] **Step 4: Verify and commit**

Commit: `git commit -m "feat(titan-p2): wire TIR optimization pipeline with verification gate"`

---

## Constraints

- HashMap/HashSet only (no BTreeMap/BTreeSet)
- Every pass preserves TIR SSA invariants (verified after each pass)
- Every pass preserves CPython parity (no semantic changes)
- No O(N²) algorithms
- All unsafe blocks documented with safety invariants
- Tests for positive cases (optimization fires) AND negative cases (optimization correctly skips)
