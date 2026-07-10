# Miri UB / strict-provenance audit — `molt-lang-cpython-abi`

Lane: **MIRI-STRICT-PROVENANCE**. A real undefined-behaviour audit of the CPython
binary-ABI shim under [Miri](https://github.com/rust-lang/miri), targeting the
class we had been finding by hand (misaligned derefs, PyArg memory corruption,
self-deadlocks, refcount UAF, and the bridge's pointer↔integer handle-encoding
puns). Chris-Lattner standard: every finding is either FIXED at the root or
precisely characterised — nothing is suppressed to make a run green.

Host: `nightly-x86_64-pc-windows-msvc` (rustc 1.96.0-nightly, miri 0.1.0
2026-03-10). Interpreter, not codegen — see *Miri limitations* for what that
excludes.

---

## Re-run commands (repeatable gate)

Isolated target dir off `C:\Molt`, `--ignore-rust-version` because the pinned
nightly (1.96.0) trails the workspace MSRV (1.96.1):

```sh
cd <worktree>/runtime
export CARGO_TARGET_DIR=C:/Molt/miri-target CARGO_INCREMENTAL=0

# 1. Build the sysroot once (slow, cached thereafter):
cargo +nightly miri setup

# 2. Core UB audit — library unit tests, default (permissive/exposed) provenance.
#    GREEN = 0 UB. A one-time informational note per `with_exposed_provenance`
#    site is expected (see "Provenance model" below), not a failure.
cargo +nightly miri test -p molt-lang-cpython-abi --lib --ignore-rust-version

# 3. Second aliasing model (Tree Borrows) + alignment model — both must stay green:
MIRIFLAGS="-Zmiri-tree-borrows"             cargo +nightly miri test -p molt-lang-cpython-abi --lib --ignore-rust-version
MIRIFLAGS="-Zmiri-symbolic-alignment-check" cargo +nightly miri test -p molt-lang-cpython-abi --lib --ignore-rust-version

# 4. Whole-crate audit (all integration binaries). Leaks OFF because the
#    process-global immortal type/cache state is intentionally never freed
#    (see "Miri limitations"). --no-fail-fast so one blocked binary does not
#    hide the rest.
MIRIFLAGS="-Zmiri-ignore-leaks" \
  cargo +nightly miri test -p molt-lang-cpython-abi --ignore-rust-version --no-fail-fast
```

Known non-green binaries in step 4 are enumerated under *Coverage* and are all
either FFI-into-C (out of Miri's reach) or documented model/limitation
exceptions — **not** open UB.

---

## Provenance model — why NOT `-Zmiri-strict-provenance`

`-Zmiri-strict-provenance` makes **every** integer→pointer cast an *unsupported
operation* — it forbids `ptr::with_exposed_provenance` as well. The bridge's
identity maps store genuine C-extension `PyObject*` pointers **as integers**
(`raw_py: HashMap<AbiHandle, usize>`, the `foreign` map, and the NaN-box fast
path where `*mut PyObject` *is* the `MoltObject` u64) and reconstruct pointers
from them when the object crosses back to C. That is the textbook use case for
**exposed** provenance, which is fundamentally *inexpressible* under strict
provenance (there is no in-scope Rust pointer whose provenance could be
`.with_addr()`-ed — the address originated in C).

Consequently strict mode aborts on the first legitimate reconstruction
(`bridge.rs`, `with_exposed_provenance_mut`) and yields ~zero coverage. This is
a **documented mode incompatibility**, not a defect: the correct model for this
crate is the *exposed* provenance model, and the code is now provenance-correct
under it. The audit therefore runs under Miri's **default** provenance
(permissive/exposed) + Tree Borrows + symbolic alignment, all of which do detect
real aliasing/OOB/UAF/uninit UB.

The audit still made the puns provenance-*correct* (rather than bare `as`
casts):

* Addresses used only as map **keys** (identity lookup, never reconstructed) →
  `<*mut T>::addr()` (strict; no needless exposure): `from_py` / `foreign` keys.
* Addresses that **are** reconstructed into a pointer → exposed at the store
  site with `<*mut T>::expose_provenance()` and rebuilt with
  `core::ptr::with_exposed_provenance_mut::<T>()`: `raw_py` values, the
  foreign-custody `c_ptr` parameters, and `pyobject_to_bits` / `bits_to_pyobject`.

Under default provenance Miri prints one informational note per
`with_exposed_provenance` site ("Miri might miss pointer bugs"). That is inherent
to the exposed model and is **expected**, not a finding. All int→ptr sites are
now the intentional, documented handle-encoding reconstructions — there are no
stray/implicit ones.

---

## Findings and dispositions

### A. REAL UB — FIXED at the root (`src/bridge.rs`, this lane)

1. **Write through a shared-reference-derived pointer.**
   `handle_to_pyobj` / `handle_to_borrowed_pyobj` built the returned `*mut
   PyObject` as `&entry.header.py_obj as *const PyObject as *mut PyObject` and
   then wrote `ob_refcnt` through it. A pointer derived from a shared `&` carries
   only read permission (`&` lowers to LLVM `noalias readonly`); writing through
   it is UB. Miri (Stacked Borrows) flagged the write at `bridge.rs:297`.

2. **Aliasing raw pointers invalidated by re-borrow.** The bridge hands out
   multiple `*mut PyObject` to the *same* header (that is what CPython refcounting
   is: C holds a pointer, the bridge caches the same address) and mutates
   `ob_refcnt` through them. Deriving a fresh `&`/`&mut` to that field on each
   call pops previously-handed-out pointers off the borrow stack, so a later
   access through an earlier pointer is UB — a genuine miscompilation hazard (LLVM
   may cache/reorder around the reborrow). Miri flagged it at
   `tests/test_bridge.rs:136` (read `*py1` after a second `handle_to_pyobj` for
   the same handle).

   **Root-cause fix (both):** the C-aliased header field is now
   `py_obj: UnsafeCell<PyObject>` (interior mutability — exactly the right model
   for memory mutated through aliasing raw pointers). All three derivation sites
   take the pointer via `UnsafeCell::get()` from a *shared* borrow, so writes are
   legal *and* siblings coexist without invalidation. `UnsafeCell` is
   `#[repr(transparent)]`, so the `#[repr(C)]` ABI layout and the trailing
   `molt_bits` read (`read_bridge_header_bits`) are byte-identical. Verified green
   under Stacked Borrows, Tree Borrows, and symbolic alignment.

### B. REAL UB — FIXED in tests (`tests/test_sequences.rs`, reference-stealing contract)

3. **Double-free of a stolen reference (use-after-free).** `PyList_SetItem` /
   `PyTuple_SetItem` *steal* the item reference on **every** path including the
   error paths (they `Py_XDECREF` the item before returning -1, matching
   CPython's `listobject.c`/`tupleobject.c`). Four tests decref the item *again*
   after a failed `SetItem` — a double-free that only Miri catches (the mock
   runtime's `dec_ref` on a stale handle is a no-op under normal `cargo test`, so
   the bug was latent). Miri flagged the UAF at `refcount.rs:44`.

   **Fix:** removed the erroneous post-`SetItem` decrefs in
   `test_list_setitem_negative_index_returns_error`,
   `test_list_setitem_null_list_returns_error`,
   `test_tuple_setitem_null_tuple_returns_error`,
   `test_tuple_setitem_negative_index_returns_error`. The production code is
   correct; the tests encoded a wrong ownership assumption. `assert_eq!(result,
   -1)` (the actual behaviour under test) is untouched — this aligns the tests
   with the steal contract, it does not weaken them.

### C. REAL UB (model-dependent) — DOCUMENTED + flagged to the buffer/deadlock-sweep lane

4. **`Py_buffer.format` read after memoryview descriptor copy.** Reading
   `*view.format` after `PyMemoryView_FromBuffer` + `PyBuffer_Release` trips a
   Stacked-Borrows "tag does not exist in the borrow stack" error at offset
   `0x440`. Repro: `tests/test_object_protocol.rs:148`
   (`test_memoryview_from_buffer_copies_descriptor_without_sharing_release`) and
   `tests/test_modules.rs:541`
   (`test_fillinfo_uses_typed_descriptor_without_runtime_release`). The invalidated
   tag is a `SharedReadWrite` retag created inside the buffer/memoryview copy path
   (`src/api/buffer.rs` `PyBuffer_FillInfo` / `src/api/memory.rs`
   `PyMemoryView_FromBuffer`).

   **Model note:** this is flagged under **Stacked Borrows only** — it is **clean
   under Tree Borrows** (the newer, more permissive model Rust is converging on).
   So it is a model-dependent borrow-stack strictness, not a definite
   miscompilation under the semantics Rust is standardising on.

   **Disposition:** NOT fixed here — `buffer.rs`/`memory.rs` are the concurrently-
   active deadlock-sweep lane's files; a fix there belongs with that lane's
   descriptor-lifecycle context and would otherwise collide. Handed off with the
   exact repro above (and a background task) rather than stomped or ignored.

### D. Miri limitations — out of scope (documented, not molt bugs)

* **Cannot call into the compiled C shim (FFI).** Miri interprets MIR; it cannot
  execute the `cc`-compiled `shims/pyarg_variadic.c` / `molt_capi_errno`. Binaries
  that call these abort with *"can't call foreign function"*:
  `test_pyarg_parse` (`PyArg_ParseTuple`), `test_exceptions` (`molt_capi_errno`).
  These paths are covered by normal `cargo test`.
* **Windows isolation blocks filesystem reads.** `test_getset_member_descriptors`
  opens the on-disk C header to assert layout authority; Miri isolation refuses
  `CreateFileW`. Add `-Zmiri-disable-isolation` to run it (or accept it as
  covered by normal `cargo test`).
* **Leak checker flags immortal globals.** `PyType_Ready` interns MRO tuples /
  `tp_dict` into process-lifetime static type objects (CPython static-type
  semantics), and `GLOBAL_BRIDGE` is a process-global cache; neither is dropped at
  exit, so Miri's leak checker reports them. These are *not* leaks — run with
  `-Zmiri-ignore-leaks` for the UB signal. (A genuine per-test-teardown leak audit
  is a separate future effort.)
* **Function-pointer identity is unstable.** `test_type_ready_inheritance`'s four
  `tp_free`/`tp_alloc` assertions compare `fn as usize` addresses
  (`tp_free == PyObject_Free` etc.). Miri assigns synthetic fn-pointer addresses
  that do not compare equal across casts; these pass under real `cargo test`.

---

## Coverage

33 test binaries. **27 ran fully green under Miri (0 UB), ≈363 pure-Rust tests**,
after the fixes in A/B. Models exercised: default (permissive/exposed) provenance,
Tree Borrows, and symbolic alignment — the library unit suite (46 tests) is green
under all three.

| Binary | Status |
|---|---|
| lib (unit, 46) | clean (green under SB + Tree Borrows + symbolic-alignment) |
| test_bridge (26) | clean — **was UB, fixed (A1/A2)** |
| test_sequences (30) | clean — **was UAF, fixed (B3)** |
| test_numbers (46), test_strings (33), test_mapping (21), test_type_operations (17), test_refcount (15), test_hooks (14), test_typeobj_semantics (14), test_long_conversions (13), test_stringification (10), test_abstract_protocols (7), test_slice_unpack (7), test_item_access (3), test_list_setitem (3), test_slice (3), test_sys (3), test_cfunction_bridge_registration (3), test_contextvars (2), test_dict_cursor (2), test_fromspec_slots (2), test_capsule (1), test_truthiness (1), frontier_repro (1), test_f4_small_files (40) | clean |
| test_modules (24/25), test_object_protocol (34/35) | 1 test each = finding **C4** (SB-only, Tree-Borrows-clean) |
| test_type_ready_inheritance (11/15) | 4 = fn-ptr-identity Miri limitation (D) |
| test_getset_member_descriptors (7/8) | 1 = Windows isolation file read (D) |
| test_exceptions, test_pyarg_parse | FFI-into-C shim — out of Miri's reach (D) |

Honest bottom line: on Windows Miri the **pure-Rust surface is broadly reachable**
(unlike some std/OS-heavy crates) — only 2 of 33 binaries are fully FFI-blocked.
The bridge's handle-encoding puns were the strict-provenance signal, and they are
now provenance-correct (exposed model). The two real UB classes in the bridge are
fixed at the root; the reference-stealing double-frees are fixed in-test; the one
remaining (buffer `format`) is a Tree-Borrows-clean, model-dependent finding
handed to its owning lane.
