Checkpoint: 2026-01-17T19:08:45Z
Git: ac8ca9b528119e74fbe00e013a080c07912215c2 (dirty)

Summary
- Replaced traceback registry/tb entries with code objects + line markers and slot-based tracing for direct calls.
- Updated frontend IR, native/wasm backends, intrinsics, and WIT for new code/trace APIs.
- Updated traceback differential test and spec/roadmap notes for call-site line info + code object gaps.

Files touched (uncommitted)
- AGENTS.md
- CHECKPOINT.md
- Cargo.lock
- GEMINI.md
- OPTIMIZATIONS_PLAN.md
- README.md
- ROADMAP.md
- bench/results/bench.json
- bench/results/bench_wasm.json
- docs/AGENT_LOCKS.md
- docs/AGENT_MEMORY.md
- docs/BENCHMARKING.md
- docs/benchmarks/bench_summary.md
- docs/spec/0012_MOLT_COMMANDS.md
- docs/spec/0014_TYPE_COVERAGE_MATRIX.md
- docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md
- docs/spec/0019_BYTECODE_LOWERING_MATRIX.md
- docs/spec/0020_RUNTIME_SAFETY_INVARIANTS.md
- docs/spec/0021_SYNTACTIC_FEATURES_MATRIX.md
- docs/spec/0023_SEMANTIC_BEHAVIOR_MATRIX.md
- docs/spec/0400_WASM_PORTABLE_ABI.md
- docs/spec/STATUS.md
- runtime/molt-backend/src/lib.rs
- runtime/molt-backend/src/wasm.rs
- runtime/molt-obj-model/Cargo.toml
- runtime/molt-obj-model/benches/ptr_registry.rs
- runtime/molt-obj-model/src/lib.rs
- runtime/molt-runtime/fuzz/Cargo.lock
- runtime/molt-runtime/fuzz/Cargo.toml
- runtime/molt-runtime/fuzz/fuzz_targets/string_ops.rs
- runtime/molt-runtime/src/arena.rs
- runtime/molt-runtime/src/lib.rs
- src/molt/_intrinsics.pyi
- src/molt/cli.py
- src/molt/concurrency.py
- src/molt/frontend/__init__.py
- src/molt/net.py
- src/molt/shims.py
- src/molt/shims_cpython.py
- src/molt/shims_runtime.py
- src/molt/stdlib/__init__.py
- src/molt/stdlib/builtins.py
- src/molt/stdlib/io.py
- src/molt/stdlib/os.py
- src/molt/stdlib/pathlib.py
- src/molt/stdlib/sys.py
- src/molt/stdlib/traceback.py
- tests/differential/basic/descriptor_delete.py
- tests/differential/basic/getattr_calls.py
- tests/cli/test_cli_smoke.py
- tests/molt_diff.py
- tests/test_native_async_protocol.py
- tests/test_native_bytes.py
- tests/test_native_memoryview.py
- tests/test_stdlib_shims.py
- tests/test_wasm_async_protocol.py
- tests/test_wasm_bytes_ops.py
- tests/test_wasm_channel_async.py
- tests/test_wasm_control_flow.py
- tests/test_wasm_generator_protocol.py
- tests/test_wasm_list_dict_ops.py
- tests/test_wasm_memoryview_ops.py
- tests/test_wasm_string_ops.py
- tests/wasm_harness.py
- tools/bench.py
- tools/bench_wasm.py
- tools/runtime_safety.py
- wit/molt-runtime.wit
- docs/spec/0024_RUNTIME_STATE_LIFECYCLE.md
- src/molt/stdlib/tempfile.py
- tests/benchmarks/bench_ptr_registry.py
- tests/differential/basic/attr_security.py
- tests/differential/basic/iter_non_iterator.py
- tests/differential/basic/name_lookup.py
- tests/differential/basic/object_dunder_builtins.py
- tests/differential/basic/print_keywords.py
- tests/differential/basic/traceback_entries.py
- tests/differential/planned/
- tests/test_wasm_print_keywords.py
- tests/wasm_planned/

Docs/spec updates needed?
- None this turn (traceback spec/roadmap updated with line markers + code object gaps).

Tests
- Not run this turn (pending).

Benchmarks
- Not run this turn (not requested).

Profiling
- None.

Known gaps
- `str(bytes, encoding, errors)` decoding not implemented (NotImplementedError).
- `print(file=None)` uses host stdout if `sys` is not initialized.
- File I/O gaps: non-UTF-8 encodings/errors, text-mode seek/tell cookie semantics, readinto/writelines/detach/reconfigure, Windows fileno/isatty parity.
- WASM host hooks missing for full `open()` + file method parity.
- WASM `str_from_obj` does not call `__str__` for non-primitive objects.
- Backend panic for classes defining `__next__` without `__iter__` (see ROADMAP TODO).
- `sys.argv` decoding still uses lossy UTF-8/UTF-16 until filesystem-encoding + surrogateescape parity lands.
- Pointer registry lock contention optimization still pending (see OPT-0003).

CI
- Last green: https://github.com/adpena/molt/actions/runs/21060145271.
