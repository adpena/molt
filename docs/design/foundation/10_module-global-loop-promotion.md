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
