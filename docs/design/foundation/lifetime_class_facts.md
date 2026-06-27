<!-- Spine-4 Outcome 1, Mechanism M1 — the LifetimeClassFacts 4-bitset, as ONE
cached fact-plane. Build-free design artifact. Author: Lane-A prep agent.
Date: 2026-06-25. Status: DESIGN ONLY / EXECUTABLE PLAN.
PARENT BLUEPRINT: docs/design/foundation/55_memory_safety_ownership_lattice.md
§2.1 (M1). SIBLING: ownership_lattice_phase0.md (the trust root that gates this
fact's G1 parity assertions; §3 there is the trust-root-gating summary, THIS doc
is the deep single authority for the bitset). GOVERNING: CLAUDE.md Council
Operating Doctrine — "FinalizerSensitive = ONE ClassInfo/MRO/version-derived
cached fact consumed by escape + refcount-elim + stack-alloc + Free-eligibility +
ownership-lowering (no pass-local finalizer reasoning)", generalized from one
class to four. Never duplicates/contradicts doc 55; pins it to verified
file:line. -->

# LifetimeClassFacts — the One Cached Lifetime Fact-Plane (M1)

## 0. The mandate, in one sentence

There is **exactly one** place in the compiler that decides whether a heap object
has a non-trivial lifetime boundary, it derives that from
ClassInfo/MRO/class-version once, and **every** pass that makes a
lifetime/placement/Free decision reads it — so two passes can never disagree
about finalizer/weakref/resurrection/ordering significance, which is the disagreement
that produces a use-after-free.

This is doc 55 §2.1 (M1) and the council's `FinalizerSensitive`-is-one-fact
mandate, **generalized from one boundary class (finalizer) to four** (finalizer,
weakref, resurrection, inner-ref ordering) so the same single-authority discipline
covers the whole `Py_DECREF`→0 obligation set. It retires the class doc 55 names:
*"a pass re-derives finalizer/weakref reasoning and gets it subtly different from
another pass."*

Build-free design. All file:line below were read from the live tree (2026-06-25).

---

## 1. The four bits — derivation, completeness, verified current state

CPython's `Py_DECREF`→0 does exactly four lifetime-significant things beyond the
byte free (doc 55 §1.4): `tp_finalize`/`tp_del`; `PyObject_ClearWeakRefs`;
ordered subobject release; `tp_dealloc`. molt's `dec_ref_ptr`
(`runtime/molt-runtime/src/object/mod.rs:1994`) mirrors this — resurrection abort
(mod.rs:2175), `weakref_clear_for_ptr` (mod.rs:2187), per-`type_id` inner-ref
release (mod.rs:2188+), byte free. The four boundary facts are exactly the
non-byte-free obligations; there is no fifth because the byte free is
unconditional and carries no ordering/resurrection hazard.

| Bit | Predicate meaning | Derivation / source of truth | Verified current state (2026-06-25) |
|---|---|---|---|
| **B0 MayFinalize** | class/MRO defines `__del__` | runtime `HEADER_FLAG_CLASS_HAS_FINALIZER` (mod.rs:490), set by `class_refresh_finalizer_flag` (mod.rs:1493-1501) via `class_lookup_raw_mro_dict_attr(__del__)`, sealed once by `class_finish_definition` (mod.rs:1512-1516). Compile-time mirror: existing `defines_del` result attr. | EXISTS. Reused verbatim — no behavior change to #58. |
| **B1 HasWeakrefs** | an instance can be the target of a live `weakref`/`proxy` (CPython `tp_weaklistoffset != 0`) | runtime NEW `HEADER_FLAG_CLASS_SUPPORTS_WEAKREF`, refreshed on the SAME MRO/version hook beside B0 (mod.rs:1493 region), copied to instance flag on `object_set_class_bits` (mod.rs:1417). Compile-time mirror: NEW `class_supports_weakref` result attr. | NEW. grep confirms no `SUPPORTS_WEAKREF`/`weaklistoffset` anywhere in runtime/. |
| **B2 MayResurrect** | `__del__` can re-root `self` | DERIVED: `= MayFinalize` (conservative; any `__del__` can stash `self`). Not stored. | Derived from B0. |
| **B3 InnerRefOrdering** | object owns ref-counted fields whose release order a `__del__` can observe | DERIVED: `MayFinalize ∧ HEADER_FLAG_HAS_PTRS` (mod.rs:446; doc 49 field-ownership). Compile-time mirror of the ptr-field half: NEW `class_has_ptr_fields` result attr. | `HEADER_FLAG_HAS_PTRS` EXISTS (mod.rs:446, set by `object_mark_has_ptrs` mod.rs:1518). |

**Trivial lifetime** ⇔ all four false ⇔ `¬B0 ∧ ¬B1` (B2 ⊆ B0 and B3 ⊆ B0, so
the finalizer and weakref bits dominate). A Trivial object's `dec_ref_ptr` does
only the unconditional byte free — which is what makes `Free` (M3) sound for it
and ONLY it.

---

## 2. The compile-time struct + the ONE fixpoint

In `runtime/molt-passes/src/tir/passes/ownership_lattice_min.rs`, beside the existing
`finalizer_sensitive_roots` machinery (lines 564-683), add (doc 55:183-202):

```rust
#[derive(Clone, Debug, Default)]
pub(crate) struct LifetimeClassFacts {
    may_finalize_roots: HashSet<ValueId>,   // = today's finalizer_alloc_roots ∪ container closure
    has_weakref_roots: HashSet<ValueId>,    // NEW seed: class_supports_weakref attr
    has_ptr_field_roots: HashSet<ValueId>,  // NEW seed: class_has_ptr_fields attr
}
impl LifetimeClassFacts {
    pub(crate) fn is_trivial_lifetime_root(&self, r: ValueId) -> bool {
        !self.may_finalize_roots.contains(&r) && !self.has_weakref_roots.contains(&r)
    }
    pub(crate) fn may_finalize_root(&self, r: ValueId) -> bool { self.may_finalize_roots.contains(&r) }
    pub(crate) fn has_weakref_root(&self, r: ValueId) -> bool { self.has_weakref_roots.contains(&r) }
    pub(crate) fn may_resurrect_root(&self, r: ValueId) -> bool { self.may_finalize_root(r) }
    pub(crate) fn inner_ref_ordering_root(&self, r: ValueId) -> bool {
        self.may_finalize_root(r) && self.has_ptr_field_roots.contains(&r)
    }
}
```

**One fixpoint, three stored bitsets.** The three seed sets flow through the
SAME container-absorption fixpoint that already produces
`finalizer_sensitive_roots` (ownership_lattice_min.rs:564-683): a container that
holds a weakref-able / finalizer-able / ptr-field object is itself
boundary-significant for release ordering. This is NOT four analyses — it is the
existing fixpoint seeded with two more bitsets. `finalizer_sensitive_roots`
remains the `may_finalize` view (zero behavior change to #58 ordering).

**Seeds (frontend → TIR).** `src/molt/frontend/visitors/classes.py` emits two NEW
allocation-op result attrs beside the existing `defines_del`:

- `class_supports_weakref: bool` — seeds `has_weakref_roots`.
- `class_has_ptr_fields: bool` — seeds `has_ptr_field_roots`.

These are the compile-time mirror of the runtime header flags, so the placement
decision reads a fact rather than re-deriving finalizer-sensitivity per pass.
(Frontend touches `visitors/classes.py`, a doc 21c/F1 mixin target — sequence the
small attr emission before/with F1's class-visitor extraction; the change is
move-stable.)

**The classifier is GENERATED, not hand-matched.** Add a `lifetime_class`
classifier set to `runtime/molt-ir/src/tir/op_kinds.toml` + `tools/gen_op_kinds.py`
so which ops carry/propagate lifetime facts is a generated authority, never a new
`matches!` semantic-fallthrough (STRUCTURAL_AUDIT deletion-candidate discipline;
`tools/gen_op_kinds.py --check` is the gate).

---

## 3. The single-authority consumer contract (binding)

Every consumer reads `LifetimeClassFacts`; none re-derives. This table is the M1
acceptance: after M1, grep for finalizer/weakref reasoning must find it ONLY in
`LifetimeClassFacts`, with each site below reading the struct.

| Consumer (file:line, verified) | Today's narrow query | M1 replacement |
|---|---|---|
| **refcount-elim Step 5** (`refcount_elim.rs:586,605`) — RC strip | `!finalizer_roots.contains(&root)` (finalizer-only) | `is_trivial_lifetime_root(root)` (all four) |
| **refcount-elim Step 6** (`refcount_elim.rs:698-704`) — `DecRef→Free` (M3) | `!finalizer_roots.contains(&val)` at line 701 | `is_trivial_lifetime_root(aliases.root(val))` |
| **escape analysis** (`escape_analysis.rs`, `finalizer_alloc_roots`) — stack-promotion decline | finalizer-only guard | `¬is_trivial_lifetime_root` |
| **stack-allocation** | finalizer-only | `¬is_trivial_lifetime_root` blocks stack alloc |
| **ownership-lowering / rung-3 placement** (`ownership_lattice_min.rs:495-510`; `drop_insertion.rs` queries `is_finalizer_sensitive_root`) — M2 | `is_finalizer_sensitive_root` | `¬is_trivial_lifetime_root` ⇒ Python-lifetime-boundary release |

The M3 site is the sharpest: `refcount_elim.rs:698-704` currently promotes
`DecRef→Free` under `*balance == 0 && alloc_vals.contains(&val) &&
!heap_exposed... && !finalizer_roots.contains(&val)`. The `finalizer_roots`
clause (line 701) is the **single-class** guard; its own rationale
(refcount_elim.rs:643-659) admits it is "defense-in-depth … disjoint by
construction" and warns it "keeps it true if a finalizer alloc ever reaches this
set." M1 supplies the fact that lets M3 replace that fragile one-class guard with
`is_trivial_lifetime_root` — all four classes, the structurally complete gate.
The Step-6 guard tests (refcount_elim.rs:1566-1690, currently asserting "no
`OpCode::Free`" for finalizer values) are extended to all four classes in M3/G3.

---

## 4. Fail-closed posture (binding)

An allocation whose class facts are UNKNOWN — a dynamically-typed `Call` result
with no `defines_del`/`class_supports_weakref` attr — is `¬Trivial`: all four
facts conservatively TRUE (doc 55:216-222). Rationale: over-conservatism costs a
finalizer-aware DecRef branch (a tag check); under-conservatism costs a UAF. The
constitution's choice is forced. The generated `op_kinds.toml` classifier already
fails closed for `non_owning_copy` (ownership_lattice_min.rs:282-307); the
`lifetime_class` set extends the identical posture. `is_trivial_lifetime_root`
returns `false` for any root not positively proven Trivial.

---

## 5. Two refinements that are FACTS, not Free special-cases (doc 55 §6)

Both risks below are resolved by making the FACT more precise — never by relaxing
`is_trivial_lifetime_root` (which would re-open the corruption class).

- **HasWeakrefs over-breadth.** Most CPython classes support weakrefs, so a naive
  B1 = "class supports weakref" makes nearly everything `¬Trivial` and kills
  Free-perf. Refine B1 for *release* purposes: load-bearing only when a weakref to
  the instance can actually exist — `class_supports_weakref ∧ (a weakref/proxy was
  created in-program OR escape says the instance escapes to where one could be)`.
  This is a rung-2 fact refinement measured against the weakref differential
  corpus, NOT a guard inside Free.

- **Conservatism kills Free-perf** (every dynamic alloc `¬Trivial`). The fix is
  MORE facts: when a `Call` result's class is provably Trivial (monomorphic
  call-site + class-version guard — the IC/shape facts the Performance Constitution
  names), the fact-plane learns it and Free re-fires. Report any Free-loss as a
  missing-fact in the PERF/SPEED STATUS block; never widen the gate to recover
  perf. The lattice is the extension point (a genuine fifth boundary class is a
  fifth bitset consumed by the same `is_trivial_lifetime_root`).

---

## 6. Gate (G1) — fact-only, no placement change

This phase ONLY computes and exposes the four facts and proves them against
CPython class semantics. Placement/lowering behavior changes land in M2/M3.

- `cargo test -p molt-backend --lib --features native-backend
  ownership_lattice_min` — extend the existing 20+ fixtures
  (ownership_lattice_min.rs:835-1617) with weakref/ptr-field/resurrection
  fixtures mirroring `container_absorbing_*` / `nested_container_propagates`:
  prove `has_weakref_root` propagates through the container fixpoint exactly as
  the finalizer seed does.
- `tools/gen_op_kinds.py --check` green (the `lifetime_class` set is generated).
- **Parity probe** (under the Phase-0 honest gauge, ownership_lattice_phase0.md):
  `has_weakref_root` agrees with CPython `cls.__weakref__`-availability for
  `tests/differential/basic/dict_subclass_slots_weakref*.py`.
- **Byte-identical TIR/artifacts** on the full differential corpus (fact-only).
  `structural_audit.py --check` does not regress; `duplicate_authorities` stays 0
  (the new classifier is generated, not a second hand-matched authority).

---

## 7. Why this is structurally correct (constitution check)

- **One authority** (council mandate): `LifetimeClassFacts` is the single
  finalizer/weakref/resurrection/ordering fact; §3 enumerates every consumer
  reading it; no pass re-derives.
- **Fixes the representation** (Performance Constitution): the missing fact was
  "this object's lifetime class"; add it once, every consumer reads it. We do NOT
  add a guard per finalizer composition (the rejected DropInsertion-special-case
  path doc 55 §1.1).
- **No partial / no asymmetry** (Zero-Workarounds): all four classes seeded
  through the one fixpoint, all five consumers migrated together. The current
  finalizer-only state (refcount_elim.rs:701) IS the asymmetric partial the
  constitution forbids; M1+M3 close it symmetrically.
- **Completeness proven** (doc 55 §1.4): four facts = the exact non-byte-free
  `Py_DECREF`→0 obligations; a fifth is a fifth bitset, not a new guard.

*Design only / executable plan. No code changed in this session. Co-Authored-By:
Claude Opus 4.8 <noreply@anthropic.com>*
