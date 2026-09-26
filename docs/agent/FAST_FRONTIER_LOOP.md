# Fast Frontier Loop — reproduce CPython-ABI witness frontiers natively in seconds

> **The lever.** Every runtime-semantic frontier the numpy/scipy **wasm witness**
> hits (a silent `-1`, a wrong answer, a panic, a trap) has historically cost a
> **~20–30 minute** wasm build+run E2E to *discover*, and another full cycle per
> *fix* iteration. But the divergent code lives entirely in
> `runtime/molt-cpython-abi/` — **platform-independent Rust**. So the *same*
> divergence reproduces as a plain `cargo test` in **seconds**, with a real
> backtrace, a debugger, and sanitizers. This is the biggest cycle-time lever
> left on the witness path.

## TL;DR — use it

```bash
# Reproduce all catalogued frontiers (build + run), timed:
python tools/fast_frontier_cycle.py

# Reproduce just one (substring match on the test name):
python tools/fast_frontier_cycle.py --repro 08

# Prove the harness is live (green control, runs in the default gate):
python tools/fast_frontier_cycle.py --verify

# Raw cargo (no wrapper):
cargo test -p molt-lang-cpython-abi --test frontier_repro -- --ignored --nocapture
```

Set `CARGO_TARGET_DIR` to a fast off-OneDrive volume (e.g. `C:/Molt/cargo-target-XXX`)
for the best incremental times.

## Measured cycle time (this box, Windows/msvc, 2026-07-10)

| Step | Time |
|---|---|
| Cold build of the `molt-cpython-abi` frontier test binary | **~13 s** |
| Warm incremental edit → signal (`--repro`) | **~0.4–1 s** |
| One numpy/scipy **wasm witness** frontier cycle (baseline) | **~1800 s (~30 min)** |

That is **~140×** faster counting a cold build and **>1000×** on the warm
incremental loop that dominates a fix session. The `molt-cpython-abi` crate is
lightweight (13 normal deps, no `molt-runtime`), which is *why* the loop is cheap.

## Native C-extension DISCOVERY engine — LANDED (the deeper Tier A)

The **discovery** variant — driving a REAL prebuilt extension's `PyInit` against
molt's ABI + REAL `molt-runtime` hooks to catch **unknown** frontiers — is now
LANDED and RUNS real numpy 1.26.4 `_multiarray_umath` init natively (macOS/Linux;
not Windows — no flat namespace). Use it for numpy/scipy-import frontier
discovery instead of a wasm-witness cycle:

```bash
# Build the single-static-pool harness (incremental), static symbol-gap check,
# then drive PyInit and localise the runtime frontier (MOLT_TRACE_CAPI on):
CARGO_TARGET_DIR=<fast-dir> tools/native_numpy_discovery.sh _multiarray_umath
```

* Harness: `runtime/molt-cext-discovery` — a `cdylib` linking BOTH `molt-runtime`
  AND `molt-cpython-abi` into ONE image → a single ABI static pool that owns the
  REAL hooks (not the no-op `STUB_HOOKS`), the `Py*` numpy calls, and the loader.
* Driver: `tools/native_cext_driver.c` (`dlopen … RTLD_GLOBAL`, then drive PyInit).
* Measured **warm edit→frontier cycle: ~7 s** (vs ~1800 s for a wasm witness).
* The ORDERED frontier list it surfaced (symbol-gap + runtime tiers):
  [NATIVE_DISCOVERY_FRONTIERS.md](NATIVE_DISCOVERY_FRONTIERS.md).

Current numpy-init frontier surfaced: `PyCapsule_Import("datetime.datetime_CAPI")`
**silent-failure** (molt has no importable `datetime` CAPI capsule) → `PyInit`
returns NULL. Re-run after each fix to advance to the next frontier in seconds.

## What the harness is

`runtime/molt-cpython-abi/tests/frontier_repro.rs` turns the
[CPython-ABI Divergence Ledger](CPYTHON_ABI_DIVERGENCE_LEDGER.md) into
**executable native reproductions**. Each `frontier_*` test:

* drives the ABI entrypoint the way numpy's C code does (inline ints / raw
  pointers need no runtime; a minimal `alloc_str`/`str_data` fake materializes
  strings without pulling in `molt-runtime`);
* asserts the **CPython 3.12–correct** behavior;
* is `#[ignore]`d **only** because the fix has not landed yet.

Consequences:

* **default `cargo test` skips them → gates stay green** (no gate runs
  `--include-ignored`; verified against `.github/workflows/ci.yml`);
* `--ignored` runs them → each **fails loudly with a real backtrace in < 1 s** —
  that failure *is* the frontier reproduction;
* one **green control** test (`harness_drives_real_abi_code`) is *not* ignored:
  it runs in the default gate and proves the loop actually executes real ABI code.

### Currently catalogued reproductions (both verified reproducing 2026-07-10)

| Test | Ledger | What it catches natively |
|---|---|---|
| `frontier_08_pylong_aslong_silent_overflow` | #8, `numbers.rs` | `PyLong_AsLong(2**31+5)` → `-2147483643`, **no OverflowError** (CPython: `-1` + OverflowError). The scariest class — a silent wrong shape/stride/index on numpy array construction. |
| `frontier_06_pyobject_str_theater` | #6, `typeobj.rs` | `PyObject_Str(int)` → `"<molt object>"` (CPython: `"2147483653"`). Corrupts every `str()`/`repr()` and `%S` error message. |

## Workflow for burning down a frontier

1. Pick a High-severity, numpy-imported row from the
   [ledger](CPYTHON_ABI_DIVERGENCE_LEDGER.md) top-10 (or one a witness run just
   surfaced).
2. Add a `frontier_NN_*` test to `frontier_repro.rs`: call the ABI fn with the
   numpy-shaped input, `assert` the CPython-correct answer, `#[ignore]` it with a
   ledger reference. Confirm it **reproduces**: `--repro NN`.
3. Fix the ABI code. Re-run `--repro NN` on every edit (~1 s each).
4. When the test goes green, **delete its one `#[ignore]` line** — it is now a
   permanent regression guard in the default gate.

## Tier feasibility verdict (honest)

This lane evaluated three tiers. **Tier A landed** in its cheapest, highest-ROI
form (direct native ABI reproduction); the C-extension *discovery* variant and
Tier B/C are scoped below with the exact remaining gaps.

### Tier A — native reproduction of ABI frontiers — **LANDED (cheapest form)**

* **Direct-drive (landed).** Call the diverging ABI function natively with the
  argument shapes numpy uses; assert CPython semantics. Reproduces *known*
  frontiers (the ledger has 180 catalogued, 11 High) in seconds with real
  backtraces, cross-platform, no C toolchain. This kills the reproduce-and-fix
  loop for the entire known-divergence queue without a single wasm E2E.
* **C-extension discovery (deeper Tier A).** To catch *unknown* frontiers,
  run real C-extension init sequences (ideally numpy's `_multiarray_umath`)
  against the ABI and live `molt-runtime` hooks. Runtime and ABI state must
  be one linked image: an executable that links the ABI rlib while the
  extension binds a separately built ABI library has two bridges, type sets and
  error indicators, and registering shared hooks does not merge them. The
  `cext_host` example of `molt-runtime` is that image; `cext_dlopen_smoke.rs`
  links `hello.c` to it through `runtime/test_support/cext_fixture.rs`, loads
  only the host, and runs the whole scenario inside it: binding topology,
  single-phase init, physical method identity, `greet()`, an exact C
  exception, owner/cache/state release, collection and runtime shutdown.
  Build the host in the test's exact target/profile/features, then run it:

  ```bash
  cargo build -p molt-runtime --features cext_loader --example cext_host --test cext_dlopen_smoke
  cargo test -p molt-runtime --features cext_loader --test cext_dlopen_smoke
  ```

  Build both targets with the same features; the fixture uses Cargo's compiled
  host/target identities for C-tool selection,
  including cross-target execution. A missing host fails naming that build
  command and a missing C toolchain
  fails at compiler discovery; neither skips. This is a smoke path, not
  proof that the full numpy discovery lane works; record its native executable
  result per claimed host before claiming that. `molt-cext-discovery` shows
  the same link topology but carries discovery stubs and is not acceptance.
  `cext_integration.rs` covers only pre-open admission.
  1. **Real numpy compile.** `_multiarray_umath` is a meson build of many objects
     needing numpy's generated headers/config, not one `.c`. The witness already
     has a wasm seal pipeline (`tmp/pact_numpy_multiarray_sealed_for_witness`,
     `tools/pact_witness_numpy_generated_modules.py`); a native seal reuses it
     with a host triple. Multi-day. **Nearer-term substitute:** a hand-written
     "numpy-init-shaped" C extension (PyType_Ready with getset/members,
     PyModule_Create, PyArg `O!`, PyDict population) exercises the same frontier
     *classes* without compiling all of numpy.

### Tier B — `molt build --target native` a numpy witness — not evaluated to completion

Native `molt build` links `molt-runtime` + `molt-cpython-abi` statically and is
known to work for other extensions (e.g. tinygrad native builds). Whether the
native tier statically links a **cpython-abi C extension** end-to-end was not
proven this session; it shares the native numpy-compile gap above. Lower ROI
than Tier A for *frontier* work because a native `molt build` is far heavier
than a `molt-cpython-abi`-only `cargo test`.

### Tier C — fast wasm re-link loop — not needed

The fallback (rebuild runtime.wasm dev-fast, swap the runtime module, node
replay) would cut ~30 min → ~5 min. Tier A's ~1 s loop dominates it for the ABI
frontier class, so Tier C was not built. It remains the right tool for frontiers
that are **not** in `molt-cpython-abi` (e.g. codegen/lowering divergences visible
only in a full app.wasm).

## Next steps (precise follow-ups)

1. **Grow the reproduction set** to the rest of the ledger top-10 that are
   single-/double-pointer-arg (e.g. `PyObject_IsTrue([])` #5, `PyLong_AsSsize_t`
   #7). `PyArg_ParseTuple` O!/b-format memory bugs (#1/#2) need the C-ext path
   (variadic) — see below.
2. **Prove the cross-platform C-extension smoke** with a native executable
   result for each claimed host, then extend the one-image fixture to a
   numpy-init-shaped extension. A static compile or a loader-admission test
   does not prove the one-image binding boundary.
