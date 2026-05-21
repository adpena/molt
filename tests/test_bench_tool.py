from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import textwrap
from pathlib import Path

from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
BENCH_TOOL_PATH = REPO_ROOT / "tools" / "bench.py"
BENCH_TOOL_SPEC = importlib.util.spec_from_file_location(
    "bench_tool_under_test", BENCH_TOOL_PATH
)
assert BENCH_TOOL_SPEC is not None and BENCH_TOOL_SPEC.loader is not None
bench_tool = importlib.util.module_from_spec(BENCH_TOOL_SPEC)
BENCH_TOOL_SPEC.loader.exec_module(bench_tool)


def _run_bench(*args: str) -> subprocess.CompletedProcess[str]:
    return run_native_test_process(
        ["python3", "tools/bench.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_bench_no_cpython_sets_null_baseline(tmp_path: Path) -> None:
    script = tmp_path / "fast_script.py"
    script.write_text("print(1)\n", encoding="utf-8")
    out_json = tmp_path / "bench.json"

    res = _run_bench(
        "--no-cpython",
        "--no-pypy",
        "--no-codon",
        "--no-nuitka",
        "--no-pyodide",
        "--samples",
        "1",
        "--warmup",
        "0",
        "--json-out",
        str(out_json),
        "--script",
        str(script),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads(out_json.read_text(encoding="utf-8"))
    entry = payload["benchmarks"][script.name]
    assert entry["cpython_time_s"] is None
    assert entry["cpython_samples_s"] is None
    assert entry["molt_ok"] is True, res.stderr
    assert len(entry["molt_samples_s"]) == 1
    assert entry["molt_speedup"] is None
    assert entry["molt_output_parity"] == {
        "checked": False,
        "ok": None,
        "reference_runtime": "cpython",
        "reason": "reference_unavailable",
        "stdout_match": None,
        "stderr_match": None,
        "reference_stdout_sha256": None,
        "molt_stdout_sha256": None,
        "reference_stderr_sha256": None,
        "molt_stderr_sha256": None,
    }


def test_bench_runtime_timeout_marks_molt_not_ok(tmp_path: Path) -> None:
    script = tmp_path / "slow_script.py"
    script.write_text(
        textwrap.dedent(
            """
            import time

            time.sleep(2.0)
            print("done")
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    out_json = tmp_path / "bench_timeout.json"

    res = _run_bench(
        "--no-cpython",
        "--no-pypy",
        "--no-codon",
        "--no-nuitka",
        "--no-pyodide",
        "--samples",
        "1",
        "--warmup",
        "0",
        "--runtime-timeout-sec",
        "0.1",
        "--json-out",
        str(out_json),
        "--script",
        str(script),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads(out_json.read_text(encoding="utf-8"))
    entry = payload["benchmarks"][script.name]
    assert entry["molt_ok"] is False
    assert entry["molt_time_s"] is None
    assert entry["molt_samples_s"] == []
    assert entry["molt_output_parity"]["ok"] is None
    assert entry["molt_output_parity"]["reason"] == "reference_unavailable"


def test_molt_build_cmd_supports_explicit_profile() -> None:
    assert bench_tool._molt_build_cmd("release") == [
        "uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "-m",
        "molt.cli",
        "build",
        "--build-profile",
        "release",
    ]


def test_canonical_bench_env_uses_repo_roots_and_preserves_session() -> None:
    env = bench_tool._canonical_bench_env({"MOLT_SESSION_ID": "bench-review"})

    assert env["MOLT_EXT_ROOT"] == str(bench_tool.REPO_ROOT)
    assert env["CARGO_TARGET_DIR"] == str(bench_tool.REPO_ROOT / "target")
    assert env["MOLT_CACHE"] == str(bench_tool.REPO_ROOT / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(bench_tool.REPO_ROOT / "tmp" / "diff")
    assert env["TMPDIR"] == str(bench_tool.REPO_ROOT / "tmp")
    assert env["PYTHONPATH"] == str(bench_tool.REPO_ROOT / "src")
    assert env["MOLT_SESSION_ID"] == "bench-review"


def test_bench_run_cmd_uses_memory_guard_by_default(monkeypatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_guard(command, **kwargs):
        calls.append({"command": command, **kwargs})
        return subprocess.CompletedProcess(command, 0, "out", "err")

    monkeypatch.setattr(
        bench_tool.harness_memory_guard,
        "guarded_completed_process",
        fake_guard,
    )

    result = bench_tool._run_cmd(["tool", "arg"], env={}, capture=True, tty=False)

    assert result == bench_tool._RunResult(0, "out", "err")
    assert calls[0]["command"] == ["tool", "arg"]
    assert calls[0]["prefix"] == "MOLT_BENCH"


def test_bench_run_cmd_routes_tty_through_guard_without_raw_pty(monkeypatch) -> None:
    limits = bench_tool.harness_memory_guard.HarnessMemoryLimits(
        enabled=False,
        max_process_rss_gb=1.0,
        max_total_rss_gb=1.0,
        max_global_rss_gb=1.0,
        poll_interval=0.1,
    )
    calls: list[dict[str, object]] = []

    def fake_guard(command, **kwargs):
        calls.append({"command": command, **kwargs})
        return subprocess.CompletedProcess(command, 0, None, None)

    monkeypatch.setattr(
        bench_tool.harness_memory_guard,
        "guarded_completed_process",
        fake_guard,
    )

    result = bench_tool._run_cmd(
        ["tool", "arg"], env={}, capture=False, tty=True, limits=limits
    )

    assert result == bench_tool._RunResult(0, "", "")
    assert calls[0]["command"] == ["tool", "arg"]
    assert calls[0]["capture_output"] is False
    assert calls[0]["limits"] is limits


def test_measure_runtime_uses_guard_child_elapsed(monkeypatch) -> None:
    completed = subprocess.CompletedProcess(["tool"], 0, "out", "")
    completed.elapsed_s = 0.0125
    limits = bench_tool.harness_memory_guard.limits_from_env("MOLT_BENCH", {})
    calls: list[dict[str, object]] = []

    def fake_guard(*args, **kwargs):
        calls.append(kwargs)
        return completed

    monkeypatch.setattr(
        bench_tool.harness_memory_guard, "guarded_completed_process", fake_guard
    )

    sample = bench_tool.measure_runtime(["tool"], label="unit", limits=limits)

    assert sample is not None
    assert sample.elapsed_s == 0.0125
    assert sample.stdout == "out"
    assert calls[0]["limits"] is limits


def test_measure_molt_run_uses_guard_child_elapsed(monkeypatch, tmp_path: Path) -> None:
    binary = tmp_path / "molt-bin"
    binary.write_text("binary", encoding="utf-8")
    completed = subprocess.CompletedProcess([str(binary)], 0, "out", "")
    completed.elapsed_s = 0.034
    limits = bench_tool.harness_memory_guard.limits_from_env("MOLT_BENCH", {})
    calls: list[dict[str, object]] = []

    def fake_guard(*args, **kwargs):
        calls.append(kwargs)
        return completed

    monkeypatch.setattr(
        bench_tool.harness_memory_guard, "guarded_completed_process", fake_guard
    )

    sample = bench_tool.measure_molt_run(binary, label="unit", limits=limits)

    assert sample is not None
    assert sample.elapsed_s == 0.034
    assert sample.stdout == "out"
    assert calls[0]["limits"] is limits


def test_bench_batch_server_starts_in_guarded_process_group(monkeypatch) -> None:
    captured: dict[str, object] = {}

    class FakeClient:
        def __init__(self, cmd, **kwargs) -> None:
            captured["cmd"] = cmd
            captured.update(kwargs)

        def close(self, timeout: float = 5.0) -> None:
            captured["closed"] = timeout

    monkeypatch.setattr(bench_tool, "BatchCompileServerClient", FakeClient)

    server = bench_tool._BenchBatchBuildServer({})
    server.close()

    guard_context = captured["guard_context"]
    assert guard_context.prefix == "MOLT_BENCH"
    assert guard_context.limits.enabled is True
    assert "process_group_kwargs" not in captured
    assert "force_close" not in captured
    assert captured["closed"] == 5.0


def test_bench_defaults_baseline_to_canonical_results_path() -> None:
    assert bench_tool.DEFAULT_BASELINE_PATH == (
        bench_tool.REPO_ROOT / "bench" / "results" / "baseline.json"
    )


def test_compare_baseline_rejects_incompatible_timing_metadata() -> None:
    current = {
        "timing_mode": "warm_throughput",
        "warmup": 1,
        "samples": 1,
        "benchmarks": {"bench.py": {"molt_cpython_ratio": 1.0}},
    }
    baseline = {
        "timing_mode": "cold_first_run",
        "warmup": 0,
        "samples": 1,
        "benchmarks": {"bench.py": {"molt_cpython_ratio": 0.5}},
    }

    assert bench_tool.compare_baseline(current, baseline, 0.15) == [
        "incompatible benchmark baseline: "
        "timing_mode differs: current='warm_throughput', baseline='cold_first_run'; "
        "warmup differs: current=1, baseline=0; "
        "regenerate the baseline with matching benchmark timing settings"
    ]


def test_bench_cli_passes_molt_profile(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    monkeypatch.setattr(bench_tool, "_enable_line_buffering", lambda: None)
    monkeypatch.setattr(bench_tool, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(
        bench_tool,
        "bench_results",
        lambda *args, **kwargs: (
            captured.update({"molt_profile": args[10], "benchmarks": args[0]}) or {}
        ),
    )
    monkeypatch.setattr(bench_tool, "_git_rev", lambda: "deadbeef")
    monkeypatch.setattr(bench_tool, "write_json", lambda path, payload: None)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "tools/bench.py",
            "--molt-profile",
            "release",
            "--script",
            str(tmp_path / "bench_sample.py"),
        ],
    )
    (tmp_path / "bench_sample.py").write_text("print(1)\n", encoding="utf-8")

    bench_tool.main()

    assert captured["molt_profile"] == "release"
    assert captured["benchmarks"] == [str(tmp_path / "bench_sample.py")]


def test_bench_cli_defaults_molt_profile_to_release(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, object] = {}

    monkeypatch.setattr(bench_tool, "_enable_line_buffering", lambda: None)
    monkeypatch.setattr(bench_tool, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(
        bench_tool,
        "bench_results",
        lambda *args, **kwargs: captured.update({"molt_profile": args[10]}) or {},
    )
    monkeypatch.setattr(bench_tool, "_git_rev", lambda: "deadbeef")
    monkeypatch.setattr(bench_tool, "write_json", lambda path, payload: None)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "tools/bench.py",
            "--script",
            str(tmp_path / "bench_sample.py"),
        ],
    )
    (tmp_path / "bench_sample.py").write_text("print(1)\n", encoding="utf-8")

    bench_tool.main()

    assert captured["molt_profile"] == "release"


def test_summarize_samples_retains_raw_sample_evidence() -> None:
    stats = bench_tool.summarize_samples([1.0, 1.2])

    assert stats["samples_s"] == [1.0, 1.2]


class _TempDirStub:
    def cleanup(self) -> None:
        pass


class _BatchServerStub:
    closed = False

    def __init__(self, env: dict[str, str]) -> None:
        self.env = env

    def close(self) -> None:
        self.closed = True


def _bench_results_with_mocked_native_outputs(
    monkeypatch,
    tmp_path: Path,
    *,
    cpython_outputs: list[tuple[str, str]],
    molt_outputs: list[tuple[str, str]],
    samples: int | None = None,
    warmup: int = 0,
) -> dict:
    script = tmp_path / "bench_native_output.py"
    script.write_text("print('real path is not executed')\n", encoding="utf-8")
    sample_count = samples if samples is not None else len(molt_outputs)
    cpython_iter = iter(cpython_outputs)
    molt_iter = iter(molt_outputs)

    monkeypatch.setattr(bench_tool, "_canonical_bench_env", lambda env: {})
    monkeypatch.setattr(bench_tool, "_BenchBatchBuildServer", _BatchServerStub)
    monkeypatch.setattr(
        bench_tool,
        "prepare_molt_binary",
        lambda *args, **kwargs: bench_tool.MoltBinary(
            tmp_path / "molt-bin", _TempDirStub(), 0.25, 64.0
        ),
    )

    def fake_measure_runtime(*args, **kwargs):
        stdout, stderr = next(cpython_iter)
        return bench_tool.RunSample(2.0, stdout, stderr)

    def fake_measure_molt_run(*args, **kwargs):
        stdout, stderr = next(molt_iter)
        return bench_tool.RunSample(1.0, stdout, stderr)

    monkeypatch.setattr(bench_tool, "measure_runtime", fake_measure_runtime)
    monkeypatch.setattr(bench_tool, "measure_molt_run", fake_measure_molt_run)

    return bench_tool.bench_results(
        [str(script)],
        sample_count,
        warmup,
        True,
        False,
        False,
        False,
        False,
        False,
        None,
        "release",
        tty=False,
        nuitka_cmd=None,
        pyodide_cmd=None,
    )[script.name]


def test_prepare_molt_binary_uses_batch_build_server(
    monkeypatch, tmp_path: Path
) -> None:
    script = tmp_path / "bench_sample.py"
    script.write_text("print(1)\n", encoding="utf-8")
    requests: list[tuple[dict[str, object], float]] = []

    class _FakeBatchServer:
        def request_build(
            self, params: dict[str, object], *, timeout_s: float
        ) -> dict[str, object]:
            requests.append((params, timeout_s))
            out_dir = Path(str(params["out_dir"]))
            output = out_dir / "bench_sample_molt"
            output.write_bytes(b"binary")
            return {
                "ok": True,
                "returncode": 0,
                "stdout": json.dumps({"data": {"output": str(output)}}),
                "stderr": "",
            }

    monkeypatch.setattr(bench_tool, "_canonical_bench_env", lambda env: {"BASE": "1"})
    monkeypatch.setattr(bench_tool, "_prune_backend_daemons", lambda: None)

    binary = bench_tool.prepare_molt_binary(
        str(script),
        ["--type-hints", "trust", "--stdlib-profile", "full"],
        env={},
        build_profile="release",
        batch_server=_FakeBatchServer(),
        build_timeout_s=12.5,
    )

    assert binary is not None
    try:
        assert binary.path.read_bytes() == b"binary"
        assert binary.size_kb > 0
        params, timeout_s = requests[0]
        assert timeout_s == 12.5
        assert params["file_path"] == str(script)
        assert params["profile"] == "release"
        assert params["type_hints"] == "trust"
        assert params["stdlib_profile"] == "full"
        assert params["trusted"] is True
        assert params["json_output"] is True
        assert params["env_overrides"] == {"BASE": "1"}
    finally:
        binary.temp_dir.cleanup()


def test_bench_results_records_raw_native_sample_arrays(
    monkeypatch, tmp_path: Path
) -> None:
    entry = _bench_results_with_mocked_native_outputs(
        monkeypatch,
        tmp_path,
        cpython_outputs=[("same\n", ""), ("same\n", "")],
        molt_outputs=[("same\n", ""), ("same\n", "")],
    )

    assert entry["cpython_samples_s"] == [2.0, 2.0]
    assert entry["molt_samples_s"] == [1.0, 1.0]
    assert entry["molt_time_s"] == 1.0
    assert entry["molt_ok"] is True
    assert entry["molt_output_parity"]["ok"] is True
    assert entry["molt_output_parity"]["reason"] == "match"


def test_bench_results_records_warmup_samples_separately(
    monkeypatch, tmp_path: Path
) -> None:
    entry = _bench_results_with_mocked_native_outputs(
        monkeypatch,
        tmp_path,
        cpython_outputs=[("same\n", ""), ("same\n", ""), ("same\n", "")],
        molt_outputs=[("same\n", ""), ("same\n", ""), ("same\n", "")],
        samples=2,
        warmup=1,
    )

    assert entry["cpython_warmup_samples_s"] == [2.0]
    assert entry["cpython_samples_s"] == [2.0, 2.0]
    assert entry["molt_warmup_samples_s"] == [1.0]
    assert entry["molt_samples_s"] == [1.0, 1.0]
    assert entry["molt_time_s"] == 1.0
    assert entry["molt_ok"] is True


def test_bench_results_gates_molt_ok_on_stdout_mismatch(
    monkeypatch, tmp_path: Path
) -> None:
    entry = _bench_results_with_mocked_native_outputs(
        monkeypatch,
        tmp_path,
        cpython_outputs=[("expected\n", "")],
        molt_outputs=[("actual\n", "")],
    )

    assert entry["molt_ok"] is False
    assert entry["molt_time_s"] is None
    assert entry["molt_speedup"] is None
    assert entry["molt_output_parity"]["checked"] is True
    assert entry["molt_output_parity"]["ok"] is False
    assert entry["molt_output_parity"]["reason"] == "stdout_mismatch"
    assert entry["molt_output_parity"]["stdout_match"] is False
    assert "expected" not in json.dumps(entry["molt_output_parity"])
    assert "actual" not in json.dumps(entry["molt_output_parity"])


def test_bench_results_gates_molt_ok_on_stderr_mismatch(
    monkeypatch, tmp_path: Path
) -> None:
    entry = _bench_results_with_mocked_native_outputs(
        monkeypatch,
        tmp_path,
        cpython_outputs=[("", "expected diagnostic\n")],
        molt_outputs=[("", "actual diagnostic\n")],
    )

    assert entry["molt_ok"] is False
    assert entry["molt_time_s"] is None
    assert entry["molt_output_parity"]["reason"] == "stderr_mismatch"
    assert entry["molt_output_parity"]["stdout_match"] is True
    assert entry["molt_output_parity"]["stderr_match"] is False


def test_bench_results_rejects_unstable_cpython_reference(
    monkeypatch, tmp_path: Path
) -> None:
    entry = _bench_results_with_mocked_native_outputs(
        monkeypatch,
        tmp_path,
        cpython_outputs=[("first\n", ""), ("second\n", "")],
        molt_outputs=[("first\n", ""), ("first\n", "")],
    )

    assert entry["molt_ok"] is False
    assert entry["molt_time_s"] is None
    assert entry["molt_output_parity"]["checked"] is True
    assert entry["molt_output_parity"]["ok"] is False
    assert entry["molt_output_parity"]["reason"] == "reference_unstable"


def test_bench_results_skips_external_reference_for_molt_only_intrinsic_benchmark(
    monkeypatch, tmp_path: Path
) -> None:
    script = REPO_ROOT / "tests" / "benchmarks" / "bench_channel_throughput.py"

    monkeypatch.setattr(bench_tool, "_canonical_bench_env", lambda env: {})
    monkeypatch.setattr(bench_tool, "_BenchBatchBuildServer", _BatchServerStub)
    monkeypatch.setattr(
        bench_tool,
        "prepare_molt_binary",
        lambda *args, **kwargs: bench_tool.MoltBinary(
            tmp_path / "molt-bin", _TempDirStub(), 0.25, 64.0
        ),
    )
    monkeypatch.setattr(
        bench_tool,
        "measure_molt_run",
        lambda *args, **kwargs: bench_tool.RunSample(1.0, "intrinsic-only\n", ""),
    )

    def fail_external_reference(*args, **kwargs):
        raise AssertionError("Molt-only intrinsic benchmark ran an external baseline")

    monkeypatch.setattr(bench_tool, "measure_runtime", fail_external_reference)

    entry = bench_tool.bench_results(
        [str(script)],
        1,
        0,
        True,
        False,
        False,
        False,
        False,
        False,
        None,
        "release",
        tty=False,
        nuitka_cmd=None,
        pyodide_cmd=None,
    )[script.name]

    assert entry["reference_runtime"] == "molt"
    assert (
        entry["reference_reason"]
        == "molt_runtime_intrinsics_without_external_reference"
    )
    assert entry["cpython_time_s"] is None
    assert entry["cpython_samples_s"] is None
    assert entry["molt_ok"] is True
    assert entry["molt_output_parity"] == {
        "checked": False,
        "ok": None,
        "reference_runtime": "molt",
        "reason": "molt_runtime_intrinsics_without_external_reference",
        "stdout_match": None,
        "stderr_match": None,
        "reference_stdout_sha256": None,
        "molt_stdout_sha256": None,
        "reference_stderr_sha256": None,
        "molt_stderr_sha256": None,
    }


def test_bench_results_custom_same_basename_keeps_external_reference(
    monkeypatch, tmp_path: Path
) -> None:
    script = tmp_path / "bench_channel_throughput.py"
    script.write_text("print('custom')\n", encoding="utf-8")

    monkeypatch.setattr(bench_tool, "_canonical_bench_env", lambda env: {})
    monkeypatch.setattr(bench_tool, "_BenchBatchBuildServer", _BatchServerStub)
    monkeypatch.setattr(
        bench_tool,
        "prepare_molt_binary",
        lambda *args, **kwargs: bench_tool.MoltBinary(
            tmp_path / "molt-bin", _TempDirStub(), 0.25, 64.0
        ),
    )
    monkeypatch.setattr(
        bench_tool,
        "measure_molt_run",
        lambda *args, **kwargs: bench_tool.RunSample(1.0, "custom\n", ""),
    )
    monkeypatch.setattr(
        bench_tool,
        "measure_runtime",
        lambda *args, **kwargs: bench_tool.RunSample(2.0, "custom\n", ""),
    )

    entry = bench_tool.bench_results(
        [str(script)],
        1,
        0,
        True,
        False,
        False,
        False,
        False,
        False,
        None,
        "release",
        tty=False,
        nuitka_cmd=None,
        pyodide_cmd=None,
    )[script.name]

    assert entry["reference_runtime"] == "cpython"
    assert entry["reference_reason"] == "cpython_reference"
    assert entry["cpython_time_s"] == 2.0
    assert entry["molt_output_parity"]["checked"] is True
    assert entry["molt_output_parity"]["reference_runtime"] == "cpython"


def test_main_writes_json_then_exits_nonzero_on_output_parity_failure(
    monkeypatch, tmp_path: Path
) -> None:
    script = tmp_path / "bench_sample.py"
    script.write_text("print(1)\n", encoding="utf-8")
    out_json = tmp_path / "bench.json"
    writes: list[Path] = []

    monkeypatch.setattr(bench_tool, "_enable_line_buffering", lambda: None)
    monkeypatch.setattr(bench_tool, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(
        bench_tool,
        "bench_results",
        lambda *args, **kwargs: {
            script.name: {
                "molt_output_parity": {
                    "checked": True,
                    "ok": False,
                    "reason": "stdout_mismatch",
                }
            }
        },
    )
    monkeypatch.setattr(bench_tool, "_git_rev", lambda: "deadbeef")
    monkeypatch.setattr(
        bench_tool,
        "write_json",
        lambda path, payload: writes.append(path),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "tools/bench.py",
            "--no-pypy",
            "--no-codon",
            "--no-nuitka",
            "--no-pyodide",
            "--samples",
            "1",
            "--warmup",
            "0",
            "--json-out",
            str(out_json),
            "--update-baseline",
            "--script",
            str(script),
        ],
    )

    try:
        bench_tool.main()
    except SystemExit as exc:
        assert exc.code == 1
    else:
        raise AssertionError("expected output parity failure to exit nonzero")

    assert writes == [out_json]
