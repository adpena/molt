from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]


def _run_bench(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", "tools/bench.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def _load_bench_module():
    module_path = REPO_ROOT / "tools" / "bench.py"
    spec = importlib.util.spec_from_file_location("bench_tool_under_test", module_path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_prepare_molt_binary_uses_selected_build_profile(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")
    seen_cmd: list[str] = []

    def _fake_run(
        cmd: list[str],
        *,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        timeout: float,
    ) -> subprocess.CompletedProcess[str]:
        del env, capture_output, text, timeout
        seen_cmd[:] = cmd
        out_dir = Path(cmd[cmd.index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        output = out_dir / "script_molt"
        output.write_bytes(b"\x00")
        return subprocess.CompletedProcess(
            cmd, 0, stdout=json.dumps({"output": str(output)}), stderr=""
        )

    monkeypatch.setattr(bench.subprocess, "run", _fake_run)
    built = bench.prepare_molt_binary(
        str(script), env={}, build_profile="release", build_timeout_s=42.0
    )

    assert built is not None
    assert "--profile" in seen_cmd
    assert seen_cmd[seen_cmd.index("--profile") + 1] == "release"
    assert built.path.exists()
    built.temp_dir.cleanup()


def test_prepare_molt_binary_timeout_prints_actionable_diagnostic(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")

    def _timeout(
        cmd: list[str],
        *,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        timeout: float,
    ) -> subprocess.CompletedProcess[str]:
        del env, capture_output, text
        raise subprocess.TimeoutExpired(
            cmd=cmd,
            timeout=timeout,
            output="building...",
            stderr="still compiling",
        )

    monkeypatch.setattr(bench.subprocess, "run", _timeout)
    built = bench.prepare_molt_binary(
        str(script), env={}, build_profile="release", build_timeout_s=1.5
    )

    assert built is None
    stderr = capsys.readouterr().err
    assert "Molt build timed out" in stderr
    assert "--build-timeout-sec" in stderr
    assert "--build-profile dev" in stderr
    assert "still compiling" in stderr
    assert "Build command:" in stderr


def test_prepare_molt_binary_failure_surfaces_stderr_summary(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")

    def _failed_run(
        cmd: list[str],
        *,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        timeout: float,
    ) -> subprocess.CompletedProcess[str]:
        del env, capture_output, text, timeout
        return subprocess.CompletedProcess(
            cmd,
            2,
            stdout="",
            stderr="cargo build failed\nbackend panic: unresolved symbol",
        )

    monkeypatch.setattr(bench.subprocess, "run", _failed_run)
    built = bench.prepare_molt_binary(str(script), env={}, build_profile="dev")

    assert built is None
    stderr = capsys.readouterr().err
    assert "Molt build failed for" in stderr
    assert "exit code 2" in stderr
    assert "backend panic: unresolved symbol" in stderr


def test_prepare_molt_binary_uses_batch_build_client(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")

    class _FakeClient:
        def __init__(self) -> None:
            self.calls: list[tuple[dict[str, object], dict[str, str]]] = []

        def build(self, *, params: dict[str, object], env_overrides: dict[str, str]):
            self.calls.append((dict(params), dict(env_overrides)))
            out_dir = Path(str(params["out_dir"]))
            out_dir.mkdir(parents=True, exist_ok=True)
            output = out_dir / "script_molt"
            output.write_bytes(b"\x00")
            return bench._RunResult(
                0, stdout=json.dumps({"output": str(output)}), stderr=""
            )

    fake = _FakeClient()
    monkeypatch.setattr(
        bench.subprocess,
        "run",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("subprocess path should not be used")
        ),
    )
    built = bench.prepare_molt_binary(
        str(script),
        extra_args=["--type-hints", "trust"],
        env={},
        build_profile="release",
        build_client=fake,
    )

    assert built is not None
    assert len(fake.calls) == 1
    params, env_overrides = fake.calls[0]
    assert params["file_path"] == str(script)
    assert params["profile"] == "release"
    assert params["trusted"] is True
    assert params["json_output"] is True
    assert params["type_hints"] == "trust"
    assert env_overrides == {"PYTHONPATH": "src"}
    built.temp_dir.cleanup()


def test_prepare_molt_binary_falls_back_for_unsupported_batch_args(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")
    seen_subprocess = {"called": False}

    class _FakeClient:
        def build(self, *, params: dict[str, object], env_overrides: dict[str, str]):
            del params, env_overrides
            raise AssertionError("batch client should not be used")

    def _fake_run(
        cmd: list[str],
        *,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        del env, capture_output, text, timeout
        seen_subprocess["called"] = True
        out_dir = Path(cmd[cmd.index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        output = out_dir / "script_molt"
        output.write_bytes(b"\x00")
        return subprocess.CompletedProcess(
            cmd, 0, stdout=json.dumps({"output": str(output)}), stderr=""
        )

    monkeypatch.setattr(bench.subprocess, "run", _fake_run)
    built = bench.prepare_molt_binary(
        str(script),
        extra_args=["--emit", "ir"],
        env={},
        build_profile="dev",
        build_client=_FakeClient(),
    )

    assert built is not None
    assert seen_subprocess["called"] is True
    built.temp_dir.cleanup()


def test_prepare_molt_binary_accepts_json_with_stdout_noise(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")

    def _fake_run(
        cmd: list[str],
        *,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        del env, capture_output, text, timeout
        out_dir = Path(cmd[cmd.index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        output = out_dir / "script_molt"
        output.write_bytes(b"\x00")
        noisy_stdout = (
            "Backend warmup...\n" + json.dumps({"output": str(output)}) + "\n"
        )
        return subprocess.CompletedProcess(cmd, 0, stdout=noisy_stdout, stderr="")

    monkeypatch.setattr(bench.subprocess, "run", _fake_run)
    built = bench.prepare_molt_binary(str(script), env={}, build_profile="dev")

    assert built is not None
    assert built.path.exists()
    built.temp_dir.cleanup()


def test_base_python_env_sets_backend_daemon_start_timeout_default(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bench = _load_bench_module()
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_START_TIMEOUT", raising=False)

    env = bench._base_python_env()

    assert env["MOLT_BACKEND_DAEMON_START_TIMEOUT"] == "90"


def test_base_python_env_preserves_backend_daemon_start_timeout_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bench = _load_bench_module()
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_START_TIMEOUT", "45")

    env = bench._base_python_env()

    assert env["MOLT_BACKEND_DAEMON_START_TIMEOUT"] == "45"


def test_bench_build_timeout_arg_rejects_non_positive() -> None:
    res = _run_bench("--build-timeout-sec", "0")
    assert res.returncode != 0
    assert "--build-timeout-sec must be > 0" in res.stderr


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
    assert entry["molt_ok"] is True
    assert entry["molt_speedup"] is None


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


def test_main_forwards_build_flags(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")
    out_json = tmp_path / "bench.json"
    captured: dict[str, object] = {}

    def _fake_bench_results(*args, **kwargs):
        del args
        captured.update(kwargs)
        return {}

    monkeypatch.setattr(bench, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(bench, "bench_results", _fake_bench_results)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bench.py",
            "--no-cpython",
            "--no-pypy",
            "--no-codon",
            "--no-nuitka",
            "--no-pyodide",
            "--samples",
            "1",
            "--warmup",
            "0",
            "--build-profile",
            "release",
            "--build-timeout-sec",
            "7.5",
            "--json-out",
            str(out_json),
            "--script",
            str(script),
        ],
    )

    bench.main()

    assert captured["build_profile"] == "release"
    assert captured["build_timeout_s"] == 7.5
    assert captured["use_batch_build_server"] is True


def test_main_forwards_no_batch_build_server(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    bench = _load_bench_module()
    script = tmp_path / "script.py"
    script.write_text("print(1)\n", encoding="utf-8")
    out_json = tmp_path / "bench.json"
    captured: dict[str, object] = {}

    def _fake_bench_results(*args, **kwargs):
        del args
        captured.update(kwargs)
        return {}

    monkeypatch.setattr(bench, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(bench, "bench_results", _fake_bench_results)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bench.py",
            "--no-cpython",
            "--no-pypy",
            "--no-codon",
            "--no-nuitka",
            "--no-pyodide",
            "--samples",
            "1",
            "--warmup",
            "0",
            "--no-batch-build-server",
            "--json-out",
            str(out_json),
            "--script",
            str(script),
        ],
    )

    bench.main()
    assert captured["use_batch_build_server"] is False
