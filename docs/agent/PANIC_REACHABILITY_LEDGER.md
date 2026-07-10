# Panic-Site Reachability Ledger — CPython-ABI surface

> **Scope.** Every Rust `panic`/`abort` that is reachable from a C-extension caller
> across the Molt CPython extension-ABI boundary (`runtime/molt-cpython-abi/` and the
> runtime-side hook vtable in `runtime/molt-runtime/src/cpython_abi_hooks.rs`, plus the
> C-reachable object/call/alloc paths those hooks reach). On the shipped witness the
> panic strategy is `panic = "abort"` (`[profile.wasm-release]` / `[profile.release-output]`
> in `Cargo.toml`), so **every** reachable panic becomes an **opaque `RuntimeError: unreachable`
> trap** with no diagnostic — the exact failure mode that stalls witness bring-up. This
> ledger enumerates those sites, ranks the ones most likely to trap next, and gives a
> ≤3-lane batch-fix plan.
>
> **Anchor.** Line numbers are as of `origin/main` **`2072dfedce`** (2026-07-10). They are
> anchors, not identities — re-anchor by grepping the function name when a line drifts
> (see the Refresh protocol). All findings below were verified against the Molt Rust
> source; a representative sample (`hooks.rs:337`, `bridge.rs:163`,
> `cpython_abi_hooks.rs:151-205`, the `Cargo.toml` panic profiles) was re-read in-tree
> for this ledger.
>
> **Firewall status (load-bearing).** `runtime/molt-cpython-abi/src/` contains **548**
> `#[no_mangle] extern "C"` entrypoints and **zero** `catch_unwind` and **zero**
> `with_gil_entry!` uses. The runtime's ~1121 catching `with_gil_entry!`/`with_core_gil!`
> FFI sites live in `molt-runtime`/`molt-runtime-core`, **not** on this ABI surface. The
> C boundary therefore has **no uniform panic firewall.** See Batch-fix plan Lane 1.

---

## 1. Executive summary

**22 verified panic/abort sites**, 21 of them reachable directly from a C-extension caller
(1 is dev-build-only, reachable only from compiled-Python instantiation).

### Counts by kind

| Kind | Count | What it is |
|---|---:|---|
| `panic` | 7 | explicit `panic!`/`unwrap`/`unreachable` invariant break |
| `arithmetic_overflow` | 4 | `+`/`*` on an untrusted C length overflows (debug-assert panic; release wrap→OOB) |
| `c_abort` | 3 | eager infallible alloc / `Py_FatalError` → `handle_alloc_error`/`process::abort` (**not** catch_unwind-catchable) |
| `lock_poison` | 3 | `Mutex::lock().unwrap()` on a poisoned lock (**cannot fire under `panic=abort`**) |
| `alignment` | 2 | typed slice/deref over a C-provided under-aligned pointer (debug-assert trap) |
| `expect` | 1 | `OnceCell::get().expect()` on a pre-init crossing |
| `index_oob` | 1 | unchecked `chunk[1]` on a possibly-torn pair |
| `assert` | 1 | `debug_assert_eq!` drift detector (dev-only, not C-reachable) |
| **Total** | **22** | |

### Counts by severity

| Severity | Count | Sites |
|---|---:|---|
| **Med** | 5 | `hooks.rs:337`, `abstract_sequence.rs:361`, `strings.rs:157`, `cpython_abi_hooks.rs:157`, `cpython_abi_hooks.rs:202` |
| Low | 17 | the remaining rows |

### Firewall-coverage reality (honest)

A `catch_unwind` firewall (Lane 1) converts a **panic** into a caught error — but only
when the build uses `panic = "unwind"`, and only for the **panic class** (`panic!`,
`unwrap`, `expect`, overflow-in-debug, index-OOB, `with_capacity`/`resize` capacity-overflow,
`from_raw_parts` debug-assert). It does **not** help three things:

1. **The shipped `panic=abort` witness** — `catch_unwind` is a documented no-op there
   (`Cargo.toml`: *"panic = 'abort' broke this entirely — catch_unwind was a no-op"*).
2. **The `c_abort`/OOM class** — `handle_alloc_error` and `std::process::abort()` abort
   **without unwinding**, so `catch_unwind` never sees them even under `panic=unwind`.
3. **Type-confusion in `release`** — a caught trap in `debug` is a **silent heap-corrupting
   OOB write** in `release` (e.g. `cpython_abi_hooks.rs:157`); the firewall hides the trap
   but not the corruption.

So the firewall is necessary but **not sufficient**: it must be paired with a witness
panic-mode decision (Lane 1) and the per-site fail-closed fixes (Lanes 2–3). This is
spelled out per-lane below.

### Top-10 most-likely next witness traps (ranked)

Ranked by realistic probability of firing on the numpy/scipy → WASM witness path (the
witness is currently at the numpy `_multiarray_umath` C-ext init frontier; the split
runtime installs hooks and runs `abi_init` as **separate** steps, which elevates the
init-ordering rows).

| # | Site | Kind / Sev | Why it ranks here |
|---|---|---|---|
| 1 | `runtime/molt-runtime/src/cpython_abi_hooks.rs:157` (`hook_list_append`) | alignment / **med** | **Type confusion, not a rare value.** `PyList_Append`/`SetItem` punries a specialized `TYPE_ID_LIST_INT`/`_BOOL` payload (`*mut ListIntStorage`) as `*mut Vec<u64>` — no `TYPE_ID_LIST` guard (the sibling `_len` at :166 *has* one). Specialized lists are a live Molt representation, so this is input-reachable, not implausible. Also `:178`, `:198`, `:225`. |
| 2 | `runtime/molt-runtime/src/cpython_abi_hooks.rs:202` (`hook_tuple_set`) | arithmetic_overflow / **med** | `PyTuple_SetItem` slot index `i` (a `Py_ssize_t` cast to `usize`) drives `v.resize(i+1, …)`: negative→huge index overflow-panics `i+1` (debug) and `resize` capacity-overflows in **all** profiles. Missing `TYPE_ID_TUPLE` guard too. On the tuple-construction path numpy init exercises heavily. |
| 3 | `runtime/molt-cpython-abi/src/hooks.rs:337` (`set_runtime_hooks`) | panic / **med** | Double `register_hooks` → `panic!("runtime hooks already registered")`. The split-runtime bring-up re-runs init / re-resolves the exported `molt_cpython_abi_register_hooks`; a second call aborts to opaque unreachable. Idempotent `try_set_runtime_hooks` already exists next to it (:355). |
| 4 | `runtime/molt-cpython-abi/src/bridge.rs:163` (`tag_to_type` scalar path) | expect / low | `TAG_TABLE.get().expect("init_tag_table not called")`. On wasm32 this scalar branch is the live one (SIMD variants cfg'd out), reached from **every** handle→PyObject materialization (`molt_foreign_getattr/setattr/call`). One init-ordering slip (hooks registered before `abi_init`) aborts on the first crossing. Very hot path; guaranteed if ordering ever inverts. |
| 5 | `runtime/molt-cpython-abi/src/api/strings.rs:157` (`PyUnicode_FromKindAndData`) | alignment / **med** | `from_raw_parts(data.cast::<u16>()/…<u32>(), size)` over a C-provided UCS2/UCS4 buffer. Debug `from_raw_parts` traps on an under-aligned `data` (legal input; CPython does unaligned reads). Reached whenever a C ext builds a unicode from a kind+data buffer. |
| 6 | `runtime/molt-cpython-abi/src/api/abstract_sequence.rs:361` (`PySequence_Repeat`) | arithmetic_overflow / **med** | `len * count` for `tuple * n`. On wasm32 `usize` is 32-bit → overflows at 2^32 (≈100k-elem tuple × ≈50k). Debug: overflow panic; **release: silent wrap → too-small alloc → OOB writes** (second bug). |
| 7 | `runtime/molt-cpython-abi/src/api/errors.rs:646` (`molt_tuple_items`) | panic / low | `(0..len).map(..).collect::<Vec<_>>()` on the `PyArg_ParseTuple` **hot path** (≈every C-ext function entry). Pathological reported length pre-reserves `len*8` → capacity-overflow/alloc abort. Hottest path in the set; low only because real arg counts are tiny. |
| 8 | `runtime/molt-runtime/src/cpython_abi_hooks.rs:252` (`hook_dict_set`/`_get`) | index_oob / low | `dict_order(ptr).chunks_mut(2)` then unconditional `chunk[1]`; a torn (odd-length) order Vec panics. Sibling `hook_object_call` (:445) already guards `if chunk.len()==2`; these two do not. Reachable via dict ops crossing from C. |
| 9 | `runtime/molt-cpython-abi/src/api/object.rs:2052` (`prepend_bound_self`) | arithmetic_overflow / low | Bound-method dispatch: `PyTuple_New(len + 1)` where `len = PyTuple_Size(args)`; `ob_size == Py_ssize_t::MAX` overflow-panics `len+1` under debug assertions. |
| 10 | `runtime/molt-cpython-abi/src/api/object.rs:2039` (`tuple_arg_vec`) | panic / low | `METH_FASTCALL` path: `Vec::with_capacity(PyTuple_Size(args) as usize)`; corrupt/huge `ob_size` aborts in `with_capacity`. Same class as `:1629`. |

Everything below the cut is lower-probability: the remaining `c_abort` OOM sites need an
*implausibly large but legal* size; the three `lock_poison` sites **cannot fire on the
`panic=abort` witness at all** (no unwind ⇒ no poisoning) and are iteration-build-only; the
two `cold_header` slab guards need ~4.29 B live objects or prior corruption; `memory.rs:200`
is CPython's intended `Py_FatalError` contract; `class_init.rs:249` is a dev-only drift assert.

---

## 2. Sites by kind

Legend: **Reach** = the C entrypoint / call chain. **Sev** M/L. **Lane** = batch-fix lane
(see §3). "Fires on `panic=abort` witness?" = whether the site aborts the *shipped* artifact
(vs. only iteration/debug builds).

### 2.1 `panic` (7)

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/hooks.rs:337` | `molt_cpython_abi_register_hooks` (:364→:369) | second hook registration → `panic!("runtime hooks already registered")` | M | yes | **2** |
| `cpython-abi/src/api/object.rs:1629` | `PyObject_Call` args → `molt_tuple_bits_from_c_tuple` | `Vec::with_capacity(n)` where `n = ob_size` (only `<0` rejected); capacity-overflow/alloc abort | L | yes (alloc-abort) | **3** |
| `cpython-abi/src/api/object.rs:2039` | `molt_cfunction_call` METH_FASTCALL → `tuple_arg_vec` | `Vec::with_capacity(PyTuple_Size)`; corrupt `ob_size` aborts | L | yes (alloc-abort) | **3** |
| `cpython-abi/src/api/errors.rs:646` | `PyArg_ParseTuple` → `molt_tuple_items` (list fallback :642) | `(0..len).collect::<Vec>()` pre-reserves `len*8`; overflow/alloc abort | L | yes (alloc-abort) | **3** |
| `cpython-abi/src/api/sequences.rs:210` | `PyTuple_New(size)` | `vec![null_mut(); n]`; `n>~2^29` (wasm32) → capacity-overflow / `handle_alloc_error` | L | yes (alloc-abort) | **3** |
| `runtime/src/object/cold_header.rs:63` | `alloc_cold_header` (modules / C-fn callables / foreign wrappers) | `panic!("cold header slab exhausted")` at `u32::MAX` entries | L | yes | **2** |
| `runtime/src/object/cold_header.rs:113` | object dealloc (`dec_ref` of cold-header object) | `panic!("cold header slab double free")` (+ `:47` free-list corruption) after prior corruption | L | yes | **2** |

Note: the four `with_capacity`/`vec![…; n]`/`collect` rows are **alloc-abort** — a
`catch_unwind` firewall catches the *capacity-overflow panic* variant but **not** the
`handle_alloc_error` (real OOM) variant. Both need the Lane 3 `try_reserve` fix to fail
closed with `PyErr_NoMemory()`.

### 2.2 `arithmetic_overflow` (4)

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/api/abstract_sequence.rs:361` | `PySequence_Repeat`/`InPlaceRepeat` (`tuple * n`) | `len * count`; wasm32 32-bit `usize` overflows at 2^32. **Release: silent wrap → OOB writes.** | M | debug: panic / release: **OOB (worse)** | **3** |
| `runtime/src/cpython_abi_hooks.rs:202` | `PyTuple_SetItem`/`SET_ITEM` → `hook_tuple_set` | `v.resize(i+1,…)`; `i+1` overflow (debug) + `resize` capacity-overflow (all profiles); missing `TYPE_ID_TUPLE` guard | M | yes (`resize` overflow) | **3** (+2 for guard) |
| `cpython-abi/src/api/object.rs:2052` | `molt_method_call` bound dispatch → `prepend_bound_self` | `PyTuple_New(len + 1)`, `len==Py_ssize_t::MAX` → `len+1` overflow panic (debug) | L | debug only | **3** |
| `cpython-abi/src/api/slice.rs:164` | direct C call to exported `PySlice_AdjustIndices` with `step=PY_SSIZE_T_MIN` | `-step` negation overflows (`isize::MIN` has no positive) (debug). numpy routes through `PySlice_Unpack` which clamps, so only a contract-violating direct caller reaches it. | L | debug only | **3** |

### 2.3 `c_abort` — eager alloc / fatal (3)

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/api/strings.rs:769` | `PyBytes_FromStringAndSize(NULL, len)` | `vec![0u8; len]` eager alloc; large-but-legal `len` → `handle_alloc_error` abort vs CPython NULL+MemoryError | L | yes (alloc-abort) | **3** |
| `cpython-abi/src/api/strings.rs:362` | `PyUnicode_New(size, maxchar)` | `vec![b' '; size]` eager alloc; same OOM-abort class | L | yes (alloc-abort) | **3** |
| `cpython-abi/src/api/memory.rs:200` | `Py_FatalError(msg)` | `eprintln!` then `std::process::abort()` — **intended** CPython contract (unrecoverable state), not a bug | L | yes (by design) | **none** (enumeration only; optionally route msg via `diagnostics::emit_line`) |

`catch_unwind` does **not** catch any row here — they abort without unwinding. Lane 3
`try_reserve` is the only fix that makes the two eager-alloc rows fail closed.

### 2.4 `lock_poison` (3) — cannot fire on the `panic=abort` witness

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `runtime/src/object/cold_header.rs:132` | `alloc/free/get_cold_header` | `cold_header_slab().lock().unwrap()` on a poisoned mutex | L | **no** (abort never poisons) | **2** |
| `runtime/src/object/ops.rs:1847` | attr/dict ops on a dict-subclass instance (`defaultdict`) from C → `dict_subclass_storage_bits` | `dict_subclass_storage.lock().unwrap()` | L | **no** | **2** |
| `runtime/src/call/dispatch.rs:95` | `molt_call_builtin` (unresolved builtin name) | `module_cache.lock().unwrap()` (lone bare `.lock().unwrap()` on a non-test path in `call/`) | L | **no** | **2** |

Class note: other representatives of the same pattern (out of ledger scope but same fix):
`builtins/attributes.rs:104`, `builtins/attributes/state.rs:156`,
`object/ops_format.rs:531/541`. These fire only in `panic=unwind` iteration builds after a
*prior* panic already poisoned the lock — a secondary cascade, never a primary witness trap.

### 2.5 `alignment` (2)

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/api/strings.rs:157` (+`:163`) | `PyUnicode_FromKindAndData` UCS2/UCS4 | `from_raw_parts(data.cast::<u16>()/<u32>(), size)` over an under-aligned C buffer → debug precondition trap | M | debug only (release UB read) | **3** |
| `runtime/src/cpython_abi_hooks.rs:157` (+`:178/:198/:225`) | `PyList_Append/SetItem`, tuple item/set | **type confusion**: specialized `TYPE_ID_LIST_INT/_BOOL` payload punned as `*mut Vec<u64>`; missing `TYPE_ID_LIST` guard | M | **debug: trap / release: heap corruption** | **2** |

Distinct from the separate `bridge.rs:571` misaligned-deref lane (landed `a98ef2978e`) —
that fix does not touch either `from_raw_parts` or the hook type-punning; fix locally.

### 2.6 `expect` (1)

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `cpython-abi/src/bridge.rs:163` | every `handle_to_pyobj`/`handle_to_borrowed_pyobj` → `allocate_pyobj_entry` → `tag_to_type` (from `molt_foreign_getattr/setattr/call`, `c_layout_tuple_from_molt`) | `TAG_TABLE.get().expect("init_tag_table not called")` on a pre-`abi_init` crossing | L | yes | **2** |

### 2.7 `index_oob` (1)

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `runtime/src/cpython_abi_hooks.rs:252` (+`:275`) | `hook_dict_set`/`hook_dict_get` via dict ops from C | `chunks_mut(2)` then unconditional `chunk[1]` on an odd-length order Vec (sibling `:445` guards, these don't) | L | yes (if torn) | **3** |

### 2.8 `assert` (1) — dev-only, not C-reachable

| Site | Reach | Trigger | Sev | Fires on abort-witness? | Lane |
|---|---|---|---|---|---|
| `runtime/src/call/class_init.rs:249` | compiled-Python `Class()` with a compile-time-folded size (NOT `molt_type_new`/`call_type_with_builder`) | `debug_assert_eq!(payload_size_bytes, class_layout_size, "frontend layout drift")` — a compiler-invariant drift detector | L | **no** (debug-assert; not C-reachable) | **none** (deliberate fail-closed drift detector; optionally downgrade to `diagnostics::emit_line` + slow-path fallback for iteration robustness) |

---

## 3. Batch-fix plan (3 lanes, grouped by mechanism)

Three lanes, each a coherent mechanism, orderable in parallel except that Lane 1's
witness-panic-mode decision gates whether Lane 1 has any effect on the shipped artifact.

### Lane 1 — `PANIC-FW`: `catch_unwind` firewall at every `extern "C"` boundary  ← **required, structural**

**Mechanism.** Wrap every `#[no_mangle] extern "C"` body in `molt-cpython-abi` (548 sites)
in a single boundary macro — `abi_entry! { … }` — that does
`std::panic::catch_unwind(AssertUnwindSafe(|| { … }))` and on `Err` calls
`record_silent_failure(...)`, sets a pending `SystemError` (`PyErr_SetString`), and returns
the function's fail sentinel (`NULL` / `-1` / `0`). This is the **structural** fix: it makes
the entire **panic class** (rows in §2.1 explicit-panic, §2.2 debug-overflow, §2.5
`strings.rs:157`, §2.6, §2.7) non-fatal **at once**, converting an opaque
`RuntimeError: unreachable` into an honest Python `SystemError` + a recorded silent-failure
breadcrumb — instead of hand-editing 22 sites and re-auditing forever.

**Audit finding that makes this Lane 1.** The ABI surface has **no** panic firewall today:
`grep -c catch_unwind runtime/molt-cpython-abi/src` = 0, `grep -c with_gil_entry …` = 0,
across **548** `no_mangle` entrypoints. The runtime's ~1121 `with_gil_entry!` catching sites
are in `molt-runtime`/`-core` and do **not** cover this crate. So the uniform firewall is
absent and must be built.

**Perf cost — evaluated honestly.**
- On the **non-panic (happy) path** `catch_unwind` is **zero-cost**: no runtime branch, no
  register pressure; it only emits a landing pad in the unwind tables. Under `panic=abort`
  the landing pad itself is **cfg-eliminated before codegen** (this is exactly the mechanism
  the shared `with_gil_entry_body!` macro already relies on per `Cargo.toml` L459-461), so
  there is **not even table bloat** on the shipped abort artifact.
- **Honest limitation #1 — `panic=abort` inertness.** `catch_unwind` only *catches* under
  `panic = "unwind"`. The shipped witness is `panic = "abort"` (`wasm-release` /
  `release-output`), where `Cargo.toml` documents *"catch_unwind was a no-op."* So on the
  **shipped witness the firewall catches nothing.** To make Lane 1 live on the witness you
  must **either** (a) build the witness/iteration acceptance profile with `panic = "unwind"`
  (the `dev`/`dev-fast`/`release-fast` iteration profiles already are — the firewall is
  immediately live there, which is where the debug-assert overflow/alignment panics actually
  fire), **or** (b) accept that on the abort artifact the firewall is a no-op and rely on
  Lanes 2–3 for the shipped path. **Principal-engineer call: adopt the macro now** (it is
  correct and free), make it live on all `panic=unwind` iteration/acceptance builds
  immediately, and do **not** flip the shipped `wasm-release`/`release-output` to unwind
  (that re-inflates the size profile with dead unwind tables — the size lane deliberately
  removed them). The shipped abort artifact is covered by Lanes 2–3, which fail closed
  *before* any panic.
- **Honest limitation #2 — alloc-abort & `process::abort`.** `handle_alloc_error` (§2.1
  `with_capacity`/`vec!` OOM, §2.3 eager alloc) and `Py_FatalError`'s `process::abort()`
  abort **without unwinding** → `catch_unwind` never sees them even under `panic=unwind`.
  These **require** Lane 3.
- **Honest limitation #3 — release type-confusion.** The firewall catches the §2.5
  `cpython_abi_hooks.rs:157` *trap* in debug, but in release that site is a silent
  heap-corrupting OOB write with no panic to catch. That row **must** land in Lane 2
  regardless of the firewall.

**Verdict: Lane 1 is recommended and is the structural centerpiece — but it is
necessary-not-sufficient.** It neutralises the panic class cheaply on every `panic=unwind`
build; Lanes 2 and 3 remain load-bearing for the `panic=abort` shipped witness and for the
memory-safety rows the firewall cannot see.

**Teeth.** A gate test that greps `molt-cpython-abi/src` for `#[no_mangle] extern "C"`
bodies not wrapped in `abi_entry!` (fail closed on any new bare entrypoint), plus a
unit test that a deliberately-panicking hooked call returns NULL with `SystemError` set
under `panic=unwind`.

### Lane 2 — `PANIC-PLUMB`: idempotent boundaries, type guards, poison-tolerance, Result-plumbing

**Mechanism.** Replace `panic!`/`expect`/`unwrap`/type-punning with fail-closed semantics
that set an honest pending exception (or degrade to a neutral value) — the fixes the
firewall cannot substitute for because they are about *correct behavior*, not just
*not-aborting*.

- `hooks.rs:337` — make the `extern "C"` path idempotent: `molt_cpython_abi_register_hooks`
  calls the **already-existing** `try_set_runtime_hooks` (returns `bool`, silent no-op on
  second registration); reserve loud `set_runtime_hooks` for a single trusted in-process init.
- `bridge.rs:163` — `TAG_TABLE.get_or_init(build_tag_table)` so first use self-initializes,
  **or** fall back to `&raw mut PyBaseObject_Type` (the honest "object" neutral already
  returned for tag-not-found) when `get()` is `None` — a pre-init crossing degrades to an
  object-typed proxy instead of aborting.
- `cpython_abi_hooks.rs:157/178/198/225` — **memory safety, highest value in this lane.**
  Add the `if object_type_id(ptr) != TYPE_ID_LIST { record_silent_failure; return }` guard
  the `_len` hooks already use, in all four list/tuple hooks; route `TYPE_ID_LIST_INT/_BOOL`
  through their real accessors (`list_int_vec_ref` etc.) or promote-to-generic before
  mutation, so a specialized list is never punned as `Vec<u64>`.
- `cold_header.rs:63/113` — return the alloc-failure sentinel (Option/None → MemoryError up
  the alloc chain) instead of `panic!`; keep the double-free guard but `emit_line` + leak the
  slot (fail-closed early return) rather than abort.
- `cold_header.rs:132`, `ops.rs:1847`, `dispatch.rs:95` (+ the class siblings) —
  `.lock().unwrap_or_else(|e| e.into_inner())` (or a shared poison-tolerant helper); the
  guarded state is structurally valid after a panic. Matches the pattern already used at
  `call/bind.rs:200/208`.

**Teeth.** Unit tests: (a) double `register_hooks` no-ops; (b) `PyList_Append` on a
`TYPE_ID_LIST_INT` handle sets an error and does not corrupt; (c) a poisoned lock path
recovers `into_inner`. A gate that no `.lock().unwrap()` remains on a C-reachable non-test
path in the listed files.

### Lane 3 — `PANIC-CHECKED`: checked arithmetic, validated indexing, `try_reserve` alloc, unaligned reads

**Mechanism.** Never do infallible arithmetic/allocation/typed-slicing on an **untrusted
C-provided length or pointer**. These are the fixes that protect the **`panic=abort` shipped
witness** (fail closed *before* the abort) and close the release-mode OOB/UB rows.

- **Checked arithmetic** (§2.2): `len.checked_mul(count as usize)` (`abstract_sequence.rs:361`,
  → `MemoryError`/`OverflowError`, NULL); `checked_add(1)` (`object.rs:2052`; also close the
  release wrap→OOB by validating before alloc); `hook_tuple_set` (`cpython_abi_hooks.rs:202`)
  bounds-check `i` against real length → pending `IndexError`, `checked_add` for `i+1`,
  `try_reserve`-backed growth (never grow a fixed-size tuple); `slice.rs:164` clamp
  `step = step.max(-Py_ssize_t::MAX)` at entry (mirrors `PySlice_Unpack`) or
  `step.unsigned_abs()`.
- **`try_reserve` / bounded alloc** (§2.1 alloc rows, §2.3 eager rows): replace
  `Vec::with_capacity(n)` / `vec![…; n]` / `(0..len).collect()` with `try_reserve(n)` (or a
  sane `MAX_ARGS`/backing-allocation cap) → on `Err`, `record_silent_failure` +
  `PyErr_NoMemory()`/`SystemError` and return the fail sentinel. Covers `object.rs:1629/2039`,
  `errors.rs:646`, `sequences.rs:210`, `strings.rs:769`, `strings.rs:362`.
- **Validated indexing** (§2.7): `cpython_abi_hooks.rs:252/275` iterate `chunks_exact(2)`
  (or guard `if chunk.len()==2`, matching `hook_object_call:445`).
- **Unaligned reads** (§2.5 `strings.rs:157`): do not build a typed slice over an unaligned C
  pointer — `ptr::read_unaligned` per element, or `copy_nonoverlapping` the raw bytes into an
  aligned `Vec<u16>/Vec<u32>`; validate `size.checked_mul(elem) <= isize::MAX`.

**Teeth.** Differential/unit tests feeding oversized/`MAX`/misaligned lengths through each
entrypoint and asserting an honest Python exception (`MemoryError`/`OverflowError`/`IndexError`/
`SystemError`) + NULL, never a trap; and asserting the release `tuple * n` overflow path no
longer writes OOB.

### Out of fix scope (correct as-is)

- `memory.rs:200` `Py_FatalError` — CPython's intended unconditional-abort contract; only
  optionally route the rendered message through `crate::diagnostics::emit_line` (per M51,
  the parity-safe channel) before `process::abort()` so the wasm host doesn't lose the cause
  to stderr buffering on trap.
- `class_init.rs:249` `debug_assert_eq!` — a deliberate compiler-drift detector, not
  C-reachable; optionally downgrade to `emit_line` + `class_layout_size` slow-path fallback
  for iteration-build robustness.

---

## 4. Refresh protocol

This ledger is a **hypothesis about reachability**, not a static grep — refresh it (a) before
every witness-acceptance attempt and (b) whenever a `molt-cpython-abi` ABI surface or the
`cpython_abi_hooks.rs` vtable lands.

1. **Drift-sweep.** `git fetch origin`; work from a worktree off `origin/main`
   (`python tools/tree_drift_check.py --fetch`) — never audit line numbers against a stale
   shared checkout.
2. **Re-enumerate candidate panics** across the C-reachable surface:
   ```
   rg -n 'panic!|\.unwrap\(\)|\.expect\(|unreachable!|debug_assert|with_capacity|vec!\[|\.resize\(|from_raw_parts|\.lock\(\)\.unwrap\(\)|\[\s*1\s*\]' \
     runtime/molt-cpython-abi/src runtime/molt-runtime/src/cpython_abi_hooks.rs
   ```
   plus the object/call/alloc paths those hooks reach (`object/ops*.rs`, `call/*.rs`,
   `object/cold_header.rs`).
3. **Prove C-reachability per candidate.** A site counts only if a chain exists from a
   `#[no_mangle] extern "C"` export to it. Trace it; drop sites reachable only from
   compiled-Python or Rust-internal paths (mark `reachable_from_c:false`, like
   `class_init.rs:249`).
4. **Classify** into the §2 kinds and set severity by *realistic* witness trigger (numpy/scipy
   arg counts are tiny; type-confusion and init-ordering are the realistic ones).
5. **Re-rank the top-10** against the current witness frontier (today: numpy
   `_multiarray_umath` init; the split runtime's separate hook/`abi_init` steps elevate the
   ordering rows `hooks.rs:337`/`bridge.rs:163`).
6. **Cross-check the firewall invariant.** Re-run `grep -c catch_unwind` +
   `grep -c 'no_mangle'` over `molt-cpython-abi/src`. If Lane 1's `abi_entry!` has landed,
   every entrypoint must be wrapped (the gate test enforces it) and this ledger's "no firewall"
   framing flips to "firewalled; residual abort-class per Lane 3."
7. **Move landed rows to a `§Landed` section** with the fixing SHA (mirroring the divergence
   ledger's landed-fixes convention), so the live table only shows open traps. Re-anchor line
   numbers by function name, not by absolute line.
8. **Confirm on `panic=abort`.** Line-number and reachability are necessary but not
   sufficient — a row is only *closed for the witness* when the shipped `wasm-release`
   (`panic=abort`) build fails closed with an honest exception (Lane 3) or the trap is proven
   unreachable, not merely caught in a `panic=unwind` iteration build.

---

*Ledger authored 2026-07-10 against `origin/main 2072dfedce`. Lane names: `PANIC-FW`,
`PANIC-PLUMB`, `PANIC-CHECKED`.*
