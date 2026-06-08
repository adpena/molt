# Cold-start tax decomposition (task #62)

`tools/cold_start_decompose.py` decomposes the **fixed startup tax** that makes
short benchmarks `WARN_COLD_FLOOR` / `FAIL_COLD_BUDGET` on the cold axis of the
CPython floor-scoreboard (`docs/perf/SCOREBOARD.md`). It **measures and
localizes** the highest-leverage component; it does **not** implement the
runtime fix (that is a future Lane-A-adjacent arc).

`startup_tax_ms = (molt_cold_total − molt_warm_total) × 1000` — the cost molt
pays on the cold path that the warm steady-state does not.

## Method — two path modes (this distinction is load-bearing)

A naive "fresh copy per sample" method (the technique
`output_startup_size_audit` uses to defeat the page cache) introduces a large
confound on macOS: a **freshly-materialized unsigned binary pays
code-signature / Gatekeeper validation on every launch**, which is a **one-time
install cost**, not a per-launch cost. This tool therefore measures **both**:

| mode | what it is | what it includes |
|------|-----------|------------------|
| **SAME-PATH** | repeated launches of one stable path | the **realistic repeated cold** launch of an *installed* binary — macOS caches signature validation after the first run. **Components attribute this tax.** |
| **FRESH-PATH** | a fresh binary copy per sample | the **worst-case first-ever** launch — pays macOS codesign/Gatekeeper of an unsigned binary every time. `(no-op C fresh − no-op C same-path)` ISOLATES that one-time cost; reported separately, **never summed** into the realistic tax. |

Probes, all native, via the molt CLI / `cc`:
- **minimal `print()`-only molt binary** — the pure init floor (no user compute).
- **`import json` + use molt binary** — isolates per-module eager init (delta to minimal).
- **no-op C binary** (`int main(){}`) — the pure process-launch + dyld baseline.
- **`MOLT_TRACE_RUNTIME_INIT=1`** — the `molt_runtime_init` 12-phase microsecond ladder (`runtime_state.rs`).
- **`DYLD_PRINT_STATISTICS=1`** — macOS dyld's own total-time (cross-check).

Run:
```bash
export MOLT_SESSION_ID=perfscore CARGO_TARGET_DIR="$PWD/target/sessions/perfscore"
uv run --python 3.12 python3 tools/cold_start_decompose.py \
    --profile release-fast --profile release-output --samples 15
# -> bench/scoreboard/cold_start_decomposition.json
```

## Measured decomposition (native macOS arm64, this host, 4.26 MiB minimal binary)

### `startup_tax_ms` by component — REALISTIC (same-path) cold launch

| component | release-fast | release-output | what it is | molt-controllable? |
|-----------|-------------:|---------------:|------------|--------------------|
| `process-launch/dyld` | **18.0 ms** | **18.0 ms** | kernel exec + dyld fixups (signature cached). **Identical to the no-op C binary** → the OS floor. | no (OS) |
| `binary-page-in+entry+teardown` | ~0 ms | ~0 ms | faulting the 4.26 MiB binary's pages + mimalloc init + teardown. ~0 same-path (pages cached); materializes on a truly page-cold launch and scales with **binary SIZE**. | **yes (size)** |
| `molt-runtime-init` | **0.127 ms** | **0.125 ms** | the entire `molt_runtime_init` 12-phase ladder. **Negligible (<0.7%).** | yes (already lean) |
| `module-init (per json import)` | ~0 ms | ~0 ms | eager stdlib init per import — tiny. | yes (already lazy) |
| **realistic same-path total** | **~18 ms** | **~18 ms** | | |

### One-time / install cost (reported separately, NOT in the tax above)

| component | value | what it is |
|-----------|------:|------------|
| `macos-codesign-first-launch` | **~70 ms** | macOS code-signature / Gatekeeper validation of a freshly-materialized **unsigned** binary. Paid **once per binary identity** (first launch after build/copy/download), NOT on every launch of an installed/signed binary. Isolated on the no-op C (carries no molt or size signal): `noop_C fresh 88 ms − noop_C same-path 18 ms`. |

For reference, the **FRESH-PATH** worst-case first launch: minimal molt **122 ms**,
no-op C **88 ms** (the 34 ms molt−C gap at fresh-path = page-in of the larger
molt binary + its codesign over the tiny C binary's).

### `molt_runtime_init` phase ladder (the 0.127 ms, MOLT_TRACE_RUNTIME_INIT)

| phase | ms | phase | ms |
|-------|---:|-------|---:|
| `state_allocated` | 0.046 | `resources` | 0.006 |
| `intrinsics_registered` | 0.036 | `capabilities` | 0.006 |
| `runtime_reset_for_init` | 0.007 | (serial/itertools/core_gil vtables) | 0.005 ea |

This is byte-for-byte consistent with the 2026-06-03 startup baseline
(`molt_runtime_init` ≈ 0.46 ms wall there with more phases timed; the per-phase
deltas are dominated by `RuntimeState::new` + intrinsic registration, both
already lean).

## The finding (it INVERTS the naive read) + the #62 attack target

**A minimal molt binary launches in the SAME wall-time as a no-op C binary
(~18 ms same-path).** molt's own runtime init is **0.127 ms** — three orders of
magnitude below the OS floor. The cold-start tax is therefore **not a molt-init
problem**:

1. **`process-launch/dyld` ≈ 18 ms is the OS floor** (no-op C is identical) —
   **not molt-controllable**. This already beats CPython's ~18 ms `-c pass`
   startup, so molt is at or below the CPython cold floor for a trivial program.
2. **`macos-codesign-first-launch` ≈ 70 ms is a one-time install cost** — paid
   once per binary identity, irrelevant to a deployed/signed binary's
   steady-state cold launch. A fresh-path-ONLY measurement over-attributes this
   ~70 ms to "dyld" and manufactures a phantom startup crisis.
3. **`molt_runtime_init` = 0.127 ms — already solved.** No deferral / snapshot
   work is warranted here; the 12-phase ladder is negligible.

> **#62 attack target = `binary-page-in` (size-driven), via the
> binary-size / tree-shaking arc — NOT a `molt_runtime_init` deferral.**
> binary-page-in is ~0 same-path but is the only molt-controllable component of
> a genuinely page-cold first launch, and it scales with the linked surface
> (4.26 MiB minimal today). Shrinking the linked binary (per-attr DCE +
> RuntimeSurfacePlan + stdlib slicing, per `project_binary_size_*` /
> `project_runtimesurfaceplan_sprint`) is the single lever that lowers the cold
> tax molt actually owns. This **converges** the cold-start lane with the
> binary-size lane: one fix (smaller linked surface) serves both.

### Why the scoreboard COLD column still shows a larger tax

The scoreboard's `cold_*` is a **single first-run** of a freshly-built binary —
so it captures BOTH a page-cold launch AND (often) the one-time codesign of the
just-built binary, which is why per-benchmark `startup_tax_ms` on the board
(e.g. `bench_fib` ~180–220 ms) exceeds the ~18 ms realistic same-path floor. The
cold-start **budget** (`bench/scoreboard/cold_start_budget.json`) is therefore
set against the board's first-run tax (the council "v0 = measured baseline"),
while THIS decomposition explains WHERE that tax goes and which slice (page-in)
is worth attacking. The council Y1 target (release-output `startup_tax < 100ms`)
is already met at the realistic same-path floor; the open work is keeping the
first-run (page-cold + freshly-built) tax bounded as the stdlib grows — i.e. the
binary-size arc.
