# 49 — Object inline-field ownership invariant (the #86 rule, binding)

Status: BINDING (2026-06-09). Written down so no second field-release authority is
ever created (that would reintroduce the #86 leak or a double-free/UAF). Landed by
`ac73ab954` (#86). Part of the Finalizer Lifetime Closure macro-tranche.

## The invariant
An object instance's **inline typed attribute field** slots hold **owning** pointer
references (`object_field_set_ptr_raw` / `object_field_init_ptr_raw` `inc_ref` the
value on store and `dec_ref` the displaced old value). Exactly one authority
releases each such reference, chosen by the object's allocation mode:

- **Heap objects** (reach `dec_ref_ptr` rc→0): the **runtime free path** is the SOLE
  owner. `dec_ref_ptr`'s `TYPE_ID_OBJECT` arm calls
  `dec_ref_object_inline_fields` (gated on `HEADER_FLAG_HAS_PTRS`), which walks the
  class `__molt_field_offsets__` and `dec_ref`s each pointer slot exactly once
  (deduped across MRO, bounds-checked, slot cleared before dec). `TYPE_ID_DATACLASS`
  releases its `dataclass_fields` Vec the same way (one authority per representation).
- **Stack-promoted / folded / immortal objects** (NEVER reach `dec_ref_ptr` rc→0):
  the **compiler/lowering** owns release — the constructor fold tracks `self.attr`
  as SSA values and the drop pass releases them at scope exit, or proves no release
  is needed (immortal). These objects' field slots are not released by the runtime
  because the runtime free path never runs for them.

## The two rules (a verifier target — #TV-x)
1. **No object is covered by BOTH authorities** (→ double-free / UAF). The boundary is
   "does the object reach the runtime free path?" Folded objects must be stack-promoted
   or immortal (never heap-freed); if a folded object can be heap-freed, the compiler
   MUST NOT also emit field DecRefs for it (the runtime will).
2. **No object is covered by NEITHER authority** (→ the #86 leak). Every heap object with
   pointer fields has `HEADER_FLAG_HAS_PTRS` set (the store path sets it) and is released
   by the runtime. Every non-heap object with pointer fields is released by the compiler.

## Why the gate is `HEADER_FLAG_HAS_PTRS`
The store path sets `HAS_PTRS` iff a pointer field was ever stored
(`object_mark_has_ptrs`). So primitive-only objects (int/float fields) carry `HAS_PTRS`
clear → the runtime field walk is skipped → the hot path is byte-identical (zero cost).
This is the representation fact that makes the single-authority release free on the hot
path: ownership is read off a flag the store already maintains, not recomputed.

## Safety facts the release relies on (must stay true)
- inline fields are **NaN-boxed** (`object_field_set_ptr_raw` stores `val_bits`) → a
  primitive field `dec_ref`s to a no-op; do NOT store raw-tagged (un-NaN-boxed) values in
  a field slot the runtime walk will visit.
- the payload is **zero-initialised** at alloc (`alloc_object` `write_bytes(...,0,...)`) →
  an unset field reads `0` (no-op).
- the slot **owns** its ref (inc_ref on store) → the runtime `dec_ref` is balanced.
- the released slot is **cleared to 0** before `dec_ref` → a resurrecting `__del__`
  re-entry cannot double-dec the same field.

## Relationship to the finalizer lifecycle (the macro-layer)
This invariant is one rung of the lifecycle vertical:
placement (instance reaches rc→0) → execution (`__del__` runs once, exceptions
swallowed — doc 48 / #65) → **field release (this doc / #86)** → child finalization →
leak gauge → ordering (#58). Layer-1 placement (#87, #63) is UPSTREAM: if an instance
never reaches the free path, this release never runs (and cannot leak or double-free —
it simply does not execute). Do not add a placement-era field release to "compensate";
fix placement so the instance reaches the one runtime authority here.
