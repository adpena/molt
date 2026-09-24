<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: S5 phase 2+: Memory SSA on the alias oracle, then E2 SROA + E6 MemGVN/store-forwarding/cross-block-DSE -->

# MemorySSA and Memory Optimization Blueprint

Historical architecture blueprint, not a current implementation or performance
scoreboard. MemorySSA and MemGVN now exist. Current field ownership and physical
alias contracts are maintained in [49 — Object field ownership](49_object_field_ownership.md)
and the source-bound tests; estimates below are not measured release evidence.

## 1. Precise Problem Statement

### Why this is load-bearing

The current memory pipeline has five hard ceilings that block the 5-year perf goals:

**LoadAttr is excluded from LICM (effects.rs `opcode_is_pure_movable`).** A shape-valid fixed-offset read can still run Python through dictionary or missing-field fallback. Only the exact-site access plan proves inline backing and a present value; guarded and unknown reads remain `GenericHeap`. Even a proven read is memory-dependent. Moving it requires invariant memory, valid receiver lifetime and execution/exception safety at the destination, not merely its original-site purity.

**GVN cannot deduplicate loads.** Two `LoadAttr` ops with the same object and offset in dominator-ordered blocks should produce the same value when no store intervenes. Without a memory-versioning layer between them, GVN has no way to prove they are equal.

**Dead-store elimination is single-block only** (`dead_store_elim.rs` line 50 documents this explicitly). The cross-block kill of a store in block A by a store in block B that dominates all reads is structurally impossible without memory phi placement.

**SROA requires complete allocation and effect proof.** The current pass can
remove callback-free raw allocations with wholly unobserved boxed-neutral uses.
Known field offsets and nonescape do not make Python class construction or
destruction removable. The unproved class-frame opcode has been retired. Broader
class scalar replacement must preserve its class, callback and owned-edge
semantics as well as reaching-memory definitions; historical benchmark ratios
are not current evidence.

**LICM cannot hoist field reads out of loops.** `for i in range(n): x = obj.field * i` — `obj.field` never changes, but LICM today cannot hoist the load because it is not `pure_movable`.

Together these gaps cost at least 3-5x on struct-heavy workloads (`bench_struct`) and prevent any loop that touches object fields from reaching the CPython performance floor.

### Quantified stakes

- `bench_struct`: 1M iterations × (2 proven-neutral `store` writes eliminated by current DSE, 2 `store` remaining, 1 alloc, 2 typed-slot loads per iteration). After SROA these become register moves. The allocation disappears entirely. Estimated 10-30x improvement on this benchmark alone.
- Every Python program that reads an object field inside a loop (nearly all non-trivial programs) benefits from MemGVN (load deduplication) and LICM-of-loads.
- Cross-block DSE eliminates dead stores across basic block boundaries — the common case for any control flow that writes then overwrites a field.

---

## 2. Structurally Correct Design

### The end-state architecture

```
AliasAnalysis (S5 phase 1, LANDED)
    |
    v
MemorySSA analysis (S5 phase 2, THIS ARC)
    |
    +---> MemGVN pass (load dedup / store-to-load forwarding)
    |
    +---> Cross-block DSE pass
    |
    +---> LICM-of-loads (extend existing licm.rs)
    |
    v
SROA pass (S5 phase 3, THIS ARC)
    |
    +---> Dead SSA values → DCE → no allocation, no load/store
```

### MemorySSA: the key data structure

MemorySSA assigns a **memory version** to every point in the program where memory is defined or consumed. The memory SSA form has three node kinds:

```
MemoryDef(n)   — an op that writes to memory; produces a new memory version n
MemoryUse(n)   — a load that reads memory version n
MemoryPhi      — at join points, selects a memory version from predecessor edges
```

Each memory node's "defining memory access" is the single dominating MemoryDef (or MemoryPhi) whose write the node observes. This is the single-source-of-truth answer to "which store produced the value this load reads?" — the question that enables all four optimizations.

### Data structure design

```rust
// runtime/molt-passes/src/tir/passes/memory_ssa.rs

/// A memory access ordinal — unique per-function, allocated sequentially.
/// Version 0 is the "LiveOnEntry" def (all externally-visible memory before
/// the function's first op).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemVersion(pub u32);

pub const LIVE_ON_ENTRY: MemVersion = MemVersion(0);

/// A memory access in the MemorySSA graph.
#[derive(Debug, Clone)]
pub enum MemAccess {
    /// A defining write. The op at (block, op_idx) defines memory version `ver`,
    /// consuming the preceding definition `def_ver` (the memory it "flows
    /// through" — needed for cross-block kill queries).
    Def {
        ver: MemVersion,
        def_ver: MemVersion,  // immediate memory dominator's version
        block: BlockId,
        op_idx: usize,
        region: MemRegion,    // from AliasAnalysis::region_of
    },
    /// A memory use. The op at (block, op_idx) reads memory version `def_ver`
    /// — the most recent dominating MemoryDef/MemoryPhi whose region may alias
    /// the load's region.
    Use {
        def_ver: MemVersion,
        block: BlockId,
        op_idx: usize,
        region: MemRegion,
    },
    /// A Phi node placed at a join point where multiple distinct memory
    /// versions meet.
    Phi {
        ver: MemVersion,
        block: BlockId,
        /// (predecessor BlockId, incoming MemVersion) pairs.
        incoming: Vec<(BlockId, MemVersion)>,
        /// Conservative union of regions from all incoming defs.
        region: MemRegion,
    },
}

/// The complete MemorySSA result for one function.
#[derive(Debug, Clone)]
pub struct MemorySsaResult {
    /// All memory accesses, keyed by version (Defs and Phis have a version;
    /// Uses do not define a new version so they are keyed by a synthetic index).
    pub defs: HashMap<MemVersion, MemAccess>,
    /// Map from (block, op_idx) → MemVersion for each Def in that block.
    pub block_op_to_def: HashMap<(BlockId, usize), MemVersion>,
    /// Map from (block, op_idx) → MemVersion for each Use (the version it reads).
    pub block_op_to_use_def: HashMap<(BlockId, usize), MemVersion>,
    /// The memory Phi nodes placed per block.
    pub block_phis: HashMap<BlockId, MemVersion>,
    /// The "reaching def" at the END of each block (the MemVersion that exits
    /// the block). Used during construction and by the pass consumers.
    pub exit_def: HashMap<BlockId, MemVersion>,
    /// Next fresh version counter (needed when passes insert new Defs).
    pub next_version: u32,
}

impl MemorySsaResult {
    /// The memory version that reaches a USE at (block, op_idx): the most
    /// recent def dominating that use whose region may alias `region`.
    pub fn reaching_def_for_use(&self, block: BlockId, op_idx: usize) -> Option<MemVersion> {
        self.block_op_to_use_def.get(&(block, op_idx)).copied()
    }

    /// For a MemoryDef, return the single defining Def/Phi it reads from
    /// (the memory it "flows through" in the clobber graph).
    pub fn def_version_of(&self, ver: MemVersion) -> Option<MemVersion> {
        match self.defs.get(&ver)? {
            MemAccess::Def { def_ver, .. } => Some(*def_ver),
            MemAccess::Phi { .. } => Some(ver),
            MemAccess::Use { .. } => None,
        }
    }

    /// True if `store_ver` is the immediate (or optimized-away-trivial-phi)
    /// defining write for the load at (block, op_idx). Used for
    /// store-to-load forwarding.
    pub fn is_direct_def_of_use(
        &self,
        store_ver: MemVersion,
        load_block: BlockId,
        load_op_idx: usize,
    ) -> bool {
        self.block_op_to_use_def.get(&(load_block, load_op_idx))
            .copied()
            == Some(store_ver)
    }
}
```

### Construction algorithm

MemorySSA construction follows the standard SSA phi-placement algorithm, re-using the existing `dominators.rs` / `AnalysisManager` infrastructure:

**Phase A — MemoryDef placement.** Walk every block in RPO. For each op, call `AliasAnalysisResult::region_of` (already implemented in `alias_analysis.rs`). Callback-capable operations become `GenericHeap` MemoryDefs even when the affected object is absent from their operands. Ordinary `store` remains a `GenericHeap` def unless the shared typed-slot plan proves its exact site has a boxed-neutral old value; only that fact can discharge the destructor path. Ops with `region != ScalarRegister` and `load_purity == ProvenPure` become MemoryUses.

**Phase B — Phi placement.** Using the existing `dominators::compute_dominance_frontiers` (computable from the `ImmediateDoms` analysis in `AnalysisManager`), place MemoryPhis at every block in the dominance frontier of any block containing a MemoryDef. Iterate to a fixpoint (standard IDF algorithm — same machinery `ssa.rs` uses for value phis).

**Phase C — Renaming.** Walk the dominator tree (using `DomChildren` from `AnalysisManager`). Maintain a stack of the current "live" MemVersion. For each block arg at a MemoryPhi, push a fresh version. For each op: if it is a MemoryDef, record the version; if it is a MemoryUse, record the current live version as its reaching def. On dominator-tree exit from a block, restore the stack.

**Region-aware reaching-def.** A direct load has a physical `Field` extent with optional exact allocation identity. A proven release-neutral store to a disjoint extent does not kill the load. An ordinary replacing store without the shared exact-site fact remains a `GenericHeap` clobber, even at a different offset or under a different class view: its displaced owner can finalize and reenter. Class strings never establish physical disjointness.

### Soundness model

The soundness guarantee is: every transformation that observes a MemorySSA result must only assert "this load reads exactly the value written by store X" when `is_direct_def_of_use` returns true AND the store's region is disjoint from every possible intermediate MemoryDef in the control-flow path between store and load. Because the region-based reaching-def conservatively includes every aliasing store (MemRegion::may_alias is conservative — it returns true when in doubt), and because every heap-barrier op (`opcode_is_heap_barrier` in `alias_analysis.rs`) creates a MemoryDef against `GenericHeap` which may-alias everything, the analysis is fail-closed: a missed barrier only prevents an optimization, never enables a miscompile.

---

## 3. Component Design

### Files to create

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/memory_ssa.rs`**

Responsibilities:
- `MemVersion`, `MemAccess`, `MemorySsaResult` types
- `MemorySSA` marker type implementing `Analysis` (registered in `AnalysisId`)
- `compute(func)` — phases A/B/C above
- `MemorySsaResult::reaching_def_for_use`, `is_direct_def_of_use`, `def_version_of`
- `MemorySsaResult::store_result_for_use` — given a load's (block, op_idx), return the result `ValueId` of the dominating store if it is the single direct def (nil if multi-def/phi). This is the forwarded value.
- `MemorySsaResult::invalidate_op` — called by passes that insert/remove a Def or Use, returning a `PartialInvalidation` hint (which blocks need re-renaming vs. which are unaffected); used by MemGVN to update the result in-place after forwarding rather than triggering a full recompute.
- Unit tests covering: single-block store-then-load forwarding, cross-block store-then-load with phi, aliasing-load blocked by intervening GenericHeap def, physical field disambiguation for release-neutral stores, allocation identity with opaque callback barriers, nested loop with loop-invariant load.

Dependencies: `AliasAnalysis` (must be computed first), `ImmediateDoms`, `DomChildren`, `PredMap` (all available in `AnalysisManager`).

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mem_gvn.rs`**

Responsibilities:
- Store-to-load forwarding: for each `LoadAttr` with `load_purity == ProvenPure`, consult MemorySSA for the single reaching def. If the reaching def is a `store` with an exact-site callback-free proof from the shared typed-slot plan, at a statically-known offset that matches the load's region, and the stored `ValueId` is in scope (dominates the load), replace the load with a `Copy` of the stored value. An unproved replacing `store` is not a source: releasing the old value can reenter Python and change the slot before the store returns.
- Redundant-load elimination: for each `LoadAttr`, if MemorySSA shows the same `(object_root, offset)` was already loaded under the same reaching def version in a dominating block, replace the load with a `Copy` of the earlier load's result.
- Post-replacement: call `MemorySsaResult::invalidate_op` for removed loads (they are now Uses of nothing), then invalidate `AnalysisId::AliasAnalysis` and `AnalysisId::MemorySSA` (the copy prop and DCE passes that follow will clean up the `Copy` chains).
- Mutation class: `Mutates::OpsOnly` (no new blocks or edges).

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/sroa.rs`**

Responsibilities:
- Input: a fixed-size raw `Alloc` whose effects and complete surviving uses
  prove callback-free whole-object removal. Automatic class frame promotion
  is retired; nonescape alone is not immutable destruction.
- MemGVN first forwards eligible typed-slot loads. SROA removes only a complete
  allocation, its transparent aliases, boxed-neutral stores, and RC operations.
  A surviving observation or arbitrary-heap callback preserves the entire root.
- Class-bound allocation remains observable (class sealing and owned class
  release); it is not scalar-replaced.
- Mutation class: `Mutates::OpsOnly`; no later DCE is needed to repair ownership.

### Files to modify

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mod.rs`**

Add: `pub mod memory_ssa;`, `pub mod mem_gvn;`, `pub mod sroa;`

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/analysis/mod.rs`**

Add `MemorySSA` to `AnalysisId`:
```rust
pub enum AnalysisId {
    // ... existing 10 variants ...
    MemorySSA,   // S5 phase 2
}

pub const ALL: [AnalysisId; 11] = [/* existing 10 */ AnalysisId::MemorySSA];
```

Add registration in `cfg_sensitive` / `ops_sensitive` match arms. `MemorySSA` is both CFG-sensitive and ops-sensitive (a new store invalidates the reaching-def map).

Add `MemorySSA` marker type implementing `Analysis`:
```rust
pub struct MemorySSA;
impl Analysis for MemorySSA {
    type Result = MemorySsaResult;
    const ID: AnalysisId = AnalysisId::MemorySSA;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        // Delegates to memory_ssa::compute, which itself calls
        // AliasAnalysis::compute inline (Analysis::compute takes only &TirFunction).
        super::passes::memory_ssa::compute_standalone(func)
    }
}
```

NOTE: `compute_standalone` is a variant of the full computation that builds `AliasAnalysisResult` inline (since `Analysis::compute` takes `&TirFunction`, not `&mut AnalysisManager`). The fast path in passes uses `am.get::<AliasAnalysis>(func)` and `am.get::<MemorySSA>(func)` via `AnalysisManager::get`, which ensures AliasAnalysis is computed first (since it has a lower `AnalysisId` ordinal and `MemorySSA::compute` calls it).

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/pass_manager.rs`**

Add `mem_gvn` and `sroa` passes to `build_default_pipeline`. The updated pipeline (inserting into the Memory optimization phase, between `dead_store_elim` and `type_guard_hoist`):

```
// ── Memory optimization ──────────────────────────────────────────
"refcount_elim"      (OpsOnly)
"dead_store_elim"    (OpsOnly)   ← existing; already cross-block-safe within block
"mem_gvn"            (OpsOnly)   ← NEW: store-to-load forwarding + redundant-load elim
"sroa"               (OpsOnly)   ← NEW: field promotion on NoEscape objects
// ── Value optimization ───────────────────────────────────────────
"type_guard_hoist"   (Cfg)
...
```

Update the `default_pipeline_preserves_canonical_pass_order` test in `pass_manager.rs` to include the two new passes (the count goes from 25 to 27).

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/licm.rs`**

Extend `is_hoistable` to accept `ProvenPure` typed-slot loads when MemorySSA confirms the load's reaching def is loop-invariant (defined outside the loop body):

```rust
fn is_hoistable_with_mem(op: &TirOp, alias: &AliasAnalysisResult, mem: &MemorySsaResult,
                          loop_blocks: &HashSet<BlockId>, block: BlockId, op_idx: usize) -> bool {
    // Existing opcode-level check
    if super::effects::opcode_is_pure_movable(op.opcode) || op.is_plain_value_copy() {
        return true;
    }
    // NEW: ProvenPure LoadAttr is hoistable when its reaching def is loop-invariant
    if mem.slot_access.load_purity_at((block, op_idx), op) == LoadPurity::ProvenPure {
        if let Some(def_ver) = mem.reaching_def_for_use(block, op_idx) {
            // The reaching def must be outside the loop (block containing that def
            // is not in loop_blocks).
            if let Some(MemAccess::Def { block: def_block, .. }) = mem.defs.get(&def_ver) {
                return !loop_blocks.contains(def_block);
            }
        }
    }
    false
}
```

The `run` function signature changes from `(func, am)` to `(func, am)` unchanged externally — but internally it calls `am.get::<MemorySSA>(func)` after `am.get::<LoopForest>`. This is `Mutates::Cfg` (already correct).

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/dead_store_elim.rs`**

Add cross-block DSE as a second pass within `run`, after the existing single-block pass:

```rust
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    // Phase 1: existing single-block DSE (unchanged)
    let alias = am.get::<AliasAnalysis>(func).clone();
    let mut total_removed = 0usize;
    for block in func.blocks.values_mut() {
        total_removed += run_block(block, &alias);
    }

    // Phase 2: cross-block DSE via MemorySSA
    // A StoreAttr at ver V is dead iff:
    //   - Every path from V leads to another StoreAttr at the same (root, offset)
    //     before any MemoryUse or MemoryPhi that may observe V.
    //   - Concretely: in the MemorySSA clobber graph, V's only "downstream" user
    //     before reaching another def of (root, offset) is another Def (an overwrite).
    let mem = am.get::<MemorySSA>(func).clone();
    total_removed += run_cross_block_dse(func, &alias, &mem);

    PassStats { name: "dead_store_elim", values_changed: 0, ops_removed: total_removed, ops_added: 0 }
}
```

The cross-block DSE walk: iterate over all MemoryDefs. For each `StoreAttr` def V at region R, walk the MemorySSA use-def chain forward (following "which ops use version V?"). If all uses of V are other MemoryDefs with the same or narrower region (i.e., overwrites), and there are no MemoryUse nodes that read V, then V is dead. Remove it.

This is `Mutates::OpsOnly` (no CFG change). The `dead_store_elim` pass stays `OpsOnly` in the pipeline.

---

## 4. Soundness Argument

**MemorySSA construction is sound because:**

1. The generated `OpcodeEffects.may_access_arbitrary_heap` fact is checked before operand-root refinement. Callback-capable arithmetic/index/module reads, calls, yields, raises, and replacing stores therefore become `MemoryDef`s against `GenericHeap`, which may alias every heap region even when the captured object is absent from the operation's operands. A marked async-work `CheckException` poll is instance-refined to the same impure floor.

2. Physical `Field` regions carry a boxed-word extent and optional exact allocation identity. Unknown receivers may alias any allocation at overlapping offsets; `_class` is never a disjointness proof because inherited class views can name the same slot. Exact allocation identities derive from definition sites and transparent aliases, not may-alias CFG parameters. Every field and whole-allocation region aliases `GenericHeap`: identity and placement do not prove destruction/callback stability.

3. `ProvenPure` plain typed-slot `LoadAttr` ops become MemoryUses (not barriers). Guarded and generic `LoadAttr` forms become MemoryDefs with `GenericHeap` region because their fallback can dispatch `__getattr__` and mutate any field. `TirOp::plain_typed_slot_load` owns the exact direct-load shape. The shape-only store parser does not prove callback freedom. The shared typed-slot planner supplies exact-site pristine/boxed-neutral history; a surface initializer spelling cannot grant this fact.

4. Memory Phi placement uses the same IDF algorithm as value phis — an established correct algorithm. Phi placement is conservative (over-places, never under-places).

5. The reaching-def renaming walk is a standard dominator-tree algorithm that never aliases a use to a def it does not dominate. The fail-closed case is `Unknown` (no reaching def), which prevents any forwarding or hoisting.

**SROA is sound because:**

The precondition gate (single reaching def, no intervening `GenericHeap` def, `NoEscape | ArgEscape`) ensures:
- No aliasing write can clobber the field between the store and the load (no `GenericHeap` def intervenes)
- The object does not escape (no external code can read the field through a pointer we don't track)
- Every load reads exactly one store (no phi needed — the field's value is fully determined by a single SSA value)

These conditions are all derived from the conservative-superset alias oracle (S5 phase 1) and the fail-closed MemorySSA. A false positive on any precondition check prevents the SROA (costs a missed optimization), never enables a miscompile.

**Store-to-load forwarding is sound because:**

The forwarded value is the `ValueId` from the store's second operand. It dominates the load (since the store's block dominates the load's block — established by the MemorySSA dominator-tree walk). No use of a value before its definition is possible in valid SSA.

---

## 5. Legacy This Arc Deletes

- `dead_store_elim.rs` comment at line 50: "Cross-block dead stores are left live ... Cross-block elimination belongs in a full memory dataflow pass with alias facts." **This comment is deleted and replaced by the cross-block DSE implementation.**
- The `LICM` hard exclusion of `LoadAttr` from `is_hoistable` — replaced by the MemorySSA-gated hoistability check.
- Any future pressure to add ad-hoc "load is invariant" annotations on ops (a workaround that would create a second source of truth) — MemorySSA provides this structurally.

No existing code is deleted outright in this arc (the new passes are additive), but the conceptual ceiling is removed: the comment documenting the limit is replaced by the working implementation.

---

## 6. Test Plan

### Rust unit tests in `memory_ssa.rs`

**`single_block_store_then_load_has_direct_reaching_def`**
Build: `entry: StoreAttr(obj, val, offset=0); r = LoadAttr(obj, offset=0); Return r`.
Assert: `reaching_def_for_use(entry, 1) == Some(ver_of_store)`.

**`cross_block_store_then_load_through_linear_chain`**
Build: `bb0: StoreAttr(obj, val, 0) → bb1 → bb2: r = LoadAttr(obj, 0); Return r`.
Assert: load in bb2 reaches the def in bb0.

**`phi_placed_at_join_of_two_stores`**
Build: diamond where bb1 stores offset 0 and bb2 stores offset 0, both feeding into bb3 which loads offset 0.
Assert: `block_phis[bb3]` exists; load in bb3 reads the phi version.

**`generic_heap_def_kills_typed_field_load_reaching_def`**
Build: `bb0: StoreAttr(obj, val, 0) → bb1: Call(obj) → bb2: r = LoadAttr(obj, 0)`.
Assert: `reaching_def_for_use(bb2, 0)` is the version of the Call (GenericHeap def), NOT the store. The load cannot be forwarded.

**`distinct_offsets_have_independent_reaching_defs`**
Build: `StoreAttr(obj, v1, 0); StoreAttr(obj, v2, 8); r0 = LoadAttr(obj, 0); r8 = LoadAttr(obj, 8)`.
Assert: load at offset 0 reaches only the offset-0 store; load at offset 8 reaches only the offset-8 store.

**`local_allocation_preserves_generic_heap_barriers`**
Build: `obj = ObjectNewBound(cls); StoreAttr(obj, v, 0); Call(some_other_obj); r = LoadAttr(obj, 0)`.
Assert: `reaching_def_for_use` for the LoadAttr returns the Call's GenericHeap version, not the StoreAttr's version. Callback mutation and late destruction remain observable for local allocations.

**`loop_invariant_load_has_preheader_reaching_def`**
Build: preheader has `StoreAttr(obj, v, 0)`; loop body has `r = LoadAttr(obj, 0)` with no stores in the loop.
Assert: load's reaching def is in the preheader; `!loop_blocks.contains(def_block)` → LICM-hoistable.

### Rust unit tests in `sroa.rs`

**`bench_struct_pattern_sroa_eliminates_all_alloc_and_stores`**
Build: a raw fixed-size `Alloc(size=16)` with only boxed-neutral typed stores, whose loads MemGVN already forwarded.
After SROA: the complete allocation, aliases, stores, and RC disappear together. Class-bound allocation is not eligible: sealing and owned class release remain observable.

**`sroa_blocked_when_mem_phi_exists`**
Build: diamond where both arms store different values into offset 0, followed by a load in the join block.
Assert: SROA does NOT fire (phi in memory chain → single-def precondition fails).

**`sroa_blocked_when_generic_heap_def_intervenes`**
Build: `StoreAttr(obj, v, 0); Call(obj); r = LoadAttr(obj, 0)`.
Assert: SROA does NOT fire (Call creates GenericHeap def between store and load).

**`sroa_blocked_when_object_escapes`**
Build: `obj = ObjectNewBound; StoreAttr(obj, v, 0); Return obj`.
Assert: SROA does NOT fire (GlobalEscape).

### Differential test shapes (Python, `tests/differential/`)

**`tests/differential/basic/struct_field_forwarding.py`**
```python
class P:
    x: int
    y: int

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y


def f(n: int) -> int:
    p = P(3, 4)
    return p.x + p.y


assert f(0) == 7
```
Expected: `p.x` and `p.y` forwarded from the constructor stores; no `LoadAttr` in codegen.

**`tests/differential/basic/struct_loop_licm.py`**
```python
class P:
    x: int

    def __init__(self, x: int = 0) -> None:
        self.x = x


def f(n: int) -> int:
    p = P(42)
    acc = 0
    for i in range(n):
        acc += p.x  # p.x is loop-invariant
    return acc


assert f(1000) == 42000
```
Expected: `p.x` load hoisted out of the loop by LICM-of-loads.

**`tests/differential/basic/struct_sroa.py`** (the bench_struct pattern)
```python
class Point:
    x: int
    y: int

    def __init__(self, x: int = 0, y: int = 0) -> None:
        self.x = x
        self.y = y


def main() -> int:
    total = 0
    for i in range(10):
        p = Point(0, 0)
        p.x = i
        p.y = i + 1
        total += p.x + p.y
    return total


assert main() == 100
```
Expected: after SROA + DCE, the `Point` allocation is eliminated; `p.x` and `p.y` are SSA values.

**`tests/differential/basic/struct_cross_block_dse.py`**
```python
class S:
    v: int

    def __init__(self, v: int = 0) -> None:
        self.v = v


def f(cond: bool, n: int) -> int:
    s = S(0)
    if cond:
        s.v = 1
    else:
        s.v = 2
    s.v = n  # overwrites both branches — the if/else stores are dead
    return s.v


assert f(True, 5) == 5
assert f(False, 5) == 5
```
Expected: cross-block DSE removes the stores in both branches.

**Adversarial / exception cases:**

```python
# struct_with_try_block.py — must NOT forward/sroa across exception boundaries
class C:
    x: int

    def __init__(self, x=0):
        self.x = x


def f():
    c = C(1)
    try:
        c.x = 2  # store
        risky()  # may raise — c.x must NOT be forwarded past this
    except:
        pass
    return c.x  # must read 1 if risky() raised before store
```
(Ensure: GenericHeap def from `risky()` call blocks forwarding. Result must be `c.x` = 2 or 1 depending on whether exception raised. CPython-correct.)

**BigInt safety:**
```python
class N:
    v: int

    def __init__(self, v=0):
        self.v = v


def f(x: int) -> int:
    n = N(x)
    return n.v


assert f(1 << 60) == 1 << 60  # must stay BigInt-correct after forwarding
```
Expected: `n.v` forwarded to `x`, which is `MaybeBigInt`. The forwarded `Copy(x)` carries the `MaybeBigInt` repr — no trusted-unbox introduced.

**Cross-backend: all 4 backends must produce identical results on all of the above.**

---

## 7. Perf-Gate Plan

### Benchmarks

| Benchmark | Expected delta | How measured |
|-----------|---------------|--------------|
| `bench_struct.py` (bench_struct pattern) | 10-30x vs pre-SROA (closes the 0.04x cliff documented in memory.md) | `python3 -m molt build --target native --output /tmp/bench_struct_out tests/benchmarks/bench_struct.py --rebuild && python3 tools/safe_run.py --rss-mb 512 --timeout 30 -- /tmp/bench_struct_out` vs `python3 tests/benchmarks/bench_struct.py` |
| Loop with field load (`bench_field_licm`, new) | 2-5x (eliminates per-iteration `LoadAttr` call) | Same pattern |
| General stdlib programs | No regression | Full differential suite: `python3 tests/molt_diff.py basic stdlib` |

### Perf contract verification

The contract is: molt MUST be faster than CPython on bench_struct after this arc lands. Concretely:
- Pre-arc: bench_struct runs at ~0.04x CPython speed (the documented cliff)
- Post-arc target: >= 1.0x CPython speed on bench_struct native release-fast
- WASM target: >= 0.8x CPython (acceptable WASM overhead)
- LLVM: >= 1.0x CPython

Every benchmark is run on all 3 backends (native Cranelift, WASM, LLVM) in release-fast profile. `MOLT_SESSION_ID` must be set before each build.

---

## 8. Risk, Rollback, and Dependencies

### Blocked by

- S5 phase 1 (`AliasAnalysis`) — **LANDED** at `fb574b289`. The `MemRegion`, `LoadPurity`, `AliasUnionFind`, `AliasAnalysisResult`, and all barrier queries are in place.
- S1 (`AnalysisManager`, dominance analyses) — **LANDED** at `ef284d182`. `ImmediateDoms`, `DomChildren`, `PredMap` are all available via `am.get::<...>()`.

### Unblocks

- **E2 full generality**: This blueprint IS E2 (SROA). After landing, the `bench_struct` cliff is closed.
- **E6 MemGVN**: Partially included here (store-to-load forwarding + redundant-load elim). The remaining E6 work (LICM-of-loads is included here; MemGVN cross-function promotion is a separate arc).
- **Loop-IV accumulator perf**: MemorySSA enables LICM of pure loads, which unblocks recognizing loop-invariant field reads as part of the induction-variable analysis.
- **E1 inliner activation**: When the inliner splices a callee into the caller, MemorySSA must be re-run on the merged function. This is handled naturally by `AnalysisId::MemorySSA`'s `OPS_SENSITIVE = true` flag, which drops the cached result after every `OpsOnly` mutation (the inliner is `Mutates::OpsOnly` for the splice body; the module-phase driver uses `Mutates::Cfg` which drops everything).

### Risks

1. **MemorySSA construction correctness.** The IDF algorithm and renaming walk have subtle corner cases (unreachable blocks, self-loops, critical edges). Mitigation: the existing `verify::verify_function` catch structural SSA violations; additionally, add a `MOLT_VERIFY_MEMORY_SSA=1` debug mode (analogous to `MOLT_VERIFY_ANALYSIS`) that recomputes from scratch and asserts equality after each pass.

2. **Raw allocation with a phi in its memory chain.** Whole-allocation removal
   must prove every reaching store and owner across the join. The shared typed
   slot planner may remove neutral overwrites before SROA; it cannot erase a
   terminal store or a callback-capable class allocation to force eligibility.

3. **Cross-backend parity on forwarded values.** Store-to-load forwarding introduces `Copy` ops that must round-trip through `lower_to_simple` and back. The `Copy` with no `_original_kind` is the pure SSA-move case, which all backends already handle correctly (confirmed by `copy_is_known_local_alias` in `alias_analysis.rs`). Risk: WASM `emit_get_boxed_for_repr` or LIR repr inference may assign a different repr to the forwarded value than to the original load. Mitigation: the forwarded `Copy` inherits the `ValueId` type from the store's value operand, which carries the correct repr from `representation_plan.rs`; this is the same path that `unboxing.rs` already exercises.

4. **LICM-of-loads needs destination admission.** Original-site purity is necessary but insufficient for hoisting, especially across zero-iteration or exception paths. The receiver must be alive, its backing and present slot established at the destination, and memory invariant. The illustrative hook above is not this complete proof; current LICM excludes field loads. Missing/dictionary fallback cannot be suppressed by an opcode or offset classifier.

### Rollback

Since MemorySSA, MemGVN, and SROA are all additive passes (they do not modify existing pass behavior), rollback is: remove the three new passes from `build_default_pipeline` and revert the two `AnalysisId` additions. No existing pass logic is changed except the LICM `is_hoistable` extension, which is a conservative guard (it can only hoist MORE than before, never hoist something incorrect).

---

## 9. Phased Landing Sequence

Each phase is a COMPLETE structural piece, verifiable in isolation.

### Phase 2a: MemorySSA analysis (standalone, no consumers)

**Checklist:**
- [ ] Create `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/memory_ssa.rs` with `MemVersion`, `MemAccess`, `MemorySsaResult`, `compute_standalone`
- [ ] Add `MemorySSA` to `AnalysisId::ALL` in `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/analysis/mod.rs` (add variant, add to `cfg_sensitive`/`ops_sensitive` match arms, add `MemorySSA` impl struct, add to `assert_analyses_fresh` macro in pass_manager.rs)
- [ ] Add `pub mod memory_ssa;` to `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mod.rs`
- [ ] Write all 7 Rust unit tests in `memory_ssa.rs`
- [ ] `cargo test -p molt-backend --features native-backend` — all tests pass, 0 new warnings
- [ ] `MOLT_VERIFY_ANALYSIS=1` — no divergence panics on the existing differential suite

**Acceptance:** MemorySSA computes correctly on all existing test shapes. No pipeline behavior changes (no consumers yet). Test count increases by 7.

### Phase 2b: MemGVN pass (store-to-load forwarding + redundant-load elimination)

**Checklist:**
- [ ] Create `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mem_gvn.rs` with `run(func, am) -> PassStats`
- [ ] Add `pub mod mem_gvn;` to `passes/mod.rs`
- [ ] Insert `"mem_gvn" (OpsOnly)` into `build_default_pipeline` after `dead_store_elim`
- [ ] Update `default_pipeline_preserves_canonical_pass_order` test (count 25 → 26)
- [ ] Write Rust unit tests: single-block forwarding, cross-block forwarding, blocked-by-call, blocked-by-phi
- [ ] Add differential test `struct_field_forwarding.py` to test suite
- [ ] BigInt safety test `struct_field_forwarding_bigint.py`
- [ ] `cargo test` — all tests pass, 0 new warnings
- [ ] Run differential suite on native: `python3 tests/molt_diff.py basic` — all byte-identical to CPython

**Acceptance:** Store-to-load forwarding works. LoadAttr ops eliminated in simple forwarding cases. Zero differential regressions.

### Phase 2c: Cross-block DSE

**Checklist:**
- [ ] Extend `dead_store_elim::run` with the MemorySSA-backed cross-block DSE phase (`run_cross_block_dse`)
- [ ] Write Rust unit tests in `dead_store_elim.rs`: cross-block kill, phi-guarded preservation, GenericHeap def blocks DSE
- [ ] Add differential test `struct_cross_block_dse.py`
- [ ] `cargo test` — all tests pass
- [ ] Verify `bench_struct.py` partial improvement (proven-neutral store elimination extends cross-block)

**Acceptance:** Cross-block DSE fires on the bench_struct pattern's proven-neutral store chain. No regressions.

### Phase 2d: SROA (object-field promotion — the bench_struct proving ground)

**Checklist:**
- [ ] Create `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/sroa.rs`
- [ ] Add `pub mod sroa;` to `passes/mod.rs`
- [ ] Insert `"sroa" (OpsOnly)` into `build_default_pipeline` after `mem_gvn`
- [ ] Update pipeline order test (count 26 → 27)
- [ ] Write all 4 Rust unit tests in `sroa.rs`
- [ ] Add differential test `struct_sroa.py`
- [ ] Add exception-safety differential test `struct_with_try_block.py`
- [ ] Run bench_struct benchmark: `python3 tools/safe_run.py --rss-mb 512 --timeout 60 -- ...`
- [ ] Confirm >= 1.0x CPython on native release-fast
- [ ] Confirm WASM and LLVM backends produce correct output on all differential tests
- [ ] `cargo test` — all tests pass
- [ ] Full differential suite: `python3 tests/molt_diff.py basic stdlib` — 0 regressions

**Acceptance:** bench_struct executes >= 1.0x CPython speed. All backends correct. Test suite fully green.

### Phase 2e: LICM-of-loads

**Checklist:**
- [ ] Extend `licm::run` to call `am.get::<MemorySSA>` and pass it to `is_hoistable_with_mem`
- [ ] Write LICM test `loop_invariant_load_hoisted_with_mem_ssa`
- [ ] Add differential test `struct_loop_licm.py`
- [ ] Confirm `bench_loop_field_licm` benchmark: loop reading `p.x` repeatedly hoists the load
- [ ] `cargo test` — all tests pass
- [ ] Full differential suite — 0 regressions

**Acceptance:** Loop-invariant typed-slot loads are hoisted. No regressions. Perf gate: bench_loop_field_licm >= 1.0x CPython.

---

## Critical Implementation Details

### MemorySSA invalidation on SROA

When SROA fires and removes a `LoadAttr`, the `MemorySsaResult` cache must be invalidated. Because SROA is declared `Mutates::OpsOnly`, the PassManager calls `am.invalidate_ops()` after it, which drops `MemorySSA` (ops-sensitive). The next consumer (MemGVN, if SROA is placed before it — or LICM) recomputes from scratch. **SROA must not be placed AFTER MemGVN in the same pipeline run without a second MemGVN pass**, because the forwarded loads that SROA creates (actually, SROA removes LoadAttr ops, creating Copy ops) may expose new forwarding opportunities for MemGVN. The ordering `mem_gvn → sroa → copy_prop → dce` is correct: MemGVN first reduces loads, SROA promotes the survivors, copy_prop cleans up the Copy chains, DCE removes the now-dead allocation.

### repr safety on forwarded values

When `mem_gvn` replaces `LoadAttr(obj, offset) → r` with `Copy(stored_val) → r`, the new `Copy` carries no `_original_kind`, making it a pure SSA move. The type of `r` in `func.value_types` is already set by the type-refine pass (which ran before the memory passes). The forwarded value `stored_val` must have a compatible or more-precise type. Since `StoreAttr` stores an arbitrary Python value and `LoadAttr` reads it back, both have type `DynBox` at the TIR level in the unrefined case. If type refinement has run and the field is a `TypedField` with a known primitive type (e.g., `I64` for `int` fields), the stored value has type `I64` and the load result already has type `I64` in `value_types`. The forwarded `Copy(stored_val) → r` preserves this because `Copy` is a representation-transparent pass-through in `lower_to_lir.rs`.

The one critical check: never introduce a trusted-unbox of a `MaybeBigInt` value. Since the forwarded value is the exact SSA value the store put in, and that value's repr is whatever `representation_plan.rs` assigned it at store time, forwarding it to the load cannot introduce a repr that is more aggressive than what was already computed. If the store value is `MaybeBigInt`, the load result becomes `MaybeBigInt` — correct.

### Dominance frontier computation for MemorySSA

The existing `ssa.rs` computes dominance frontiers inline during SSA construction (it does not expose a standalone function). For MemorySSA, compute dominance frontiers from the already-available `ImmediateDoms` analysis:

```rust
fn compute_dominance_frontiers(
    func: &TirFunction,
    idoms: &HashMap<BlockId, Option<BlockId>>,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut df: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for (&b, preds) in pred_map {
        if preds.len() >= 2 {
            for &p in preds {
                let mut runner = p;
                while idoms[&b].map_or(true, |idom| idom != runner) {
                    df.entry(runner).or_default().insert(b);
                    runner = match idoms[&runner] {
                        Some(idom) => idom,
                        None => break,
                    };
                }
            }
        }
    }
    df
}
```

This is the standard Cooper/Harvey/Kennedy algorithm, O(n²) in the worst case but fast on typical CFGs.

### The `NoEscape ArgEscape` distinction for SROA

SROA is legal on both `NoEscape` and `ArgEscape` objects. `ArgEscape` requires an explicit non-capture fact for the relevant argument; an `effect_free` classification, builtin name, or frontend receiver hint is not such a fact. In the absence of that proof, builtin and dynamic-method calls produce `GlobalEscape`, preventing both SROA and RC stripping across the opaque boundary.

The current intraprocedural authority admits local operand uses only through
`op_kinds.toml`'s generated `opcode_has_local_only_operands_table`. Dynamic
arithmetic, truth conversion, attribute/index access, iteration and generic
stores remain capture boundaries, independently of result purity. Exact copies
and every CFG edge carry escape obligations in both directions. A mixed block
parameter cannot grant local-owner containment or RC-removal permission merely
because one predecessor supplies a local allocation. Typed offset stores retain
their value through a proven local owner; finalizer-bearing owners force their
retained children to remain heap allocated. Promotion and RC removal share one
rewrite-root projection, including positive layout-size admission.
