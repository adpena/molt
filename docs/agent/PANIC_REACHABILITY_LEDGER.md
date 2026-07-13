# Panic-Site Reachability Ledger — CPython-ABI surface

> **Scope.** Every semantic Rust panic/abort row reachable from a C-extension
> caller across the Molt CPython extension-ABI boundary
> (`runtime/molt-cpython-abi/` and the runtime-side hook vtable in
> `runtime/molt-runtime/src/cpython_abi_hooks.rs`, plus the object/call/alloc
> paths those hooks reach). A row may group sibling expressions that implement
> one failure mechanism. On the shipped witness the panic strategy is
> `panic = "abort"` (`[profile.wasm-release]` / `[profile.release-output]` in
> `Cargo.toml`), so every reachable panic becomes an opaque
> `RuntimeError: unreachable` trap with no diagnostic. Non-unwinding allocator
> and invariant aborts are worse: no panic firewall can catch them.
>
> **Anchor.** Function names and semantic mechanisms are the identities; line
> numbers are only navigation anchors. This refresh was audited on 2026-07-13
> against **`937a17b727a96fd4b46f7499874650ea627bb13a` plus the current dirty L7
> authority tree**.
>
> **Firewall status (load-bearing).** `runtime/molt-cpython-abi/src/` contains
> **613** `#[no_mangle]` / `#[unsafe(no_mangle)] extern "C" fn` entrypoints and
> **zero** `catch_unwind` and **zero** `with_gil_entry!` uses. The runtime's
> catching FFI macros live in `molt-runtime`/`molt-runtime-core`, not on this ABI
> surface. The C boundary therefore has **no uniform panic firewall**.

---

## 1. Executive summary

**34 verified semantic panic/abort rows**: **33 C-reachable** and one deliberate
dev-only compiled-Python assertion. This is the current reconciliation of the
former 22-row ledger: 8 old rows resolved or disappeared and 20 new same-class
L7 rows entered scope.

### Counts by kind

| Kind | Count | What it is |
|---|---:|---|
| `panic` | 8 | explicit `panic!`/`unwrap`/`unreachable`, or infallible capacity growth |
| `arithmetic_overflow` | 4 | unchecked `+`/`*`/negation over a C-visible size or index |
| `c_abort` | 10 | non-unwinding allocator, fatal-contract, provenance, RC, or lifecycle abort |
| `lock_poison` | 5 | `Mutex::lock().unwrap()` on a C-reachable path |
| `alignment` | 1 | typed slice over a C-provided under-aligned pointer |
| `expect` | 5 | pre-init, pairing, ordinal, or deallocation invariant `expect` |
| `assert` | 1 | deliberate dev-only compiler drift detector |
| `index_oob` | 0 | the former torn-dict pair indexing was removed |
| **Total** | **34** | **33 C-reachable + 1 non-C** |

### Counts by severity

| Severity | Count | Sites |
|---|---:|---|
| **Med** | 2 | `PySequence_Repeat`; `PyUnicode_FromKindAndData` alignment |
| Low | 32 | the remaining rows, including the non-C drift assertion |

Severity measures realistic witness trigger probability, not the seriousness of
the consequence. Every `c_abort` row is terminal if its invariant breaks.

### Firewall-coverage reality

A `catch_unwind` firewall converts a panic into a Python error only under
`panic = "unwind"`. It does not help:

1. The shipped `panic=abort` witness.
2. `handle_alloc_error`, `Py_FatalError`, or any explicit `process::abort()`.
3. Release-mode memory corruption where a debug assertion was the only trap.

The firewall remains useful and structurally required for unwind builds, but
the shipped artifact depends on per-site checked arithmetic/allocation and on
the RC/lifecycle invariants being correct before any terminal abort is reached.

### Top-10 most-likely next witness traps

| # | Site | Kind / Sev | Why it ranks here |
|---|---|---|---|
| 1 | `cpython-abi/src/bridge.rs:618/:637/:661` (`tag_to_type`) | expect / low | Every managed-handle materialization crosses the tag table; one pre-`abi_init` crossing aborts. |
| 2 | `cpython-abi/src/api/strings.rs:225/:231` (`PyUnicode_FromKindAndData`) | alignment / **med** | A legal under-aligned UCS2/UCS4 C buffer traps in debug and is UB in release. |
| 3 | `cpython-abi/src/api/abstract_sequence.rs:1103/:1125/:1135` (`PySequence_Repeat`) | arithmetic / **med** | Tuple multiplication can wrap to OOB writes; string/bytes repetition can capacity-abort. |
| 4 | `cpython-abi/src/bridge.rs:1931-2100` | c_abort / low | Fresh, hot canonical-view RC/finalization lifecycle authority; any state-machine defect is terminal. |
| 5 | `cpython-abi/src/api/refcount.rs:33` (`Py_INCREF`) | c_abort / low | A managed C view retained after terminal runtime death aborts immediately. |
| 6 | `cpython-abi/src/api/errors.rs:2684-2704` (`molt_tuple_items`) | panic / low | Infallible collection remains on the hot `PyArg_ParseTuple` path. |
| 7 | `cpython-abi/src/api/object.rs:2242-2250` | panic / low | `PyObject_Call` tuple marshalling reserves from C-visible tuple length. |
| 8 | `cpython-abi/src/api/object.rs:2580-2670` | arithmetic / low | Vectorcall flattening adds positional/keyword counts and allocates infallibly. |
| 9 | `cpython-abi/src/api/object.rs:2940-2942` (`tuple_arg_vec`) | panic / low | FASTCALL tuple conversion reserves directly from tuple length. |
| 10 | `runtime/src/cpython_abi_hooks.rs:98/:112` | expect / low | Mismatched GIL/thread-state pairing crosses a direct C hook and aborts. |

Everything below the cut requires a rarer contract violation, prior corruption,
lock poisoning, extreme allocation, or a terminal RC/finalization invariant
break. `Py_FatalError` is intentionally terminal; the class-layout assertion is
not C-reachable.

---

## 2. Sites by kind

Legend: **Reach** is the C entrypoint/call chain. **Sev** is M/L. **Lane** is
the batch-fix lane in §3. A row groups sibling line sites only when they enforce
one semantic mechanism.

### 2.1 `panic` (8)

| Site | Reach | Trigger | Sev | Fires on abort witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/api/object.rs:2242/:2250` | `PyObject_Call` → `molt_tuple_bits_from_c_tuple` | `Vec::with_capacity(ob_size)` capacity overflow or allocator abort | L | yes | **3** |
| `cpython-abi/src/api/object.rs:2940/:2942` | C-function FASTCALL → `tuple_arg_vec` | `Vec::with_capacity(PyTuple_Size)` | L | yes | **3** |
| `cpython-abi/src/api/errors.rs:2684/:2696/:2704` | `PyArg_ParseTuple` → `molt_tuple_items` | list/tuple range collection pre-reserves from runtime-visible length | L | yes | **3** |
| `cpython-abi/src/bridge.rs:1272/:1327/:1329` | managed tuple materialization/refresh | tuple entry disappears or changes kind while its handle shard is locked | L | yes | **2** |
| `runtime/src/cpython_abi_hooks.rs:427/:453` | C list slice replacement → specialized-list snapshot | second exact-type dispatch reaches `unreachable!` after corruption | L | yes | **2** |
| `runtime/src/object/mod.rs:767/:853/:1003/:1552` | C-reachable class/object lifecycle and deallocation | invalid immutable aux kind reaches debug validation or downstream `unreachable!` | L | yes | **2** |
| `cpython-abi/src/api/strings.rs:206/:215` | `PyUnicode_FromKindAndData` | infallible reserve from C-provided code-unit count | L | yes | **3** |
| `cpython-abi/src/api/abstract_sequence.rs:297-322/:1439/:1504` | sequence materialization, in-place concat/repeat | infallible snapshot capacity from C-visible sequence length | L | yes | **3** |

The capacity rows can fail as a catchable capacity-overflow panic or as an
uncatchable allocator abort. They require checked `try_reserve`, not merely a
panic firewall.

### 2.2 `arithmetic_overflow` (4)

| Site | Reach | Trigger | Sev | Fires on abort witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/api/abstract_sequence.rs:1072/:1103/:1125/:1135` | `PySequence_Repeat`/`InPlaceRepeat` | `len * reps`; release can wrap to too-small tuple allocation and OOB writes; str/bytes `.repeat` can capacity-abort | M | yes | **3** |
| `cpython-abi/src/api/object.rs:2953/:2955` | bound-method dispatch → `prepend_bound_self` | `PyTuple_New(len + 1)` at `Py_ssize_t::MAX` | L | debug only | **3** |
| `cpython-abi/src/api/slice.rs:191/:222` | direct C `PySlice_AdjustIndices` | `-step` for `PY_SSIZE_T_MIN` | L | debug only | **3** |
| `cpython-abi/src/api/object.rs:2580-2670` | `PyObject_Vectorcall*` / `_PyVectorcall_Call` | `1 + nargs + nkw` plus infallible capacities at `:2597/:2654` | L | yes | **3** |

### 2.3 `c_abort` (10)

`c_abort` means a non-unwinding abort: allocator failure, the deliberate
`Py_FatalError` contract, or a provenance/RC/lifecycle guard that calls
`process::abort()`. `catch_unwind` cannot intercept any row here.

| Site | Reach | Trigger | Sev | Fires on abort witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/api/strings.rs:1106/:1114` | `PyBytes_FromStringAndSize` | NULL input uses `vec![0; len]`; non-NULL input uses infallible `.to_vec()` | L | yes | **3** |
| `cpython-abi/src/api/strings.rs:478/:482` | `PyUnicode_New` | `vec![b' '; size]` allocator abort instead of NULL + `MemoryError` | L | yes | **3** |
| `cpython-abi/src/api/memory.rs:258/:265` | `Py_FatalError` | intended unconditional CPython fatal contract | L | yes, by design | **none** |
| `cpython-abi/src/bridge.rs:1931-2100` | C INCREF/DECREF, GC root adjustment, finalization | invalid canonical-view lifecycle/refcount transition; terminal sites at `:1947/:1954/:1981/:2000/:2015/:2020/:2033/:2040/:2077/:2081/:2100` | L | yes | **2** |
| `cpython-abi/src/api/numbers.rs:2508/:2517/:2718/:2727` | numeric carrier `tp_dealloc` | bridge allocation provenance is missing | L | yes | **2** |
| `cpython-abi/src/api/refcount.rs:21/:33` | `Py_INCREF` on managed view | runtime rejects view retention after terminal death | L | yes | **2** |
| `cpython-abi/src/api/sequences.rs:650/:755` | `PyTuple_SetItem` | runtime mutation committed but canonical ABI view update failed | L | yes | **2** |
| `runtime/src/object/mod.rs:1992-2348` | ABI hook `inc_ref`/`dec_ref` → runtime RC | overflow, corrupt type id, retain after terminal death, or underflow; aborts at `:2001/:2039/:2046/:2107/:2114/:2330/:2348` | L | yes | **2** |
| `runtime/src/object/mod.rs:2607-2641` | C-visible finalizer/weakref deallocation window | committed-dead object is reopened or its type id changes | L | yes | **2** |
| `runtime/src/object/backing.rs:275/:278` | C-triggered deallocation of Vec-backed runtime objects | tracked vector owner provenance is null/corrupt | L | yes | **2** |

#### Load-bearing RC invariant-abort family

The seven provenance/RC/lifecycle rows above are not substitutes for validation;
they are terminal last lines of defense against use-after-free and ownership
corruption. They are deliberately not converted to recoverable Python errors.
Closure requires structural proof that normal C refcount traffic, finalization,
weakref callbacks, tuple mutation, numeric-carrier retirement, and tracked
backing retirement preserve the invariant. A panic firewall has no effect on
these `process::abort()` sites.

### 2.4 `lock_poison` (5)

These cannot be the primary failure on the shipped `panic=abort` witness because
an abort never unwinds and therefore never poisons a mutex. They can cascade in
`panic=unwind` iteration builds after an earlier panic.

| Site | Reach | Trigger | Sev | Fires on abort witness? | Lane |
|---|---|---|---|---|---|
| `runtime/src/object/ops.rs:1808/:1849` | C attr/dict operation on dict-subclass instance | dict-subclass sidecar mutex was poisoned | L | no | **2** |
| `runtime/src/call/dispatch.rs:95` | unresolved C-triggered builtin call | module-cache mutex was poisoned | L | no | **2** |
| `runtime/src/cpython_abi_hooks.rs:1154/:1186` | `PyEval_GetBuiltins` hook fallback | module-cache mutex was poisoned | L | no | **2** |
| `runtime/src/cpython_abi_hooks.rs:2094/:2112` | `PyImport_AddModule` hook | module-cache mutex was poisoned | L | no | **2** |
| `runtime/src/object/mod.rs:3299` | C DECREF of dict-subclass object | dict-subclass sidecar removal sees poisoned mutex | L | no | **2** |

### 2.5 `alignment` (1)

| Site | Reach | Trigger | Sev | Fires on abort witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/api/strings.rs:206/:225/:231` | `PyUnicode_FromKindAndData` UCS2/UCS4 | `from_raw_parts(data.cast::<u16/u32>(), size)` over an under-aligned legal C buffer | M | debug trap; release UB | **3** |

### 2.6 `expect` (5)

| Site | Reach | Trigger | Sev | Fires on abort witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/bridge.rs:602/:618/:637/:661` | every managed handle → `PyObject*` materialization | `TAG_TABLE` used before `abi_init` | L | yes | **2** |
| `runtime/src/cpython_abi_hooks.rs:93/:98` | `PyGILState_Release` | no matching `PyGILState_Ensure` guard | L | yes | **2** |
| `runtime/src/cpython_abi_hooks.rs:107/:112` | `PyEval_RestoreThread` | no matching `PyEval_SaveThread` guard | L | yes | **2** |
| `runtime/src/object/gc.rs:155/:166` | allocation of any cyclic-capable object through C hooks | `u64` GC allocation ordinal exhausts | L | yes | **2** |
| `runtime/src/object/mod.rs:3034/:3038` | C DECREF of exception through ABI retirement | exception edges were not detached before retirement | L | yes | **2** |

### 2.7 `index_oob` (0)

The former `hook_dict_set`/`hook_dict_get` odd-pair `chunk[1]` row is resolved:
the hooks now delegate to `dict_set_in_place` / `dict_get_in_place` and do not
index a torn order vector.

### 2.8 `assert` (1) — dev-only, not C-reachable

| Site | Reach | Trigger | Sev | Fires on abort witness? | Lane |
|---|---|---|---|---|---|
| `runtime/src/call/class_init.rs:259-264` | compiled-Python `Class()` with a compile-time-folded size | `debug_assert_eq!(payload_size_bytes, class_layout_size, "frontend layout drift")` | L | no | **none** |

### 2.9 Resolved in the current L7 tree (8 former rows)

| Former row | Current structural resolution |
|---|---|
| runtime hook double registration | `molt_cpython_abi_register_hooks` validates and returns `-1` when `try_set_runtime_hooks` cannot install; no panic |
| `PyTuple_New` eager `vec![NULL; n]` | checked `Layout` + raw `alloc_zeroed`; NULL + `MemoryError` on failure |
| slab exhaustion panic | slab authority deleted |
| slab double-free panic | slab authority deleted |
| slab mutex poison | slab authority deleted |
| list/tuple hook payload type-punning | list mutations use canonical promotion/dispatch; tuple mutation exact-tag guards |
| `hook_tuple_set` `i + 1` overflow/capacity growth | `checked_add`, exact tuple tag, and capacity bound |
| dict hook torn-pair indexing | hooks delegate to the runtime dict authority; no `chunk[1]` |

The registration result is a failure sentinel, not an idempotent successful
no-op. Do not restore the old panic or describe the new behavior as success.

---

## 3. Batch-fix plan (3 lanes, grouped by mechanism)

### Lane 1 — `PANIC-FW`: uniform unwind-build ABI firewall

Wrap all **613** `#[no_mangle]` / `#[unsafe(no_mangle)] extern "C" fn`
entrypoints in one typed boundary mechanism that catches unwind panics, records
the function and failure, installs `SystemError`, and returns the correct C fail
sentinel. A gate must reject every newly added bare entrypoint.

This is zero-cost on the non-panic path and inert under `panic=abort`. Do not
flip the size-oriented shipped profiles merely to activate it. It is protection
for unwind iteration/acceptance builds; Lanes 2-3 remain load-bearing for the
shipped witness, allocator aborts, release corruption, and every RC invariant
abort.

### Lane 2 — `PANIC-PLUMB`: state-machine invariants and poison-free plumbing

Open work:

- Make tag-table use fail closed or self-initialize instead of `expect`.
- Replace the two GIL/thread-state guard-stack `expect`s with explicit pairing
  validation and an honest C error path where the API contract permits it.
- Replace all five C-reachable `lock().unwrap()` rows with a single shared
  poison policy; do not leave sibling module-cache or dict-subclass paths.
- Replace tuple-view refresh `expect`/`unreachable` with a checked locked-state
  transition that releases staged ownership on every mismatch.
- Remove the specialized-list snapshot `unreachable` arm by retaining one
  validated representation discriminator through the snapshot transaction.
- Make aux-kind validation a release-build checked authority. Corruption must
  be diagnosed before any wrong sidecar pointer is dereferenced.
- Prove the load-bearing RC invariant-abort family end-to-end. Preserve terminal
  aborts for genuine ownership corruption; remove only reachable false positives
  by unifying lifecycle authority.

Already resolved and not open work: double-registration panic, list/tuple
type-punning, tuple index/capacity overflow, and every removed slab row.

### Lane 3 — `PANIC-CHECKED`: checked arithmetic, allocation, and unaligned reads

Open work:

- `PySequence_Repeat`: `checked_mul`, checked destination indexing, and
  `try_reserve`/honest `MemoryError` for tuple, str, and bytes repetition.
- `prepend_bound_self`, vectorcall stack construction, and all C-size additions:
  `checked_add` before allocation or indexing.
- `PySlice_AdjustIndices`: handle `PY_SSIZE_T_MIN` with `unsigned_abs` or the
  same clamp used by `PySlice_Unpack`.
- Replace infallible `Vec::with_capacity`, `collect`, `.to_vec()`, and `vec![...]`
  in the listed marshalling/materialization rows with `try_reserve`-backed
  construction that installs `MemoryError` and returns the C fail sentinel.
- `PyUnicode_FromKindAndData`: validate `size * element_width`, then use
  `ptr::read_unaligned` or copy raw bytes into aligned storage.

Already resolved and not open work: `PyTuple_New` eager allocation,
`hook_tuple_set` growth/indexing, and dict hook pair indexing.

### Out of fix scope (correct as-is)

- `Py_FatalError` remains an unconditional abort by CPython contract. It may
  route the rendered message through the durable diagnostics channel first.
- The class-layout `debug_assert_eq!` remains a non-C compiler-drift detector.
- Terminal RC/provenance aborts remain correct last defenses for genuine
  corruption; the engineering task is to prove normal traffic cannot reach
  them, not to catch or downgrade them.

---

## 4. Refresh protocol

This ledger is a reachability hypothesis, not a static grep. Refresh it before
each witness-acceptance attempt and whenever the ABI surface, hook vtable,
bridge lifecycle, object header, RC, GC, or deallocator authority changes.

1. Work from the live canonical worktree and record both the commit and dirty
   state. Re-anchor by function name, never by stale line number.
2. Re-enumerate candidate panics and non-unwinding aborts:
   ```text
   rg -n 'panic!|\.unwrap\(\)|\.expect\(|unreachable!|debug_assert|with_capacity|vec!\[|\.resize\(|from_raw_parts|process::abort|handle_alloc_error' \
     runtime/molt-cpython-abi/src runtime/molt-runtime/src/cpython_abi_hooks.rs
   ```
   Then trace only the object/call/alloc/RC/GC/deallocation paths reached by
   those hooks.
3. Prove C reachability per semantic row. Exclude inline tests, native-JIT-only
   runtime exports, and branches whose failure variant is eliminated by an
   immediate authority check.
4. Group only sibling expressions enforcing one mechanism. Keep separate rows
   when the kind or remedy differs, as with Unicode capacity versus alignment.
5. Recount kind, severity, and C reachability from the live tables. The total
   must equal the sum of the kind rows.
6. Recount `no_mangle` extern functions with an attribute-to-function match;
   do not count exported statics. Recheck `catch_unwind` and `with_gil_entry!`.
7. Move landed rows to §2.9 with the structural resolution. Delete references
   to removed authorities instead of preserving them as compatibility history.
8. Confirm shipped-profile closure separately. An unwind firewall does not
   close `panic=abort`, allocator abort, explicit process abort, or release UB.

---

*Ledger refreshed 2026-07-13 against `937a17b727a96fd4b46f7499874650ea627bb13a`
plus the current dirty L7 authority tree. Lane names: `PANIC-FW`,
`PANIC-PLUMB`, `PANIC-CHECKED`.*
