# Molt Roadmap (Active)

For current supported state, use [docs/spec/STATUS.md](docs/spec/STATUS.md).
This file is forward-looking only.

## Strategic Target

- Reach full CPython `>=3.12` parity for the supported Molt subset.
- Ship standalone binaries with no hidden host Python installation fallback.
- Outperform CPython on the benchmark suites Molt claims as core product lanes.
- Preserve Molt's design exclusions around runtime monkeypatching,
  unrestricted dynamic execution, and unrestricted reflection.

## Current Priorities

1. Close correctness gaps in the compiler/runtime path before claiming broader
   compatibility.
2. Replace hint-driven backend recovery with a shared representation-aware
   backend contract so native, WASM, and future LLVM lowering optimize from the
   same typed facts.
3. Drive native and WASM toward the same supported contract.
4. Simplify tooling and developer workflow around build, daemon, and validation.
5. Make performance reporting and compatibility reporting generator-owned
   instead of manually synchronized across multiple docs.

## Milestone Sequence

### Near Term

- Finish the documentation-architecture cleanup and turn doc ownership into CI
  policy.
- Tighten compatibility rollups around generated evidence.
- Make typed SSA / explicit representation facts survive lowering without
  degrading into transport-only hints.
- Close the highest-value native and WASM parity blockers.

### Medium Term

- Expand language and stdlib coverage under the Rust-first lowering model.
- Retire `fast_int` / `fast_float` / `raw_int` as the architectural center of
  backend optimization in favor of a shared representation-aware lowering path.
- Harden daemon, build, and harness workflows for multi-agent development.
- Move more hot semantics into runtime primitives and intrinsics.

### Long Term

- Broaden extension support through `libmolt`.
- Push native and WASM performance toward the project target.
- Continue converging on a larger practical CPython 3.12+ surface without
  regressing determinism or packaging guarantees.

## Active Blockers

- Incomplete same-contract parity between native and WASM for important surfaces.
- Incomplete compatibility coverage across language and stdlib.
- Current backend lowering still degrades typed representation facts into
  transport hints before codegen.
- Benchmark suite results are not yet consistently faster than CPython across
  all tracked lanes.

## Deferred By Policy

- Unrestricted `exec` / `eval` / `compile`.
- Runtime monkeypatching as a default compatibility strategy.
- Hidden host-CPython fallback paths in compiled binaries.
- Unrestricted reflection that violates Molt's AOT constraints.
