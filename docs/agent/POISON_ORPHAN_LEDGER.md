# Poison-Orphan Ledger — runtime / cpython-abi / registry surface

> **Scope.** An adversarial audit of *poison-orphans* across the Molt runtime,
> the CPython extension-ABI tier, the include/ header overlays, the wasm/backend
> CLI, and the `tools/fail_closed_registry.toml` ratchet. A **poison-orphan** is
> code that is either (a) **silently wrong** on a live path (FAIL_OPEN /
> HIDDEN_THEATER — returns a plausible-but-wrong value with no error), (b) a
> **half-wired mechanism** (HALF_WIRED — a dead producer/consumer that leaves a
> live reader unreachable or a live caller mis-served), (c) a **capability gap**
> masquerading as complete (FUTURE_WIP / UNIMPLEMENTED_CAPABILITY), (d) a **live
> registry-tracked debt** (LIVE_REGISTRY_POISON), or (e) genuinely **dead code**
> hiding behind `#[allow(dead_code)]` (DEAD_DELETE). LEGIT entries are surfaces
> that were audited and cleared — recorded so the negative result is not re-run.
>
> **Anchor.** Line numbers are as of `origin/main` **`e735c50c09`** (2026-07-10).
> They are anchors, not identities — re-anchor by grepping the symbol name when a
> line drifts. Every finding below was verified against the Rust/C/Python source
> in `C:/Molt/molt-src`; a representative high-severity sample was independently
> re-read in-tree for this ledger (see **Verification** below).
>
> **Doctrine.** This ledger operationalizes M05 (zero fakes), M34 (perf/capability
> paths silently degrade to naive — fix the class: full-capability default OR
> degrade LOUDLY + gate), and M32 (the fail-closed ratchet). A poison-orphan is
> the M34 failure one level up: *landed-but-not-effective*.

> **Task #60 refresh (2026-07-11, `e1baed8d8e`).** Re-verified every row against
> current `origin/main`. Lane A was already closed by `d60eea673e`, `efbf8b99a8`,
> `ceab1628f8`, `f2a57f3a88`, and `1873baf464`/`5a11008e27`; Lane E was already
> closed by `f4a8d8b2dd`. This refresh closes rows #7, #8, #10, #11, and #12 by
> making the live paths fail closed, and closes #15 by moving the valuable
> experiment outside the shipping runtime. It also lands the CLASS3-DISPATCH
> completeness gate. Rows #5, #6, #13, and #14 remain live as separately owned
> structural migrations (import-bedrock PR2/PR4, task#10 generated header
> authority, task#73.2 generic source-plan custody); no shim was added here.

### Task #60 row status

| Row | Live verdict | Decision | Fix / remaining owner | Mask-proof |
| ---: | --- | --- | --- | --- |
| 1-4, 9 | already fixed | implement / fail closed | Lane A SHAs above | dedicated parity and C-API tests in Lane A |
| 5 | still live | wire, not shim | import-bedrock PR2 owner | existing transition tests are test-only until cutover |
| 6 | still live | generate one authority | task#10 owner | header drift gates remain green |
| 7 | **closed `e1baed8d8e`** | rip stale state | clear diagnostic at both module-exec entries | stale record is absent after exec-boundary clear |
| 8 | **closed `e1baed8d8e`** | implement honest errors | audited dict/module/type sentinel paths set exceptions | NULL dict arguments now assert pending error |
| 10 | **closed `e1baed8d8e`** | fail loud | missing `wasm-tools` disables artifact reuse | validator-absent pytest requires error text |
| 11 | **closed `e1baed8d8e`** | implement honest error | host send failures raise `OSError` | `ECONNRESET` classifies as error, never close |
| 12 | **closed `e1baed8d8e`** | rip duplicate | source header only includes owning `Python.h` | table-drift and header compile gates pass |
| 13 | still live | delete after generic custody | task#73.2 owner | fail-closed registry remains pinned |
| 14 | still live | implement real snapshot reinit | import-bedrock PR4 owner | current behavior remains fail closed |
| 15 | **closed-moved `e1baed8d8e`** | move | `demos/experiments/runtime_string_repr.rs` | live crate no longer compiles the scaffold |
| 16, 23-31 | LEGIT | none | audit verdict unchanged | existing gates/tests |
| 17-22, 24 | already fixed | delete / cfg(test) | `f4a8d8b2dd` | Lane E crate tests and dead-code ratchet |

---

## 1. Executive summary

**32 audited findings across 31 distinct locations** (`PyModule_GetName` at
`modules.rs:118` was surfaced independently by two audit passes). Of these, **13
carry a real defect** requiring action; **9 are LEGIT** (audited-and-clean,
no action); the remainder are dead-code hygiene or tracked capability gaps.

### By classification

| Classification | Count | What it means | Danger |
| --- | ---: | --- | --- |
| **FAIL_OPEN** | 4 | Correctness path silently degrades to a wrong-but-valid value on a live path | **Highest — silent wrong answer** |
| **HIDDEN_THEATER** | 3 (2 distinct) | A working-looking API returns a fabricated constant / synthetic object | **High — silent wrong answer** |
| **HALF_WIRED** | 3 | Dead producer/consumer leaves a live reader unreachable or a live caller mis-served | **High — latent wrong answer / degraded diagnostic** |
| **LIVE_REGISTRY_POISON** | 3 | Live debt correctly tracked in the fail-closed registry; drift/duplicate-authority risk | Med — latent, gated |
| **FUTURE_WIP** | 3 | Staged capability, fails-stale/closed today | Low — capability gap |
| **UNIMPLEMENTED_CAPABILITY** | 1 | Genuinely-absent capability, fails honestly | Low — capability gap |
| **DEAD_DELETE** | 6 | Truly-dead code behind `#[allow(dead_code)]` | Low — hygiene / masks future deadness |
| **LEGIT** | 9 | Audited and clean; recorded to avoid re-audit | None |

### By severity

| Severity | Count | Findings |
| --- | ---: | --- |
| **high** | 1 | `random.seed(str\|bytes)` non-CPython seed (FAIL_OPEN) |
| **med** | 5 | `add_methods_to_dict` fail-open (witness); `datetime.h` overlay stub; `PyModule_GetName` fabrication; `module_table` view-mutation half-wired; `Python.h` D1 declaration drift (witness) |
| **low** | 26 | remainder |

### Witness-relevance

**9 findings are witness-relevant** (on the numpy/scipy → WASM path): the
`add_methods_to_dict` method-drop, the `capi_trace` stale-diagnostic and the
`mapping.rs` record-without-exception class (all degrade the very diagnostics
built to pinpoint witness import failures), `PyModule_GetName`, the wasm-tools
validation fail-open, the (LEGIT-cleared) array-buffer lease, and the three
registry rows `Python.h` D1 / `structmember.h` D2 / numpy-multiarray B1. Note
that **none of the witness-relevant defects is a hard witness *blocker*** — they
are latent-wrong or diagnostic-degrading, which is worse in one specific way:
they surface as a mis-attributed `AttributeError`/`SystemError` *much later*
during numpy compute, with no exec-time failure and no gate.

### The single most important structural takeaway

The correctness poison clusters in **two mechanisms that were built to be
honest and then left half-honest**: (1) the `record_silent_failure` /
`capi_trace` diagnostic surface — *effective* (its reader is live, not theater),
but with one fail-open method-drop, one stale-slot bug, and a class of
record-without-exception gaps that reproduce the opaque failure the system
exists to eliminate; and (2) the `include/` header overlays — where the
duplicate-authority registry scan *line-counts twins* and cannot see that one
twin is a **fail-open stub** (`datetime.h`) while another is memory-safe by
generation (`Python.h`). Fix the mechanism, not the instances.

---

## 2. Ranked findings — most-dangerous first

Silently-wrong and latent-bug classes (FAIL_OPEN / HIDDEN_THEATER / HALF_WIRED /
the silently-wrong registry row) are at the top; capability gaps, then dead-code
hygiene, then audited-clean LEGIT entries at the bottom. **W** = witness-relevant.

| # | Location | Class | Sev | W | Action | Defect (one line) |
| --: | --- | --- | --- | :-: | --- | --- |
| 1 | `runtime/molt-runtime-math/src/random_mod.rs:508-520` + `Cargo.toml:26` | FAIL_OPEN | **high** | | rip-and-fix | `random.seed(str\|bytes\|bytearray)` produces a **non-CPython MT seed**: the SHA-512 correct path is `#[cfg(feature="crypto")]`, `crypto` is `default=[]` and enabled by **no** crate in the graph, so the digest-less fallback always compiles → deterministic-but-wrong stream, no error. **CONFIRMED** end-to-end. |
| 2 | `runtime/molt-cpython-abi/src/api/typeobj.rs:274-281` (`add_methods_to_dict`) | FAIL_OPEN | med | ✓ | rip-and-fix | `PyType_Ready` records a `PyDict_SetItemString` failure but **does not abort** — returns READY(0) with the method silently missing from `tp_dict`. Inverse of the sibling `add_members_/add_getset_` paths (which return -1). A numpy scalar/DType type can be marked ready while missing methods → later `AttributeError`/wrong dispatch. |
| 3 | `include/datetime.h:20-44` (registry row `D2_datetime`) | FAIL_OPEN | med | | rip-and-fix | Overlay-tier copy is a **silently-wrong fail-open stub**: `PyDateTime_CAPI` is a 4-byte placeholder, `PyDate_/PyDateTime_/PyDelta_Check` unconditionally `return 0`, `PyTime_Check` missing. Wrong branch taken silently; `PyDateTimeAPI` reads past the 4-byte struct (**OOB → memory unsafety**). The registry row falsely calls it a "hand-synced" benign duplicate of D1. Gated to the source-compat tier. |
| 4 | `runtime/molt-cpython-abi/src/api/modules.rs:118` (`PyModule_GetName`) | HIDDEN_THEATER | med | ✓ | rip-and-fix | Returns the **hardcoded constant** `c"molt.module"` for *every* non-null module instead of `__name__`; no `record_silent_failure`, no marker. `PyImport_AddModule(PyModule_GetName(m))` would collide every module under one key. The real name is reachable in-process. *(Surfaced by two audit passes: low + med; taking med.)* |
| 5 | `runtime/molt-runtime/src/builtins/module_table.rs:441-475` + `369-378` | HALF_WIRED | med | | wire | `module_table_view_replace`/`_tombstone` (the §4.4 `sys.modules` view-mutation entry points) have **zero production callers** — only `#[cfg(test)]` drives them, so `STATE_REPLACED`/`STATE_TOMBSTONE` are never set in prod. Consequence: `del sys.modules['foo']` then `import foo` returns the **stale cached module** instead of re-executing (CPython re-executes). Tracked as PR2 cutover, blocked on R1 native call lane. |
| 6 | `include/molt/Python.h:1` vs `runtime/molt-cpython-abi/include/Python.h:1758` (`D1_python_h`) | LIVE_REGISTRY_POISON | med | ✓ | implement | Two substantive headers (471KB source-recompile tier vs 82KB ABI tier). The **memory-unsafe half is closed** (layout generated from one `abi_types.rs` authority; `--check` green; `_Static_assert` on every build). Residual **live debt** = the hand-maintained *declaration surface* (macros/decls/inline helpers) in two copies → can silently break source-recompiled extension builds (numpy/scipy `#include <Python.h>`). Tracked task#10. |
| 7 | `runtime/molt-cpython-abi/src/capi_trace.rs:59` + `api/modules.rs:43` | HALF_WIRED | low | ✓ | rip-and-fix | `record_silent_failure` is a **last-write-wins thread-local with no exec-scoped reset**. When a benign recorded failure is later raised-then-cleared by the extension, `PyErr_Occurred()` is null again and the live reader returns the **stale benign site**, mis-attributing the real exec failure. Degrades the precision of the diagnostic built to pinpoint witness import failures. |
| 8 | `runtime/molt-cpython-abi/src/api/mapping.rs:76,135` (+ `modules.rs:140`, `typeobj.rs:58,424,439`) | HALF_WIRED | low | ✓ | rip-and-fix | Record a silent failure and return the sentinel (-1/NULL) **without setting a Python exception** — the exact "Molt-owned bug to close" the module doc names. Mitigated during module-exec by the live reader, but a non-exec caller checking `<0` finds no pending exception → reproduces the opaque failure this system exists to eliminate. numpy registers descriptor maps via `PyDict_SetItem`. |
| 9 | `runtime/molt-cpython-abi/src/api/object.rs:865` (`PyThreadState_GetFrame`) | HIDDEN_THEATER | low | | implement | Fabricates a fresh empty `PyFrameObject` (NULL code/globals/locals, `f_lineno=0`) for any non-null tstate; `PyFrame_GetCode` synthesizes an empty code object. A C extension walking the frame reads fabricated zeros as the real execution frame. Unmarked; fails OPEN with a synthetic object (contrast weakref which fails closed). `PyTraceBack_Here` is the benign no-op sibling. |
| 10 | `src/molt/cli/runtime_wasm_validation.py:97-122` | FAIL_OPEN | low | ✓ | implement | `_validate_wasm_structural()` returns `None` (= "no structural error = valid") when `wasm-tools` is **absent from PATH**; `_is_reusable_wasm_artifact` treats that as reusable. On a box without wasm-tools, the DEEP structural gate is **silently skipped** (no warning, contra M34). The shallow parser checks still fail-closed, but the deep gate degrades silently vs the sibling validators which fail closed. |
| 11 | `runtime/molt-runtime/src/async_rt/channels/websocket.rs:247-254` | FUTURE_WIP | low | | implement | wasm32 WS send host-hook maps **any** non-EWOULDBLOCK/EAGAIN send error to `MoltObject::none()` ("closed socket") — `// Treat send errors as a closed socket for now.` A transient/protocol error is reported as a normal close, masking the failure. wasm32-only, off the witness path. |
| 12 | `include/structmember.h:12-37` vs `runtime/molt-cpython-abi/include/structmember.h:30-55` (`D2_structmember`) | LIVE_REGISTRY_POISON | low | ✓ | rip-and-fix | **Accurately** described byte-identical hand-synced alias table (`T_*`→`Py_T_*`). No wrong answer today; pure drift-risk (edit one, forget the other). numpy `_core` includes it via the cpython-abi tier → drift would be witness-relevant. Burn-down: collapse to a thin `<20-line` forwarder. |
| 13 | `tools/regen_numpy_multiarray_meson_wasm.py:1` (`B1_numpy_multiarray`) | LIVE_REGISTRY_POISON | low | ✓ | implement | numpy-specific Meson/ninja regen helper for one extension. Honestly delegates to numpy's own vendored Meson (does not bake build semantics into Molt), so a build-lane crutch, not correctness poison. Burn-down = generic auto-provision source-plan authority (task#73.2), then DELETE. |
| 14 | `runtime/molt-runtime/src/builtins/module_table.rs:79-80` + `98-99` (`MODULE_FLAG_REINIT_RESURRECT`) | FUTURE_WIP | low | | implement | Flag parsed+validated at install but **never read** by any production decision; the `STATE_TOMBSTONE`+`MODULE_KIND_EXTENSION` arm fails closed for all extension modules regardless. Fail-closed and benign — a capability gap (a resurrect-able module still can't re-import after `del sys.modules[...]`). Tracked PR4. |
| 15 | `runtime/molt-runtime/src/object/mod.rs:67-68` → `object/string_repr.rs` | FUTURE_WIP | low | | delete | "Project TITAN Phase 0" multi-representation string module (`classify_string`, `StringReprKind`) has **no production consumer**; live string storage uses the flat UTF-8 layout. Consistent with M48 (borrowed-view string Repr not wired). Inert scaffolding, module-level `#[allow(dead_code)]` masks it. Delete-or-finish. |
| 16 | `runtime/molt-cpython-abi/src/api/object.rs:733` (`PyObject_ClearWeakRefs`) | UNIMPLEMENTED_CAPABILITY | low | | none (track) | C-level weakref support absent (`PyWeakref_Check` always 0). The record site is `!head.is_null()`-guarded and unreachable today (list head always NULL) → unreachable-defensive, fails honestly (CPython-faithful no-op). numpy import doesn't require C-level weakref creation. Flagged so the gap is tracked, not silently assumed complete. |
| 17 | `runtime/molt-runtime/src/object/mod.rs:37-38` → `object/gil.rs` | DEAD_DELETE | low | | delete | "GIL removal infrastructure — Phase 1" (`ObjectLock`, `GIL_RELEASED`, `is_gil_released`) is fully orphaned (all 12 refs inside `gil.rs`). Its docstring claims it removes `gil_assert()` from hot refcount paths, but live `inc_ref_ptr`/`dec_ref_ptr` **still call `gil_assert()`** — purpose unrealized. **Loaded gun**: exposes `set_gil_released()`/`ObjectLock` but container ops never take the lock → a future lane flipping the flag opens a data race. The real wired module is `crate::concurrency::gil`. |
| 18 | `runtime/molt-worker/src/main.rs:237` (`CancelRegistry::take_cancelled`) | DEAD_DELETE | low | | delete | The **one genuinely-dead item in the whole `molt-worker` crate** (found via `--force-warn dead_code`), hidden by the impl block's blanket `#[allow(dead_code)]`. Zero callers; the live cancellation path uses `register()`/`mark_cancelled()`/`clear()`+`is_cancelled()`. Orphaned accessor, safe to remove. |
| 19 | `runtime/molt-passes/src/tir/passes/ownership_lattice_min.rs:556` + `723-726` | DEAD_DELETE | low | | delete | `statement_release_finalizer_roots` HashSet computed on every drop_insertion run but **no production reader** — the live consumer iterates the parallel `..._boundaries` Vec instead. All 7 getter readers are `#[cfg(test)]`. Redundant dead producer state (DecRef ordering is still correct via the regular zero-use dead-result path; only the test's assertion gives false comfort). |
| 20 | `runtime/molt-tir/src/representation_plan.rs:929,936,943,950` | DEAD_DELETE | low | | delete | Four scalar-carrier predicates gated `#[cfg_attr(not(feature="wasm-backend"),allow(dead_code))]` — declared live-under-wasm, but the wasm backend **never calls them** (it uses `op_direct_numeric_repr`, which subsumes these coarser name-keyed checks). Superseded dead predicates; callers only in `scalar_facts.rs` tests. Not a capability gap (the wasm fast lane is wired via the newer authority). |
| 21 | `runtime/molt-passes/src/tir/passes/ownership_lattice_min.rs:709-712` | DEAD_DELETE | low | | delete | `OwnershipLattice::non_owning_copy_result_roots()` whole-set getter has **zero** production and test callers; the live consumer reads the `OwnershipRootFacts` variant + the predicate. Truly unused. |
| 22 | `runtime/molt-passes/src/tir/passes/ownership_lattice_min.rs:338-341,699-702,714-717` | DEAD_DELETE | low | | deregister | Three whole-set/predicate getters read **only by unit tests**; production uses the live predicates. Test-scaffolding getters — cleaner as `#[cfg(test)]` than `#[allow(dead_code)]`. Harmless. |
| 23 | `runtime/molt-worker/src/db_protocol.rs:59-933` (28 `#[allow(dead_code)]`) | LEGIT | low | | none | All 28 items are **LIVE** production code — one coherent, fully-wired `db_query`/`db_exec` chain reachable from `fn main` under both sync and async runtimes (params actually BOUND at `main.rs:1246-1274`, not resolve-then-ignore). The attributes are **stale lint suppression**. Residual hygiene risk: the blanket allows will MASK any future genuine deadness in this file — tighten them (see Lane E). |
| 24 | `runtime/molt-passes/src/tir/passes/ownership_lattice_min.rs:563-566` (`compute()`) | LEGIT | low | | none | Test-only convenience constructor; production uses `compute_with_root_facts` to share the built `OwnershipRootFacts`. Legit scaffolding — cleaner as `#[cfg(test)]`; no poison. |
| 25 | `runtime/molt-backend/src/backend_request.rs` (whole file) | LEGIT | low | | none | Clean. Every dead-marked field is `#[cfg_attr(not(feature=...),allow(dead_code))]` and consumed under its owning backend feature; `read_cbor_ir_document` fails CLOSED without the cbor feature. Standard conditional-compilation hygiene. |
| 26 | `runtime/molt-backend/src/backend_process/protocol.rs` (whole file) | LEGIT | low | | none | Clean. Entire module is `#[cfg(any(unix,test))]` (unix-socket daemon; test-compiled for coverage; inert on Windows host). All DTO fields consumed by the live job runner (`backend_process/job.rs:150/159/191/219`). No parse-but-ignore field. |
| 27 | `runtime/molt-cpython-abi/src/api/object.rs:87` (~24 record-and-raise sites) | LEGIT | low | | none | The majority of `record_silent_failure` sites are **correct honest error paths** — record for diagnostics AND immediately set a CPython-shaped exception, then return the sentinel (abstract_number/object/typeobj/capsule, all fail-closed). LEGIT_ERROR_PATH, not poison. |
| 28 | `runtime/molt-runtime/src/c_api/molt_api.rs:370-434` (array-buffer lease) | LEGIT | low | ✓ | none | The array-buffer lease interlock (the class the task flagged) is now **fully fail-closed**: `export_buffer` returns -1 on every bad-precondition branch and releases+zeroes the view on a post-export invalid-view; `unregister_type` revokes both the type mapping and the exporter/releaser table. No dead-producer/live-reader gap remains. |
| 29 | `tools/fail_closed_registry.toml:201-234` (G1 luau / G2 rust) + `backend_output.rs` | LEGIT | low | | none | Both `fail_open_backend_dispatch` rows are **effective, not half-wired**: `emit_unsupported_op` pushes into `self.unsupported_ops` and the only production callers (`compile_via_ir`/`compile_checked`) reject a non-empty accumulator before writing source. The unchecked `.compile()` calls are wasm/native only. Registry correctly freezes the surface. |
| 30 | `runtime/molt-backend-luau/src/luau/op_emitter.rs:63-79` (`G1`) | LEGIT | low | | none | Placeholder-emit surface, VERIFIED genuinely fail-CLOSED. The `local x = nil` text is only a readable diagnostic; the fail-closed authority is the `unsupported_ops` accumulator, honored on the real build path (`backend_output.rs:163-168`). |
| 31 | `runtime/molt-backend-rust/src/rust/op_emitter.rs:29-40` (`G2`) | LEGIT | low | | none | Same shape as G1: `MoltValue::None` marker is diagnostic-only; the fail-closed decision is the accumulator, read by the shipping `rust_source_for_ir`→`compile_checked` path. Regression test asserts the accumulator path, not the text scan. |

*(31 rows; finding #4 consolidates the two independent `PyModule_GetName` reports.)*

---

## 3. Batch-fix plan — 5 coherent lanes

Ordered by danger. Each lane is a coherent cut a single owner can land end-to-end.

### Lane A — Silently-wrong live paths: rip-and-fix *(the correctness poison; do first)*

The FAIL_OPEN + HIDDEN_THEATER items that return a plausible-but-wrong value with
no error. These are the M05/M34 violations — a wrong answer with a green build.

- **#1 `random.seed(str|bytes)`** — make the SHA-512 seed path unconditional (it is
  small and pure-Rust; either move `sha2`/`digest` out of the `crypto` feature for
  the seed digest, or vendor a minimal SHA-512 into `molt-runtime-math` with no
  feature gate) so the fallback that omits the digest suffix can never compile.
  Add a parity test: `random.seed("x"); random.random()` must equal CPython 3.12.
- **#2 `add_methods_to_dict`** — return -1 on `PyDict_SetItemString` failure like
  its `add_members_/add_getset_` siblings, so `PyType_Ready` fails CLOSED instead
  of marking a type ready with a dropped method. (Root-cause the store failure too:
  `PyCFunction_NewEx` falling back to a non-bridge-registered raw `PyCFunctionObject`.)
- **#3 `datetime.h` overlay** — make the overlay-tier copy fail **closed**: `#error`
  or an undefined link symbol for the datetime C-API in the source-compat tier, OR
  implement real `PyDate_*/PyTime_*` checks via the NaN-box type registry. Then
  correct the `D2_datetime` registry row (Lane D) so no maintainer "syncs" a
  fail-open stub as an agreeing copy.
- **#4 `PyModule_GetName`** — read the real `__name__` from the module dict
  (`PyModule_GetDict` already routes there) and return it; keep the NULL→SystemError
  path. Implement `PyModule_GetNameObject` as the authority.
- **#9 `PyThreadState_GetFrame` / `PyFrame_GetCode`** — either return NULL (fail
  closed, CPython-legal) or mark the synthetic frame with `record_silent_failure`
  so introspection callers can tell it is a modeling stub, not the real frame.

### Lane B — Half-wired mechanism: wire the consumer / reset the slot

The HALF_WIRED items where a live reader is unreachable or a live caller is mis-served.

- **#5 `module_table` view-mutation** — promote `module_table_view_replace/_tombstone`
  from the PR1 test seam to production `del sys.modules[...]` / re-import, so
  `STATE_REPLACED`/`STATE_TOMBSTONE` are set in prod and re-import re-executes. This
  is the tracked **PR2 cutover** (`import_bedrock_pr2_sys_modules_view_cutover.md`), blocked on the R1
  native call lane — unblock or land the R1 dependency first.
- **#7 `capi_trace` stale slot** — reset `LAST_SILENT_FAILURE` at each module-exec
  entry (`modules.rs:389/455`) so the annotation reflects the *current* exec, not a
  stale benign site. Cheap, high-leverage for witness diagnostics.
- **#8 `mapping.rs`/`typeobj.rs` record-without-exception** — at the six sites that
  return the sentinel bare, set the honest CPython exception (`SystemError`/
  `MemoryError`) alongside the record, matching the sibling `PyDict_SetItem`
  unresolved-key branch that already does.

### Lane C — Capability gaps: implement, or degrade LOUDLY + gate *(M34)*

FUTURE_WIP / UNIMPLEMENTED / silent-degrade. Each must become full-capability OR
fail loud with a gate — never a silent skip.

- **#10 `runtime_wasm_validation`** — when `wasm-tools` is absent, do NOT return
  `None` (=valid). Either provision it (M15 auto-provision lane) or return a LOUD
  degraded verdict + warning + gate so a build box can't silently accept a
  deeply-malformed wasm for cache reuse.
- **#11 `string_repr` (Project TITAN)** — delete the inert module now (M48: borrowed-
  view string Repr is not the win) or finish-and-wire it; do not leave scaffolding
  behind a module-level `#[allow(dead_code)]`.
- **#14 `MODULE_FLAG_REINIT_RESURRECT`** — implement the tracked **PR4** extension-
  snapshot reinit lane so a resurrect-marked extension can re-import after
  `del sys.modules[...]`; until then it correctly fails closed.
- **#16 `PyObject_ClearWeakRefs`** — track the C-level weakref gap explicitly (it
  fails honestly today); implement when an extension needs C-level weakref creation.
- **#11(ws) `websocket.rs` send** — surface the real send error instead of masking
  it as a socket close; wasm32-only, low urgency, but a shipped "for now" divergence.

### Lane D — Registry burn-down + one reclassification

The LIVE_REGISTRY_POISON rows and the one misclassified row. Each burn-down retires
a registry row and lowers a baseline (M32 ratchet).

- **#6 `Python.h` D1** *(task#10)* — generate the declaration surface (macros / extern
  decls / inline helpers) from one shared symbol table, teach the D-scan to recognize
  a generated single-authority header, retire the row, lower the `duplicate_authority`
  baseline. (The memory-unsafe layout half is already single-authority + `_Static_assert`.)
- **#12 `structmember.h` D2** — collapse `include/structmember.h` to a `<20-line`
  forwarder that `#include`s the cpython-abi authority; it drops out of the scan and
  the row retires with a baseline decrement.
- **#13 numpy-multiarray B1** *(task#73.2)* — implement the generic package-build-system
  source-plan custody authority, then DELETE the numpy-specific lane and lower the
  `ecosystem_build_crutch` baseline to 0.
- **`D2_datetime` reclassification** *(ties to #3)* — the row's "hand-synced benign
  duplicate" justification is materially false; rewrite it to name the fail-open stub,
  and make the header fail closed (Lane A #3) rather than "sync" it.

### Lane E — Dead-code hygiene: delete + tighten the blanket allows

Pure DEAD_DELETE + the residual masking risk. Small, mechanical, no behavior change —
but it restores the dead_code signal the other lanes rely on.

- **Delete**: #17 `object/gil.rs` (loaded gun — orphaned lock infra), #18
  `take_cancelled`, #19 `statement_release_finalizer_roots` (field+getter), #20 the
  four `representation_plan` predicates, #21 `non_owning_copy_result_roots()` getter.
- **Deregister → `#[cfg(test)]`**: #22 the three test-only lattice getters, #24
  `OwnershipLattice::compute()`.
- **Tighten the blanket allows**: replace the crate-wide/impl-wide `#[allow(dead_code)]`
  in `db_protocol.rs` (#23) and `molt-worker/main.rs` with per-item allows (or remove
  them now that the chain is wired), so future genuine deadness in these files is no
  longer masked — this is the *landed-but-not-effective* risk one level up.

---

## 4. Orchestrator task-list tracking

| Finding | Already tracked as | Net-new? |
| --- | --- | --- |
| #5 `module_table` view-mutation | **PR2 cutover** (`import_bedrock_pr2_sys_modules_view_cutover.md`), blocked on R1 native call lane | tracked |
| #6 `Python.h` D1 | **task#10** (registry D1 burn-down) + `fail_closed_registry.toml` | tracked |
| #12 `structmember.h` D2 | `fail_closed_registry.toml` `D2_structmember` (burn-down described, no task#) | tracked (registry) |
| #13 numpy-multiarray B1 | **task#73.2** (generic source-plan authority) + M15/M55 | tracked |
| #14 `MODULE_FLAG_REINIT_RESURRECT` | **PR4** extension-snapshot reinit lane | tracked |
| #3/#12/#13/#6/#29-31 registry rows | rows in `tools/fail_closed_registry.toml` (D1/D2/B1/G1/G2) | tracked (registry) |
| **#1 `random.seed`** | — | **NET-NEW (high)** |
| **#2 `add_methods_to_dict`** | — | **NET-NEW (med, witness)** |
| **#3 `datetime.h` stub / D2 misclassification** | registry row exists but **misclassifies** it | **NET-NEW defect** |
| **#4 `PyModule_GetName`** | — | **NET-NEW (med, witness)** |
| **#7 `capi_trace` stale slot** | — | **NET-NEW (witness diagnostic)** |
| **#8 `mapping.rs` record-without-exception** | — | **NET-NEW (witness diagnostic)** |
| **#9 `PyThreadState_GetFrame`** | — | **NET-NEW** |
| **#10 `runtime_wasm_validation` fail-open** | — | **NET-NEW** |

The five net-new correctness items (#1, #2, #3, #4, #9) are **not** in any tracked
lane — they are the true yield of this audit and Lane A should be spun off first.

---

## 5. Verification

Findings were supplied as a verified JSON audit and independently re-checked
against the live tree at `origin/main` `e735c50c09` for this ledger:

- **#1** `random_mod.rs` — confirmed `default = []` (Cargo.toml:26), `crypto` gated
  fallback at `random_mod.rs:508/516/518` (`// Without crypto support, fall back to
  using the raw seed bytes.`), and **zero** `math/crypto`/`molt-runtime-math/crypto`
  enablers across all `*.toml`.
- **#3** `include/datetime.h` — confirmed `int _molt_reserved;` placeholder (:11),
  three unconditional `return 0` checks (:33/:38/:43), no `PyTime_Check`.
- **#4** `modules.rs:118` — confirmed body returns `c"molt.module".as_ptr()`.
- **#2** `typeobj.rs:270-271` — confirmed the "record it for diagnostics but do not
  abort readiness" comment.
- **#5** `module_table.rs` — confirmed `module_table_view_replace/_tombstone`
  definitions (:442/:459) with callers only at :1372/:1388/:1398/:1405 (tests).
- **#7** `modules.rs` — confirmed the live reader `set_module_system_error_if_clear`
  (:27/:43) with real call sites, i.e. the diagnostic surface is EFFECTIVE, not theater.
- **#18** `molt-worker/main.rs:237` — confirmed `take_cancelled` present and dead.

A PASS is a hypothesis until reproduced (M05); the action items in Lanes A–E are
each an independently-verifiable change with a stated fail-closed test.

---

*Ledger anchor `e735c50c09` · 2026-07-10 · re-anchor symbols by grep when lines drift.*
