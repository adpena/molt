# Runtime-wasm dual-compile: findings & corrected plan (2026-07-10)

Task #20 ("kill the dual runtime-wasm rebuild") attempted a **single-compile split
runtime**: one `cargo rustc --crate-type=staticlib` compile, then two `wasm-ld`
links (reloc `-r`; shared via a hand-rolled cdylib recipe). It was **reverted** —
M05 verification caught a correctness regression AND the perf premise is unproven.

## What was measured (isolated caches, `--kind both`, release-output)
| mode | wall | shared exports | shared size |
|---|---|---|---|
| single-compile (hand-link) | 332s | **2181** | 33.67 MB |
| dual-compile (rustc cdylib) | 343s | **3260** | 33.24 MB |

## Finding 1 — the hand-link REGRESSES exports (blocker, why it was reverted)
The hand-linked shared runtime is **missing 1079 exports** the rustc cdylib keeps,
including `Py_IncRef`/`Py_DecRef`, every `__molt_collections_*` / `__molt_logging_*`
/ `__molt_asyncio_*`, and the whole `molt_PyBytes_*` / `molt_PyCapsule_*` / `molt_PyCFunction_*`
C-API surface. **Root cause:** rustc's cdylib link enumerates EVERY `#[no_mangle]`
symbol as an explicit `--export` (the ~3260-entry list in the captured
`--print=link-args`). The hand-link only passed the curated
`wasm_runtime_shared_export_link_args` allowlist (2181) and `--gc-sections`
stripped the other 1079. The split-runtime app imports those by name → this
runtime would fail to link the app. Imports were fine (92 == 92, memory+table
present); the defect is purely the export enumeration.

**Lesson:** the shared runtime's export contract is "all no_mangle symbols," not a
curated allowlist. Any hand-link must reproduce rustc's full no_mangle enumeration
— fragile to maintain.

## Finding 2 — the perf premise is UNPROVEN in the integrated flow
Walls were ~equal (332 vs 343s; the dual run was also under concurrent build
contention). The prior lane's "dual = 384.7s, single-compile saves ~115-205s" came
from **isolated per-scenario caches that prevented cargo's own cross-crate-type
codegen sharing**. In the real `--kind both` flow both passes use one target dir,
so the second pass is largely a re-link, not a full recompile — the "wasted second
compile" is much smaller than the isolated measurement implied. **Not yet measured
per-phase**, so no win is claimed (M05/M10: no unmeasured perf).

## Corrected plan (measure-first, then design B if warranted)
1. **Instrument per-phase** in ONE quiet (uncontended) window: wall around the
   cargo compile vs each `wasm-ld` link vs `wasm-opt`, and whether the reloc pass
   recompiles the crate or cargo-cache-hits. This tells us where the ~330s
   actually goes (compile vs the -O2 link/opt on 30-50 MB modules) and whether a
   second compile is real headroom at all.
2. **Only if the second compile is proven costly → Design B** (not the hand-link):
   make the reloc and shared passes issue an RUSTFLAGS-identical cargo invocation
   (the shared import link-args are inert to the staticlib the reloc pass consumes)
   so cargo compiles the crate ONCE and the second pass cache-hits — while keeping
   **rustc's authoritative cdylib link** (correct 3260-export enumeration). No
   fragile hand-reproduction of the export set.
3. If per-phase shows `wasm-opt`/link dominates, retarget the P0 there instead.

The diagnostic `MOLT_RUNTIME_WASM_PRINT_LINK_ARGS=1` (prints rustc's link line) is
worth keeping for future link parity work.
