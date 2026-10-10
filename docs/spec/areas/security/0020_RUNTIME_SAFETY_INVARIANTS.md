# Runtime Safety Invariants
**Spec ID:** 0020
**Status:** Draft (implementation-targeting)
**Owner:** runtime + backend
**Goal:** Document critical safety invariants for Molt's runtime and the tooling
entrypoints used to validate them.

---

## 1. Object Representation Invariants
- All values are NaN-boxed `u64` (`MoltObject`).
- Object addresses must fit the unsigned 48-bit pointer payload; boxing rejects
  wider addresses in every profile instead of truncating them.
- Runtime and generated native pointer decoding preserve bit 47 and zero-extend
  the masked address. Decoding must also fit the target pointer width.
- Debug `MoltObject::from_ptr` registers exposed addresses for provenance checking.
  `as_ptr` preserves registered provenance when available; compiled-minted boxes
  also recover through exposed provenance. A registry miss must not change
  address identity or make debug and release decoding disagree.
- `molt_handle_resolve` consumes this object-address contract. Opaque Rust handles
  use the separate generational ID protocol below.
- `molt_alloc` returns boxed object bits; raw field-access pointers obey the same
  representability and lifetime requirements when exposed as object bits.
- Rust-owned opaque handles must not use pointer-tagged object bits. Non-Molt
  Rust allocations are exposed with `opaque_handle_bits`, which registers the
  pointer in a sharded generational slab behind a bounded synthetic
  immediate-int ID; host virtual addresses are never truncated into the
  carrier, and a released stale generation cannot resolve after slot reuse.
  Only the owning intrinsic may resolve/release that ID.

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
- `RUST_NIGHTLY=$(tr -d '\r\n' < config/rust_nightly_toolchain.txt); cargo "+$RUST_NIGHTLY" miri setup` must be run once per toolchain install.
- `cargo install cargo-fuzz` is required for `tools/runtime_safety.py fuzz`.
