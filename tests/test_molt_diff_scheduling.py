"""Admission and result custody regressions without compiler/worker processes."""

from __future__ import annotations

import concurrent.futures
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

_ROOT = Path(__file__).resolve().parents[1]
for _path in (_ROOT, _ROOT / "tests", _ROOT / "src"):
    if str(_path) not in sys.path:
        sys.path.insert(0, str(_path))

import molt_diff  # noqa: E402


@pytest.fixture
def suite(tmp_path, monkeypatch):
    files = [tmp_path / f"case{index}.py" for index in range(5)]
    state = SimpleNamespace(files=files, calls=[], status="fail", guard_trip=False)
    for name in (
        "_ensure_diff_run_lock",
        "_prune_orphan_diff_workers",
        "_prune_orphan_build_helpers",
        "_prune_backend_daemons",
        "_prune_stale_build_locks",
        "_prepare_memory_guard_run",
        "_print_rss_top",
        "_emit_json",
    ):
        monkeypatch.setattr(molt_diff, name, lambda *a, **k: None)
    monkeypatch.setattr(molt_diff, "_should_preemptive_dyld_quarantine", lambda: False)
    monkeypatch.setattr(
        molt_diff.test_policy, "collect_test_files", lambda *a, **k: files
    )
    config = molt_diff._DiffMemoryGuardConfig(
        max_process_kb=30_000, max_tree_kb=40_000, global_kb=100_000, poll_interval=0.01
    )
    monkeypatch.setattr(molt_diff, "_diff_memory_guard_config", lambda: config)
    monkeypatch.setattr(molt_diff, "_diff_memory_guard_limits", lambda *a: object())
    monkeypatch.setattr(
        molt_diff, "_constrain_jobs_for_memory_guard", lambda jobs, **k: jobs
    )
    monkeypatch.setattr(molt_diff, "_order_test_files", lambda files, jobs: files)
    monkeypatch.setattr(molt_diff, "_diff_run_id", lambda: "scheduler-test")
    monkeypatch.setattr(molt_diff, "_diff_max_tasks_per_child", lambda: None)
    monkeypatch.setattr(molt_diff, "_diff_prune_every", lambda: 0)
    monkeypatch.setattr(molt_diff, "_top_rss_entries", lambda *a, **k: [])
    monkeypatch.setattr(molt_diff, "_aggregate_rss_metrics", lambda *a: {})
    monkeypatch.setattr(molt_diff, "_config_payload", lambda *a: {})
    monkeypatch.setattr(molt_diff, "_memory_guard_scheduler_per_job_gb", lambda *a: 0.1)
    monkeypatch.setattr(
        molt_diff,
        "_memory_guard_trip_message",
        lambda: "tripped" if state.guard_trip else None,
    )
    monkeypatch.setattr(
        molt_diff.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        staticmethod(
            lambda *a, **k: SimpleNamespace(start_repo_sentinel=lambda **k: None)
        ),
    )

    def payload(path):
        if path == str(files[0]) and state.status == "guard":
            state.guard_trip = True
        return {
            "path": path,
            "status": state.status
            if path == str(files[0]) and state.status != "guard"
            else "pass",
            "duration_s": 0.25,
            "stdout": "worker output",
            "stderr": "",
        }

    def serial(path, *a, **k):
        state.calls.append(path)
        return payload(path)

    class Executor:
        def __init__(self, **kwargs):
            self.max_workers = kwargs["max_workers"]

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            pass

        def submit(self, worker, path, *args):
            assert worker is molt_diff._diff_worker
            state.calls.append(path)
            future = concurrent.futures.Future()
            future.path = path
            future.set_running_or_notify_cancel()
            return future

    def complete_one(pending, **kwargs):
        assert kwargs["return_when"] == concurrent.futures.FIRST_COMPLETED
        assert len(pending) <= 2
        future = next(iter(pending))
        if future.path == str(files[0]) and state.status == "exception":
            future.set_exception(RuntimeError("worker protocol failed"))
        else:
            future.set_result(payload(future.path))
        return {future}, set(pending) - {future}

    monkeypatch.setattr(molt_diff, "_diff_run_single", serial)
    monkeypatch.setattr(molt_diff.concurrent.futures, "ProcessPoolExecutor", Executor)
    monkeypatch.setattr(molt_diff.concurrent.futures, "wait", complete_one)
    return state


@pytest.mark.parametrize("jobs", (1, 2))
@pytest.mark.parametrize("status", ("fail", "oom", "uncalibrated", "guard"))
def test_fail_fast_stops_admission_and_retains_running_results(
    suite, tmp_path, jobs, status
):
    suite.status = status
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=jobs,
        fail_fast=True,
        retry_oom=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert suite.calls == [str(path) for path in suite.files[:jobs]]
    assert summary["discovered"] == jobs
    assert summary["failed"] == 1
    assert summary["passed"] == (jobs if status == "guard" else jobs - 1)
    assert len(summary["item_results"]) == jobs


def test_parallel_success_replenishes_bounded_workers(suite, tmp_path):
    suite.status = "pass"
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=2,
        fail_fast=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert suite.calls == [str(path) for path in suite.files]
    assert summary["passed"] == len(suite.files)
    assert summary["failed"] == 0


@pytest.fixture
def guarded_suite(suite, monkeypatch):
    suite.exits = []
    suite.exit_callbacks = []
    suite.close_error = None

    class Sentinel:
        def __exit__(self, *exc):
            suite.exits.append(exc)
            if suite.close_error is not None:
                raise suite.close_error

    monkeypatch.setattr(
        molt_diff.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        staticmethod(
            lambda *a, **k: SimpleNamespace(start_repo_sentinel=lambda **k: Sentinel())
        ),
    )
    monkeypatch.setattr(molt_diff.atexit, "register", suite.exit_callbacks.append)
    monkeypatch.setattr(molt_diff.atexit, "unregister", suite.exit_callbacks.remove)
    return suite


def test_suite_guard_covers_oom_retry_and_summary(guarded_suite, tmp_path, monkeypatch):
    guarded_suite.status = "oom"
    summary_emissions = []

    def retry(path, *args, **kwargs):
        assert guarded_suite.exits == []
        assert len(guarded_suite.exit_callbacks) == 1
        return {
            "path": path,
            "status": "pass",
            "duration_s": 0.1,
            "stdout": "retry output",
            "stderr": "",
        }

    def emit_summary(*args, **kwargs):
        assert guarded_suite.exits == []
        summary_emissions.append((args, kwargs))

    monkeypatch.setattr(molt_diff, "_diff_run_single", retry)
    monkeypatch.setattr(molt_diff, "_emit_json", emit_summary)
    summary = molt_diff.run_diff(
        guarded_suite.files,
        sys.executable,
        jobs=2,
        retry_oom=True,
        failures_output=tmp_path / "failures.txt",
    )

    assert summary["failed"] == 0
    assert summary["passed"] == len(guarded_suite.files)
    assert summary_emissions
    assert len(guarded_suite.exits) == 1
    assert guarded_suite.exit_callbacks == []


def test_worker_exception_drains_running_results_and_releases_suite_guard_once(
    guarded_suite, tmp_path, monkeypatch
):
    guarded_suite.status = "exception"
    log = tmp_path / "statuses.log"
    sentinel_key = molt_diff.harness_memory_guard.repo_sentinel_active_env_key(
        "MOLT_DIFF"
    )
    monkeypatch.setenv(sentinel_key, "previous-owner")
    with pytest.raises(ExceptionGroup, match="differential workers failed") as caught:
        molt_diff.run_diff(
            guarded_suite.files,
            sys.executable,
            jobs=2,
            fail_fast=True,
            log_file=log,
            failures_output=tmp_path / "failures.txt",
        )
    assert "worker protocol failed" in str(caught.value.exceptions[0])
    assert guarded_suite.calls == [str(path) for path in guarded_suite.files[:2]]
    assert f"[PASS] {guarded_suite.files[1]}" in log.read_text()
    assert len(guarded_suite.exits) == 1
    assert guarded_suite.exit_callbacks == []
    assert molt_diff.os.environ[sentinel_key] == "previous-owner"


@pytest.mark.parametrize("previous_owner", (None, "parent-owner"))
def test_suite_cleanup_failure_restores_environment_and_releases_callback(
    guarded_suite, tmp_path, monkeypatch, previous_owner
):
    sentinel_key = molt_diff.harness_memory_guard.repo_sentinel_active_env_key(
        "MOLT_DIFF"
    )
    if previous_owner is None:
        monkeypatch.delenv(sentinel_key, raising=False)
    else:
        monkeypatch.setenv(sentinel_key, previous_owner)
    guarded_suite.close_error = RuntimeError("sentinel close failed")
    with pytest.raises(RuntimeError, match="sentinel close failed"):
        molt_diff.run_diff(
            guarded_suite.files,
            sys.executable,
            jobs=1,
            failures_output=tmp_path / "failures.txt",
        )
    assert len(guarded_suite.exits) == 1
    assert guarded_suite.exit_callbacks == []
    assert molt_diff.os.environ.get(sentinel_key) == previous_owner


def test_failed_failure_receipt_is_not_silently_accepted(guarded_suite, tmp_path):
    guarded_suite.status = "pass"
    blocked = tmp_path / "failures.txt"
    blocked.mkdir()
    with pytest.raises(OSError):
        molt_diff.run_diff(
            guarded_suite.files,
            sys.executable,
            jobs=1,
            failures_output=blocked,
        )
    assert len(guarded_suite.exits) == 1
    assert guarded_suite.exit_callbacks == []
