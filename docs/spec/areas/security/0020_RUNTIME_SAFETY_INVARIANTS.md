# Runtime Safety Invariants
**Spec ID:** 0020
**Status:** Draft (implementation-targeting)
**Owner:** runtime + backend
**Goal:** Document critical safety invariants for Molt's runtime and the tooling
entrypoints used to validate them.

---

## 1. Object Representation Invariants
- All values are NaN-boxed `u64` (`MoltObject`).
- Heap pointers must fit in 48 bits and are stored directly with the pointer tag.
- `MoltObject::from_ptr` registers canonical pointer addresses in the pointer
  registry; `MoltObject::as_ptr` resolves through the registry (no int->ptr).
- Pointer unboxing in the backend sign-extends bit 47; non-canonical pointers are
  rejected in debug builds.
- `molt_handle_resolve` resolves via the pointer registry (not a raw unbox).
- `molt_alloc` returns boxed object bits; raw pointers are only used internally
  for field access and must be registered when exposed as bits.

## 2. Header/Layout Invariants
- `MoltHeader` is prepended to every heap object.
- `type_id` is immutable after allocation.
- `state` stores class bits for `TYPE_ID_OBJECT` instances.
- `poll_fn != 0` marks async objects; attribute mutation is forbidden for these.

## 3. Reference Counting
- Any store into heap structures must `inc_ref` the new value and `dec_ref` the
  old value.
- Objects in containers or class dicts must always be stored as boxed
  `MoltObject` bits.

## 4. Dict and Sequence Invariants
- Dict order vector stores key/value pairs in insertion order.
- Dict hash table indexes into the order vector; empty slots are `0`.
- List/tuple backing vectors are never reallocated without updating length.

## 5. Class and Descriptor Invariants
- Class MRO must be computed before attribute resolution.
- Data descriptor precedence applies for `__get__`, `__set__`, `__delete__`.
- `descriptor_is_data` must accept boxed pointers via `maybe_ptr_from_bits`.

## 6. Async/Generator Invariants
- `state` stores either a logical state id (non-negative) or an encoded resume
  target (negative) for pending awaits; encoded values use bitwise NOT of the
  resume op index.
- `state` is only advanced by poll loops; pending encodings must be decoded
  before dispatch.
- Return slots for async/generators are stored in closures and loaded after
  state labels.

## 7. Unsafe Boundaries
- All `unsafe` blocks must validate `object_type_id` before casting.
- Pointer arithmetic must remain within the object payload.
- Raw pointers must be derived from boxed bits and must not be stored in
  collections or globals without boxing.

### 7.1 Strict-Provenance Hardening Checklist
- ✅ Pointer-typed host ABI (`molt-ptr`) is treated as raw addresses (no fallback
  boxing or int->ptr casts in the runtime).
- ✅ WASM harness enforces raw-pointer inputs for `molt-ptr` imports.
- ✅ Pointer registry sharded to reduce lock contention (OPT-0003 phase 1).
- TODO(runtime-provenance, owner:runtime, milestone:RT2, priority:P2, status:planned): audit remaining pointer
  registries/handles for explicit release on shutdown and on error paths.
- TODO(runtime-provenance, owner:runtime, milestone:RT1, priority:P2, status:partial): benchmark sharded registry
  and evaluate lock-free alternatives once correctness is locked in
  (see OPT-0003 in `OPTIMIZATIONS_PLAN.md`).

## 8. Validation Entry Points
Use `tools/runtime_safety.py` for standardized checks:
- `python tools/runtime_safety.py asan`
- `python tools/runtime_safety.py tsan`
- `python tools/runtime_safety.py ubsan`
- `python tools/runtime_safety.py miri`
- `python tools/runtime_safety.py fuzz --target string_ops --runs 10000`
- `python tools/runtime_safety.py clippy`

Notes:
- The miri entrypoint sets `MIRIFLAGS=-Zmiri-disable-isolation` by default so
  runtime tests can access time/filesystem APIs. Override as needed.
- `--log-dir` defaults to `logs/` and can be disabled by passing `--log-dir=`.

## 9. Tooling Prerequisites
- `cargo +nightly miri setup` must be run once per toolchain install.
- `cargo install cargo-fuzz` is required for `tools/runtime_safety.py fuzz`.
