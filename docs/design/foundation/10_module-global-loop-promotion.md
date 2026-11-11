<!-- Foundation design: module-global loop promotion (bench_sum 16× root cause). Authored 2026-06-04 from direct measurement; supersedes the bug-#15 baton's attribution of bench_sum. -->

# Module-global loop promotion — the bench_sum 16× root cause and its structural fix

## The measured decomposition (2026-06-04, native release, 5 samples, min wall)

| Variant | molt | CPython 3.14 | ratio |
|---|---|---|---|
| `bench_sum.py` (module-level loop, 10M, sum < 2^46) | 9.85s | 0.61s | **16× SLOWER** |
| same loop in a function (locals), 10M | **0.046s** | 0.211s | **4.6× FASTER** |
| function locals, 30M (sum > 2^47) | 1.254s | 0.565s | 2.2× slower (**bug #15**, separate) |

bench_sum's accumulator tops out at ~5×10^13 < 2^46 — it NEVER crosses the NaN-box
inline limit. **The 16× is not the bug-#15 cliff.** The loop chunk's SimpleIR shows
every loop-carried variable (`N`, `total`, `i`) is a **module attribute**: each
assignment is a `module_set_attr`, each read a module-attr load, plus a
`check_exception` after each — per iteration. Function-local code is already 4.6×
FASTER than CPython, so the entire deficit is module-global traffic (boxed
attr-store/load + RC churn ≈ 200× the cost of a register-carried local iteration).

Bug #15 proper (the >2^47 boxed-accumulator cliff; dual-loop peel design in
[[project_loop_iv_osc_15_baton]]) is real but SECONDARY: 2.2×, and only above 2^47.

## The structural fix: scalar promotion of module slots across loops

Classic register promotion (LICM store/load promotion), instantiated for Python
module-dict slots, at the TIR level (backend-neutral):

For each natural loop (the S1 `LoopForest`) and each module-attr slot
(`ModuleGetAttr`/`ModuleSetAttr` on the module object with a constant name) that is
read or written in the loop:

1. **Preheader**: load the slot once into an SSA value.
2. **Loop**: carry it as a header block argument (phi). In-loop reads use the
   carried value; in-loop writes redefine it. The get/set ops disappear from the
   body.
3. **Normal exits**: store the carried value back once per written slot.
4. **Exception exits (the Python-specific obligation)**: every `CheckException`
   edge that leaves the loop gets a **compensation landing block** that stores the
   carried values live AT THAT POINT before continuing to the original handler
   target — an exception handler (or another module observing after propagation)
   sees exactly the values as-if every iteration had stored. This is the same
   compensation-code discipline as deoptimization state.

### Soundness obligations (each fail-closed)

- **No other observers in the loop**: refuse promotion if the loop body contains
  any op that can read/write the module dict or run arbitrary code — `Call`,
  `CallFunc`, `CallMethod`, `CallBuiltin` (unless proven-pure via the effects
  oracle), `Import`, `ModuleCacheGet/Set/Del`, `globals()`-style reflection, or a
  `ModuleGetAttr/SetAttr` with a NON-constant name. First cut: allow only
  effects-oracle-pure ops + `CheckException` + the promoted slots' own get/sets.
- **Aliasing**: `ModuleSetAttr` with a dynamic name, or a second module object
  value, refuses promotion of all slots (conservative).
- **Concurrent observers (threads)**: CPython permits another thread to observe
  module globals mid-loop. Promotion across 10M iterations changes what a polling
  thread sees. Promote ONLY when threading is provably unreachable program-wide —
  the static import/intrinsic reachability already computed for the resolver
  manifest (no `thread.*` / `threading` reachable ⇒ no concurrent observer can
  exist). Fail-closed: threading reachable ⇒ no promotion (the loop keeps its
  per-iteration stores). This keeps the documented Tier-0 determinism contract
  intact with zero relaxation.
- **The exception-exit values**: compensation stores use the SSA values live at
  the specific `CheckException`'s program point (not the loop-entry or loop-exit
  values) — byte-identical observable state vs the unpromoted loop.

### Why this is the right altitude

- It is the module-scope analogue of what already makes function bodies 4.6×
  faster than CPython (locals in registers/SSA). Module-level scripts — the
  default shape of benchmarks and small programs — currently fall off a 16×
  cliff for lack of it.
- It composes: after promotion, the carried value is an ordinary SSA loop phi, so
  the EXISTING RawI64Safe promotion / value-range / (future) bug-#15 peel apply
  unchanged. Promotion turns the module-level loop INTO the function-local shape
  the rest of the optimizer already handles.
- No new backend work: the transform is pure TIR (ops in, ops out); all backends
  inherit it through the standard lowering.

### Implementation sketch

- New TIR pass `module_slot_promotion` (tir/passes/), running after lift +
  refine, before LICM (so LICM/value-range see the promoted phis). Pass class
  `Mutates::Cfg` (it adds compensation blocks + rewrites exception-edge labels to
  land in them).
- Uses: S1 `LoopForest` (natural loops), S3 effects oracle (op purity),
  `label_id_map` + fresh labels for the compensation blocks (the same
  exception-label machinery the E1 inliner uses — `build_label_remap` precedent).
- Program-level threading-reachability bit: threaded in like the cost model
  (computed once per module build from the resolved-module set / intrinsic
  manifest; `thread`/`threading`/`_thread` absent ⇒ promotable).
- Differential gates: module-level loop (values + prints), exception thrown
  mid-loop by an arithmetic op observing correct intermediate globals in the
  handler, another module importing and reading the globals after, threading
  present (promotion refused — behavior byte-identical), bigint crossing 2^46
  inside a promoted loop (composition with the boxed path).
- Perf gate: bench_sum ≥ CPython (target: the function-local 4.6× once promoted +
  under-2^46), no regression elsewhere.

## Sequencing vs bug #15

1. **This pass first** — it converts module-level loops to the local shape (the
   16× → expected ~4.6× FASTER for bench_sum, which stays under 2^46).
2. **Bug-#15 dual-loop peel second** — it then covers ALL loops (local or
   promoted) whose accumulators genuinely exceed 2^47 (the remaining 2.2×),
   per the corrected design in the baton (raw-i64 carrier + branchless overflow
   check + boxed continuation; the bare-iadd-wraps-at-2^63 trap documented there).

## Status 2026-11-11: promotion across the poll, by loop versioning

From 2026-09-14 to 2026-11-11 the pass refused every module-level loop. Measured with
`MOLT_PROMOTE_DEBUG=1` on `bench_sum`, `bench_while`, `bench_float` and `bench_math_numeric`
(LLVM release, macOS arm64), four refusals stacked. Each is fixed at its source.

1. **`ModuleGetGlobal` was a wildcard.** Module-level code reads every global through
   `ModuleGetGlobal(root, const name)`, and the pass skipped any function that held one. A
   const-named read on the module root is a keyed read. It differs from `ModuleGetAttr` in one
   way: a miss falls back to builtins. So a hoisted load keeps the read's opcode, and a written
   slot is refused when its entry value comes from such a read (its store-back would publish a
   builtin in the module's namespace). Role table: `module_slot_access_roles` in `op_kinds.toml`.
2. **The asynchronous-work poll.** Every loop latch carries a `CheckException` fused with the
   poll. Serviced work is Python code (a signal handler) that can read and write module
   globals, so no slot may sit in a register across it. See the design below.
3. **`FrameSetLocation` stopped the entry-seed scan.** The op stores a location id in the frame
   record. Its opcode row did not say so, and the alias oracle classed it as a write to any
   heap region, the module dictionary included. The row is now `may_access_arbitrary_heap =
   false` and memory-inert.
4. **Dead name constants.** The `ConstStr` that named a deleted access stayed in the loop and
   built and released a string on each iteration. The pass deletes a name constant whose only
   readers were the accesses it deleted.

Two more defects kept a promoted loop slow or unbuildable, and are fixed with it:

- The loop guard was `Bool(Lt(i, n))`. After promotion `Lt` is an exact bool, and
  canonicalization now folds `Bool` of an exact bool to its operand, so the guard is the
  comparison the counted-loop and range recognizers read.
- The counted-loop body path refused any conditional branch. A branch with exactly one target
  in the loop (`if x: break`, the pending-work test) is a conditional exit: it only shortens
  the run, so the recurrence still bounds the induction variable. Loop unrolling refuses such
  a body; the range analysis uses it.

### Design for the poll: leave the promoted loop when work is pending

Rejected: store, poll, then reload into the carried values. A reloaded value is `DynBox`. It
joins the exact scalar at the latch, the header phi widens, and the loop loses the raw
arithmetic that promotion exists to enable. It also breaks the pass's rule that every writer
of a carried slot has producer-derived scalar identity.

Built: loop versioning, the deoptimization exit of HotSpot and V8 (uncommon trap, OSR exit).
`version_loop_at_polls` in `module_slot_promotion/promote.rs`:

```text
latch:  ...                       ; the polls are plain exception checks
        %p = AsyncWorkPending     ; one volatile load of the pending-work word
        cond_br %p, ^stores, ^back
back:   br ^header(args, slots)   ; the loop's one unconditional back edge
stores: ModuleSetAttr ...         ; the dirty slots, this iteration's values
        br ^slow_header(args)     ; the loop as written: dictionary traffic and polls
```

- The loop is cloned before promotion. The clone keeps every module access and every poll and
  carries no loop metadata. The original loses its polls and gains one `AsyncWorkPending` test
  per back edge.
- The edge into the clone leaves the promoted loop, so the pass's existing exit machinery
  stores the dirty slots on it. No reloaded value enters the promoted loop; its types stay
  exact.
- The clone polls at its own back edge, one iteration later, and never returns to the
  promoted loop. Pending work is rare; after one serviced signal the loop runs unpromoted
  until it exits. Re-entry needs typed guards on reloaded values and is a later step.
- `AsyncWorkPending` is a TIR op (Bool, cannot raise, pinned at its program point). Cranelift,
  LLVM and the WASM op loop lower it inline beside their poll; the WASM LIR lane falls back as
  it does for a poll.
- Poll placement (`async_work_poll.rs`) puts no poll on a back edge that is taken only when
  `AsyncWorkPending` is false. Without that rule the re-optimization after promotion would
  mark the promoted loop's latch again.
- Refused, loop left as written: a loop value that code after the loop reads directly (two
  copies would need a merge), a loop block that is an exception target, a conditional back
  edge.

Measured, LLVM release, macOS arm64, instructions retired, output identical: `bench_sum`
36.1 G to 0.37 G (CPython 3.12.16: 15.0 G).

Not covered: a loop that calls a function. A call can touch any global, so the call stays a
barrier. Module slot arrays (`call_anatomy.md` section 4.5) make the unpromoted access cheap
and are the general fix; they also cover global reads inside functions.
