# Agent Locks

This file coordinates parallel agent work to prevent file collisions.

## How to use
- Claim a file or directory by adding a line: `<agent-id> -> <path>`.
- Keep claims narrow and time-bound.
- Remove your claim when finished.

## Active locks
codex-3267 -> logs/
codex-3267 -> CHECKPOINT.md
codex-3267 -> docs/AGENT_MEMORY.md
codex-3267 -> src/molt/stdlib/errno.py
codex-3267 -> src/molt/stdlib/test/__init__.py
codex-3267 -> src/molt/stdlib/test/import_helper.py
codex-3267 -> src/molt/stdlib/test/list_tests.py
codex-3267 -> src/molt/stdlib/test/os_helper.py
codex-3267 -> src/molt/stdlib/test/seq_tests.py
codex-3267 -> src/molt/stdlib/test/support.py
codex-3267 -> tests/differential/basic/import_star.py
codex-3267 -> tests/differential/basic/string_format_errors.py
codex-3267 -> src/molt/stdlib/asyncio.py
codex-3267 -> src/molt/stdlib/types.py
codex-3267 -> runtime/molt-backend/src/lib.rs
codex-3267 -> runtime/molt-backend/src/wasm.rs
codex-3267 -> runtime/molt-backend/tests/loop_continue.rs
codex-3267 -> src/molt/frontend/__init__.py
codex-3267 -> tests/differential/basic/del_global_basic.py
codex-3267 -> src/molt/stdlib/__init__.py
codex-3267 -> runtime/molt-runtime/src/lib.rs
codex-3267 -> runtime/molt-obj-model/src/lib.rs
codex-3267 -> wit/molt-runtime.wit
codex-3267 -> tools/dev.py
codex-3267 -> tools/dev_test_runner.py
codex-3267 -> src/molt/cli.py
codex-3267 -> tests/differential/basic/pep649_lazy_annotations.py
codex-3267 -> docs/spec/STATUS.md
codex-3267 -> ROADMAP.md
codex-3267 -> docs/spec/areas/compat/0023_SEMANTIC_BEHAVIOR_MATRIX.md
codex-3267 -> src/molt/stdlib/inspect.py
codex-3267 -> tests/differential/basic/asyncio_run_shutdown_asyncgens.py
codex-3267 -> tests/differential/basic/async_generator_asend_after_close.py
codex-3267 -> tests/differential/basic/async_generator_asend_none_edges.py
codex-3267 -> tests/differential/basic/async_generator_athrow_after_close.py
codex-3267 -> tests/differential/basic/async_generator_athrow_after_stop.py
codex-3267 -> tests/differential/basic/async_generator_close_semantics.py
codex-3267 -> tests/differential/basic/async_generator_completion_edges.py
codex-3267 -> tests/differential/basic/async_generator_completion_more.py
codex-3267 -> tests/differential/basic/async_generator_finalization.py
codex-3267 -> tests/differential/basic/async_generator_ge_after_stop.py
codex-3267 -> tests/differential/basic/async_generator_post_stop_edges.py
codex-3267 -> tests/differential/basic/async_generator_protocol.py
codex-3267 -> tests/wasm_harness.py
