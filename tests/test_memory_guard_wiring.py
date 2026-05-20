from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


GUARDED_ENTRYPOINTS = {
    "tests/molt_diff.py": (
        "harness_memory_guard",
        "_DiffGlobalMemoryMonitor",
        "memory_guard.ProcessTreeTracker",
    ),
    "tools/bench.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
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
    "tools/cpython_regrtest.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
        "MOLT_REGRTEST",
    ),
    "tools/translation_validate.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "guarded_harness_scope",
    ),
    "tests/harness/run_molt_conformance.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
    ),
    "tests/harness/run_monty_conformance.py": (
        "harness_memory_guard",
        "guarded_completed_process",
        "repo_process_sentinel",
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
