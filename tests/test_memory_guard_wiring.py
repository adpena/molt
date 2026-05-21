from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


GUARDED_ENTRYPOINTS = {
    "tests/molt_diff.py": (
        "harness_memory_guard",
        "_DiffGlobalMemoryMonitor",
        "HarnessExecutionContext",
        "memory_guard.ProcessTreeTracker",
    ),
    "tools/bench.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "HarnessExecutionContext",
        "repo_process_sentinel",
    ),
    "tools/batch_compile_client.py": (
        "harness_memory_guard",
        "HarnessExecutionContext",
        "start_repo_sentinel",
        "process_group_kwargs",
    ),
    "tools/guarded_exec.py": (
        "harness_memory_guard",
        "HarnessExecutionContext",
        "canonical_harness_env",
    ),
    "deploy/scripts/benchmark_simd.py": (
        "harness_memory_guard",
        "HarnessExecutionContext",
        "canonical_harness_env",
    ),
    "tools/cloudflare_demo_verify.py": (
        "harness_memory_guard",
        "HarnessExecutionContext",
        "canonical_harness_env",
    ),
    "drivers/cloudflare/thin_adapter/verify.py": (
        "harness_memory_guard",
        "HarnessExecutionContext",
        "canonical_harness_env",
    ),
    "tools/bench_wasm.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tools/bench_individual.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tools/bench_friends.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tools/bench_audit.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tools/bench_backend_incremental.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tools/benchmark_luau_vs_cpython.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tools/cpython_regrtest.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
        "MOLT_REGRTEST",
    ),
    "tools/molt_regrtest_shim.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_REGRTEST",
    ),
    "tools/profile.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tools/throughput_matrix.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tools/translation_validate.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "guarded_harness_scope",
    ),
    "tools/wasm_hotspot_profile.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tools/wasm_pipeline.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tools/wasm_profile.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tools/wasm_run_matrix.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tools/wasm_stub_wasi.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tools/wasm_strip_unused.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tests/harness/run_molt_conformance.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "HarnessExecutionContext",
        "repo_process_sentinel",
    ),
    "tests/harness/run_monty_conformance.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tests/benchmarks/bench_generator.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
    ),
    "tests/compliance/process_guard.py": (
        "harness_memory_guard",
        "run_guarded_test_process",
        "MOLT_COMPLIANCE",
    ),
    "tests/runtime_compat/test_runtime_compat.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "MOLT_RUNTIME_COMPAT",
    ),
    "src/molt/harness_layers.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "src/molt/cli.py": (
        "_load_cli_harness_memory_guard",
        "guarded_completed_process",
        "MOLT_BENCH",
        "MOLT_CONFORMANCE",
        "MOLT_DIFF",
    ),
}


def test_default_memory_guard_wiring_for_harness_entrypoints() -> None:
    missing: list[str] = []
    for rel_path, required_tokens in GUARDED_ENTRYPOINTS.items():
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        for token in required_tokens:
            if token not in text:
                missing.append(f"{rel_path}: {token}")

    assert missing == []


def test_legacy_shell_entrypoints_enter_guarded_python_wrappers() -> None:
    required = {
        "bench/run_all.sh": ("tools/bench.py", "TMPDIR"),
        "bench/scripts/run_stack.sh": ("tools/guarded_exec.py", "MOLT_GUARDED_STACK_INNER"),
        "bench/scripts/run_db_stub.sh": ("run_db_stub.py", "TMPDIR"),
        "tests/parity/run_parity.sh": ("tools/parity_gate.py", "TMPDIR"),
    }
    missing: list[str] = []
    for rel_path, tokens in required.items():
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        for token in tokens:
            if token not in text:
                missing.append(f"{rel_path}: {token}")

    assert missing == []
