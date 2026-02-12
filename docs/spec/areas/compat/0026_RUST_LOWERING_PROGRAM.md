# Rust Lowering Program (Core To Full Stdlib)
**Spec ID:** 0026
**Status:** Draft (execution plan)
**Owner:** runtime + stdlib + frontend + tests

## Objective
Eliminate Python-side semantic implementations from compiled Molt binaries by
lowering required behavior into Rust intrinsics for both native and wasm
targets. Python stdlib files are wrappers only: argument normalization, error
mapping, and capability gating.

## Non-Negotiable Gates
- No host-Python/CPython fallback in compiled binaries.
- No `_py_*` fallback modules in compiled binaries.
- Missing required intrinsic must fail fast (`RuntimeError`/`ImportError`).
- Core lane import closure must be fully lowered (`intrinsic-backed` only).
- Native and wasm behavior must move in lockstep for lowered features.

## Execution Order
### Program Board (2026-02-06)
| Phase | Scope | Status | Entry Gate | Exit Gate |
| --- | --- | --- | --- | --- |
| 0 | Enforcement spine | Completed | Intrinsics audit in CI | `tools/check_stdlib_intrinsics.py` + `tools/check_core_lane_lowering.py` green in CI |
| 1 | Core-lane closure lowering | Completed | Phase 0 complete | `tests/differential/basic/CORE_TESTS.txt` green + closure contains only `intrinsic-backed` modules |
| 2 | Concurrency substrate | In progress (active) | Phase 1 complete | `socket` + `threading` + `asyncio` clusters green in native and wasm without Python-side semantics |
| 3 | Core-adjacent stdlib | Planned | Phase 2 complete | target families promoted to `intrinsic-backed` |
| 4 | Capability-gated long tail | Planned | Phase 3 complete | shipped compiled surface has no `python-only` modules |

### Phase 0: Enforcement Spine (Completed, keep strict)
1. Keep generated intrinsic audit (`tools/check_stdlib_intrinsics.py`) in CI and local lint (`tools/dev.py lint`).
2. Keep strict core-lane lowering gate (`tools/check_core_lane_lowering.py`) in CI.
3. Keep differential core lane as fail-fast gate before broader stdlib sweeps.

### Phase 1: Core-Lane Blockers (Completed, maintain)
Completed bootstrap-critical lowering for the core differential lane import closure:
1. `types`
2. `abc` / `_abc` / `collections.abc` / `_collections_abc`
3. `weakref` / `_weakrefset`
4. `typing`
5. `traceback`
6. `__future__`
7. core-used `asyncio` surface

Maintenance rule: no new core-lane test may introduce `intrinsic-partial`, `probe-only`, or `python-only` imports.

### Phase 2: Runtime Concurrency Substrate (P0, active queue)
Work in strict dependency order:
1. `socket` + `select` + `selectors`
2. `threading`
3. `asyncio`

#### 2.1 Socket/Select tranche (first unblocker)
- Required lowering: socket construction/connect/bind/listen/accept/send/recv/sendall/recv_into/setsockopt/getsockopt/shutdown/dup/detach/timeouts/error mapping.
- Required substrate: deterministic poller integration (`select`/`selectors`) with capability-gated I/O.
- Required tests: `tests/differential/stdlib/socket_*.py`, relevant `select*`/`selectors*` stdlib-lane cases, and native+wasm parity checks.
- Exit rule: socket/select/selectors are `intrinsic-backed` (or explicitly capability-gated with intrinsic-only implementations) and no Python semantic fallback path remains.

#### 2.2 Threading tranche (second unblocker)
- Required lowering: thread lifecycle, ids, lock/rlock/event/condition/semaphore primitives, thread-local behavior, and error parity.
- Required tests: `tests/differential/stdlib/threading_*.py` cluster + stress reruns with RSS tracking.
- Exit rule: threading primitives and lifecycle are intrinsic-backed and deterministic under the runtime lock model.

#### 2.3 Asyncio tranche (third unblocker)
- Required lowering: event loop core, transports/protocol adapters, task/future/wait/gather semantics, callbacks/readiness, cancellation/error propagation.
- Dependency: only start full asyncio lowering after socket/select/selectors and threading tranches are green.
- Required tests: `tests/differential/stdlib/asyncio_*.py` + async long-running/stability cases in native and wasm.
- Exit rule: asyncio surface is intrinsic-backed for compiled execution and no Python-side semantic fallback remains.

### Phase 3: Core-Adjacent Stdlib (P1)
Lower performance/semantics-critical families:
- `builtins`, `math`, `re`, `struct`, `time`, `inspect`, `functools`, `itertools`, `operator`, `contextlib`.

Exit criteria:
- Families move from `intrinsic-partial`/`python-only` to `intrinsic-backed`.
- Targeted differential suites are green with RSS tracking and memory limits.

### Phase 4: Capability-Gated And Long-Tail Stdlib (P2/P3)
Lower remaining modules by deterministic capability domain:
1. Filesystem/import tooling (`pathlib`, `importlib.*`, `pkgutil`, `glob`, `shutil`, `py_compile`, `compileall`).
2. Data/codecs (`json`, `csv`, `pickle`, `enum`, `ipaddress`, encodings family).
3. Networking/process edges (`ssl`, `subprocess`, `concurrent.futures`, remaining `http.*`).

Exit criteria:
- Audit has no `python-only` modules for the shipped compiled surface.
- Capability-gated behavior is explicit, intrinsic-backed, and documented.

## Tracking And Reporting
- Source-of-truth audit: `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`
- Core lane list: `tests/differential/basic/CORE_TESTS.txt`
- Stdlib lane list: `tests/differential/stdlib/TESTS.txt`
- Status roll-up: `docs/spec/STATUS.md`
- Sequencing and milestones: `docs/ROADMAP.md`

## Safety And Performance Requirements
- Keep runtime mutation behind the GIL token and runtime safety invariants.
- Preserve deterministic behavior and capability gating.
- For each phase completion, run:
  - core/targeted differential tests with RSS enabled,
  - `tools/dev.py lint`,
  - `tools/dev.py test`,
  - relevant Rust tests for touched runtime areas.
