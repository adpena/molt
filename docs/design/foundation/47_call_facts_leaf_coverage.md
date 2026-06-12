<!-- Foundation design 47. Supervisor-authored from tools/call_fact_coverage.py's
28.6% finding + council directive 2026-06-08 ("CallFacts is the missing primitive").
Implementation spec, not a survey. HEAD-anchored at origin/main c05a4aff0.
The IR-fact half of doc 46 §4.1 (FactGraph); the perf-root companion to doc 45. -->

# CallFacts — the call-site fact record (the 28.6% perf root)

## 0. The measurement that forced this (not a benchmark patch)

`tools/call_fact_coverage.py` (the census, gated in CI) reports **call-site fact
coverage = 2/7 = 28.6%**. Of the seven facts a world-class compiler records on a
call site, only **`direct_target`** (the `s_value` attr) and **`typed_return`**
(the result `Repr`) are attached to the call op. The other five —
**`leaf` · `no_throw` · `no_alloc` · `inlinable` · `noescape_args`** — are
*computed inside a pass and discarded*:
- `leaf` — `tir/call_graph.rs` (`leaf_functions()` / `makes_any_call`), a MODULE
  fact never propagated onto the call site.
- `inlinable` — `tir/passes/inliner.rs::is_inlineable`, recomputed each inline
  pass; the decision AND the why-not reason are thrown away.
- `noescape_args` / `no_alloc` — `tir/passes/escape_analysis.rs` (per-`ValueId`
  `EscapeState`, the `Alloc`→`StackAlloc` rewrite), never summarized onto the call.
- `no_throw` — opcode-level `may_throw` (op_kinds.toml) is `true` for ALL call
  opcodes (`Call`/`CallMethod`/`CallBuiltin`), so no-throw is necessarily a
  *per-call-site* fact, and there is no per-site record.

**Consequence (the perf thesis):** a large share of molt's runtime-helper traffic
exists because the compiler *cannot carry the proof needed to remove it*. Each
backend re-derives or falls back to the generic call helper. This is not "inline
more" — it is a missing IR representation. CallFacts is the primitive that makes
the generic-call class *unexpressible* once the proof exists.

## 1. The IR primitive

A `CallFacts` record attached to every call site (the `Call`/`CallMethod`/
`CallBuiltin` op). Each field is a `FactValue`, not a bare bool — so the compiler
distinguishes *proven* from *guarded* from *unknown* and never silently assumes:

```rust
/// Confidence lattice. UNKNOWN is the fail-closed default (treated as the
/// pessimistic answer by every consumer); FALSE is a proven-negative (also
/// pessimistic but cacheable). Guarded/Profiled carry their guard/evidence.
pub enum FactValue {
    Proven,                       // statically established, no runtime check
    Guarded(GuardId),             // true under a runtime guard (class-version, type)
    Profiled(Confidence),         // observed; needs a guard to exploit soundly
    Unknown,                      // fail-closed default — assume the hazard
    False,                        // proven NOT to hold
}

pub struct CallFacts {
    pub target: CallTargetFact,        // #71 — typed CallableTarget, never raw bits
    pub typed_return: Option<Repr>,    // result Repr if precise (else None = DynBox)
    pub leaf: FactValue,               // callee makes no further calls
    pub no_throw: FactValue,           // call provably cannot raise on this edge
    pub no_alloc: FactValue,           // call performs no heap allocation
    pub no_escape_args: ArgFactMask,   // per-arg: does the callee let it escape?
    pub inlinable: InlineEligibility,  // Eligible | WhyNot(reason)
    pub purity: PurityFact,            // Pure | SideEffecting | Unknown (from effects oracle)
    pub ownership_abi: CallOwnershipAbi, // who consumes/borrows each operand (op_kinds operand_ownership)
    pub deopt_or_guard: Option<GuardFact>, // the guard that must hold + its deopt edge
}
```

`CallTargetFact` is the #71 typed `CallableTarget` (`DirectCodePtr | RuntimeMarker
| Closure | BoundMethod | MethodDescriptor | Deopt`) — the IC-marker SIGSEGV
(#59) class becomes unexpressible because the target is a typed variant, never a
decoded raw `u64`. `InlineEligibility::WhyNot` carries the reason
(`Recursive | HasHandlers | OverBudget | Closure | Generator`) so the inliner
stops recomputing it and the coverage tool can report *why* a call stayed generic.

**Attachment.** Phase 1 attaches `CallFacts` as a side-table on the
`TirFunction` keyed by the call op's result `ValueId` (cheap, no op-layout
change, survives existing passes that don't know about it). Once stable it can
move into the op's `AttrDict` or a dedicated op field. The side-table is built by
a `CallFactsAnalysis` (a cached `AnalysisId`, like `AliasAnalysis`) and
invalidated by the same events that invalidate its producers.

## 2. Producers — each field is filled by an analysis that already exists

| field | producer (already computes it) | today |
| --- | --- | --- |
| `target` | `call_graph.rs` `classify_call_op` → `CallEdge::StaticDirect` | attached (attr string) → make it a typed `CallTargetFact` (#71) |
| `typed_return` | `types.rs` result `TirType`/`Repr`; `ip_summary.rs::return_type` | attached |
| `leaf` | `call_graph.rs` `leaf_functions()` / `!makes_any_call` | DISCARDED → record on site |
| `no_throw` | `effects.rs::op_may_throw` + callee `has_exception_handlers` + builtin allowlist | DISCARDED |
| `no_alloc` | `escape_analysis.rs` (`Alloc`→`StackAlloc`) + callee alloc summary | DISCARDED |
| `no_escape_args` | `escape_analysis.rs` per-`ValueId` `EscapeState` | DISCARDED |
| `inlinable` | `inliner.rs::is_inlineable` (+ the 6 exclusion gates as `WhyNot`) | DISCARDED (recomputed) |
| `purity` | op_kinds.toml effect oracle (`purity`/`side_effecting`) | per-opcode only |
| `ownership_abi` | op_kinds.toml `operand_ownership` (the #70–#73 ladder) | generated — read it |

**Key insight: Phase 1 builds almost no new analysis — it stops *discarding*
what `call_graph`/`ip_summary`/`escape_analysis`/`inliner`/`effects` already
compute.** That is why this is high-leverage and low-risk.

## 3. Consumers — one fact, many unlocks (the "pop many into place")

- **inliner** — reads `inlinable` + `leaf` instead of recomputing; `WhyNot`
  drives the next coverage target.
- **call lowering (all 4 backends)** — `leaf` → frame-elision / no-spill fast
  path; `direct_target` → direct branch instead of the generic helper; this is
  where the heterogeneous-backend divergence (doc 46 §4.7) gets one shared fact
  to lower from.
- **refcount / ownership** — `no_alloc` + `no_escape_args` → elide inc/dec_ref
  across the call, pass args borrowed-not-owned (`ownership_abi`).
- **exception normal-edge (ties doc 45 ExceptionRegion)** — `no_throw` → the
  normal edge pays ZERO exception-stack churn; this is the AOT analogue of
  CPython-3.11 zero-cost exceptions and directly attacks `exception_heavy`'s
  ~12% exception-bookkeeping.
- **devirt / monomorphization** — `target` + `Guarded(class_version)` → direct
  dispatch under a class-version guard with a `deopt` edge.
- **generator fusion / typed return** — `typed_return` propagates `Repr` across
  the call so the result stays raw.

## 4. Why this is the root of the named perf reds (not isolated flaws)

| benchmark | missing call fact today | what CallFacts unlocks |
| --- | --- | --- |
| `fib` (#67: PyPy 0.51× / Codon 0.26×) | recursive self-call not direct; return boxed | `target=DirectCodePtr` self-call + `typed_return=I64` → unboxed-int recursion |
| `etl_orders` (#68 0.60×) | dataclass ctor + method calls generic; fields generic | `target` direct ctor + `Guarded` method devirt + `no_alloc` ctor + field-offset (→ doc 48 shapes) |
| `exception_heavy` (#77 0.68×) | normal-path calls not `no_throw` | `no_throw` normal edge → no exception-stack churn (with doc 45) |
| `class_hierarchy` (0.01×) | method target generic; bound-method allocated each call | `target=BoundMethod` `Guarded(class_version)` → no per-call bound-method alloc |
| `bytes_find` / `csv` | helper calls generic; string path allocates; return boxed | `direct_target` helper + `no_alloc` view path + `typed_return` |

If 71.4% of call-site facts are unrepresented, then many of these are the *same*
missing-representation bug wearing different benchmark names. That is the
compression-ladder payoff: one primitive retires a *class* of slowness.

## 5. Phased implementation (each phase moves coverage % and deletes a fallback)

- **Phase 1 — persist what's already computed.** `CallFactsAnalysis` side-table;
  fill `target` (typed), `typed_return`, `leaf`, `no_throw` (skeleton: opcode +
  callee-has-no-handlers + builtin allowlist). Inliner reads `inlinable`/`leaf`
  from it. Coverage 28.6% → target ≥ 60%. No backend change yet (representation
  first). Gate: `call_fact_coverage.py --check` ratchets UP; differential
  byte-identical (facts are advisory in Phase 1).
- **Phase 2 — `no_alloc` + `no_escape_args`** from escape analysis; first
  consumer: RC elision across proven `no_alloc ∧ no_throw` calls. Gate: leak
  corpus green + a warm-perf delta on a no-throw-heavy loop.
- **Phase 3 — `Guarded` targets** (class-version) + `deopt_or_guard`; devirt for
  `etl_orders`/`class_hierarchy`. Ties #71 (typed CallableTarget) + the guard/
  deopt machinery.
- **Phase 4 — backend lowering consumes `leaf`/`direct_target`/`no_throw`** on
  native first, then the OP_SUPPORT_MATRIX (doc 46 §4.7) tracks which backends
  lower each fact (the shared semantic contract the four dispatch paradigms lack).

## 6. The coverage tool becomes a performance dashboard

`tools/call_fact_coverage.py` (built) gains, as the facts graduate from
`Unknown`→`Proven`/`Guarded`:
- per-field aggregate coverage % (direct_target / typed_return / leaf / no_throw /
  no_alloc / noescape_arg / inlinable);
- `generic_call_fallback_by_reason` (the `InlineEligibility::WhyNot` + target-
  unknown reasons) — *why* each call stayed generic;
- a `--bench <name>` per-benchmark row (fib / etl_orders / exception_heavy /
  class_hierarchy / bytes_find) joining the census to a quiescent build's
  `typed_repr_report` dumps.
The `--check` ratchet turns every regression (a fact silently un-attaching, or a
benchmark's direct-call % dropping) into a red build. This is the FactGraph
(doc 46 §4.1) made concrete for the call subject.

## 7. Invariants (so this never becomes a second source of truth)

- `Unknown` is fail-closed: every consumer treats it as the pessimistic answer.
  A wrong `Proven` is a miscompile; a conservative `Unknown` is only a missed opt.
- `CallFacts.ownership_abi` and `purity` are READ from the generated op_kinds
  registry, never re-decided (the discovery-vs-authority rule, doc 46 §1).
- `target` is the typed `CallableTarget` (#71) — no raw marker bits ever cross
  into a fast path (the #59 class).
- Producers and the side-table invalidate together (same `AnalysisId` deps).

## 8. Status

DESIGN (this note). Phase 1 is the next build-bearing CallFacts lane, sequenced
AFTER ExceptionRegion Phase 1 (the open correctness root) frees build capacity —
the two share the `no_throw` fact, so ExceptionRegion lands first and CallFacts
Phase 1 consumes its normal-edge result. Related: doc 46 §4.1 (FactGraph), doc 45
(ExceptionRegion / `no_throw` consumer), #71 (CallableTarget = `target`), #67/#68
(devirt/etl consumers), `tools/call_fact_coverage.py` (the meter).
