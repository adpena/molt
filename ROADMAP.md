# Molt Roadmap (Active)

## Performance
- Vector reduction kernels now cover `sum`/`prod`/`min`/`max` with trusted fast paths; next up: float reductions and typed-buffer kernels.
- String kernel SIMD paths cover find/split/replace with Unicode-safe index translation; next: Unicode index caches and wider SIMD.

## Type Coverage
- memoryview (Partial): constructor, slicing, `tobytes`, writable views, strides, buffer export.
- TODO(type-coverage, owner:runtime, milestone:TC3): memoryview format codes and multidimensional shapes.
- Implemented: class objects + basic descriptors (`classmethod`, `staticmethod`, `property`).
- Implemented: exception chaining with explicit `__cause__`, implicit `__context__`, and `__suppress_context__`.
- TODO(type-coverage, owner:runtime, milestone:TC2): inheritance semantics + full descriptor protocol (setters/deleters, `super`).
- Partial: wasm generator state machines + closure slot intrinsics + async pending/block_on parity landed; remaining generator state object/StopIteration work and full async iteration/scheduler semantics.

## Tooling
- Keep type facts + `ty` validation wired into build/lint flows and surface regressions early.
- Track complex performance work in `OPTIMIZATIONS_PLAN.md` before large refactors.
