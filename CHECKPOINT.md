Checkpoint: 2026-01-07 12:46:44 CST
Git: 9557241 docs: refresh checkpoint

Summary
- Added guarded direct-call lowering (`CALL_GUARDED`) for named functions (non-async) and backend support.
- Decoupled Unicode count cache from UTF-8 index cache to avoid expensive prefix builds on first count.
- Added exact-local tracking to skip guarded setattr for constructor-bound locals with fixed layouts (non-dataclass).

Files touched (uncommitted)
- src/molt/frontend/__init__.py
- tests/differential/basic/async_yield_spill.py
- runtime/molt-backend/src/lib.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/cli.py
- main_stub.c
- OPTIMIZATIONS_PLAN.md

Tests run
- PYTHONPATH=src uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic

Known gaps
- Async yield spill audit still pending for deeper compare chains and wasm parity.
- OPT-0005/6/7 needs benchmark validation (fib/struct/unicode count benches).

Pending changes
- CHECKPOINT.md (this update)
- OPTIMIZATIONS_PLAN.md
- main_stub.c
- runtime/molt-backend/src/lib.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/cli.py
- src/molt/frontend/__init__.py
- tests/differential/basic/async_yield_spill.py

Next 5-step plan
1) Re-run differential suites covering async/coroutine semantics after any further yield-spill changes.
2) Continue OPT-0005/6/7 implementation (direct-call lowering, unicode count cache metadata, struct slot stores).
3) Add more async yield spill probes (compare chains, nested boolops, call args) and fix dominance gaps.
4) Decide on keeping main_stub.c in sync with CLI or removing to avoid divergence.
5) Update docs/spec/STATUS.md if global/async semantics scope expanded.
