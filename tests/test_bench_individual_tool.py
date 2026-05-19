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
        warmup=1,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert cleanups == 0
    assert result["build_ok"] is True
    assert result["run_ok"] is True
    assert result["molt_ok"] is True
    assert result["molt_warmup_samples_s"] == [0.02]
    assert result["molt_samples_s"] == [0.02]
    assert result["cpython_warmup_samples_s"] == [0.04]
    assert result["cpython_samples_s"] == [0.04]


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
        warmup=1,
        timeout_build=1.0,
        timeout_run=1.0,
        isolate_daemon=True,
    )

    assert cleanups == 1


def test_bench_individual_rejects_partial_sample_failure(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
    calls = 0

    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )

    def fake_run_binary(binary: Path, timeout_s: float) -> tuple[bool, float, str]:
        nonlocal calls
        calls += 1
        if calls == 3:
            return False, 0.03, ""
        return True, 0.02, "1"

    monkeypatch.setattr(bench, "run_binary", fake_run_binary)
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s: (True, 0.04, "1"),
    )

    result = bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=2,
        warmup=1,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert result["run_ok"] is False
    assert result["molt_ok"] is False
    assert result["molt_warmup_samples_s"] == [0.02]
    assert result["molt_samples_s"] == [0.02]
    assert result["error"] == "Molt run failed during sample 2/2"


def test_bench_individual_records_molt_run_failure_detail(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()

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
        lambda binary, timeout_s: (
            False,
            0.02,
            "rc=7\nstderr:\nruntime intrinsic missing",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s: (True, 0.04, "1"),
    )

    result = bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=1,
        warmup=0,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert result["run_ok"] is False
    assert result["molt_failure_detail"] == "rc=7\nstderr:\nruntime intrinsic missing"
    assert "runtime intrinsic missing" in result["error"]


def test_bench_individual_marks_intrinsic_benchmarks_molt_only(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()

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
        lambda binary, timeout_s: (True, 0.02, "intrinsic-only"),
    )

    def fail_cpython(script: str, timeout_s: float) -> tuple[bool, float, str]:
        raise AssertionError("Molt-only intrinsic benchmarks must not run CPython")

    monkeypatch.setattr(bench, "run_cpython", fail_cpython)

    result = bench.bench_one(
        "tests/benchmarks/bench_ptr_registry.py",
        samples=1,
        warmup=0,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert result["reference_runtime"] == "molt"
    assert result["reference_reason"] == "molt_runtime_intrinsics_without_external_reference"
    assert result["molt_ok"] is True
    assert result["cpython_samples_s"] is None
    assert result["cpython_time_s"] is None
    assert result["output_match"] is None


def test_bench_individual_custom_same_basename_keeps_cpython_reference(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
    custom = tmp_path / "bench_ptr_registry.py"
    custom.write_text("print('custom')\n", encoding="utf-8")

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
        lambda binary, timeout_s: (True, 0.02, "custom"),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s: (True, 0.03, "custom"),
    )

    result = bench.bench_one(
        str(custom),
        samples=1,
        warmup=0,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert result["reference_runtime"] == "cpython"
    assert result["reference_reason"] == "cpython_reference"
    assert result["cpython_time_s"] == 0.03
    assert result["output_match"] is True
