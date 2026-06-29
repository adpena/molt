<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: D1 Coroutine-frame elision + generator inlining/fusion -> os.walk-as-CPython-Python-generator (the proving ground) -->

# Generator Fusion — Complete Implementation Blueprint

## 1. Precise Problem Statement

**Why it is load-bearing for the 5-year perf goals.**

Every `def`-with-`yield` generator in molt AOT-compiles to:
- `AllocTask(task_kind="generator", poll=P, closure_size=N)` — a heap frame of N bytes via `molt_task_new`
- Per-iteration `IterNext` → `molt_generator_send(g, None)` → indirect call into `P(frame)` → `STATE_SWITCH` dispatch → resume to current yield block → execute body → `StateYield(pair, next_state)` → store state + return pair to caller

The cost per-element is: one indirect call + one heap round-trip on the pair + two refcount adjustments + a switch dispatch. For a generator consumed in a tight for-loop (the `sum(x for x in gen())` / os.walk shape), this is a 10-30× perf gap versus a direct counting loop.

**The strategic prize is not merely perf.** The treadmill problem is: every stdlib function (os.walk, itertools.chain, itertools.islice, ...) that needs to be fast currently requires a hand-written Cranelift intrinsic. Each intrinsic is ~300-800 lines of unsafe Rust, binds to exactly one backend, and decouples stdlib semantics from CPython. Generator fusion makes a compiler that is good enough to compile idiomatic Python generators to machine-code equivalent to the hand-written version. Then os.walk, itertools, and the entire itertools family can live as pure Python generators, and the intrinsics can be deleted.

The specific open correctness bug: os.walk's current implementation is DELETED from the tree (reverted at HEAD 934938665). The OOM (eager materialization) and SIGSEGV (native recursion on deep trees) are open. Generator fusion and a rewrite of os.walk as a Python generator is the only structurally correct fix.

## 2. Key Open Question Resolved

**Are genexprs lowered through the same coroutine repr as def-yield?**

YES. From `docs/design/generator_fusion.md` line 19-21 (verified against the design doc committed at 1ad199725): "genexpr and `def`-yield generators share ONE representation: both → a `poll_func` + `ALLOC_TASK(task_kind="generator")`. Frontend: `visit_FunctionDef` frontend/__init__.py:30765-31043 (def-yield), `visit_GeneratorExp` :14198-14347 (genexpr — builds `ast.Yield` and uses the SAME `visit_Yield` path :32478)."

However, the existing `deforestation.rs` pass already fuses the `sum/any/all/list(genexpr)` shapes by operating at the `CallBuiltin` + `ForIter` level BEFORE the coroutine frame appears — the frontend's `_try_emit_inline_sum_genexpr` inlines these before the coroutine is emitted, and `deforestation.run()` covers the leftover `ForIter`-carried shapes for pure bodies.

The gap is: **def-yield generators whose body is not pure (has a `Call`, `ClosureLoad`, etc.), and ALL generators consumed by a `for` loop rather than a builtin consumer, are not fused**. This covers:
- `for x in my_generator(): ...` — any loop body
- `for root, dirs, files in os.walk(path): ...` — the primary target
- `for x in itertools.chain(...): ...` etc.

The architectural decision: a specialized `generator_fusion` TIR pass, not a general inliner+SROA combination. The specialized pass is bounded, fits the existing devirt-pass pattern (`iter_devirt.rs`, `range_devirt.rs`), and all 4 backends benefit because the fusion happens at TIR before backend lowering.

## 3. Structurally Correct End-State Architecture

### Core insight: the coroutine frame is a loop-carried state machine

A generator body lowered as:
```
entry: STATE_SWITCH(state) -> [state_0: init_block, state_1: resume_block_1, ...]
init_block: ...compute... STATE_YIELD(pair_0, next_state=1) -> RETURN pair_0
resume_block_1: ...compute... STATE_YIELD(pair_1, next_state=2) -> RETURN pair_1
...
exhausted_block: RETURN (None, True)
```

Fused into the consumer loop becomes:
```
preheader: [initialize frame-slot SSA phis to initial values]
loop_header(frame_slots..., state_phi):
  dispatch on state_phi -> [init_block', resume_1', ...]
  
init_block': ...compute... -> (elem=value_0, break=false) -> consumer_body
consumer_body: [user loop body using elem] -> back_edge(next_state=1)

resume_1': ...compute... -> (elem=value_1, break=false) -> consumer_body
...
exhausted': -> loop_exit

loop_exit: ...
```

All `STATE_YIELD` points become yield_sites where: (a) the yielded value binds to the consumer's for-target, (b) the consumer body runs, (c) the back-edge carries the next-state value to the state_phi. No heap frame. No indirect call. No pair allocation.

### Data structures

**New TIR attribute on `AllocTask`:** already has `task_kind`, `s_value` (poll name), `value` (closure size). No new ops needed. The pass reads these.

**New per-function annotation on `TirFunction` (optional, for phase tracking):**
```rust
// In TirFunction — NOT added to the struct; tracked as a function attr in attrs HashMap
// to preserve zero-cost on non-generator functions.
// attrs["generator_fusion_applied"] = AttrValue::Bool(true)
```

**New pass file:**
`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/generator_fusion.rs`

### Recognition predicate

A fusion candidate is a triple `(alloc_site, get_iter_site, for_loop_header)` where:

1. `alloc_val = AllocTask(task_kind="generator", poll=P, closure_size=N)` — in the caller function
2. `P` is the name of a function in the `TirModule` (cross-function — the whole-program IR)
3. `alloc_val` has exactly one use: a `GetIter(alloc_val)` → produces `iter_val`
4. `iter_val` has exactly one use: an `IterNext(iter_val)` or `IterNextUnboxed(iter_val)` or `ForIter(iter_val)` in a loop header
5. No `.send()`, `.throw()`, `.close()` uses of `alloc_val` or `iter_val`
6. `P` contains no `YieldFrom` op (delegation cannot be linearized in phase A)
7. `P` contains no `StateBlockStart`/`StateBlockEnd` (async generator — excluded)
8. `P` is not in the recursive set (call_graph.recursive_set())
9. `P`'s poll body is available in the same `TirModule`

**This is a module-level pass** — it must see both the caller and the poll function body. It runs in `module_phase.rs`'s `run_module_pipeline`, after the E1 inliner, before the per-function pipeline.

### The splice transform

For each candidate `(alloc_val, iter_val, loop_header)`:

**Step 1 — Analyze P's frame structure.**
Walk P's blocks finding all `StateYield(pair, next_state)` ops (there are N of them). Extract: the closure-slot reads (`ClosureLoad` ops) and writes (`ClosureStore` ops) — these become the loop-carried phi variables. Build a map `closure_slot_index → (type, init_value, phi_id)`.

**Step 2 — Mint SSA names for all frame slots + state.**
For each closure slot i referenced in P: `frame_slot_i_phi = fresh_value()`. Add a `state_phi = fresh_value()`.

**Step 3 — Clone P's blocks into the caller.**
Use the same `clone_function_body_with_fresh_ids` machinery from the inliner (same SSA-preserving clone with fresh ValueId/BlockId), with these overrides:
- P's entry block args (the `self` frame pointer) → replaced by the `frame_slot_*` phi values (no frame pointer in scope)
- `ClosureLoad(frame_ptr, slot_i)` → replaced by `frame_slot_i_phi` (a direct SSA value read)
- `ClosureStore(frame_ptr, slot_i, val)` → record `(slot_i, val)` as the phi update for slot_i on this yield's back-edge
- `StateYield(pair, next_state)` → this yield site becomes a "fusion splice point"
- `StateSwitch(frame_ptr)` → becomes a `Switch` on `state_phi`

**Step 4 — Wire the consumer loop.**
The caller loop structure is:
```
preheader: init frame_slot_i_phis, state_phi=entry_state
           Branch -> fused_dispatch_block
fused_dispatch_block(frame_slot_0..N, state_phi):
           Switch(state_phi) -> [yield_site_0_body, yield_site_1_body, ..., exhausted_block]
yield_site_K_body:
           [body of P up to yield K]
           [elem = pair.first; done = pair.second = False]
           Branch -> consumer_body_block(elem, frame_slot_updates_from_yield_K, next_state_K)
consumer_body_block(elem, frame_slots..., state_phi):
           [original consumer for-body ops, with elem bound to the for-target]
           [on continue: Branch -> fused_dispatch_block(updated_frame_slots, state_phi)]
           [on break: Branch -> loop_exit]
exhausted_block:
           Branch -> loop_exit
loop_exit:
           ...
```

**Step 5 — Delete the frame-creation ops.**
Remove `AllocTask(alloc_val)`, `GetIter(alloc_val → iter_val)`, the `IterNext`/`ForIter` ops. Remove any `IncRef`/`DecRef` on `alloc_val` or `iter_val`. The consumer body's RefCount on `elem` is preserved (the yielded value still has +1 ownership semantics from the old `STATE_YIELD` retain).

**Step 6 — Re-run the per-function pipeline on the merged caller.**
After splicing, run `run_pipeline(merged_caller, tti)`. SCCP fold-propagates the state_phi for single-yield generators (trivial 2-way switch → straight line). LICM hoists any loop-invariant sub-expressions from P's body. escape_analysis eliminates any remaining local allocs. This is the joint optimization that makes the fusion profitable.

## 4. Complete File-by-File Implementation Map

### Files to CREATE

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/generator_fusion.rs`**

This is the main pass. Structure:

```rust
//! Generator fusion pass — TIR coroutine-frame elision.
//! ...

pub struct FusionStats { pub frames_elided: usize, pub yield_sites_spliced: usize }

/// A recognized fusion candidate within a caller function.
struct FusionCandidate {
    caller_name: String,
    alloc_op_block: BlockId,
    alloc_op_idx: usize,
    alloc_val: ValueId,
    poll_func_name: String,      // P's name in the TirModule
    closure_size: i64,
    get_iter_block: BlockId,
    get_iter_op_idx: usize,
    iter_val: ValueId,
    loop_header_block: BlockId,
    iter_next_op_idx: usize,
    iter_next_opcode: OpCode,    // IterNext / IterNextUnboxed / ForIter
    elem_val: ValueId,
    done_val: ValueId,
    exit_block: BlockId,
    body_block: BlockId,
}

/// Closure-slot usage in P's poll function.
struct FrameSlotInfo {
    index: usize,
    /// Type inferred from the first ClosureLoad result or ClosureStore operand.
    ty: TirType,
}

/// A yield site extracted from P.
struct YieldSite {
    /// State ID (next_state value from StateYield attr).
    state_id: i64,
    /// The yielded pair value.
    pair_val: ValueId,
    /// Which block in P (remapped to caller) this yield lives in.
    callee_block: BlockId,
    /// Frame slot updates: (slot_index, new_value) at this yield point.
    slot_updates: Vec<(usize, ValueId)>,
}

pub fn run_generator_fusion(
    module: &mut TirModule,
    call_graph: &CallGraph,
    tti: &TargetInfo,
) -> FusionStats;

fn collect_fusion_candidates(
    caller: &TirFunction,
    module: &TirModule,
    call_graph: &CallGraph,
) -> Vec<FusionCandidate>;

fn is_poll_fusable(poll: &TirFunction, call_graph: &CallGraph) -> bool;

fn extract_yield_sites(poll: &TirFunction) -> Vec<YieldSite>;

fn extract_frame_slots(poll: &TirFunction) -> Vec<FrameSlotInfo>;

fn apply_fusion(
    module: &mut TirModule,
    candidate: &FusionCandidate,
    tti: &TargetInfo,
) -> bool; // returns true iff splice succeeded (may bail conservatively)
```

Key implementation details:

`is_poll_fusable` checks: no `YieldFrom`, no `StateBlockStart`/`StateBlockEnd`, not recursive in `call_graph`, entry block has no predecessor (same guard as the inliner), `closure_size` is statically known.

`extract_frame_slots`: walk every block in P, collect all `ClosureLoad`/`ClosureStore` ops; their `slot_index` attr (an `AttrValue::Int`) identifies the slot. Group by slot index. The type comes from the `ClosureLoad` result's `value_types` entry in P, or `DynBox` as the conservative fallback.

`apply_fusion`: the most complex function. Full algorithm:

1. Clone P's blocks into the caller using a variant of `clone_function_body_with_fresh_ids`. The caller frame pointer (the `alloc_val`'s post-alloc pointer) is NOT passed — instead, `ClosureLoad(frame_ptr, slot_i)` ops in the clone are replaced by `frame_slot_i_phi` values. `ClosureStore(frame_ptr, slot_i, val)` ops are collected as yield-point updates and removed from the ops list.

2. Allocate phi values: `state_phi`, `frame_slot_{i}_phi` for each slot i. These become block args on `fused_dispatch_block`.

3. Build `fused_dispatch_block(state_phi, slot_0, slot_1, ...)` with a `Switch` terminator. The switch targets are the cloned blocks corresponding to P's `state_id` entry points (each `StateYield`'s `next_state` attr).

4. For each yield site: wire the cloned block path up to the yield, extract the yielded element (the pair's `.0` — a `LoadAttr` or `Index(pair, 0)` op, or if `IterNextUnboxed` is the consumer, direct elem + done), then splice the consumer body using the existing loop body blocks unchanged. The consumer loop body's back-edge branches to `fused_dispatch_block` carrying updated slot values.

5. The `exhausted_block` (where P's last state returns `(None, True)`) branches to `loop_exit`.

6. Wire the preheader: before the original `alloc_val` definition, insert ops that define `frame_slot_i_phi` initial values. For each slot, the initial value is the value that P's entry block writes before the first yield — often a parameter or constant. If not statically determinable: conservative bail.

7. Delete `AllocTask`, `GetIter`, `IterNext`/`ForIter` ops. Update `loop_roles`, `loop_pairs`, `loop_break_kinds`, `loop_cond_blocks` on the merged caller to reflect the new loop structure.

8. Call `run_pipeline(caller, tti)` for joint optimization.

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/generator_fusion.rs` — conservative bail conditions (never miscompile)**

Return `false` (no change) if:
- The `ClosureStore` initial-value analysis cannot statically determine any slot's entry value (slot has no write in P's entry path before the first `StateSwitch`) — bail
- Any yield site's pair is not immediately destructured (a `CallMethod` on the pair, a `.send()` — means non-plain-for consumption) — bail
- P's body contains a `Raise` without a corresponding `TryStart`/`TryEnd` pair AND the consumer function has `has_exception_handling = false` — bail conservatively (the raise must propagate; we'll handle this in phase B)
- More than one `AllocTask` with the same poll name is in scope (multiple generator instances; phase A handles single-instance only) — bail
- Any other structural assumption violated — bail with zero change to the IR

### Files to MODIFY

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mod.rs`**

Add: `pub mod generator_fusion;`
After line 20 (`pub mod inliner;`).

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/module_phase.rs`**

In `run_module_pipeline` (currently at line 110), add the generator_fusion call after the E1 inliner:

```rust
// E1: function inliner
let _inline_stats = super::passes::inliner::run_inliner(module, &call_graph, &summaries, tti);

// E2/D1: generator frame elision (generator_fusion)
let _fusion_stats = super::passes::generator_fusion::run_generator_fusion(
    module, &call_graph, tti
);

// Rebuild after both transforms
let call_graph = CallGraph::build(module);
let summaries = ModuleSummaries::compute(module, &call_graph);
```

Note: rebuild the call graph once after BOTH transforms (the fused functions no longer contain `AllocTask` edges; the call graph must reflect this for the leaf-set the backends read).

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/escape_analysis.rs`**

Phase A prerequisite: `AllocTask` is already listed in `is_alloc_site` (line 196 in the file). The conservative escape path already marks `AllocTask` as `GlobalEscape` through the `ScfFor`/`AllocTask`/... arm at line 600. This is correct — an `AllocTask` that escapes must stay heap-allocated.

For phase B precision, the escape analysis needs to classify a non-escaping `AllocTask` (one consumed only by a local `GetIter` + loop) as `NoEscape`. This enables the fusion pass to fire without a separate single-use scan. However, for phase A correctness, the single-use check in the fusion pass itself (the recognition predicate) is sufficient. The escape_analysis change is a separate phase-B cleanup.

**`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/pass_manager.rs`**

No change needed for phase A. The generator_fusion pass is a MODULE-level pass in `module_phase.rs`, not a per-function pass in the pipeline. This is correct architecture: it requires seeing multiple functions simultaneously.

## 5. Soundness Argument

**Conservative-correct by construction.** The bail conditions enumerate every case where the splice could produce incorrect code. Each bail leaves the IR unchanged — the coroutine frame remains, the generator executes correctly via the runtime, and the only cost is a foregone optimization.

**SSA validity after splice.** The clone uses the same `clone_function_body_with_fresh_ids` infrastructure as the E1 inliner (already proven sound at commit `f14b196ce`). The frame-slot phis are well-formed: each is a block argument of `fused_dispatch_block`, every back-edge passes exactly the right count of updated slot values. Verified by `verify_function` called inside `run_pipeline` after the splice.

**Refcount balance.** `StateYield(pair, next_state)` does `inc_ref_obj(pair)` before returning (native backend, line 21785). After fusion, the pair's element (`elem`) is extracted from the pair value and used directly in the consumer body. The `IncRef` that protected the pair is matched by the `DecRef` that the consumer body's loop would already emit on scope exit. The pair itself (the `(value, False)` tuple) is eliminated: the fusion pass does NOT allocate a pair tuple — it binds the yielded value directly to `elem_val`. This means the pair's `IncRef` from the generator body must be dropped. Concretely: in the cloned `StateYield`'s replacement, instead of emitting the `inc_ref + store-state + return` sequence, we emit a direct branch to the consumer body with `elem_val` bound to the pair's first component. The refcount on `elem_val` comes from P's computation of the yielded value — which in P's source has +0 (borrowed) semantics as a pair component. The consumer body takes +1 ownership via the existing `IterNext` calling convention. Correctness: the fusion pass must emit an explicit `IncRef(elem_val)` at the yield splice point to replicate the `IterNext` calling convention's +1 transfer. This is the one explicit refcount op the fusion pass adds.

**Exception propagation.** For phase A: if P's body contains `CheckException` ops, the clone preserves them verbatim (the label remap from the inliner handles label collision). The consumer function's own `has_exception_handling` flag absorbs P's exception edges. A raise in P propagates through the now-inlined exception path to the consumer function's handler. This is byte-identical to the unfused case: the un-inlined `molt_generator_send` would propagate the exception via the C-stack into the caller's handler. Phase A bails on any P whose `has_exception_handlers()` is true (real try/except blocks) — this is the same conservative gate as the E1 inliner.

**Multi-backend correctness.** The fusion happens at TIR. The TIR→SimpleIR lowering (`lower_to_simple.rs`) and then SimpleIR→Cranelift / WASM / LLVM / Luau all see the fused CFG with no `AllocTask` and no `StateYield`. They treat it as an ordinary loop. No backend-specific changes needed.

**BigInt correctness.** Generator frame slots may carry `MaybeBigInt` values. The phi values for frame slots are typed `DynBox` unless proven otherwise by `value_types`. The `run_pipeline` re-run includes the full `unboxing` pass which promotes them to `RawI64Safe` only when the proof exists. Conservative-correct: no trusted-unbox on `MaybeBigInt`.

## 6. Legacy This Arc Deletes

**Phase 5 of this arc** (os.walk as Python generator) is the deletion milestone. When os.walk-as-Python compiles, fuses, and passes the perf gate:

- Delete `molt_os_walk` intrinsic from `runtime/molt-backend/src/intrinsic_symbols.rs` (and `generated.rs`, `wasm_abi_manifest.toml`/generated WASM ABI tables, `wasm.rs`, `simple_backend.rs` manifest tables, `gen_intrinsics.py`)
- Delete the native Rust os.walk implementation in the runtime crate
- The Python `os.walk` in `src/molt/stdlib/os.py` becomes the canonical implementation

The phase 5 deletion is gated on: the Python generator version fuses correctly (measured via `MOLT_DUMP_TIR`: no `molt_task_new` in the emitted code), passes all parity tests vs CPython 3.12/3.13/3.14, and meets the perf contract on all targets/profiles.

**Phase 6** (itertools): same pattern for `chain`, `islice`, `product`, etc. — retire each native intrinsic as the Python version fuses. Each is a separate deletion.

No legacy is deleted until the replacement is verified. No dual paths are introduced: the fusion pass either transforms (and deletes the frame-creation ops) or bails with zero change. There is no "hybrid" mode where both the coroutine frame and the fused loop exist simultaneously.

## 7. Test Plan

### Rust unit tests (in `generator_fusion.rs` `#[cfg(test)]` section)

**Test 1: `simple_generator_no_yield_not_fused`**
A TirModule with a poll function that contains zero `StateYield` ops. `run_generator_fusion` returns `frames_elided=0`. The caller is unchanged.

**Test 2: `single_yield_single_consumer_fused`**
Hand-build a minimal TirModule:
- Caller: `alloc_val = AllocTask("gen_poll", size=8)` → `iter_val = GetIter(alloc_val)` → loop header with `IterNextUnboxed(iter_val) → (elem, done)` → `CondBranch(done, exit, body)`
- `gen_poll`: entry → init slot 0 → `StateYield(pair_0, next_state=1)` → Return; resume_1 → `StateYield(pair_1, next_state=2)` → Return; state_2 → Return exhausted
- After fusion: `frames_elided=1`, no `AllocTask` op in caller, no `GetIter`, no `IterNextUnboxed`. Caller has a `Switch` on `state_phi`. `verify_function` passes.

**Test 3: `generator_with_escaping_frame_not_fused`**
Caller stores `alloc_val` into a list before the `GetIter`. `run_generator_fusion` bails (`frames_elided=0`).

**Test 4: `recursive_poll_not_fused`**
`gen_poll` has a `Call` back to itself. `is_poll_fusable` returns false.

**Test 5: `yield_from_poll_not_fused`**
`gen_poll` contains a `YieldFrom` op. `is_poll_fusable` returns false.

**Test 6: `frame_slot_phi_correct`**
A generator with one closure slot (slot 0) initialized to 0, incremented by 1 at each yield. After fusion the loop-carried phi for slot 0 is updated on every back-edge. Assert the phi update value is the incremented result.

**Test 7: `refcount_inc_ref_added_at_yield_splice`**
After fusion, the elem value has an `IncRef` op immediately before it enters the consumer body (the +1 ownership handoff). Assert `IncRef(elem_val)` is present.

**Test 8: `verify_function_passes_after_fusion`**
After `apply_fusion`, call `verify::verify_function` on the merged caller. Assert `Ok(())`.

### Differential tests (Python, in `tests/differential/basic/`)

**`gen_simple_for.py`**
```python
def gen():
    yield 1
    yield 2
    yield 3

total = 0
for x in gen():
    total += x
assert total == 6
```
Expected: byte-identical to CPython. `MOLT_DUMP_TIR=1` shows no `alloc_task` in the fused function.

**`gen_stateful.py`**
```python
def counter(n):
    i = 0
    while i < n:
        yield i
        i += 1

result = list(counter(5))
assert result == [0, 1, 2, 3, 4]
```

**`gen_early_break.py`**
```python
def naturals():
    i = 0
    while True:
        yield i
        i += 1

first_5 = []
for x in naturals():
    if x >= 5:
        break
    first_5.append(x)
assert first_5 == [0, 1, 2, 3, 4]
```
Correctness: break in consumer must exit cleanly, generator frame (now phis) must NOT be accessed after the break.

**`gen_exception_in_body.py`**
```python
def gen():
    yield 1
    yield 2

try:
    for x in gen():
        if x == 1:
            raise ValueError("hit")
except ValueError as e:
    pass
```
Expected: exception propagates correctly, byte-identical to CPython.

**`gen_exception_in_generator.py`**
```python
def gen():
    yield 1
    raise RuntimeError("from gen")

result = []
try:
    for x in gen():
        result.append(x)
except RuntimeError:
    pass
assert result == [1]
```

**`gen_bigint_yield.py`** — BigInt correctness
```python
def big():
    yield 1 << 60
    yield (1 << 60) + 1

vals = list(big())
assert vals[0] == (1 << 60)
assert vals[1] == (1 << 60) + 1
```

**`gen_multiple_frames.py`** — multiple independent generator instances in same scope
```python
def gen():
    yield 1; yield 2

a = list(gen())
b = list(gen())
assert a == b == [1, 2]
```

**`gen_nested_for.py`** — non-fused outer, fused inner
```python
def inner():
    yield 1; yield 2

result = []
for outer in [10, 20]:
    for x in inner():
        result.append(outer + x)
assert result == [11, 12, 21, 22]
```

**`gen_yield_from_not_fused.py`** — must stay unfused, still correct
```python
def sub():
    yield 1; yield 2
def gen():
    yield from sub()

result = list(gen())
assert result == [1, 2]
```

**`gen_send_not_fused.py`** — send() usage blocks fusion, still correct
```python
def gen():
    x = yield 0
    yield x + 1

g = gen()
assert next(g) == 0
assert g.send(10) == 11
```

### Cross-backend tests

All differential tests above must pass on native (Cranelift), WASM, LLVM, and Luau. Run with `python3 -m molt build --target <target>` for each.

## 8. Perf-Gate Plan

### Benchmarks

**`bench/bench_gen_simple.py`** (new)
```python
def gen(n):
    i = 0
    while i < n:
        yield i
        i += 1

N = 10_000_000
total = 0
for x in gen(N):
    total += x
print(total)
```
Expected: fused version eliminates `molt_task_new` and the per-iter pair alloc. Target: >= CPython 3.14 speed (CPython's `for x in gen(N): total += x` at ~100ms for N=10M). Verify via:
1. `MOLT_DUMP_TIR=1` — assert no `alloc_task` opcode in the compiled output
2. Wall-clock comparison: `time python3 bench_gen_simple.py` vs `time molt run bench_gen_simple.py`

**`bench/bench_gen_stateful.py`** (new)
```python
def fibonacci(n):
    a, b = 0, 1
    for _ in range(n):
        yield a
        a, b = b, a + b

total = sum(fibonacci(100_000))
```
Expected: no heap frame; frame slots `a` and `b` become loop-carried phis.

**os.walk benchmark** (phase 5 gate only)
```python
import os
total = 0
for root, dirs, files in os.walk("/usr/include"):
    total += len(files)
print(total)
```
Must be >= CPython on the native target before deleting `molt_os_walk`.

### Measurement protocol

For each benchmark:
- `cargo build --profile release-fast -p molt-backend --features native-backend` with `MOLT_SESSION_ID=gen-fusion-bench`
- All 4 backends: `--target native`, `--target wasm`, `--target llvm`, `--target luau`
- All 3 profiles: `release-fast`, `dev-fast`, `debug-with-asserts`
- Compare vs `uv run --python 3.14 python3 <benchmark>`

Perf gate: molt must be **strictly faster** on the simple generator benchmark on native/release-fast before the arc is declared complete. The frame-elision removes a `molt_task_new` allocation + N indirect calls — this should be a 5-30× win depending on body complexity.

## 9. Risk and Dependency Notes

### Blocked-by

**E1 inliner (DONE, commit `f14b196ce`):** The `clone_function_body_with_fresh_ids` primitive is reused verbatim. The generator_fusion pass must import it from `inliner.rs` or extract it to a shared `tir/util/clone.rs`. Prefer extracting: the clone primitive is independently useful and reduces coupling.

**S4 module_phase (DONE, commit `7915b29a0`):** `run_module_pipeline` already exists and runs the E1 inliner. Generator fusion slots in after E1 in the same function.

**E1 dormancy issue (ACTIVE):** The inliner is dormant on real code because `CheckException` sets `has_exception_handling` → `is_inlineable` returns false. Generator fusion has the SAME issue in a different form: the poll function will almost certainly have `CheckException` ops (from the universal exception-observation change `430e09793`). The fusion pass must use `has_exception_handlers()` (the narrow check for real TryStart/TryEnd regions), NOT `has_exception_handling` (which is set by any CheckException and is almost always true).

This is NOT a blocker for correctness — the bail is conservative. But it IS a blocker for the performance win on real code. The fusion pass will need to handle observation-only CheckException callees the same way the E1 inliner phase-c designs handle them: clone the callee's CheckException ops verbatim (they reference the callee's own exception-exit label, which gets a fresh id in the caller via `build_label_remap`). The phase-c design in `inliner.rs` (line 36-40 of the file) is the exact template for this.

**Recommendation:** Phase A of generator_fusion can use the strict `has_exception_handlers()` gate and deliver value for generators that are truly exception-free. Phase B extends to observation-only CheckException (same design as inliner phase-c). Do NOT slip Phase A on Phase B.

### Unblocks

- **os.walk fix** (open OOM/SIGSEGV): unblocked only by Phase 5 of this arc
- **itertools native retirement**: unblocked by Phase 6
- **deforestation.rs `is_impure` relaxation**: once generator_fusion runs first, the loop body may become pure (no more `IterNext` Call), enabling deforestation to fuse further — e.g., `sum(x for x in gen())` with a non-trivial `gen` body may fuse end-to-end

### Refusal policy

The pass bails conservatively in every unrecognized case — no IR change, no correctness risk. There is no process-global environment rollback for `run_generator_fusion`; a fusable shape must be optimized, and non-fusable shapes must be rejected by the pass predicates with deterministic IR.

### The SROA dependency question

The design doc mentions "general TIR inliner + general SROA as the Codon/LLVM recipe." The specialized generator_fusion pass avoids SROA by directly turning frame slots into SSA phis during the splice (rather than relying on mem2reg after allocation). This is sound: it is SROA for the specific `ClosureLoad`/`ClosureStore` pattern. A general SROA pass (E2 in the roadmap) remains separately valuable but is NOT required for this arc.

## 10. Phased Landing Sequence

Each phase is a COMPLETE STRUCTURAL PIECE. No phase leaves the codebase in a hybrid state.

**Phase A — Foundation splice (the keystone)**
Deliverable: `generator_fusion.rs` pass wired into `module_phase.rs`. Handles: single-yield and multi-yield generators with no `YieldFrom`, no real exception handlers (`has_exception_handlers()` = false), no escaping frames, local single-use `AllocTask`. Full differential tests for all shapes in section 7. Perf gate on `bench_gen_simple.py`.

Checklist:
- [ ] Extract `clone_function_body_with_fresh_ids` from `inliner.rs` to `tir/util/clone.rs` (or keep as `pub(crate)` in `inliner.rs` and import from there)
- [ ] Create `generator_fusion.rs` with `FusionCandidate`, `YieldSite`, `FrameSlotInfo` structs
- [ ] Implement `collect_fusion_candidates` (single-use `AllocTask` → `GetIter` → `ForIter`/`IterNextUnboxed` pattern)
- [ ] Implement `is_poll_fusable` with all conservative gates
- [ ] Implement `extract_frame_slots` and `extract_yield_sites`
- [ ] Implement `apply_fusion` with the 8-step splice algorithm
- [ ] Wire into `module_phase.rs::run_module_pipeline` after E1
- [ ] Register in `passes/mod.rs`
- [x] Keep generator fusion always wired; do not add a process-global bail gate
- [ ] All 8 Rust unit tests pass
- [ ] All differential tests pass on all 4 backends
- [ ] `cargo test -p molt-backend` passes (must be >= current count, 0 new warn)
- [ ] `MOLT_DUMP_TIR=1` shows no `alloc_task` for fused cases
- [ ] Perf gate: `bench_gen_simple` >= CPython on native/release-fast

**Phase B — Observation-only exception propagation**
Extend fusion to poll functions carrying `CheckException` (observation-only) but no real `TryStart`/`TryEnd`. Same design as E1 inliner phase-c: clone the `CheckException` ops, remap exception labels. This is the critical phase for real-world generators (nearly every generated Python function has `CheckException` after the universal exception-observation change). Perf gate: re-measure `bench_gen_simple` on a function whose poll has `CheckException`.

**Phase C — Multi-yield generators in loops (os.walk shape)**
Handle the `while stack: ... yield` pattern where the generator body itself has a loop containing yields. The `STATE_SWITCH` dispatch becomes the outer loop header; the generator's inner loop body is cloned wholesale. Verify on `gen_stateful.py` and the fibonacci benchmark. This phase is required for os.walk.

**Phase D — Lazy scandir primitive**
Implement a lazy `scandir` runtime iterator (the irreducible OS boundary). `os.scandir()` currently returns an eager list; make it a proper lazy iterator at the runtime/intrinsic level. This is a small runtime change with no TIR impact, but required for os.walk-as-Python to be non-OOM on large directories.

**Phase E — os.walk as pure Python**
Write `os.walk` in `src/molt/stdlib/os.py` as a CPython-verbatim iterative generator (work-stack, top-down + bottom-up, in-place `dirnames` pruning, `onerror`, `followlinks`). Verify: (a) Phase A/B/C fusion fires on it (`MOLT_DUMP_TIR` shows no `alloc_task`), (b) all parity tests pass vs CPython 3.12/3.13/3.14, (c) perf >= CPython on the walk benchmark across all targets, (d) no OOM on a large directory tree, (e) no SIGSEGV on a deep tree. ONLY AFTER (a-e) pass: delete `molt_os_walk` from `intrinsic_symbols.rs`, `generated.rs`, `wasm_abi_manifest.toml`/generated WASM ABI tables, `wasm.rs` manifest tables, `simple_backend.rs`, `gen_intrinsics.py`.

**Phase F — itertools retirement (long tail)**
For each itertools native (chain, islice, product, compress, dropwhile, takewhile, cycle, repeat): write the Python generator equivalent, verify fusion, verify perf parity, delete the native intrinsic. Each is independent; prioritize by usage frequency.
