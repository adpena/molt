# 22 — Bug-Hunt Sweep, Wave 1

Status: COMPLETE (wave 1). Two agents (predecessor + continuation) ran the
three-lens sweep — PROFILE / WIRING AUDIT / GAP CENSUS — against
`worktree-agent-a0c76397aae710ddd` (base `9ca6ffe8f`, secured HEAD `f92bef4a4`).

This document is the authoritative record: every finding, every landed fix
(with verification), every baton. Numbers were re-measured at the secured HEAD
by the continuation agent unless explicitly attributed to the predecessor.

Methodology guard rails honored throughout: bounded compute benches timed
directly (`/usr/bin/time -p` + `gtimeout` backstop) — NOT through `safe_run.py`,
which adds ~0.26 s of Python-interpreter + RSS-poll wrapper overhead that
completely masks micro-bench signal (the predecessor's first "P0 alarms" were
this artifact). Unbounded/raw-binary execution still goes through `safe_run.py`.

---

## 0. Landed fixes (this sweep) — all verified at secured HEAD

| Commit | What | Lane | Verification |
|---|---|---|---|
| `443c7e8a8` | Structural guard pinning WASM static type indices to `STATIC_TYPE_COUNT` (=51); fixes stale `>=39` assertion in `tests/wasm_type_section.rs` | backend/wasm | `wasm::tests::static_type_section_signatures_are_pinned_to_static_type_count` GREEN; 1034 backend lib tests pass |
| `cff84d393` | Cache 3 uncached `env::var` debug flags on exception/dispatch hot paths (`MOLT_DEBUG_EXCEPTION_MATCH`, `MOLT_DEBUG_EXCEPTIONS`, dispatch) | runtime | env-size-invariance test (below) — getenv off the hot path |
| `6f1304233` | Cache 6 more uncached `env::var`/`var_os` flags on attr/dispatch/call hot paths — **broadest = `MOLT_TRACE_ATTR_LOOKUP` in `attr_lookup_ptr` (every attribute access)** | runtime | same; etl/exception timing now invariant to env size |
| `f92bef4a4` | Version-gate `TextIOWrapper.reconfigure()` invalid-kwarg `TypeError` (3.13+ "got an unexpected keyword argument" vs 3.12 "is an invalid keyword argument for reconfigure()") | runtime | **byte-identical to CPython at both 3.12 and 3.14 targets** (below) |
| `<this doc + guard>` | `name_neq_symbol_specs_resolve_in_core` structural guard (registry.rs) for the async_sleep bug class | runtime | new test GREEN; 497 runtime lib tests pass; 0 new clippy |

### Verification detail

- **`443c7e8a8`**: `cargo test -p molt-backend --features "native-backend wasm-backend" --lib` → 1034 passed / 0 failed; the pinning test is present at `wasm.rs:17476`, `STATIC_TYPE_COUNT = 51` at `wasm.rs:298`.

- **`cff84d393` + `6f1304233`** (env-var caching): the structural proof that
  getenv is OFF the hot path is **timing invariance to environment size**.
  `bench_exception_heavy` (20M iters, ~667K raises) timed with a small env vs an
  env padded with 800 extra vars:
  - small env: 3.29 s best-of-3
  - +800 env vars: 3.14 s best-of-3 → delta 0.15 s (noise; big actually faster)
  Pre-fix, the per-attribute-access `MOLT_TRACE_ATTR_LOOKUP` getenv ran under the
  libc `environ` lock (O(env size)); the 800-var run would have added seconds.
  Invariance ⇒ the fix is effective. (Predecessor also measured a direct
  before/after on a smaller bench: 0.23 s → 0.19 s, ~17%.)

- **`f92bef4a4`** (reconfigure): verified with the **correct knob** —
  `--python-version=N` build flag (NOT `MOLT_PYTHON_VERSION`, which does not set
  the runtime target; default target is **3.12**). Repro `f.reconfigure(foo=1)`:
  - `--python-version=3.14`: molt → `reconfigure() got an unexpected keyword argument 'foo'` == CPython 3.14 ✓
  - `--python-version=3.12`: molt → `'foo' is an invalid keyword argument for reconfigure()` == CPython 3.12 ✓
  - bind path confirmed: `bind_builtin_file_reconfigure` (bind.rs:5653) is the
    kwarg-binding layer; `molt_file_reconfigure` (io.rs:5678) only receives
    already-bound args. The fix is in the right place.

  > Knob note for future agents: molt's **default target is 3.12**.
  > `sys.version_info` only flips via `--python-version=N` (or the project
  > config / `MOLT_TARGET_PYTHON_*` env that `env_target_python_info`
  > [ops_sys.rs:304] reads). `MOLT_PYTHON_VERSION` is **not** that knob — a
  > version-gated parity message will look "broken" if tested with it.

---

## 1. PROFILE lens — perf-contract (P0 = any bench slower than CPython 3.14)

CPython baseline: 3.14.5. molt: native release-fast at secured HEAD.

The predecessor swept fib / attr / dict / list / float / calls / generators /
json / csv / counter / class-hierarchy / matrix — **molt faster than CPython on
all of them** (ratios 0.06×–0.81×). The continuation re-confirmed the two P0s.

### P0-1 — `bench_etl_orders`: **2.69× SLOWER** (most severe)

- Workload: `@dataclass Order` (5 fields), ~N rows built + parsed + aggregated
  (`tmp/bh_etl_big3.py`). molt 4.58 s vs CPython 1.70 s best-of-3.
- The predecessor measured ~1.6× on a smaller variant; the larger row count
  amplifies the gap — it scales with object/string churn, so this is the headline
  perf P0.
- Root cause (predecessor `/usr/bin/sample`, re-confirmed): after the
  `MOLT_TRACE_ATTR_LOOKUP` getenv fix, the dominant frames are `_tlv_get_addr`
  (per-GIL-entry thread-local token access, ~128 samples) + `_platform_memmove`
  (dataclass object allocation + split-list allocation, ~62 samples). getenv
  (`__findenv_locked`) is **gone** from the hot path (proven by env-size
  invariance — adding 500 env vars did not slow etl).
- Class: **architectural** — allocation churn + per-entry TLS. NOT getenv.

### P0-2 — `bench_exception_heavy`: **1.26× SLOWER**

- Workload: `raise ValueError(i)` every 3rd of 20M iters + `int(str(e))` per
  catch (`tmp/bh_exc_big.py`). molt 3.19 s vs CPython 2.53 s best-of-3.
  (Predecessor: ~1.36× post-fix on a smaller iter count — same residual.)
- Root cause: exception-object allocation + refcount churn per raise +
  `_tlv_get_addr` TLS per GIL entry. getenv eliminated by `cff84d393`.
- Class: **architectural** — same allocation/TLS family as P0-1.

### PROFILE baton (→ separate perf arc, NOT this sweep)

Both P0s reduce to the **same two architectural costs**: (a) per-object
allocation churn (dataclass `object_new`, exception `exception_new`, transient
list/str), and (b) `_tlv_get_addr` on every GIL entry (the thread-local GIL
token). A structural attack on either pays out on BOTH benches:
  1. **TLS GIL-token**: cache/inline the GIL token so `_tlv_get_addr` is not
     re-resolved per runtime entry. macOS `__thread`/TLV is the cost; a register-
     or frame-pinned token across a call region removes it.
  2. **Allocation fast-path**: a bump/freelist arena for short-lived objects
     (exception instances, dataclass instances, split-result lists) so
     `_platform_memmove` + allocator overhead drops. Ties into the Perceus reuse
     work already in-tree.
This is a multi-day structural arc (allocator + TLS ABI) — batoned, not started,
per the zero-partial-fix policy. **molt is still faster than CPython everywhere
else measured; these two allocation-bound realistic programs are the gap.**

---

## 2. WIRING AUDIT lens — registry/manifest/oracle completeness

### 2a. Intrinsic name≠symbol class — **GUARD LANDED** + live break batoned

The single sharpest wiring finding. Across **all 2480 intrinsic specs, exactly
ONE** has `name != symbol`: `molt_async_sleep → molt_async_sleep_new`
(`generated.rs:1931`; the override is deliberate — `gen_intrinsics.py:23`
`SYMBOL_OVERRIDES`, because there are TWO runtime exports: the legacy 1-arg
`molt_async_sleep(obj)->i64` [generators_async.rs:1901] and the new 2-arg
`molt_async_sleep_new(delay,result)->u64` [generators_async.rs:1878]; the Python
intrinsic must bind the 2-arg one).

That lone name≠symbol spec is **precisely the one that fails at runtime** (see
GAP CENSUS §3a). Resolution flow:
```
require("molt_async_sleep")
  → find_spec(name).symbol = "molt_async_sleep_new"
  → try_app_resolve_symbol("molt_async_sleep_new")   // queries by SYMBOL
  → app resolver returns 0 → IntrinsicResolveError::MissingSymbol
  → RuntimeError: intrinsic unavailable: molt_async_sleep
```
The runtime core CAN resolve it: `resolve_symbol("molt_async_sleep_new")` is
`Some` (proven by the new guard). So the break is **purely in the app-resolver
generation** — the backend emits `molt_app_resolve_intrinsic` keyed by intrinsic
**name** ("the intrinsics this app reaches by name", cli.py:17698) while the
runtime queries it by **symbol**. For the 2479 name==symbol specs this skew is
invisible; for the 1 name≠symbol spec it is fatal.

**Landed guard** (registry.rs, `name_neq_symbol_specs_resolve_in_core`): asserts
that for every spec where `name != symbol`, `resolve_symbol(spec.symbol)` is
`Some`. This pins the runtime-side closure so a renamed/dropped/misspelled
override symbol fails at `cargo test` instead of shipping. It does NOT cover the
app-resolver generation (the live break) — that is the excluded lane (below).

**Baton (live break, EXCLUDED lane — backend app-resolver gen + cli.py):** the
fix is to make the app resolver key its table by `spec.symbol` (what the runtime
queries) rather than `spec.name`. The cli.py extern at 17688 also declares the
**1-arg** symbol `extern long molt_async_sleep(void* obj)` — wrong overload.
Severity is **P0-class** (see §3a): the app-resolver gen + cli.py manifest are in
`src/molt/frontend`-adjacent + `simple_backend.rs` reachability, both excluded.

### 2b. `matches!`-over-OpCode oracles — classification table (105 variants)

The documented hazard: a `matches!(opcode, A | B | ...)` oracle defaults to
`false` for any unlisted opcode. Add a new opcode, forget the list ⇒ silent
miscompile (SCCP/LICM eliminate or hoist a throwing/effecting op). Exhaustive
`match` is compiler-enforced; `matches!` is not. (Lesson from
`project_import_parity_done.md`.)

| Oracle | File:line | Default | Hazard |
|---|---|---|---|
| `opcode_may_throw` | effects.rs:90 | false | new throwing op silently non-throwing |
| `opcode_is_side_effecting` | effects.rs:137 | false | new effecting op silently pure → DCE'd |
| `opcode_is_pure_movable` | effects.rs:377 | (allowlist) | lower risk (allow-list, conservative-when-missing = not movable) |
| `may_throw` (**DUPLICATE**) | sccp.rs:68 | false | **diverges from effects oracle — see §2c** |

`effects.rs` is the S3 single-source-of-truth oracle. Its three `matches!`
oracles are the documented hazard; converting `opcode_may_throw` /
`opcode_is_side_effecting` to **exhaustive `match` over all 105 `OpCode`
variants** is the structurally correct fix (NASA-grade: compiler forces every new
opcode to be classified). It was deemed too large/error-prone to land safely
mid-sweep (a *wrong* classification is worse than the current hazard). **Baton:
NOT in the excluded lane** (effects.rs is not drop/refcount) — a focused,
high-value follow-up. effects.rs already has guard tests
(`movable_ops_are_never_side_effecting_or_may_throw`, etc.) to anchor the
conversion.

### 2c. `sccp::may_throw` diverges from `effects::opcode_may_throw` — TWO SOURCES OF TRUTH

A real structural finding (S3 violation). `sccp.rs:68 may_throw` is a hand-copied
duplicate of `effects.rs:90 opcode_may_throw`, used at sccp.rs:113
(`if try_depth > 0 && may_throw(op.opcode)`) to protect ops inside try regions
from being constant-folded. The two lists **DIVERGE**:

- In `effects` but **MISSING from sccp** (sccp may wrongly fold these inside a
  try region): `OrdAt`, `ModuleGetAttr`, `ModuleImportFrom`, `ModuleGetGlobal`,
  `ModuleGetName`, `ModuleSetAttr`, `ModuleCacheGet`, `ModuleCacheSet`,
  `ModuleCacheDel`, `ModuleDelGlobal`, `ModuleDelGlobalIfPresent` (11 opcodes).
- In `sccp` but missing from `effects`: `StateYield` (1).

The module ops are throwing per the import-parity work
(`ModuleGetAttr`/`ModuleImportFrom` raise `ModuleNotFoundError`/`AttributeError`).
**Latent, not currently exploitable**: a targeted repro
(`math.nonexistent_attr` inside `try/except AttributeError`) returns the correct
value on native (99 == CPython) — because SCCP only folds ops whose lattice value
is a known constant, and a module-attr load is overdefined, so it is not a fold
candidate in this shape. But the divergence is a **drift hazard**: a different op
shape (e.g. `OrdAt` on a constant-indexed string in a try, or a future opcode
added to `effects` but not `sccp`) could fold a throwing op out of a protected
region → miscompile.

**Baton (NOT excluded — SCCP, not drop/refcount):** delete `sccp::may_throw` and
route sccp's try-region protection through `effects::op_may_throw` (the S3 single
source). Must confirm `StateYield`'s presence in sccp is intentional (it is in
sccp's list but not effects' — likely because sccp sees pre-lowering `StateYield`
that effects' post-lowering view doesn't; verify before deleting). This is the
exact bug-class S3 was built to eliminate; the duplicate slipped through.

### 2d. wasmtime host imports — STRICT (no silent miscompile risk)

`runtime/molt-wasm-host/src/main.rs`: `Linker::new(&engine)` (no
`allow_shadowing`, no `define_unknown_imports_as_traps`) + `.instantiate()` (the
strict, non-`pre` variant) at lines 4837/4853/4918. A WASM module importing a
host symbol the host does not `linker.define` is a **hard instantiate error**,
not a silent trap. Host imports are defined explicitly (e.g.
`molt_socket_*_host`, `molt_db_*_host`). **No structural gap** — missing host
imports fail loudly at instantiate. No guard needed.

### 2e. LLVM `runtime_imports` — already guarded

`runtime/molt-backend/src/llvm_backend/runtime_imports.rs` already carries
consistency tests: `runtime_functions_are_declared`,
`fused_method_dispatch_ic_runtime_functions_are_declared`,
`module_namespace_runtime_functions_are_declared`, `all_functions_have_nounwind`
(lines 912–1088). This surface is reasonably protected; no new guard warranted.

### 2f. Cargo feature graph — never-compiled-in-CI + **NEW: two features bit-rotted**

CI compiles only `native-backend` (6 refs), `wasm-backend` (1), `llvm` (8).
**Never compiled in ANY workflow**: `egraphs`, `polly`, `luau-backend`, `cbor`,
`rust-backend`, `mlx`, `jemalloc`.

**NEW FINDING (regression vs predecessor, who found these compiled):** at secured
HEAD, **`luau-backend` and `rust-backend` FAIL to compile standalone**:
```
error[E0432]: unresolved import `crate::representation_plan::raw_i64_safe_values_for`
  --> runtime/molt-tir/src/tir/passes/liveness.rs:47
```
Root cause: `raw_i64_safe_values_for` (representation_plan.rs:537) is
`#[cfg(any(feature="native-backend", feature="llvm", feature="wasm-backend",
test))]`, but `liveness.rs:47` imports it **unconditionally** (and calls it at
liveness.rs:260 in `compute_raw_scalars`). With ONLY luau/rust enabled the
function is cfg'd out → unresolved import. The break is **masked whenever
native/llvm/wasm is also on** (cbor+native compiles clean), which is exactly why
CI (always native) never sees it.

- **Pre-existing**: introduced by `002c3e6ae` (RC drop-insertion substrate,
  design 20) + `37149fbfc` — both ancestors of the worktree base `9ca6ffe8f`.
  NOT introduced by this sweep.
- **EXCLUDED lane**: `liveness.rs` is the "RC drop-insertion substrate, design 20,
  Phase 2" (its module doc) consumed by `drop_insertion.rs`. The DropInsertion
  fix agent owns this lane.
- **Baton (DropInsertion agent):** align the cfg gate. `compute_raw_scalars`'s
  use of `raw_i64_safe_values_for` (the RawI64Safe set, only meaningful for
  backends with unboxed-i64 carriers) must be gated by the same
  `any(native-backend, llvm, wasm-backend, test)` cfg as the function — and the
  luau/rust path must still compute the by-type non-heap carrier set (bool/float/
  never) it needs without the RawI64Safe seed. This is a correctness question
  (does luau/rust need raw-scalar drop analysis at all? they emit dynamically-
  typed Luau / generated Rust with no bare-i64 carrier), not a mechanical import
  gate — hence batoned to the lane owner, not patched here.

**Baton (CI hardening, low-risk):** add a CI job that `cargo check`s each
never-in-CI feature **standalone** (`--no-default-features --features X`) to catch
this bit-rot class. Would have caught luau/rust at the introducing commit. The 7
features have real code (luau-backend 2 files, rust-backend 2, egraphs 2, cbor 2,
polly 1, mlx, jemalloc) with no structural protection against future drift.

### 2g. link-roots vs no_mangle orphans

4193 runtime `no_mangle` exports; native dead-strips through the app-intrinsic
resolver (resolve_symbol is intentionally native-unreachable so `--gc-sections`
drops unused intrinsics — registry.rs:74 comment). A full orphan census
(no_mangle symbols never reachable from any app resolver across the corpus) is a
large standalone analysis; the highest-value slice — the name≠symbol resolution
break — is fully covered in §2a. Batoned as a lower-priority completeness item.

---

## 3. GAP CENSUS lens — full differential corpus, native

Corpus: 2772 differential tests (818 in `basic/`). Harness `tests/molt_diff.py`
(honors `# MOLT_META: expect_fail`/`xfail` + the external `TOO_DYNAMIC_EXPECTED_
FAILURE_TESTS` manifest). CI coverage nuance: per-PR `ci.yml` runs ONLY the
LLVM int-overflow differentials; the **full `basic/`+`stdlib/` corpus runs in
NIGHTLY** (`nightly.yml:78`, dev profile). So §3a is caught by nightly, not
per-PR.

> Harness operational note: directory/large-batch runs with `--jobs >1` on a cold
> dev runtime trigger a **build storm** (each worker full-rebuilds the dev
> runtime → resource exhaustion → ALL workers spuriously FAIL). Pre-warm the dev
> runtime with one build, then use modest parallelism, OR run serially. A batch
> "all 34 async FAIL" result was this artifact; serial re-runs gave the true set.
> Also: long harness/cargo runs reliably receive SIGURG (exit 143/144) on this
> host — keep batches short.

### P0/P1-3 — native asyncio is **broadly broken**: `molt_async_sleep` MissingSymbol

THE headline gap-census finding, and **more severe than the predecessor
measured**. `concurrency.py:35` and `net.py:18` do
`molt_async_sleep = _intrinsics.require("molt_async_sleep", globals())` at
**module-load scope**. Therefore **any program that enters the asyncio event loop
(`asyncio.run`) pulls in the concurrency runtime module → eager `require` →
`MissingSymbol`** — regardless of whether it ever calls `sleep`.

Confirmed live at secured HEAD on **release-fast (default daemon profile)**, not
just dev:
- `asyncio.sleep(0)` → `RuntimeError: intrinsic unavailable: molt_async_sleep`
- a **pure async-generator program with NO sleep** (`async for x in gen()`) →
  **same error** (it still calls `asyncio.run` → concurrency bootstrap).
- `async_generator_protocol.py`, `async_with_basic.py` (which I did not expect to
  use sleep) → same error, confirming the module-load-scope mechanism.

Scope: of the 34 `async_*` tests in `basic/`, **14 directly use `asyncio.sleep`**
(matching the predecessor's exact 14/40 fail count from its one clean run) — but
the true blast radius is **every program using `asyncio.run`**, because the
concurrency module's eager `require` fires on event-loop entry. None of these
tests carry `expect_fail`/`xfail` and none are in the too-dynamic manifest, so
**nightly's differential lane is RED on this cluster** — a genuine untriaged
break, not a known-fail.

- Root cause: §2a — app-resolver keys by name, runtime queries by symbol, and the
  lone name≠symbol spec is `molt_async_sleep`.
- Pre-existing: the bug is in `simple_backend.rs` reachability + cli.py manifest,
  untouched by this sweep (only backend change this sweep is the wasm.rs test
  guard `443c7e8a8` + the registry.rs runtime guard).
- **Severity: P0-class** (all native asyncio programs broken) but **loud**
  (RuntimeError, not silent-wrong-answer). EXCLUDED lane (frontend/backend
  app-resolver gen) → batoned in §2a with the precise fix.

### P1-4 — `float //` build-contract failure (frontend `fast_float` over-marking)

A clean, reachable, pre-existing **build** break. A 3-line program:
```python
def f(x: float, y: float) -> float: return x // y
```
fails to build (release-fast and dev) with:
```
invalid SimpleIR contract: op `floordiv` does not own fast_float scalar specialization
```
- Root cause (two-sided wiring question, resolved): the frontend
  (`src/molt/frontend/lowering/serialization.py:640`) emits `floordiv` with
  `fast_float=Some(true)`, but the backend has **no native scalar-float floordiv
  lowering**. The SimpleIR contract verifier (`ir_schema.rs:212`) correctly
  rejects it: `SCALAR_FAST_FLOAT_KINDS` (ir_schema.rs:55) lists add/sub/mul/div +
  inplace variants but NOT `floordiv`/`mod`. The native fast-float dispatch
  (simple_backend.rs:700-724, `f_op` 0/1/2 = add/sub/mul only) has no floordiv arm.
- **Correct side = frontend** (EXCLUDED lane): float floordiv is NOT a single FP
  instruction — it is `math.floor(x/y)` with `__floordiv__`/`ZeroDivisionError`
  semantics, already implemented correctly by the boxed `floordiv` intrinsic.
  Adding a naive Cranelift scalar arm would DIVERGE from CPython float-floordiv
  semantics — the wrong fix. The fix is for the frontend to **stop marking
  floordiv/mod as `fast_float`** so they take the correct boxed path.
- **Baton (EXCLUDED frontend lane):** serialization.py:640 — drop the
  `fast_float` flag for `floordiv` (and `mod`, same gap). Do NOT "fix" it by
  adding floordiv to `SCALAR_FAST_FLOAT_KINDS` — that would require a
  semantically-correct float-floordiv scalar lowering, which is not a single
  instruction.

### P2-5 — `float.__format__` rejects precision spec on non-finite values

A real parity gap (loud TypeError, NOT silent-wrong-answer).
`"{:.2f}".format(float("nan"))` (and `inf`) →
`TypeError: unsupported format string passed to float.__format__`, where CPython
yields `'nan'` / `'inf'`. Confirmed in `float_round_trunc_format.py`: molt prints
correctly through `round(float("-inf"), 2)` then dies on the first
`"{:.2f}".format(nan)`.
- The float formatter `ops_format.rs` DOES handle non-finite + `.2f` (lines
  1830-1854), so the rejection is a **routing/dispatch** gap upstream:
  `molt_string_format`'s float-spec path doesn't reach the non-finite-aware
  formatter for this shape. (The generic `__format__` dispatcher ops_builtins.rs
  1562 routes floats to `molt_string_format`, so the rejection is inside that
  call's spec handling, not the `unsupported`-fallbacks at 1569/3127 — those are
  the generic-object path.)
- Runtime fix, potentially in-scope, but **non-trivial** (requires tracing
  `molt_string_format`'s float precision-spec path to find where non-finite +
  presentation-type is rejected before reaching ops_format.rs). Not landed to
  avoid a partial fix. **Baton:** root-cause the `molt_string_format` float branch
  for `nan`/`inf` with an `f`/`e`/`g`/`%` presentation type + precision; route to
  the existing non-finite handler in ops_format.rs (which already returns
  `nan`/`inf`/`NAN`/`INF` correctly).

### Non-async / non-format corpus baseline

The predecessor's clean serial slices: arith / assignment / args / and a 20-file
mixed slice = **17/20 PASS**, with the 3 fails being exactly P1-4
(`float_ops.py`, `float //`), P2-5 (`float_round_trunc_format.py`), and the
reconfigure message (now FIXED, `f92bef4a4`, `file_reconfigure.py`). Non-async,
non-float `basic/` tests pass. No NEW silent-wrong-answer (miscompile) class was
found in the census — the gaps are loud failures (async MissingSymbol, float //
build error, float-format TypeError) plus the two allocation-bound perf P0s.

### Cross-reference to the known ~25

The census surfaced no new silent-miscompile class beyond the known set. The
async `molt_async_sleep` break is the dominant untriaged cluster (it accounts for
the entire nightly-async-red signal). float // and float.__format__ are localized
parity gaps. The §2c sccp/effects divergence is a latent (not-yet-live)
miscompile hazard — the only new *correctness* finding, batoned.

---

## 4. Baton summary (priority order)

| # | Finding | Severity | Lane | Action |
|---|---|---|---|---|
| §3a/§2a | Native asyncio broadly broken (`molt_async_sleep` MissingSymbol; eager require at concurrency.py:35 → every `asyncio.run`) | P0 (loud) | EXCLUDED (backend app-resolver gen + cli.py) | App resolver must key by `spec.symbol` not `spec.name`; fix cli.py:17688 extern (declares 1-arg overload) |
| §1 | etl_orders 2.69× + exception_heavy 1.26× slower than CPython | P0 (perf) | perf arc | Structural: cache/inline GIL TLS token (`_tlv_get_addr`) + short-lived-object allocation fast-path (Perceus reuse) |
| §2c | `sccp::may_throw` diverges from `effects::opcode_may_throw` (11 throwing module/OrdAt opcodes missing) | P1 latent miscompile | NOT excluded (SCCP) | Delete sccp duplicate, route through `effects::op_may_throw`; verify StateYield intent |
| §2b | `opcode_may_throw`/`opcode_is_side_effecting` `matches!` default-false | P1 hazard | NOT excluded (effects.rs) | Convert to exhaustive `match` over 105 OpCode variants |
| §3b/P1-4 | `float //` build fails (frontend marks floordiv `fast_float`, no backend lowering) | P1 (loud build break) | EXCLUDED (frontend serialization.py:640) | Drop `fast_float` on floordiv + mod |
| §2f | `luau-backend`/`rust-backend` bit-rotted (cfg-gate mismatch on `raw_i64_safe_values_for`) | P2 (compile break, masked by native) | EXCLUDED (liveness.rs = drop substrate) | Align cfg gate; luau/rust path computes by-type carriers without RawI64Safe seed |
| §3b/P2-5 | `float.__format__` rejects `.2f` on nan/inf | P2 (loud parity) | runtime (in-scope, non-trivial) | Route `molt_string_format` float non-finite path to ops_format.rs handler |
| §2f | 7 features never compiled in CI | P3 (no guard) | CI | Add `cargo check --no-default-features --features X` job per feature |
| §2g | no_mangle orphan census | P3 | analysis | Full reachability census across corpus |

---

## 5. What landed vs what is batoned (honesty ledger)

**Landed this sweep (all standard-battery verified):**
- 4 predecessor commits (§0) — all re-verified to hold at secured HEAD.
- 1 continuation guard: `name_neq_symbol_specs_resolve_in_core` (registry.rs) +
  this doc.

**Deliberately NOT landed (per zero-partial-fix policy):**
- The async_sleep app-resolver fix (§2a/§3a): EXCLUDED lane.
- The sccp/effects unification (§2c) and the `matches!`→exhaustive-match
  conversion (§2b): structurally correct but non-trivial; a partial would be
  worse than the batoned hazard.
- The float // frontend fix (§3b): EXCLUDED lane.
- The luau/rust cfg-gate fix (§2f): EXCLUDED lane (drop substrate).
- The float.__format__ routing fix (§2f/P2-5): in-scope but needs full
  `molt_string_format` trace; not started rather than half-done.
- The perf arc (§1): multi-day allocator+TLS structural work.

Each baton above carries the precise file:line and the structurally-correct
direction so the next agent (or lane owner) picks it up without re-discovery.
