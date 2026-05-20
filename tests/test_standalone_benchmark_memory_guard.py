from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def _load_module(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _completed(cmd: list[str], stdout: str = "", stderr: str = ""):
    proc = subprocess.CompletedProcess(cmd, 0, stdout, stderr)
    proc.elapsed_s = 0.031
    return proc


def test_wasm_bench_compile_uses_memory_guard(monkeypatch, tmp_path: Path) -> None:
    module = _load_module(REPO_ROOT / "bench" / "wasm_bench.py", "wasm_bench_unit")
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(command, **kwargs):
        calls.append({"command": list(command), **kwargs})
        out_dir = Path(command[command.index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output.wasm").write_bytes(b"\0asm\x01\0\0\0")
        return _completed(list(command))

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    source = tmp_path / "bench.py"
    source.write_text("print(1)\n", encoding="utf-8")
    out_dir = tmp_path / "out"
    limits = module.harness_memory_guard.limits_from_env("MOLT_BENCH", {})

    result = module._compile_wasm(source, out_dir, limits=limits)

    assert result.ok is True
    assert result.elapsed_s == 0.031
    assert calls[0]["prefix"] == "MOLT_BENCH"
    assert calls[0]["limits"] is limits
    assert calls[0]["command"][:4] == [sys.executable, "-m", "molt.cli", "build"]


def test_wasm_bench_main_installs_repo_sentinel(monkeypatch, tmp_path: Path) -> None:
    module = _load_module(
        REPO_ROOT / "bench" / "wasm_bench.py",
        "wasm_bench_main_unit",
    )
    sentinel: dict[str, object] = {}

    class FakeSentinel:
        def __enter__(self):
            sentinel["entered"] = True
            return self

        def __exit__(self, exc_type, exc, tb):
            sentinel["exited"] = True

    def fake_repo_process_sentinel(**kwargs):
        sentinel.update(kwargs)
        return FakeSentinel()

    monkeypatch.setattr(
        module.harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(module, "run_benchmarks", lambda *args, **kwargs: [])
    monkeypatch.setattr(module, "build_report", lambda entries, *, limits: {})
    monkeypatch.setattr(
        sys,
        "argv",
        ["wasm_bench.py", "--out", str(tmp_path / "wasm.json")],
    )

    module.main()

    assert sentinel["repo_root"] == module.ROOT
    assert sentinel["artifact_root"] == module.ROOT / "tmp" / "bench"
    assert sentinel["label"] == "wasm_bench"
    assert sentinel["entered"] is True
    assert sentinel["exited"] is True
    assert (
        module.DEFAULT_OUTPUT_PATH
        == module.ROOT / "bench" / "results" / "wasm_baseline.json"
    )


def test_luau_benchmark_compile_uses_guard_and_canonical_tmp(
    monkeypatch,
    tmp_path: Path,
) -> None:
    module = _load_module(
        REPO_ROOT / "bench" / "luau" / "run_benchmarks.py",
        "luau_bench_unit",
    )
    monkeypatch.setattr(module, "TMP_ROOT", tmp_path / "tmp" / "luau")
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(command, **kwargs):
        calls.append({"command": list(command), **kwargs})
        out_path = Path(command[command.index("--output") + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text("print('ok')\n", encoding="utf-8")
        return _completed(list(command))

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    source = tmp_path / "bench_luau.py"
    source.write_text("print(1)\n", encoding="utf-8")
    limits = module.harness_memory_guard.limits_from_env("MOLT_BENCH", {})

    out_path, elapsed_ms, error = module.compile_to_luau(
        source,
        "uv run python -m molt.cli",
        limits=limits,
    )

    assert error == ""
    assert elapsed_ms == 31.0
    assert out_path == module.TMP_ROOT / "bench_luau.luau"
    assert out_path.exists()
    assert calls[0]["prefix"] == "MOLT_BENCH"
    assert calls[0]["limits"] is limits
    assert calls[0]["command"][:5] == ["uv", "run", "python", "-m", "molt.cli"]


def test_luau_benchmark_main_installs_repo_sentinel(
    monkeypatch,
    tmp_path: Path,
) -> None:
    module = _load_module(
        REPO_ROOT / "bench" / "luau" / "run_benchmarks.py",
        "luau_bench_main_unit",
    )
    monkeypatch.setattr(module, "TMP_ROOT", tmp_path / "tmp" / "luau")
    monkeypatch.setattr(module, "RESULTS_DIR", tmp_path / "results")
    monkeypatch.setattr(module, "DEFAULT_RESULTS_PATH", tmp_path / "results.json")
    sentinel: dict[str, object] = {}

    class FakeSentinel:
        def __enter__(self):
            sentinel["entered"] = True
            return self

        def __exit__(self, exc_type, exc, tb):
            sentinel["exited"] = True

    def fake_repo_process_sentinel(**kwargs):
        sentinel.update(kwargs)
        return FakeSentinel()

    monkeypatch.setattr(
        module.harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(
        sys,
        "argv",
        ["run_benchmarks.py", "--benchmarks", "missing.py"],
    )

    module.main()

    assert sentinel["repo_root"] == module.REPO_ROOT
    assert sentinel["artifact_root"] == module.REPO_ROOT / "tmp" / "bench"
    assert sentinel["label"] == "luau_run_benchmarks"
    assert sentinel["entered"] is True
    assert sentinel["exited"] is True
    assert module.DEFAULT_RESULTS_PATH.exists()
