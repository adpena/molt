# Free-Threading Readiness (PEP 703 / `Py_GIL_DISABLED`) — Audit + Design

Status: Phase 1 (audit + design) and Phase 2 (non-conflicting landings) COMPLETE.
Phase 3 (bridge concurrency redesign) SPECCED below, blocked on the
CLASS2-DECODE newtype boundary landing in `bridge.rs`.

Lane: GILFREE-READY. Anchored at origin/main 9a25b62b44 (worktree
C:\Molt\wt-gilfree). Phase 2 landed as 21046db556.

Charter: make the molt↔C boundary and the molt refcount model ready for a
free-threaded (no-GIL) future without any single-threaded perf regression.
Operator doctrine: build for the future, never accumulate debt; simplify and
canonicalize.

---

## 0. Executive summary

1. **molt is better positioned than feared.** Runtime header words now share
   one generated ABI authority but retain distinct custody policies: native
   flags are atomic because scheduler workers touch them before the GIL;
   refcounts use `Cell<u32>` under the deterministic GIL and `AtomicU32` only
   under explicit native `free-threaded`; wasm32 uses cells for both. The
   dec-to-zero path retains canonical Release/Acquire semantics in atomic mode.
2. **The two scalability walls** are (a) molt's process-global runtime GIL
   (`PREINIT_GIL`) and (b) the process-global `GLOBAL_BRIDGE` mutex over four
   `HashMap`s. Both are *correctness-sound today* and both serialize
   everything under N threads.
3. **The forward-debt items found and their disposition:**
   - `Py_mod_gil` slot silently DISCARDED; `PyUnstable_Module_SetGIL` a
     `return 0` stub (**FIXED in Phase 2** — recorded, queryable, gated).
   - `PyUnstable_Module_SetGIL` prototype deviated from CPython (`int` vs
     `void*`) (**FIXED in Phase 2**).
   - `PyGILState_Ensure`/`Release` and `PyEval_SaveThread`/`RestoreThread` are
     **no-ops** while molt has a real GIL — a genuine latent bug **today**
     (§3.4). Deliberately NOT wired in Phase 2: a naive blocking
     implementation can convert today's silent race into a deadlock (§5.3).
     Fixing it is inseparable from the Phase-3 GIL-custody design.
   - The bridge implements exactly the **GIL-ful CPython ABI** (non-atomic
     `ob_refcnt`, GIL-build `PyObject` layout). That is *correct parity*, not
     a bug — but a `Py_GIL_DISABLED`-built extension (`cp313t`/`cp314t`
     wheels) has a **different object header layout** and must never be
     loadable against this bridge without a loud failure (§4.4).
4. **Scoping confirmed from the tree and primary sources:** wasm32 is
   single-threaded (no threads in stable CPython WASM targets either); molt
   already compiles the GIL and the refcount to zero-cost single-threaded
   forms there. Free-threading is a **native-target** concern. No design
   below adds any cost to the wasm path.

---

## 1. Primary-source foundation (all verified 2026-07-11, not from memory)

- **PEP 703** — biased reference counting (owner-thread fast path on
  `ob_ref_local`, atomic `ob_ref_shared` with 2-bit state in the low bits:
  default/maybe-weakref/queued/merged), immortalization, deferred RC for
  functions/code/modules/methods, mimalloc requirement, per-object mutex +
  critical sections. <https://peps.python.org/pep-0703/>
- **CPython 3.14 `Include/refcount.h` / `Include/object.h`** — the
  free-threaded `PyObject` header is a *different layout*: `uintptr_t ob_tid;
  uint16_t ob_flags; PyMutex ob_mutex; uint8_t ob_gc_bits; uint32_t
  ob_ref_local; Py_ssize_t ob_ref_shared;` — **there is no `ob_refcnt` field
  at all**. Even the owner fast path uses relaxed atomic load/store, not a
  plain `++`. `Py_REFCNT` is a computed sum.
  <https://raw.githubusercontent.com/python/cpython/3.14/Include/refcount.h>
- **Immortality constants changed per version** (molt's
  `abi_types::IMMORTAL_REFCNT` already version-gates this correctly —
  verified against the headers):
  - 3.12/3.13 (64-bit): `_Py_IMMORTAL_REFCNT == UINT_MAX`, check is equality.
  - 3.14 (64-bit): `_Py_IMMORTAL_INITIAL_REFCNT == 3<<30`, check is
    **sign-bit of the low 32 bits** (`(int32)ob_refcnt < 0`), incref treats
    `>= initial` as immortal.
  - Free-threaded builds: `ob_ref_local == UINT32_MAX`.
- **`Py_mod_gil` semantics** (3.13+): absent slot ⇒ default
  `Py_MOD_GIL_USED`; importing a `GIL_USED`/undeclared module on a
  free-threaded interpreter **re-enables the GIL** with a `RuntimeWarning`,
  unless the user forces `PYTHON_GIL=0` / `-X gil=0`.
  `int PyUnstable_Module_SetGIL(PyObject *module, void *gil)` is the
  single-phase-init counterpart. <https://docs.python.org/3.14/c-api/module.html>,
  <https://docs.python.org/3.14/howto/free-threading-extensions.html>
- **numpy ≥ 2.1** declares `{Py_mod_gil, Py_MOD_GIL_NOT_USED}` in
  `numpy/_core/src/multiarray/multiarraymodule.c` (gated `PY_VERSION_HEX >=
  0x030d00f0`), and opts OUT of subinterpreters. scipy follows the same
  pattern. <https://raw.githubusercontent.com/numpy/numpy/main/numpy/_core/src/multiarray/multiarraymodule.c>
- **Status**: 3.13 free-threading experimental; 3.14 officially *supported*
  (PEP 779 Phase II), single-threaded overhead documented at **5–10%**
  (3.14 whatsnew). Free-threaded builds do **not** support the Limited
  API/stable ABI; wheels carry the `t` ABI tag.
  <https://peps.python.org/pep-0779/>, <https://docs.python.org/3.14/whatsnew/3.14.html>
- **WASM**: no threads in stable CPython WASM targets (`wasm32-wasi` lacks
  threads entirely); free-threading is a native-only concern.
  <https://github.com/python/cpython/issues/90473>

---

## 2. Audit — where molt's refcount + boundary state actually lives

### 2.1 Runtime heap refcount (molt objects) — mode-selected and ABI-bound

- `runtime/molt-codegen-abi/src/lib.rs` owns `MoltRefCount` and `MoltFlags`
  through one generated implementation macro with separate compile-time
  policies. Native flags remain atomic; refcounts are cell-backed under the
  deterministic GIL and atomic only under explicit native `free-threaded`;
  wasm32 uses cells for both. Both remain four-byte, layout-stable words and
  the mode/version-specific link witness rejects mixed backend/runtime objects.
- retain: one checked compare-exchange transition rejects zero resurrection,
  terminal/deallocating state, immortality-sentinel corruption, and overflow.
- release: one Release decrement performs an Acquire fence only on the 1→0
  edge before finalization/deallocation — the canonical `Arc` discipline; the
  typed revival window admits only the zero-owner or one-stable-view baseline.
- **Immortality for heap objects already exists**: `HEADER_FLAG_IMMORTAL`
  checked before any ordinary RC write. Value-immortality
  (None/bool/small ints) is free: they are NaN-boxed inline values with no
  heap cell at all.
- Verdict: under N-thread no-GIL, the atomic counter itself does not corrupt.
  What breaks without the GIL is **everything around it** — the objects'
  interior mutability (list/dict/str storage), which the molt GIL serializes
  today. The refcount is not the gating problem; object-state custody is.

### 2.2 molt's own GIL — real, reentrant, single-thread-optimized

- `runtime/molt-runtime/src/concurrency/gil.rs:177` — one `'static`
  `PREINIT_GIL: Mutex<()>` for the whole process (deliberately, after a Miri
  data-race find with split mutexes).
- Single-thread fast path: `GIL_THREAD_COUNT <= 1 && GIL_DEPTH > 0` ⇒
  re-entrant acquisition is a **pure no-op** (no TLS writes, no atomics
  beyond one relaxed load) — `gil.rs:247-260`. This is molt's analogue of
  CPython's "the GIL costs almost nothing single-threaded".
- wasm32: all GIL types are zero-cost no-op structs (`gil.rs:75-154`).
- Every FFI entry (`with_gil_entry!`/`with_gil_entry_nopanic!`) acquires it;
  `gil_assert()` guards runtime mutation (`debug_assert` unless
  `molt_debug_gil`).
- The runtime can hold the GIL across calls (`hold_runtime_gil`,
  `state/runtime_state.rs:786,839`) — i.e. in single-threaded programs the
  main thread effectively owns the GIL for the process lifetime and re-enters
  via the no-op lane.

### 2.3 The bridge (`molt-cpython-abi`) — GIL-ful CPython ABI, sharded identity custody

- `runtime/molt-cpython-abi/src/bridge.rs` — `GLOBAL_BRIDGE:
  Lazy<ObjectBridge>` shards `*mut PyObject ↔ handle` crossings by the CLASS2
  identity/value boundary. Address stripes own `from_py` + `foreign`; handle
  stripes own `to_py` + `raw_py`; `next_raw_handle` is atomic on native and a
  zero-atomic `Cell` on wasm32. The invariants remain (a) bidirectional
  identity consistency (`from_py[addr] == bits ⇔ to_py[bits].addr == addr`),
  (b) exactly-one `BridgeEntry` (allocation) per live proxied handle, (c)
  foreign-wrapper identity + one strong C-ref per `foreign` entry.
- Proxy refcounts are **plain non-atomic RMWs on `ob_refcnt`**: inside the
  lock in `handle_to_pyobj` (`bridge.rs:331-334,351-353`) and *outside* any
  lock in `Py_INCREF`/`Py_DECREF` (`api/refcount.rs:28-30,45-50`). Two
  unsynchronized-with-each-other paths mutate the same field; today both are
  serialized by the molt GIL contract (extension code runs under a
  GIL-holding runtime entry) — exactly the CPython GIL-build contract.
- Immortals are never written on any path (`is_immortal_refcnt` checked
  before every RMW), so the `static mut` singletons (`Py_None`, `PyExc_*`,
  `Py*_Type`, `abi_types.rs:877+`) are **already free-threading-safe on the
  refcount axis** — reads only. (Their *type-slot* fields are init-once
  under `Once`.)
- `IMMORTAL_REFCNT` (`abi_types.rs:793-812`) version-gates the CPython
  encoding off `TARGET_PY_MINOR` — 3.12/3.13 `UINT_MAX` vs 3.14 `3<<30`,
  with the 64-bit low-word-bit-31 predicate — matching the primary sources
  above. Compile-time asserts + anti-duplication tests already gate it.

### 2.4 Thread-state / GIL C-API surface — the honest gap list

- `PyGILState_Ensure` → returns 0; `PyGILState_Release` → no-op;
  `PyGILState_Check` → 1 (`api/object.rs:3418-3428`).
- `PyEval_SaveThread` → returns `&MOLT_THREAD_STATE` without releasing
  anything; `PyEval_RestoreThread` → no-op (`api/object.rs:3459-3464`).
- ONE process-global `static mut MOLT_THREAD_STATE` (`api/object.rs:3394`)
  returned to every thread; `PyThreadState_GetID` → constant 1.
- Per-thread state that IS correct: `PyThreadState_GetDict` is a real
  `thread_local!` dict (`api/sys.rs:30-49`); pending-exception state is
  thread-local; `PyThread_tss_*` is a real TLS implementation
  (`api/thread.rs`).
- **Latent bug class (exists TODAY, no free-threading needed):** a C
  extension worker thread that calls `PyGILState_Ensure()` before touching
  Python objects — the documented CPython contract — gets a success token
  and proceeds **unserialized** against runtime code holding the molt GIL:
  a data race on non-atomic proxy `ob_refcnt` and on object storage.
  Conversely `Py_BEGIN_ALLOW_THREADS` does not release the molt GIL, so a
  blocking BLAS/IO region in an extension holds the runtime hostage —
  inverted CPython semantics (starvation/deadlock exposure for threaded
  molt programs). Why this is NOT a Phase-2 wiring: see §5.3.

### 2.5 `Py_mod_gil` / module declarations — was discarded, now recorded

- Pre-change: `modules.rs` slot loops matched `PY_MOD_MULTIPLE_INTERPRETERS |
  PY_MOD_GIL => {}` (two sites) and `PyUnstable_Module_SetGIL` was a stub.
  The one signal a future free-threaded molt needs — *which extensions are
  audited for no-GIL* — was thrown away. Fixed in Phase 2 (§4.1).
- molt's overlay header pins `Py_GIL_DISABLED 0`
  (`include/molt/Python.h:561`) — correct: extensions built against molt's
  ABI get the GIL-build layout and never call `PyUnstable_Module_SetGIL`
  (conformant callers guard with `#ifdef Py_GIL_DISABLED`). The slot path is
  unguarded (numpy declares it on any 3.13+ target) and is the real
  recording surface.

---

## 3. Design principles (the CPython-parity cost model)

1. **Never naive atomics on the default hot path.** CPython free-threading pays
   5–10% single-threaded (3.14, documented) *with* biased RC +
   immortalization + deferred RC. A design that puts `lock xadd` on every
   inc/dec is strictly worse than the reference design and violates the
   charter. molt's deterministic GIL and wasm paths now use plain cell
   updates; explicit free-threaded mode keeps the atomic protocol. The future
   biased design (§5.1) removes cross-thread cost where ownership allows if
   measured shared-object workloads justify the extra header/state machine.
2. **Capability tokens already model custody.** `PyToken<'gil>` /
   `CoreGilToken` prove GIL possession in signatures. Free-threading refines
   what the token *means* (from "the one global lock" to "the right to
   mutate this object/shard") without changing the call graph. This is the
   same lattice CLASS2's `BridgeIdentity`/`MoltValueHandle` newtypes build at
   the bridge: encode the invariant in types, not in comments.
3. **Fail loud at the ABI boundary, honest-early (M08/M34).** A
   free-threaded-ABI extension binary must be rejected at load with a real
   diagnostic, not limp into layout UB. An undeclared-GIL extension under a
   hypothetical free-threaded molt must force GIL-equivalent serialization
   (CPython parity), loudly attested.
4. **wasm pays nothing.** Every mechanism below must keep compiling to the
   `Cell`/no-op forms on `target_arch = "wasm32"`, and the future
   wasm-threads work item is explicitly out of scope until wasm threads are
   a real molt target.

---

## 4. Phase 2 — landed now (non-conflicting, zero bridge.rs edits)

### 4.1 `Py_mod_gil` declaration registry (landed 21046db556)

- New `runtime/molt-cpython-abi/src/gil_declarations.rs`:
  `ModuleGilDeclaration { GilUsedDefault, GilUsedExplicit, GilNotUsed }`,
  keyed by module name; explicit never downgraded by a later default pass;
  unattributable declarations counted (`unresolved_gil_declaration_count`),
  never dropped. Queries: `module_gil_declaration(name)`,
  `modules_requiring_gil()` — the honest input for any future
  "can this program run free-threaded?" gate and for support-matrix claims.
- Recording at every module-definition entry point:
  `module_from_def_and_slots` (multi-phase, before creation — CPython stamps
  `md_gil` at the same point), `PyModule_ExecDef` (two-step loader path),
  `PyModule_Create2` (single-phase default), `PyUnstable_Module_SetGIL`.
- `PyUnstable_Module_SetGIL` prototype corrected to CPython's
  (`void *gil`); keeps the historical always-0 return (a recorder must not
  add a failure path to extension init; enforcement is the free-threaded
  runtime's job, deviation documented in-code).
- Mask-proof: `tests/test_module_gil_declarations.rs` (5 tests: numpy-shaped
  NOT_USED slot, explicit USED, absent-slot default, ExecDef path,
  unresolved counting) — verified FAIL 5/5 with recording neutered
  (pre-change discard semantics), PASS 5/5 post. Hooks-free by design so
  recording provably does not depend on module-creation success.
- Cost: one map insert per module definition at import time. O(1) amortized,
  zero steady-state, zero per-object cost. No hot path touched — no perf
  delta to attest beyond structural reasoning (no refcount/dispatch/lock
  code changed; the only mutated functions run once per extension import).

### 4.2 Cross-header constants already drift-gated

`tools/check_table_drift.py` `_PYMOD_SLOT_IDS`/`_PYMOD_SLOT_VALUES` already
bind `Py_mod_gil == 4`, `Py_MOD_GIL_USED == ((void*)0)`,
`Py_MOD_GIL_NOT_USED == ((void*)1)` across both header homes vs CPython 3.12+
(PASS verified post-change). The Rust-side
`PY_MOD_GIL_NOT_USED_VALUE: usize = 1` cites that binding.

### 4.3 What Phase 2 deliberately does NOT do

- No atomics added to `Py_INCREF`/`Py_DECREF` (CPython GIL-build parity;
  making them atomic without the custody redesign would cost the
  single-thread path and fix nothing — the race root is §2.4, not the RMW).
- No `PyGILState`/`PyEval_SaveThread` wiring (§5.3 — deadlock risk without
  the custody analysis; it is Phase-3 work with a spec below).
- No overlay-header (`include/molt/Python.h`) edits — that file is owned by
  the M56 header-unification arc and has no conformant SetGIL caller
  (`Py_GIL_DISABLED` pinned 0).

### 4.4 Follow-on gate (small, land-anytime): reject `t`-ABI extensions

The loader should fail closed if an extension binary was built for the
free-threaded ABI (different `PyObject` layout ⇒ instant UB). Signals: wheel
ABI tag `cp3XXt` at the packaging layer. This is a
loader/packaging change (no bridge.rs), listed as the first Phase-3 sub-item
so it rides the same lane.

---

## 5. Phase 3 — the specced follow-on (blocked on CLASS2 newtypes)

### 5.1 Refcount model: keep atomics, adopt biased RC only if measured

Decision (principal-engineer call): molt does **not** copy CPython's
split-field biased RC into `MoltHeader` now. Reasons:
- deterministic GIL mode now uses a plain cell, while free-threaded mode keeps
  the Release/Acquire-correct atomic path. The field remains four bytes and
  link-witnessed across independently cached backend/runtime artifacts.
- BRC's real win appears only under heavy cross-thread sharing; molt's
  free-threaded milestone 1 (shard the GIL) can land without any header
  change (`MoltRefCount` is `#[repr(transparent)]` — layout-stable).
- IF profiling under real N-thread workloads shows the shared `lock xadd`
  as the bottleneck, the BRC upgrade is: widen `MoltHeader` with
  `owner_tid: usize` + reuse the existing `ref_count: u32` as the local
  count + add `shared: AtomicI32` with the 2-bit state machine
  (default/queued/merged) exactly as `refcount.h` — behind
  `cfg(not(target_arch = "wasm32"))`. The immortal flag stays a header
  flag (already branch-first on both paths). This is an additive,
  measured-only step (M10: profile first, attest delta).

### 5.2 The bridge: shard on the CLASS2 newtype boundary — DONE (milestone 1a)

Once `BridgeIdentity` (identity-only) vs `MoltValueHandle` (decodable) land
in bridge.rs, the map custody splits cleanly:
- **Shard `from_py`/`raw_py`/`foreign` by pointer address** (the key is an
  address): `N = next_pow2(2×cores)` stripes, stripe =
  `parking_lot::Mutex<HashMap<..>>`, index = `(addr >> 4) & (N-1)`
  (BridgeHeaders are 8/16-aligned; low bits carry no entropy). Identity
  lookups (`pyobj_to_handle`, the hottest crossing) touch exactly one
  stripe.
- **Shard `to_py` by handle bits** with the same stripe count, index off the
  NaN-box payload bits. The bidirectional invariant (§2.3a) becomes a
  two-stripe protocol; lock ordering = (from_py-stripe, then to_py-stripe)
  by fixed rank to stay deadlock-free; `release_pyobj` (the only
  two-map-write path besides insert) takes both in rank order.
- `next_raw_handle` → `AtomicU64::fetch_add(0x10)` (already collision-checked
  by the insert loop).
- Read-mostly future: identity lookups can move to lock-free reads
  (`dashmap`-style or leapfrog) later; the stripe design is the
  measured-first step that preserves every invariant CLASS2's types encode.
- wasm32: `N = 1` (compile-time), stripes collapse to today's single map —
  zero added cost.

Landed implementation result (2026-07-11): native `N = next_pow2(2×cores)`;
address index `(addr >> 4) & (N-1)`; handle index `(bits >> 4) & (N-1)`.
Every two-map mutation uses the single `lock_address_then_handle` authority,
so rank is globally fixed as address stripe before handle stripe. The release
profile's identical pre/post executable measured the former global mutex at
18.72 ns/crossing and the sharded path at 13.29 ns/crossing (`-29.01%`), with
4-thread throughput 291.39 Mops/s and 8-thread throughput 524.23 Mops/s versus
the baseline's 8.38 and 4.27 Mops/s respectively. The ignored release-profile
test machine-checks `Δ≤0` against that captured baseline and requires 4-thread
throughput to reach at least 1.5× the single-thread rate. Crossed-index and
concurrent crossing/release watchdog tests cover deadlock and invariant drift.

### 5.3 GIL custody: make `PyGILState_*`/`PyEval_SaveThread` real — carefully

**DONE 2026-07-11 (Phase-3 milestone 1b).** `RuntimeHooks` now owns
`gil_ensure`/`gil_leave`/`gil_release`/`gil_restore`/`gil_check`; the runtime
stores recursive Ensure guards and SaveThread release guards on per-thread
stacks. Exact current-thread ownership is separate from the optimized logical
single-thread held lane. When a second GIL-capable thread appears, the existing
`GilGuard::new()` multi-thread branch converts the permanent runtime hold into
scoped call custody and restores it on drop; the single-thread branch is
unchanged and wasm remains a compile-time no-op.

Proof: 8 workers ? 2,000 recursive acquisitions serialize a deliberately
non-atomic foreign-proxy mutation (`max_active == 1`, 16,000/16,000 updates)
while the main thread repeatedly performs SaveThread/RestoreThread under a
20-second watchdog. The release microbench measured 53.526 ns baseline versus
53.496 ns custody (`-0.057%`).

The correct CPython-parity wiring, and why it must not be landed blind:
- `PyEval_SaveThread` ⇒ `GilReleaseGuard::new()` pushed on a TLS stack
  (runtime side, exposed to the ABI crate through **new `RuntimeHooks`
  entries** — `gil_release`, `gil_restore`, `gil_ensure`, `gil_leave` — the
  ABI crate cannot depend on molt-runtime, hooks are the canonical channel);
  `PyEval_RestoreThread` pops. Nested pairs = stack depth.
- `PyGILState_Ensure` ⇒ acquire (`GilGuard::new()`), store guard in a TLS
  vec, return its depth; `Release` pops. `PyGILState_Check` reads
  `gil_held()`.
- **The hazard that blocks blind wiring:** in single-threaded molt programs
  the main thread *permanently holds* the GIL (`hold_runtime_gil`,
  `runtime_state.rs:786,839`) and re-enters via the no-op lane. A worker
  thread calling a newly-real `PyGILState_Ensure` would block **forever** —
  today's silent race becomes a hard deadlock. Prerequisite: the runtime
  must release the held GIL around foreign blocking regions, i.e. the
  custody redesign — either (a) drop the permanent hold and pay the
  depth-0 mutex on every FFI entry (measure!), or (b) keep the hold but add
  preemption points (release/reacquire around extension calls, which is also
  what `Py_BEGIN_ALLOW_THREADS` parity needs anyway). (b) is the
  CPython-shaped answer and the recommended one; it needs the extension-call
  sites enumerated (they are the `foreign_*`/`PyObject_Call` bridge paths —
  same files Phase 3 already touches).
- Gate: a threaded stress test (N workers × PyGILState_Ensure + proxy
  inc/dec + container mutation) run under TSan/Miri-where-possible, plus a
  no-deadlock watchdog test for the ALLOW_THREADS round-trip.

### 5.4 Free-threaded ABI variant (far future, explicitly out of scope now)

Hosting `cp3XXt` extensions requires a second `PyObject` layout
(`ob_tid/ob_ref_local/ob_ref_shared`), split-refcount `Py_INCREF` exports,
`_Py_MergeZeroLocalRefcount`, per-object `PyMutex`, critical-section API —
a parallel ABI surface selected at load time. Do not start until a real
demand signal exists; the §4.4 fail-closed gate keeps us honest meanwhile.

### 5.5 Phase-3 execution order

1. §4.4 loader fail-closed gate (independent, small).
2. **DONE 2026-07-11:** rebase onto CLASS2 newtypes → §5.2 sharded bridge (perf-gated:
   single-thread crossing micro-bench before/after must be Δ≤0; N-thread
   crossing bench must scale).
3. **DONE 2026-07-11:** ?5.3 GIL hooks + custody redesign (stress + watchdog +
   single-thread performance gate).
4. §5.1 biased-RC only if the post-§5.2/§5.3 profile names the shared RMW.

Dependency note: if CLASS2 lands newtypes with different names/shape, the
stripe keys in §5.2 keep their semantics (identity-keyed by address,
value-keyed by bits) — the design binds to the *distinction*, not the names.

---

## 6. Verification ledger (Phase 2)

Phase-3 milestone 1b receipts:
- watchdog/race/recursive proof: `20260711T092035-gil-custody-targeted-tests-compile2-aaf1d6254097405e`;
- single-thread gate: `20260711T102710-gil-custody-single-thread-bench-inlined-b539d2a7ec3d41a5`
  (`53.526 ns` baseline, `53.496 ns` custody, `-0.057%`);
- full ABI suite: `20260711T103142-gil-custody-full-abi-02afd750b1844fe9`;
- touched clippy `-D warnings`, ABI wasm32, fail-closed, table-drift,
  wasm-ABI generation, and rustfmt gates pass.

- `cargo test -p molt-lang-cpython-abi --no-fail-fast` — all green (all
  binaries; includes 5 new integration tests + 5 new unit tests).
- Mask-proof: recording neutered ⇒ 5/5 FAIL; restored ⇒ 5/5 PASS.
- `cargo clippy -p molt-lang-cpython-abi --all-targets -- -D warnings` —
  clean (also fixed pre-existing clippy-1.96 `manual_contains` in
  `test_dict_cursor.rs`).
- `cargo check --target wasm32-wasip1 -p molt-lang-cpython-abi` — OK.
- `tools/fail_closed_gate.py` OK; `tools/check_table_drift.py --check` PASS
  (incl. PyModuleDef_Slot token binding across both headers);
  `tools/gen_wasm_abi.py --check` in sync (no ABI-surface change: no new
  externs; SetGIL's wasm signature is unchanged — `*mut c_void` and `c_int`
  are both i32 on wasm32, and natives pass both in registers).
- rustfmt clean on touched files.

## 7. Operator-owned calls surfaced (not decided here)

1. **Public support-matrix claim**: when/whether molt advertises
   "free-threading-ready boundary" externally. The registry + this doc make
   the claim *checkable*; making it is a product decision.
2. **Phase-3 milestone 1 scheduling** (sharded bridge + GIL custody): it is
   a multi-session arc touching the hottest paths; worth an explicit lane
   assignment after CLASS2 lands rather than opportunistic pickup.
