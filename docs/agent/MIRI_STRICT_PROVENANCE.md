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

### C. REAL UB (model-dependent) — FIXED at the root (`src/api/buffer.rs`, lane MIRI-BUFFER-UB)

4. **`Py_buffer.format`/`shape`/`strides` read after buffer/memoryview install.**
   Reading `*view.format` after `PyMemoryView_FromBuffer`/`PyBuffer_FillInfo`
   tripped a Stacked-Borrows "tag does not exist in the borrow stack" error at
   offset `0x440` (the `format` field: `MoltBufferView` = data@0 … shape@64
   [512B] … strides@576 [512B] … **format@1088 = 0x440**). Repro:
   `tests/test_object_protocol.rs:148`
   (`test_memoryview_from_buffer_copies_descriptor_without_sharing_release`) and
   `tests/test_modules.rs:541`
   (`test_fillinfo_uses_typed_descriptor_without_runtime_release`).

   **Root cause (precise).** `install_buffer_internal` built the C-visible
   interior pointers via `apply_molt_view(view, obj, &mut internal.descriptor,
   …)`, where `<[T; N]>::as_mut_ptr()` takes `&mut self` — a `SharedReadWrite`
   retag at `[0x440..0x450]` derived from that `&mut`. It then called
   `Box::into_raw(internal)`, whose internal `&mut *box` reborrow is a **Unique
   retag over the whole allocation `[0x0..0x458]`** (`BufferInternal` = 8-byte
   `release_kind` + 1104-byte `MoltBufferView` = 1112 = 0x458) that pops the
   reference-derived interior tags off the borrow stack. The pointers survive in
   the `Py_buffer` and are read by C long afterward → read through a popped tag.
   The fragile fields were exactly the three that store *pointers into* the boxed
   descriptor: `format`, `shape`, `strides` (`buf`/`len`/`itemsize`/… are copied
   by value and were never fragile).

   **Root-cause fix (all three fields, one model — raw projection).**
   `install_buffer_internal` now `Box::into_raw`s the descriptor **first**, then
   derives `format`/`shape`/`strides` from the raw `internal_ptr` by raw
   projection (`(&raw mut (*descriptor).format).cast::<c_char>()`, likewise
   `shape`/`strides`) — never through a `&`/`&mut` array reborrow. Raw→raw
   projection creates no reference retag, so no later Unique retag of the box
   (`into_raw`, or `Box::from_raw` at `PyBuffer_Release`) can pop them; the
   pointers stay live for the whole view lifetime. This is the buffer analogue of
   the bridge header's `UnsafeCell` fix (A): aliasing raw pointers into memory C
   also holds, kept valid by never routing through a reference reborrow. A
   `'static` format table was considered and rejected — `shape`/`strides` are
   inherently per-buffer, and `format` is copied per-buffer from an arbitrary
   exporter (`descriptor_from_pybuffer`), so raw projection fixes all three
   uniformly at the source with no interning machinery.

   **Miri re-run (post-fix), lane MIRI-BUFFER-UB.** Both repros PASS under
   **Stacked Borrows AND Tree Borrows** (`-Zmiri-ignore-leaks`). Whole-binary
   under both models: `test_object_protocol` 36/36, `test_modules` 25/25, `lib`
   unit 54/54 — 0 UB. (Fixing C4 un-masked a second, unrelated finding E, below,
   in the same binary — Miri aborts the process on the *first* UB, so C4 had been
   hiding it.)

### E. REAL UB (model-dependent) — FIXED in test (`tests/test_modules.rs`, surfaced by the C fix)

5. **`PyModuleDef_Init` return pointer invalidated by a later `&mut def`.**
   `test_moduledef_init_returns_definition_pointer` calls
   `PyModuleDef_Init(&mut def)` (returns `(PyObject*)def`, i.e. `out` aliases the
   local `def`), then asserts `out.cast() == &mut def as *mut PyModuleDef` and
   reads `(*out).ob_refcnt`. Forming `&mut def` for the comparison is a Unique
   retag over `def` that pops `out`'s tag, so the following read through `out` is
   UB (`alloc[0x0]`, `test_modules.rs:762`). **Production `PyModuleDef_Init` is
   correct** (all raw-pointer: `(*def).m_base… = …; def.cast()`) — the bug is the
   test's aliasing `&mut`, the same class as finding B. Pre-existing on
   origin/main; it had been *masked* by C4 aborting first. SB-only, TB-clean.

   **Fix:** take the raw `def` pointer once (`let def_ptr: *mut PyModuleDef =
   &raw mut def;`), pass it to `PyModuleDef_Init`, and compare/read through
   `def_ptr`/`out` — no fresh `&mut def` after `out` exists. Same assertions, no
   weakening; `test_modules` is now 25/25 under SB + TB.

   **Flaky cross-binary SIGSEGV — verdict.** The intermittent full-suite SIGSEGV
   in the buffer tests is **the same root cause as C4**, not a distinct race: the
   invalidated `format`/`shape`/`strides` tag is a compile-time provenance
   hazard, so under `-O` LLVM was free to treat the descriptor store as dead /
   reorder it around the `Box::into_raw` reborrow, leaving `view.format` pointing
   into a reused/clobbered slot on real hardware (only *sometimes*, hence flaky).
   The raw-projection fix removes the reborrow that licensed that reordering, so
   the C-held pointers now provably alias the live descriptor. No separate
   allocation/race was found; the fix that closes C4 closes the flaky SIGSEGV.

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
| test_modules (25), test_object_protocol (36) | clean — **was UB, fixed (C4 buffer + E5 moduledef-in-test); green under SB + Tree Borrows** |
| test_type_ready_inheritance (11/15) | 4 = fn-ptr-identity Miri limitation (D) |
| test_getset_member_descriptors (7/8) | 1 = Windows isolation file read (D) |
| test_exceptions, test_pyarg_parse | FFI-into-C shim — out of Miri's reach (D) |

Honest bottom line: on Windows Miri the **pure-Rust surface is broadly reachable**
(unlike some std/OS-heavy crates) — only 2 of 33 binaries are fully FFI-blocked.
The bridge's handle-encoding puns were the strict-provenance signal, and they are
now provenance-correct (exposed model). The two real UB classes in the bridge are
fixed at the root; the reference-stealing double-frees are fixed in-test; the
buffer `format`/`shape`/`strides` descriptor pointers (C4) are fixed at the root
via raw projection off the heap-published box (lane MIRI-BUFFER-UB), which also
closes the flaky cross-binary buffer SIGSEGV (same provenance hazard); and the
`PyModuleDef_Init`-return finding (E5) it un-masked is fixed in-test. All named
findings are RESOLVED — `test_modules`, `test_object_protocol`, and the `lib`
unit suite are green under Stacked Borrows *and* Tree Borrows.

---

## Addendum — BUFFER-DISTILL-55 (2026-07-10)

The registry-free buffer rewrite (`src/api/buffer.rs`, task #55) preserved the
finding-C discipline (all C-visible descriptor pointers are raw projections;
the `ExportInternal` allocation is `std::alloc`-raw with **no** reference ever
formed over it) and surfaced two further items, both fixed at the root:

### F. REAL UB (model-dependent) — self-referential `Py_buffer` vs `&mut` re-borrow

`PyBuffer_FillInfo` is now CPython-exact: `shape = &view->len`,
`strides = &view->itemsize` (self-pointers INTO the struct). A **Rust** caller
that re-borrows the filled struct (`&mut view`) to pass it onward (e.g. to
`PyMemoryView_FromBuffer`) mints a Unique retag over the whole struct that pops
the stored self-pointer tags — a later deref of `info.shape` reads through a
dead tag even though the address is right. Pure C flows never retag and are
unaffected. **Fix:** `descriptor_from_pybuffer` detects the two CPython
self-referential patterns by **address equality** (access-free — tags do not
participate in comparison) and reads the *fields* instead of dereferencing the
interior pointers (1-D enforced; other ranks with self-pointers fail closed).
Caught by the new anti-dangle gate
`test_memoryview_descriptor_outlives_constructing_frame` under SB.

### G. Miri limitation — counting global allocator (bench), excluded under Miri

`buffer_export_bench.rs`'s `CountingAlloc` (`#[global_allocator]`, test-binary
only; landed with the Phase-1 profile `e7d2f82332`) made **lib under Miri red
on main**: with a custom global allocator Miri interprets the REAL Windows
`System` code, whose dealloc of an over-aligned allocation reads the alignment
header stored *before* the payload — outside the payload-ranged Unique tag a
`Box` carries (trips in libtest's own mpmc-channel teardown, 128-byte-aligned
nodes). Not molt code and not reachable in production. **Fix:** the allocator
is `#[cfg(not(miri))]`; the budget test still runs every export cycle under
the interpreter (UB coverage), and the deterministic allocation counts are
asserted on every native `cargo test` run.

Post-rewrite matrix: lib (57) + `test_modules` (25) + `test_object_protocol`
(38, incl. the anti-dangle gate) — **0 UB under Stacked Borrows AND Tree
Borrows** (`-Zmiri-ignore-leaks` for the documented immortal-global exception,
which now also covers the datetime capsule/timezone statics the newer lib
tests initialize).
