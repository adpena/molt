from __future__ import annotations

import io
import subprocess
import tempfile
from pathlib import Path

import tools.bench_wasm as bench_wasm


def _fake_runtime_build(cmd: list[str], env: dict[str, str]) -> None:
    target_root = Path(env["CARGO_TARGET_DIR"])
    src = target_root / "wasm32-wasip1" / "release" / "molt_runtime.wasm"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_bytes(b"\x00asm\x01\x00\x00\x00")


def test_build_runtime_wasm_uses_wasm_release_profile_and_aggressive_features(
    monkeypatch,
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "target"
    monkeypatch.setattr(bench_wasm, "_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(bench_wasm, "_repo_root", lambda: tmp_path)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURES", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURE_MODE", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURES_EXTRA", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_CPU", raising=False)

    captured: list[tuple[list[str], dict[str, str]]] = []

    def _fake_run_cmd(  # type: ignore[no-untyped-def]
        cmd: list[str],
        *,
        env: dict[str, str],
        capture: bool,
        tty: bool,
        log,
        timeout_s: float | None = None,
        limits=None,
    ):
        del capture, tty, log, timeout_s, limits
        captured.append((list(cmd), dict(env)))
        _fake_runtime_build(cmd, env)
        return bench_wasm._RunResult(returncode=0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", _fake_run_cmd)
    output = tmp_path / "runtime.wasm"
    assert bench_wasm.build_runtime_wasm(
        reloc=False,
        output=output,
        tty=False,
        log=None,
    )
    assert output.exists()
    assert output.read_bytes().startswith(b"\x00asm")
    cmd, env = captured[0]
    assert cmd[:3] == ["cargo", "build", "--release"]
    assert "--no-default-features" in cmd
    features = set(cmd[cmd.index("--features") + 1].split(","))
    assert {"molt_gpu_primitives", "stdlib_micro"} <= features
    assert "stdlib_full" not in features
    assert "sqlite" not in features
    # Non-relocatable builds use standard import/export link flags
    rustflags = env.get("RUSTFLAGS", "")
    assert "--import-memory" in rustflags
    assert "--export-if-defined=molt_frozenset_add" in rustflags
    assert "--export-dynamic" not in rustflags


def test_build_runtime_wasm_uses_explicit_shared_link_flags(
    monkeypatch,
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "target"
    monkeypatch.setattr(bench_wasm, "_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(bench_wasm, "_repo_root", lambda: tmp_path)

    captured: list[tuple[list[str], dict[str, str]]] = []

    def _fake_run_cmd(  # type: ignore[no-untyped-def]
        cmd: list[str],
        *,
        env: dict[str, str],
        capture: bool,
        tty: bool,
        log,
        timeout_s: float | None = None,
        limits=None,
    ):
        del capture, tty, log, timeout_s, limits
        captured.append((list(cmd), dict(env)))
        _fake_runtime_build(cmd, env)
        return bench_wasm._RunResult(returncode=0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", _fake_run_cmd)
    output = tmp_path / "runtime_legacy.wasm"
    assert bench_wasm.build_runtime_wasm(
        reloc=False,
        output=output,
        tty=False,
        log=None,
    )
    cmd, env = captured[0]
    assert cmd[:3] == ["cargo", "build", "--release"]
    assert "--no-default-features" in cmd
    rustflags = env.get("RUSTFLAGS", "")
    assert "--import-memory" in rustflags
    assert "--growable-table" in rustflags
    assert "--export-if-defined=molt_frozenset_add" in rustflags
    assert "--export-dynamic" not in rustflags


def test_build_runtime_wasm_full_profile_uses_wasm_safe_full_feature_set(
    monkeypatch,
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "target"
    monkeypatch.setattr(bench_wasm, "_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(bench_wasm, "_repo_root", lambda: tmp_path)
    monkeypatch.setenv("MOLT_STDLIB_PROFILE", "full")

    captured: list[tuple[list[str], dict[str, str]]] = []

    def _fake_run_cmd(  # type: ignore[no-untyped-def]
        cmd: list[str],
        *,
        env: dict[str, str],
        capture: bool,
        tty: bool,
        log,
        timeout_s: float | None = None,
        limits=None,
    ):
        del capture, tty, log, timeout_s, limits
        captured.append((list(cmd), dict(env)))
        _fake_runtime_build(cmd, env)
        return bench_wasm._RunResult(returncode=0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", _fake_run_cmd)
    assert bench_wasm.build_runtime_wasm(
        reloc=False,
        output=tmp_path / "runtime_full.wasm",
        tty=False,
        log=None,
    )
    cmd, _env = captured[0]
    assert "--no-default-features" in cmd
    features = set(cmd[cmd.index("--features") + 1].split(","))
    assert {
        "molt_gpu_primitives",
        "stdlib_crypto",
        "stdlib_compression",
        "stdlib_logging_ext",
        "builtin_contextvars",
    } <= features
    assert "stdlib_full" not in features
    assert "sqlite" not in features


def test_failed_wasm_run_has_null_time_and_samples(monkeypatch, tmp_path: Path) -> None:
    script = tmp_path / "bench_fail.py"
    script.write_text("print(1)\n", encoding="utf-8")
    temp_dir = tempfile.TemporaryDirectory()
    wasm = bench_wasm.WasmBinary(
        run_env={},
        temp_dir=temp_dir,
        build_s=0.25,
        size_kb=12.5,
        linked_used=True,
        import_count_total=None,
        import_count_functions=None,
        import_count_tables=None,
    )

    monkeypatch.setattr(bench_wasm, "prepare_wasm_binary", lambda *args, **kwargs: wasm)
    monkeypatch.setattr(
        bench_wasm,
        "collect_samples",
        lambda *args, **kwargs: (
            [],
            False,
            bench_wasm._SampleResult(
                elapsed_s=None,
                returncode=1,
                error="runtime failed",
                error_class="runtime_error",
            ),
        ),
    )

    results = bench_wasm.bench_results(
        [str(script)],
        samples=1,
        warmup=0,
        super_run=True,
        require_linked=False,
        runner_cmd=["node"],
        runner_name="node",
        control_runner_cmd=None,
        control_runner_name=None,
        tty=False,
        log=None,
        keep_temp=False,
    )

    entry = results["bench_fail"]
    assert entry["molt_wasm_ok"] is False
    assert entry["molt_wasm_time_s"] is None
    assert entry["molt_wasm_samples_s"] == []
    assert entry["molt_wasm_failure_class"] == "runtime_error"


def test_measure_wasm_run_uses_guard_child_elapsed(monkeypatch) -> None:
    limits = bench_wasm.harness_memory_guard.limits_from_env("MOLT_BENCH", {})
    calls: list[dict[str, object]] = []

    def fake_run_cmd(*args, **kwargs):
        calls.append(kwargs)
        return bench_wasm._RunResult(
            returncode=0,
            stdout="",
            stderr="",
            elapsed_s=0.045,
        )

    monkeypatch.setattr(
        bench_wasm,
        "_run_cmd",
        fake_run_cmd,
    )

    result = bench_wasm.measure_wasm_run(
        {},
        ["node", "wasm/run_wasm.js"],
        runner_name="node",
        log=None,
        limits=limits,
    )

    assert result.elapsed_s == 0.045
    assert result.error is None
    assert calls[0]["limits"] is limits


def test_wasm_run_cmd_routes_tty_timeout_through_guard(monkeypatch) -> None:
    limits = bench_wasm.harness_memory_guard.HarnessMemoryLimits(
        enabled=False,
        max_process_rss_gb=1.0,
        max_total_rss_gb=1.0,
        max_global_rss_gb=1.0,
        poll_interval=0.1,
    )
    calls: list[dict[str, object]] = []

    def fake_guard(command, **kwargs):
        calls.append({"command": command, **kwargs})
        completed = subprocess.CompletedProcess(
            command,
            bench_wasm.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            "stdout",
            "TERM_CLEANUP\n",
        )
        completed.elapsed_s = 0.1
        return completed

    monkeypatch.setattr(
        bench_wasm.harness_memory_guard,
        "guarded_completed_process",
        fake_guard,
    )

    log = io.StringIO()
    result = bench_wasm._run_cmd(
        ["node", "runner.js"],
        env={},
        capture=False,
        tty=True,
        log=log,
        timeout_s=0.1,
        limits=limits,
    )

    assert (
        result.returncode
        == bench_wasm.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
    )
    assert result.timed_out is True
    assert result.elapsed_s == 0.1
    assert "TERM_CLEANUP" in result.stderr
    assert "TERM_CLEANUP" in log.getvalue()
    assert calls[0]["command"] == ["node", "runner.js"]
    assert calls[0]["capture_output"] is True
    assert calls[0]["timeout"] == 0.1
    assert calls[0]["limits"] is limits


def test_partial_wasm_sample_failure_has_null_time(monkeypatch, tmp_path: Path) -> None:
    script = tmp_path / "bench_partial.py"
    script.write_text("print(1)\n", encoding="utf-8")
    temp_dir = tempfile.TemporaryDirectory()
    wasm = bench_wasm.WasmBinary(
        run_env={},
        temp_dir=temp_dir,
        build_s=0.25,
        size_kb=12.5,
        linked_used=True,
        import_count_total=None,
        import_count_functions=None,
        import_count_tables=None,
    )

    monkeypatch.setattr(bench_wasm, "prepare_wasm_binary", lambda *args, **kwargs: wasm)
    monkeypatch.setattr(
        bench_wasm,
        "collect_samples",
        lambda *args, **kwargs: (
            [0.01],
            False,
            bench_wasm._SampleResult(
                elapsed_s=None,
                returncode=1,
                error="second sample failed",
                error_class="runtime_error",
            ),
        ),
    )

    results = bench_wasm.bench_results(
        [str(script)],
        samples=2,
        warmup=0,
        super_run=False,
        require_linked=False,
        runner_cmd=["node"],
        runner_name="node",
        control_runner_cmd=None,
        control_runner_name=None,
        tty=False,
        log=None,
        keep_temp=False,
    )

    entry = results["bench_partial"]
    assert entry["molt_wasm_ok"] is False
    assert entry["molt_wasm_time_s"] is None
    assert entry["molt_wasm_samples_s"] == [0.01]


def test_collect_samples_rejects_partial_sample_failure(monkeypatch) -> None:
    temp_dir = tempfile.TemporaryDirectory()
    wasm = bench_wasm.WasmBinary(
        run_env={},
        temp_dir=temp_dir,
        build_s=0.25,
        size_kb=12.5,
        linked_used=True,
        import_count_total=None,
        import_count_functions=None,
        import_count_tables=None,
    )
    results = iter(
        [
            bench_wasm._SampleResult(0.01, 0, None, None),
            bench_wasm._SampleResult(None, 1, "failed", "runtime_error"),
        ]
    )
    monkeypatch.setattr(
        bench_wasm, "measure_wasm_run", lambda *args, **kwargs: next(results)
    )

    samples, ok, failure = bench_wasm.collect_samples(
        wasm,
        samples=2,
        warmup=0,
        runner_cmd=["node"],
        runner_name="node",
        log=None,
    )

    assert samples == [0.01]
    assert ok is False
    assert failure is not None
    assert failure.error_class == "runtime_error"


def test_zero_duration_wasm_run_is_invalid_sample(monkeypatch) -> None:
    monkeypatch.setattr(bench_wasm.time, "perf_counter", lambda: 10.0)
    monkeypatch.setattr(
        bench_wasm,
        "_run_cmd",
        lambda *args, **kwargs: bench_wasm._RunResult(returncode=0),
    )

    result = bench_wasm.measure_wasm_run({}, ["node"], runner_name="node", log=None)

    assert result.elapsed_s is None
    assert result.returncode == 0
    assert result.error_class == "invalid_timing"
