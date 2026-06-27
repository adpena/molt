# INT-lane unification

The third complete structural cut of molt's scalar-repr canonicalization (after
the FLOAT-lane cut, `docs/design/float_lane_canonicalization.md`, and molt-check):
split the conflated integer carrier into a **three-tier int `Repr` lattice** and
**decouple Bounds-Check Elimination** from the raw-i64 safety proof so a
full-range checked accumulator used as an index can never silently skip its
bounds check.

Status: IMPLEMENTED THROUGH THE NATIVE AUTHORITY CUT (2026-06-25). This doc
began as the implementable spec; current code/tests are now authority for STEPs
1-5. The remaining frontier is STEP 6 value-keyed lowering/proof hardening plus
any backend parity checks that expose the same representation invariant.

This doc mirrors the FLOAT-lane cut's structure: dual-authority problem → the
real structural fork → the migration steps → the binding gates. The companion
artifacts are:
- `docs/design/int_lane_unification_migration_map.md` — the STEP-4 atomic
  `&ScalarRepresentationPlan` threading survey (every `int_primary_vars` site).
- `tests/differential/overflow_scalar/checked_int_overflow_slow_path.py` —
  Memory-Safety Gate 1 (slow-path value-correctness).
- `tests/differential/memory_safety/full_deopt_index_requires_bounds_check.py` —
  Memory-Safety Gate 2 (BCE non-elision).


## The problem: one `RawI64Safe` tier carrying two incompatible proofs

molt answers "is this scalar a bare i64?" for integers via the single
`Repr::RawI64Safe` tier (`runtime/molt-ir/src/repr.rs:56`). But that one tier is
minted from **two structurally different proofs** that the lattice cannot tell
apart:

1. **47-bit inline-window proof.** `raw_i64_safe_value_seed`
   (`runtime/molt-tir/src/representation_plan.rs:1213`) seeds an op result into
   `RawI64Safe` when `vr.fits_inline_int47(result)` proves its entire range fits
   the signed inline window `[-2^46, 2^46 - 1]`
   (`runtime/molt-passes/src/tir/passes/value_range.rs:522`). This is a genuinely
   bounded value: it fits a NaN-box payload, never overflows, never reaches a
   heap BigInt.

2. **Full-range overflow-peel proof.** The SAME seeder
   (`representation_plan.rs:1231`) ALSO inserts every `CheckedAdd` / `CheckedMul`
   result UNCONDITIONALLY, with no interval proof — because the accumulator is
   genuinely unbounded (that is the point of the peel). Its safety comes from a
   completely different mechanism: the hardware overflow flag gates dispatch, and
   the `overflow_peel` CFG re-executes the failed iteration on a boxed BigInt
   carrier. The seeder's own comment (`representation_plan.rs:1220-1230`) already
   describes this as a "FULL-RANGE raw-i64 carrier."

These two proofs are collapsed into one `RawI64Safe` tier, and the name-keyed
authority inherits the conflation: `int_carrier_names()`
(`representation_plan.rs:1669`) is exactly `{name | repr_by_name[name]
.is_raw_i64_safe()}`. So a full-range checked accumulator and a 47-bit-bounded
loop temporary are INDISTINGUISHABLE at the type level.

### Why the conflation is a latent P0, not just untidy

Two consequences follow from "the lattice cannot distinguish a 47-bit-bounded
value from a full-range checked carrier":

- **Cross-backend type confusion at the box boundary.** A `RawI64Safe` value
  reaching a value-keyed backend (WASM/LLVM/LIR) may be either. The 47-bit case
  can be inline-boxed directly; the full-range case MUST route through
  `emit_box_i64_overflow_safe`. Today the seeder comment asserts the invariant is
  held "because the 47-bit checked triple now requires an explicit value-range
  proof" — but that is an *unenforced* convention riding on one tier, not a
  lattice fact. A future edit that inline-boxes any `RawI64Safe` value silently
  reinterprets a wrapped 64-bit accumulator as a 47-bit SMI.

- **The BCE coupling trap.** BCE
  (`runtime/molt-passes/src/tir/passes/bce.rs:82`) proves index safety via
  `proves_index_in_bounds` / `proves_index_lt_len_symbolically`. It does NOT
  consult the Repr tier today — and that separation is exactly the invariant we
  must lock down. The moment anyone "optimizes" BCE to trust `is_raw_i64_safe`
  (or makes `proves_index_in_bounds` lean on the same full-range fact that
  promotes the carrier), a full-range RawI64FullDeopt accumulator used as a list
  index would be marked `bce_safe`, the fast lane would drop its bounds check,
  and an overflowing index would perform a SILENT OUT-OF-BOUNDS HEAP WRITE/READ.
  A false `bce_safe` is memory corruption, not a panic (bce.rs:7-9).

The structural fix is to give the two proofs **two tiers**, and to give BCE a
**separate, strictly-narrower index-safety query** that a full-range carrier can
never satisfy.


## SOLUTION: a three-tier int lattice + a decoupled BCE query

### The three int tiers (a proof refinement, not a coding convention)

Extend `repr.rs`'s int family to three strictly-ordered tiers, each with a
distinct proof source AND a distinct codegen path (no tier is "just a marker"):

```rust
pub enum Repr {
    Never,

    // Int family, by specificity of proof:
    RawI64FullDeopt, // NEW. Full i64 range [-2^63, 2^63-1]. Sound ONLY under the
                     // overflow_peel CFG: CheckedAdd/CheckedMul results whose
                     // every consumption is overflow-flag-guarded (DCE-dead on
                     // overflow, or And(flag==0, ...)-guarded), with the failed
                     // iteration re-executed boxed on the slow path.
    RawI64Safe,      // Proven inline-47 window [-2^46, 2^46-1]: fits a NaN-box
                     // payload, no overflow, raw machine ops sound with no flag.
                     // Proof: value_range::fits_inline_int47.
    MaybeBigInt,     // Conservative floor: NaN-boxed Python int, heap-possible.
                     // No proof. Unproven / polymorphic ints.

    Bool,
    RawF64,          // (FloatUnboxed today; renamed by the float cut's STEP 1.)
    DynBox,
}
```

**Tier semantics:**

- `RawI64FullDeopt` — a value is in this tier ONLY when **all three** hold:
  1. it is the `results[0]` of a `CheckedAdd` / `CheckedMul`
     (`representation_plan.rs:1231`), or value-identity-propagated from one;
  2. its consumption is gated by the overflow flag — dead on overflow, or
     explicitly `And(flag_is_false, …)`-guarded — i.e. the `overflow_peel` CFG is
     present and reachable;
  3. the slow path re-executes the iteration with boxed `molt_add` / `molt_mul`,
     never a raw machine op.
  Codegen: emit the checked machine op; at boxing sites emit
  `emit_box_i64_overflow_safe` (native: the existing `ensure_boxed_overflow_safe`
  lowering, unchanged) — NEVER an inline-47 box. The flag, not a range proof, is
  the gate.

- `RawI64Safe` — entire proven range fits `[-2^46, 2^46 - 1]`
  (`fits_inline_int47`). Codegen: emit the raw machine op; at boxing sites emit
  the inline box directly (the range proof guarantees the payload fits; no flag,
  no overflow check).

- `MaybeBigInt` — conservative floor, no proof. Codegen: emit boxed
  `molt_add` / etc. (heap-safe, BigInt-correct). `default_for(I64 | BigInt)`
  stays `MaybeBigInt` (`repr.rs:73`, unchanged).

### Lattice join rules

The current join (`repr.rs:91`) has exactly one non-top mixed int rule:
`RawI64Safe ⊔ MaybeBigInt = MaybeBigInt` (`repr.rs:98`). The new tier extends the
int chain as follows (all joins commutative + idempotent; everything else
fail-closed to `DynBox`):

| join | result | rationale |
| --- | --- | --- |
| `RawI64FullDeopt ⊔ RawI64Safe` | `RawI64FullDeopt` | full range subsumes the inline-47 window; the inline-47 value is trivially representable in the full-range checked carrier |
| `RawI64FullDeopt ⊔ MaybeBigInt` | `RawI64FullDeopt` | control-flow proof dominates: the checked carrier's overflow flag + slow-path re-execution already handle the heap-BigInt case, so the merge stays full-range-checked rather than collapsing to an unconditionally-boxed carrier |
| `RawI64Safe ⊔ MaybeBigInt` | `MaybeBigInt` | UNCHANGED (`repr.rs:98`) — an inline-47 value with no checked-op structure has no overflow flag, so a merge with a possibly-heap int must floor to boxed |
| `RawI64FullDeopt ⊔ {Bool, RawF64}` | `DynBox` | fail-closed across scalar families |
| `RawI64FullDeopt ⊔ Never` | `RawI64FullDeopt` | bottom identity |
| `RawI64FullDeopt ⊔ DynBox` | `DynBox` | top |

> **Asymmetry note (binding, must be a documented test).** `RawI64FullDeopt ⊔
> MaybeBigInt = RawI64FullDeopt` while `RawI64Safe ⊔ MaybeBigInt = MaybeBigInt`.
> This is deliberate and load-bearing: the full-deopt tier carries its own
> overflow flag + boxed slow path, so merging it with a possibly-heap int does
> not lose soundness (both the overflowed-fast and the heap-slow cases are
> already handled); the inline-47 tier has no such machinery and must floor. The
> `repr.rs` join doc and a unit test must assert BOTH rules so the asymmetry can
> never be "simplified" into a single rule that re-introduces the conflation.

### Decouple BCE: a separate, strictly-narrower index-safety query

BCE must NEVER share the fact that promotes a raw-i64 carrier. Add a new query in
`value_range.rs` and switch BCE to it:

```rust
// runtime/molt-passes/src/tir/passes/value_range.rs — NEW
/// The BCE-ONLY index-safety query. Strictly narrower than the raw-i64 carrier
/// proof: it proves 0 <= index < len AND that the index's proven range fits a
/// CONSERVATIVE index-safety window, so a full-range RawI64FullDeopt carrier can
/// NEVER satisfy it even when its slow-path boxed value happens to be in range.
pub fn proves_index_in_bounds_conservatively(
    &self, bid: BlockId, container: ValueId, index: ValueId,
) -> bool {
    (self.proves_index_in_bounds(bid, container, index)
        || self.proves_index_lt_len_symbolically(bid, container, index))
        && self.range_of(index).fits_inline_int47()
}
```

```rust
// runtime/molt-passes/src/tir/passes/bce.rs:82-83 — CHANGED
let proven = vr.proves_index_in_bounds_conservatively(*bid, container, index);
```

- `proves_index_in_bounds` (`value_range.rs:454`) and
  `proves_index_lt_len_symbolically` (`value_range.rs:497`) are UNCHANGED.
- `fits_inline_int47` (`value_range.rs:522`) is UNCHANGED — it is reused as the
  conservative window so a full-range carrier (whose range is `FULL_I64`,
  `value_range.rs:444`) fails the new conjunct outright.
- The narrower window is the deliberate choice: a full-range RawI64FullDeopt
  index satisfies neither `proves_index_in_bounds` (its range hi is `i64::MAX`,
  rejected at `value_range.rs:468`) NOR the new `fits_inline_int47` conjunct —
  belt-and-suspenders. The conjunct exists so that even if a future symbolic
  bound (`proves_index_lt_len_symbolically`) ever discharged a full-range index,
  the window guard still rejects it. BCE stays 47-bit-conservative; raw carriers
  go full-range under peel control. The two analyses can never re-couple.

This is the single most important structural safety invariant of the cut. It is
gated by Memory-Safety Gate 2 below.


## Relationship to the float cut

The FLOAT-lane cut (`c72371501`, `docs/design/float_lane_canonicalization.md`) is
build #2, landing on a separate worktree. Its STEP 1 renames `FloatUnboxed →
RawF64` atomically across molt-tir. That moots this doc's original STEP 7 rename:

> **On a clean `RawF64` base, the INT cut is 6 steps, not 7.** Do NOT re-do the
> `FloatUnboxed → RawF64` rename here. STEP 7 in the source design existed only
> because the rename was deferred into the int cut for atomicity; once the float
> cut lands it is already done. The int cut's STEPs 1–6 below assume `RawF64`.

The float cut established the exact migration shape this cut mirrors: it
stopped native handlers from owning a cloned raw-F64 membership set and migrated
"all 41 `fc/` handler files + `function_compiler.rs` as one structural arc"
(float doc, STEP 4). The int cut used the same full-arc shape for the deleted
`int_primary_vars` authority (1353 pre-migration audit occurrences; see the
migration map). The later native scalar-plan cuts completed that authority class
for native lowering: bool/F64 membership and raw-int membership now enter every
native handler through one `ScalarRepresentationPlan` reference and predicates,
and the plan's name-keyed carrier facts are folded into one `repr_by_name`
lattice instead of storing bool/F64 side sets beside the raw-int map.

One difference in expected outcome: the float cut was classified
PARITY-PRESERVING / TIE because the native float phi was already raw `f64`. The
int cut is NOT primarily a perf heal either — its product is the new full-range
tier + the BCE decoupling guard rail (a class of cross-backend type-confusion and
a class of silent-OOB made unexpressible). The CheckedAdd/CheckedMul carriers are
already raw on native today; the value is the *typed* separation, the enforced
box-site discipline on value-keyed backends, and the locked-in BCE decoupling.


## Migration: 6 steps (mirroring the float cut)

### STEP 1 — Repr enum + join + tests (1 file: `repr.rs`)
Add `RawI64FullDeopt`. `default_for` UNCHANGED (`I64`/`BigInt` still floor to
`MaybeBigInt`, `repr.rs:73`). Add the join rows above. Add `is_raw_i64_full_deopt(self)`
next to `is_raw_i64_safe` (`repr.rs:82`). The existing repr suite (66 tests) plus
new tests for the two asymmetric join rules and the new predicate must pass.

### STEP 2 — value-keyed full-deopt proof + cross-backend parity (2 files)
In `representation_plan.rs`, SPLIT the conflated seed:
- `raw_i64_safe_value_seed` (`representation_plan.rs:1213`) keeps ONLY the
  `fits_inline_int47` results (lines 1264-1276) and the proven-`[0,63]` shift
  case; it STOPS unconditionally seeding `CheckedAdd`/`CheckedMul`
  (delete the `representation_plan.rs:1231-1236` arm from this function).
- Add the internal full-deopt seed inside `repr_by_value_for`: seed
  `CheckedAdd`/`CheckedMul` `results[0]`, then propagate across value-identity
  edges (`Copy` chains and all-incomings phis) reusing the shared raw-i64
  propagation structure. This is the value-keyed full-range proof WASM/LLVM
  consume; do not expose it as a second carrier authority.
- `repr_by_value_for` (`representation_plan.rs:1120`) now inserts
  `RawI64FullDeopt` for the full-deopt set and `RawI64Safe` for the inline-47 set
  (full-deopt wins on overlap, per the join).
Unit tests: a `CheckedAdd` result is `RawI64FullDeopt` and survives a loop phi
all-incomings rule; a non-checked arithmetic result proven in `[-2^46,2^46-1]`
stays `RawI64Safe`; the GPU-intrinsic pre-seed
(`gpu_intrinsic_raw_i64_values`, `representation_plan.rs:1290`) stays
`RawI64Safe` (bounded `[0,2^20)`). Extend the cross-backend firewall
`wasm_and_llvm_derive_identical_repr_from_one_value_range` with full-deopt-tier
assertions.

### STEP 3 — project value-keyed int proof into `repr_by_name` (1 file)
`seed_repr_by_name` raises BOTH int tiers into `repr_by_name` by projecting the
TIR value-keyed `repr_by_value_for` authority through `SimpleValueNames`.
`RawI64Safe` comes from the inline-47 value-range proof and
`RawI64FullDeopt` comes from exact-i64 overflow/deopt carriers. The name-keyed
accessors are `is_full_deopt_int_name(name)`,
`is_inline_safe_int_name(name)`, and `is_raw_int_carrier_name(name)`;
`int_carrier_names()` returns the `RawI64Safe` view while
`int_raw_carrier_names()` covers "any raw int carrier" for native storage. There
is no legacy name-keyed interval proof to compare against; projection coherence
is proven by the representation-plan shard.

### STEP 4 — migrated ALL native consumers (atomic; see migration map)
Every native consumer of the deleted `int_primary_vars: &BTreeSet<String>` clone
now reads the int carrier from `ScalarRepresentationPlan` via
`is_raw_int_carrier_name`, `is_inline_safe_int_name`, or
`is_full_deopt_int_name`, depending on whether the site needs storage,
inline-boxing, or checked full-i64 box-site semantics. All `fc/` files,
`function_compiler.rs`, and `scalar_carriers.rs` moved as one structural arc; see
`docs/design/int_lane_unification_migration_map.md` for the pre-migration audit
and binding gate.

### STEP 5 — deleted the legacy seeding path (no hybrid)
The `let int_primary_vars = primary_names.int` derivation and native handler
threading path are gone. Raw-int proof now flows once through
`value_range_for`/`repr_by_value_for`, then projects into native names for
lowering. Name-keyed native code may consume the projection, but it must not
recompute carrier proof.
BINDING ZERO-OCCURRENCE GATE: `git grep int_primary_vars -- runtime/molt-backend`
== 0 (no second source of truth). Historical mentions in this design doc and
the migration map are provenance only.

### STEP 6 — value-keyed lowering consumes both tiers (3 files)
`lower_to_lir.rs`, `lower_to_wasm.rs`, `llvm_backend/lowering.rs`: recognize
`RawI64FullDeopt` (lowers to the same LIR/`i64` machine type as `RawI64Safe` — the
tier is a STATIC proof, not a runtime tag). The ONLY lowering difference is the
box site: a `RawI64FullDeopt` value boxes via `emit_box_i64_overflow_safe` (no
47-bit inline guard; the flag is the gate), a `RawI64Safe` value inline-boxes.
The checked-overflow triple (`result`, `boxed_overflow`, `overflow_flag`) already
exists; no new LIR/WASM opcode is needed. The LLVM backend already documents the
missing tier ("no speedup until the RawI64Full lattice extension lands",
`runtime/molt-backend/src/llvm_backend/lowering.rs:4638,4722`); this step is what
those comments are waiting on.


## Critical safety gates (binding)

### Memory-Safety Gate 1 — slow-path value-correctness
`tests/differential/overflow_scalar/checked_int_overflow_slow_path.py` (authored,
validated as plain Python). An int accumulator overflows i64 (sum past 2^63,
product = 25!, mixed doubling+add, and interleaved fast/slow accumulators); the
result must be bit-identical to CPython's arbitrary-precision value. Proves the
overflow flag gates dispatch, the slow path re-executes the iteration boxed, and
the fast lane is not corrupted by the slow lane. RawI64FullDeopt's soundness
relies ENTIRELY on the overflow_peel CFG being live; if it is missing, a wrapped
value is a silent wrong answer. Expected CPython output is recorded in the
fixture header. Run on native/WASM/LLVM post-int-cut-build.

### Memory-Safety Gate 2 — BCE non-elision (the decoupling invariant)
`tests/differential/memory_safety/full_deopt_index_requires_bounds_check.py`
(authored, validated as plain Python). A full-range RawI64FullDeopt accumulator
(CheckedAdd and CheckedMul) is used as a list index; the bounds check must NOT be
elided. Unguarded, the index walks OOB and CPython raises a deterministic
IndexError (molt must match, not read OOB heap); guarded by `< len`, it returns
the correct element (proving the decoupled conservative query does not break a
genuinely-safe full-range-carried index, only retains its runtime check).
Expected CPython output is recorded in the fixture header. Plus a molt-tir unit
test asserting the `Index` op is NOT marked `bce_safe` when its index is
RawI64FullDeopt, mirroring the existing bce.rs negative tests
(`bce.rs:539,629,648`).

> Why Gate 2 is the keystone: a false `bce_safe` on a full-range index is a
> silent OOB heap access (P0 memory corruption). The decoupling
> (`proves_index_in_bounds_conservatively`) makes that state unexpressible: the
> carrier proof and the index-safety proof are separate queries, and the
> index-safety query's `fits_inline_int47` conjunct rejects every full-range
> carrier.


## Risk assessment

**High:**
1. *Dynamic IV full-range proof* — `range(n)` with unproven-large `n` is a
   full-range IV; the existing `overflow_peel` heuristics (multi-preheader /
   ambiguous-IV disqualifiers) must keep rejecting the unsafe shapes. The
   keystone CheckedMul doc (`docs/design/keystone_checkedmul_loop_unbox.md`)
   already records GAP-3 dynamic-IV as BLOCKED precisely because widening it
   wrong is a silent-OOB BCE hazard — Gate 2 is the regression that guards it.
2. *Cross-backend Repr divergence* — native projection vs value-keyed proofs
   could disagree if `SimpleValueNames` loses provenance. The representation
   shard checks projection coherence, and the STEP-2 cross-backend parity test
   ensures WASM/LLVM agree.
3. *Slow-path re-execution correctness under complex control flow* — early-exit,
   conditional accumulation, nested loops on the slow path. The interleaved
   fast/slow case in Gate 1 exercises shared-saved-value re-execution; extend
   with a nested/early-break fixture if STEP 6 lowering changes the CFG.

**Medium:**
1. *Box-site discipline on value-keyed backends* (STEP 6) — a `RawI64FullDeopt`
   value boxed via the inline-47 path would mis-encode a wrapped value. Gate 1's
   printed BigInt result diverges if this happens.
2. *Native handler churn* (STEP 4) — retired in the 2026-06-25 native authority
   cut: 1353 pre-migration `int_primary_vars` occurrences across 44 files moved
   to plan predicates as one threading change. Keep the zero-occurrence gate live.

**Low:**
1. `default_for` (no change). 2. Other-lane joins (fail-closed, no change).
3. Full-deopt value propagation (mirrors the existing `RawI64Safe` propagation).


## Ready-to-implement gate

Ready: **YES**, conditioned on the FLOAT-lane cut having landed (so `RawF64`
exists and STEP 7 is moot), AND:
1. The three tiers are load-bearing — each has a distinct proof source and box
   path; none is a redundant marker (verified against the current conflated
   seeder at `representation_plan.rs:1213-1281`).
2. BCE decoupling is proven safe in isolation — `proves_index_in_bounds_
   conservatively` is defined and a differential (Gate 2) proves full-range
   accumulators are never `bce_safe`.
3. The overflow_peel CFG is sound — the pass already re-executes on the slow
   path; Gate 1 gates it; no new loop verification is introduced.
4. Cross-backend parity is achievable — WASM/LLVM already derive Repr from the
   value-range; adding full-deopt seeding preserves the firewall.
5. The migration is atomic — all native `int_primary_vars` consumers migrated
   together (STEP 4 is one arc, per the migration map).
