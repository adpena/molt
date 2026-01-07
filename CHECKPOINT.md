Checkpoint: 2026-01-07 11:36:33 CST
Git: 3fc8408 rustfmt: fix backend clif dump

Summary
- Fixed async channel resume dominance by introducing STATE_LABEL and limiting state-switch resume targets; added MOLT_DUMP_CLIF for backend IR dumps.
- Reloaded async locals after channel yields in augmented assignment via _expr_may_yield detection.
- Implemented tuple()/bytes() conversions and TUPLE_FROM_LIST/BYTES_FROM_OBJ ops with runtime helpers and iterator guard paths.
- Added bench coverage (async await, channel throughput, dict views, list/tuple slice/pack, bytearray find/replace) and updated BENCHMARKS + bench/results/bench.json.
- rustfmt cleanup for the CLIF dump condition; CI run 20790552040 green.

Files touched (committed in 7670b9f + 3fc8408)
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/frontend/__init__.py
- tests/wasm_harness.py
- wit/molt-runtime.wit
- tools/bench.py
- tests/benchmarks/bench_async_await.py
- tests/benchmarks/bench_bytearray_find.py
- tests/benchmarks/bench_bytearray_replace.py
- tests/benchmarks/bench_channel_throughput.py
- tests/benchmarks/bench_dict_views.py
- tests/benchmarks/bench_list_slice.py
- tests/benchmarks/bench_tuple_pack.py
- tests/benchmarks/bench_tuple_slice.py
- bench/results/bench.json

Tests run
- uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json
- uv run --python 3.12 python3 tools/dev.py lint
- uv run --python 3.12 python3 tools/dev.py test
- cargo test
- cargo fmt

Known gaps
- Async yield spill coverage beyond augmented assignment still needs audit/tests.

Pending changes
- CHECKPOINT.md (this update)

Next 5-step plan
1) Monitor CI for 3fc8408 and fix any regressions.
2) Audit async yield spill across binops/calls and add coverage for await + channel cases.
3) Revisit fib/struct benches with OPT-0005/6/7 follow-through and profiling counters.
4) Refresh STATUS/README/ROADMAP if async semantics scope shifts.
5) Check wasm parity for any state_label/resume edge cases.
