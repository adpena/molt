# Runtime Safety Invariants
**Spec ID:** 0020
**Status:** Draft (implementation-targeting)
**Owner:** runtime + backend
**Goal:** Document critical safety invariants for Molt's runtime and the tooling
entrypoints used to validate them.

---

## 1. Object Representation Invariants
- All values are NaN-boxed `u64` (`MoltObject`).
- Heap handles must fit in 48 bits and are stored with the pointer tag.
- Handle table resolution must validate generation before returning a pointer.
- Raw pointers returned by `molt_alloc` are registered in `RAW_OBJECTS` and must
  only be interpreted via `maybe_ptr_from_bits`.

## 2. Header/Layout Invariants
- `MoltHeader` is prepended to every heap object.
- `type_id` is immutable after allocation.
- `state` stores class bits for `TYPE_ID_OBJECT` instances.
- `poll_fn != 0` marks async objects; attribute mutation is forbidden for these.

## 3. Reference Counting
- Any store into heap structures must `inc_ref` the new value and `dec_ref` the
  old value.
- Objects in containers or class dicts must always be stored as MoltObject bits
  (boxed or raw pointer with `RAW_OBJECTS` registration).

## 4. Dict and Sequence Invariants
- Dict order vector stores key/value pairs in insertion order.
- Dict hash table indexes into the order vector; empty slots are `0`.
- List/tuple backing vectors are never reallocated without updating length.

## 5. Class and Descriptor Invariants
- Class MRO must be computed before attribute resolution.
- Data descriptor precedence applies for `__get__`, `__set__`, `__delete__`.
- `descriptor_is_data` must accept raw pointers via `maybe_ptr_from_bits`.

## 6. Async/Generator Invariants
- `state` values are monotonic and only advanced by poll loops.
- Return slots for async/generators are stored in closures and loaded after
  state labels.

## 7. Unsafe Boundaries
- All `unsafe` blocks must validate `object_type_id` before casting.
- Pointer arithmetic must remain within the object payload.
- Any raw pointer stored in collections must remain registered until freed.

## 8. Validation Entry Points
Use `tools/runtime_safety.py` for standardized checks:
- `python tools/runtime_safety.py asan`
- `python tools/runtime_safety.py tsan`
- `python tools/runtime_safety.py ubsan`
- `python tools/runtime_safety.py miri`
- `python tools/runtime_safety.py fuzz --target string_ops`

Notes:
- The miri entrypoint sets `MIRIFLAGS=-Zmiri-disable-isolation` by default so
  runtime tests can access time/filesystem APIs. Override as needed.
