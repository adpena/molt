from __future__ import annotations

import importlib.util
import json
import os
import signal
import subprocess
import sys
import textwrap
import types
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"
TOOL_PATH = REPO_ROOT / "tools" / "bench_friends.py"
ADAPTER_PATH = REPO_ROOT / "tools" / "tinygrad_off_shelf_adapter.py"
NUMPY_ADAPTER_PATH = REPO_ROOT / "tools" / "numpy_off_shelf_adapter.py"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_authority  # noqa: E402


def _load_tool_module():
    spec = importlib.util.spec_from_file_location("bench_friends_under_test", TOOL_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _load_tinygrad_adapter_module():
    spec = importlib.util.spec_from_file_location(
        "tinygrad_off_shelf_adapter_under_test", ADAPTER_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _load_numpy_adapter_module():
    spec = importlib.util.spec_from_file_location(
        "numpy_off_shelf_adapter_under_test", NUMPY_ADAPTER_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _has_env_pair_casefold(env: dict[str, str], name: str, value: str) -> bool:
    folded = name.upper()
    return any(
        key.upper() == folded and candidate == value for key, candidate in env.items()
    )


def _run_tool(*args: str) -> subprocess.CompletedProcess[str]:
    return run_native_test_process(
        [sys.executable, "tools/bench_friends.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def _sample_suite_result(module, suite_id: str = "replay_smoke"):
    phase = module.PhaseResult(
        cmd=["python3", "-c", "print('ok')"],
        returncode=0,
        elapsed_s=0.0125,
        timed_out=False,
        stdout_path="stdout.log",
        stderr_path="stderr.log",
        stdout_json={"status": "ok"},
        guard_status="pass",
        guard_orphaned_process_groups=[4321],
        guard_cargo_incremental_quarantine={"status": "not_needed"},
    )
    runner = module.RunnerResult(
        name="tinygrad",
        role="workload",
        status="ok",
        reason=None,
        build=phase,
        runs=[phase],
        run_samples_s=[0.0125],
        run_median_s=0.0125,
        run_mean_s=0.0125,
        run_stdev_s=0.0,
        structured_outputs=[{"workload": "ok"}],
        structured_samples_s={"workload": [0.0125]},
        structured_median_s={"workload": 0.0125},
    )
    return module.SuiteResult(
        id=suite_id,
        friend="tinygrad",
        display_name="Replay Smoke",
        semantic_mode="runs_unmodified",
        source="local",
        suite_root="/tmp/replay-suite",
        suite_workdir="/tmp/replay-suite",
        resolved_ref=None,
        requested_ref=None,
        source_custody=module.SourceCustody(
            source="local",
            requested_ref=None,
            expected_ref=None,
            head_ref=None,
            ref_verified=None,
            git_clean=None,
            git_status_porcelain=None,
            git_ignored_artifacts=None,
            suite_root_overridden=False,
            verification="local_path",
        ),
        status="ok",
        reason=None,
        adapter_notes="replay-only",
        tags=["unit"],
        runners={"tinygrad": runner},
        metrics={
            "cpython_median_s": 0.02,
            "tinygrad_median_s": 0.0125,
            "molt_median_s": None,
            "molt_cpython_ratio": None,
            "molt_vs_friend_speedup": None,
            "molt_vs_numpy_speedup": None,
        },
    )


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run_native_test_process(
        ["git", *args],
        cwd=repo,
        text=True,
        capture_output=True,
        check=True,
    )


def _init_git_repo(repo: Path) -> str:
    repo.mkdir(parents=True, exist_ok=True)
    _git(repo, "init")
    (repo / "script.py").write_text("print('ok')\n", encoding="utf-8")
    _git(repo, "add", "script.py")
    _git(
        repo,
        "-c",
        "user.email=molt@example.invalid",
        "-c",
        "user.name=Molt Test",
        "commit",
        "-m",
        "initial",
    )
    return _git(repo, "rev-parse", "HEAD").stdout.strip()


def test_default_output_root_is_canonical_bench_results(monkeypatch) -> None:
    module = _load_tool_module()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(REPO_ROOT))

    output_root = module._default_output_root()

    assert output_root.parent == REPO_ROOT / "bench" / "results" / "friends"


def test_project_python_prefers_active_virtualenv(monkeypatch, tmp_path: Path) -> None:
    module = _load_tool_module()
    venv = tmp_path / "venv"
    python_path = venv / (
        "Scripts/python.exe" if module.os.name == "nt" else "bin/python"
    )
    python_path.parent.mkdir(parents=True)
    python_path.write_text("", encoding="utf-8")
    monkeypatch.setenv("VIRTUAL_ENV", str(venv))
    monkeypatch.setattr(module.sys, "prefix", str(venv))
    monkeypatch.setattr(module.sys, "base_prefix", str(tmp_path / "base"))

    assert module._project_python() == str(python_path)


def test_base_run_env_preserves_windows_toolchain_roots(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _load_tool_module()
    monkeypatch.setenv("ProgramFiles(x86)", r"C:\Program Files (x86)")
    monkeypatch.setenv("ProgramFiles", r"C:\Program Files")
    monkeypatch.setenv("ProgramW6432", r"C:\Program Files")
    monkeypatch.setenv(
        "CommonProgramFiles(x86)", r"C:\Program Files (x86)\Common Files"
    )
    monkeypatch.setenv("CommonProgramFiles", r"C:\Program Files\Common Files")
    monkeypatch.setenv("CommonProgramW6432", r"C:\Program Files\Common Files")
    monkeypatch.setenv("ProgramData", r"C:\ProgramData")
    monkeypatch.setenv("LOCALAPPDATA", r"C:\Users\tester\AppData\Local")

    env = module._base_run_env()

    assert _has_env_pair_casefold(env, "ProgramFiles(x86)", r"C:\Program Files (x86)")
    assert _has_env_pair_casefold(env, "ProgramFiles", r"C:\Program Files")
    assert _has_env_pair_casefold(env, "ProgramW6432", r"C:\Program Files")
    assert _has_env_pair_casefold(
        env,
        "CommonProgramFiles(x86)",
        r"C:\Program Files (x86)\Common Files",
    )
    assert _has_env_pair_casefold(
        env,
        "CommonProgramFiles",
        r"C:\Program Files\Common Files",
    )
    assert _has_env_pair_casefold(
        env,
        "CommonProgramW6432",
        r"C:\Program Files\Common Files",
    )
    assert _has_env_pair_casefold(env, "ProgramData", r"C:\ProgramData")
    assert _has_env_pair_casefold(
        env,
        "LOCALAPPDATA",
        r"C:\Users\tester\AppData\Local",
    )


def test_bench_friends_result_dict_round_trip_preserves_renderer_fields() -> None:
    module = _load_tool_module()
    suite = _sample_suite_result(module, "round_trip_smoke")
    payload = module._suite_to_dict(suite)

    round_tripped = module._suite_to_dict(module._suite_from_dict(payload))

    assert round_tripped == payload


def test_bench_friends_summary_row_spacing_is_regular() -> None:
    module = _load_tool_module()
    suite = _sample_suite_result(module, "spacing_smoke")

    summary_text = module._render_summary_markdown(
        run_started_at="2026-06-16T20:18:20+00:00",
        manifest_path=Path("bench/friends/manifest.toml"),
        json_rel="bench/results/friends/example/results.json",
        suites=[suite],
    )

    header = next(
        line for line in summary_text.splitlines() if line.startswith("| Suite")
    )
    row = next(
        line for line in summary_text.splitlines() if line.startswith("| spacing")
    )
    assert "|-" not in row
    assert len(header.strip().strip("|").split("|")) == len(
        row.strip().strip("|").split("|")
    )


def test_bench_friends_render_existing_json_does_not_run_workloads(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_tool_module()
    suite = _sample_suite_result(module)
    results_json = tmp_path / "results.json"
    summary_out = tmp_path / "summary.md"
    payload = {
        "schema_version": 1,
        "generated_at": "2026-06-16T20:18:20+00:00",
        "manifest_path": str(tmp_path / "manifest.toml"),
        "interrupted": None,
        "backend_daemon_cleanup": [],
        "memory_guard_incidents": [],
        "suites": [module._suite_to_dict(suite)],
    }
    results_text = json.dumps(payload, indent=2, sort_keys=True)
    results_json.write_text(results_text, encoding="utf-8")

    def fail_if_called(*_args, **_kwargs):
        raise AssertionError("render-only mode must not execute benchmark paths")

    monkeypatch.setattr(module, "_acquire_suite", fail_if_called)
    monkeypatch.setattr(module, "_run_prepare_steps", fail_if_called)
    monkeypatch.setattr(module, "_run_runner", fail_if_called)
    monkeypatch.setattr(module, "_cleanup_backend_daemons", fail_if_called)
    monkeypatch.setattr(
        module.sys,
        "argv",
        [
            "bench_friends.py",
            "--render-existing-json",
            str(results_json),
            "--summary-out",
            str(summary_out),
        ],
    )

    assert module.main() == 0
    assert results_json.read_text(encoding="utf-8") == results_text
    summary_text = summary_out.read_text(encoding="utf-8")
    assert "replay_smoke" in summary_text
    assert "|-" not in next(
        line for line in summary_text.splitlines() if line.startswith("| replay")
    )


def test_bench_friends_render_existing_json_rejects_workload_checkout_flag(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    module = _load_tool_module()
    results_json = tmp_path / "results.json"
    summary_out = tmp_path / "summary.md"
    results_json.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "generated_at": "2026-06-16T20:18:20+00:00",
                "manifest_path": str(tmp_path / "manifest.toml"),
                "suites": [],
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        module.sys,
        "argv",
        [
            "bench_friends.py",
            "--render-existing-json",
            str(results_json),
            "--summary-out",
            str(summary_out),
            "--no-checkout",
        ],
    )

    assert module.main() == 2
    assert "--checkout/--no-checkout" in capsys.readouterr().err
    assert not summary_out.exists()


def test_bench_friends_local_suite_runs(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)

    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "local_smoke"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 2
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "-c", "import time; time.sleep(0.01)"]

            [suite.runners.molt]
            run_cmd = ["{{python}}", "-c", "import time; time.sleep(0.02)"]

            [suite.runners.friend]
            run_cmd = ["{{python}}", "-c", "import time; time.sleep(0.03)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr

    results_json = output_root / "results.json"
    assert results_json.exists()
    payload = json.loads(results_json.read_text(encoding="utf-8"))
    assert payload["suites"]
    suite = payload["suites"][0]
    assert suite["id"] == "local_smoke"
    assert suite["status"] == "ok"
    assert suite["metrics"]["molt_vs_cpython_speedup"] is not None

    summary = output_root / "summary.md"
    assert summary.exists()
    summary_text = summary.read_text(encoding="utf-8")
    assert "local_smoke" in summary_text


def test_bench_friends_materializes_output_env_paths_before_prepare(
    tmp_path: Path,
) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    probe = tmp_path / "prepare_probe.py"
    probe.write_text(
        textwrap.dedent(
            """
            import os
            from pathlib import Path

            cachedb = Path(os.environ["CACHEDB"])
            cache_home = Path(os.environ["XDG_CACHE_HOME"])
            generated_root = Path(os.environ["MOLT_MODULE_ROOTS"].split(os.pathsep)[-1])
            assert cache_home.is_dir(), cache_home
            assert cachedb.parent.is_dir(), cachedb.parent
            assert generated_root.is_dir(), generated_root
            assert not cachedb.exists(), cachedb
            cachedb.write_text("ok\\n", encoding="utf-8")
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "env_path_smoke"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30
            env = {{ CACHEDB = "{{output_root}}/cache/tinygrad/cache.db", XDG_CACHE_HOME = "{{output_root}}/cache", MOLT_MODULE_ROOTS = "{{suite_root}}{{pathsep}}{{output_root}}/generated_modules" }}
            prepare_cmds = [
              ["{{python}}", "{probe.as_posix()}"],
            ]

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "-c", "print('ok')"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr
    assert (output_root / "cache").is_dir()
    assert (output_root / "cache" / "tinygrad").is_dir()
    assert (output_root / "generated_modules").is_dir()
    assert (output_root / "cache" / "tinygrad" / "cache.db").read_text(
        encoding="utf-8"
    ) == "ok\n"


def test_bench_friends_include_disabled_with_dry_run(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "dry_out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "disabled_suite"
            enabled = false
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "requires_adapter"

            [suite.runners.cpython]
            skip_reason = "not configured"
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--include-disabled",
        "--dry-run",
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    assert payload["suites"][0]["status"] == "skipped"


def test_bench_friends_interrupt_writes_partial_results_and_cleans_daemon(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_tool_module()
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "interrupted_out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "interrupt_suite"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.molt]
            run_cmd = ["{{python}}", "-c", "print('never reaches runner')"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    class DummySignalScope:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            return None

    cleanup_calls: list[dict[str, object]] = []

    def fake_run_runner(*args, **kwargs):  # noqa: ANN002, ANN003
        raise module.BenchInterrupted(signal.SIGTERM)

    def fake_cleanup_backend_daemons(**kwargs):  # noqa: ANN003
        cleanup_calls.append(kwargs)
        return {
            "status": "ok",
            "reason": kwargs["reason"],
            "session_id": kwargs["run_env"].get("MOLT_SESSION_ID", ""),
            "terminated_count": 1,
            "terminated": [{"pid": 1234}],
        }

    monkeypatch.setattr(module, "BenchSignalScope", DummySignalScope)
    monkeypatch.setattr(module, "_run_runner", fake_run_runner)
    monkeypatch.setattr(
        module, "_cleanup_backend_daemons", fake_cleanup_backend_daemons
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bench_friends.py",
            "--manifest",
            str(manifest),
            "--output-root",
            str(output_root),
        ],
    )

    assert module.main() == 143

    assert cleanup_calls
    assert cleanup_calls[0]["reason"] == "interrupted"
    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    assert payload["interrupted"]["signame"] == "SIGTERM"
    assert payload["backend_daemon_cleanup"][0]["reason"] == "interrupted"
    assert payload["backend_daemon_cleanup"][0]["terminated_count"] == 1
    summary = (output_root / "summary.md").read_text(encoding="utf-8")
    assert "## Interruption" in summary
    assert "## Backend Daemon Cleanup" in summary


def test_bench_friends_phase_result_preserves_memory_guard_diagnostics(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_tool_module()
    stdout_path = tmp_path / "phase.stdout.log"
    stderr_path = tmp_path / "phase.stderr.log"

    class GuardedResult(subprocess.CompletedProcess):
        elapsed_s = 1.25
        violation = None
        timed_out = False
        limit_at_violation = None
        orphaned_process_groups = (2345,)
        cargo_incremental_quarantine = None

    def fake_guarded_completed_process(*args, **kwargs):  # noqa: ANN002, ANN003
        return GuardedResult(
            args=["python3", "-c", "raise SystemExit"],
            returncode=143,
            stdout="",
            stderr="terminated\n",
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    phase = module._run_command(
        ["python3", "-c", "raise SystemExit"],
        cwd=tmp_path,
        env={},
        timeout_sec=30,
        stdout_path=stdout_path,
        stderr_path=stderr_path,
        dry_run=False,
        limits=module.harness_memory_guard.limits_from_env("MOLT_BENCH", {}),
    )

    payload = module._phase_to_dict(phase)
    assert payload["returncode"] == 143
    assert payload["guard_status"] == "signal_exit"
    assert payload["guard_orphaned_process_groups"] == [2345]
    assert payload["guard_exit_signal"]["name"] == "SIGTERM"
    assert payload["guard_violation"] is None


def test_bench_friends_molt_runner_classifies_daemon_empty_response(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_tool_module()

    class GuardedResult(subprocess.CompletedProcess):
        elapsed_s = 208.19
        violation = None
        timed_out = False
        limit_at_violation = None
        orphaned_process_groups = ()
        cargo_incremental_quarantine = None

    def fake_guarded_completed_process(*args, **kwargs):  # noqa: ANN002, ANN003
        return GuardedResult(
            args=["python3", "-m", "molt.cli", "run"],
            returncode=1,
            stdout="",
            stderr=(
                "Backend daemon compile failed: "
                "backend daemon returned empty response\n"
            ),
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    runner = module.RunnerSpec(
        name="molt",
        role="workload",
        build_cmd=None,
        run_cmd=["python3", "-m", "molt.cli", "run"],
        env={},
        skip_reason=None,
        json_stdout=False,
    )
    suite = module.SuiteSpec(
        id="tinygrad_off_the_shelf",
        friend="tinygrad",
        display_name="tinygrad",
        enabled=True,
        source="local",
        repo_url=None,
        repo_ref=None,
        local_path=None,
        workdir=None,
        semantic_mode="runs_unmodified",
        adapter_notes=None,
        tags=[],
        timeout_sec=300,
        repeat=1,
        env={},
        prepare_cmds=[],
        runners={"molt": runner},
    )

    result = module._run_runner(
        runner,
        suite=suite,
        suite_workdir=tmp_path,
        suite_env={},
        tokens={},
        logs_dir=tmp_path / "logs",
        dry_run=False,
        limits=module.harness_memory_guard.limits_from_env("MOLT_BENCH", {}),
    )

    payload = module._runner_to_dict(result)
    assert result.status == "failed"
    assert result.reason == (
        "run 1 failed: daemon_crash (backend_daemon_empty_response)"
    )
    assert payload["molt_failure"]["phase"] == "build"
    assert payload["molt_failure"]["status"] == "daemon_crash"
    assert payload["molt_failure"]["detail"] == "backend_daemon_empty_response"
    assert payload["molt_failure"]["log_refs"][0]["kind"] == "stdout"
    assert payload["molt_failure"]["log_refs"][1]["kind"] == "stderr"
    assert payload["runs"][0]["molt_failure"] == payload["molt_failure"]


def test_bench_friends_daemon_failure_writes_custody_artifacts(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_tool_module()
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "daemon_failure_out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "tinygrad_off_the_shelf"
            enabled = true
            friend = "tinygrad"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.molt]
            run_cmd = ["{{python}}", "-m", "molt.cli", "run"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    def fake_run_runner(runner, **kwargs):  # noqa: ANN001, ANN003
        logs_dir = kwargs["logs_dir"]
        stdout_path = logs_dir / "molt.run1.stdout.log"
        stderr_path = logs_dir / "molt.run1.stderr.log"
        failure = {
            "phase": "build",
            "status": "daemon_crash",
            "detail": "backend_daemon_empty_response",
            "message": (
                "Backend daemon compile failed: backend daemon returned empty response"
            ),
            "returncode": 1,
            "timed_out": False,
            "elapsed_s": 208.19,
            "signal": None,
            "guard_violation": None,
            "orphaned_process_groups": [],
            "log_refs": [
                {"kind": "stdout", "path": str(stdout_path)},
                {"kind": "stderr", "path": str(stderr_path)},
            ],
        }
        phase = module.PhaseResult(
            cmd=["python3", "-m", "molt.cli", "run"],
            returncode=1,
            elapsed_s=208.19,
            timed_out=False,
            stdout_path=str(stdout_path),
            stderr_path=str(stderr_path),
            guard_status="failed",
            molt_failure=failure,
        )
        return module.RunnerResult(
            name=runner.name,
            role=runner.role,
            status="failed",
            reason="run 1 failed: daemon_crash (backend_daemon_empty_response)",
            runs=[phase],
            molt_failure=failure,
        )

    monkeypatch.setattr(module, "_run_runner", fake_run_runner)
    monkeypatch.setattr(
        module,
        "_cleanup_backend_daemons",
        lambda **kwargs: {
            "status": "ok",
            "reason": kwargs["reason"],
            "terminated_count": 0,
            "terminated": [],
        },
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bench_friends.py",
            "--manifest",
            str(manifest),
            "--output-root",
            str(output_root),
        ],
    )

    assert module.main() == 1

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    assert payload["partial"] is False
    assert payload["custody_artifacts"]["summary_md"] == str(
        (output_root / "summary.md").resolve()
    )
    cleanup_sidecar = Path(payload["custody_artifacts"]["backend_daemon_cleanup_jsonl"])
    assert cleanup_sidecar.name == "backend_daemon_cleanup.jsonl"
    assert cleanup_sidecar.parent.name == "memory_guard"
    details = payload["molt_failure_details"]
    assert details["total"] == 1
    assert details["records"][0]["suite"] == "tinygrad_off_the_shelf"
    assert details["records"][0]["detail"] == "backend_daemon_empty_response"
    detail_sidecar = Path(payload["custody_artifacts"]["molt_failure_details_jsonl"])
    assert detail_sidecar.exists()
    assert "backend_daemon_empty_response" in detail_sidecar.read_text(encoding="utf-8")
    summary = (output_root / "summary.md").read_text(encoding="utf-8")
    assert "## Custody Artifacts" in summary
    assert "## Molt Failure Details" in summary
    assert "backend_daemon_empty_response" in summary
    assert "molt.run1.stderr.log" in summary


def test_bench_friends_sentinel_violation_emergency_writes_results(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_tool_module()
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "sentinel_out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "sentinel_suite"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.molt]
            run_cmd = ["{{python}}", "-c", "print('never reaches runner')"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    class DummySignalScope:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            return None

    class FakeSentinel:
        def __init__(self, **kwargs):
            self._on_violation = kwargs["on_violation"]

        def __enter__(self):
            self._on_violation(
                object(),
                object(),
                {
                    "event": "repo_process_guard_tripped",
                    "guard_started_at": "2026-06-12T00:00:00Z",
                    "observed_at": "2026-06-12T00:00:01Z",
                    "elapsed_s": 1.0,
                    "active_pgids": [123],
                    "kill_scope": "current-tree",
                    "victim_pgid": 123,
                    "victim_command": "molt-backend --daemon",
                    "action": "terminated process group",
                    "limits": {"max_process_rss_gb": 12.0},
                    "violation": {
                        "pgid": 123,
                        "reason": "process_rss",
                        "peak_rss_gb": 12.01,
                    },
                },
            )
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            return None

    def fake_run_runner(*args, **kwargs):  # noqa: ANN002, ANN003
        raise module.BenchInterrupted(signal.SIGTERM)

    monkeypatch.setattr(module, "BenchSignalScope", DummySignalScope)
    monkeypatch.setattr(
        module.harness_memory_guard,
        "repo_process_sentinel",
        lambda **kwargs: FakeSentinel(**kwargs),
    )
    monkeypatch.setattr(module, "_run_runner", fake_run_runner)
    monkeypatch.setattr(
        module,
        "_cleanup_backend_daemons",
        lambda **kwargs: {
            "status": "ok",
            "reason": kwargs["reason"],
            "terminated_count": 0,
            "terminated": [],
        },
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bench_friends.py",
            "--manifest",
            str(manifest),
            "--output-root",
            str(output_root),
        ],
    )

    assert module.main() == 143

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    assert payload["memory_guard_incidents"][0]["violation"]["reason"] == "process_rss"
    assert payload["memory_guard_incidents"][0]["victim_pgid"] == 123
    summary = (output_root / "summary.md").read_text(encoding="utf-8")
    assert "## Memory Guard Incidents" in summary
    assert "process_rss" in summary


def test_bench_friends_nuitka_pyodide_runners(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_ext"
    suite_root = tmp_path / "suite_ext"
    suite_root.mkdir(parents=True, exist_ok=True)

    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "ext_runners_smoke"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "requires_adapter"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "-c", "import time; time.sleep(0.01)"]

            [suite.runners.molt]
            run_cmd = ["{{python}}", "-c", "import time; time.sleep(0.02)"]

            [suite.runners.nuitka]
            run_cmd = ["{{python}}", "-c", "import time; time.sleep(0.03)"]

            [suite.runners.pyodide]
            run_cmd = ["{{python}}", "-c", "import time; time.sleep(0.04)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "ok"
    assert suite["runners"]["nuitka"]["status"] == "ok"
    assert suite["runners"]["pyodide"]["status"] == "ok"
    assert suite["metrics"]["nuitka_median_s"] is not None
    assert suite["metrics"]["pyodide_median_s"] is not None
    assert suite["metrics"]["molt_vs_nuitka_speedup"] is not None
    assert suite["metrics"]["molt_vs_pyodide_speedup"] is not None

    summary_text = (output_root / "summary.md").read_text(encoding="utf-8")
    assert "Nuitka s" in summary_text
    assert "Pyodide s" in summary_text
    assert "NumPy s" in summary_text


def test_bench_friends_dynamic_runner_keys_are_manifest_authority() -> None:
    module = _load_tool_module()

    suite = module._parse_suite(
        {
            "id": "dynamic_runner_smoke",
            "enabled": True,
            "friend": "tinygrad",
            "source": "local",
            "local_path": ".",
            "semantic_mode": "runs_unmodified",
            "runners": {
                "tinygrad": {
                    "run_cmd": ["{python}", "-c", "print('ok')"],
                    "structured_stdout": "json",
                }
            },
        },
        {},
    )

    assert suite.runners["tinygrad"].json_stdout is True
    assert suite.runners["tinygrad"].role == "workload"
    assert suite.runners["tinygrad"].run_cmd == ["{python}", "-c", "print('ok')"]

    with pytest.raises(ValueError, match="invalid runner name"):
        module._parse_suite(
            {
                "id": "bad_runner",
                "enabled": True,
                "friend": "local",
                "source": "local",
                "local_path": ".",
                "semantic_mode": "runs_unmodified",
                "runners": {"bad runner": {"run_cmd": ["python3", "-c", "pass"]}},
            },
            {},
        )
    with pytest.raises(ValueError, match="role must be one of"):
        module._parse_suite(
            {
                "id": "bad_role",
                "enabled": True,
                "friend": "local",
                "source": "local",
                "local_path": ".",
                "semantic_mode": "runs_unmodified",
                "runners": {
                    "audit": {
                        "role": "timed_audit",
                        "run_cmd": ["python3", "-c", "pass"],
                    }
                },
            },
            {},
        )


def test_bench_friends_token_resolution_exposes_platform_pathsep() -> None:
    module = _load_tool_module()

    assert module._resolve_tokenized(
        ["{repo_root}{pathsep}{suite_root}"],
        {
            "repo_root": "repo",
            "suite_root": "suite",
            "pathsep": os.pathsep,
        },
    ) == [f"repo{os.pathsep}suite"]


def test_bench_friends_non_workload_runner_excluded_from_speed_metrics() -> None:
    module = _load_tool_module()
    runners = {
        "molt": module.RunnerResult(
            name="molt",
            role="workload",
            status="ok",
            run_samples_s=[0.10],
            run_median_s=0.10,
            structured_median_s={"kernel": 0.10},
        ),
        "numpy": module.RunnerResult(
            name="numpy",
            role="custody_audit",
            status="ok",
            run_samples_s=[0.01],
            run_median_s=0.01,
            structured_median_s={"kernel": 0.01},
        ),
    }

    metrics = module._suite_metrics(runners)

    assert metrics["numpy_median_s"] is None
    assert metrics["numpy_time_s"] is None
    assert metrics["molt_vs_numpy_speedup"] is None
    assert "numpy_kernel_median_s" not in metrics
    assert metrics["molt_kernel_median_s"] == 0.10


def test_bench_friends_suite_metrics_serializes_ratio_directions() -> None:
    module = _load_tool_module()
    runners = {
        "cpython": module.RunnerResult(
            name="cpython",
            role="workload",
            status="ok",
            run_samples_s=[0.40],
            run_median_s=0.40,
            structured_median_s={"kernel": 0.50},
        ),
        "molt": module.RunnerResult(
            name="molt",
            role="workload",
            status="ok",
            run_samples_s=[0.20],
            run_median_s=0.20,
            structured_median_s={"kernel": 0.25},
        ),
        "friend": module.RunnerResult(
            name="friend",
            role="workload",
            status="ok",
            run_samples_s=[0.10],
            run_median_s=0.10,
            structured_median_s={"kernel": 0.125},
        ),
    }

    metrics = module._suite_metrics(runners)

    assert metrics["molt_speedup"] == pytest.approx(2.0)
    assert metrics["molt_cpython_ratio"] == pytest.approx(0.5)
    assert metrics["friend_vs_molt_speedup"] == pytest.approx(2.0)
    directions = metrics["ratio_directions"]
    assert directions["molt_speedup"] == perf_authority.RatioDirection.SPEEDUP.value
    assert (
        directions["molt_cpython_ratio"]
        == perf_authority.RatioDirection.MOLT_OVER_BASELINE.value
    )
    assert (
        directions["molt_vs_cpython_kernel_speedup"]
        == perf_authority.RatioDirection.SPEEDUP.value
    )
    assert (
        directions["molt_vs_friend_kernel_speedup"]
        == perf_authority.RatioDirection.SPEEDUP.value
    )


def test_bench_friends_suite_root_override_and_structured_json_metrics(
    tmp_path: Path,
) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_json"
    suite_root = tmp_path / "suite_override"
    suite_root.mkdir(parents=True, exist_ok=True)
    emitter = tmp_path / "emit_json.py"
    emitter.write_text(
        textwrap.dedent(
            """
            import json
            import sys

            scale = float(sys.argv[1])
            print(json.dumps({
                "status": "ok",
                "workloads": {
                    "elementwise_chain": {"elapsed_s": scale},
                    "matmul_2x2": {"elapsed_s": scale * 2.0},
                },
            }))
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "json_metrics"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{(tmp_path / "missing").as_posix()}"
            semantic_mode = "requires_adapter"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            json_stdout = true
            run_cmd = ["{{python}}", "{emitter.as_posix()}", "0.20"]

            [suite.runners.molt]
            json_stdout = true
            run_cmd = ["{{python}}", "{emitter.as_posix()}", "0.10"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--suite-root",
        f"json_metrics={suite_root}",
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "ok"
    assert suite["source_custody"]["suite_root_overridden"] is True
    assert suite["runners"]["cpython"]["structured_median_s"] == {
        "elementwise_chain": 0.2,
        "matmul_2x2": 0.4,
    }
    assert suite["runners"]["molt"]["structured_median_s"] == {
        "elementwise_chain": 0.1,
        "matmul_2x2": 0.2,
    }
    assert suite["metrics"]["cpython_elementwise_chain_median_s"] == 0.2
    assert suite["metrics"]["molt_elementwise_chain_median_s"] == 0.1
    assert suite["metrics"]["molt_vs_cpython_elementwise_chain_speedup"] == 2.0


def test_bench_friends_runner_filter_runs_only_selected_lane(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_runner_filter"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "runner_filter"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "-c", "print('cpython')"]

            [suite.runners.molt]
            run_cmd = ["{{python}}", "-c", "raise SystemExit(99)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--runner",
        "cpython",
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert payload["options"]["runner_filter"] == ["cpython"]
    assert set(suite["runners"]) == {"cpython"}
    assert suite["runners"]["cpython"]["status"] == "ok"


def test_bench_friends_git_suite_records_clean_ref_custody(tmp_path: Path) -> None:
    origin = tmp_path / "origin"
    commit = _init_git_repo(origin)
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_git"
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "git_smoke"
            enabled = true
            friend = "local"
            source = "git"
            repo_url = "{origin.as_posix()}"
            repo_ref = "{commit}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "script.py"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--repos-root",
        str(tmp_path / "repos"),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["resolved_ref"] == commit
    assert suite["requested_ref"] == commit
    assert suite["source_custody"]["expected_ref"] == commit
    assert suite["source_custody"]["ref_verified"] is True
    assert suite["source_custody"]["git_clean"] is True
    assert suite["source_custody"]["verification"] == "post_run_git_ref_and_clean_tree"


def test_bench_friends_git_suite_rejects_post_run_dirty_checkout(
    tmp_path: Path,
) -> None:
    origin = tmp_path / "origin_post_dirty"
    commit = _init_git_repo(origin)
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_post_dirty"
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "post_dirty_git"
            enabled = true
            friend = "local"
            source = "git"
            repo_url = "{origin.as_posix()}"
            repo_ref = "{commit}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "-c", "from pathlib import Path; Path('runner_dirty.py').write_text('dirty\\\\n', encoding='utf-8')"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--repos-root",
        str(tmp_path / "repos"),
    )
    assert res.returncode == 1

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "failed"
    assert "post-run source custody check failed" in suite["reason"]
    assert "runner_dirty.py" in suite["reason"]
    assert suite["runners"]["cpython"]["status"] == "ok"
    assert suite["source_custody"]["git_clean"] is False
    assert "runner_dirty.py" in suite["source_custody"]["git_status_porcelain"]
    assert suite["source_custody"]["verification"] == "post_run_git_ref_and_clean_tree"


def test_bench_friends_git_suite_rejects_post_run_ignored_artifacts(
    tmp_path: Path,
) -> None:
    origin = tmp_path / "origin_post_ignored"
    commit = _init_git_repo(origin)
    (origin / ".gitignore").write_text("build/\n", encoding="utf-8")
    _git(origin, "add", ".gitignore")
    _git(
        origin,
        "-c",
        "user.email=molt@example.invalid",
        "-c",
        "user.name=Molt Test",
        "commit",
        "-m",
        "ignore build artifacts",
    )
    commit = _git(origin, "rev-parse", "HEAD").stdout.strip()
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_post_ignored"
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "post_ignored_git"
            enabled = true
            friend = "local"
            source = "git"
            repo_url = "{origin.as_posix()}"
            repo_ref = "{commit}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "-c", "from pathlib import Path; Path('build').mkdir(); Path('build/cache.db').write_text('cache\\\\n', encoding='utf-8')"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--repos-root",
        str(tmp_path / "repos"),
    )
    assert res.returncode == 1

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "failed"
    assert "post-run source custody check failed" in suite["reason"]
    assert "ignored artifacts" in suite["reason"]
    assert "build/cache.db" in suite["reason"]
    assert suite["runners"]["cpython"]["status"] == "ok"
    assert suite["source_custody"]["git_clean"] is False
    assert "build/cache.db" in suite["source_custody"]["git_ignored_artifacts"]
    assert suite["source_custody"]["verification"] == "post_run_git_ref_and_clean_tree"


def test_bench_friends_git_suite_rejects_dirty_override_checkout(
    tmp_path: Path,
) -> None:
    checkout = tmp_path / "dirty_checkout"
    commit = _init_git_repo(checkout)
    (checkout / "untracked.py").write_text("print('dirty')\n", encoding="utf-8")
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_dirty"
    manifest.write_text(
        textwrap.dedent(
            """
            schema_version = 1

            [[suite]]
            id = "dirty_git"
            enabled = true
            friend = "local"
            source = "git"
            repo_url = "unused"
            repo_ref = "PINNED_COMMIT_REQUIRED"
            semantic_mode = "runs_unmodified"

            [suite.runners.cpython]
            run_cmd = ["{python}", "script.py"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--suite",
        "dirty_git",
        "--output-root",
        str(output_root),
        "--suite-root",
        f"dirty_git={checkout}",
        "--repo-ref",
        f"dirty_git={commit}",
        "--no-checkout",
    )
    assert res.returncode == 1
    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "failed"
    assert "git checkout is dirty" in suite["reason"]
    assert suite["source_custody"]["suite_root_overridden"] is True


def test_bench_friends_git_suite_rejects_ignored_override_artifacts(
    tmp_path: Path,
) -> None:
    checkout = tmp_path / "ignored_checkout"
    _init_git_repo(checkout)
    (checkout / ".gitignore").write_text("build/\n", encoding="utf-8")
    _git(checkout, "add", ".gitignore")
    _git(
        checkout,
        "-c",
        "user.email=molt@example.invalid",
        "-c",
        "user.name=Molt Test",
        "commit",
        "-m",
        "ignore build artifacts",
    )
    commit = _git(checkout, "rev-parse", "HEAD").stdout.strip()
    (checkout / "build").mkdir()
    (checkout / "build" / "ignored.so").write_bytes(b"not a real extension")
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_ignored"
    manifest.write_text(
        textwrap.dedent(
            """
            schema_version = 1

            [[suite]]
            id = "ignored_git"
            enabled = true
            friend = "local"
            source = "git"
            repo_url = "unused"
            repo_ref = "PINNED_COMMIT_REQUIRED"
            semantic_mode = "runs_unmodified"

            [suite.runners.cpython]
            run_cmd = ["{python}", "script.py"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--suite",
        "ignored_git",
        "--output-root",
        str(output_root),
        "--suite-root",
        f"ignored_git={checkout}",
        "--repo-ref",
        f"ignored_git={commit}",
        "--no-checkout",
    )

    assert res.returncode == 1
    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "failed"
    assert "ignored artifacts" in suite["reason"]
    assert "build/ignored.so" in suite["reason"]


def test_friend_manifest_registers_tinygrad_off_the_shelf_suite() -> None:
    module = _load_tool_module()
    _meta, suites = module._load_manifest(REPO_ROOT / "bench/friends/manifest.toml")
    suite = next(s for s in suites if s.id == "tinygrad_off_the_shelf")

    assert suite.enabled is True
    assert suite.friend == "tinygrad"
    assert suite.source == "git"
    assert suite.repo_url == "https://github.com/tinygrad/tinygrad.git"
    assert suite.repo_ref == "a83710396c991272241e40da94489747c2393851"
    assert suite.semantic_mode == "runs_unmodified"
    assert suite.env == {
        "CACHEDB": "{output_root}/tinygrad_cache/tinygrad/cache.db",
        "DEV": "PYTHON",
        "PYTHONDONTWRITEBYTECODE": "1",
        "XDG_CACHE_HOME": "{output_root}/tinygrad_cache",
    }
    assert suite.prepare_cmds == [
        [
            "{python}",
            "{repo_root}/tools/tinygrad_upat_static_exec_registry.py",
            "--suite-root",
            "{suite_root}",
            "--repo-root",
            "{repo_root}",
            "--workload",
            "all",
            "--iterations",
            "1",
            "--manifest-output",
            "{output_root}/tinygrad_static_exec/manifest.json",
            "--module-output",
            "{output_root}/tinygrad_static_exec/_molt_tinygrad_upat_static_exec_registry.py",
            "--json",
        ],
    ]
    assert {"gpu", "mlir", "tinygrad", "compatibility", "benchmark-suite"} <= set(
        suite.tags
    )

    cpython = suite.runners["cpython"]
    molt = suite.runners["molt"]
    tinygrad = suite.runners["tinygrad"]
    assert cpython.json_stdout is True
    assert molt.json_stdout is True
    assert cpython.run_cmd is not None
    assert cpython.run_cmd[:8] == [
        "uv",
        "run",
        "--isolated",
        "--no-project",
        "--python",
        "{python}",
        "--with",
        "typeguard",
    ]
    assert any(
        part.endswith("tools/tinygrad_off_shelf_adapter.py") for part in cpython.run_cmd
    )
    assert "--suite-root" in cpython.run_cmd
    assert "{suite_root}" in cpython.run_cmd
    assert cpython.env == {"DEV": "PYTHON", "PYTHONDONTWRITEBYTECODE": "1"}
    assert molt.run_cmd is not None
    assert molt.skip_reason is None
    assert molt.run_cmd[:4] == ["{project_python}", "-m", "molt.cli", "run"]
    assert "--capabilities" in molt.run_cmd
    assert "ffi.unsafe" in molt.run_cmd
    assert "--build-arg=--stdlib-profile" in molt.run_cmd
    assert "--build-arg=full" in molt.run_cmd
    assert "--build-arg=--rebuild" in molt.run_cmd
    assert "--build-arg=--no-cache" in molt.run_cmd
    assert any(
        part.endswith("tools/tinygrad_off_shelf_adapter.py") for part in molt.run_cmd
    )
    assert molt.env["DEV"] == "PYTHON"
    assert (
        molt.env["MOLT_MODULE_ROOTS"]
        == "{suite_root}{pathsep}{output_root}/tinygrad_static_exec"
    )
    assert (
        molt.env["MOLT_EXTERNAL_STATIC_PACKAGES"]
        == "tinygrad _molt_tinygrad_upat_static_exec_registry"
    )
    assert (
        molt.env["MOLT_STATIC_IMPORT_MODULES"]
        == "tinygrad.runtime.ops_python tinygrad.uop.ops _molt_tinygrad_upat_static_exec_registry"
    )
    assert (
        molt.env["MOLT_TINYGRAD_UPAT_STATIC_EXEC_ROOT"]
        == "{output_root}/tinygrad_static_exec"
    )
    assert molt.env["PYTHONDONTWRITEBYTECODE"] == "1"
    assert (
        molt.env["PYTHONPATH"]
        == "{repo_root}/src{pathsep}{suite_root}{pathsep}{output_root}/tinygrad_static_exec"
    )
    assert tinygrad.skip_reason is None
    assert tinygrad.run_cmd == [
        "uv",
        "run",
        "--isolated",
        "--no-project",
        "--python",
        "{python}",
        "--with",
        "typeguard",
        "python",
        "test/test_tiny.py",
    ]
    assert tinygrad.env == {
        "CHECK_OOB": "0",
        "DEV": "CPU",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONPATH": "{suite_root}",
        "TYPED": "1",
    }


def test_friend_manifest_does_not_register_upat_compile_diagnostic_lane() -> None:
    module = _load_tool_module()
    _meta, suites = module._load_manifest(REPO_ROOT / "bench/friends/manifest.toml")
    suite_ids = {suite.id for suite in suites}
    tinygrad_suite = next(
        suite for suite in suites if suite.id == "tinygrad_off_the_shelf"
    )

    assert "tinygrad_upat_interpret_diagnostic" not in suite_ids
    assert "molt_upat_interpret" not in tinygrad_suite.runners
    for runner in tinygrad_suite.runners.values():
        assert "UPAT_COMPILE" not in runner.env
        assert all("UPAT_COMPILE" not in part for part in (runner.run_cmd or []))


def test_friend_manifest_registers_numpy_off_the_shelf_suite() -> None:
    module = _load_tool_module()
    _meta, suites = module._load_manifest(REPO_ROOT / "bench/friends/manifest.toml")
    suite = next(s for s in suites if s.id == "numpy_off_the_shelf")

    assert suite.enabled is True
    assert suite.friend == "numpy"
    assert suite.source == "git"
    assert suite.repo_url == "https://github.com/numpy/numpy.git"
    assert suite.repo_ref == "c81c49f77451340651a751e76bca607d85e4fd55"
    assert suite.semantic_mode == "runs_unmodified"
    assert {"ecosystem", "numpy", "c-api", "scientific-python", "compile-time"} <= set(
        suite.tags
    )

    cpython = suite.runners["cpython"]
    molt = suite.runners["molt"]
    c_api_scan = suite.runners["c_api_scan"]
    source_audit = suite.runners["source_audit"]
    assert cpython.skip_reason is None
    assert cpython.role == "workload"
    assert cpython.json_stdout is True
    assert cpython.run_cmd[:7] == [
        "uv",
        "run",
        "--isolated",
        "--python",
        "{python}",
        "--with",
        "numpy==2.4.2",
    ]
    assert any(
        part.endswith("tools/numpy_off_shelf_adapter.py") for part in cpython.run_cmd
    )
    assert "--require-version" in cpython.run_cmd
    assert "2.4.2" in cpython.run_cmd
    assert molt.skip_reason is None
    assert molt.role == "workload"
    assert molt.json_stdout is True
    assert molt.run_cmd[:4] == ["{project_python}", "-m", "molt.cli", "run"]
    assert any(
        part.endswith("tools/numpy_off_shelf_adapter.py") for part in molt.run_cmd
    )
    assert "--capabilities" in molt.run_cmd
    assert "module.extension.exec" in molt.run_cmd
    assert "--require-module-under" in molt.run_cmd
    assert molt.env["MOLT_MODULE_ROOTS"] == "{suite_root}"
    assert molt.env["MOLT_EXTERNAL_STATIC_PACKAGES"] == "numpy"
    assert molt.env["PYTHONPATH"] == "{repo_root}/src:{suite_root}"
    assert c_api_scan.skip_reason is None
    assert c_api_scan.role == "c_api_scan"
    assert c_api_scan.json_stdout is True
    assert c_api_scan.run_cmd == [
        "{project_python}",
        "-m",
        "molt.cli",
        "extension",
        "scan",
        "--project",
        "{suite_root}",
        "--source",
        "{suite_root}/numpy",
        "--fail-on-missing",
        "--json",
    ]
    assert source_audit.skip_reason is None
    assert source_audit.role == "custody_audit"
    assert source_audit.json_stdout is True
    assert source_audit.run_cmd == [
        "{python}",
        "{repo_root}/tools/numpy_off_shelf_adapter.py",
        "--suite-root",
        "{suite_root}",
        "--source-tree-audit",
        "--workload",
        "none",
        "--json",
    ]


def test_tinygrad_off_shelf_adapter_runs_public_api_workloads(tmp_path: Path) -> None:
    tinygrad_pkg = tmp_path / "tinygrad"
    tinygrad_pkg.mkdir()
    (tinygrad_pkg / "__init__.py").write_text("", encoding="utf-8")
    (tinygrad_pkg / "tensor.py").write_text(
        textwrap.dedent(
            """
            class Tensor:
                def __init__(self, data):
                    self.data = data

                def _binary(self, other, op):
                    if isinstance(self.data[0], list):
                        return Tensor([
                            [op(a, b) for a, b in zip(row_a, row_b)]
                            for row_a, row_b in zip(self.data, other.data)
                        ])
                    return Tensor([op(a, b) for a, b in zip(self.data, other.data)])

                def __add__(self, other):
                    return self._binary(other, lambda a, b: a + b)

                def __mul__(self, other):
                    return self._binary(other, lambda a, b: a * b)

                def __matmul__(self, other):
                    rows = []
                    cols = list(zip(*other.data))
                    for row in self.data:
                        rows.append([sum(a * b for a, b in zip(row, col)) for col in cols])
                    return Tensor(rows)

                def scaled_dot_product_attention(
                    self,
                    key,
                    value,
                    attn_mask=None,
                    dropout_p=0.0,
                    is_causal=False,
                    enable_gqa=False,
                ):
                    import math

                    if dropout_p != 0.0:
                        raise AssertionError("fake tinygrad adapter does not support dropout")
                    if enable_gqa:
                        raise AssertionError("fake tinygrad adapter does not support gqa")
                    if is_causal:
                        raise AssertionError("attention_core passes an additive mask instead")
                    if attn_mask is None:
                        raise AssertionError("attention_core must pass an additive mask")
                    batches = []
                    for q_batch, k_batch, v_batch, mask_batch in zip(
                        self.data, key.data, value.data, attn_mask.data
                    ):
                        heads = []
                        for q_head, k_head, v_head, mask_head in zip(
                            q_batch, k_batch, v_batch, mask_batch
                        ):
                            rows = []
                            scale = 1.0 / math.sqrt(len(q_head[0]))
                            for q_vec, mask_row in zip(q_head, mask_head):
                                scores = [
                                    sum(q * k for q, k in zip(q_vec, k_vec)) * scale + mask
                                    for k_vec, mask in zip(k_head, mask_row)
                                ]
                                max_score = max(scores)
                                exps = [math.exp(score - max_score) for score in scores]
                                denom = sum(exps)
                                probs = [exp / denom for exp in exps]
                                rows.append([
                                    sum(prob * v_vec[col] for prob, v_vec in zip(probs, v_head))
                                    for col in range(len(v_head[0]))
                                ])
                            heads.append(rows)
                        batches.append(heads)
                    return Tensor(batches)

                def where(self, x, y):
                    x_data = x.data if isinstance(x, Tensor) else x
                    y_data = y.data if isinstance(y, Tensor) else y
                    return Tensor([
                        (x_data[idx] if isinstance(x_data, list) else x_data)
                        if cond
                        else (y_data[idx] if isinstance(y_data, list) else y_data)
                        for idx, cond in enumerate(self.data)
                    ])

                def pad(self, padding):
                    row_pad, col_pad = list(reversed([
                        tuple(padding[idx:idx + 2])
                        for idx in range(0, len(padding), 2)
                    ]))
                    top, bottom = row_pad
                    left, right = col_pad
                    width = len(self.data[0])
                    padded = [[0.0] * (width + left + right) for _ in range(top)]
                    padded.extend(
                        [[0.0] * left + row + [0.0] * right for row in self.data]
                    )
                    padded.extend([[0.0] * (width + left + right) for _ in range(bottom)])
                    return Tensor(padded)

                def shrink(self, bounds):
                    (row_start, row_end), (col_start, col_end) = bounds
                    return Tensor([
                        row[col_start:col_end]
                        for row in self.data[row_start:row_end]
                    ])

                def flip(self, axis):
                    if axis == 1:
                        return Tensor([list(reversed(row)) for row in self.data])
                    return Tensor(list(reversed(self.data)))

                def contiguous(self):
                    return Tensor([list(row) for row in self.data])

                def realize(self):
                    return self

                def numpy(self):
                    return self

                def tolist(self):
                    return self.data
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = run_native_test_process(
        [
            sys.executable,
            "tools/tinygrad_off_shelf_adapter.py",
            "--suite-root",
            str(tmp_path),
            "--workload",
            "all",
            "--iterations",
            "2",
            "--json",
        ],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert sorted(payload["workloads"]) == [
        "attention_core",
        "elementwise_chain",
        "matmul_2x2",
        "movement_views",
        "where_promotion",
    ]
    assert payload["workloads"]["attention_core"]["result"] == [
        [[[10.0, 1.0], [2.0, 20.0]]],
    ]
    assert payload["workloads"]["elementwise_chain"]["result"] == [
        5.0,
        10.0,
        15.0,
        20.0,
    ]
    assert payload["workloads"]["matmul_2x2"]["result"] == [
        [19.0, 22.0],
        [43.0, 50.0],
    ]
    assert payload["workloads"]["where_promotion"]["result"] == [
        5,
        2.5,
        5,
        4.5,
    ]
    assert payload["workloads"]["movement_views"]["result"] == [
        [5.0, 4.0],
        [0.0, 0.0],
    ]
    assert not list(tmp_path.rglob("__pycache__"))


def test_tinygrad_off_shelf_adapter_installs_static_upat_exec_registry(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    module = _load_tinygrad_adapter_module()
    tinygrad_pkg = tmp_path / "tinygrad"
    (tinygrad_pkg / "uop").mkdir(parents=True)
    (tinygrad_pkg / "__init__.py").write_text("", encoding="utf-8")
    (tinygrad_pkg / "tensor.py").write_text(
        "class Tensor:\n    pass\n", encoding="utf-8"
    )
    (tinygrad_pkg / "uop" / "__init__.py").write_text("", encoding="utf-8")
    (tinygrad_pkg / "uop" / "upat.py").write_text(
        textwrap.dedent(
            """
            exec = 'upstream dynamic exec placeholder'

            def compile_matcher(namespace):
                exec("# match for fake\\n", {}, namespace)
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    registry_root = tmp_path / "registry"
    registry_root.mkdir()
    (registry_root / "_molt_tinygrad_upat_static_exec_registry.py").write_text(
        textwrap.dedent(
            """
            def exec_static(source, globals=None, locals=None):
                if locals is None:
                    locals = globals
                locals["compiled_match"] = lambda uop, ctx: ("static", uop, ctx)
                return None
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    saved_modules = {
        name: sys.modules.get(name)
        for name in (
            "_molt_tinygrad_upat_static_exec_registry",
            "tinygrad",
            "tinygrad.tensor",
            "tinygrad.uop",
            "tinygrad.uop.upat",
        )
        if name in sys.modules
    }
    for name in (
        "_molt_tinygrad_upat_static_exec_registry",
        "tinygrad",
        "tinygrad.tensor",
        "tinygrad.uop",
        "tinygrad.uop.upat",
    ):
        sys.modules.pop(name, None)
    monkeypatch.setenv(
        module.STATIC_EXEC_REGISTRY_ROOT_ENV,
        str(registry_root),
    )
    original_path = list(sys.path)
    try:
        with module._suite_root_import_path(tmp_path):
            tinygrad = module._import_tinygrad()
            assert module._install_tinygrad_upat_static_exec_registry(tinygrad) is True
            from tinygrad.uop import upat

            namespace: dict[str, object] = {}
            upat.compile_matcher(namespace)
            assert namespace["compiled_match"]("uop", "ctx") == ("static", "uop", "ctx")
            assert getattr(tinygrad, "_molt_upat_static_exec_registry").__name__ == (
                "_molt_tinygrad_upat_static_exec_registry"
            )
    finally:
        sys.path[:] = original_path
        for name in (
            "_molt_tinygrad_upat_static_exec_registry",
            "tinygrad",
            "tinygrad.tensor",
            "tinygrad.uop",
            "tinygrad.uop.upat",
        ):
            sys.modules.pop(name, None)
        sys.modules.update(saved_modules)


def test_tinygrad_off_shelf_adapter_propagates_upat_interpret_nameerror(
    tmp_path: Path,
) -> None:
    tinygrad_pkg = tmp_path / "tinygrad"
    tinygrad_pkg.mkdir()
    (tinygrad_pkg / "__init__.py").write_text("", encoding="utf-8")
    (tinygrad_pkg / "tensor.py").write_text(
        textwrap.dedent(
            """
            import os

            class Tensor:
                def __init__(self, data):
                    self.data = data

                def scaled_dot_product_attention(
                    self,
                    key,
                    value,
                    attn_mask=None,
                    dropout_p=0.0,
                    is_causal=False,
                    enable_gqa=False,
                ):
                    if os.environ.get("UPAT_COMPILE") == "0":
                        raise NameError("name 'do_substitute' is not defined")
                    return Tensor([[[[10.0, 1.0], [2.0, 20.0]]]])
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = run_native_test_process(
        [
            sys.executable,
            "tools/tinygrad_off_shelf_adapter.py",
            "--suite-root",
            str(tmp_path),
            "--workload",
            "attention_core",
            "--iterations",
            "1",
            "--json",
        ],
        cwd=REPO_ROOT,
        env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1", "UPAT_COMPILE": "0"},
        text=True,
        capture_output=True,
        check=False,
    )

    assert res.returncode == 1
    assert "NameError: name 'do_substitute' is not defined" in res.stderr
    assert not list(tmp_path.rglob("__pycache__"))


def test_tinygrad_off_shelf_adapter_import_restores_process_state(
    tmp_path: Path,
) -> None:
    module = _load_tinygrad_adapter_module()
    tinygrad_pkg = tmp_path / "tinygrad"
    tinygrad_pkg.mkdir()
    (tinygrad_pkg / "__init__.py").write_text("", encoding="utf-8")
    (tinygrad_pkg / "tensor.py").write_text(
        "class Tensor:\n    pass\n", encoding="utf-8"
    )

    saved_modules = {
        name: sys.modules.get(name)
        for name in ("tinygrad", "tinygrad.tensor")
        if name in sys.modules
    }
    for name in ("tinygrad", "tinygrad.tensor"):
        sys.modules.pop(name, None)
    original_path = list(sys.path)
    original_dont_write_bytecode = sys.dont_write_bytecode
    try:
        with (
            module._suppress_bytecode_writes(),
            module._suite_root_import_path(tmp_path),
        ):
            tinygrad = module._import_tinygrad()
            assert tinygrad.Tensor.__name__ == "Tensor"
            assert sys.dont_write_bytecode is True
            assert sys.path[0] == str(tmp_path.resolve())
        assert sys.path == original_path
        assert sys.dont_write_bytecode == original_dont_write_bytecode
    finally:
        for name in ("tinygrad", "tinygrad.tensor"):
            sys.modules.pop(name, None)
        sys.modules.update(saved_modules)


def test_tinygrad_off_shelf_adapter_prefers_tolist_without_numpy() -> None:
    module = _load_tinygrad_adapter_module()

    class TinygradLikeTensor:
        def tolist(self):
            return [1.0, 2.0]

        def numpy(self):
            raise AssertionError("adapter should not require numpy when tolist exists")

    assert module._as_nested_list(TinygradLikeTensor()) == [1.0, 2.0]


def test_numpy_off_shelf_adapter_source_tree_audit(tmp_path: Path, capsys) -> None:
    module = _load_numpy_adapter_module()
    suite_root = tmp_path / "numpy_src"
    (suite_root / "numpy" / "_core").mkdir(parents=True)
    (suite_root / "numpy" / "__init__.py").write_text("__version__ = '2.4.2'\n")
    (suite_root / "pyproject.toml").write_text("[project]\nname = 'numpy'\n")

    rc = module.main(
        [
            "--suite-root",
            str(suite_root),
            "--source-tree-audit",
            "--workload",
            "none",
            "--json",
        ]
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    assert payload["source_tree"]["required_paths"] == [
        "pyproject.toml",
        "numpy/__init__.py",
        "numpy/_core",
    ]


def test_numpy_off_shelf_adapter_rejects_loaded_submodule_origin_escape(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_numpy_adapter_module()
    suite_root = tmp_path / "numpy_src"
    outside_root = tmp_path / "outside"
    (suite_root / "numpy").mkdir(parents=True)
    (outside_root / "numpy").mkdir(parents=True)
    top_level = suite_root / "numpy" / "__init__.py"
    escaped = outside_root / "numpy" / "_core.py"
    top_level.write_text("__version__ = '2.4.2'\n", encoding="utf-8")
    escaped.write_text("VALUE = 1\n", encoding="utf-8")
    for name in list(sys.modules):
        if name == "numpy" or name.startswith("numpy."):
            monkeypatch.delitem(sys.modules, name, raising=False)
    monkeypatch.setitem(
        sys.modules,
        "numpy",
        types.SimpleNamespace(__file__=str(top_level)),
    )
    monkeypatch.setitem(
        sys.modules,
        "numpy._core",
        types.SimpleNamespace(__file__=str(escaped)),
    )

    with pytest.raises(RuntimeError, match="numpy._core"):
        module._audit_loaded_numpy_modules(require_module_under=suite_root)


def test_numpy_off_shelf_adapter_runs_public_api_workloads(capsys) -> None:
    np = pytest.importorskip("numpy")
    module = _load_numpy_adapter_module()

    rc = module.main(
        [
            "--workload",
            "all",
            "--iterations",
            "1",
            "--json",
            "--require-version",
            np.__version__,
        ]
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    assert payload["numpy_version"] == np.__version__
    assert payload["numpy_modules"]["modules"]
    assert payload["numpy_modules"]["required_root"] is None
    assert sorted(payload["workloads"]) == [
        "array_dtype_shape_tolist",
        "broadcast_where",
        "matmul_2x2",
        "sum_reshape",
    ]
