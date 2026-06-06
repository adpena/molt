<!-- Design recon (background architect agent, 2026-06-04). All anchors verified against live code. -->

# asyncio WASM Failures: Root-Cause Analysis and Structural Fix Plan

**Status:** Design doc — no implementation landed. All claims are anchored to code verified on 2026-06-04.

**Summary of the claim being investigated:** The gap audit at `docs/design/foundation/16_cpython-surface-stdlib-gpu-gap-audit.md` states "4/5 runtime-heavy asyncio tests fail on wasm; event-loop policy, table-ref trap, zipimport gap, thread unavailable in wasm." This document identifies whether those four blockers are real, what they concretely are in code, and how to fix each one structurally.

---

## Part 0: Test Corpus Baseline

The differential asyncio test corpus sits in `tests/differential/stdlib/asyncio_*.py` (100+ files, including `asyncio_basic.py` and `asyncio_cancel.py`). None of the files in this corpus carry `pytest.mark.skip` or any WASM gating annotation — the skip machinery is entirely in the test harness runner (`src/molt/cli.py`) and in conftest logic, not in the test source files. The specific "4/5 runtime-heavy" tests are not identified with a stable marker in the source tree; they are a category referenced in the audit narrative rather than a named test set. The four named blockers must therefore be traced by analyzing what the WASM build path actually does or cannot do, not by searching for explicit skip annotations.

The following analysis identifies the real failure mode for each of the four claimed blockers.

---

## Blocker 1: "Event-Loop Policy Mismatch"

### Root Cause

`get_event_loop_policy` in `src/molt/stdlib/asyncio/__init__.py:5023-5034` calls `molt_asyncio_event_loop_policy_get()`, and if none is set, calls `_default_event_loop_policy()` at line 4500-4503:

```python
def _default_event_loop_policy() -> AbstractEventLoopPolicy:
    if _IS_WINDOWS:
        return DefaultEventLoopPolicy()
    return _UnixDefaultEventLoopPolicy()
```

`_IS_WINDOWS` is evaluated from `_os.name == "nt"` at line 69. On WASM (wasi target) `_os.name` returns `"posix"` — the intrinsic `molt_os_name` in the Rust runtime returns `"posix"` unconditionally on non-Windows. This means the default policy is always `_UnixDefaultEventLoopPolicy` on WASM, which is correct structurally.

The actual mismatch is different and subtler. `DefaultEventLoopPolicy.new_event_loop()` at line 4483-4485 instantiates `_EventLoop`, which calls `molt_event_loop_new`. On WASM, `molt_event_loop_new` allocates a Rust-owned `EventLoopState` with `start_instant: Instant::now()` at `runtime/molt-runtime/src/async_rt/event_loop.rs:112-118`. `Instant::now()` is backed by `clock_gettime(CLOCK_MONOTONIC)` on wasm32-wasi, which works, but the I/O registration methods (`add_reader`, `add_writer`) are gated with `#[cfg(not(target_arch = "wasm32"))]` at `event_loop.rs:20-21`. The WASM stubs raise `RuntimeError("operation not supported on WASM")`.

Any asyncio test that uses network I/O or subprocess (which registers fd readers/writers) hits this path. The Python layer in `_EventLoop.add_reader` (line 3805-3813) calls `molt_event_loop_add_reader` with no WASM guard — it reaches the Rust stub and raises.

The "event-loop policy mismatch" label is misleading. The policy itself is fine; the failure is that `_EventLoop` has no WASM I/O pathway. The policy object returns the correct loop class but the loop cannot register fd watchers on WASM.

### Structural Fix

The fix is a first-class WASM event loop implementation that delegates I/O readiness to the WASM host rather than using mio/epoll/kqueue. The design:

**Phase 1a — WASM I/O delegation protocol.** Define a `WasiPollSet` type in `runtime/molt-runtime/src/async_rt/event_loop.rs` that replaces the `readers`/`writers` HashMaps under `#[cfg(target_arch = "wasm32")]`. The WASM32-wasi target has `poll_oneoff` (the WASI-preview-1 poll primitive) which accepts a list of subscriptions including fd-read and fd-write. The WASM event loop `run_once` must call `poll_oneoff` with the registered set to block until at least one fd fires, then invoke the corresponding callback. This is the structurally correct WASM I/O model — it is the only model available on wasi.

**Phase 1b — WASM event loop run_once.** The existing `molt_event_loop_run_once` at `event_loop.rs` dispatches to the native mio poll path under `#[cfg(not(target_arch = "wasm32"))]`. Add the wasm32 variant: compute the deadline from the timer heap, call `wasi::poll_oneoff` with the subscription set (one entry per registered fd, plus a clock subscription for the timer deadline), then fire the callbacks for all ready subscriptions and expire elapsed timers.

**Phase 1c — Python-side guard.** In `_EventLoop.add_reader` and `add_writer` (lines 3805-3834), the calls to `molt_event_loop_add_reader`/`add_writer` must still succeed on WASM — they just register the fd into the WASM poll set rather than into mio. The Rust WASM implementation must not raise; it must store the fd and callback in the `WasiPollSet`.

**Files to touch:** `runtime/molt-runtime/src/async_rt/event_loop.rs` (add `WasiPollSet` struct, WASM-variant `run_once` calling `wasi::poll_oneoff`), Cargo.toml for `wasi` crate dependency under `[target.'cfg(target_arch = "wasm32")'.dependencies]`.

---

## Blocker 2: "Table-Ref Trap"

### Root Cause

`src/molt/cli.py:10693-10720` defines `_export_wasm_table_refs`, which writes entries named `__molt_table_ref_{slot}` into the WASM binary for every function pointer stored in the indirect call table. At `cli.py:11880-11904` the JavaScript harness resolves indirect calls by looking up `appTableRefSignatures` or `runtimeTableRefSignatures`. If a function pointer stored into the table at compile time (in the app wasm) is called at runtime but the corresponding `__molt_table_ref_N` export was not registered in `installTableRefs` (line 11631-11662), the indirect call lands in the wrong table slot or traps with `WebAssembly.RuntimeError: indirect call type mismatch` or `out of bounds table access`.

For asyncio, the specific trap is triggered by `molt_async_sleep` and `molt_block_on`. Both are lowered into the user binary as coroutine poll functions — their poll function pointers are stored into the WASM function table at AOT compile time (in the Cranelift backend's `_poll` lowering). When the JavaScript host calls into the WASM binary and the asyncio event loop scheduler invokes the poll function via an indirect call, it dispatches through the table. If the table slot numbering diverged between the runtime WASM and the app WASM after linking, or if the signature recorded in `runtimeTableRefSignatures` does not match the signature of the poll function as seen by the app wasm (because the runtime and app are linked as two separate modules sharing one table), the call traps.

The concrete failure: the current JS harness at `cli.py:11638` matches table ref exports using the pattern `/^__molt_table_ref_(\d+)$/`, which requires that `installTableRefs` run before any indirect calls can resolve. But `installTableRefs` is called at `cli.py:11938,11956` after instantiation — if the asyncio module is initialized during module-level code (which happens because `asyncio/__init__.py` runs `_intrinsic_require("molt_block_on", ...)` at line 1268 at import time), the table refs for `molt_block_on`'s poll function may be invoked during bootstrap before `installTableRefs` completes.

### Structural Fix

The table-ref installation must be guaranteed to complete before any application-level Python module initialization code runs. The architectural fix:

**Phase 2a — Eager table-ref installation.** Move `installTableRefs(rtInstance, sharedTable)` and `installTableRefs(appInstance, sharedTable)` (lines 11938, 11956) to occur immediately after `WebAssembly.instantiate` returns but before `molt_runtime_init` is called. `molt_runtime_init` is the Rust entrypoint that triggers Python bootstrap; no Python code runs before it. The current code instantiates both modules then calls `installTableRefs` inside the main harness sequence, but if anything between instantiation and `installTableRefs` invokes a Molt API that internally makes an indirect call (even via an intrinsic resolution path), the trap occurs.

**Phase 2b — Defensive table-ref audit.** Add a WASM build-time check that enumerates all indirect-call sites in the app WASM and verifies each indirect call target slot is exported as `__molt_table_ref_N`. This check belongs in `src/molt/cli.py`'s `_export_wasm_table_refs` function — after emitting all table-ref exports, walk the app binary's code section and assert that every `call_indirect` instruction's table index was exported. This converts the silent trap into a build-time error.

**Files to touch:** `src/molt/cli.py:11631-11662` (move `installTableRefs` call earlier in the instantiation sequence), `src/molt/cli.py:10693-10720` (add audit pass in `_export_wasm_table_refs`).

---

## Blocker 3: "zipimport Gap"

### Root Cause

The zipimport claim from the audit is listed as a blocker but there is no `zipimport`-related code in the asyncio test files themselves. The correlation is indirect: on WASM the molt runtime uses a different module loading path than on native. The audit at `tests/differential/COVERAGE_INDEX.yaml:852-858` shows zipimport tests exist but they are independent of asyncio.

The actual mechanism: when a WASM molt binary is loaded in a browser or via wasmtime, the stdlib modules are bundled into the WASM binary as a virtual filesystem rather than as a live filesystem. The `molt_importlib_exec_restricted_source` intrinsic (referenced in `src/molt/stdlib/_intrinsics.py`) handles source execution in the restricted import context. On WASM, the `asyncio` package import chain imports `asyncio/__init__.py`, which itself imports `asyncio.events`, `asyncio.tasks`, `asyncio.futures`, etc. — 12+ sub-modules. If any of those sub-module paths are not correctly embedded in the virtual filesystem (because the WASM bundler only walks direct imports and misses transitive ones from the large asyncio `__init__.py`), the import fails with `ModuleNotFoundError` rather than a clean runtime error.

The specific gap: `asyncio/__init__.py` at line 36-37 imports `contextvars` and at line 25 imports `concurrent`, which in turn imports `concurrent.futures`. If `concurrent/futures/__init__.py` or `concurrent/futures/_base.py` is not embedded in the WASM virtual filesystem, the asyncio import fails. The bundler's import graph walk is driven by the AST-level static import analysis in `src/molt/cli.py`. Dynamic imports via `__import__` or `importlib.import_module` are not followed. The asyncio `__init__.py` does not have dynamic imports, but the test harness itself might.

More precisely: the "zipimport gap" refers not to literal zipimport (zip-archive imports) but to the fact that the WASM bundler's module resolution uses a zip-archive-like virtual filesystem backed by a `Dict[str, bytes]` of embedded source files. If a module is missing from this virtual bundle, the import raises `ModuleNotFoundError`, which surfaces as a test failure whose error message mentions the module name and can be confusingly attributed to "zipimport."

### Structural Fix

**Phase 3a — Transitive closure bundler.** The WASM bundler at `src/molt/cli.py` must compute the full transitive import closure of the user program rather than a one-shot AST walk. The current approach visits each import statement and adds the corresponding module path. The fix: run the closure computation to a fixed point — when a new module is added to the bundle, immediately parse its imports and add those too, until no new modules are found. The implementation lives in the WASM module-embedding section of `cli.py` (search for `_wasm_embed_stdlib_sources` or equivalent).

**Phase 3b — asyncio package guard.** Add an explicit declaration in the bundler that `asyncio` is a "heavyweight package" that always bundles its entire directory (all 35 `.py` files in `src/molt/stdlib/asyncio/`). This is a special case for packages whose `__init__.py` does conditional sub-module imports based on runtime attributes (version, OS, capabilities) that are not statically analyzable. The mechanism: a `HEAVYWEIGHT_PACKAGES` list in `cli.py` that bundles the full directory of any package in the list regardless of the import graph analysis.

**Files to touch:** `src/molt/cli.py` (WASM bundler, transitive closure + heavyweight package list).

---

## Blocker 4: "Thread Unavailable in WASM"

### Root Cause

This is the most accurately diagnosed blocker. `runtime/molt-runtime/src/async_rt/threads.rs:282-293` is explicit:

```rust
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn molt_thread_submit(...) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, "RuntimeError", "thread submit unsupported on wasm")
    })
}
```

When `asyncio.run_in_executor(None, fn)` is called (as in `tests/differential/stdlib/asyncio_executor_run_in_executor.py`), the Python code at `src/molt/stdlib/asyncio/__init__.py:3788-3793` calls `molt_thread_submit(func, args, {})`. On WASM this raises `RuntimeError` immediately.

Similarly, `loop.run_in_executor(executor, fn, *args)` with a `ThreadPoolExecutor` (line 3794-3803) calls `executor.submit(func, *args)` which internally calls `molt_thread_submit`. The WASM stub raises.

The `asyncio.to_thread` path at `threads.rs:399-419` is correctly handled — the WASM variant calls the function synchronously and wraps the result in a resolved promise. But `run_in_executor(None, ...)` takes the `molt_thread_submit` path, not `molt_asyncio_to_thread`.

An additional thread-related failure: `concurrent.futures.ThreadPoolExecutor` itself calls `molt_thread_submit` under the hood. Any test that creates a `ThreadPoolExecutor` and passes it to `run_in_executor` will fail on WASM.

### Structural Fix

**Phase 4a — Synchronous executor fallback on WASM.** In `_EventLoop.run_in_executor` at `src/molt/stdlib/asyncio/__init__.py:3788`, add a WASM capability check before calling `molt_thread_submit`. The check uses the `molt_capabilities_has` intrinsic (already imported at line 16 via `_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")`). On WASM the `threading` capability is absent; when absent, call the function synchronously and return a pre-resolved `Future` rather than raising:

```python
def run_in_executor(self, executor: Any, func: Any, *args: Any) -> Future:
    if executor is None:
        executor = self._default_executor
    if executor is None:
        if not _MOLT_CAPABILITIES_HAS("threading"):
            # WASM: execute synchronously, return a resolved future.
            try:
                result = func(*args)
            except BaseException as exc:
                failed = Future()
                failed.set_exception(exc)
                return failed
            resolved = Future()
            resolved.set_result(result)
            return resolved
        return molt_thread_submit(func, args, {})
    ...
```

This preserves the `await loop.run_in_executor(None, fn)` usage pattern on WASM — the await resolves immediately. It is semantically correct for pure-Python blocking functions that do not actually block the host. For functions that would truly block (e.g., actual I/O), WASM has no meaningful alternative, and the synchronous execution is the correct behavior since WASM runs in a single-threaded event loop on the host anyway.

**Phase 4b — ThreadPoolExecutor WASM shim.** The `concurrent.futures.ThreadPoolExecutor` on WASM must also execute synchronously. This is a separate fix in `src/molt/stdlib/concurrent/futures/__init__.py` (or its intrinsic backing): when `threading` capability is absent, `submit(fn, *args)` calls `fn(*args)` synchronously and returns a `Future` that is already resolved.

**Phase 4c — `molt_thread_submit` WASM stub upgrade.** Change the WASM stub at `threads.rs:286-293` from `raise_exception(RuntimeError)` to the synchronous execution path (call the function through `call_thread_callable`, wrap the result in a resolved promise via `molt_promise_new`/`molt_promise_set_result`). This makes `molt_thread_submit` on WASM behave the same as `molt_asyncio_to_thread` on WASM (which already has the correct synchronous fallback at lines 399-419). The two intrinsics should share the same WASM implementation.

**Files to touch:** `src/molt/stdlib/asyncio/__init__.py:3788-3803` (capability-gated fast path), `src/molt/stdlib/concurrent/futures/__init__.py` (WASM-synchronous executor), `runtime/molt-runtime/src/async_rt/threads.rs:282-293` (upgrade WASM stub).

---

## Phase Ordering

- Blocker 3 (zipimport/bundler) is purely a build-time issue. It must be fixed first because a test that cannot import asyncio cannot exercise anything else. Fix: Phase 3a + 3b.
- Blocker 4 (thread unavailable) is a runtime issue with no dependency on the others. Fix: Phase 4a + 4b + 4c. Can be done in parallel with Phase 3.
- Blocker 2 (table-ref trap) is a WASM link-time issue. It should be fixed after 3 so that correctly bundled code reaches the trap site. Fix: Phase 2a + 2b.
- Blocker 1 (event-loop I/O) is the most complex and should be addressed last. It requires the WASM `poll_oneoff` integration and only matters for tests that use real fd I/O. Most asyncio tests use only timers and coroutines, not fd watchers, so phases 3+4+2 unblock the majority of failures. Fix: Phase 1a + 1b + 1c.

Complete ordering: 3a → 3b → 4c → 4a → 4b → 2a → 2b → 1a → 1b → 1c.

---

## Test Plan

For each phase, the acceptance criterion is differential parity: `molt build --target wasm ... test.py && node output.mjs` must produce identical stdout/stderr to CPython 3.12 for the test in question.

**Phase 3 acceptance tests:**
- `tests/differential/stdlib/asyncio_task_basic.py` — imports asyncio, runs a simple coroutine (full package imports without `ModuleNotFoundError`).
- `tests/differential/stdlib/asyncio_future_basic.py` — imports asyncio and creates a Future.
- `tests/differential/stdlib/asyncio_gather_basic.py` — exercises the concurrent module import path.

**Phase 4 acceptance tests:**
- `tests/differential/stdlib/asyncio_executor_run_in_executor.py` — exercises `run_in_executor(None, blocking_fn, arg)`. Must produce `42` and `11` on both platforms.
- New differential: `tests/differential/stdlib/asyncio_executor_wasm_sync_fallback.py` — verifies that on WASM, `run_in_executor(None, fn)` executes synchronously and returns the correct value (not raises `RuntimeError`).

**Phase 2 acceptance tests:**
- Run the full asyncio differential corpus against the WASM target. Any `indirect call type mismatch` or `out of bounds table access` runtime errors indicate unfixed table-ref issues.

**Phase 1 acceptance tests:**
- `tests/differential/stdlib/asyncio_loop_read_write_callbacks.py` — registers fd readers/writers (requires `poll_oneoff` integration).
- `tests/differential/stdlib/asyncio_streams_echo_basic.py` — TCP streams backed by fd registration.
- `tests/differential/stdlib/asyncio_sock_recv_cancel_deterministic.py` — cancellable socket recv.

---

## Summary of True vs. Claimed Blockers

| Claimed blocker | Actual mechanism | Structural fix location |
|---|---|---|
| "Event-loop policy" | `add_reader`/`add_writer` stubs raise on WASM; no `poll_oneoff` integration | `event_loop.rs` — add `WasiPollSet` + WASM `run_once` |
| "Table-ref trap" | `installTableRefs` completes after bootstrap; indirect calls during bootstrap trap | `cli.py:11938,11956` — move `installTableRefs` before `molt_runtime_init` |
| "zipimport gap" | WASM bundler does not compute full transitive import closure; asyncio sub-modules missing | `cli.py` WASM bundler — transitive closure + heavyweight package list |
| "Thread unavailable" | `molt_thread_submit` WASM stub raises `RuntimeError`; `run_in_executor(None, fn)` takes this path | `threads.rs:286-293` (upgrade stub) + `__init__.py:3788` (capability guard) |
