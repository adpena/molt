<!-- Foundation blueprint 63. Arc: DEFORESTATION / FUSION — and eliminating the
DEFORESTATION KILLERS. End-state: Python's high-level data-flow (comprehensions,
generator expressions, itertools chains, map/filter/zip/sum pipelines) compiles to
ZERO-INTERMEDIATE-ALLOCATION fused loops. The end-state IR fact makes "this chain
allocated an intermediate" an UNEXPRESSIBLE class. DEEPENS doc 53 (perf compression
ladder) Rung 6 — this is its deforestation sub-arc made first-class.
Author: portfolio-architect. Date: 2026-06-24. Status: DESIGN ONLY / EXECUTABLE PLAN.
No code written by this doc; the lead integrates.

NUMBERING: assigned path 63_deforestation_fusion.md. Slots 60/61/62 are sibling
portfolio arcs for tree-shaking, size, and startup; 64-67 carry the measurement,
performance, CPython-compat, and tinygrad/DFlash arcs. Kept 63 per assignment.

All file:line anchors were verified read-only against the worktree snapshot
available on 2026-06-24. Code beats this doc when it drifts — re-verify against
current files and executable tests before acting. -->

# 63 — Deforestation / Fusion: the zero-intermediate-allocation data-flow plane

> **Status: EXECUTABLE PLAN (design only).** This arc DEEPENS one lever of doc 53
> (the Performance Compression Ladder): **Rung 6** ("Resumable-frame ownership +
> generator fusion"). Doc 53 §3 Rung 6 *names* "fusion eligibility — a
> `FusionBarrier`/`no_heap_move` fact … extended to def-yield generators so
> producer/consumer loops fuse" but does not derive it. This document is that
> derivation, made first-class: the **fusion-eligibility fact family**, the
> enumerated **deforestation killers** (the barriers that block fusion) and the
> structural mechanism to remove or hoist past each, across all four backends
> (native/LLVM/WASM/Luau) and all profiles (dev-fast/release-fast/release-output).
>
> **Cross-arc dependency (stated once, not re-derived):** this arc consumes the
> **ownership lattice** (doc 53 Rung 1 / doc 55 / `ownership_lattice_min.rs`) for the
> escape and borrow facts that decide whether a fused chain's intermediate is
> droppable-on-the-stack; the **`FactValue` confidence lattice** (`call_facts.rs:117`)
> as its soundness substrate; the **CallFacts IP summaries** (doc 53 Rung 2 /
> doc 47 / `call_facts.rs`) for the cross-call fusion case; and the **op_kinds.toml
> generated authority** (doc 25 / doc 59) as the home of every per-op fusion fact.
> It does NOT re-specify those mechanisms; it adds the fusion-specific facts that ride
> on them and names the producer→consumer edges.

---

## 0. End-state outcome (the time-traveler's destination)

**In the end state, "this comprehension / generator expression / `map`-`filter`-`sum`
pipeline allocated an intermediate list or iterator" is not a performance bug you can
file — because the absence of an intermediate is a *structural property of the IR*, not
an optimization that may or may not have fired.** A Pythonista writes the elegant chain:

```python
total = sum(x * x for x in data if x > 0)          # genexpr → sum
evens = [y for xs in matrix for y in xs if y % 2 == 0]   # nested comprehension
top   = max(len(w) for w in words)                  # genexpr → max
pairs = dict((k, f(v)) for k, v in items.items())   # genexpr → dict
seen  = set(map(normalize, filter(valid, rows)))    # itertools-style chain
```

…and the Rustacean gets, for each, a **single tight loop with no intermediate `list`,
no intermediate iterator object, no per-element `(value, done)` pair tuple, no heap
generator frame, and no per-element refcount traffic on values that never escape the
loop body** — on native, LLVM, WASM, and Luau identically, because the fused form is
*portable TIR*, not a backend-local rewrite.

Concretely, at the destination:

- **The fusion-eligibility fact is first-class and generated.** A producer/consumer
  pair is fusable or it is not, and *why not* terminates in exactly one of the
  enumerated **deforestation killers** (§2.3). "It didn't fuse" is answerable the same
  way every time: name the killer, point at the barrier op, decide whether it is
  *removed* (it was a false barrier), *hoisted past* (it is loop-invariant or
  escapes the chain), or *honored* (it is a real cross-iteration dependency — fusion
  is correctly declined and the program is still correct, just not fused).
- **There is ONE fusion authority, not three.** Today the codebase has *three*
  disjoint, partially-dead fusion mechanisms (§1.2): the dead-and-incorrect
  `deforestation::run` iterator-chain matcher, the live but Phase-1-limited
  `generator_fusion.rs` splice, and the `iter_devirt.rs` list→index lowering. The
  end-state unifies the *recognition* of "a producer loop feeding a consumer" behind a
  single **fusion-eligibility analysis** that all three lowering strategies consume.
- **The `Copy[fused=…]`-tag divergence trap is deleted.** No backend ever re-interprets
  an ad-hoc string tag to reconstruct fused semantics (the current dead path's fatal
  flaw — §1.2). Fusion emits *real fused IR* (a single structural loop), verified by
  `verify_function`, identical across backends.
- **The CPython floor is green on the data-flow benchmark cluster** —
  `bench_generator_iter`, `bench_dict_comprehension`, `bench_sum`/`bench_prod_list`/
  `bench_min_list`/`bench_max_list`, and the new pipeline benchmarks (§5) — warm AND
  cold, every target, every profile, on `tools/perf_scoreboard.py`, with the win
  attributable to "no intermediate allocated" (a DIMENSIONAL alloc-count drop that is
  *also* a warm-cycle win, classified per CLAUDE.md, never an alloc-count-only claim).
- **PyPy/Codon gap closed on the comprehension/generator class** — generator fusion +
  frame elision is exactly what PyPy gets from trace inlining and Codon from eager
  loop compilation (doc 53 Rung 6); molt gets it AOT from the fusion-eligibility fact.

The end-state IR fact makes "this chain allocated an intermediate" an **unexpressible
class**: a fusable chain *has no node* that allocates the intermediate, because the
producer and consumer share one loop with the element threaded in SSA.

---

## 1. The method, and the current state (verified against `main`)

### 1.1 A rung deepened is a FACT, not a faster pass (binding restatement)

Per CLAUDE.md ("fix the REPRESENTATION, not the pass") and doc 53 §1: this arc is
complete only when (a) a **fusion-eligibility fact family** exists as a typed, cached
record in `runtime/molt-tir/src/tir/`, `FactValue`-typed (the shared substrate); (b)
the three lowering strategies *consume* it instead of each re-deriving "is this a
producer feeding a consumer"; (c) a validator turns every fusion into a checkable
obligation (the fused loop is *observationally equivalent* to the unfused chain —
same elements, same order, same count, same exception timing, same side-effect
ordering); and (d) the scoreboard rows the fact targets are GREEN and *stay* green
because the slow (allocating) lowering is structurally absent. "We added a pattern that
fuses `sum(genexpr)` 10% faster" is **not** the deliverable; the fact that makes the
allocating chain unexpressible is.

**The deforestation-specific soundness law (the one invariant every phase obeys):**

> **Fusion is legal iff it preserves the *observable trace* of the unfused chain:**
> (1) the same sequence of elements in the same order; (2) the same total count
> (no early/late termination unless the consumer is itself early-exit — `any`/`all`/
> `next`); (3) the same exception, raised at the same logical element, with the same
> pending-state; (4) the same *ordering* of any side effects relative to element
> production; and (5) the same *laziness* observable to user code (a genexpr is lazy
> *after* its outermost iterable is evaluated — `comprehensions.py:109`; eager
> materialization of a lazy genexpr changes exception/side-effect timing and is
> therefore itself a killer unless the consumer is strict).

This law is why `fusion_barrier_opcodes` is **deliberately distinct from
`side_effecting`** (`op_kinds.toml:317-351`): an allocation / attribute-read / may-throw
op (`ObjectNewBound`, `LoadAttr`, `Index`, `Div`) preserves the trace under fusion
(it runs once per element either way), so it is SAFE to fuse and is correctly NOT a
barrier — even though it is side-effecting/may-throw for DCE's purposes. The barriers
are exactly the ops that change *cross-iteration / control state* or *suspend*.

### 1.2 The current state: THREE fusion mechanisms, one dead-and-wrong, one Phase-1, one orthogonal

Verified read-only against the 2026-06-24 worktree snapshot. This is *completion + unification + correctness
repair*, not a greenfield build — but the central deliverable is repairing a structural
defect, not adding features.

| Mechanism | Where | State (verified) |
|---|---|---|
| **`deforestation::run`** iterator-chain matcher (`sum`/`any`/`all`/`min`/`max`/`list`/`len`/`set`/`tuple`/`sorted`/`reversed` over a `ForIter` loop) | `tir/passes/deforestation.rs:130-860` | **DEAD AND INCORRECT.** Not wired into the pass manager — `pass_manager.rs:393` registers only `run_tuple_scalarize`, NEVER `deforestation::run`. And its output is *unconsumed*: it emits `Copy[fused="sum"]` etc. (`deforestation.rs:329-340`), but **no backend reads `get("fused")`** (grep over `runtime/molt-backend`: zero matches; only `copy_prop.rs:267` and `counted_loop.rs:150` *preserve* the attr as opaque). If it were enabled it would MISCOMPILE: `Copy[fused="sum"](acc)` lowers as a plain copy of the accumulator's last value, not the sum. |
| **`run_tuple_scalarize`** (BuildTuple immediately unpacked → direct SSA copies; the Fibonacci-swap case) | `tir/passes/deforestation.rs:911-1124` | **LIVE + correct** (`pass_manager.rs:393`). The one genuinely working deforestation in the file. A true zero-allocation rewrite (deletes the BuildTuple+unpack pair). Keep; extend (§4 phase 1). |
| **`generator_fusion::run_generator_fusion`** module-level splice (generator frame elision + yield-site inlining into the consumer loop) | `tir/passes/generator_fusion.rs` (2467 lines); driven from `module_phase.rs:261` after the inliner | **LIVE + structurally correct, Phase-1-limited.** This is the *real* fusion: it splices the `_poll` body into the caller, promotes frame slots to loop phis, eliminates the heap frame + `(value,done)` pair + indirect poll call + state dispatch. Bails (Tier-D, correct) on: multi-yield-SITE (`apply_fusion` `yield_count != 1`, `:784`), `YieldFrom`, `.send`/`.throw`/`.close`, >1 instance, exception handlers, recursive poll, consumer loop carrying block args (`:822-830`). |
| **`iter_devirt::run`** (list iteration → index loop) | `tir/passes/iter_devirt.rs`; `pass_manager.rs:390` | **LIVE + correct.** Not fusion per se — it devirtualizes `for x in list` to `Index(list, i)`. A *producer* of the clean counted-loop shape the fusion analysis recognizes. Composes; do not duplicate. |
| **`fusion_barrier_opcodes`** generated barrier authority | `op_kinds.toml:317-351` → `op_kinds_generated.rs:1218` (`opcode_is_fusion_barrier_table`) | **LIVE + correct + the right foundation.** The ONE exhaustive-over-OpCode barrier set (rendered by `gen_op_kinds.py`; a new OpCode must be classified before the backend compiles). This arc *extends the registry* here, never hand-matches. |
| **The fusion-ELIGIBILITY fact** (producer/consumer pairing; what is fusable, under which guard; the multiple-consumer/escape integration; the cross-call IP summary) | — | **DOES NOT EXIST.** There is a barrier set but no eligibility analysis. Each of the three mechanisms re-derives "is this a producer feeding a consumer" by its own ad-hoc CFG walk (`deforestation.rs:201` `find_fusable_chains`, `generator_fusion.rs:430` `collect_fusion_candidates`, `iter_devirt.rs:161` `find_candidates`). **This is the largest single hole and the unification target.** |

**The load-bearing diagnosis.** molt does not lack fusion *machinery* — it has a
sophisticated, verified generator-splice. It lacks (1) a **single eligibility fact** the
three mechanisms share, (2) **correctness repair** of the dead `deforestation::run`
(delete the tag-divergence path; re-express its value through real fused IR), and
(3) **coverage** of the killers that make the common Pythonic chains bail today:
nested comprehensions (multi-loop), `map`/`filter`/`zip` chains (multi-yield after
inlining), and cross-call pipelines (a fused chain whose producer is a *named function*,
not an inline genexpr).

### 1.3 Why comprehensions/genexprs are already in scope for fusion

Comprehensions and generator expressions lower through **poll-function generators**:
`visit_GeneratorExp` (`comprehensions.py:103`) always emits a `_poll` lowering (lazy
semantics, `:109`), and `visit_ListComp`/`SetComp`/`DictComp` build a genexpr then call
`_emit_list_from_iter` / `_emit_set_from_iter` / `_emit_dict_fill_from_iter`
(`comprehensions.py:56-101`). So a list comprehension is, post-lowering, exactly the
shape `generator_fusion.rs` recognizes: `AllocTask(generator) → GetIter → IterNext loop
→ consume`. **The genexpr→`sum`/`list`/`set`/`dict` pipeline is already a
generator-fusion candidate** — it just bails on the multi-yield / nested / chained
cases. This arc's coverage extensions (§3) are therefore *extensions of the existing
splice's recognition + a new strict-consumer recognizer*, not a parallel system.

The `_can_inline_list_comp` fast path (`comprehensions.py:54`) handles the *simplest*
single-loop strict-list case in the frontend (eager inline, no generator); this arc
makes the **general** case (nested, filtered, chained, non-list-strict-consumer) fuse
in TIR where the eligibility fact lives — frontend special-casing every shape is the
workaround this arc replaces (doctrine: "no regex where structural parsing is needed").

---

## 2. The structural facts/mechanisms to build (the deforestation plane)

The plane is **one analysis** (the fusion-eligibility fact), **one extended registry**
(the per-op fusion semantics), and **one unified lowering** (three strategies behind one
recognizer), each tied to the class of waste it retires, across all backends/profiles.

### 2.1 Fact family A — `FusionEligibility` (the producer/consumer pairing fact)

**The class retired:** *the re-derived, ad-hoc, mechanism-local "is this fusable" walk*
— three CFG walks that can disagree about what counts as a producer feeding a consumer.
The end-state: one cached analysis answers "is this `(producer, consumer)` pair fusable,
and under what guard," and all lowering consumes it.

**The representation.** A new cached analysis `fusion_facts.rs`, registered as
`AnalysisId::FusionFacts` (the same `AnalysisManager` + fail-closed invalidation +
`MOLT_VERIFY_ANALYSIS=1` contract as every other fact — `analysis/mod.rs:66`; never a
side channel):

```rust
/// A recognized producer→consumer fusion pair, with its eligibility verdict.
struct FusionEligibility {
    producer: ProducerKind,      // what generates the element stream
    consumer: ConsumerKind,      // what folds/collects/early-exits it
    chain: Vec<StageKind>,       // intermediate map/filter/zip stages, in order
    verdict: FactValue,          // Proven | Guarded(GuardId) | Unknown | False
    killer: Option<Killer>,      // when verdict != Proven: WHICH barrier (§2.3)
    element: ValueId,            // the per-element value threaded in SSA
    laziness: Laziness,          // Lazy(after_outer_eval) | Strict
}

enum ProducerKind {
    RangeLoop,                   // for i in range(...) — range_devirt already lowers
    ListIndexLoop,               // for x in list — iter_devirt already lowers
    GeneratorPoll(FuncName),     // a genexpr/comprehension _poll (generator_fusion)
    IterableArg(ValueId),        // an opaque iterable (GetIter on a param/field)
}
enum ConsumerKind {
    Reduce(ReduceOp),            // sum | prod | min | max | len | any | all | next
    Collect(CollectKind),       // list | set | tuple | dict | sorted
    Reiterate,                   // another for-loop (pipeline stage, not terminal)
}
enum StageKind { Map(FuncOrExpr), Filter(PredOrExpr), Zip(Vec<ProducerKind>), Enumerate }
```

- `verdict = Proven` when the chain body has **no live killer** (every barrier op is
  either absent, removable, or hoistable — §2.3) and laziness is satisfiable.
- `verdict = Guarded(class_version)` when fusion is legal only under a runtime guard
  (e.g. the consumer's `__iter__`/`__next__` are the builtin ones, guarded by a
  class-version check that deopts to the unfused chain — reuses doc 53 Rung 4
  `ClassVersionGuard`).
- `verdict = Unknown` (fail-closed default — the value never fuses; the chain stays as
  written, allocating its intermediates, which is *correct*, just not optimized).
- `verdict = False` when a structural property *proves* non-fusability (an escape of the
  intermediate — §2.3 killer K3 — with no hoist available).

**Producers:** `alias_analysis.rs` + the ownership lattice (escape of the intermediate),
the extended `op_kinds.toml` fusion columns (§2.2), `call_graph.rs` (resolving
`GeneratorPoll`/`Map(FuncName)` targets), CallFacts IP summaries (§2.4, cross-call).
**Consumers:** the three lowering strategies (§2.5), unified behind reading this fact.

**The benchmark class healed:** every comprehension/genexpr/pipeline benchmark (§5).
**The PyPy/Codon gap closed:** trace-inlined producer/consumer fusion (PyPy) / eager
loop compilation (Codon) — doc 53 Rung 6.

### 2.2 Fact family B — the per-op fusion semantics columns (extend `op_kinds.toml`)

**The class retired:** *the hand-`match`ed fusion property* — today there is exactly one
generated fusion column (`fusion_barrier_opcodes`), but the eligibility analysis needs
two more per-op facts, and adding them as hand-matches in the new pass would re-open the
default-false drift trap doc 25/doc 59 exist to close. Per doc 53 §4 invariant 3 ("one
op-semantics authority"): a rung that needs a per-op fact *adds a column to the
registry*, never a hand-`match`.

Three columns on `op_kinds.toml`, rendered exhaustively over OpCode by `gen_op_kinds.py`,
`--check`-gated (drift uncompilable):

1. **`fusion_barrier_opcodes`** (EXISTS, `:331-351`) — the cross-iteration/suspend
   barrier. **Verify-and-keep.** This arc does not weaken it.
2. **`fusion_order_sensitive_opcodes`** (NEW) — ops whose result *observably depends on
   evaluation order within the element body* even though they are not cross-iteration
   barriers (the per-element ordering axis of the soundness law clause 4). Most pure ops
   are order-insensitive (reordering `x*x` and `x+1` within one element is invisible);
   the order-sensitive set is the ops whose *relative* order to element production is
   user-observable. This column lets a `Map` stage be reordered/fused with a `Filter`
   stage only when both are order-insensitive — the missing fact that makes
   `map(f, filter(p, xs))` ⟷ `filter(p, map(f, xs))` legality decidable.
3. **`fusion_strict_required_opcodes`** (NEW) — consumer ops that are *strict* (force the
   entire stream eagerly: `list`/`set`/`tuple`/`sorted`/`sum`/`len`/`prod`) vs *lazy/
   early-exit* (`any`/`all`/`next`/`Reiterate`). This column carries the laziness axis
   (soundness law clause 5): a strict consumer may fuse with a lazy producer (it forces
   it anyway); a lazy consumer fused with a producer that has *observable side effects
   per element* must preserve early termination, so the fused loop must keep the
   consumer's break, not run the producer to exhaustion.

These three columns are the **generated, drift-proof** carriers of soundness-law clauses
4 and 5. They compose with the existing `may_throw`/`side_effecting`/`operand_ownership`
columns (clauses 3 and the RC calculus) — one registry, more columns, zero hand-matches.

### 2.3 The DEFORESTATION KILLERS — enumerated, each with its structural treatment

This is the heart of the arc: the barriers that block fusion, and *the structural
mechanism to remove or hoist past each*. A killer is not "give up"; it is a typed
classification with a defined treatment. Each `FusionEligibility.killer` is one of:

| # | Killer | What it is | Structural treatment (remove / hoist / honor) |
|---|---|---|---|
| **K1** | **Materialization point** | The chain is forced to a concrete container mid-stream (`list(...)` feeding another stage; `sorted(...)`; a `len()` on a lazy stage that must count). | **REMOVE** when the materialization is *itself the terminal consumer* (fuse producer→`Collect` directly, never building then copying — the `fuse_list`/`fuse_set`/`fuse_tuple` *intent* of the dead path, done as real IR). **HOIST** a mid-stream `sorted`/`len` only when a strict barrier is genuinely required (then the chain splits into two fused segments at the materialization, each segment fused internally — never a single allocation per element AND a whole-list copy). |
| **K2** | **Multiple consumers** | The producer's element stream (or the intermediate container) is consumed by ≥2 sinks (`xs = [f(x) for x in data]; a = sum(xs); b = max(xs)`). A generator can only be drained once. | **HONOR by materializing ONCE** (the intermediate is built exactly once and both consumers read it — already correct, no double-drain) **OR fuse-and-split** when the producer is *cheaply re-runnable and pure* (a `range`/`list`-index producer with a pure body): emit two fused loops over the same source, eliminating the intermediate, when the cost model (`TargetInfo`) proves re-running beats materializing. The use-count is read from the ownership/alias analysis (the same use-count `run_tuple_scalarize` uses, `deforestation.rs:919`), never a private walk. |
| **K3** | **Escape of the intermediate** | The intermediate container/iterator *escapes* the fusion region (returned, stored to a field/global, passed to an opaque callee, captured by a closure). | **HONOR** (cannot fuse away an escaping value — `verdict = False`) **UNLESS** the escape analysis + ownership lattice (doc 53 Rung 1) prove the escape is itself fusable downstream (the cross-call case, K6). The escape fact is read from `escape_analysis.rs` (`EscapeState`, `:23`) + the ownership lattice — **not re-derived**. This is the producer→consumer edge to Rung 1: a `NoEscape` intermediate is fusion-eligible; a `GlobalEscape` one is `False`. |
| **K4** | **Per-element side effects (ordering)** | The element body performs an observable side effect (`print`, a mutation, an attribute store) whose *order* relative to other elements/effects is user-visible. | **HONOR ordering, still fuse the loop.** Fusion *preserves* per-element order (soundness law clause 1+4), so a side-effecting-but-order-preserving body is STILL fusable into one loop — the `Call`/`StoreAttr`/`StoreIndex` barrier in `fusion_barrier_opcodes` is conservative for the *cross-iteration* case; the `fusion_order_sensitive_opcodes` column (§2.2) lets the analysis distinguish "reorderable within an element" from "fixed order." The treatment is: fuse the loop, do NOT reorder stages across an order-sensitive effect. |
| **K5** | **Cross-iteration state** | The body reads/writes state that flows *between* iterations (a closure cell updated each element, a `yield from` delegation, an async suspend, a `raise` that crosses the loop). | **HONOR** — this is the true barrier set (`fusion_barrier_opcodes`: `ClosureStore`/`ClosureLoad`/`YieldFrom`/`StateYield`/`Raise`/the async suspend ops). `verdict = Unknown` (stays unfused, correct). This is the one killer with no removal/hoist — by construction (a real cross-iteration dependency is not deforestable). The mechanism is *exhaustive classification* so a NEW such op cannot silently bypass the barrier. |
| **K6** | **Cross-call boundary** | The producer is a *named function* returning an iterator/list, consumed by a loop in a *different* function (`def gen(n): return (i*i for i in range(n))` … `sum(gen(N))`), or a stdlib `itertools`-style helper. The fusion region spans a call edge. | **HOIST PAST via the IP summary fact** (§2.4): a `FusionSummary` on the callee (does it return a fusable producer? what is its element body's killer set?) lets the caller fuse across the boundary *after inlining* (the inliner already runs before generator_fusion — `module_phase.rs:234,261`), or *without* inlining when the summary proves the producer is a pure fusable stream. This is the producer→consumer edge to Rung 2 (CallFacts). |
| **K7** | **Eager-materialization-changes-timing** | Fusing a *lazy* genexpr into a *strict* position would force it eagerly, changing exception/side-effect timing if the consumer would have short-circuited (`any(expensive(x) for x in data)` must stop at the first truthy). | **HONOR laziness via the `fusion_strict_required_opcodes` column** (§2.2): a lazy/early-exit consumer fused with a side-effecting producer keeps the consumer's break in the fused loop (the `fuse_any_all` early-exit *intent* of the dead path, done correctly). A strict consumer forces anyway, so no timing change. The laziness axis is a generated fact, not a per-case guess. |

**The enumeration IS the deliverable.** Every "why didn't it fuse" terminates in exactly
one Kn with a defined treatment; there is no residual "it just didn't." K5 is the only
honor-always killer (by construction); K1/K6 are remove/hoist; K2/K3/K4/K7 are
context-dependent (the cost model / escape fact / laziness fact decides). A NEW barrier
op added to `op_kinds.toml` lands in K5 by default (fail-closed) until classified finer.

### 2.4 Fact family C — `FusionSummary` (the cross-call IP fact, K6's mechanism)

**The class retired:** *the fusion region that stops at a function boundary* — today a
genexpr returned from a helper, or a stdlib iterator-producer, defeats fusion entirely
(the consumer sees an opaque `GetIter` on a call result).

**The representation.** A per-function interprocedural summary, computed in the module
phase and *seeded* via the existing `AnalysisManager::prepopulate` +
`CallFactsTable::build_module` pattern (`call_facts.rs:46-63`) — the same seeding
machinery, a new summary, NOT a side channel:

```rust
struct FusionSummary {
    returns_fusable_producer: FactValue,   // does the fn return a drainable stream?
    producer_kind: Option<ProducerKind>,   // RangeLoop | GeneratorPoll | ...
    body_killer_set: KillerMask,           // the union of killers in the element body
    element_is_pure: FactValue,            // re-runnable (K2 fuse-and-split eligibility)
    laziness: Laziness,
}
```

This is a **member of the CallFacts family** (doc 53 Rung 2 / doc 47), not a new
top-level table — `CallFacts` already carries `no_alloc`/`no_escape_args`/`typed_return`;
`FusionSummary` is the iterator-shaped sibling. **Producers:** the per-function fusion
analysis (§2.1) run bottom-up over the call graph. **Consumers:** the caller's fusion
analysis (resolves a `GetIter` on a call result to the callee's `producer_kind`), the
inliner's profitability (a fusable-producer callee is worth inlining *to enable fusion*
— the inliner already has a "deforestation unlock" notion, `inliner.rs:325-357`).

### 2.5 The unified lowering — three strategies, one recognizer, REAL fused IR

**The class retired:** *the three-mechanism split and the `Copy[fused=…]` tag-divergence
trap.* The end-state: one recognizer (the `FusionEligibility` fact) drives three
*lowering strategies*, each emitting real structural IR (verified by `verify_function`),
identical across all four backends.

1. **Strict-consumer fusion** (replaces the dead `deforestation::run`): a `Proven`
   `(producer, Reduce|Collect)` pair lowers to a single loop with the fold/collect
   inlined into the body — the accumulator threaded as a loop-carried block arg (the SSA
   loop-phi form, NOT a tagged `Copy` the backend must decode). `sum`→`acc = Add(acc,
   elem)` carried on the back-edge; `list`→`list_append(lst, elem)` on a pre-built list
   carried as a loop-invariant; `any`/`all`→early-exit `CondBranch` (K7 honored). The
   result is the *exact* fused IR the dead path *intended* but emitted as un-consumed
   tags. **This is the correctness repair**: delete the tag path, emit real IR.
2. **Generator-pipeline fusion** (extends `generator_fusion.rs`): the existing
   single-yield splice generalized to the multi-yield-SITE case (doc-26 Phase-1
   Finding #1, the return-dispatch over yield-delimited segments the current code bails
   on at `:784`) and to the consumer-carried-block-args case (the `:822-830` bail) — so
   nested comprehensions and filtered genexprs fuse. The splice already emits real IR
   (it is the model); this is coverage extension behind the shared eligibility fact.
3. **Stage fusion** (`map`/`filter`/`zip`/`enumerate` chains): a multi-stage `chain`
   collapses to one loop body with each stage as a guard/transform on `element`, stages
   ordered per `fusion_order_sensitive_opcodes` (§2.2). A `Filter` stage becomes a
   `CondBranch`-continue; a `Map` stage a transform; `zip` a parallel multi-producer
   advance; `enumerate` an induction-counter add. No intermediate iterator per stage.

All three consume `FusionEligibility`; all three emit IR `verify_function` accepts; all
three are **portable TIR** (Rung 7 / doc 53: the fused loop crosses to WASM/LLVM/Luau
because it is ordinary loop IR, not a native-only rewrite — this arc has NO backend-local
fusion code, which is the structural guarantee of cross-backend parity).

### 2.6 Profile behavior (dev-fast / release-fast / release-output)

Fusion is a *correctness-preserving structural simplification that reduces allocation*,
so it is **on in every profile** (unlike speculative guarded devirt). Per doc 51 §2
(profiles are separate products, none hides a regression):

- **dev-fast** — fusion is *cheap to run* (a CFG-local analysis + a structural rewrite)
  and *reduces* downstream work (fewer ops, no allocation), so it improves dev compile
  latency, not just runtime. Run it. The cost is bounded by the eligibility analysis
  being O(uses) like `run_tuple_scalarize`.
- **release-fast / release-output** — full coverage (all three strategies, K6 cross-call,
  K2 fuse-and-split under the cost model). release-output additionally lets the
  fuse-and-split (K2) cost threshold favor re-running pure producers more aggressively
  (smaller artifact, fewer allocations) since compile time is not the constraint.
- The fused IR is identical across profiles; only the *aggressiveness* of K2's
  cost-model threshold and the K6 inline-to-fuse budget differ. No profile emits a less
  fused (more allocating) form silently — a profile-specific allocation is a tracked
  DIMENSIONAL difference, never a hidden regression (doc 53 §0.1).

---

## 3. Phases in dependency order (each independently landable green)

Each phase is a complete structural piece (doctrine: "structural change as the unit of
work"), lands green, and is gated on the full matrix it touches. The arc's *first*
landable value is the **correctness repair** (the dead path is a latent miscompile-if-
enabled and a code-smell the doctrine forbids leaving), then the unifying fact, then
coverage.

### Phase 0 — Delete the dead-and-incorrect tag path; lock the barrier authority

**Goal:** remove the structural defect before building on the file. The
`deforestation::run` iterator-chain matcher (`:130-860`) emits `Copy[fused=…]` tags no
backend consumes; it is not wired in; if wired it miscompiles. Per doctrine ("code smell
… is wrong; fix it properly"; "no TODO/FIXME as excuse to ship broken code"): delete the
unconsumed-tag lowering bodies (`fuse_sum`/`fuse_any_all`/`fuse_min_max`/`fuse_list`/
`fuse_len`/`fuse_set`/`fuse_tuple`/`fuse_sorted`/`fuse_reversed`, `:300-860`) and their
`find_fusable_chains` driver, **keeping** `run_tuple_scalarize` (live + correct).
Re-home the recognition *intent* (the `FusableBuiltin` set, the chain shapes) as the seed
for the §2.1 eligibility analysis — the knowledge is not lost, it moves to where it can
be emitted as real IR. Verify-and-lock `fusion_barrier_opcodes` (add a registry test
that the set is exhaustive and that `side_effecting ≠ barrier` for the documented safe
ops, `:317-330`).

**Gate:** the file no longer contains an unconsumed-tag emitter (a `tools/` grep gate:
zero `AttrValue::Str("…")` under a `"fused"` key that no backend reads); `cargo test -p
molt-tir` green; `run_tuple_scalarize` tests unchanged and green; `gen_op_kinds.py
--check` green; **byte-identical generated codegen** for every benchmark (the dead path
was never running, so deleting it changes no output — proven by the differential suite).
This phase is pure debt-removal: `debt_markers_total` and the dead-code surface go down.

### Phase 1 — The `FusionEligibility` analysis + registry columns (the fact, consumed by ONE strategy)

**Goal:** land the fact family A (§2.1) + registry columns B (§2.2) and wire the FIRST
lowering strategy (strict-consumer fusion, §2.5 strategy 1) to consume it — proving the
fact is load-bearing, not representation-only (doctrine: a representation-only sub-phase
is acceptable only as an explicitly-noted intermediate within a rung's arc, never the
terminus; this phase's terminus is a *consumed* fact).

- 1a — `fusion_facts.rs` + `AnalysisId::FusionFacts`; the `FusionEligibility` record,
  `FactValue`-typed, fail-closed `Unknown`. Recognizes the strict-consumer single-loop
  case (the `range`/`list-index`/`genexpr-poll` producer → `sum`/`min`/`max`/`len`/`list`/
  `set`/`tuple` consumer). Reads escape (K3) from `escape_analysis.rs`, use-count (K2)
  from the alias analysis, barriers (K5) from `opcode_is_fusion_barrier_table`. Adds the
  `fusion_order_sensitive_opcodes` + `fusion_strict_required_opcodes` columns to
  `op_kinds.toml` + `gen_op_kinds.py`.
- 1b — strict-consumer lowering (strategy 1): a `Proven` reduce/collect pair lowers to
  one loop, accumulator/collector threaded as a loop-carried block arg (real SSA IR, no
  tag). The validator: the fused loop is observationally equivalent (a differential
  harness that runs the fused and unfused forms on the same input and asserts identical
  element trace + result + exception). K1 (materialization-as-terminal) and K7
  (early-exit `any`/`all`) handled.

**Gate:** `gen_op_kinds.py --check` green; the eligibility analysis self-validates under
`MOLT_VERIFY_ANALYSIS=1`; the equivalence validator green on the data-flow benchmark set;
`bench_sum`/`bench_prod_list`/`bench_min_list`/`bench_max_list` warm GREEN on the
scoreboard, native + LLVM + WASM + Luau, with an alloc-count DIMENSIONAL drop that is
*also* a warm-cycle win (classified GREEN, not DIMENSIONAL-only); **no CPython-red
regression** on any cell; differential parity all backends. The fact's selection is a
`perf_causality` row (Rung 0): the targeted benchmark's hot "intermediate allocation"
helper is gone.

### Phase 2 — Generator-pipeline fusion coverage (extend the splice behind the fact)

**Goal:** generalize `generator_fusion.rs` (strategy 2) to the cases the common Pythonic
comprehensions hit, all driven by the shared `FusionEligibility` fact (replace its
private `collect_fusion_candidates` recognition with the fact; keep its verified splice
mechanism).

- 2a — multi-yield-SITE generators (the `:784` bail): the return-dispatch over
  yield-delimited segments (doc-26 Phase-1 Finding #1). Unlocks sequential-yield
  comprehension bodies.
- 2b — consumer-carried-block-args (the `:822-830` bail): re-thread the consumer's
  loop-carried values (an accumulator `total` as a header block arg) through the fused
  loop. Unlocks nested comprehensions and filtered genexprs whose consumer carries state.
- 2c — multi-instance + the laziness/early-exit (K7) cases for generators.

**Gate:** `bench_generator_iter` + `bench_dict_comprehension` warm GREEN, all backends;
the equivalence validator green (the splice already `panic!`s on a malformed splice,
`:903` — extend the validator to the new shapes); frame-elision count up in
`MOLT_INLINE_STATS`; differential parity (the multi-yield dispatch is the riskiest —
exhaustive differential on yield-count 1..N, with/without filter, with/without nesting);
no red.

### Phase 3 — Stage fusion: map/filter/zip/enumerate chains (strategy 3)

**Goal:** the `itertools`-style chain (`set(map(f, filter(p, xs)))`) collapses to one
loop, stages ordered per `fusion_order_sensitive_opcodes`. K4 (order-sensitive effects)
honored: stages do not reorder across an order-sensitive effect.

**Gate:** the new pipeline benchmarks (§5) warm GREEN all backends; the equivalence
validator green on `map`/`filter`/`zip`/`enumerate` permutations; the order-sensitivity
column proven correct by a differential test that reorders stages and asserts identical
trace only when both stages are order-insensitive; no red.

### Phase 4 — Cross-call fusion: the `FusionSummary` IP fact (K6)

**Goal:** fact family C (§2.4) + K6's hoist-past treatment. A fusable producer returned
from a named function (or a stdlib iterator helper) fuses into a consumer in another
function — after inlining (the common case) or via the summary without inlining (a pure
fusable producer).

**Gate:** a cross-call pipeline benchmark (a `def gen(): return (… for …)` consumed by
`sum`/`list` in `main`) warm GREEN all backends; `FusionSummary` seeded via
`prepopulate` (no side channel), self-validated against `call_fact_coverage.py`; the
inliner's fuse-to-inline profitability proven (a fusable-producer callee gets inlined
*and* the result fuses, measured); deopt/correctness differential (a callee whose
returned producer is *not* actually fusable at a call site takes the unfused path); no red.

### Phase 5 — Backend-parity closure + footprint (Rung 7 / Rung 8 tie-in)

**Goal:** certify every fused-loop fact crosses to every backend (Rung 7 matrix) and
measure the footprint dimension (Rung 8): fewer allocations → lower peak RSS, and the
fused loop → smaller code than the unfused chain + helpers.

**Gate:** the Rung 7 backend support matrix (doc 53 Rung 7 / `op_kinds.toml` backend
columns) GREEN for the new fusion ops on all four backends; peak-RSS DIMENSIONAL drop on
the data-flow cluster recorded; binary-size DIMENSIONAL check (fused loops should not
grow the artifact — if a strategy inlines a large body, the K6 budget gates it); cold AND
warm reported for every cell. The arc is "done" only when this matrix is green.

---

## 4. Measurement and gates (the Performance Constitution discipline)

Per CLAUDE.md and doc 53 §1: every phase reports, via `tools/perf_scoreboard.py`, for
every touched benchmark: `benchmark → target → backend → profile → CPython ratio →
PyPy ratio → Codon ratio → binary size → peak RSS → compile time → cold/warm → log
artifact`, with ≥5 samples, CV stability, classified GREEN / RED_STABLE / RED_NOISY /
TIE / DIMENSIONAL_WIN.

**The deforestation-specific measurement law:** a fusion win is a *both-dimensions* claim
or it is not a heal. The alloc-count drop (the intermediate is gone) is the DIMENSIONAL
signal; the warm-cycle improvement is the speed signal. Per CLAUDE.md ("no warm-time
claim from allocation counters alone"), an alloc-count drop that does NOT flip the warm
gate is reported as DIMENSIONAL_WIN (the allocation was already cheap / the warm cost was
elsewhere), never as a speed heal — and that result *names the next fact* (the warm cost
that the missing-intermediate did not retire). The targeted scoreboard rows:

- **CPython floor (table 1):** `bench_sum`, `bench_prod_list`, `bench_min_list`,
  `bench_max_list`, `bench_generator_iter`, `bench_dict_comprehension`, + the new
  pipeline benchmarks (§5). Any `< 1.00×` is RED and blocks the phase.
- **PyPy (table 2):** the comprehension/generator dynamic subset — name the residual
  missing fact where PyPy still wins (it should be Rung 2/3 facts — direct-call /
  unboxed-element — not a fusion gap, once this arc lands).
- **Codon (table 3):** `bench_sum`/`bench_prod_list` on matched semantics — the fused
  reduction loop should approach Codon's eager loop.
- **Backend (table 4):** native/LLVM/WASM/Luau each — the fused loop is portable IR, so
  a native fusion win that does not appear on WASM is a *bug in this arc* (a backend-local
  rewrite slipped in), not a WASM gap. This is the structural self-check.
- **Profile (table 5):** dev/release-fast/release-output — fusion on in all three; report
  the dev compile-latency *improvement* (fewer ops downstream).

**The equivalence validator (the #75 Alive2-discipline obligation for this arc):** a
differential harness `tools/fusion_equivalence.py` (or a `cargo test` in `fusion_facts.rs`)
that, for every `Proven`/`Guarded` fusion, runs the fused and unfused forms on a battery
of inputs (empty stream, single element, exception-at-element-k, side-effect-ordering
probe, early-exit probe) and asserts identical observable trace. A fusion without this
validator is a half-rung (doc 53 §4 invariant 4).

---

## 5. Composition (21a-e decomposition + the 50-59/53 arcs)

**With doc 53 (the compression ladder) — this arc IS Rung 6's deforestation sub-arc:**
- **Consumes Rung 1** (ownership lattice / `ownership_lattice_min.rs`): the escape fact
  (K3) and the borrow fact (the fused loop's per-element value is `Borrowed` in the body
  → no per-element RC traffic) come from Rung 1. This arc does NOT re-derive escape.
- **Consumes Rung 2** (CallFacts / IP summaries): `FusionSummary` (§2.4) is a CallFacts
  family member; K6 cross-call fusion rides the inliner + IP summary machinery.
- **Consumes Rung 3** (`Repr`): a fused reduction over a `RawI64Safe` stream is the
  *precondition* for Rung 5 vectorizing it — a fused `sum` loop with an unboxed element
  is what SIMD reduces. This arc produces the fused loop; Rung 5 vectorizes it.
- **Consumes Rung 4** (`ClassVersionGuard`): the `Guarded` fusion verdict (a consumer
  whose `__iter__`/`__next__` is the builtin, under a version guard) reuses Rung 4's
  guard + deopt edge — not a new guard mechanism.
- **Is consumed by Rung 5** (loops): the fused loop is the loop Rung 5's induction/range/
  SIMD facts then specialize. **Is certified by Rung 7** (backend matrix): every fusion
  fact gets a matrix row. **Feeds Rung 8** (footprint): fewer allocations → lower RSS.

**With doc 59 (the semantic fact plane):** every fusion fact follows the doc-59 workflow
— `op_kinds.toml` columns generated + `--check`-gated (the two new columns §2.2), the
analysis a registered `AnalysisId` with the standard invalidation, the IP summary a
CallFacts member seeded via `prepopulate`. Zero hand-classified matches added; the new
columns are rendered exhaustively (a new OpCode is a build error until classified into a
killer class). This arc *exercises* doc 59's machinery; it adds no second authority.

**With the 21a-e decomposition (doctrine principle 1 — reduce/don't-grow god-files):**
- The new `fusion_facts.rs` lands as a focused module respecting the 21b crate graph
  (precise visibility, test-util feature for cross-crate accessors — per memory "crate
  extraction precise visibility"). It does NOT bloat an existing god-file.
- **This arc LOWERS the god-file ratchet, not raises it.** Phase 0 *deletes* ~560 lines
  of dead code from `deforestation.rs` (`:130-860`), shrinking the file toward a single
  cohesive concern (`run_tuple_scalarize` + the eligibility-seed shapes). The
  `structural_audit` ratchet must go down across this arc, never up (doc 53 §5, binding).
- The compiler-killer this arc touches: it does NOT grow the per-backend
  `function_compiler.rs` monolith (21a's codegen-unit killer) — by construction it emits
  *portable TIR* with **no backend-local fusion code**, so the four backends gain nothing
  to maintain. The fused loop lowers through the *existing* loop/op lowering. This is the
  structural reason cross-backend parity is free here.

**With the concurrent 50-59 arcs (named, not re-derived):** depends on
`53_perf_compression_ladder` (Rung 1/2/4/5/7 facts above) and `55_memory_safety_
ownership_lattice` (the escape/borrow facts). References `53_perf_scoreboards_and_harness`
for the measurement path (every gate is a scoreboard row). No overlap with `52`/`54`/
`56`/`57` (compat/throughput/DX/UX) or `59` beyond consuming its machinery. This arc's
file set (`fusion_facts.rs`, `deforestation.rs`, `generator_fusion.rs`, `op_kinds.toml`
columns) is **disjoint** from the other arcs' edited files — it is a lane-B perf arc that
runs after Rung 1 (lane A) lands the ownership lattice it consumes.

---

## 6. Risks and their structural (not band-aid) treatments

| Risk | Band-aid (forbidden) | Structural treatment |
|---|---|---|
| **A fusion changes observable behavior** (reorders a side effect, changes exception timing, eagerly forces a lazy genexpr) | special-case the failing program; add a per-shape guard | the **soundness law** (§1.1, five clauses) is the spec; the **equivalence validator** (§4, #75 discipline) is the checkable obligation run on every `Proven`/`Guarded` fusion; the laziness (K7) and order-sensitivity (K4) axes are *generated facts* (§2.2 columns), not per-case judgment. A fusion that would change the trace is `Unknown` (fail-closed, unfused). |
| **The dead `deforestation::run` path is "fixed" by wiring it in** instead of repaired | enable the tag path + add backend `get("fused")` consumers | **NO.** That re-creates the tag-divergence trap across four backends (doc 46 §4.7: four incompatible dispatch paradigms). Phase 0 *deletes* the tag path; fusion emits real structural IR (loop-carried block args), identical across backends, verified by `verify_function`. The tag was the workaround; real IR is the fix. |
| **Three mechanisms drift** (the eligibility walk diverges between strict-consumer, generator, and stage fusion) | keep three private recognizers, sync them by hand | **ONE** `FusionEligibility` fact (§2.1); the three strategies are *lowerings* that consume it, never re-recognizers. `generator_fusion.rs`'s private `collect_fusion_candidates` is *replaced* by the fact (Phase 2), not kept in parallel (doctrine: asymmetry forbidden — migrate all three). |
| **K6 cross-call fusion explodes inlining / compile time** | inline everything to enable fusion; disable K6 | the `FusionSummary` (§2.4) lets the caller fuse *without* inlining when the producer is a pure fusable stream; inline-to-fuse is **budget-gated** by the cost model (`TargetInfo`) and the binary-size/compile-time scoreboard dimensions, profile-tiered (release-output more aggressive). Report the DIMENSIONAL cost. |
| **K2 fuse-and-split re-runs an impure/expensive producer** | always fuse-and-split (double-drains effects); never fuse-and-split (always materialize) | the verdict reads `element_is_pure` (§2.4) + the use-count (alias analysis) + the cost model: fuse-and-split ONLY a `Proven`-pure, cheaply-re-runnable producer when re-running beats materializing; otherwise materialize-once (correct, no double-drain). The purity is a fact, not an assumption. |
| **A new OpCode silently bypasses a fusion barrier** (the default-false drift trap doc 25/59 exist to close) | a wildcard `_ => not_a_barrier` arm | the three fusion columns (§2.2) are rendered **exhaustively over OpCode** by `gen_op_kinds.py`; a new variant is a **build error** until classified. A new barrier-shaped op lands in K5 (honor-always) by default (fail-closed) until classified finer. |
| **An alloc-count drop is reported as a speed heal** without a warm-cycle win | accept the green alloc counter as the heal | per CLAUDE.md / §4: an alloc drop that does not flip the warm gate is DIMENSIONAL_WIN, never a speed heal; it *names the next fact* (the warm cost the missing-intermediate did not retire). No warm claim from alloc counters alone. |
| **A native fusion win does not cross to WASM/LLVM/Luau** | a backend-specific scoreboard exception | by construction there is **no backend-local fusion code** — fusion is portable TIR. A native-only win is therefore a *bug in this arc* (a backend-local rewrite slipped in), caught by the Rung 7 backend matrix (§3 Phase 5) being RED. Gated, never excepted. |
| **ShapeFacts/CallFacts (Rung 2/4) not yet landed** when this arc starts | hand-roll a private escape/target walk inside fusion | this arc is **lane-B, ordered after Rung 1** (it consumes the ownership lattice). The strict-consumer + generator + stage strategies (Phases 1-3) need only Rung 1 (escape) + the existing alias analysis; **only K6 (Phase 4)** needs Rung 2's IP summaries. So Phases 0-3 land on the *current* substrate; Phase 4 waits for Rung 2 — a clean dependency edge, not a workaround. |

---

## 7. Definition of done (this arc, not a phase)

This arc is complete when, on `tools/perf_scoreboard.py` run authoritatively against
`origin/main`, across the data-flow benchmark cluster (§5) × {native, LLVM, WASM, Luau} ×
{dev-fast, release-fast, release-output}:

1. **Every comprehension / generator-expression / `map`-`filter`-`zip`-`sum` pipeline in
   the cluster compiles to a single fused loop with zero intermediate `list`/iterator/
   pair-tuple/heap-frame allocation** — verified by an alloc-count of zero for the
   intermediate on every cell (the structural property), AND a warm-cycle ratio `> 1.00×`
   vs CPython on every cell (the speed property).
2. **There is ONE fusion-eligibility authority** (`fusion_facts.rs` / `FusionFacts`)
   consumed by all three lowering strategies; the dead `Copy[fused=…]` tag path is
   deleted; `deforestation.rs` is a single cohesive concern; the god-file ratchet is
   LOWER than at arc start.
3. **Every "why didn't it fuse" terminates in exactly one enumerated killer K1-K7** with
   its defined treatment; K5 is the only honor-always killer; a new barrier op is a build
   error until classified.
4. **Every fusion is backed by the equivalence validator** (#75 discipline) — the fused
   and unfused forms are observationally identical on the input battery.
5. **The fused loop is portable TIR** — the Rung 7 backend matrix is green for every
   fusion fact on all four backends; there is no backend-local fusion code.

At that point, "this Python data-flow chain allocated an intermediate" is an
**unexpressible class**: a fusable chain *has no node* that allocates the intermediate,
because the property that made it allocate — a re-derived, mechanism-local, tag-divergent
recognition — has been replaced by a single generated, validated, cross-backend IR fact.
