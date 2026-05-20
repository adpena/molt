from __future__ import annotations

import importlib.util
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BENCH_GENERATOR = ROOT / "tests" / "benchmarks" / "bench_generator.py"


def _load_bench_generator():
    spec = importlib.util.spec_from_file_location("bench_generator_memory_guard", BENCH_GENERATOR)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_generator_benchmark_uses_shared_memory_guard(monkeypatch) -> None:
    module = _load_bench_generator()
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="Python 3.12.0\n", stderr="")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = module._run_timed_command(["python3", "--version"])

    assert result.stdout == "Python 3.12.0\n"
    assert captured["cmd"] == ["python3", "--version"]
    assert captured["kwargs"]["prefix"] == "MOLT_BENCH"
    assert captured["kwargs"]["capture_output"] is True
    assert captured["kwargs"]["timeout"] == module.DEFAULT_RUN_TIMEOUT_SEC


def test_generator_benchmark_default_binary_is_canonical_tmp() -> None:
    module = _load_bench_generator()

    assert module.DEFAULT_MOLT_BINARY == module.REPO_ROOT / "tmp" / "generator_molt"
