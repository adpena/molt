# Generator Fusion — the keystone that keeps molt's stdlib in Python

Status: DESIGN (2026-06-03). Author: autonomous session. Seeds a multi-session arc.
Origin: user reframe "What would Chris Lattner do / what does Codon do" — reject hand-written
native stdlib iterators (the CPython C-extension treadmill); make the COMPILER good enough that
idiomatic Python generators compile to tight native loops, exactly as Codon ships its whole
stdlib in Python. User chose "Deep fix only": discard the native os.walk iterator (done — tree at
HEAD 934938665); build generator fusion; rewrite os.walk as the CPython-verbatim Python generator.

## The principle
An AOT Python compiler must not escape Python's perf problems by hand-coding native objects per
stdlib function. That doesn't compose, multiplies `unsafe` surface, and re-erects the native/Python
boundary the compiler exists to erase. Codon ships its entire stdlib in (typed) Python: its
generators lower to LLVM coroutines but are INLINED into for-loop consumers and the frame is elided
(LLVM `CoroElide`), so the coroutine compiles away. molt must do the equivalent at TIR (so ALL
backends — Cranelift/LLVM/wasm/luau — benefit, not just LLVM).

## Grounded findings (2026-06-03 source trace; cite when implementing)
- **genexpr and `def`-yield generators share ONE representation**: both → a `poll_func` +
  `ALLOC_TASK(task_kind="generator")`. Frontend: `visit_FunctionDef` frontend/__init__.py:30765-31043
  (def-yield), `visit_GeneratorExp` :14198-14347 (genexpr — builds `ast.Yield` and uses the SAME
  `visit_Yield` path :32478). Generator detection: `_function_contains_yield` :2837.
- **`yield v` lowering** (`visit_Yield` :32478): `TUPLE_NEW([v, False])` → `STATE_YIELD(pair,
  next_state_id)` (writes pair to return slot, stores resume state in frame, returns). Frame control
  area = `GEN_CONTROL_SIZE=48` bytes (:247); locals persisted at `async_locals_base + i*8`; entry
  has `STATE_SWITCH` dispatch (:30922). `.send`/`.throw` slots at GEN_SEND/GEN_THROW offsets.
- **Frame alloc at the call site**: `ALLOC_TASK` → native `molt_task_new(poll_addr, closure_size,
  TASK_KIND_GENERATOR)` (function_compiler.rs:32134) — heap frame.
- **Consumer side**: `iter = GetIter(g)`; loop header `IterNextUnboxed(iter) -> (elem, done)` +
  `CondBranch(done, exit, body)` (or `ForIter`). Each `IterNext` = `molt_generator_send(g, None)`
  → calls `poll(frame)`.
- **TIR opcodes** (tir/ops.rs): `AllocTask`, `StateSwitch` (:119), `StateYield` (:121), `Yield`
  (:126), `YieldFrom` (:127), `GetIter`, `IterNext`, `IterNextUnboxed`, `ForIter`.
- **Why deforestation doesn't fuse def-yield**: it matches `GetIter→ForIter→CallBuiltin(fusable)`
  with a loop body that has no generated fusion barriers (`op_kinds.toml`
  `fusion_barrier_opcodes` via `opcode_is_fusion_barrier_table`). That fact is
  distinct from `side_effecting`/`may_throw`: fusion preserves per-element
  evaluation order, but opaque calls, closure state, coroutine state, imports,
  raises, and yields still block fusion. A coroutine consumer loop calls
  `IterNext`/state machinery through barrier-class ops and is never fused.
  The frontend `sum/any/all(simple genexpr)` fast-path (`_try_emit_inline_sum_genexpr` :14006,
  called :23177) inlines BEFORE coroutine-ization — but only for those builtins, only same-module.
- **NO TIR function inliner; NO mem2reg/SROA** (pass list passes/mod.rs:156-229: range_devirt,
  iter_devirt, deforestation, loop_unroll, canonicalize, unboxing, block_versioning, gvn, licm,
  escape_analysis, refcount_elim, reuse_analysis, dead_store_elim, sccp, strength_reduction,
  fast_math, branchless_count, bce, vectorize, polyhedral, copy_prop, dce). SCCP + escape_analysis
  exist; inliner + SROA do not.
- **Whole-program TIR**: cli.py `_build_module_code_ops` assembles ALL modules (stdlib + user) into
  ONE IR; every function body (incl. os.walk's poll) is visible to backend/TIR passes. So
  cross-module generator inlining is possible at TIR WITHOUT building LTO. (Frontend AST inlining is
  NOT cross-module — frontend visits one module at a time.) → **the fix lives in TIR.**
- **escape_analysis.rs**: per-ValueId lattice {NoEscape,ArgEscape,GlobalEscape}; tracks `Alloc`/
  `ObjectNewBound` only; `AllocTask` is conservatively GlobalEscape (:526). Must extend to track
  generator-frame escape.

## Architecture decision: a TIR `generator_fusion` devirt pass (NOT frontend, NOT general inliner+SROA)
Rejected alternatives:
- **Frontend AST inlining** (generalize the genexpr fast-path): can't cross module boundaries
  (os.walk is stdlib, consumer is user code); AST-level body-splicing with break/continue/try-finally
  is messy and unprincipled. Reject.
- **General TIR function inliner + general heap-SROA** (the literal Codon/LLVM recipe): correct and
  generally valuable, but two large from-scratch passes. Defer the *general* versions; they're a
  separate roadmap item.
Chosen: a **specialized `generator_fusion` TIR pass** that does inline+frame-elision *fused* for the
recognized shape `for x in <non-escaping, non-recursive, plain-for-consumed generator>` — the same
spirit as `iter_devirt` (specialized GetIter+IterNext fusion for lists). Fits molt's devirt-pass
pattern, one bounded pass, reuses CFG/SSA infra, all backends benefit. This is the right-sized
realization of "generator inlining" for molt.

### Recognition (when the pass fires)
For a `GetIter(g)` whose loop the pass can identify:
1. `g` is produced by `AllocTask(task_kind="generator", poll=P)` in the SAME whole-program IR
   (P's body is available).
2. `g` does NOT escape: its only uses are the `GetIter` (and the `IterNext`/`ForIter` driven by it)
   — no store to a var that outlives the loop, no pass-as-arg, no return, no `.send`/`.throw`/`.close`
   call, no second consumer. (Phase A: a conservative local single-use check; Phase B: extend
   escape_analysis to AllocTask for precision.)
3. P is NOT recursive and contains NO `YieldFrom` (yield from delegation can't be linearized cleanly
   — fall back). [Later phase may handle `yield from` over a fusable inner generator by recursive splice.]
4. P does not depend on a sent value (the `for` consumer always sends None) and no `.throw` path is
   reachable — both true for plain-for consumption.
If any condition fails → leave the coroutine frame as-is (correct, just not fused). Emit a `log`/stat
so we can measure fall-back frequency (no silent caps).

### The splice transform (inline + frame-elision, fused)
Clone P's CFG into the consumer function and weave it into the consumer loop:
- **Frame slots → SSA**: every persisted local (closure slot read/written across a yield) becomes a
  **loop-carried phi** in the consumer loop. The `STATE_SWITCH` resume-state becomes a loop-carried
  phi too (init = entry state before the loop). No `AllocTask`, no heap frame.
- **`STATE_YIELD(pair, next_state)`** → the yielded value is the loop element: bind the consumer's
  for-target to it, run the consumer loop body, then on the back-edge set the state-phi = next_state
  and re-enter P's dispatch (which jumps to the resume block). I.e. the consumer body is spliced at
  the yield point; the generator body is the outer loop.
- **Generator exhaustion** (`poll` returns `(None, True)` / falls off the end) → loop exit edge.
- **`STATE_SWITCH`** stays as the resume dispatch on the state-phi; for a single-yield generator it's
  a trivial 2-way branch that SCCP/jump-threading straightens. (Folding it is polish, not required.)
- Delete `AllocTask`, `GetIter`, the `IterNext`/`ForIter` op; rewrite refcounts (the per-iter
  `(elem,done)` pair is scalar-replaced — it's destructured immediately).
- Correctness must preserve: exception semantics inside P (a raise mid-body propagates exactly as the
  unfused version would — reuse the needs-exception-stack discipline; see [[project_iter_consume_hang_baton]]),
  refcount balance of P's locals across the now-phi'd boundary, and `break`/`continue`/`return` in
  the consumer body (map to loop exit / back-edge as today).

### Pipeline position
After `iter_devirt` (so list-source generators are already devirted) and BEFORE `deforestation`
(so a fused generator loop with a now-pure body can additionally fuse into `sum/list/...` consumers)
and before `gvn`/`licm`/`escape_analysis` (so the promoted phis get optimized). Candidate slot:
passes/mod.rs between :157 (iter_devirt) and :158 (deforestation), as `run_pass!("generator_fusion", …)`.

## Phases (each a complete structural piece; land green before the next)
1. **escape_analysis → AllocTask** (prereq, small, testable): add `AllocTask` to `is_alloc_site`;
   classify a generator frame whose only uses are a single local `GetIter`+loop as `NoEscape`. No
   behavior change yet — but unit-tested escape facts. (Lands as infra; pair with Phase 2 so it
   delivers value, per the no-orphan-infra rule.)
2. **`generator_fusion` pass — recognition + splice for the single-yield, single-loop, non-escaping
   case** (the keystone; large). Covers the dominant shape incl. an iterative os.walk. CFG-splice +
   frame→phi. Extensive differential tests (generator semantics preserved: ordering, early break,
   exception-in-body, refcount/no-leak) + perf: a hot `for x in gen()` must show the frame alloc +
   IterNext call eliminated (inspect TIR/asm) and beat the coroutine version.
3. **Multi-yield / loop-in-generator generalization** (os.walk's `while stack: … yield` has the yield
   inside a loop — verify Phase 2 handles it; if not, generalize the state-weaving).
4. **Lazy native `scandir` primitive + DirEntry scalar-replacement**: make scandir a lazy iterator
   (the irreducible OS boundary), and let escape analysis + scalar-replace remove the per-entry
   DirEntry inside os.walk. (scandir is currently EAGER → DirEntry list.)
5. **os.walk = CPython-verbatim Python generator** (iterative work-stack, top-down + bottom-up,
   honoring in-place `dirnames` pruning + `onerror` + `followlinks`). Verify it FUSES (Phase 2/3)
   and meets the perf contract (≥ CPython on the walk benchmark, all targets/profiles) BEFORE
   deleting the native `molt_os_walk` intrinsic (+ manifest/generated.rs/wasm tables, gen_intrinsics.py).
   This finally fixes the OOM (laziness) + deep-tree SIGSEGV (no native recursion) — in Python.
6. **Retire the bespoke itertools natives** (chain/product/islice/… — debt by the same standard):
   re-express as Python generators once fusion is proven. (Long-tail; lower priority.)

## Fallbacks (must stay correct, just unfused — never miscompile)
generator escapes / stored / returned / multi-consumer / `.send`/`.throw`/`.close` / recursive /
`yield from` / async generator → keep the coroutine frame. The pass is an OPTIMIZATION; absence of
fusion is a perf bail, never a correctness change.

## Verification contract
- Per phase: green build (0 warn, native+wasm+llvm), backend lib tests, differential parity.
- Phase 2 acceptance: a microbench `for x in mygen(): acc += x` shows (a) no `molt_task_new` in the
  emitted code, (b) no per-iter pair alloc, (c) wall-clock ≥ the equivalent hand-written loop and
  ≥ CPython. Inspect via `MOLT_DUMP_TIR`/asm.
- Phase 5 acceptance: os.walk-as-Python ≥ CPython on a walk benchmark across native/wasm/llvm, deep
  tree (no overflow), large tree (no OOM — lazy), in-place pruning + onerror + followlinks parity
  vs CPython. Only THEN delete the native intrinsic.

## Risk / honesty
- This is multi-week. The two hard parts are the CFG-splice + frame→phi SSA reconstruction (Phase 2)
  and getting exception/refcount semantics byte-identical to the coroutine version. High correctness
  risk → differential-test heavily; fall back conservatively.
- Until Phase 5 lands, os.walk's eager-OOM + deep-tree-SIGSEGV REMAIN OPEN (user accepted this under
  "Deep fix only"). Do not re-introduce the native iterator as a stopgap.
- A general TIR function inliner + general heap-SROA are separately valuable; if Phase 2's bespoke
  splice proves too gnarly, escalate to building those general passes (the literal Codon/LLVM recipe)
  — but try the specialized devirt pass first (matches molt's pattern, smaller).
