# molt Optimizer Foundation — the engine for "Mojo/Julia/Codon/Nuitka, but better"

Status: NORTH-STAR ARCHITECTURE (2026-06-03). Supersedes the narrower framing in
`generator_fusion.md` (generator fusion is now a *consequence* of this foundation, not the
architecture). Origin: user vision — "bleeding edge, like Mojo and Julia but better and faster;
like Codon but more ecosystem compatibility / language support / flexibility; like Nuitka but
way faster and even more flexible."

## What "better than each" technically requires
- **vs Nuitka** (full CPython compat, but slow — inherits CPython's runtime/object model via
  libpython): molt keeps the compatibility but has its OWN runtime, so the win comes from an
  optimizer that escapes the object model — **SROA** removes the per-operation allocation, the
  **inliner** removes the call barrier. Tiny standalone binaries, no libpython.
- **vs Codon** (fast AOT Python, generator inlining, but a restricted dialect — weak C-extension /
  package ecosystem, less dynamism): match its codegen + generator elision while keeping FULL Python
  semantics + ecosystem reach. Same engine (inline + coro-elide + specialization), fewer compromises.
- **vs Julia** (world-class numerical speed via specialization + inlining + escape analysis feeding
  LLVM, but JIT warmup + its own language): molt's typed-IR/`Repr` work is the specialization half;
  the **inliner + SROA** are the other half — delivered AOT (no warmup) over the Python ecosystem.
- **vs Mojo** (MLIR progressive lowering, ownership, SIMD/GPU, zero-cost abstractions, but NOT real
  Python): molt already has progressive lowering (TIR→SimpleIR→Cranelift/LLVM/wasm/luau), `vectorize`/
  `polyhedral`, and the tinygrad GPU lane — on REAL Python. SROA supplies the zero-cost abstractions.

All four converge on one root cause and one fix.

## The structural gap (measured 2026-06-03)
- `run_pipeline(func: &mut TirFunction)` is **strictly per-function** (passes/mod.rs:72; called
  per-function at main.rs:2199, wasm.rs:2116, parallel.rs:28). There is **no whole-program pass
  phase** — molt has inter-procedural *summaries* (`compute_return_alias_summaries`) but no body-level
  inter-procedural access. **Every call is an optimization barrier.**
- molt has **no function inliner** and **no mem2reg/SROA** (pass list passes/mod.rs:156-229).
- Consequence: Python's call-heavy, allocation-per-operation style can't be optimized across
  boundaries → the project is pushed toward hand-written native objects per stdlib function (the
  CPython C-extension treadmill) — the exact thing this foundation exists to make unnecessary.

The inliner + SROA/mem2reg are, in LLVM and Julia, THE load-bearing passes: they're what make every
other pass (GVN, LICM, SCCP, escape analysis, the `Repr` specialization) pay off across call and
allocation boundaries. molt has the downstream passes already; it's missing the two that feed them.

## The foundation (build order — each a complete, perf-gated, differential-tested piece)
1. **Inter-procedural pass phase + call graph.** A whole-program step before the per-function
   pipelines, with access to all `TirFunction`s + a call graph (call site → callee). Reuse the
   existing summary precedent. This is the architectural unlock; the inliner and all future IPO
   live here. (Driver: main.rs ~2199 + wasm.rs ~2116 collect funcs; add the phase before the loop.)
2. **Function inliner** — the bridge from inter- to intra-procedural. CONSERVATIVE-CORRECT first cut:
   inline only small, non-recursive, directly-called leaf functions with simple CFGs; profitability +
   size-growth gated; recursion-cycle-safe; identical semantics (params→args, return threading, value/
   block-id remapping, refcount/exception correctness). Then EXPAND by measurement. All backends. This
   alone is likely molt's single biggest perf lever (kills call overhead + unlocks cross-call opt).
3. **mem2reg / SROA.** Promote non-escaping memory + aggregates (stack slots, non-escaping heap
   objects, tuples/small structs, coroutine frames) to SSA values. Drives the allocation tax to zero.
   **Coroutine-frame elision lives here** (generator frame slots → loop-carried phis; SCCP folds the
   resume `STATE_SWITCH`). Built on the existing `escape_analysis` (extend it to `AllocTask`/`Alloc`).
4. **Specialization / monomorphization.** Deepen the typed-IR/`Repr` convergence with inlining so
   call sites specialize on argument repr (Julia-class type-stable native code), AOT.

## Consequences (fall out of the foundation — not separately engineered)
- **Generator fusion** = inline the generator `poll` (pass 2) + elide the frame (pass 3) = exactly
  LLVM's `CoroElide`. The earlier bespoke `generator_fusion` devirt pass is RETIRED in favor of this
  (it would have reimplemented half an inliner anyway). os.walk-as-Python is the *proving ground*.
- **Stdlib stays Python.** Stdlib bodies inline into user code + SROA the allocations → no native
  iterators. Retire the bespoke itertools/os.walk natives → Python.
- **os.walk** rewritten as the CPython-verbatim Python generator (iterative work-stack; top-down +
  bottom-up; in-place `dirnames` pruning + `onerror` + `followlinks`), fast via fusion — fixing its
  eager-OOM + deep-tree-SIGSEGV in Python, once passes 2+3 prove ≥ CPython on the walk benchmark.

## Engineering discipline (bleeding edge ≠ reckless)
The best inliners/SROA (LLVM, Julia) were built as conservative-correct first cuts that EXPAND by
measurement and NEVER miscompile. Hold that bar: absence of an optimization is a perf bail, never a
correctness change; differential-test every shape against CPython; the perf contract (≥ CPython on
every benchmark × target × profile) gates each landing. No half-passes on main — each phase lands
complete or leaves a clean baton.

## Risk / honesty
Passes 1–3 are genuinely multi-week, high-correctness-risk foundational compiler work (the inliner's
remap + the frame-SROA's SSA reconstruction are the hard parts). This is the correct investment: it's
the engine the entire 5-year vision runs on, and it subsumes the per-function-workaround approach.
Until pass 3 lands, os.walk's eager-OOM + deep-tree-SIGSEGV remain open (accepted; no native stopgap).
