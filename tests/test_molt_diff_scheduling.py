"""Admission and result custody regressions without compiler/worker processes."""

from __future__ import annotations

import concurrent.futures
import sys
import signal
from pathlib import Path
from types import SimpleNamespace

import pytest

_ROOT = Path(__file__).resolve().parents[1]
for _path in (_ROOT, _ROOT / "tests", _ROOT / "src"):
    if str(_path) not in sys.path:
        sys.path.insert(0, str(_path))

import molt_diff  # noqa: E402


def _started(summary: dict) -> list[dict]:
    return [row for row in summary["item_results"] if row["status"] != "not_run"]


@pytest.fixture
def suite(tmp_path, monkeypatch):
    files = [tmp_path / f"case{index}.py" for index in range(5)]
    state = SimpleNamespace(
        files=files,
        calls=[],
        status="fail",
        guard_trip=False,
        trip_failure=None,
        guard_signal=None,
        original_trip_outcome=molt_diff._memory_guard_trip_outcome,
    )
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
        "_memory_guard_trip_outcome",
        lambda sentinel=None: (
            molt_diff.compat_backends.BackendResult(
                "",
                "tripped",
                137,
                rss_limit_exceeded=state.trip_failure is None,
                infrastructure_failure=state.trip_failure,
            )
            if state.guard_trip
            else None
        ),
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
            if state.trip_failure is not None and hasattr(state, "sentinel"):
                state.sentinel.tripped = True
                state.sentinel.infrastructure_failure = state.trip_failure
        return {
            "path": path,
            "status": state.status
            if path == str(files[0]) and state.status != "guard"
            else "pass",
            "guard_signal": state.guard_signal if path == str(files[0]) else None,
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
    # Unstarted tests are named, never dropped.
    rows = summary["item_results"]
    assert len(rows) == len(suite.files)
    assert summary["not_run"] == len(suite.files) - jobs
    assert {row["reason"] for row in rows if row["status"] == "not_run"} == {
        "admission stopped"
    }


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


@pytest.mark.parametrize("jobs", (1, 2))
def test_deadline_admits_only_tests_that_can_finish(suite, tmp_path, monkeypatch, jobs):
    suite.status = "pass"
    monkeypatch.setenv("MOLT_DIFF_TIMEOUT", "100")
    # Each test may take build 300 s + run 100 s + CPython 100 s; a 400 s
    # budget cannot fit one, so nothing starts and every test is named.
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=jobs,
        failures_output=tmp_path / "failures.txt",
        deadline_s=400,
    )
    assert suite.calls == []
    assert summary["not_run"] == len(suite.files)
    assert {row["reason"] for row in summary["item_results"]} == {"deadline"}

    suite.calls.clear()
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=jobs,
        failures_output=tmp_path / "failures.txt",
        deadline_s=10_000,
    )
    assert suite.calls == [str(path) for path in suite.files]
    assert (summary["passed"], summary["not_run"]) == (len(suite.files), 0)


@pytest.mark.parametrize(
    ("cutoff_phase", "expected_retries"),
    [("parallel-drain", 0), ("first-retry", 1), ("never", 2)],
)
def test_oom_retries_share_the_run_admission_deadline(
    suite, tmp_path, monkeypatch, capsys, cutoff_phase, expected_retries
):
    suite.files[:] = suite.files[:2]
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(molt_diff, "_PROCESS_STARTED", 0.0)
    monkeypatch.setattr(molt_diff.time, "monotonic", lambda: clock.now)
    monkeypatch.setenv("MOLT_DIFF_TIMEOUT", "10")
    monkeypatch.setenv("MOLT_DIFF_BUILD_TIMEOUT", "30")
    # The real admission policy reserves 30 s build + 10 s run + 10 s CPython.
    # Advancing to 50 s exhausts a 100 s run's remaining test budget.
    retries = []

    def payload(path, status):
        return {
            "path": str(path),
            "status": status,
            "guard_signal": None,
            "duration_s": 0.25,
            "stdout": "",
            "stderr": "",
        }

    def parallel(_executor, files, *args, **kwargs):
        for path in files:
            assert kwargs["admits"]()
            suite.calls.append(str(path))
            yield payload(path, "oom")
        if cutoff_phase == "parallel-drain":
            clock.now = 50.0

    def retry(path, *args):
        retries.append(path)
        if cutoff_phase == "first-retry":
            clock.now = 50.0
        return payload(path, "pass")

    monkeypatch.setattr(molt_diff, "_parallel_diff_results", parallel)
    monkeypatch.setattr(molt_diff, "_diff_run_single", retry)
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=2,
        retry_oom=True,
        deadline_s=100,
        failures_output=tmp_path / "failures.txt",
    )
    assert retries == [str(path) for path in suite.files[:expected_retries]]
    assert summary["discovered"] == 2 and summary["not_run"] == 0
    assert summary["passed"] == expected_retries
    assert summary["failed"] == 2 - expected_retries
    assert summary["oom"] == 2 - expected_retries
    assert ("[RETRY-OOM]" in capsys.readouterr().out) is bool(expected_retries)


def test_deadline_requires_a_per_test_bound(suite, tmp_path, monkeypatch):
    monkeypatch.delenv("MOLT_DIFF_TIMEOUT", raising=False)
    with pytest.raises(ValueError, match="requires MOLT_DIFF_TIMEOUT"):
        molt_diff.run_diff(
            suite.files,
            sys.executable,
            jobs=2,
            failures_output=tmp_path / "failures.txt",
            deadline_s=10_000,
        )


@pytest.fixture
def guarded_suite(suite, monkeypatch):
    suite.exits = []
    suite.exit_callbacks = []
    suite.close_error = None

    class Sentinel:
        tripped = False
        infrastructure_failure = None

        def __exit__(self, *exc):
            suite.exits.append(exc)
            if suite.close_error is not None:
                raise suite.close_error

    suite.sentinel = Sentinel()
    monkeypatch.setattr(
        molt_diff.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        staticmethod(
            lambda *a, **k: SimpleNamespace(
                start_repo_sentinel=lambda **k: suite.sentinel
            )
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
            "guard_signal": None,
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
    assert f"[PASS] {guarded_suite.files[1]}" in log.read_text(encoding="utf-8")
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
    previous_handler = signal.getsignal(signal.SIGTERM)
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
    assert signal.getsignal(signal.SIGTERM) is previous_handler


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


@pytest.mark.parametrize("jobs", [1, 2])
@pytest.mark.parametrize("origin", ["marker", "sentinel"])
def test_parent_trip_infrastructure_stops_admission_without_oom(
    guarded_suite, tmp_path, monkeypatch, jobs, origin
):
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

    suite = guarded_suite
    suite.status = "guard"
    suite.trip_failure = GuardInfrastructureFailure(
        phase="rss_trip_evidence", details=("callback publication failed",)
    )
    monkeypatch.setattr(
        molt_diff, "_memory_guard_trip_outcome", suite.original_trip_outcome
    )
    marker = tmp_path / "trip.json"
    monkeypatch.setattr(molt_diff, "_diff_memory_guard_trip_file", lambda: marker)
    if origin == "marker":
        original = molt_diff._memory_guard_trip_outcome

        def read_marker(sentinel=None):
            if suite.guard_trip:
                marker.write_text(
                    '{"event":"guard_tripped","violation":null}', encoding="utf-8"
                )
            return original(None)

        monkeypatch.setattr(molt_diff, "_memory_guard_trip_outcome", read_marker)
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=jobs,
        retry_oom=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert suite.calls == [str(path) for path in suite.files[:jobs]]
    assert summary["discovered"] == jobs
    assert summary["failed"] == 1 and summary["oom"] == 0
    assert summary["guard_infrastructure_failures"] == 1
    guard = summary["config"]["memory_guard"]
    assert guard["admission_stopped"] and not guard["tripped"]
    assert guard["infrastructure_failure"]["phase"] == "rss_trip_evidence"
    assert summary["passed"] == jobs


@pytest.mark.parametrize("jobs", (1, 2))
def test_typed_signal_stops_without_fail_fast_and_drains_running_results(
    guarded_suite, tmp_path, jobs
):
    guarded_suite.status = "uncalibrated"
    guarded_suite.guard_signal = signal.SIGTERM
    summary = molt_diff.run_diff(
        guarded_suite.files,
        sys.executable,
        jobs=jobs,
        retry_oom=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert guarded_suite.calls == [str(path) for path in guarded_suite.files[:jobs]]
    assert summary["guard_signal"] == signal.SIGTERM
    assert summary["failed"] == 1 and summary["passed"] == jobs - 1
    assert summary["item_results"][0]["guard_signal"] == signal.SIGTERM
    assert len(_started(summary)) == jobs
    assert summary["not_run"] == len(summary["item_results"]) - jobs
    assert summary["config"]["memory_guard"]["admission_stopped"]
    assert len(guarded_suite.exits) == 1


@pytest.mark.parametrize("jobs", (1, 2))
def test_between_commands_signal_defers_exit_until_running_results_drained(
    guarded_suite, tmp_path, monkeypatch, jobs
):
    guarded_suite.status = "pass"
    previous = signal.getsignal(signal.SIGTERM)
    receiver = "_diff_run_single" if jobs == 1 else "wait"
    owner = molt_diff if jobs == 1 else molt_diff.concurrent.futures
    original = getattr(owner, receiver)
    delivered = []

    def receive(*args, **kwargs):
        if not delivered:
            delivered.append(True)
            signal.raise_signal(signal.SIGTERM)
        return original(*args, **kwargs)

    monkeypatch.setattr(owner, receiver, receive)
    summary = molt_diff.run_diff(
        guarded_suite.files,
        sys.executable,
        jobs=jobs,
        retry_oom=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert guarded_suite.calls == [str(path) for path in guarded_suite.files[:jobs]]
    assert summary["guard_signal"] == signal.SIGTERM
    assert summary["passed"] == jobs and summary["failed"] == 1
    assert summary["failed_files"] == ["<operator_signal>"]
    assert len(_started(summary)) == jobs
    assert summary["not_run"] == len(summary["item_results"]) - jobs
    assert len(guarded_suite.exits) == 1
    assert signal.getsignal(signal.SIGTERM) is previous


def test_signal_during_warmup_stops_case_and_further_warmup_admission(
    guarded_suite, tmp_path, monkeypatch
):
    warmups = []
    monkeypatch.setattr(molt_diff, "_backend_execution_context", lambda *a: object())

    def warmup(path, *args, **kwargs):
        warmups.append(path)
        return molt_diff.compat_backends.BackendResult(
            None,
            "interrupted warmup",
            130,
            guard_signal=signal.SIGINT,
        )

    monkeypatch.setattr(molt_diff, "run_molt_build_only", warmup)
    summary = molt_diff.run_diff(
        guarded_suite.files,
        sys.executable,
        jobs=1,
        warm_cache=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert warmups == [str(guarded_suite.files[0])]
    assert guarded_suite.calls == []
    assert summary["guard_signal"] == signal.SIGINT
    assert _started(summary) == [] and summary["passed"] == 0
    assert summary["not_run"] == len(guarded_suite.files)
    assert summary["failed_files"] == ["<operator_signal>"]
    assert len(guarded_suite.exits) == 1


def test_signal_during_oom_retry_stops_next_retry(guarded_suite, tmp_path, monkeypatch):
    original_wait = molt_diff.concurrent.futures.wait

    def all_oom(*args, **kwargs):
        done, pending = original_wait(*args, **kwargs)
        for future in done:
            # All initial cases fail ordinarily; only a later retry is cancelled.
            future.result()["status"] = "oom"
        return done, pending

    retries = []

    def retry(path, *args, **kwargs):
        retries.append(path)
        return {
            "path": path,
            "status": "uncalibrated",
            "guard_signal": 15,
            "duration_s": 0.1,
            "stdout": "",
            "stderr": "interrupted",
        }

    monkeypatch.setattr(molt_diff.concurrent.futures, "wait", all_oom)
    monkeypatch.setattr(molt_diff, "_diff_run_single", retry)
    summary = molt_diff.run_diff(
        guarded_suite.files,
        sys.executable,
        jobs=2,
        retry_oom=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert retries == [str(guarded_suite.files[0])]
    assert summary["guard_signal"] == 15
    assert summary["oom"] == len(guarded_suite.files) - 1
    assert summary["failed"] == len(guarded_suite.files)
    assert len(guarded_suite.exits) == 1


@pytest.mark.parametrize("jobs", (1, 2))
@pytest.mark.parametrize("status", ("fail", "oom", "uncalibrated"))
def test_ordinary_failure_without_operator_signal_keeps_admitting(
    suite, tmp_path, jobs, status
):
    suite.status = status
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=jobs,
        failures_output=tmp_path / "failures.txt",
    )
    assert suite.calls == [str(path) for path in suite.files]
    assert summary["guard_signal"] is None
    assert summary["passed"] == len(suite.files) - 1
    assert summary["failed"] == 1
    assert not summary["config"]["memory_guard"]["admission_stopped"]
