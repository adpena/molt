from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tests" / "molt_diff.py"


def _load_diff_module():
    spec = importlib.util.spec_from_file_location(
        "molt_diff_module_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_expected_failure_status_maps_fail_to_xfail_pass() -> None:
    module = _load_diff_module()
    status, reason = module._resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="fail",
        cpython_returncode=0,
    )
    assert status == "pass"
    assert reason == "xfail"


def test_expected_failure_status_maps_pass_to_xpass_fail() -> None:
    module = _load_diff_module()
    status, reason = module._resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="pass",
        cpython_returncode=0,
    )
    assert status == "fail"
    assert reason == "xpass"


def test_expected_failure_status_ignored_when_cpython_fails() -> None:
    module = _load_diff_module()
    status, reason = module._resolve_expected_failure_status(
        expect_molt_fail=True,
        raw_status="fail",
        cpython_returncode=1,
    )
    assert status == "fail"
    assert reason is None


def test_manifest_expected_failure_marks_exec_eval_cases(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    manifest = tmp_path / "stdlib_full_coverage_manifest.py"
    manifest.write_text(
        "STDLIB_FULLY_COVERED_MODULES = ()\n"
        "STDLIB_REQUIRED_INTRINSICS_BY_MODULE = {}\n"
        "TOO_DYNAMIC_EXPECTED_FAILURE_TESTS = (\n"
        "  'tests/differential/basic/exec_locals_scope.py',\n"
        "  'tests/differential/basic/eval_locals_scope.py',\n"
        ")\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(module, "_stdlib_full_coverage_manifest_path", lambda: manifest)
    module._too_dynamic_expected_failure_tests.cache_clear()

    assert module._manifest_marks_expected_failure(
        "tests/differential/basic/exec_locals_scope.py"
    )
    assert module._manifest_marks_expected_failure(
        "tests/differential/basic/eval_locals_scope.py"
    )
    assert not module._manifest_marks_expected_failure(
        "tests/differential/basic/arith.py"
    )


def test_repo_manifest_covers_all_exec_eval_cases() -> None:
    module = _load_diff_module()
    module._too_dynamic_expected_failure_tests.cache_clear()
    declared = module._too_dynamic_expected_failure_tests()

    basic_dir = REPO_ROOT / "tests" / "differential" / "basic"
    required = {
        f"tests/differential/basic/{path.name}" for path in basic_dir.glob("exec*.py")
    } | {f"tests/differential/basic/{path.name}" for path in basic_dir.glob("eval*.py")}

    missing = sorted(required - declared)
    assert not missing


def test_repo_manifest_has_no_policy_deferred_runpy_dynamic_cases() -> None:
    module = _load_diff_module()
    module._too_dynamic_expected_failure_tests.cache_clear()
    declared = module._too_dynamic_expected_failure_tests()
    deferred_runpy = sorted(path for path in declared if "/stdlib/runpy_" in path)
    assert not deferred_runpy


def test_repo_manifest_dynamic_policy_docs_exist() -> None:
    manifest_path = REPO_ROOT / "tools" / "stdlib_full_coverage_manifest.py"
    namespace = {}
    exec(manifest_path.read_text(encoding="utf-8"), namespace)
    docs = namespace.get("TOO_DYNAMIC_POLICY_DOC_REFERENCES", ())
    assert isinstance(docs, tuple)
    assert docs
    required = {
        "docs/spec/areas/core/0000-vision.md",
        "docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md",
        "docs/spec/areas/testing/0007-testing.md",
        "docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md",
    }
    assert required.issubset(set(docs))
    missing = [doc for doc in docs if not (REPO_ROOT / doc).exists()]
    assert not missing


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
    client._response_queue = module.queue.Queue()

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


def test_batch_compile_server_ping_failure_force_closes_and_cools_down(
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
    assert "ping timeout" in error
    assert ("close", True) in events

    client_retry, retry_error = module._batch_compile_server_client(
        {},
        request_timeout=0.1,
    )
    assert client_retry is None
    assert retry_error is not None
    assert "temporarily disabled" in retry_error


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
            return None, "transient startup failure"
        return _FakeClient(), None

    def _fake_reset_disabled() -> None:
        resets["count"] += 1

    monkeypatch.setattr(module, "_batch_compile_server_client", _fake_client_factory)
    monkeypatch.setattr(
        module, "_batch_compile_server_reset_disabled", _fake_reset_disabled
    )

    rc, stdout, stderr, error = module._run_batch_compile_build(
        env={"MOLT_CODEC": "msgpack"},
        file_path="tests/differential/basic/arith.py",
        output_root=tmp_path,
        output_binary=tmp_path / "arith_molt",
        build_profile="dev",
        no_cache=False,
        rebuild=False,
        request_timeout=12.0,
        strict_mode=True,
    )

    assert error is None
    assert rc == 0
    assert stdout == "ok"
    assert stderr == ""
    assert attempts["count"] == 2
    assert resets["count"] == 1


def test_run_batch_compile_build_error_path_force_closes_server(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_diff_module()
    shutdown_calls: list[bool] = []
    disabled_reasons: list[str] = []

    class _FailingClient:
        def request(self, op: str, *, params=None, timeout: float) -> dict[str, object]:
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

    rc, stdout, stderr, error = module._run_batch_compile_build(
        env={"MOLT_CODEC": "msgpack"},
        file_path="tests/differential/basic/arith.py",
        output_root=tmp_path,
        output_binary=tmp_path / "arith_molt",
        build_profile="dev",
        no_cache=False,
        rebuild=False,
        request_timeout=8.0,
        strict_mode=False,
    )

    assert rc == 0
    assert stdout == ""
    assert stderr == ""
    assert error == "build timed out"
    assert shutdown_calls == [True]
    assert disabled_reasons == ["build timed out"]
