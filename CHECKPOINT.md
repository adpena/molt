Checkpoint: 2026-01-07 14:45:40 CST
Git: 66769a9 runtime: cargo fmt

Summary
- Refactored shims runtime binding setup and concurrency branches to avoid backend SSA/dominance failures.
- Reworked stdlib os/io/warnings/collections shims (init helpers, normpath early-return, capability parsing loop, view stubs, deferred builtins usage) to unblock compiled runs.
- Adjusted net runtime senders and typing (StreamSenderBase hierarchy, payload validation) and clarified backend attr errors.
- Bench channel throughput now builds/runs; refreshed bench results.

Files touched (uncommitted)
- bench/results/bench.json
- runtime/molt-backend/src/lib.rs
- src/molt/capabilities.py
- src/molt/frontend/__init__.py
- src/molt/net.py
- src/molt/shims.py
- src/molt/stdlib/collections/__init__.py
- src/molt/stdlib/collections/abc.py
- src/molt/stdlib/importlib/__init__.py
- src/molt/stdlib/io.py
- src/molt/stdlib/os.py
- src/molt/stdlib/pathlib.py
- src/molt/stdlib/types.py
- src/molt/stdlib/warnings.py

Tests run
- PYTHONPATH=src uv run --python 3.12 python3 tests/molt_diff.py tests/differential/basic
- PYTHONPATH=src uv run --python 3.12 pytest tests/test_wasm_control_flow.py
- PYTHONPATH=src uv run --python 3.12 python3 tools/dev.py lint
- PYTHONPATH=src uv run --python 3.12 python3 tools/dev.py test
- PYTHONPATH=src uv run --python 3.14 python3 tools/bench.py --json-out bench/results/bench.json

Known gaps
- OPT-0005/6/7 perf follow-through still pending (bench_fib/bench_struct/str_count_unicode_warm remain slow vs CPython).
- Builtins module import remains unimplemented; stdlib shims now avoid module-level builtins imports but a proper builtins module should still land.

Pending changes
- bench/results/bench.json
- runtime/molt-backend/src/lib.rs
- src/molt/capabilities.py
- src/molt/frontend/__init__.py
- src/molt/net.py
- src/molt/shims.py
- src/molt/stdlib/collections/__init__.py
- src/molt/stdlib/collections/abc.py
- src/molt/stdlib/importlib/__init__.py
- src/molt/stdlib/io.py
- src/molt/stdlib/os.py
- src/molt/stdlib/pathlib.py
- src/molt/stdlib/types.py
- src/molt/stdlib/warnings.py

Next 5-step plan
1) Commit the compiler/shim fixes plus refreshed bench.json and push.
2) Monitor CI and fix any failures until green.
3) Start OPT-0005/6/7 work on fib/struct/str-count warm path hotspots.
4) Add async yield spill probes (compare chains, call args) and verify wasm parity.
5) Update docs/spec/STATUS.md and ROADMAP.md if behavior scope changes.
