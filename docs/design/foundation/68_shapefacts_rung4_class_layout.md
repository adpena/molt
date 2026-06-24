<!--
Foundation blueprint 68 — ShapeFacts: the perf compression-ladder Rung 4.
Arc: PERFORMANCE FRONTIER (Lane B). The class made unexpressible-as-slow:
the shape-blind dynamic attribute load/store + method dispatch.
Author: portfolio-architect.
Date: 2026-06-24.
Status: DESIGN ONLY / EXECUTABLE PLAN. No implementation landed by this doc.

NUMBERING NOTE: assigned path docs/design/foundation/68_shapefacts_rung4_class_layout.md
(68 was the next free slot in the 68-79 range; 54-67 are taken).

All file:line anchors verified read-only against the worktree on 2026-06-24.
Code beats this doc when it drifts. DEEPENS doc 65 §Rung 4; feeds Rung 2 (doc 47)
+ Rung 3 (representation_plan); uses doc 59 (fact authority) + doc 64 (the gate).
-->

# 68 — ShapeFacts: ClassShape / FieldOffset / ClassVersionGuard (Rung 4 of the ladder)

## 0. End-state outcome (the time-traveler's destination)

**In the end state, "boxed dict-based attribute access" is an unexpressible-as-slow
class whenever the shape is statically known.** The Pythonista writes `obj.attr`,
`obj.attr = v`, and `obj.method()`; the Rustacean gets a shape-indexed fixed-offset
field load / store and a direct (or inline-cached) call. The dynamism is not erased —
it is *represented precisely enough* to compile away its cost when the shape is
`Proven`, executed under a `ClassVersionGuard` with a sound deopt edge when the shape
is `Guarded`, and routed to the existing full dynamic path (fail-closed) when `Unknown`.

Concretely, at the destination, for a monomorphic `UserClass` receiver with a stable
layout:
- `obj.attr` (`LoadAttr`) lowers to a single `load [obj_ptr + offset]` (feeds Rung 3
  Repr: the loaded value carries the slot's `Repr`, so a `RawI64Safe` field stays
  unboxed across the access), with **no** runtime `class_layout_size` / MRO-walk /
  `__molt_field_offsets__` dict lookup.
- `obj.attr = v` (`StoreAttr`) lowers to `store [obj_ptr + offset], v` with the slot's
  ownership discipline (a `DecRef` old + `IncRef` new only where `FieldSlot.ownership`
  proves a reference slot — doc 49's object-field ownership, *consumed not re-derived*).
- `obj.method()` (`CallMethod`) feeds Rung 2: `CallFacts.target = MethodDescriptor`
  + `FactValue::Guarded(class_version)` -> a direct call (no per-call bound-method
  allocation, no IC megamorphic dispatch).
- A `Proven` `FieldOffset` load is `ProvenPure` for doc 02 MemorySSA/LICM — a loop-
  invariant `obj.attr` read hoists out of the loop; SROA can promote a shaped field.

And the soundness floor: **every shape-changing operation in the allowed subset
(class `__dict__`/`__class__` mutation, `setattr`/`delattr` on the class, an
allowed-subset monkeypatch) bumps the class version, and every `Guarded` access
re-checks that version and deopts to the proven-slow dynamic path on mismatch.**
There is no exec/eval/compile and no unrestricted monkeypatch in the molt subset
(CLAUDE.md), which is precisely what bounds the set of shape mutations the guard must
cover.

This rung closes the **single largest greenfield hole** (doc 46 §3 Q6: "0% — there is
no shape system"; doc 65 §2 "ShapeFacts / ClassShape / FieldOffset / ClassVersionGuard
DOES NOT EXIST … the largest single hole"). It is the PyPy **maps / hidden-classes**
mechanism and the V8 "shapes" mechanism, delivered AOT via a `FactValue`-typed IR fact.

### 0.1 The load-bearing discovery that reframes this entire rung

A read-only sweep of the worktree establishes a structural fact that the "0% — no shape
system" finding must be read against precisely: **the runtime and all four backend
lowerings for shape-guarded direct field access ALREADY EXIST and are live. What does
not exist is the TIR FACT.** The shape *decision* is made today as an **untyped,
un-validated, second-authority heuristic resident in the Python frontend**, dissolved
into kind-strings before TIR ever sees it. This is the exact anti-pattern doc 46 §0
names ("a Python-visible fact — object shape — is dissolved into low-level SSA early,
then reconstructed by fragile per-pass analysis"). The evidence:

| Layer | What already exists | Where |
|---|---|---|
| **Runtime guard** | `molt_guard_layout_ptr(obj, class, expected_version) -> bool` — reads class slot-4 version, compares, profiles deopt | `runtime/molt-runtime/src/object/accessors.rs:393` |
| **Runtime guarded field ops** | `molt_guarded_field_get_ptr` / `_set_ptr` / `_init_ptr` — version-guard THEN direct offset access, **with the exact CPython-correct deopt fallback to `molt_get_attr_ptr`/`molt_set_attr_ptr` on guard fail** | `accessors.rs:414`, `:442`, `:468` |
| **Runtime offset authority** | `class_field_offset(class, attr)` — walks the **MRO**, reads `__molt_field_offsets__` per class; `class_own_slot_field_offset` handles `__slots__` | `runtime/molt-runtime/src/builtins/attr.rs:1167`, `:1235` |
| **Runtime version** | per-class layout version at **class object slot 4** (`class_layout_version_bits`/`_set_`), `class_bump_layout_version` ALSO bumps the **global type version** for IC invalidation | `runtime/molt-runtime/src/object/layout.rs:1311`, `:1376` |
| **Backend lowering (all 4)** | `guarded_field_get`/`_set`/`_init` + `guard_layout`/`guard_dict_shape` lowered on native, LLVM, WASM, Luau | native `.../fc/memory.rs:1156-1573`; LLVM `lowering.rs:1612`,`:1900`,`:10429`,`:12439`; Luau `luau.rs:3144`; WASM `wasm.rs` |
| **Op-kind vocabulary** | `guarded_field_get -> LoadAttr`, `guarded_field_set/_init -> StoreAttr`, `guard_layout/guard_dict_shape/guard_layout_ptr/guard_type` classified | `op_kinds_generated.rs:59-74`, `:255-259`, `:499`; `classifier_inert_marker` in `op_kinds.toml:508` |
| **The DECISION (the gap)** | `field_offset(cls, attr)` reads the frontend `self.classes[cls]["fields"][attr]` dict; `_class_layout_stable` (heuristic) gates it; `_collect_module_class_mutations` is the monkeypatch boundary; the op is emitted with a string `"class"` + a runtime-read `expected_version` | frontend `lowering/serialization.py:481`, `:2283`; `__init__.py:3291`, `:2512` |

**Therefore Rung 4 is not "build a shape system from scratch." It is the structural
correction the doctrine demands: lift the shape decision out of the frontend's ad-hoc
Python predicates into a `FactValue`-typed, `AnalysisManager`-cached, validator-gated,
coverage-censused TIR analysis (`shape_facts.rs`), keyed off the landed
`TirType::UserClass`, that PRODUCES the same `guarded_field_*` ops the backends already
lower — but now as a *proven fact with a deopt obligation*, not a frontend guess.** This
retires a *duplicate-authority* (the structural_audit `duplicate_authorities` metric)
and an *un-validated soundness boundary* simultaneously. The "0% shape system" finding
is true at the layer that matters (the IR fact plane); the runtime plumbing existing is
what makes this rung *low-risk to land and immediately measurable*, exactly as Rung 2
(CallFacts) was "completion not greenfield" because `call_graph`/`escape_analysis`
already computed the facts (doc 47 §2).

> **Refusal recorded (deletes a bad plan).** The naive plan — "add a `ShapeFacts` field
> to the frontend `ClassInfo` dict and emit more `guarded_field_get` ops from
> `serialization.py`" — is **REJECTED**. It deepens the second authority this rung
> exists to retire: the shape fact would still live in untyped Python, still have no
> `FactValue` confidence, still have no `ClassVersionGuard` as a typed deopt obligation,
> still be invisible to `AnalysisManager` invalidation, MemorySSA, SROA, and the
> CallFacts devirt consumer, and still have no Alive2-style validator. The
> structurally-correct design is **one TIR analysis (`AnalysisId::ShapeFacts`) that is
> the single authority for "what is this object's shape and is it safe to access by
> offset," consumed by every pass and lowered by every backend from one fact** (the
> doc 59 fact-plane rule; the doc 46 §4.7 backend-matrix rule). This is the load-bearing
> architectural decision of this document.

---

## 1. The method: a rung is a fact, not a pass (binding restatement)

Per CLAUDE.md ("fix the REPRESENTATION, not the pass") and doc 65 §1, Rung 4 is
*complete* only when (a) a new fact family `ShapeFacts` exists as a typed, cached,
serializable record in `runtime/molt-tir/src/tir/`, (b) every consumer (field
load/store lowering, CallMethod devirt, MemorySSA/SROA, the frontend) reads the fact
instead of re-deriving the shape, (c) a validator turns the fact into a checkable
obligation (the deopt-correctness differential — every `Guarded` access's guard-fail
path is exercised by a class-mutation test), and (d) the scoreboard rows the fact
targets are GREEN warm-AND-cold across native/WASM/LLVM/Luau and *stay* green because
the shape-blind lowering is now structurally absent when the shape is `Proven`.

**Shared substrate this rung reuses (does NOT fork — doc 65 §1, doc 65 §4):**
- The confidence lattice `FactValue { Proven | Guarded(GuardId) | Profiled(Confidence)
  | Unknown | False }` (`runtime/molt-tir/src/tir/call_facts.rs:117`). `Unknown` is the
  fail-closed default. A wrong `Proven` is a miscompile; a wrong `Guarded` deopt is a
  miscompile; a conservative `Unknown` is only a missed opt. **`GuardId(u32)`**
  (`call_facts.rs:102`) is the existing typed guard token — `ClassVersionGuard` is a
  `GuardId`, not a new id space.
- The cached-analysis machinery: a new `AnalysisId::ShapeFacts` variant
  (`tir/analysis/mod.rs:66`, currently 14 variants ending at `ExceptionRegions`),
  the `Analysis` trait (`:123`: `CFG_SENSITIVE`/`OPS_SENSITIVE`/`compute`), fail-closed
  `invalidate_cfg`/`invalidate_ops`, the `MOLT_VERIFY_ANALYSIS=1` self-check.
- The interprocedural seeding pattern: a new `ShapeFactsTable::build_module` in the
  module phase + `AnalysisManager::prepopulate` (`analysis/mod.rs:349`), mirroring
  `CallFactsTable::build_module` (`call_facts.rs:368`). Class layout is a *module-wide*
  fact (the class def + MRO live across functions), so the precise table is built
  whole-program and seeded; the per-function `compute` is the fail-closed `Unknown`
  floor — sound by construction, never out-claiming the precise table (the exact model
  doc 47 §1 / `call_facts.rs:55-63` established and proved monotone).
- The generated op-semantics registry: `op_kinds.toml` -> `gen_op_kinds.py --check`
  (doc 25, doc 59). The shape-guard ops are *already* in the registry vocabulary; this
  rung makes the registry the **one generated authority** for their semantics (§6).

**Measurement discipline (the gate, doc 64):** every sub-phase reports
`benchmark -> target -> backend -> profile -> CPython ratio -> PyPy ratio -> Codon
ratio -> binary size -> peak RSS -> compile time -> log artifact` via
`tools/perf_scoreboard.py` (the warm/cold split), `>=5` samples, CV stability,
classified GREEN_STABLE/RED_STABLE/RED_NOISY/TIE/DIMENSIONAL_WIN. A DIMENSIONAL_WIN
(alloc/RSS improved, warm gate did not flip) is reported as dimensional, never as a
speed heal — and this rung's gate requires an **alloc/dispatch-CYCLE win**, not just an
alloc-count win, so the warm gate flipping is the bar.

---

## 2. The ShapeFacts IR representation (the new fact family)

A **new** module `runtime/molt-tir/src/tir/shape_facts.rs`, registered as
`AnalysisId::ShapeFacts`, built on `TirType::UserClass(String)` (`types.rs:46`, which
*already documents this exact use*: "prove static field offsets for direct load/store",
"prove monomorphic method receivers for direct dispatch"). Class identity is the
qualified class name (the frontend already deduplicates these — `types.rs:44`), so a
`ClassId` is the interned qualified name string, matching the `UserClass(String)` key
and the existing `"class"` attr carried on `guarded_field_get` (`serialization.py:2295`).

### 2.1 The records (all `FactValue`-typed where confidence applies)

```rust
/// A class's qualified name — the same key TirType::UserClass(String) carries
/// and the same string the frontend deduplicates and stamps on the op `"class"` attr.
pub struct ClassId(pub String);

/// A monotone version token. Mirrors the runtime per-class layout version stored at
/// class object slot 4 (layout.rs:1311) and bumped — together with the GLOBAL type
/// version (layout.rs:1382) — on every allowed-subset shape mutation. The IR carries
/// the version the shape fact was PROVEN against; the ClassVersionGuard re-checks it.
pub struct ClassVersion(pub u64);

/// One instance-field slot in the monomorphic layout. `offset` is the byte offset
/// authoritative against the runtime `class_field_offset` MRO-walk (attr.rs:1167) /
/// `class_own_slot_field_offset` for __slots__ (attr.rs:1235). `repr` feeds Rung 3:
/// the loaded value carries this Repr (RawI64Safe int stays unboxed across the load).
/// `ownership` is doc 49's object-field ownership — CONSUMED here, never re-derived.
pub struct FieldSlot {
    pub name: String,              // attribute name (the `s_value` on the op)
    pub offset: u32,               // byte offset from the object payload base
    pub repr: Repr,                // repr::Repr of the slot value
    pub ownership: FieldOwnership, // Owned reference | Raw scalar (doc 49)
    pub source: SlotSource,        // Annotation | Slots | DataclassField (provenance)
}

/// The monomorphic layout of a class: the ordered field slots derived from the class
/// def + its MRO + __slots__. The single authority replacing the frontend ClassInfo
/// `fields`/`field_order`/`mro`/`slots` dict (classes.py:934).
pub struct ClassShape {
    pub class: ClassId,
    pub version: ClassVersion,
    pub fields: Vec<FieldSlot>,              // by offset order; index = MRO-merged slot
    pub field_index: BTreeMap<String, u32>, // attr name -> index into `fields` (det.)
    pub stability: ShapeStability,           // why this shape is/isn't trustable (§3)
}

/// The per-ACCESS fact attached to a LoadAttr/StoreAttr op, keyed by the op's result
/// (or operand) ValueId in the per-function ShapeFactsTable — exactly as CallFacts is
/// keyed (call_facts.rs:330). This is the fact a backend lowers from.
pub struct FieldAccessFact {
    pub class: ClassId,
    pub slot: u32,              // index into ClassShape.fields
    pub offset: u32,            // denormalized for the lowering (= fields[slot].offset)
    pub repr: Repr,             // = fields[slot].repr (Rung 3 propagation)
    pub access: FactValue,      // Proven (no guard) | Guarded(class_version) | Unknown
    pub guard: Option<GuardId>, // Some(class_version_guard) iff access == Guarded
}

/// The deopt guard token. A `GuardId` (call_facts.rs:102) — NOT a new id space.
/// Conditions a `FactValue::Guarded` access on the runtime class version matching
/// `ClassShape.version`; the guard-fail edge is the proven-slow dynamic path.
pub type ClassVersionGuard = GuardId;

/// Value-slot flow for a stable-key dict (the csv/etl row-dict path). Phase 4c.
pub struct DictShape {
    pub keys: StableKeySet,     // the proven-stable key set (string literals)
    pub value_repr: Repr,       // the homogeneous value Repr if proven, else DynBox
    pub access: FactValue,      // Guarded(dict-shape-guard) | Unknown
}
```

`FieldAccessFact::access` is **`Proven`** iff the receiver is a `UserClass(c)` whose
`ClassShape` is `ShapeStability::Frozen` (no allowed-subset mutation can change the
layout — e.g. a `__slots__` class with no `__dict__`, no monkeypatch, no metaclass
`__setattr__`, see §3); **`Guarded(class_version_guard)`** for a stable-but-mutable
layout (direct offset access under a version guard with a deopt edge — the common case,
and the one the existing `molt_guarded_field_get_ptr` path already implements); else
**`Unknown`** -> the current full runtime lookup (`get_attr_generic_ptr` /
`molt_get_attr_ptr`), fail-closed.

### 2.2 Why a `ClassShape` table + a per-access `FieldAccessFact`, not one record

The split mirrors CallFacts' `CallFactsTable` (per-function, keyed by `ValueId`) vs the
module-wide call graph. `ClassShape` is a **module-level** fact (one per class,
interprocedural, built once in the module phase). `FieldAccessFact` is a
**per-function, per-op** fact (keyed by the access op's `ValueId`, cached/invalidated
per function exactly like `CallFactsTable`). A field access in function `f` references
the module `ClassShape` for its receiver's class but carries its own confidence
(`Proven`/`Guarded`/`Unknown`) because the *receiver-class proof* is local to the def
chain in `f` (the `type_refine.rs` `UserClass` refinement at `:303`/`:1193`). One
authority for layout (the shape), one authority for "is this access safe" (the fact).

---

## 3. Shape DISCOVERY: how the shape is produced and what forces `Unknown`

The shape is **discovered in the module phase** by reading the same frontend
class-layout knowledge that today lives in the `ClassInfo` dict — but the discovery
*output* is the typed `ClassShape`, and the *trust* is the typed `ShapeStability`
lattice, replacing the ad-hoc `_class_layout_stable` boolean (`__init__.py:3291`).

### 3.1 The producer pipeline

```
frontend ClassInfo (classes.py:934)            runtime class_field_offset MRO-walk
  fields / field_order / mro / slots /            (attr.rs:1167) — the OFFSET AUTHORITY
  dynamic / dataclass / custom_metaclass /              |  (validation oracle, §5.4)
  decorated / module / layout_version                   |
          |  surfaced (NOT re-decided) as                v
          v                                       ShapeFactsTable::build_module
   ClassShape { fields: Vec<FieldSlot>, ... }  <-- reconciles frontend offsets with the
          |                                          runtime authority; disagreement =
          |  type_refine UserClass(c) on receiver     hard error (no silent divergence)
          v  (type_refine.rs:303, :1193)
   FieldAccessFact { class, slot, offset, access: Proven|Guarded|Unknown }
```

The frontend already computes the MRO (`ClassInfo["mro"]`, `classes.py:941`), the field
order with `__slots__` consumption (`classes.py:904-929`, the `_SlotsNames` parsing at
`classes.py:48`), the dataclass field indices (`classes.py:929`), and `field_hints`
(the per-field type hints -> `Repr`). Rung 4 **surfaces** this as `ClassShape` rather
than re-deriving it (no second authority for layout) and **validates** the surfaced
offsets against the runtime `class_field_offset` MRO-walk in a debug self-check (§5.4),
so the IR fact and the runtime can never silently disagree (the exact failure mode the
"exact CPython semantics preserved" clause guards against).

### 3.2 The `ShapeStability` lattice — what makes a shape `Frozen` / `Stable` / `Unknown`

This is the **typed replacement** for `_class_layout_stable` (a 4-line frontend boolean
checking `dynamic`/`dataclass`/`mutated_classes` — `__init__.py:3291`). The lattice
encodes *why* and feeds the `FactValue` choice:

```
ShapeStability in {
  Frozen,    // layout cannot change under ANY allowed-subset op -> FieldAccessFact = Proven
  Stable(g), // layout fixed now, mutation is version-detectable -> Guarded(g)
  Unknown,   // layout not statically known / dynamism defeats it -> Unknown (dynamic path)
}
```

| Class shape feature | Modeled as | Forces |
|---|---|---|
| `__slots__`-only class, no `__dict__`, no monkeypatch, no metaclass `__setattr__` | the layout is sealed by CPython itself | **`Frozen`** -> `Proven` (no guard needed) |
| Plain class / dataclass, stable module-class, no observed mutation | layout fixed now, a future `setattr`/`__dict__` write bumps the version | **`Stable(g)`** -> `Guarded(class_version)` |
| `dynamic` (built via dynamic namespace `ns`, `classes.py` `ClassScope.ns`) | layout not statically enumerable | **`Unknown`** |
| `custom_metaclass` / `__prepare__` / metaclass overriding `__setattr__`/`__getattribute__` | shape semantics are metaclass-defined | **`Unknown`** (fail-closed) |
| class in `mutated_classes` (`setattr`/`delattr`/attr-assign on the class name — `_collect_module_class_mutations`, `__init__.py:2512`) | the allowed-subset monkeypatch boundary | **`Unknown`** for offsets (conservative cut; a value-only add could be `Stable(g)` later) |
| a `property` / data descriptor / `__getattr__` / `__getattribute__` override on the attr | the attr access is NOT a plain slot load (CPython routes through the descriptor) | **`Unknown` for that attr** (per-attr, not per-class — §3.3) |
| a non-data descriptor (e.g. a plain method) shadowing the attr | resolved at the class level; instance dict (if any) wins, else the descriptor | **`Unknown` for that attr** unless `Frozen` slots prove no instance-dict shadow |
| `__del__` anywhere in the MRO (finalizer) | not a shape hazard per se, but constructor-fold + stack-promotion is unsound (`classes.py:262-285`) | shape access stays `Guarded`/`Proven`; the *lifetime* treatment is doc 49/50's, consumed not duplicated |
| inheritance / multiple-inheritance MRO | the field layout is the **MRO-merged** slot set (`class_field_offset` walks `class_mro_ref` — `attr.rs:1179`) | `Frozen`/`Stable` per the merged layout; an `Unknown`-shape base or ambiguous MRO -> **`Unknown`** |

**The per-attr granularity is load-bearing for descriptors (the descriptor/property
corner case).** A class can be `Stable` for field `x` but `Unknown` for property `y`
(which routes through `y.__get__`). So `FieldAccessFact` is decided **per access op**
against the `ClassShape` + a descriptor map: if the accessed attr name resolves (through
the MRO) to a data descriptor / property / `__getattribute__` override, the access is
`Unknown` regardless of the class's overall stability — CPython semantics (data
descriptor > instance dict > non-data descriptor > class dict) are preserved by
*declining to specialize* exactly the attrs where a slot load would diverge. This is the
fail-closed-by-construction discipline: an un-shaped access is never miscompiled, only
un-optimized (doc 65 §8 "ShapeFacts greenfield … partiality *sound*").

### 3.3 `__slots__` vs `__dict__` (the explicit modeling requirement)

- **`__slots__`-only (no `__dict__`):** CPython guarantees the instance has *exactly*
  the slot descriptors and *no* `__dict__` — the layout is sealed. `class_own_slot_field_offset`
  (`attr.rs:1235`) already resolves these. -> `Frozen` -> `Proven` (no version guard
  required, because no allowed-subset op can add/remove a slot at runtime).
- **`__dict__`-bearing (the default):** the instance has a `__dict__`; a `setattr` adds
  a key. molt's runtime already models this: `sync_materialized_instance_dict_for_field_offset`
  (`accessors.rs:60`) keeps the offset slot and the materialized `__dict__` coherent. A
  shaped field still has a fixed offset (the runtime stores declared fields at offsets
  AND mirrors them in `__dict__` on demand), so the access is `Stable(g)` -> `Guarded`:
  the version guard catches a `__class__` reassignment or a class-level layout change; a
  per-instance `__dict__` key addition does NOT change the *class* layout (it adds a dict
  entry, not a slot), so the offset for a *declared* field stays valid — and the existing
  `molt_guarded_field_get_ptr` already handles the "field is a declared slot but the
  value was shadowed in the instance dict" case by checking `is_missing_bits` and falling
  back (`accessors.rs:429`). This is why the runtime path is the correct lowering target
  and the IR fact's job is only to *prove the offset and emit the guard*.
- **Mixed (`__slots__` + a base with `__dict__`):** the MRO merge gives slot fields
  fixed offsets and the `__dict__` handles the rest — `Stable(g)` for the slot fields,
  `Unknown` for non-slot attrs.

---

## 4. The DEVIRTUALIZATION it unblocks (producer -> consumer edges)

ShapeFacts is a **producer**; its consumers are the existing passes/backends that today
either re-derive the shape (the frontend) or fall back to the dynamic path.

1. **Attr access -> field load/store (feeds Rung 3 Repr).** The native/LLVM/WASM/Luau
   `LoadAttr`/`StoreAttr` lowering consumes `FieldAccessFact`:
   - `Proven` -> emit a bare `load/store [obj_ptr + offset]` (the `Frozen` slot case);
     no guard, no runtime call. *New* fast path (the existing ops always emit a guard).
   - `Guarded(g)` -> emit the existing `guarded_field_get`/`_set`/`_init` op (native
     `fc/memory.rs:1156`, LLVM `lowering.rs:1612`/`:1900`, Luau `luau.rs:3144`) — but
     now *driven by the proven IR fact*, with the `offset`/`class`/`version` from the
     `ClassShape`, not a frontend dict lookup.
   - The loaded value carries `FieldSlot.repr` -> Rung 3 keeps a `RawI64Safe` field
     unboxed across the access (the `bench_struct` `Point(i, i+1)` field reads stay raw).
2. **Method dispatch -> direct call / IC (feeds Rung 2, doc 47).** `ShapeFacts` produces
   the `CallFacts.target = MethodDescriptor` + `FactValue::Guarded(class_version)` that
   doc 47 §5 Phase 3 / doc 65 §Rung 2 2d names as fed by "Rung 4 ClassVersionGuard." The
   method's MRO resolution (the same `class_mro_ref` walk) gives the direct code pointer;
   the version guard makes it sound. This is the `bench_class_hierarchy` 0.01× heal:
   `BoundMethod` + `Guarded(class_version)` -> no per-call bound-method allocation.
3. **MemorySSA / SROA / LICM (doc 02).** A `Proven` `FieldOffset` load is the
   `ProvenPure` typed-slot load doc 02 §1 needs: the alias oracle can treat
   `[obj_ptr + offset]` as a typed slot (the `op_kinds.toml` `alias_typed_slot_*`
   classification — `:71`), so MemGVN dedups repeated `obj.attr` reads and LICM hoists a
   loop-invariant one. The frontend already carries a `"class"+offset TypedField alias
   region (S5-1.5)" note (`serialization.py:2293`); this rung makes that alias region a
   *consequence of the proven fact* rather than a string passed through.
4. **DictShape -> the etl/csv row-dict path (Phase 4c).** A stable-key row dict
   (`csv_parse_wide`) gets `Guarded(dict-shape)` value-slot flow, reusing the existing
   `guard_dict_shape` op (which shares `molt_guard_layout_ptr` — `lowering.rs:10429`).

---

## 5. SOUNDNESS: exact CPython semantics preserved + the version-guard deopt

The hard requirement: **exact CPython semantics — getattr/setattr/`__getattribute__`/
descriptors/MRO order — and the version guard must deopt on every shape-changing
operation in the allowed subset.** The treatment, all fail-closed:

### 5.1 The attribute-resolution order is preserved by declining to specialize
CPython's attribute lookup is: data descriptor in `type(obj).__mro__` > `obj.__dict__` >
non-data descriptor / class attr > `__getattr__`. ShapeFacts specializes **only** the
case where this order provably reduces to a fixed-offset slot read:
- a data descriptor / property / `__getattribute__` override on the attr (anywhere in
  the MRO) -> `Unknown` for that attr (§3.2/§3.3); the dynamic path runs the descriptor.
- a `__slots__` slot with no shadowing data descriptor -> the slot read IS the CPython
  semantics (`Frozen`/`Proven`).
- a `__dict__` field -> `Guarded`, and the runtime `molt_guarded_field_get_ptr` already
  encodes the "declared field but instance-dict-shadowed" fallback (`accessors.rs:429`)
  AND the guard-fail fallback to `molt_get_attr_ptr` (`accessors.rs:436`), which runs
  full CPython attribute resolution. So a `Guarded` access is *observably identical* to
  the dynamic access on every path — the equivalence the validator checks (§7).

### 5.2 The `ClassVersionGuard` covers every allowed-subset shape mutation
The molt subset forbids exec/eval/compile and unrestricted runtime monkeypatching
(CLAUDE.md), which bounds the shape-mutation set to: a write to a class attribute
(`Cls.x = ...`), `setattr(Cls, ...)`/`delattr(Cls, ...)`, a `__class__` reassignment on
an instance, a `__dict__` replacement, and an allowed-subset class-level monkeypatch.
**Every one of these already bumps the version**: `class_bump_layout_version`
(`layout.rs:1376`) increments the per-class slot-4 version AND the global type version
(`:1382`). The structural obligation Rung 4 adds is the *typed deopt edge*:
- `FactValue::Guarded(g)` where `g = ClassVersionGuard` is the **first-class `Deopt`
  obligation** (doc 46 §4.4 Typed Runtime Interface, the `Deopt` `CallableTarget`
  variant). The guard re-reads the runtime version and compares to the
  `ClassShape.version` the fact was proven against; on mismatch it takes the
  proven-slow dynamic path — **never a wrong fast path** (doc 65 §8 risk row 1).
- The frontend mutation scan (`_collect_module_class_mutations`, `__init__.py:2512`) is
  *promoted* to a TIR-consumed fact: a class observed mutated -> its `ShapeStability` is
  capped at `Stable(g)` (never `Frozen`), so a `Proven` (no-guard) access is *only* ever
  emitted for a `Frozen` shape the version bump cannot reach (the `__slots__`-sealed
  case). This is the structural guarantee that a guardless access can never go stale.

### 5.3 Multiple-inheritance MRO (the explicit corner case)
The field layout is the **MRO-merged** slot set; `class_field_offset` already walks
`class_mro_ref` (`attr.rs:1179`) in C3-linearization order, taking the first class in
the MRO that declares the field's offset. ShapeFacts mirrors this:
- a diamond where two bases declare the same slot name -> the MRO order decides the
  offset (first wins), exactly as `class_field_offset` resolves it; the validator (§5.4)
  cross-checks the IR's chosen offset against the runtime walk.
- an MRO with an `Unknown`-shape base (dynamic, metaclass) anywhere -> the derived class
  is `Unknown` (a base's dynamism poisons the layout).
- C3 linearization failure / ambiguous MRO -> the frontend already errors at class-def
  time (CPython parity); ShapeFacts never sees a class whose MRO is ill-formed.

### 5.4 The validation oracle (no silent divergence)
A debug self-check (gated like `MOLT_VERIFY_ANALYSIS=1`) reconciles, for every
`ClassShape` field, the IR offset against the runtime `class_field_offset` MRO-walk
authority (`attr.rs:1167`). Disagreement is a hard error, not a silent fallback —
because a wrong offset is a memory-safety bug (`field_get_raw` bounds-checks at
`accessors.rs:143`, but a wrong-in-range offset reads the wrong field = a silent
wrong-answer P0). This makes the IR fact and the runtime *provably* one authority.

---

## 6. The `op_kinds.toml` authority for the shape-guard ops (one generated authority)

Per doc 59 / doc 46 §1 (discovery-vs-authority) and doc 65 §4 (one op-semantics
authority): the shape-guard ops must have their semantics in **one generated registry**,
drift uncompilable via `gen_op_kinds.py --check`. The ops already exist in the
vocabulary; this rung completes their registry authority:

- **Kind->OpCode mapping (exists, keep):** `guarded_field_get -> LoadAttr`,
  `guarded_field_set`/`guarded_field_init -> StoreAttr`, `guard_layout`/
  `guard_dict_shape`/`guard_layout_ptr`/`guard_type` classified
  (`op_kinds_generated.rs:59-74`, `:255`, `:499`).
- **Effect oracle (the `[[opcode]]` table, `op_kinds.toml:815+`):** `LoadAttr` /
  `StoreAttr` carry `may_throw` (a guarded access CAN raise on the deopt path —
  `AttributeError` from `molt_get_attr_ptr`), `operand_ownership`, `purity`. A `Proven`
  field load is pure-on-the-fast-path; the registry must express that the *guardless*
  (`Proven`) lowering of a `LoadAttr` is nothrow+pure while the *guarded* one inherits
  the dynamic op's `may_throw`. This is a **new generated column** on the shape-guard
  ops: `shape_guard_role in {none, version_guard, dict_shape_guard}` and a
  `proven_access_nothrow` bit, rendered by `gen_op_kinds.py` and consumed by the
  ExceptionRegion/`no_throw` analysis (doc 45) so a `Proven` access pays zero
  exception-stack churn (ties Rung 2 2c).
- **The classifier sets (`op_kinds.toml`):** the guard ops are in `classifier_inert_marker`
  (`:508`: `guard_layout`/`guard_dict_shape`/`guard_layout_ptr` — they own no surviving
  heap ref). The `guarded_field_get`/`_set`/`_init` ops are in the alias/ownership
  classifier sets (`alias_typed_slot_*`, `:71`) — Rung 4 adds the `TypedField` alias
  region as a *generated* classification so MemorySSA/SROA (doc 02) read it from the
  registry, never a backend hand-list.
- **`--check` gate:** `tools/gen_op_kinds.py --check` (its `def main`; `--check`
  documented at the header `:32`) re-renders in memory and asserts equality with the
  checked-in `op_kinds_generated.rs`/`.py`; `tests/test_gen_op_kinds.py` fails CI on
  drift. Any new column added for shape ops is `--check`-gated the same way.

---

## 7. Phases (dependency order; each independently landable with green gates)

> Build/test discipline every phase (CLAUDE.md): `export MOLT_SESSION_ID=shape-<phase>`
> before any build; route any raw-binary run through `tools/safe_run.py --rss-mb 2048
> --timeout 15`; never `cargo clean`; max 2 build-triggering agents. The new fact family
> lands in a focused module `runtime/molt-tir/src/tir/shape_facts.rs` (respecting the
> 21b crate graph, the doc 65 §5 ratchet rule — it must LOWER the god-file ratchet,
> never raise it; `type_refine.rs` is 3650 ln and `function_compiler.rs` is a god-file,
> so the shape logic does NOT land in either — it is its own module + thin consumers).

### Phase 4a — `ClassShape`/`FieldSlot` representation + module-phase discovery (representation only)
Build `shape_facts.rs`: the `ClassShape`/`FieldSlot`/`FieldAccessFact` types, the
`ShapeStability` lattice, `AnalysisId::ShapeFacts` (the 15th variant), `ShapeFactsTable`
(per-function, keyed by `ValueId`, mirroring `CallFactsTable` at `call_facts.rs:330`),
and `ShapeFactsTable::build_module` (interprocedural, mirroring `call_facts.rs:368`)
that *surfaces* the frontend `ClassInfo` layout (carried through the existing op `"class"`
attr + a new module-side class-layout descriptor) into `ClassShape`, validated against
the runtime `class_field_offset` walk (§5.4). Fields filled for `@dataclass` and simple
classes (no metaclass, no `__slots__` surprises) first.
- **Gate:** `gen_op_kinds.py --check` green (no op changes yet); a new
  `tools/shape_coverage.py --check` ratchet (mirroring `call_fact_coverage.py`, doc 47
  §6) emits the shape-coverage census (% of `UserClass` accesses with a `Proven`/
  `Guarded`/`Unknown` `FieldAccessFact`); `MOLT_VERIFY_ANALYSIS=1` self-check passes;
  the IR-offset-vs-runtime-offset reconciliation (§5.4) green on the bench corpus;
  **byte-identical codegen** (consumed by nothing yet — representation only, an
  explicitly-noted intermediate within this rung's arc, never its terminus —
  CLAUDE.md / doc 65 §8 half-rung risk row).

### Phase 4b — `FieldAccessFact = Proven` direct load/store for `Frozen` monomorphic receivers
Wire the four backends' `LoadAttr`/`StoreAttr` lowering to consume a `Proven`
`FieldAccessFact`: emit a bare `load/store [obj_ptr + offset]` (the *new* guardless
path) for `Frozen` (`__slots__`-sealed) shapes. The frontend STOPS deciding offsets for
these (the `field_offset` heuristic at `serialization.py:481` is retired *for the cases
the fact now covers* — asymmetry forbidden, migrate the `get`/`set`/`init` sites
together, `serialization.py:2283`/`:2080`/`:2124`).
- **Gate:** `bench_attr_access` warm GREEN native+LLVM+WASM+Luau (a dispatch-cycle win,
  not just alloc-count — the bar); the `LoadAttr` becomes LICM-hoistable (ties doc 02
  MemorySSA — a `Proven` `FieldOffset` load is `ProvenPure`); `shape_coverage.py --check`
  `Proven` % UP; differential parity (the equivalence validator: shape-specialized
  output observably identical to the dynamic path) green on all four backends; the
  god-file ratchet (`structural_audit.py --check`) DOWN (frontend shape heuristic
  shrinks, `duplicate_authorities` for "field offset" -> 0).

### Phase 4c — `Guarded(class_version)` + deopt for `Stable` receivers; `DictShape` for stable-key dicts
Wire the `Guarded` case to *drive* the existing `guarded_field_get`/`_set`/`_init` ops
from the proven fact (offset/class/version from `ClassShape`, the guard a typed
`ClassVersionGuard = GuardId`). Add `DictShape` value-slot flow for stable-key row dicts
(the `csv_parse_wide`/`etl_orders` path) via the existing `guard_dict_shape` op. Promote
the frontend `_collect_module_class_mutations` boundary (`__init__.py:2512`) to the
TIR-consumed `ShapeStability` cap (§5.2).
- **Gate:** `bench_etl_orders` 0.60× -> warm GREEN and `bench_csv_parse_wide` 0.68× ->
  warm GREEN (native+WASM+LLVM+Luau); **the deopt-correctness differential** (the
  validator obligation, doc 65 §8 row 1): a test that mutates the class version at
  runtime (`Cls.new_attr = ...` / `setattr`) takes the proven-slow path and produces the
  identical observable result — exercised for *every* `Guarded` fact (the Alive2-style
  checkable obligation); PyPy ratio recorded on the dynamic subset; no CPython-red
  regression on any cell.

### Phase 4d — feed Rung 2's `MethodDescriptor`/`BoundMethod` devirt from `ClassShape`
Produce the `CallFacts.target = MethodDescriptor` + `FactValue::Guarded(class_version)`
for `CallMethod` ops whose receiver is a `Stable`/`Frozen` `UserClass` (the co-scheduled
edge doc 65 §7 names: "2d and 4c co-schedule"). The method MRO resolution reuses the
same `class_mro_ref` walk; the bound-method allocation is elided under the guard.
- **Gate:** `bench_class_hierarchy` warm GREEN and beats CPython (the 0.01× -> >1.0×
  heal) on all four backends; `bench_descriptor_property` stays correct (descriptor
  attrs are `Unknown` -> dynamic, §3.3) and is not regressed; PyPy ratio recorded (the
  PyPy maps/hidden-class parity lever); `call_fact_coverage.py --check`
  `MethodDescriptor` coverage UP; deopt differential for the method-devirt guard green.

---

## 8. Risks and their structural (Proven / Guarded / Unknown) treatments

| Risk | Band-aid (forbidden) | Structural treatment |
|---|---|---|
| **Shape mutation paths** (a class layout changes at runtime in the allowed subset) | special-case the failing program; cap the guard | every allowed-subset mutation bumps `class_bump_layout_version` (`layout.rs:1376`, exists); `FactValue::Guarded(class_version)` re-checks it and deopts to the proven-slow path (`accessors.rs:436`); guardless `Proven` is emitted ONLY for `Frozen` (`__slots__`-sealed) shapes a version bump cannot reach (§5.2). Validator: every `Guarded` fact's deopt edge exercised by a mutation test (Phase 4c gate). |
| **Descriptor / property corner cases** (`__get__`/`__set__`/`__getattribute__`) | specialize anyway and hope | per-ATTR `FactValue` (§3.3): an attr that resolves through a data descriptor / property / `__getattribute__` override is `Unknown` for that attr, regardless of class stability — the dynamic path runs the descriptor (exact CPython data-descriptor > instance-dict > non-data-descriptor order preserved by *declining to specialize*). |
| **Metaclass corner cases** (`__prepare__`, metaclass `__setattr__`) | ignore the metaclass | `custom_metaclass` (`classes.py:954`) -> `ShapeStability::Unknown` (fail-closed); a metaclass overriding attribute semantics never gets a shaped access. |
| **Multiple-inheritance MRO** (diamond, conflicting slot names) | pick a base arbitrarily | the offset is the MRO-merged slot (C3 order, first-declarer wins) mirroring `class_field_offset`'s `class_mro_ref` walk (`attr.rs:1179`); the validator (§5.4) cross-checks the IR offset against the runtime walk; an `Unknown`-shape base anywhere poisons the layout to `Unknown`. |
| **Greenfield is large; risk of a partial shape system** | ship offsets for dataclasses only, leave the rest "for later" | `FactValue` makes partiality SOUND: an un-shaped access is `Unknown` = the current full runtime lookup (fail-closed). Coverage grows monotonically via `shape_coverage.py --check`; no program is ever miscompiled by a missing shape, only un-optimized (doc 65 §8). |
| **IR offset diverges from the runtime offset** (memory-safety P0) | trust the frontend dict | the §5.4 reconciliation against `class_field_offset` (`attr.rs:1167`) is a hard error in the verify build; `field_get_raw` bounds-checks (`accessors.rs:143`) but a wrong-in-range offset is a silent-wrong-answer P0 — the validator catches it before codegen. |
| **A native opt fails to cross to WASM/LLVM/Luau** | a backend-specific scoreboard exception | all four backends ALREADY lower the guarded ops (§0.1); Rung 7's backend matrix (doc 65) gates the `Proven` guardless path on all four — a native win with a WASM red is a portable-IR fact gap (doc 46 §4.7), gated not excepted. |
| **A rung lands the fact but discards it** (the half-rung trap) | "representation only, consumer later" as a permanent state | a rung is *defined* as fact + consumer + validator + green row (§1). Phase 4a (representation-only) is an explicitly-noted intermediate within this rung's arc, never its terminus (CLAUDE.md "structural change as the unit of work"). |

---

## 9. Composition with the doctrine + the decomposition (21a-e) + cross-arc deps

**Doctrine (DESIGN_DOCTRINE.md) checklist:**
1. *Reduces concern-mixing god-files?* YES — the shape logic leaves the frontend
   god-files (`__init__.py`, `serialization.py`) and `type_refine.rs`, landing in a
   focused `shape_facts.rs`; `duplicate_authorities` for "field offset" -> 0. Retires the
   **dual-maintenance + ownership-collision** killer for shape decisions.
2. *Preserves exact CPython semantics AND feels Pythonic?* YES — §5: getattr/setattr/
   descriptor/MRO order preserved by specializing only the provably-equivalent slot-load
   case; the deopt fallback runs full CPython resolution. The Pythonista writes `obj.attr`.
3. *Fixes a REPRESENTATION, not a symptom?* YES — the missing IR fact (`ShapeFacts`),
   not a peephole; it makes shape-blind access unexpressible when the shape is `Proven`.
4. *One generated/checkable authority?* YES — `op_kinds.toml` + `gen_op_kinds.py --check`
   for the op semantics (§6); `shape_coverage.py --check` for coverage; the §5.4
   reconciliation for offset authority.
5. *Memory safety structural?* YES — `FieldSlot.ownership` is doc 49's lattice (consumed
   not re-derived); the offset-reconciliation validator makes a wrong offset uncompilable.
6. *Win measured across the full matrix?* YES — §7 gates are warm-AND-cold ×
   native/WASM/LLVM/Luau vs CPython/PyPy/Codon (doc 64).

**Decomposition (doc 65 §5):** 21c (frontend mixin decomposition, `visitors/classes.py`)
is where the shape PRODUCTION detaches cleanly — Rung 4 *removes* shape-decision logic
from the frontend, helping 21c. 21b: `shape_facts.rs` is a focused molt-tir module
respecting the crate graph. The new code must LOWER the `structural_audit.py --check`
ratchet (doc 65 §5 binding).

**Cross-arc dependencies:**
- **Depends on:** Rung 2 / doc 47 (the `Guarded`/`MethodDescriptor` `CallFacts`
  machinery — `call_facts.rs`; Phase 1a landed); the landed `TirType::UserClass`
  (`types.rs:46`); the landed `FactValue`/`GuardId` (`call_facts.rs:117`/`:102`); the
  `AnalysisManager` (`analysis/mod.rs`).
- **Feeds:** Rung 3 (the loaded `FieldSlot.repr` keeps fields unboxed —
  `repr::Repr`); Rung 2 2d (the `ClassVersionGuard`-fed method devirt);
  doc 02 (a `Proven` `FieldOffset` load is the `ProvenPure` typed-slot load LICM/MemGVN
  need); doc 49/50 (consumes `FieldSlot.ownership`, no second authority).
- **Uses:** doc 59 (the fact-authority discipline + `gen_op_kinds --check`); doc 64 (the
  perf scoreboard gate + `perf_causality.py` `MissingFact::FieldOffset`/`ClassVersionGuard`
  which this rung makes resolvable).
- **Independent of:** Rung 1 (ownership/RC) except where `FieldSlot.ownership` consumes
  doc 49; Rung 5/6/7/8.

## 10. Definition of done (this rung)
ShapeFacts is complete when: `shape_facts.rs` exists as an `AnalysisId::ShapeFacts`
cached analysis with `build_module` seeding; the frontend no longer decides field
offsets (the `field_offset` heuristic + the per-site `guarded_field_*` emission migrate
to fact-driven lowering, all sites together); `bench_attr_access`, `bench_etl_orders`,
`bench_csv_parse_wide`, `bench_class_hierarchy` are warm-AND-cold GREEN across
native/WASM/LLVM/Luau with a measured dispatch/alloc-CYCLE win vs CPython (PyPy/Codon
ratios recorded); the equivalence validator proves every `Proven`/`Guarded` access is
observably identical to the dynamic path including the deopt; `gen_op_kinds.py --check`
and `shape_coverage.py --check` are green and ratcheting the right way; and the
god-file/`duplicate_authorities` ratchet went down. At that point "boxed dict-based
attribute access" is an unexpressible-as-slow class whenever the shape is `Proven`.
