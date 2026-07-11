# Startup Baseline

Status: **fail-closed baseline recorded; Molt release cells blocked**  
Claim: `STARTUP-BASELINE`  
Attestation: `docs/agent/evidence/startup/startup_baseline.json`  
Proof row: `20260711T171843-startup-baseline-010b0b8cdd484105`

This baseline composes with `docs/design/foundation/62_startup_cold_start.md`.
It does not create a second cold-start authority: `tools/startup_bench.py`
reuses `tools/output_startup_size_audit.py` for canonical builds and adds the
workload matrix, release evidence grade, native runtime-init phases, and Node
linked/split phase attribution requested by the startup arc.

## Method

- Machine: Windows 11, AMD64 Family 23 Model 113, Python 3.12.13, Node 24.
- Build profile: release only; no dev-profile numbers are accepted.
- Samples: five serial process launches; reported statistic is the median.
- CPython: `C:\Molt\molt-src\.venv\Scripts\python.exe -I`, with project
  `PYTHONPATH`, `PYTHONHOME`, and `UV_PROJECT_ENVIRONMENT` removed.
- Molt native: canonical `molt build --build-profile release --trusted
  --stdlib-profile micro`, followed by direct executable wall time and
  `MOLT_TRACE_RUNTIME_INIT=1` phase medians.
- Molt WASM: canonical linked release build, Node end-to-end wall time, and a
  preload probe that attributes `.wasm` reads, each V8 instantiate call, first
  stdout, and exit without modifying `wasm/run_wasm.js`.
- Evidence grade: A12 release median baseline. Variant-II acceptance applies
  only when a before/after startup improvement is claimed.

## Matrix

All values are milliseconds. `BLOCKED` is not a zero and is not a performance
result.

| Probe | CPython median | Molt native median | Node boot median | Molt linked WASM | Molt split WASM |
|---|---:|---:|---:|---:|---:|
| `hello` | 58.274 | BLOCKED | 39.085 | BLOCKED | BLOCKED |
| `small_compute` | 137.179 | BLOCKED | 39.085 | BLOCKED | BLOCKED |

CPython samples:

- `hello`: 65.519, 58.274, 55.052, 64.756, 52.529.
- `small_compute`: 144.321, 131.123, 138.578, 134.208, 137.179.
- Node boot: 42.521, 39.085, 38.652, 38.871, 41.889.

## Release Blocker

Both release probes fail before link or execution in
`runtime/molt-cpython-abi/src/api/typeobj.rs`. The crate denies
`unsafe_op_in_unsafe_fn`, while three calls to `PyErr_BadInternalCall` are not
inside explicit unsafe blocks (lines 59, 1939, and 1972 at commit
`d36658a1ced5ce10e82d6a2e6ec20e51fec11168`). The same failure occurs with
`stdlib-profile=full` and `stdlib-profile=micro`.

The startup lane did not weaken the lint, patch the reserved CPython-ABI lane,
or substitute stale artifacts. Therefore:

- Native-vs-CPython verdict: **unknown**, not a win or loss.
- Native pre-main/runtime-init breakdown: **unavailable**.
- WASM read/compile/instantiate/runtime-init breakdown: **unavailable**.
- Browser streaming instantiate: harness exists in `wasm/browser_host.js`, but
  no current release artifact exists to measure.
- Witness/import-heavy startup: not measured because the release build never
  reached artifact production.

## Ranked Ladder

1. **Restore release buildability.** This is the gating dependency for every
   native and WASM startup cell. Until it is green, no startup claim is citable.
2. **Remove duplicate full-byte WASM scans before instantiation.** The Node
   runner reads app/linked/runtime bytes, then separately runs import parsing
   and export-signature parsing before V8 instantiation. The preload probe will
   quantify the resulting pre-instantiate tax once artifacts exist.
3. **Attribute artifact footprint by section.** Record code, data, name/debug,
   and custom-section bytes beside read and instantiate medians. Large active
   data segments impose eager initialization; name/debug sections impose parse
   bytes but should be owned by the concurrent publication-strip lane.
4. **Keep runtime initialization below the process floor.** The runtime already
   exposes a twelve-stage trace. Historical design evidence places total init
   near 0.125 ms, so eager-runtime work is lower priority unless the new trace
   disproves that. `RuntimeState::new` creates many empty registries, but no
   oversized startup reservation was proven in this run.
5. **Measure browser streaming separately.** Node file-read startup and browser
   `instantiateStreaming` answer different questions. Browser evidence belongs
   in a dedicated loader run with network/cache state recorded.

## Budget Gate

`bench/scoreboard/startup_budget.json` is warn-only until complete release
medians are available. `--strict` promotes warn checks for release gating. The
first seeded invariant is native `hello` below CPython `hello`; linked-WASM gets
an absolute instantiate-and-execute ceiling only after a valid baseline.

Run:

```powershell
uv run --active --project . --python 3.12 python tools/startup_bench.py --samples 5 --output bench/results/startup_baseline.json
```

The command exits nonzero and writes a refused attestation whenever a release
artifact, five-sample median, or required budget cell is unavailable.
