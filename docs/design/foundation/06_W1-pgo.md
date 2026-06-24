<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: W1 PGO: instrument -> collect -> profile-directed inline/layout/devirt/unroll/vector-width (largest missing family, independent) -->

# PGO End-to-End Architecture Blueprint

## 1. Precise Problem Statement

**What exists (dead code, not a working system):**

- `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/pgo.rs` — `instrument_function` produces string snippets never inserted anywhere; `load_profile` parses `.profdata` text but is never called; `branch_weight_metadata` returns a string that `lowering.rs:5144` explicitly drops with `let _ = (branch_inst, true_weight, false_weight)`.
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs:296-298` — `FunctionLowering.pgo_branch_weights: Option<Vec<u64>>` field and `pgo_weight_index` exist but `try_lower_tir_to_llvm` at line 360 always passes `None` for `pgo_branch_weights`.
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/ir.rs:14-18` — `PgoProfileIR` carries `hot_functions: Vec<String>` only; it silently drops `branch_counts` / `call_counts` / `loop_counts` from the JSON payload (the deserializer at line 100 reads only three fields).
- `/Users/adpena/Projects/molt/src/molt/pgo_collect.py` — Python-side `sys.settrace` profiler producing a `molt_profile_version: "0.1"` JSON with `hotspots`, `branch_counts`, `call_counts`, `loop_counts`. Loader and CLI plumbing (`cli.py:27076-27217`) parse and validate this profile but never push `branch_counts`/`loop_counts` into the Rust backend.
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/target_info.rs:149-153` — `ProfileData { hot_functions: BTreeSet<String> }` as a S2 TargetInfo hook; `with_profile_data` exists; `is_pgo_hot` / `inline_budget` work correctly. But the set is always empty (no path populates it from the real `PgoProfileIR.hot_functions`).
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:303-320` — SimpleIR inliner reads `ir.profile.hot_functions` for hot budget selection. This is the ONLY working consumer. It relies on the SimpleIR stringly-typed profile, not the TargetInfo hook.
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:476-503` — `apply_profile_order` reorders functions by hot-function rank; called at `simple_backend.rs:2302` and `wasm.rs:2062`. This is the second working consumer (function layout only).

**What is completely absent:**

1. A stable on-disk profile format with block-level edge counts (needed for Cranelift and WASM block layout, not just LLVM).
2. Counter instrumentation at the TIR level: no TIR opcode, no runtime support function, no instrumented-build mode.
3. Counter collection pipeline: no binary that runs and emits a profile.
4. Profile merge across multiple runs.
5. Profile-directed block layout for native/Cranelift (Cranelift has no `!prof` metadata; reordering happens at the TIR→block-list stage).
6. Profile-directed block layout for WASM.
7. Branch weights wired into LLVM lowering (the `!prof` metadata attachment at `lowering.rs:5144` is dead due to an inkwell API gap).
8. Profile-directed inlining thresholds through the S2 `TargetInfo` channel (the TIR inliner E1 never receives real call-count data).
9. Profile-directed loop unroll trip count selection.
10. Speculative devirtualization driven by profile type frequencies.

**Why this is load-bearing for the 5-year perf goals:** Python is dynamic-dispatch-heavy. Every `x.foo()` call in CPython lands in a polymorphic megamorphic path. Molt's TIR devirts a subset structurally (range/iter), but the majority of call sites remain `Call` ops with no static type evidence. PGO is the correct mechanism for learning, at AOT time, which branches are almost always taken, which callees are hot, and which polymorphic dispatch paths resolve to one type >95% of the time — enabling speculative inlining, cold-block splitting, and profile-directed devirt without a JIT. The compiler_foundation_gap_analysis.md documents this as "10-30% on dynamic-dispatch-heavy Python" and calls it "the single largest missing optimization family."

---

## 2. Structurally Correct End-State Design

### 2.1 The Profile IR — one unified on-disk format

The existing `pgo_collect.py` JSON (`molt_profile_version: "0.1"`) is the collection format. The Rust backend needs to consume it fully. The design extends `PgoProfileIR` in `ir.rs` to carry the full payload:

```
PgoProfileIR {
    version: Option<String>,
    hash: Option<String>,
    hot_functions: Vec<String>,
    // NEW:
    call_counts: HashMap<String, u64>,       // function name → call count
    branch_counts: HashMap<String, BranchCount>,  // branch_id → (taken, not_taken)
    loop_counts: HashMap<String, LoopCount>, // loop_id → (avg_iters, max_iters)
}

BranchCount { taken: u64, not_taken: u64 }
LoopCount { avg_iterations: f64, max_iters: u64 }
```

This is NOT a new format. It is completing the deserialization of the format that already exists in `cli.py:PgoProfileSummary` and `pgo_collect.py:PgoCollector.to_profile_dict()`.

### 2.2 Profile Data Flow

```
pgo_collect.py (sys.settrace) → molt_profile.json
    ↓ cli.py _load_pgo_profile (already parses) → PgoProfileSummary
    ↓ cli.py _attach_build_metadata → pgo_profile JSON field in build payload
    ↓ ir.rs PgoProfileIR::from_json_value (extend) → PgoProfileIR with full counters
    ↓ SimpleIR.profile: Option<PgoProfileIR>
    ↓ (Phase W1a) simple_backend.rs: build TargetInfo with_profile_data
    ↓ TargetInfo.profile_data: Option<ProfileData>
    ↓ TIR pass pipeline: inliner (E1), loop_unroll, pass_manager
    ↓ (Phase W1b) Cranelift block layout via TirFunction.block_order_hint
    ↓ (Phase W1c) LLVM lowering: branch weight metadata via llvm-sys raw call
    ↓ (Phase W1d) WASM lowering: block layout hint
```

### 2.3 Branch ID → TIR CondBranch Mapping

The profiler in `pgo_collect.py` uses Python `sys.settrace` at the function/line level — it does not produce per-CondBranch edge counts, only function-level call counts. This is the key constraint.

**Design decision:** The first-cut PGO consumer uses **call-count data only** (not branch-edge counts). This is the correct and complete first arc because:

1. The existing `pgo_collect.py` produces reliable call-count data via `sys.settrace`.
2. Branch-edge counts require either: (a) a second profiling run on a compiled (non-Python) binary with counters inserted at each edge, or (b) a Python-side per-line tracer with branch disambiguation. Both are separate subsequent arcs.
3. Call-count data is sufficient to wire the three highest-value consumers: profile-directed inlining (hot callees get larger budget), profile-directed function ordering (function layout), and profile-directed loop unroll trip selection (hot loops get higher trip cap).
4. The `llvm_backend/pgo.rs` `!prof` branch-weight path is structurally ready — it just needs a counter-instrumented binary run to produce the data. That is arc W1-phase-d (instrumented native binary → LLVM profdata).

The design is:

**Phase W1-a (the complete first structural arc):** Extend `PgoProfileIR` with `call_counts`; bridge `SimpleIR.profile` to `TargetInfo.profile_data`; wire the TIR inliner, loop unroll, and function layout to consume it. Delete the dual `pgo_hot: BTreeSet<&str>` read in `passes.rs` (SimpleIR inliner) in favor of the single `tti.is_pgo_hot()` path.

**Phase W1-b (block layout — Cranelift):** Introduce `TirFunction.block_order_hint: Option<Vec<BlockId>>` populated from function-level hotness; teach `lower_to_simple.rs` to emit blocks in hot-first order when the hint is set.

**Phase W1-c (LLVM branch weights):** Add `llvm-sys` as a direct dep or use inkwell's raw `unsafe` LLVM C API surface to attach `!prof` metadata. The existing plumbing at `lowering.rs:5111-5146` is a complete TODO comment; this arc executes it.

**Phase W1-d (instrumented binary → per-edge counters):** Insert `PgoCounter` TIR ops in a new instrumented-build mode; lower them to `molt_pgo_counter_increment(slot)` runtime calls; collect at exit; write `molt_pgo.profdata`; feed back via `--pgo-profile`.

---

## 3. Component Design

### 3.1 `runtime/molt-backend/src/ir.rs` — Extended `PgoProfileIR`

**File:** `/Users/adpena/Projects/molt/runtime/molt-backend/src/ir.rs`

Extend `PgoProfileIR` (currently lines 11-18) to carry the full profile payload:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
pub struct BranchCountIR {
    pub taken: u64,
    pub not_taken: u64,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct LoopCountIR {
    pub avg_iterations: f64,
    pub max_iterations: u64,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct PgoProfileIR {
    pub version: Option<String>,
    pub hash: Option<String>,
    pub hot_functions: Vec<String>,
    // NEW: populated from pgo_collect.py JSON payload
    pub call_counts: std::collections::HashMap<String, u64>,
    pub branch_counts: std::collections::HashMap<String, BranchCountIR>,
    pub loop_counts: std::collections::HashMap<String, LoopCountIR>,
}
```

`PgoProfileIR::from_json_value` (line 95) must be extended to parse `call_counts`, `branch_counts`, `loop_counts` from the JSON object. These are the same fields `_load_pgo_profile` in `cli.py` already parses client-side.

**No other changes to `ir.rs`** — `SimpleIR.profile: Option<PgoProfileIR>` already exists; the new fields extend the existing struct.

### 3.2 `runtime/molt-tir/src/tir/target_info.rs` — Extended `ProfileData`

**File:** `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/target_info.rs`

Extend `ProfileData` (currently lines 148-153) to carry call-count data that drives:
- `inline_budget` (already wired, just needs real counts)
- New query `call_count(name) -> u64`
- New query `loop_avg_trip(loop_id) -> Option<f64>`

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileData {
    pub hot_functions: std::collections::BTreeSet<String>,
    // NEW: raw call counts for fine-grained inlining decisions
    pub call_counts: std::collections::BTreeMap<String, u64>,
    // NEW: per-loop average trip counts for unroll trip selection
    pub loop_avg_trips: std::collections::BTreeMap<String, u64>,  // floor of avg_iterations
}
```

Add to `TargetInfo`:
```rust
pub fn call_count(&self, name: &str) -> u64 {
    self.profile_data.as_ref()
        .and_then(|p| p.call_counts.get(name).copied())
        .unwrap_or(0)
}

pub fn loop_avg_trip(&self, loop_id: &str) -> Option<u64> {
    self.profile_data.as_ref()
        .and_then(|p| p.loop_avg_trips.get(loop_id).copied())
}
```

Add constructor helper:
```rust
impl ProfileData {
    /// Build from a `PgoProfileIR` (the deserialized on-disk profile).
    pub fn from_pgo_ir(ir: &crate::ir::PgoProfileIR) -> ProfileData {
        let hot_threshold = 10u64; // calls >= this → hot
        let hot_functions = ir.call_counts.iter()
            .filter(|(_, &c)| c >= hot_threshold)
            .map(|(n, _)| n.clone())
            .chain(ir.hot_functions.iter().cloned())
            .collect();
        let call_counts = ir.call_counts.iter()
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        let loop_avg_trips = ir.loop_counts.iter()
            .map(|(k, v)| (k.clone(), v.avg_iterations.floor() as u64))
            .collect();
        ProfileData { hot_functions, call_counts, loop_avg_trips }
    }
}
```

The `hot_threshold` of 10 is a first-cut constant in TargetInfo (not magic — it becomes `pgo_hot_call_threshold` from the existing field which is 1000; for the profiler-collected `call_counts` where the values are absolute call counts across one representative run, 10 is a more appropriate floor and should be a separate `TargetInfo` field `pgo_min_call_count`).

### 3.3 `runtime/molt-backend/src/native_backend/simple_backend.rs` — Wire Profile to TargetInfo

**File:** `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs`

At `simple_backend.rs:2542-2547` (the per-function TIR pipeline call), replace the hardcoded `TargetInfo::native_from_simd_caps(...)` with one that carries the profile data:

```rust
let tti = {
    let base = crate::tir::target_info::TargetInfo::native_from_simd_caps(
        crate::tir::target_info::SimdCaps::detect_host(),
    );
    if let Some(ref prof) = ir.profile {
        base.with_profile_data(
            crate::tir::target_info::ProfileData::from_pgo_ir(prof)
        )
    } else {
        base
    }
};
// use `tti` instead of inline TargetInfo::native_from_simd_caps(...)
```

Similarly at the `inline_functions` call at line 2627:
```rust
inline_functions(&mut ir, &tti);  // pass the profile-bearing tti
```

**Delete the dual hot-function lookup in `passes.rs:303-320`** (the `pgo_hot: BTreeSet<&str>` from `ir.profile.hot_functions`). This is the legacy dual source of truth. After this arc, `inline_functions` reads `tti.is_pgo_hot(name)` exclusively (the S2 cost model path), which is already wired at `passes.rs:316`.

### 3.4 `runtime/molt-tir/src/tir/passes/loop_unroll.rs` — Profile-directed Trip Cap

**File:** `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/loop_unroll.rs`

The loop_unroll pass already takes `tti: &TargetInfo` (wired by the PassManager at `pass_manager.rs:297`). Add: when a loop header has a loop_id (via `label_id_map` or `loop_roles`), query `tti.loop_avg_trip(loop_id)` and, if the average trip count is a small constant ≤ `tti.unroll_max_trip * 2`, raise the effective trip cap to `min(avg_trip, tti.unroll_max_trip * 4)`. This lets PGO extend unrolling to loops that historically iterate 12-16 times (above the static cap of 8) without touching the static default.

**Soundness:** This is a trip-cap enlargement, not a guaranteed unroll. The unroll still requires the trip count to be proven constant by SCEV. A missing or stale profile lowers back to `tti.unroll_max_trip` (8). No miscompile possible.

### 3.5 `runtime/molt-tir/src/tir/passes/inliner.rs` — Profile-directed Inline Budget

`is_inlineable` (line 214) already takes `tti: &TargetInfo` and calls `tti.inline_budget(&callee.name)` which routes through `is_pgo_hot`. After W1-a, this path receives real call-count data and requires no change in the inliner itself — the budget query is already correct. The fix is ensuring `tti.is_pgo_hot` returns `true` for hot callees, which happens via `ProfileData::from_pgo_ir`.

### 3.6 `runtime/molt-backend/src/wasm.rs` — Wire Profile to WASM TargetInfo

At `wasm.rs:2062` (`apply_profile_order`), the same `tti` construction pattern as simple_backend.rs must apply. The WASM TIR pipeline (wherever `run_pipeline` is called for WASM functions) must receive the profile-bearing `TargetInfo`. Locate the `run_pipeline` call site(s) in `wasm.rs` and apply the same `ir.profile → TargetInfo::with_profile_data` pattern.

### 3.7 `runtime/molt-backend/src/llvm_backend/lowering.rs` — LLVM Branch Weights (W1-c)

**This is the hardest arc** because of the inkwell `!prof` metadata API gap documented in the existing comment at `lowering.rs:5128-5144`.

The correct fix: add `llvm-sys` as a direct Cargo dependency under `[features] llvm` in `runtime/molt-backend/Cargo.toml`, then replace the dead `let _ = (branch_inst, ...)` block with:

```rust
#[cfg(feature = "llvm")]
unsafe {
    use llvm_sys::core::{
        LLVMConstInt, LLVMGetMDKindIDInContext,
        LLVMMDNodeInContext2, LLVMMDStringInContext2,
        LLVMSetMetadata, LLVMInt32TypeInContext,
    };
    let ctx_ref = self.backend.context.as_ctx_ref();
    let prof_kind = LLVMGetMDKindIDInContext(ctx_ref, c"prof".as_ptr(), 4);
    let bw_str = LLVMMDStringInContext2(ctx_ref, c"branch_weights".as_ptr(), 14);
    let i32_ty = LLVMInt32TypeInContext(ctx_ref);
    // Saturate to u32::MAX to avoid LLVM truncation surprises.
    let tw = true_weight.min(u32::MAX as u64) as u32;
    let fw = false_weight.min(u32::MAX as u64) as u32;
    let t_val = LLVMConstInt(i32_ty, tw as u64, 0);
    let f_val = LLVMConstInt(i32_ty, fw as u64, 0);
    let md_vals = [bw_str, t_val as _, f_val as _];
    let md_node = LLVMMDNodeInContext2(ctx_ref, md_vals.as_ptr(), 3);
    LLVMSetMetadata(branch_inst.as_value_ref(), prof_kind, md_node);
}
```

The LLVM entry point (`try_lower_tir_to_llvm`, line 356) must be extended to accept branch weights from the module-level profile. The `LlvmBackend` struct needs a `pgo_data: Option<Arc<PgoProfileIR>>` field populated from `SimpleIR.profile` before any function is lowered.

In `try_lower_tir_to_llvm_with_pgo`, the `pgo_branch_weights` vector must be constructed from the profile's `branch_counts` keyed by a stable branch ID. The branch ID scheme: `"{func_name}:{block_id}:{then_block_id}"` — a deterministic string formed during TIR lowering when a `CondBranch` is emitted.

**However:** The existing `pgo_collect.py` does NOT produce per-CondBranch edge counts. It produces per-function call counts only. The LLVM `!prof` branch weights need **per-edge** counts, which requires either:
- A second collection run using a compiled instrumented binary, OR
- Propagating call-count hotness to all CondBranch ops within hot functions (a crude but useful approximation: mark all branches in a hot function as "not strongly predicted" → uniform weights, which is the same as not attaching `!prof`).

**Decision for W1-c:** The LLVM `!prof` attachment is **gated on the W1-d instrumented binary arc** which produces real per-edge counts. W1-c's structural work is: fix the inkwell API gap (the `llvm-sys` unsafe call) so the mechanism is correct and tested; the wiring to real per-edge counter data is W1-d. The `pgo_branch_weights: Option<Vec<u64>>` field in `FunctionLowering` stays as the injection point.

### 3.8 `runtime/molt-tir/src/tir/function.rs` — Block Order Hint (W1-b)

Add to `TirFunction`:
```rust
/// Profile-directed block emission order. When Some, `lower_to_simple.rs`
/// emits blocks in this order (hot blocks first, cold blocks last) rather than
/// the default BFS/dominance order. None = static order unchanged.
pub block_order_hint: Option<Vec<BlockId>>,
```

Populate in `simple_backend.rs` before TIR lowering: when the function appears in `ProfileData.hot_functions` and `call_count >= threshold`, run a simple BFS from the entry block weighting edges by CondBranch bias (from branch_counts if available, else uniform), and set `block_order_hint` to the resulting order.

For the first-cut W1-a, `block_order_hint` is always `None` (the field is added but not populated until W1-b). This is structural preparation, not dead code: the field serves as the type-level contract the W1-b arc fills in.

### 3.9 New TIR Pass: `pgo_annotate` (W1-d prerequisite)

**File to create:** `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/pgo_annotate.rs`

This pass runs when `tti.profile_data` is `Some` and annotates TIR `CondBranch` terminators with a `pgo_weight` attribute `AttrValue::I64(taken_pct_millipercent)` derived from `profile_data.branch_counts`. Consumers:
- LLVM lowering reads this attr when `!prof` is attached.
- Cranelift block layout uses this attr to determine "cold" paths.

For W1-a/b/c, this pass is a **ReadOnly no-op** (no counts → no attrs). For W1-d it becomes the live consumer. Adding it now means the pipeline slot exists and every downstream consumer can be written against a stable attr name rather than retrofitted.

### 3.10 `runtime/molt-tir/src/tir/passes/mod.rs` + `pass_manager.rs` — Pipeline Slot

Add `pgo_annotate` as a `ReadOnly` pass immediately before `block_versioning` (phase ordering: annotation should precede type-directed specialization so block_versioning can consider hot/cold splits):

```rust
pass("pgo_annotate", ReadOnly, |f, _am, tti| {
    passes::pgo_annotate::run(f, tti)
}),
```

The pass is `ReadOnly` because it only adds `pgo_weight` attrs to terminators; it never changes the block structure or op operands.

---

## 4. Soundness Argument

**Conservative-correct invariant:** A missing or stale profile **always degrades gracefully** to the static heuristics:

1. `tti.is_pgo_hot(name)` returns `false` when `profile_data` is `None` → inline budget = base (30 ops), unchanged from pre-PGO.
2. Loop unroll trip cap with no profile = `tti.unroll_max_trip` (8), unchanged.
3. `pgo_annotate` with no `branch_counts` emits no attrs → LLVM lowering attaches no `!prof` → LLVM uses its own static prediction heuristics, unchanged.
4. `block_order_hint = None` → TIR lowering uses the existing block order, unchanged.
5. `ProfileData::from_pgo_ir` is the single deserialization point. If the JSON is structurally invalid or fields are missing, `PgoProfileIR::from_json_value` returns the default empty profile (all counters empty) → no change to any optimization decision.

**No new miscompile surface:** Every PGO consumer is a profitability gate (budget/threshold/order), not a correctness gate. The inliner's `is_inlineable` soundness checks (SSA, refcount, loop metadata, exception exclusions) are unaffected; PGO only changes whether a callee clears the budget check, not whether it clears the structural-soundness checks.

---

## 5. Legacy This Arc Deletes

**Dual hot-function read in `passes.rs`:** Lines 303-320 in `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs` build a `pgo_hot: BTreeSet<&str>` from `ir.profile.hot_functions` directly. After W1-a, the SimpleIR inliner (`inline_functions`) reads `tti.is_pgo_hot(name)` exclusively. The local `pgo_hot` set is deleted. This is the only dual-source-of-truth in the current PGO infrastructure. Deleting it means there is exactly one place (`TargetInfo.profile_data.hot_functions`) that answers "is this function hot?"

**The dead comment block in `lowering.rs:5128-5144`:** After W1-c executes the `llvm-sys` unsafe call, the 16-line TODO comment is replaced with working code. The `let _ = (branch_inst, true_weight, false_weight)` no-op becomes the real metadata attachment.

**`pgo.rs:instrument_function` string-snippet API:** After W1-d lands a real TIR-level counter-insertion pass and a runtime counter table, `instrument_function` (which returns a `Vec<String>` of LLVM IR snippets that are never inserted anywhere) is deleted. The instrumented-build mode goes through the TIR pass, not string manipulation.

---

## 6. Test Plan

### 6.1 Rust Unit Tests

**`ir.rs` — `PgoProfileIR` extended deserialization:**
```rust
#[test]
fn pgoProfileIr_deserializes_call_counts() {
    let json = r#"{"version":"0.1","hot_functions":["f"],
        "call_counts":{"f":1500,"g":5}}"#;
    let profile = PgoProfileIR::from_json_value(&serde_json::from_str(json).unwrap(), "test").unwrap();
    assert_eq!(profile.call_counts["f"], 1500);
    assert_eq!(profile.call_counts["g"], 5);
}

#[test]
fn pgoProfileIr_empty_call_counts_does_not_fail() {
    let json = r#"{"version":"0.1","hot_functions":[]}"#;
    let profile = PgoProfileIR::from_json_value(&serde_json::from_str(json).unwrap(), "test").unwrap();
    assert!(profile.call_counts.is_empty());
}

#[test]
fn pgoProfileIr_deserializes_loop_counts() {
    let json = r#"{"version":"0.1","loop_counts":{"loop_1":{"avg_iterations":12.5,"max_iterations":20}}}"#;
    let profile = PgoProfileIR::from_json_value(&serde_json::from_str(json).unwrap(), "test").unwrap();
    assert_eq!(profile.loop_counts["loop_1"].avg_iterations, 12.5);
    assert_eq!(profile.loop_counts["loop_1"].max_iterations, 20);
}
```

**`target_info.rs` — `ProfileData::from_pgo_ir`:**
```rust
#[test]
fn profile_data_from_pgo_ir_hot_threshold() {
    let mut ir = PgoProfileIR::default();
    ir.call_counts.insert("hot_fn".into(), 1500);
    ir.call_counts.insert("cold_fn".into(), 3);
    let pd = ProfileData::from_pgo_ir(&ir);
    assert!(pd.hot_functions.contains("hot_fn"));
    assert!(!pd.hot_functions.contains("cold_fn"));
}

#[test]
fn inline_budget_uses_profile_data() {
    let mut ir = PgoProfileIR::default();
    ir.call_counts.insert("hot".into(), 1500);
    let tti = TargetInfo::native_release_fast()
        .with_profile_data(ProfileData::from_pgo_ir(&ir));
    assert_eq!(tti.inline_budget("hot"), 80);   // hot budget
    assert_eq!(tti.inline_budget("cold"), 30);  // base budget
}
```

**`pass_manager.rs` — pipeline order includes `pgo_annotate`:**
Update the existing `default_pipeline_preserves_canonical_pass_order` test to include `"pgo_annotate"` in the expected names vector at the correct position (before `block_versioning`).

**`pgo_annotate.rs` — no-op when no profile:**
```rust
#[test]
fn pgo_annotate_noop_without_profile_data() {
    let mut func = minimal_function_with_cond_branch();
    let tti = TargetInfo::native_release_fast(); // no profile
    let stats = pgo_annotate::run(&mut func, &tti);
    assert_eq!(stats.ops_added, 0);
    assert_eq!(stats.values_changed, 0);
    // No CondBranch terminator has a pgo_weight attr
    for block in func.blocks.values() {
        if let Terminator::CondBranch { .. } = &block.terminator {
            // no pgo_weight in block attrs
        }
    }
}
```

### 6.2 Differential Tests (Python)

**`tests/differential/pgo/hot_function_gets_inlined.py`:**
```python
# A function called 10000 times should be inlined when PGO profile present.
def inner(x):
    return x * 2 + 1

total = 0
for i in range(10000):
    total += inner(i)
print(total)   # 100010000
```
Run: collect profile → build with `--pgo-profile` → verify output matches CPython.

**`tests/differential/pgo/cold_function_not_over_inlined.py`:**
```python
# A function called 2 times should not receive hot budget.
def large_fn(x):
    # 40+ ops body
    a = x + 1; b = a * 2; c = b - 3; d = c // 2
    # ... etc
    return a + b + c + d

print(large_fn(10))
print(large_fn(20))
```
Verify (via `MOLT_INLINE_LIMIT` + `TIR_OPT_STATS=1`) that `large_fn` is NOT inlined.

**`tests/differential/pgo/profile_stale_degrades_gracefully.py`:**
```python
# Provide a profile referencing a function that no longer exists in the source.
# Build must succeed and produce correct output.
def foo():
    return 42
print(foo())
```
Profile: `{"molt_profile_version":"0.1","call_counts":{"deleted_fn":9999},...}`
Expected: builds successfully, prints `42`.

**`tests/differential/pgo/bigint_correctness_under_pgo.py`:**
```python
# PGO must not promote a BigInt accumulator to RawI64Safe
def f():
    x = 1 << 60
    return x + 7
print(f())  # must be 1152921504606846983
```
Build with and without a profile that marks `f` as hot. Output must be byte-identical.

**`tests/differential/pgo/exception_path_under_pgo.py`:**
```python
# A hot function with a CheckException must still propagate exceptions correctly
def maybe_raise(x):
    if x < 0:
        raise ValueError(x)
    return x * 2

try:
    print(maybe_raise(5))
    print(maybe_raise(-1))
except ValueError as e:
    print(f"caught {e}")
```

**`tests/differential/pgo/loop_unroll_with_pgo.py`:**
```python
# A loop that historically ran 12 iterations (above static cap of 8)
# should be unrolled when PGO reports avg_iterations=12
total = 0
for i in range(12):
    total += i * i
print(total)  # 506
```

### 6.3 Cross-Backend Differential

All Python differential tests above must pass on native/Cranelift, WASM, and LLVM. The profile is consumed by `TargetInfo` which is target-agnostic; each backend's lowering reads the same `tti`. The test harness should run each differential test on all three active backends.

---

## 7. Perf-Gate Plan

**Benchmark suite:** `bench/pgo_training.py` is the PGO training corpus. Run it as the representative workload.

**Measurement methodology:**
1. Build `bench/pgo_training.py` without PGO → baseline wall-clock time (5 runs, median).
2. Collect profile: `python3 src/molt/pgo_collect.py bench/pgo_training.py -o /tmp/molt_pgo.json`
3. Build `bench/pgo_training.py` with `--pgo-profile /tmp/molt_pgo.json` → PGO build.
4. Compare wall-clock time: PGO build must be >= baseline (no regression), target >= 5% improvement for the training corpus.
5. Run separately on each non-training benchmark (must not regress vs baseline for any benchmark on any target).

**Per-target perf gate:**
- Native/Cranelift: sieve + mandelbrot + fib from `bench/pgo_training.py`.
- WASM: same suite via `node`.
- LLVM (when W1-c lands): same suite; additionally check that `!prof` metadata appears in LLVM IR dump (`MOLT_DUMP_IR=1`).
- Luau: unaffected by W1 (Luau backend does not use TargetInfo PGO path yet — deferred to W1-e).

**CPython gate:** Every benchmark must remain >= CPython 3.12 on release-fast.

---

## 8. Risk + Rollback + Dependencies

### Blocked by (what W1 depends on):
- S2 (TargetInfo) — **LANDED** (`9ff5d2e00`). The `ProfileData` hook, `with_profile_data`, `is_pgo_hot`, `inline_budget` are all in production code.
- S4 (module phase / call graph) — **LANDED** (`7915b29a0`). The inliner E1 runs in `run_module_pipeline`.
- E1 inliner a+b — **LANDED** (`f14b196ce`). Phase W1-a feeds real `ProfileData` to the inliner's existing budget path.

### Unblocks (what W1 enables):
- W2 CHA + speculative devirt: profile type frequencies (not yet collected) enable W2's speculative inline guard.
- E1 phase-d (multi-site / fixed-point inlining): hot-function profile data informs per-site ROI.
- L1 loop transforms: profile-reported avg_iterations enables dynamic trip unroll.

### What does NOT block W1:
- S5 (alias analysis) — independent.
- S6 (SCEV/ValueRange) — independent.
- E1 phase-c (exception-observation inlining) — independent; W1-a feeds `tti.is_pgo_hot` to the existing phase-a/b sites.

### Rollback:
Phase W1-a (the critical arc) is a pure conservative extension:
- New fields in `PgoProfileIR` and `ProfileData` default to empty.
- New `from_pgo_ir` constructor is additive.
- The `simple_backend.rs` TargetInfo construction change is a two-line conditional: if no profile, path is identical to pre-W1.
- Deleting the `pgo_hot: BTreeSet<&str>` in `passes.rs` is safe: `tti.is_pgo_hot` is the identical logic with a wider input.

If W1-a regresses any test, the rollback is reverting the `simple_backend.rs` TargetInfo construction change — all other changes (PgoProfileIR deserialization, ProfileData extension) are additive and non-breaking.

### Risk: hot_threshold calibration
`ProfileData::from_pgo_ir` uses `10` as the min-call-count threshold. If the training corpus calls `inner` 10000 times but another function that is semantically cold also gets 10 calls, the hot-function set over-approximates. **This is safe:** an over-approximation in hotness gives the inliner a larger budget (80 ops) for a function it might prefer to not inline, producing a slightly larger binary but never a wrong result. The threshold should be a `TargetInfo` field (`pgo_min_call_count: u64 = 10`) so it is tunable without code changes.

---

## 9. Phased Landing Sequence

### Phase W1-a — Core data flow (the complete first structural arc)

This is the only phase that must ship as one atomic change. It includes:

**Step 1: Extend `ir.rs`**
- Add `BranchCountIR`, `LoopCountIR` structs.
- Add `call_counts`, `branch_counts`, `loop_counts` fields to `PgoProfileIR`.
- Extend `PgoProfileIR::from_json_value` to parse all three new fields.
- Add unit tests for deserialization.

**Step 2: Extend `target_info.rs`**
- Add `call_counts`, `loop_avg_trips` to `ProfileData`.
- Add `ProfileData::from_pgo_ir` constructor.
- Add `call_count(&self, name)` and `loop_avg_trip(&self, loop_id)` to `TargetInfo`.
- Add `pgo_min_call_count: u64 = 10` field to `TargetInfo`.
- Update `native_release_fast_reproduces_legacy_literals` test.
- Add `profile_data_from_pgo_ir_hot_threshold` test.

**Step 3: Bridge `simple_backend.rs`**
- Build a single `tti` in `SimpleBackend::compile` that carries `ProfileData` when `ir.profile` is `Some`.
- Thread this `tti` through both the TIR pipeline call and the `inline_functions` call.
- Delete `pgo_hot: BTreeSet<&str>` from `passes.rs:303-320` (the dual source of truth).
- The `inline_functions` signature already takes `tti: &TargetInfo` — no signature change needed.

**Step 4: Bridge `wasm.rs`**
- Same TargetInfo construction as simple_backend.rs.

**Step 5: Add `pgo_annotate` pass skeleton**
- Create `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/pgo_annotate.rs` as a `ReadOnly` no-op pass.
- Register in `passes/mod.rs` and `pass_manager.rs` before `block_versioning`.
- Update `default_pipeline_preserves_canonical_pass_order` test.

**Acceptance gate for W1-a:** All existing 882 tests pass. The differential tests from §6.2 pass on all active backends (native, WASM). `TIR_OPT_STATS=1` on `bench/pgo_training.py` with a collected profile shows `functions_changed > 0` from the TIR inliner for the `inner`-calling benchmark. Perf: >= baseline on every benchmark.

---

### Phase W1-b — Cranelift block layout

**Step 1:** Add `block_order_hint: Option<Vec<BlockId>>` to `TirFunction` in `function.rs`.
**Step 2:** In `simple_backend.rs`, populate `block_order_hint` for hot functions using the collected branch_counts (if present) or a static RPO order (if absent).
**Step 3:** Teach `lower_to_simple.rs` to honor `block_order_hint` when emitting the SimpleIR block sequence.
**Step 4:** Add differential test `block_layout_hot_first.py` that verifies (via `MOLT_DUMP_IR=1`) that the hot basic block appears before the cold block in the emitted IR.

**Acceptance gate for W1-b:** Block layout test passes. No perf regression. `apply_profile_order` in `passes.rs` (function-level hot ordering) stays; `block_order_hint` adds intra-function block ordering.

---

### Phase W1-c — LLVM branch weight metadata

**Step 1:** Add `llvm-sys` as a direct Cargo dependency in `runtime/molt-backend/Cargo.toml` under `[target.'cfg(feature = "llvm")'.dependencies]`.
**Step 2:** Replace the `let _ = (branch_inst, true_weight, false_weight)` no-op in `lowering.rs:5144` with the `llvm-sys` unsafe call described in §3.7.
**Step 3:** Add a unit test that compiles a function with known branch counts, dumps the LLVM IR, and asserts `!prof` metadata is present.
**Step 4:** Wire the LlvmBackend construction (in `native_backend/simple_backend.rs` LLVM path and in `llvm_backend/mod.rs`) to pass `ir.profile` branch counts as the `pgo_branch_weights` for each function.

**Acceptance gate for W1-c:** LLVM IR dump shows `!prof` metadata on conditional branches when a profile with branch_counts is provided. No miscompile (byte-identical CPython differential on all LLVM tests). LLVM build must succeed with `LLVM_SYS_211_PREFIX` set (pre-existing env requirement).

---

### Phase W1-d — Instrumented binary counter collection

**Step 1:** Add `OpCode::PgoCounter { slot: u32 }` to TIR ops.
**Step 2:** Add `molt_pgo_counter_increment(slot: u32)` to the runtime (in `molt-runtime`, exposed as an intrinsic).
**Step 3:** Add `pgo_instrument` TIR pass: for each `CondBranch`, insert `PgoCounter` ops in then/else entry blocks with stable slot IDs.
**Step 4:** Lower `PgoCounter` ops through native, WASM, and LLVM backends.
**Step 5:** At program exit (via a destructor or `atexit` registration), write slot counts + function names to `molt_pgo.profdata` (a JSON file matching `pgo_collect.py` output format).
**Step 6:** Add `--pgo-collect` flag to `molt build` that enables the `pgo_instrument` pass.
**Step 7:** Update `PgoProfileIR::from_json_value` to accept the new `branch_counts_v2` field (keyed by `"{func}:{bb}:{then_bb}"` stable IDs) produced by the instrumented binary.

**Acceptance gate for W1-d:** `molt build bench/pgo_training.py --pgo-collect` produces a `molt_pgo.profdata`. A subsequent `molt build bench/pgo_training.py --pgo-profile molt_pgo.profdata` produces correct output (differential parity) and shows real `!prof` metadata in the LLVM IR dump. WASM and native must also work.

---

## Integration Points Summary

| Component | File | Change Type |
|---|---|---|
| `PgoProfileIR` extension | `/Users/adpena/Projects/molt/runtime/molt-backend/src/ir.rs` | Extend struct + deserialization |
| `ProfileData` extension | `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/target_info.rs` | Extend struct + new constructor + new queries |
| SimpleIR inliner de-duplication | `/Users/adpena/Projects/molt/runtime/molt-backend/src/passes.rs:303-320` | Delete dual hot-function read |
| TargetInfo wiring (native) | `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs:2542-2547, 2627` | Bridge profile → TargetInfo |
| TargetInfo wiring (WASM) | `/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs` | Same pattern as native |
| `pgo_annotate` pass | `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/pgo_annotate.rs` | New file (ReadOnly no-op first cut) |
| Pipeline registration | `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/mod.rs` + `pass_manager.rs` | Add pass slot |
| `block_order_hint` field | `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/function.rs` | Extend struct (W1-b) |
| LLVM `!prof` metadata | `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs:5144` | Replace no-op with llvm-sys call (W1-c) |
| `Cargo.toml` llvm-sys dep | `/Users/adpena/Projects/molt/runtime/molt-backend/Cargo.toml` | Add dependency (W1-c) |
| `pgo.rs` cleanup | `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/pgo.rs` | Delete `instrument_function` string API (W1-d) |
