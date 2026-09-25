from __future__ import annotations

from molt.verified_subset import load_verified_subset_policy

import importlib.util
import os
import queue
import subprocess
import sys
from contextlib import contextmanager
from pathlib import Path
from types import ModuleType

import pytest

from tools.compat import test_policy


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tests" / "molt_diff.py"


def _load_diff_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "molt_diff_module_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _configure_fixture_cpython_runner(
    module: ModuleType,
    monkeypatch: pytest.MonkeyPatch,
    *,
    cwd: Path,
    tmp_path: Path,
) -> None:
    def run_from_fixture_cwd(
        cmd: list[str], *, env: dict[str, str], timeout: float | None
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            cmd,
            cwd=cwd,
            env=env,
            timeout=timeout,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="surrogateescape",
            check=False,
        )

    monkeypatch.setattr(module, "_apply_memory_limit", lambda: None)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda _path: {})
    monkeypatch.setattr(
        module, "_diff_tmp_root", lambda _environment=None: tmp_path / "diff-tmp"
    )
    monkeypatch.setattr(module, "_diff_timeout", lambda: 30.0)
    monkeypatch.setattr(
        module, "_resolve_python_command", lambda _python: [sys.executable]
    )
    monkeypatch.setattr(module, "_run_subprocess", run_from_fixture_cwd)


@pytest.mark.parametrize("script_reference", ["absolute", "relative"])
@pytest.mark.parametrize("sibling_kind", ["module", "package"])
def test_run_cpython_uses_script_directory_as_import_authority(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    script_reference: str,
    sibling_kind: str,
) -> None:
    module = _load_diff_module()
    cwd = tmp_path / "cwd"
    script_dir = tmp_path / "scripts"
    cwd.mkdir()
    script_dir.mkdir()

    if sibling_kind == "module":
        (script_dir / "reference_sibling.py").write_text(
            "SOURCE = 'script-module'\n", encoding="utf-8"
        )
        cwd_shadow = cwd / "reference_sibling"
        cwd_shadow.mkdir()
        (cwd_shadow / "__init__.py").write_text(
            "SOURCE = 'cwd-package'\n", encoding="utf-8"
        )
        expected_source = "script-module"
    else:
        script_sibling = script_dir / "reference_sibling"
        script_sibling.mkdir()
        (script_sibling / "__init__.py").write_text(
            "SOURCE = 'script-package'\n", encoding="utf-8"
        )
        (cwd / "reference_sibling.py").write_text(
            "SOURCE = 'cwd-module'\n", encoding="utf-8"
        )
        expected_source = "script-package"

    script = script_dir / "case.py"
    script.write_text(
        "import __main__\n"
        "import builtins\n"
        "import pathlib\n"
        "import sys\n"
        "from importlib.machinery import SourceFileLoader\n"
        "import reference_sibling\n"
        "print(reference_sibling.SOURCE)\n"
        "print(\n"
        "    pathlib.Path(sys.path[0]).resolve()\n"
        "    == pathlib.Path(__file__).parent.resolve()\n"
        ")\n"
        "print(__main__ is sys.modules['__main__'])\n"
        "print(globals() is vars(__main__))\n"
        "print(sys.argv[0])\n"
        "print(len(sys.argv) == 1)\n"
        "print(pathlib.Path(__file__).is_absolute())\n"
        "print(\n"
        "    __name__ == '__main__'\n"
        "    and __package__ is None\n"
        "    and __spec__ is None\n"
        "    and isinstance(__loader__, SourceFileLoader)\n"
        "    and __loader__.path == __file__\n"
        "    and __cached__ is None\n"
        "    and __builtins__ is builtins\n"
        ")\n"
        "print('_molt_diff_execute_script' not in globals())\n",
        encoding="utf-8",
    )
    requested_path = (
        str(script.resolve())
        if script_reference == "absolute"
        else os.path.relpath(script, cwd)
    )

    _configure_fixture_cpython_runner(module, monkeypatch, cwd=cwd, tmp_path=tmp_path)

    result = module.run_cpython(requested_path, sys.executable)
    stdout, stderr, returncode = result.stdout, result.stderr, result.returncode

    assert returncode == 0, stderr
    assert stderr == ""
    assert stdout.splitlines() == [
        expected_source,
        "True",
        "True",
        "True",
        requested_path,
        "True",
        "True",
        "True",
        "True",
    ]


def test_run_cpython_keeps_main_namespace_through_atexit_closures(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _load_diff_module()
    cwd = tmp_path / "cwd"
    script_dir = tmp_path / "scripts"
    cwd.mkdir()
    script_dir.mkdir()
    script = script_dir / "atexit_case.py"
    script.write_text(
        "import __main__\n"
        "import atexit\n"
        "import sys\n"
        "def register(namespace, main_module):\n"
        "    def report():\n"
        "        current = sys.modules['__main__']\n"
        "        print(\n"
        "            'atexit',\n"
        "            main_module is current,\n"
        "            namespace is vars(current),\n"
        "            report.__globals__ is namespace,\n"
        "        )\n"
        "    atexit.register(report)\n"
        "register(globals(), __main__)\n"
        "print('body', globals() is vars(sys.modules['__main__']))\n",
        encoding="utf-8",
    )
    _configure_fixture_cpython_runner(module, monkeypatch, cwd=cwd, tmp_path=tmp_path)

    result = module.run_cpython(str(script), sys.executable)
    stdout, stderr, returncode = result.stdout, result.stderr, result.returncode

    assert returncode == 0, stderr
    assert stderr == ""
    assert stdout.splitlines() == ["body True", "atexit True True True"]


def test_diff_capabilities_prefers_explicit_diff_override_then_test_contract() -> None:
    module = _load_diff_module()

    assert module._diff_capabilities({}) == "fs,env,time,random"
    assert module._diff_capabilities({"MOLT_CAPABILITIES": "process.exec"}) == (
        "process.exec"
    )
    assert module._diff_capabilities({"MOLT_CAPABILITIES": ""}) == ""
    assert (
        module._diff_capabilities(
            {
                "MOLT_CAPABILITIES": "process.exec",
                "MOLT_DIFF_CAPABILITIES": "fs,env,time,random",
            }
        )
        == "fs,env,time,random"
    )


def test_expected_failure_status_maps_fail_to_xfail_pass() -> None:
    status, reason = test_policy.resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="fail",
        cpython_returncode=0,
    )
    assert status == "pass"
    assert reason == "xfail"


def test_expected_failure_status_maps_pass_to_xpass_fail() -> None:
    status, reason = test_policy.resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="pass",
        cpython_returncode=0,
    )
    assert status == "fail"
    assert reason == "xpass"


def test_expected_failure_status_ignored_when_cpython_fails() -> None:
    status, reason = test_policy.resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="fail",
        cpython_returncode=1,
    )
    assert status == "fail"
    assert reason is None


def test_deterministic_compiler_panic_does_not_trigger_backend_retry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _load_diff_module()
    calls: list[dict[str, object]] = []
    stderr = (
        "backend compilation failed\n"
        "thread 'main' panicked at runtime/molt-passes/src/tir/passes/"
        "async_work_poll.rs:152: zero-payload exception edge"
    )

    def fail_once(*args: object, **kwargs: object):
        calls.append({"args": args, "kwargs": kwargs})
        return module.compat_backends.BackendResult(None, stderr, 1, build_failed=True)

    monkeypatch.setattr(module, "run_molt", fail_once)

    context = module.compat_backends.BackendExecutionContext(
        target_python=module.TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment={"MOLT_CAPABILITY_TIER": "none"},
    )
    assert module._run_native_backend(
        "case.py", context
    ) == module.compat_backends.BackendResult(None, stderr, 1, build_failed=True)
    assert len(calls) == 1
    assert calls[0]["kwargs"]["execution_context"] is context
    assert not module._is_backend_daemon_build_error(stderr)


def test_molt_target_python_must_match_cpython_oracle(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _load_diff_module()
    monkeypatch.setattr(module, "_python_exe_version", lambda _python: (3, 14))

    assert module._resolve_molt_target_python("python", None).short == "3.14"
    assert module._resolve_molt_target_python("python", "3.14").short == "3.14"
    with pytest.raises(ValueError, match="does not match the CPython oracle"):
        module._resolve_molt_target_python("python", "3.13")


@pytest.mark.parametrize("retry_isolated", [None, "0", "1"])
@pytest.mark.parametrize("stderr", ["build timeout after 1200s", "execution timed out"])
def test_timeout_preserves_original_failure_without_cold_rebuild(
    monkeypatch: pytest.MonkeyPatch, retry_isolated: str | None, stderr: str
) -> None:
    module = _load_diff_module()
    if retry_isolated is None:
        monkeypatch.delenv("MOLT_DIFF_RETRY_ISOLATED", raising=False)
    else:
        monkeypatch.setenv("MOLT_DIFF_RETRY_ISOLATED", retry_isolated)
    calls: list[dict[str, object]] = []
    failure = module.compat_backends.BackendResult(
        None, stderr, 124, build_failed=True, timed_out=True
    )

    def timed_out(*args: object, **kwargs: object):
        calls.append(kwargs)
        return failure

    def forbidden_isolation(*args: object, **kwargs: object) -> None:
        pytest.fail("a timeout is not evidence of cache corruption")

    monkeypatch.setattr(module, "run_molt", timed_out)
    monkeypatch.setattr(module, "_run_isolated_retry", forbidden_isolation)
    context = module.compat_backends.BackendExecutionContext(
        target_python=module.TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment={"MOLT_CAPABILITY_TIER": "none"},
    )
    assert module._run_native_backend("case.py", context) == failure
    assert calls == [{"execution_context": context}]


@pytest.mark.parametrize(
    "stderr",
    [
        "backend daemon failed to become ready",
        "backend daemon connection failed: timeout",
        "backend daemon returned invalid JSON: bad payload",
        "IncompatibleSignature(expected, actual)",
    ],
)
def test_backend_retry_classifier_accepts_only_explicit_daemon_failures(
    stderr: str,
) -> None:
    module = _load_diff_module()
    assert module._is_backend_daemon_build_error(stderr)


def test_source_scope_marks_exec_eval_cases(tmp_path: Path) -> None:
    basic = tmp_path / "tests" / "differential" / "basic"
    basic.mkdir(parents=True)
    for name in ("exec_locals_scope.py", "eval_locals_scope.py"):
        (basic / name).write_text(
            "# MOLT_META: verified_subset_scope=dynamic_execution_policy "
            "expect_fail=molt expect_fail_reason=too_dynamic_policy\n",
            encoding="utf-8",
        )
    (basic / "arith.py").write_text("print(1)\n", encoding="utf-8")
    declared = test_policy.verification_scope_paths(
        (("tests/differential/basic", False),),
        scope="dynamic_execution_policy",
        repo_root=tmp_path,
    )

    assert "tests/differential/basic/exec_locals_scope.py" in declared
    assert "tests/differential/basic/eval_locals_scope.py" in declared
    assert "tests/differential/basic/arith.py" not in declared


def test_repo_source_scopes_cover_all_exec_eval_cases() -> None:
    policy = load_verified_subset_policy()
    declared = test_policy.verification_scope_paths(
        policy.suite_selectors,
        scope="dynamic_execution_policy",
    )

    basic_dir = REPO_ROOT / "tests" / "differential" / "basic"
    required = {
        f"tests/differential/basic/{path.name}" for path in basic_dir.glob("exec*.py")
    } | {f"tests/differential/basic/{path.name}" for path in basic_dir.glob("eval*.py")}

    missing = sorted(required - declared)
    assert not missing


def test_repo_scopes_have_no_policy_deferred_runpy_dynamic_cases() -> None:
    policy = load_verified_subset_policy()
    declared = test_policy.verification_scope_paths(
        policy.suite_selectors,
        scope="dynamic_execution_policy",
    )
    deferred_runpy = sorted(path for path in declared if "/stdlib/runpy_" in path)
    assert not deferred_runpy


def test_rss_top_entries_use_final_file_status_after_retries(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    run_id = "rss_status_regression"
    metrics_path = tmp_path / "rss_metrics.jsonl"
    metrics_path.write_text(
        "\n".join(
            (
                '{"run_id":"rss_status_regression","timestamp":1.0,'
                '"file":"tests/differential/stdlib/zipimport_basic.py",'
                '"status":"run_failed","build":{"max_rss":700000},'
                '"run":{"max_rss":20000},"build_rc":0,"run_rc":1}',
                '{"run_id":"rss_status_regression","timestamp":2.0,'
                '"file":"tests/differential/stdlib/zipimport_basic.py",'
                '"status":"ok","build":{"max_rss":680000},'
                '"run":{"max_rss":15000},"build_rc":0,"run_rc":0}',
            )
        )
        + "\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("MOLT_DIFF_MEASURE_RSS", "1")
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path))

    top = module._top_rss_entries(run_id, 5, phase="run")
    assert len(top) == 1
    assert top[0]["status"] == "ok"
    # Keep max RSS from all attempts for worst-case memory visibility.
    assert top[0]["run"]["max_rss"] == 20000


def test_rss_display_status_prefers_final_diff_status() -> None:
    module = _load_diff_module()
    entry = {
        "file": "tests/differential/stdlib/zipimport_basic.py",
        "status": "run_failed",
    }
    resolved = module._rss_display_status(
        entry,
        {"tests/differential/stdlib/zipimport_basic.py": "pass"},
    )
    assert resolved == "pass"


def test_rss_display_status_matches_absolute_and_repo_relative_paths() -> None:
    module = _load_diff_module()
    absolute = str(
        (
            REPO_ROOT / "tests" / "differential" / "stdlib" / "zipimport_basic.py"
        ).resolve()
    )
    entry = {
        "file": absolute,
        "status": "run_failed",
    }
    resolved = module._rss_display_status(
        entry,
        {"tests/differential/stdlib/zipimport_basic.py": "pass"},
    )
    assert resolved == "pass"


def test_rss_display_status_normalizes_raw_run_failed_without_lookup() -> None:
    module = _load_diff_module()
    entry = {
        "file": "tests/differential/stdlib/zipimport_basic.py",
        "status": "run_failed",
    }
    resolved = module._rss_display_status(entry, {})
    assert resolved == "fail"


def test_rss_display_status_normalizes_raw_ok_without_lookup() -> None:
    module = _load_diff_module()
    entry = {
        "file": "tests/differential/stdlib/zipimport_basic.py",
        "status": "ok",
    }
    resolved = module._rss_display_status(entry, {})
    assert resolved == "pass"


def test_run_diff_serial_emits_run_line_before_file_work(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_diff_module()
    case = tmp_path / "case.py"
    case.write_text("print('ok')\n", encoding="utf-8")
    config = module._DiffMemoryGuardConfig(
        max_process_kb=30_000,
        max_tree_kb=40_000,
        global_kb=100_000,
        poll_interval=0.01,
    )
    calls: list[str] = []

    class _FakeSuiteContext:
        def start_repo_sentinel(self, **_kwargs):
            return None

    monkeypatch.setattr(module, "_ensure_diff_run_lock", lambda: None)
    monkeypatch.setattr(module, "_prune_orphan_diff_workers", lambda: None)
    monkeypatch.setattr(module, "_prune_orphan_build_helpers", lambda: None)
    monkeypatch.setattr(module, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(module, "_prune_stale_build_locks", lambda: None)
    monkeypatch.setattr(
        module.test_policy,
        "collect_test_files",
        lambda *_args, **_kwargs: (case,),
    )
    monkeypatch.setattr(module, "_diff_run_id", lambda: "serial-run-line")
    monkeypatch.setattr(module, "_diff_memory_guard_config", lambda: config)
    monkeypatch.setattr(module, "_prepare_memory_guard_run", lambda _config: None)
    monkeypatch.setattr(
        module, "_constrain_jobs_for_memory_guard", lambda jobs, *, config: jobs
    )
    monkeypatch.setattr(
        module.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        staticmethod(lambda *_args, **_kwargs: _FakeSuiteContext()),
    )
    monkeypatch.setattr(module, "_diff_memory_guard_limits", lambda _env=None: object())
    monkeypatch.setattr(module, "_order_test_files", lambda files, _jobs: list(files))

    def _fake_run_single(file_path, *_args, **_kwargs):
        calls.append(file_path)
        return {
            "path": file_path,
            "status": "pass",
            "stdout": "",
            "stderr": "",
            "duration_s": 0.25,
        }

    monkeypatch.setattr(module, "_diff_run_single", _fake_run_single)
    monkeypatch.setattr(module, "_memory_guard_trip_message", lambda: None)
    monkeypatch.setattr(module, "_top_rss_entries", lambda *_args, **_kwargs: [])
    monkeypatch.setattr(module, "_aggregate_rss_metrics", lambda _run_id: {})
    monkeypatch.setattr(module, "_print_rss_top", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(module, "_emit_json", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(
        module, "_memory_guard_scheduler_per_job_gb", lambda _config: 0.1
    )
    monkeypatch.setattr(module, "_config_payload", lambda _config: {})

    module.run_diff(
        [case],
        sys.executable,
        jobs=1,
        failures_output=tmp_path / "failures.txt",
    )

    assert calls == [str(case)]
    assert f"[RUN] {case}" in capsys.readouterr().out


def test_diff_memory_guard_config_clamps_implausible_global_limit(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", "5000")
    monkeypatch.setenv("MOLT_DIFF_MAX_TREE_RSS_GB", "4500")
    monkeypatch.setenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", "4200")

    config = module._diff_memory_guard_config()

    assert config.global_gb == pytest.approx(module._DIFF_MEMORY_GUARD_HARD_GLOBAL_GB)
    assert config.max_tree_gb == pytest.approx(config.global_gb)
    assert config.max_process_gb == pytest.approx(config.max_tree_gb)


def test_diff_memory_guard_disable_env_is_ignored(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "off")

    assert module._diff_memory_guard_enabled() is True


def test_diff_memory_guard_jsonl_rotation_bounds_artifacts(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    path = tmp_path / "global_samples.jsonl"
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_MAX_SAMPLE_MB", "0.001")

    for index in range(8):
        module._append_memory_guard_jsonl(
            path,
            {
                "event": "sample",
                "index": index,
                "payload": "x" * 600,
            },
        )

    assert path.exists()
    assert path.with_name("global_samples.jsonl.1").exists()
    assert path.stat().st_size < 2048
    assert path.with_name("global_samples.jsonl.1").stat().st_size < 2048


def test_diff_memory_guard_streams_without_sample_artifact(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_WRITE_SAMPLES", "0")
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_STREAM", "stderr")
    config = module._DiffMemoryGuardConfig(
        max_process_kb=30_000,
        max_tree_kb=40_000,
        global_kb=100_000,
        poll_interval=0.01,
    )

    module._prepare_memory_guard_run(config)
    capsys.readouterr()
    module._record_memory_guard_sample(
        {
            "event": "sample",
            "active_roots": [200],
            "total_kb": 20_000,
            "total_gb": 20_000 / (1024 * 1024),
            "trees": [
                {
                    "root_pid": 200,
                    "total": {
                        "rss_gb": 20_000 / (1024 * 1024),
                        "rss_kb": 20_000,
                    },
                }
            ],
        }
    )

    captured = capsys.readouterr()
    assert "[MEMORY-GUARD] sample" in captured.err
    assert "roots=1" in captured.err
    assert not (tmp_path / "diff" / "memory_guard" / "global_samples.jsonl").exists()
    assert (tmp_path / "diff" / "memory_guard" / "events.jsonl").exists()


def test_diff_memory_guard_kills_active_child_tree_limit(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    monkeypatch.setenv(
        module._DIFF_MEMORY_GUARD_TRIP_FILE_ENV, str(tmp_path / "tripped.json")
    )
    monkeypatch.setenv(
        module._DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV, str(tmp_path / "events.jsonl")
    )
    monkeypatch.setenv(
        module._DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV,
        str(tmp_path / "samples.jsonl"),
    )
    limits = module.harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=30_000 / (1024 * 1024),
        max_total_rss_gb=20_000 / (1024 * 1024),
        max_global_rss_gb=100_000 / (1024 * 1024),
        poll_interval=0.01,
    )
    groups = [
        module.process_sentinel.ProcessGroup(
            pgid=200,
            matched=True,
            samples=(
                module.memory_guard.ProcessSample(200, os.getpid(), 9_000, "build"),
                module.memory_guard.ProcessSample(201, 200, 12_000, "rustc"),
            ),
        )
    ]
    killed: list[int] = []
    termination_identities: list[dict[int, object]] = []
    module.harness_memory_guard._TERMINATED_PGIDS.clear()
    monkeypatch.setattr(
        module.harness_memory_guard.process_sentinel,
        "process_groups",
        lambda *args, **kwargs: groups,
    )

    def terminate_group(
        pgid: int, *, grace: float, expected_identities: dict[int, object]
    ) -> None:
        assert grace == 0.25
        killed.append(pgid)
        termination_identities.append(dict(expected_identities))

    monkeypatch.setattr(
        module.harness_memory_guard.process_sentinel,
        "terminate_group",
        terminate_group,
    )
    sentinel = module.harness_memory_guard.repo_process_sentinel(
        repo_root=Path(module._repo_root()),
        artifact_root=tmp_path,
        label="unit-diff-tree",
        limits=limits,
        on_scan=module._record_memory_guard_sentinel_sample,
        on_violation=module._record_memory_guard_sentinel_violation,
    )

    sentinel.scan_once()

    assert killed == [200]
    assert termination_identities == [
        {
            sample.pid: module.memory_guard.process_identity(sample)
            for sample in groups[0].samples
        }
    ]
    trip = (tmp_path / "tripped.json").read_text(encoding="utf-8")
    assert "per-tree memory guard tripped" in trip
    assert "scope=process_tree" in trip


def test_stderr_traceback_mode_tolerates_frame_path_differences() -> None:
    module = _load_diff_module()
    cp_err = (
        "Traceback (most recent call last):\n"
        '  File "/cpython/path/test.py", line 10, in <module>\n'
        "RuntimeError: boom\n"
    )
    molt_err = (
        "Traceback (most recent call last):\n"
        '  File "/molt/path/test.py", line 99, in <module>\n'
        '  File "/molt/path/stdlib/_asyncio.py", line 50, in get_event_loop\n'
        "RuntimeError: boom\n"
    )
    assert module._stderr_matches(cp_err, molt_err, "traceback")


def test_stderr_traceback_mode_requires_exact_exception_message() -> None:
    module = _load_diff_module()
    cp_err = "Traceback (most recent call last):\nRuntimeError: boom\n"
    molt_err = "Traceback (most recent call last):\nRuntimeError: boom2\n"
    assert not module._stderr_matches(cp_err, molt_err, "traceback")


def test_stderr_exact_mode_keeps_full_string_match() -> None:
    module = _load_diff_module()
    cp_err = "Traceback (most recent call last):\nRuntimeError: boom\n"
    molt_err = (
        "Traceback (most recent call last):\n"
        '  File "/molt/path/test.py", line 99, in <module>\n'
        "RuntimeError: boom\n"
    )
    assert not module._stderr_matches(cp_err, molt_err, "exact")


def test_diff_batch_compile_server_env_flags(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.delenv("MOLT_DIFF_BATCH_COMPILE_SERVER", raising=False)
    monkeypatch.delenv("MOLT_DIFF_BATCH_COMPILE_SERVER_STRICT", raising=False)
    assert module._diff_batch_compile_server_enabled() is False
    assert module._diff_batch_compile_server_strict() is False

    monkeypatch.setenv("MOLT_DIFF_BATCH_COMPILE_SERVER", "1")
    monkeypatch.setenv("MOLT_DIFF_BATCH_COMPILE_SERVER_STRICT", "true")
    assert module._diff_batch_compile_server_enabled() is True
    assert module._diff_batch_compile_server_strict() is True


def test_diff_batch_compile_server_timeout_env(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.delenv("MOLT_DIFF_BATCH_COMPILE_SERVER_TIMEOUT_SEC", raising=False)
    assert module._diff_batch_compile_server_request_timeout(90.0) == 90.0
    assert module._diff_batch_compile_server_request_timeout(None) == 60.0

    monkeypatch.setenv("MOLT_DIFF_BATCH_COMPILE_SERVER_TIMEOUT_SEC", "15.5")
    assert module._diff_batch_compile_server_request_timeout(90.0) == 15.5

    monkeypatch.setenv("MOLT_DIFF_BATCH_COMPILE_SERVER_TIMEOUT_SEC", "invalid")
    assert module._diff_batch_compile_server_request_timeout(42.0) == 42.0


def test_diff_build_helper_command_matches_internal_batch_server() -> None:
    module = _load_diff_module()
    cmd = f"{sys.executable} -m molt.cli internal-batch-build-server"
    assert module._is_diff_build_helper_command(cmd)


def test_batch_compile_server_readline_timeout_is_hard_deadline() -> None:
    module = _load_diff_module()

    class _DummyProc:
        def __init__(self) -> None:
            self.stderr = None

    client = module._BatchCompileServerClient.__new__(module._BatchCompileServerClient)
    client._proc = _DummyProc()
    client._response_queue = queue.Queue()

    start = module.time.monotonic()
    with pytest.raises(TimeoutError):
        client._readline(0.05)
    elapsed = module.time.monotonic() - start
    assert elapsed < 0.5


def test_shutdown_batch_compile_server_uses_force_close_path(monkeypatch) -> None:
    module = _load_diff_module()
    calls: list[tuple[bool, float | None]] = []

    class _FakeClient:
        def close(self, *, force: bool = False, timeout: float | None = None) -> None:
            calls.append((force, timeout))

    monkeypatch.setattr(module, "_BATCH_COMPILE_SERVER_CLIENT", _FakeClient())
    monkeypatch.setattr(module, "_BATCH_COMPILE_SERVER_CLIENT_PID", 12345)

    module._shutdown_batch_compile_server()

    assert calls == [(True, None)]
    assert module._BATCH_COMPILE_SERVER_CLIENT is None
    assert module._BATCH_COMPILE_SERVER_CLIENT_PID == 0


def test_batch_compile_server_ping_failure_requires_repeated_failures_before_cooldown(
    monkeypatch,
) -> None:
    module = _load_diff_module()
    events: list[tuple[str, bool | float | None | str]] = []

    class _FakeClient:
        def __init__(self, _env) -> None:
            events.append(("init", None))

        def request(self, op: str, *, params=None, timeout: float) -> dict[str, object]:
            events.append(("request", timeout))
            assert op == "ping"
            raise TimeoutError("ping timeout")

        def close(self, *, force: bool = False, timeout: float | None = None) -> None:
            events.append(("close", force))

    monkeypatch.setattr(module, "_BatchCompileServerClient", _FakeClient)
    monkeypatch.setattr(module, "_BATCH_COMPILE_SERVER_CLIENT", None)
    monkeypatch.setattr(module, "_BATCH_COMPILE_SERVER_CLIENT_PID", 0)
    module._batch_compile_server_reset_disabled()

    client, error = module._batch_compile_server_client({}, request_timeout=0.1)
    assert client is None
    assert error is not None
    assert "ping timeout" in str(error)
    assert ("close", True) in events

    client_retry, retry_error = module._batch_compile_server_client(
        {},
        request_timeout=0.1,
    )
    assert client_retry is None
    assert retry_error is not None
    assert "ping timeout" in str(retry_error)

    client_cooldown, cooldown_error = module._batch_compile_server_client(
        {},
        request_timeout=0.1,
    )
    assert client_cooldown is None
    assert cooldown_error is not None
    assert "temporarily disabled" in str(cooldown_error)


def test_run_batch_compile_build_success_resets_failure_budget(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    resets = {"count": 0}
    seen_params: list[dict[str, object]] = []

    class _Client:
        def request(self, op: str, *, params=None, timeout: float) -> dict[str, object]:
            assert op == "build"
            assert isinstance(params, dict)
            seen_params.append(dict(params))
            return {
                "id": 1,
                "ok": True,
                "returncode": 0,
                "stdout": "ok",
                "stderr": "",
            }

    def _fake_client_factory(_env, *, request_timeout: float):
        return _Client(), None

    def _fake_reset_disabled() -> None:
        resets["count"] += 1

    monkeypatch.setattr(module, "_batch_compile_server_client", _fake_client_factory)
    monkeypatch.setattr(
        module, "_batch_compile_server_reset_disabled", _fake_reset_disabled
    )

    result = module._run_batch_compile_build(
        env={"MOLT_CODEC": "msgpack"},
        file_path="tests/differential/basic/arith.py",
        output_root=tmp_path,
        output_binary=tmp_path / "arith_molt",
        build_profile="dev",
        target_python=module.TargetPythonVersion(3, 14, 0),
        no_cache=False,
        rebuild=False,
        request_timeout=8.0,
        strict_mode=False,
    )

    assert result == module.compat_backends.BackendResult("ok", "", 0)
    assert resets["count"] == 1
    assert "stdlib_profile" not in seen_params[0]
    assert seen_params[0]["python_version"] == "3.14"

    result = module._run_batch_compile_build(
        env={"MOLT_CODEC": "msgpack", "MOLT_DIFF_STDLIB_PROFILE": "full"},
        file_path="tests/differential/basic/arith.py",
        output_root=tmp_path,
        output_binary=tmp_path / "arith_molt",
        build_profile="dev",
        target_python=module.TargetPythonVersion(3, 14, 0),
        no_cache=False,
        rebuild=False,
        request_timeout=8.0,
        strict_mode=False,
    )

    assert result == module.compat_backends.BackendResult("ok", "", 0)
    assert resets["count"] == 2
    assert seen_params[1]["stdlib_profile"] == "full"
    assert seen_params[1]["python_version"] == "3.14"


def test_run_batch_compile_build_strict_mode_retries_once_on_start_error(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    attempts = {"count": 0}
    resets = {"count": 0}

    class _FakeClient:
        def request(self, op: str, *, params=None, timeout: float) -> dict[str, object]:
            assert op == "build"
            assert isinstance(params, dict)
            return {
                "id": 1,
                "ok": True,
                "returncode": 0,
                "stdout": "ok",
                "stderr": "",
            }

    def _fake_client_factory(_env, *, request_timeout: float):
        assert request_timeout == 12.0
        attempts["count"] += 1
        if attempts["count"] == 1:
            return None, RuntimeError("transient startup failure")
        return _FakeClient(), None

    def _fake_reset_disabled() -> None:
        resets["count"] += 1

    monkeypatch.setattr(module, "_batch_compile_server_client", _fake_client_factory)
    monkeypatch.setattr(
        module, "_batch_compile_server_reset_disabled", _fake_reset_disabled
    )

    result = module._run_batch_compile_build(
        env={"MOLT_CODEC": "msgpack"},
        file_path="tests/differential/basic/arith.py",
        output_root=tmp_path,
        output_binary=tmp_path / "arith_molt",
        build_profile="dev",
        target_python=module.TargetPythonVersion(3, 13, 0),
        no_cache=False,
        rebuild=False,
        request_timeout=12.0,
        strict_mode=True,
    )

    assert result == module.compat_backends.BackendResult("ok", "", 0)
    assert attempts["count"] == 2
    assert resets["count"] == 2


@pytest.mark.parametrize("strict_mode", (False, True))
def test_run_batch_compile_build_timeout_is_terminal_and_force_closes_server(
    monkeypatch, tmp_path: Path, strict_mode: bool
) -> None:
    module = _load_diff_module()
    shutdown_calls: list[bool] = []
    disabled_reasons: list[str] = []
    requests = 0

    class _FailingClient:
        def request(self, op: str, *, params=None, timeout: float) -> dict[str, object]:
            nonlocal requests
            requests += 1
            assert op == "build"
            raise TimeoutError("build timed out")

    def _fake_client_factory(_env, *, request_timeout: float):
        assert request_timeout == 8.0
        return _FailingClient(), None

    def _fake_shutdown(*, force: bool = True) -> None:
        shutdown_calls.append(force)

    def _fake_mark_disabled(reason: str) -> None:
        disabled_reasons.append(reason)

    monkeypatch.setattr(module, "_batch_compile_server_client", _fake_client_factory)
    monkeypatch.setattr(module, "_shutdown_batch_compile_server", _fake_shutdown)
    monkeypatch.setattr(
        module, "_batch_compile_server_mark_disabled", _fake_mark_disabled
    )

    result = module._run_batch_compile_build(
        env={"MOLT_CODEC": "msgpack"},
        file_path="tests/differential/basic/arith.py",
        output_root=tmp_path,
        output_binary=tmp_path / "arith_molt",
        build_profile="dev",
        target_python=module.TargetPythonVersion(3, 13, 0),
        no_cache=False,
        rebuild=False,
        request_timeout=8.0,
        strict_mode=strict_mode,
    )

    assert result.timed_out and result.build_failed
    assert result.returncode == 124
    assert result.stdout is None
    assert "build timed out" in result.stderr
    assert "timeout after 8.0s" in result.stderr
    assert requests == 1
    assert shutdown_calls == [True]
    assert disabled_reasons == ["build timed out"]


def test_run_molt_does_not_fallback_after_batch_deadline(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    deadline = module.compat_backends.BackendResult.from_deadline(
        timeout=9.0,
        stdout="partial compiler stdout",
        stderr="batch request deadline",
        build_failed=True,
    )
    subprocess_calls = []
    metric_statuses = []

    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_root", lambda: tmp_path / "diff-root")
    monkeypatch.setattr(
        module, "_diff_cargo_target_root", lambda: tmp_path / "target-root"
    )
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: False)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_diff_batch_compile_server_enabled", lambda: True)
    monkeypatch.setattr(module, "_diff_batch_compile_server_strict", lambda: False)
    monkeypatch.setattr(
        module, "_diff_batch_compile_server_request_timeout", lambda timeout: 9.0
    )
    monkeypatch.setattr(module, "_run_batch_compile_build", lambda **kwargs: deadline)
    monkeypatch.setattr(
        module,
        "_run_with_optional_time",
        lambda *args, **kwargs: subprocess_calls.append((args, kwargs)),
    )
    monkeypatch.setattr(
        module,
        "_record_rss_metrics",
        lambda *args, **kwargs: metric_statuses.append(kwargs["status"]),
    )

    context = module.compat_backends.BackendExecutionContext(
        target_python=module.TargetPythonVersion(3, 14, 0),
        build_profile="dev",
        capabilities="",
        environment={"MOLT_CAPABILITY_TIER": "none"},
    )
    result = module.run_molt_build_only(
        "tests/differential/basic/arith.py",
        "dev",
        execution_context=context,
    )

    assert result == deadline
    assert subprocess_calls == []
    assert metric_statuses == ["build_timeout"]


def test_run_molt_build_only_uses_build_profile_flag(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    seen_cmds: list[list[str]] = []
    seen_envs: list[dict[str, str]] = []
    diff_root = tmp_path / "diff-root"
    target_root = tmp_path / "target-root"

    def fake_run_with_optional_time(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        seen_cmds.append(list(cmd))
        env = kwargs.get("env")
        assert isinstance(env, dict)
        seen_envs.append(dict(env))
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text("")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(module, "_run_with_optional_time", fake_run_with_optional_time)
    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_root", lambda: diff_root)
    monkeypatch.setattr(module, "_diff_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: False)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_rss_exceeded", lambda metrics, limit: (False, ""))
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda output: None)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda file_path: {})
    monkeypatch.setattr(module, "_resolve_molt_cli_python", lambda: sys.executable)

    context = module.compat_backends.BackendExecutionContext(
        target_python=module.TargetPythonVersion(3, 14, 0),
        build_profile="dev",
        capabilities="fs,env,time,random",
        environment={"MOLT_CAPABILITY_TIER": "none"},
    )
    result = module.run_molt_build_only(
        "tests/differential/stdlib/unicodedata_basic.py",
        "dev",
        execution_context=context,
    )
    stdout, stderr, rc = result.stdout, result.stderr, result.returncode

    assert (stdout, stderr, rc) == ("", "", 0)
    assert seen_cmds == [
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "tests/differential/stdlib/unicodedata_basic.py",
            "--build-profile",
            "dev",
            "--respect-pythonpath",
            "--out-dir",
            seen_cmds[0][seen_cmds[0].index("--out-dir") + 1],
            "--output",
            seen_cmds[0][seen_cmds[0].index("--output") + 1],
            "--python-version",
            "3.14",
            "--capabilities",
            "fs,env,time,random",
        ]
    ]
    diagnostics_path = Path(seen_envs[0]["MOLT_DIAGNOSTICS_FILE"])
    assert diagnostics_path.name == "runtime_diagnostics.log"
    assert diagnostics_path.parent.parent == tmp_path


def test_run_molt_preserves_explicit_runtime_diagnostics_file(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    seen_envs: list[dict[str, str]] = []
    explicit_diagnostics = tmp_path / "explicit-runtime-diagnostics.log"

    def fake_run_with_optional_time(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = kwargs.get("env")
        assert isinstance(env, dict)
        seen_envs.append(dict(env))
        if "build" in cmd:
            output_path = Path(cmd[cmd.index("--output") + 1])
            output_path.parent.mkdir(parents=True, exist_ok=True)
            output_path.write_text("")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(module, "_run_with_optional_time", fake_run_with_optional_time)
    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_root", lambda: tmp_path / "diff-root")
    monkeypatch.setattr(
        module, "_diff_cargo_target_root", lambda: tmp_path / "target-root"
    )
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: False)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_rss_exceeded", lambda metrics, limit: (False, ""))
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda output: None)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda file_path: {})
    monkeypatch.setattr(module, "_resolve_molt_cli_python", lambda: sys.executable)

    result = module.run_molt(
        "tests/differential/stdlib/unicodedata_basic.py",
        "dev",
        extra_env={"MOLT_DIAGNOSTICS_FILE": str(explicit_diagnostics)},
    )
    stdout, stderr, rc = result.stdout, result.stderr, result.returncode

    assert (stdout, stderr, rc) == ("", "", 0)
    assert seen_envs
    assert {env["MOLT_DIAGNOSTICS_FILE"] for env in seen_envs} == {
        str(explicit_diagnostics)
    }


def test_run_molt_build_only_uses_diff_stdlib_profile_flag(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    seen_cmds: list[list[str]] = []
    diff_root = tmp_path / "diff-root"
    target_root = tmp_path / "target-root"

    def fake_run_with_optional_time(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text("")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setenv("MOLT_DIFF_STDLIB_PROFILE", "full")
    monkeypatch.setattr(module, "_run_with_optional_time", fake_run_with_optional_time)
    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_root", lambda: diff_root)
    monkeypatch.setattr(module, "_diff_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: False)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_rss_exceeded", lambda metrics, limit: (False, ""))
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda output: None)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda file_path: {})
    monkeypatch.setattr(module, "_resolve_molt_cli_python", lambda: sys.executable)

    result = module.run_molt_build_only(
        "tests/differential/stdlib/unicodedata_basic.py",
        "dev",
    )
    stdout, stderr, rc = result.stdout, result.stderr, result.returncode

    assert (stdout, stderr, rc) == ("", "", 0)
    cmd = seen_cmds[0]
    assert cmd[cmd.index("--stdlib-profile") + 1] == "full"


def test_run_molt_build_only_uses_metadata_stdlib_profile_flag(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    seen_cmds: list[list[str]] = []
    diff_root = tmp_path / "diff-root"
    target_root = tmp_path / "target-root"

    def fake_run_with_optional_time(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_cmds.append(list(cmd))
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text("")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.delenv("MOLT_DIFF_STDLIB_PROFILE", raising=False)
    monkeypatch.setattr(module, "_run_with_optional_time", fake_run_with_optional_time)
    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_root", lambda: diff_root)
    monkeypatch.setattr(module, "_diff_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: False)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_rss_exceeded", lambda metrics, limit: (False, ""))
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda output: None)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda file_path: {})
    monkeypatch.setattr(
        module.test_policy,
        "parse_metadata",
        lambda file_path: test_policy.TestMetadata(stdlib_profile="full"),
    )
    monkeypatch.setattr(module, "_resolve_molt_cli_python", lambda: sys.executable)

    result = module.run_molt_build_only(
        "tests/differential/stdlib/stringprep_semantics.py",
        "dev",
    )
    stdout, stderr, rc = result.stdout, result.stderr, result.returncode

    assert (stdout, stderr, rc) == ("", "", 0)
    cmd = seen_cmds[0]
    assert cmd[cmd.index("--stdlib-profile") + 1] == "full"


def test_run_molt_build_only_rejects_conflicting_metadata_stdlib_profile(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    seen_cmds: list[list[str]] = []
    metric_statuses: list[str] = []
    diff_root = tmp_path / "diff-root"
    target_root = tmp_path / "target-root"

    monkeypatch.setenv("MOLT_DIFF_STDLIB_PROFILE", "micro")
    monkeypatch.setattr(
        module,
        "_run_with_optional_time",
        lambda cmd, **kwargs: seen_cmds.append(list(cmd)),
    )
    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_root", lambda: diff_root)
    monkeypatch.setattr(module, "_diff_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda file_path: {})
    monkeypatch.setattr(
        module.test_policy,
        "parse_metadata",
        lambda file_path: test_policy.TestMetadata(stdlib_profile="full"),
    )
    monkeypatch.setattr(
        module,
        "_record_rss_metrics",
        lambda *args, **kwargs: metric_statuses.append(kwargs["status"]),
    )

    result = module.run_molt_build_only(
        "tests/differential/stdlib/stringprep_semantics.py",
        "dev",
    )
    stdout, stderr, rc = result.stdout, result.stderr, result.returncode

    assert stdout is None
    assert rc == 2
    assert "requires MOLT_DIFF_STDLIB_PROFILE=full but selected micro" in stderr
    assert seen_cmds == []
    assert metric_statuses == ["build_invalid_stdlib_profile"]


def test_run_molt_build_only_uses_persistent_diff_cache_by_default(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    seen_envs: list[dict[str, str]] = []
    diff_root = tmp_path / "diff-root"
    diff_cache = tmp_path / ".molt_cache"
    target_root = tmp_path / "target-root"

    def fake_run_with_optional_time(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = kwargs.get("env")
        assert isinstance(env, dict)
        seen_envs.append(dict(env))
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text("")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.delenv("MOLT_CACHE", raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(diff_root))
    monkeypatch.setenv("MOLT_DIFF_CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(module, "_run_with_optional_time", fake_run_with_optional_time)
    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: False)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_rss_exceeded", lambda metrics, limit: (False, ""))
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda output: None)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda file_path: {})
    monkeypatch.setattr(module, "_resolve_molt_cli_python", lambda: sys.executable)

    result = module.run_molt_build_only(
        "tests/differential/stdlib/unicodedata_basic.py",
        "dev",
    )
    stdout, stderr, rc = result.stdout, result.stderr, result.returncode

    assert (stdout, stderr, rc) == ("", "", 0)
    assert seen_envs[0]["MOLT_CACHE"] == str(diff_cache)
    assert diff_cache.is_dir()


def test_run_molt_build_only_preserves_explicit_molt_cache(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    seen_envs: list[dict[str, str]] = []
    explicit_cache = tmp_path / "explicit-cache"
    diff_root = tmp_path / "diff-root"
    target_root = tmp_path / "target-root"

    def fake_run_with_optional_time(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        env = kwargs.get("env")
        assert isinstance(env, dict)
        seen_envs.append(dict(env))
        output_path = Path(cmd[cmd.index("--output") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text("")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def fail_diff_cache_root() -> Path:
        raise AssertionError("explicit MOLT_CACHE should not call _diff_cache_root")

    monkeypatch.setenv("MOLT_CACHE", str(explicit_cache))
    monkeypatch.setattr(module, "_run_with_optional_time", fake_run_with_optional_time)
    monkeypatch.setattr(module, "_diff_tmp_root", lambda _environment=None: tmp_path)
    monkeypatch.setattr(module, "_diff_root", lambda: diff_root)
    monkeypatch.setattr(module, "_diff_cache_root", fail_diff_cache_root)
    monkeypatch.setattr(module, "_diff_cargo_target_root", lambda: target_root)
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: False)
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_rss_exceeded", lambda metrics, limit: (False, ""))
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda output: None)
    monkeypatch.setattr(module, "_collect_env_overrides", lambda file_path: {})
    monkeypatch.setattr(module, "_resolve_molt_cli_python", lambda: sys.executable)

    result = module.run_molt_build_only(
        "tests/differential/stdlib/unicodedata_basic.py",
        "dev",
    )
    stdout, stderr, rc = result.stdout, result.stderr, result.returncode

    assert (stdout, stderr, rc) == ("", "", 0)
    assert seen_envs[0]["MOLT_CACHE"] == str(explicit_cache)
    assert explicit_cache.is_dir()


def test_diff_root_defaults_to_repo_tmp_diff_when_ext_root_unset(
    tmp_path: Path,
) -> None:
    module = _load_diff_module()
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    environment: dict[str, str] = {}

    layout = module.DiffArtifactLayout.from_environment(
        repo_root=repo_root,
        environment=environment,
    )

    assert layout.diff_root == repo_root / "tmp" / "diff"
    assert environment == {}


def test_diff_root_defaults_to_ext_tmp_diff_when_ext_root_set(
    tmp_path: Path,
) -> None:
    module = _load_diff_module()
    ext_root = tmp_path / "ext-root"
    environment = {"MOLT_EXT_ROOT": str(ext_root)}

    layout = module.DiffArtifactLayout.from_environment(
        repo_root=tmp_path / "repo",
        environment=environment,
    )

    assert layout.diff_root == ext_root / "tmp" / "diff"
    assert environment == {"MOLT_EXT_ROOT": str(ext_root)}


def test_diff_tmp_root_defaults_to_ext_tmp_when_unset(
    tmp_path: Path,
) -> None:
    module = _load_diff_module()
    ext_root = tmp_path / "ext-root"
    environment = {"MOLT_EXT_ROOT": str(ext_root)}

    layout = module.DiffArtifactLayout.from_environment(
        repo_root=tmp_path / "repo",
        environment=environment,
    )

    assert layout.tmp_root == ext_root / "tmp"


@pytest.fixture
def admitted_guest_environment(tmp_path, monkeypatch):
    from tools.compat import diff_output_layout as output

    repo = tmp_path / "repo"
    canonical = tmp_path / "canonical"
    guest = tmp_path / "guest"
    for root in (repo, canonical, guest):
        root.mkdir()
    env = {
        "MOLT_EXT_ROOT": str(canonical),
        "MOLT_DIFF_ROOT": str(canonical / "receipts"),
        output.ROOT_ENV: str(guest),
    }
    monkeypatch.setattr(
        output.disk_capacity, "require_build_capacity", lambda *_a, **_k: None
    )
    output.admit(env, repo_root=repo, custody_root=canonical / "receipts")
    return repo, env


@pytest.mark.parametrize("mode", ["dyld", "isolated-retry"])
def test_guest_readmission_preserves_selected_mode(admitted_guest_environment, mode):
    from tools.compat import diff_output_layout as output

    repo, env = admitted_guest_environment
    root = Path(env[output.ROOT_ENV])
    parent = root / "guest-tmp"
    target = (
        parent / "dyld_quarantine" / "run" / "target"
        if mode == "dyld"
        else parent / "molt_diff_retry_run" / "target"
    )
    env.update(
        CARGO_TARGET_DIR=str(target),
        MOLT_DIFF_CARGO_TARGET_DIR=str(target),
        MOLT_DIFF_TARGET_MODE=mode,
    )
    original = dict(env)
    output.admit(env, repo_root=repo, custody_root=Path(env["MOLT_DIFF_ROOT"]))
    assert env == original
    env["MOLT_DIFF_TMPDIR"] = str(repo / "wrong")
    with pytest.raises(ValueError, match="escaped"):
        output.admit(env, repo_root=repo, custody_root=Path(env["MOLT_DIFF_ROOT"]))


@pytest.mark.parametrize("raw", ['{"schema":1,"schema":2}', '{"device":NaN}', "null"])
def test_guest_identity_requires_exact_json(admitted_guest_environment, raw):
    from tools.compat import diff_output_layout as output

    repo, env = admitted_guest_environment
    env[output.IDENTITY_ENV] = raw
    with pytest.raises(ValueError, match="malformed"):
        output.enforce_child(env, repo_root=repo)


def test_guest_root_replaced_during_capacity_is_not_admitted(
    admitted_guest_environment, monkeypatch
):
    from tools.compat import diff_output_layout as output

    repo, env = admitted_guest_environment
    env.pop(output.IDENTITY_ENV)
    original = dict(env)
    root = Path(env[output.ROOT_ENV])

    def replace_root(*_args, **_kwargs):
        root.rename(root.with_name("old-guest"))
        root.mkdir()

    monkeypatch.setattr(output.disk_capacity, "require_build_capacity", replace_root)
    with pytest.raises(ValueError, match="replaced|remounted"):
        output.admit(env, repo_root=repo, custody_root=Path(env["MOLT_DIFF_ROOT"]))
    assert env == original


def test_guest_configure_rejects_inherited_conflicting_outputs(
    admitted_guest_environment, monkeypatch
):
    from tools.compat import diff_output_layout as output

    repo, env = admitted_guest_environment
    env.pop(output.IDENTITY_ENV)
    env["CARGO_TARGET_DIR"] = str(repo / "wrong")
    module = _load_diff_module()
    monkeypatch.setattr(
        module,
        "development_artifact_env",
        lambda _root, environment, **_kwargs: dict(environment),
    )
    with pytest.raises(ValueError, match="conflicts"):
        module._configure_diff_artifact_environment(repo_root=repo, environment=env)


def test_guest_replaced_leaf_is_never_retired(admitted_guest_environment):
    from tools.compat import diff_output_layout as output

    repo, env = admitted_guest_environment
    root = Path(env["MOLT_DIFF_TMPDIR"])
    lease = output.new_guest_leaf(
        root, prefix="test_", boundary=root, environment=env, repo_root=repo
    )
    lease.path.rename(root / "original")
    lease.path.mkdir()
    (lease.path / "unowned").write_text("preserve")
    error = lease.retire(environment=env, repo_root=repo)
    assert error and "identity changed" in error
    assert (lease.path / "unowned").read_text() == "preserve"
    assert (root / "original").is_dir()


@pytest.mark.parametrize("backend", ["cpython", "native", "wasm", "llvm", "luau"])
@pytest.mark.parametrize("outcome", ["success", "failure", "exception"])
def test_guest_backends_preserve_primary_and_cleanup_failure(
    admitted_guest_environment, monkeypatch, backend, outcome
):
    from molt.target_python import TargetPythonVersion
    from tools.compat import backends, diff_output_layout as output

    repo, env = admitted_guest_environment
    module = _load_diff_module()
    monkeypatch.setattr(module, "_repo_root", lambda: repo)
    monkeypatch.setattr(backends, "_REPO_ROOT", repo)
    # Ambient selection is deliberately incompatible with the supplied context.
    monkeypatch.setenv(output.ROOT_ENV, str(repo / "ambient-missing"))
    context = backends.BackendExecutionContext(
        target_python=TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment=env,
    )
    seen = []
    primary = LookupError("primary guest failure")

    def owned(*_args, **kwargs):
        path = kwargs.get(
            "output_root", kwargs.get("cpython_tmp", kwargs.get("out_dir"))
        )
        seen.append(path)
        assert path.is_relative_to(Path(env[output.ROOT_ENV]))
        if outcome == "exception":
            raise primary
        return backends.BackendResult(
            "answer",
            "guest error" if outcome == "failure" else "",
            7 if outcome == "failure" else 0,
        )

    def blocked(*_args, **_kwargs):
        raise OSError("retirement blocked")

    monkeypatch.setattr(output, "durable_remove_path", blocked)
    # Receipt resolution failure must not replace the guest exception either.
    resolve = output.resolve_owned_path

    def resolve_receipt(path):
        if path.name == "guest_output_cleanup_failures.jsonl":
            raise ValueError("receipt custody changed")
        return resolve(path)

    monkeypatch.setattr(output, "resolve_owned_path", resolve_receipt)
    if backend == "cpython":
        for key in (
            *output.OUTPUT_KEYS,
            output.ROOT_ENV,
            output.IDENTITY_ENV,
            "MOLT_EXT_ROOT",
            "MOLT_DIFF_ROOT",
        ):
            monkeypatch.setenv(key, env[key])
        monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
        monkeypatch.delenv("MOLT_DIFF_KEEP", raising=False)
        monkeypatch.setattr(module, "_run_cpython_owned", owned)

        def run():
            return module.run_cpython("case.py")
    elif backend == "native":
        monkeypatch.setattr(module, "_run_molt_owned", owned)

        def run():
            return module.run_molt(
                "case.py", build_profile="dev", execution_context=context
            )
    else:
        adapter = {
            "wasm": backends.WasmAdapter,
            "llvm": backends.LlvmAdapter,
            "luau": backends.LuauAdapter,
        }[backend]()
        monkeypatch.setattr(adapter, "_build_and_run_owned", owned)

        def run():
            return adapter.build_and_run("case.py", context=context)

    if outcome == "exception":
        with pytest.raises(LookupError) as caught:
            run()
        assert caught.value is primary
        diagnostic = "\n".join(primary.__notes__)
    else:
        result = run()
        assert result.returncode == (7 if outcome == "failure" else 0)
        assert result.stderr == ("guest error" if outcome == "failure" else "")
        assert result.infrastructure_failure is not None
        diagnostic = "\n".join(result.infrastructure_failure.details)
    assert (
        "retirement blocked" in diagnostic and "receipt custody changed" in diagnostic
    )
    assert seen[0].exists()


def test_selected_dyld_and_retry_ignore_inherited_control_override(
    admitted_guest_environment, monkeypatch
):
    from tools.compat import diff_output_layout as output

    repo, env = admitted_guest_environment
    env["MOLT_BUILD_STATE_DIR"] = str(Path(env["MOLT_EXT_ROOT"]) / "shared-state")
    module = _load_diff_module()
    monkeypatch.setattr(module, "_repo_root", lambda: repo)
    for key, value in env.items():
        monkeypatch.setenv(key, value)
    for key in ("MOLT_DIFF_TARGET_MODE", "MOLT_DIFF_KEEP_ISOLATED_RETRY"):
        monkeypatch.delenv(key, raising=False)
    for key in ("MOLT_BUILD_STATE_DIR", "MOLT_BACKEND_DAEMON", "MOLT_DIFF_TARGET_MODE"):
        # Quarantine intentionally mutates these globals; register restoration.
        monkeypatch.setenv(key, os.environ.get(key, ""))
    target, state, activated = module._activate_dyld_quarantine_target(use_local=True)
    assert activated and target.is_relative_to(Path(env[output.ROOT_ENV]))
    assert state == output.isolated_state_root(
        target=target, environment=env, repo_root=repo
    )
    assert state != Path(env["MOLT_BUILD_STATE_DIR"])
    assert output.enforce_child(dict(os.environ), repo_root=repo) == Path(
        env[output.ROOT_ENV]
    )
    retry_states: list[Path] = []

    def run_retry(isolated):
        retry_state = Path(isolated["MOLT_BUILD_STATE_DIR"])
        assert retry_state != state and retry_state != Path(env["MOLT_BUILD_STATE_DIR"])
        assert retry_state.exists()
        retry_states.append(retry_state)
        return module.compat_backends.BackendResult("retry stdout", "", 0)

    result = module._run_isolated_retry(run_retry, environment=env)
    assert result.stdout == "retry stdout" and result.infrastructure_failure is None
    assert len(retry_states) == 1 and not retry_states[0].exists()


@pytest.mark.parametrize(
    "failure", ["body", "state-allocation", "cleanup-success", "cleanup-failure"]
)
def test_retry_lease_always_retires_and_reports_failures(
    admitted_guest_environment, monkeypatch, failure
):
    from tools.compat import diff_output_layout as output

    repo, env = admitted_guest_environment
    module = _load_diff_module()
    monkeypatch.setattr(module, "_repo_root", lambda: repo)
    primary = LookupError("primary")
    if failure == "state-allocation":

        def cannot_claim(*_args, **_kwargs):
            raise primary

        monkeypatch.setattr(output, "claim_new_guest_leaf", cannot_claim)
    retired = []

    def blocked_retirement(lease, **_kwargs):
        retired.append(lease.path)
        return "cleanup failed"

    if failure != "state-allocation":
        monkeypatch.setattr(output.GuestOutputLease, "retire", blocked_retirement)
    guest = module.compat_backends.BackendResult(
        "guest stdout",
        "guest stderr",
        7 if failure == "cleanup-failure" else 0,
        detail="guest detail",
    )

    def run_retry(isolated):
        child = {**env, **isolated}
        assert output.enforce_child(child, repo_root=repo) == Path(env[output.ROOT_ENV])
        assert Path(isolated["MOLT_BUILD_STATE_DIR"]).is_relative_to(
            Path(env["MOLT_EXT_ROOT"])
        )
        if failure == "body":
            raise primary
        return guest

    if failure in {"body", "state-allocation"}:
        with pytest.raises(LookupError) as caught:
            module._run_isolated_retry(run_retry, environment=env)
        assert caught.value is primary
        if failure == "state-allocation":
            assert not list(Path(env["MOLT_DIFF_TMPDIR"]).iterdir())
        else:
            assert primary.__notes__ == ["cleanup failed", "cleanup failed"]
    else:
        result = module._run_isolated_retry(run_retry, environment=env)
        assert (result.stdout, result.stderr, result.returncode) == (
            guest.stdout,
            guest.stderr,
            guest.returncode,
        )
        assert result.infrastructure_failure is not None
        assert result.infrastructure_failure.phase == "temporary_artifact_custody"
        assert result.infrastructure_failure.details == (
            "cleanup failed",
            "cleanup failed",
        )
        assert result.detail == "guest detail\ncleanup failed\ncleanup failed"
    if failure != "state-allocation":
        assert len(retired) == 2
        assert retired[0].is_relative_to(Path(env["MOLT_EXT_ROOT"]))
        assert retired[1].is_relative_to(Path(env[output.ROOT_ENV]))


def test_diff_lock_rejects_target_drift_without_dropping_held_lock(
    tmp_path, monkeypatch
):
    module = _load_diff_module()
    first, second = tmp_path / "one.lock", tmp_path / "two.lock"
    monkeypatch.setattr(module, "_diff_run_lock_path", lambda: first)
    module._ensure_diff_run_lock()
    held = module._DIFF_RUN_LOCK_HANDLE
    try:
        module._ensure_diff_run_lock()
        monkeypatch.setattr(module, "_diff_run_lock_path", lambda: second)
        with pytest.raises(RuntimeError, match="target changed"):
            module._ensure_diff_run_lock()
        assert module._DIFF_RUN_LOCK_HANDLE is held
        assert not second.exists()
    finally:
        module._release_diff_run_lock()


def test_guest_output_selection_binds_one_root_and_keeps_receipts_canonical(
    tmp_path: Path, monkeypatch
) -> None:
    from tools.compat import diff_output_layout

    module = _load_diff_module()
    repo = tmp_path / "repo"
    repo.mkdir()
    canonical = tmp_path / "canonical"
    canonical.mkdir()
    output = tmp_path / "output"
    output.mkdir()
    env = {
        "MOLT_EXT_ROOT": str(canonical),
        "MOLT_DIFF_ROOT": str(canonical / "receipt-one"),
        diff_output_layout.ROOT_ENV: str(output),
    }
    measured = []
    monkeypatch.setattr(
        diff_output_layout.disk_capacity,
        "require_build_capacity",
        lambda paths, **_kw: measured.extend(paths),
    )
    diff_output_layout.admit(
        env, repo_root=repo, custody_root=canonical / "receipt-one"
    )
    first = module.DiffArtifactLayout.from_environment(repo_root=repo, environment=env)
    second = module.DiffArtifactLayout.from_environment(
        repo_root=repo,
        environment={**env, "MOLT_DIFF_ROOT": str(canonical / "receipt-two")},
    )
    assert first.diff_root == canonical / "receipt-one"
    assert first.tmp_root == output / "guest-tmp"
    assert (
        first.cargo_target_root == second.cargo_target_root == output / "cargo-target"
    )
    assert first.cache_root == canonical / ".molt_cache"
    assert set(measured) == {
        output / "cargo-target",
        output / "guest-tmp",
        output / "compat-scratch",
    }
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]


def test_guest_output_replacement_refused_after_admission(tmp_path: Path, monkeypatch):
    from tools.compat import diff_output_layout

    repo = tmp_path / "repo"
    repo.mkdir()
    canonical = tmp_path / "canonical"
    canonical.mkdir()
    output = tmp_path / "output"
    output.mkdir()
    env = {
        "MOLT_EXT_ROOT": str(canonical),
        "MOLT_DIFF_ROOT": str(canonical / "diff"),
        diff_output_layout.ROOT_ENV: str(output),
    }
    monkeypatch.setattr(
        diff_output_layout.disk_capacity,
        "require_build_capacity",
        lambda *_args, **_kwargs: None,
    )
    diff_output_layout.admit(env, repo_root=repo, custody_root=canonical / "diff")
    output.rename(tmp_path / "replaced")
    output.mkdir()
    with pytest.raises(ValueError, match="replaced|remounted"):
        diff_output_layout.enforce_child(env, repo_root=repo)


def test_guest_output_capacity_failure_does_not_bind_identity(
    tmp_path: Path, monkeypatch
):
    from tools.compat import diff_output_layout

    repo = tmp_path / "repo"
    repo.mkdir()
    canonical = tmp_path / "canonical"
    canonical.mkdir()
    output = tmp_path / "output"
    output.mkdir()
    env = {
        "MOLT_EXT_ROOT": str(canonical),
        "MOLT_DIFF_ROOT": str(canonical / "diff"),
        diff_output_layout.ROOT_ENV: str(output),
    }
    monkeypatch.setattr(
        diff_output_layout.disk_capacity,
        "require_build_capacity",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(ValueError("insufficient")),
    )
    with pytest.raises(ValueError, match="insufficient"):
        diff_output_layout.admit(env, repo_root=repo, custody_root=canonical / "diff")
    assert diff_output_layout.IDENTITY_ENV not in env
    assert "CARGO_TARGET_DIR" not in env


def test_guest_output_requires_explicit_canonical_artifact_root(tmp_path: Path):
    from tools.compat import diff_output_layout

    repo = tmp_path / "repo"
    repo.mkdir()
    output = tmp_path / "output"
    output.mkdir()
    env = {diff_output_layout.ROOT_ENV: str(output)}
    with pytest.raises(ValueError, match="requires canonical MOLT_EXT_ROOT"):
        diff_output_layout.admit(
            env, repo_root=repo, custody_root=repo / "tmp" / "diff"
        )
    assert diff_output_layout.IDENTITY_ENV not in env


def test_diff_run_lock_is_same_control_as_plain_cli_across_receipts(
    tmp_path: Path, monkeypatch
):
    from molt.cli.runtime_paths import _build_state_root_cached
    from tools.compat import diff_output_layout

    module = _load_diff_module()
    repo = tmp_path / "repo"
    repo.mkdir()
    canonical = tmp_path / "canonical"
    canonical.mkdir()
    output = tmp_path / "output"
    output.mkdir()
    monkeypatch.setattr(
        diff_output_layout.disk_capacity,
        "require_build_capacity",
        lambda *_args, **_kwargs: None,
    )
    monkeypatch.setattr(module, "_repo_root", lambda: repo)
    env = {
        "MOLT_EXT_ROOT": str(canonical),
        diff_output_layout.ROOT_ENV: str(output),
        "MOLT_DIFF_ROOT": str(canonical / "receipt-one"),
    }
    diff_output_layout.admit(
        env, repo_root=repo, custody_root=canonical / "receipt-one"
    )
    monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
    for key, value in env.items():
        monkeypatch.setenv(key, value)
    first = module._diff_run_lock_path()
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(canonical / "receipt-two"))
    assert module._diff_run_lock_path() == first
    cli = _build_state_root_cached(
        str(repo),
        None,
        str(output / "cargo-target"),
        str(repo),
        None,
        str(canonical),
    )
    assert first == cli / "diff_run.lock"


@pytest.mark.parametrize(
    ("envs", "expected"),
    [
        ({"MOLT_DIFF_CARGO_TARGET_DIR": "override-target"}, Path("override-target")),
        ({"CARGO_TARGET_DIR": "cargo-target"}, Path("cargo-target")),
        ({"MOLT_EXT_ROOT": "ext-root"}, Path("ext-root") / "target"),
        ({}, Path("repo-root") / "target"),
    ],
)
def test_diff_cargo_target_root_respects_priority_order(
    tmp_path: Path, envs: dict[str, str], expected: Path
) -> None:
    module = _load_diff_module()
    repo_root = tmp_path / "repo-root"
    environment = {key: str(tmp_path / value) for key, value in envs.items()}
    original_environment = dict(environment)

    layout = module.DiffArtifactLayout.from_environment(
        repo_root=repo_root,
        environment=environment,
    )

    assert layout.cargo_target_root == tmp_path / expected
    assert environment == original_environment


@pytest.mark.parametrize("infrastructure_failure", [False, True])
def test_run_diff_warm_cache_defaults_molt_cache_from_ext_root(
    monkeypatch, tmp_path: Path, infrastructure_failure: bool
) -> None:
    module = _load_diff_module()
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    ext_root = tmp_path / "ext-root"
    target_file = tmp_path / "target.py"
    target_file.write_text("print('ok')\n", encoding="utf-8")
    seen_cache_roots: list[str | None] = []

    monkeypatch.setattr(module, "_repo_root", lambda: repo_root)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    monkeypatch.delenv("MOLT_CACHE", raising=False)
    monkeypatch.delenv("MOLT_DIFF_ROOT", raising=False)
    monkeypatch.delenv("MOLT_DIFF_TMPDIR", raising=False)
    monkeypatch.delenv("MOLT_DIFF_CARGO_TARGET_DIR", raising=False)
    monkeypatch.setattr(module, "_ensure_diff_run_lock", lambda: None)
    monkeypatch.setattr(module, "_prune_orphan_diff_workers", lambda: None)
    monkeypatch.setattr(module, "_prune_orphan_build_helpers", lambda: None)
    monkeypatch.setattr(module, "_prune_backend_daemons", lambda: None)
    monkeypatch.setattr(module, "_prune_stale_build_locks", lambda: None)
    monkeypatch.setattr(
        module.test_policy,
        "collect_test_files",
        lambda *_args, **_kwargs: (target_file,),
    )
    monkeypatch.setattr(module, "_diff_run_id", lambda: "run-id")
    monkeypatch.setattr(module, "_diff_allow_rustc_wrapper", lambda: False)
    monkeypatch.setattr(module, "_diff_trusted_default", lambda: False)
    monkeypatch.setattr(module, "_diff_backend_daemon_default", lambda: False)
    monkeypatch.setattr(module, "_diff_force_no_cache", lambda: False)
    monkeypatch.setattr(module, "_diff_force_rebuild", lambda: False)
    monkeypatch.setattr(module, "_diff_timeout", lambda: 60.0)
    monkeypatch.setattr(module, "_diff_build_timeout", lambda timeout: timeout)
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 0)
    monkeypatch.setattr(module, "_rss_exceeded", lambda metrics, limit: (False, ""))
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda output: None)
    monkeypatch.setattr(module, "_diff_log_passes", lambda: False)

    @contextmanager
    def fake_open_log_file(*_args, **_kwargs):
        yield None

    monkeypatch.setattr(module, "_open_log_file", fake_open_log_file)
    monkeypatch.setattr(
        module,
        "_diff_run_single",
        lambda *args, **kwargs: {
            "path": args[0],
            "status": "pass",
            "stdout": "",
            "stderr": "",
            "duration_s": 0.25,
        },
    )
    monkeypatch.setattr(
        module, "_order_test_files", lambda test_files, jobs: test_files
    )

    def fake_run_molt_build_only(
        file_path: str,
        build_profile: str,
        *,
        execution_context: object,
    ):
        del file_path, build_profile
        assert execution_context.target_python.short == (
            f"{sys.version_info.major}.{sys.version_info.minor}"
        )
        seen_cache_roots.append(os.environ.get("MOLT_CACHE"))
        return module.compat_backends.BackendResult(
            "",
            "",
            0,
            infrastructure_failure=(
                module.memory_guard.GuardInfrastructureFailure(
                    phase="temporary_artifact_custody", details=("cleanup failed",)
                )
                if infrastructure_failure
                else None
            ),
        )

    monkeypatch.setattr(module, "run_molt_build_only", fake_run_molt_build_only)

    if infrastructure_failure:
        with pytest.raises(
            RuntimeError, match="warm-cache infrastructure failed.*cleanup failed"
        ):
            module.run_diff(target_file, "python", warm_cache=True)
        return
    summary = module.run_diff(target_file, "python", warm_cache=True)

    assert summary["failed"] == 0
    assert summary["item_results"] == [
        {
            "path": str(target_file).replace("\\", "/"),
            "status": "pass",
            "duration_s": 0.25,
        }
    ]
    assert seen_cache_roots == [str(ext_root / ".molt_cache")]
