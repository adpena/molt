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
### Phase 0: Enforcement Spine (Immediate)
1. Keep generated intrinsic audit (`tools/check_stdlib_intrinsics.py`) in CI.
2. Enforce strict core-lane lowering gate (`tools/check_core_lane_lowering.py`):
   only `intrinsic-backed` modules are allowed in the core lane import closure.
3. Keep differential core lane as the primary bring-up lane before stdlib lane.

Exit criteria:
- Audit is current and green in CI.
- Core-lane lowering gate is green.

### Phase 1: Core-Lane Blockers (P0)
Lower all modules currently pulled by `tests/differential/core/TESTS.txt` to
`intrinsic-backed` only, starting with bootstrap-critical modules:
1. `types`
2. `abc` / `_abc` / `collections.abc` / `_collections_abc`
3. `weakref` / `_weakrefset`
4. `typing`
5. `traceback`
6. `__future__`
7. `asyncio` (only core-lane-used surface first, then complete module work in
   Phase 3)

Exit criteria:
- Core lane differential tests pass.
- Core lane import closure has zero `probe-only`, `intrinsic-partial`, and
  `python-only` modules.

### Phase 2: Runtime Concurrency Substrate (P0)
Implement in this order for correctness and dependency flow:
1. `socket` full lowering (addressing, stream/datagram semantics, options,
   errors).
2. `threading` full lowering (primitives, thread lifecycle, locality semantics).
3. `asyncio` full lowering on top of completed socket/threading/runtime poller.

Exit criteria:
- Socket/threading/asyncio differential clusters are green in native + wasm.
- No Python-side semantic fallback in these modules.

### Phase 3: Core-Adjacent Stdlib (P1)
Lower performance- and semantics-critical modules:
- `builtins`, `math`, `re`, `struct`, `time`, `inspect`, `functools`,
  `itertools`, `operator`, `contextlib`.

Exit criteria:
- Module families move from `intrinsic-partial`/`python-only` to
  `intrinsic-backed`.
- Targeted differential suites are green with RSS tracking and memory limits.

### Phase 4: Capability-Gated And Long-Tail Stdlib (P2/P3)
Lower remaining modules by capability domain with deterministic behavior:
1. Filesystem/import tooling (`pathlib`, `importlib.*`, `pkgutil`, `glob`,
   `shutil`, `py_compile`, `compileall`).
2. Data/codecs (`json`, `csv`, `pickle`, `enum`, `ipaddress`, encodings family).
3. Networking/process edges (`ssl`, `subprocess`, `concurrent.futures`,
   remaining `http.*`).

Exit criteria:
- Audit has no `python-only` modules for shipped compiled surface.
- Capability-gated behavior is explicit and documented.

## Tracking And Reporting
- Source-of-truth audit: `docs/spec/areas/compat/0016_STDLIB_INTRINSICS_AUDIT.md`
- Core lane list: `tests/differential/core/TESTS.txt`
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
