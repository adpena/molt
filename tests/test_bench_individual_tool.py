from __future__ import annotations

import importlib.util
from pathlib import Path
from types import ModuleType


ROOT = Path(__file__).resolve().parents[1]


def _load_bench_individual() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "molt_test_bench_individual",
        ROOT / "tools" / "bench_individual.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_bench_individual_reuses_backend_daemon_by_default(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
    cleanups = 0

    def fake_cleanup(*, quiet: bool = False) -> None:
        nonlocal cleanups
        del quiet
        cleanups += 1

    monkeypatch.setattr(bench, "_ensure_clean_slate", fake_cleanup)
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s: (True, 0.02, "1"),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s: (True, 0.04, "1"),
    )

    result = bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=1,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert cleanups == 0
    assert result["build_ok"] is True
    assert result["run_ok"] is True


def test_bench_individual_can_opt_into_cold_daemon_isolation(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
    cleanups = 0

    def fake_cleanup(*, quiet: bool = False) -> None:
        nonlocal cleanups
        del quiet
        cleanups += 1

    monkeypatch.setattr(bench, "_ensure_clean_slate", fake_cleanup)
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s: (True, 0.02, "1"),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s: (True, 0.04, "1"),
    )

    bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=1,
        timeout_build=1.0,
        timeout_run=1.0,
        isolate_daemon=True,
    )

    assert cleanups == 1
