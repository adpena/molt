"""Admission and result custody regressions without compiler/worker processes."""

from __future__ import annotations

import concurrent.futures
import gc
import json
import sys
import signal
import weakref
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

_ROOT = Path(__file__).resolve().parents[1]
for _path in (_ROOT, _ROOT / "tests", _ROOT / "src"):
    if str(_path) not in sys.path:
        sys.path.insert(0, str(_path))

import molt_diff  # noqa: E402
from tests.process_guard_common import (  # noqa: E402
    install_module_view,
    run_guarded_test_process,
)


@pytest.mark.parametrize("queued", [False, True])
def test_evidence_directory_uses_configured_run_scratch(tmp_path, queued):
    repo = tmp_path / "repo"
    repo.mkdir()
    artifact_root = tmp_path / "artifacts"
    env = {
        "MOLT_EXT_ROOT": str(artifact_root),
        "MOLT_TARGET_ROOT": str(tmp_path / "tools"),
        "MOLT_USE_SCCACHE": "0",
    }
    if queued:
        env["MOLT_PROOF_SCRATCH_ROOT"] = str(tmp_path / "proof-scratch")
    molt_diff._configure_diff_artifact_environment(repo_root=repo, environment=env)
    args = SimpleNamespace(
        evidence_dir="runtime/import", json_output=None, log_aggregate=None, json=True
    )

    molt_diff._configure_diff_evidence(args, repo_root=repo, environment=env)

    root = tmp_path / "proof-scratch" if queued else Path(env["MOLT_DIFF_TMPDIR"])
    evidence = root / "differential" / "runtime" / "import"
    assert evidence.is_dir()
    assert env["MOLT_DIFF_SUMMARY"] == str(evidence / "summary.json")
    assert env["MOLT_DIFF_RESULTS_JSONL"] == str(evidence / "results.jsonl")
    assert args.log_aggregate == str(evidence / "output.log")
    assert args.json_output is None
    assert args.json is True
    assert not list(repo.iterdir())


@pytest.mark.parametrize(
    "selectors",
    [
        ("--json-output",),
        ("--log-aggregate",),
        ("MOLT_DIFF_SUMMARY",),
        ("MOLT_DIFF_RESULTS_JSONL",),
        (
            "--json-output",
            "--log-aggregate",
            "MOLT_DIFF_SUMMARY",
            "MOLT_DIFF_RESULTS_JSONL",
        ),
    ],
)
def test_evidence_directory_reports_all_competing_owners(tmp_path, selectors):
    env = {"MOLT_PROOF_SCRATCH_ROOT": str(tmp_path / "scratch")}
    env.update({name: "elsewhere" for name in selectors if not name.startswith("--")})
    args = SimpleNamespace(
        evidence_dir="runtime-import",
        json_output="elsewhere" if "--json-output" in selectors else None,
        log_aggregate="elsewhere" if "--log-aggregate" in selectors else None,
    )
    original_env = dict(env)
    original_args = vars(args).copy()

    with pytest.raises(ValueError) as error:
        molt_diff._configure_diff_evidence(args, repo_root=tmp_path, environment=env)

    assert all(selector in str(error.value) for selector in selectors)
    assert env == original_env and vars(args) == original_args
    assert not (tmp_path / "scratch").exists()


@pytest.mark.parametrize(
    "purpose", ["/absolute", "../escape", "a/../escape", r"C:\escape"]
)
def test_evidence_directory_rejects_nonportable_purposes(tmp_path, purpose):
    env = {"MOLT_PROOF_SCRATCH_ROOT": str(tmp_path / "scratch")}
    args = SimpleNamespace(evidence_dir=purpose, json_output=None, log_aggregate=None)

    with pytest.raises(ValueError, match="portable relative"):
        molt_diff._configure_diff_evidence(args, repo_root=tmp_path, environment=env)

    assert not (tmp_path / "scratch").exists()


def test_explicit_artifact_paths_keep_the_existing_mode(tmp_path):
    args = SimpleNamespace(
        evidence_dir=None, json_output="summary.json", log_aggregate="output.log"
    )
    env = {"MOLT_DIFF_SUMMARY": "original", "MOLT_DIFF_RESULTS_JSONL": "results"}
    original_env = dict(env)
    original_args = vars(args).copy()

    molt_diff._configure_diff_evidence(args, repo_root=tmp_path, environment=env)

    assert env == original_env and vars(args) == original_args


def test_evidence_directory_rejects_a_link_outside_owned_scratch(tmp_path):
    scratch = tmp_path / "scratch"
    outside = tmp_path / "outside"
    scratch.mkdir()
    outside.mkdir()
    try:
        (scratch / "differential").symlink_to(outside, target_is_directory=True)
    except OSError:
        pytest.skip("directory symlinks unavailable on this host")
    args = SimpleNamespace(
        evidence_dir="runtime-import", json_output=None, log_aggregate=None
    )
    env = {"MOLT_PROOF_SCRATCH_ROOT": str(scratch)}

    with pytest.raises(ValueError, match="link|junction"):
        molt_diff._configure_diff_evidence(args, repo_root=tmp_path, environment=env)

    assert not list(outside.iterdir())


def test_cli_evidence_directory_reports_every_conflict_before_execution(tmp_path):
    env = {
        **molt_diff.os.environ,
        "MOLT_EXT_ROOT": str(tmp_path / "artifacts"),
        "MOLT_TARGET_ROOT": str(tmp_path / "tools"),
        "MOLT_USE_SCCACHE": "0",
        "MOLT_DIFF_SUMMARY": str(tmp_path / "old-summary.json"),
        "MOLT_DIFF_RESULTS_JSONL": str(tmp_path / "old-results.jsonl"),
    }
    result = run_guarded_test_process(
        [
            sys.executable,
            str(_ROOT / "tests/molt_diff.py"),
            "--evidence-dir",
            "runtime-import",
            "--json-output",
            str(tmp_path / "explicit-summary.json"),
            "--log-aggregate",
            str(tmp_path / "explicit-output.log"),
            str(_ROOT / "examples/hello.py"),
        ],
        env=env,
        capture_output=True,
        text=True,
        timeout=15,
    )

    assert result.returncode == 2, result.stdout + result.stderr
    assert (
        "conflicts with explicit artifact paths: --json-output, --log-aggregate, "
        "MOLT_DIFF_SUMMARY, MOLT_DIFF_RESULTS_JSONL"
    ) in result.stderr
    assert not (tmp_path / "explicit-summary.json").exists()
    assert not (tmp_path / "explicit-output.log").exists()


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
        original_run_single=molt_diff._diff_run_single,
        original_emit_json=molt_diff._emit_json,
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
    monkeypatch.setattr(molt_diff, "_diff_log_passes", lambda: False)
    monkeypatch.setattr(molt_diff, "_top_rss_entries", lambda *a, **k: [])
    monkeypatch.setattr(molt_diff, "_aggregate_rss_metrics", lambda *a: {})
    monkeypatch.setattr(molt_diff, "_config_payload", lambda *a: {})
    monkeypatch.setattr(molt_diff, "_memory_guard_scheduler_per_job_gb", lambda *a: 0.1)
    monkeypatch.setattr(
        molt_diff,
        "_memory_guard_trip_outcome",
        lambda sentinel=None, *, environment=None: (
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
            state.executor_kwargs = kwargs

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


def test_evidence_directory_keeps_one_summary_writer(
    suite, tmp_path, monkeypatch, capsys
):
    args = SimpleNamespace(
        evidence_dir="runtime-import", json_output=None, log_aggregate=None, json=True
    )
    env = {"MOLT_PROOF_SCRATCH_ROOT": str(tmp_path / "proof-scratch")}
    molt_diff._configure_diff_evidence(args, repo_root=tmp_path, environment=env)
    for name, value in env.items():
        monkeypatch.setenv(name, value)
    writes = []

    def emit(summary, path, stdout):
        if path:
            writes.append(path)
        suite.original_emit_json(summary, path, stdout)

    monkeypatch.setattr(molt_diff, "_emit_json", emit)
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=1,
        log_aggregate=Path(args.log_aggregate),
        failures_output=tmp_path / "failures.txt",
    )
    capsys.readouterr()
    molt_diff._emit_json(summary, args.json_output, args.json)

    assert writes == [env["MOLT_DIFF_SUMMARY"]]
    assert json.loads(Path(writes[0]).read_text()) == json.loads(json.dumps(summary))
    assert Path(args.log_aggregate).is_file()
    assert json.loads(capsys.readouterr().out) == json.loads(json.dumps(summary))


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


@pytest.mark.parametrize("live", (False, True))
@pytest.mark.parametrize("retry_oom", (False, True))
def test_parallel_diagnostics_publish_at_completion_without_retaining_history(
    suite, tmp_path, monkeypatch, capsys, live, retry_oom
):
    class CapturedText(str):
        pass

    retained = []
    statuses = ["fail", "oom", "uncalibrated", "pass", "skip"]

    def completions(_executor, files, _python, _profile, targets, _version, **kwargs):
        assert targets == ("native", "wasm")
        for index, (path, status) in enumerate(zip(files, statuses, strict=True)):
            result = {
                "path": str(path),
                "status": status,
                "guard_signal": None,
                "duration_s": 0.25,
                "stdout": CapturedText(f"detail-out-{index}\n"),
                "stderr": CapturedText(f"detail-error-{index}\n"),
            }
            retained.append(weakref.ref(result["stdout"]))
            yield result
            # Observe before another completion can hide missing diagnostics.
            captured = capsys.readouterr()
            assert f"[{status.upper()}] {path}" in captured.out
            if status in {"fail", "oom", "uncalibrated"}:
                assert captured.out.count(f"detail-out-{index}\n") == 1
                assert captured.err == f"detail-error-{index}\n"
            else:
                assert "detail-out" not in captured.out and not captured.err
            del result
            gc.collect()
            # The loop may still own its current payload, never older history.
            assert all(reference() is None for reference in retained[:-1])

    monkeypatch.setattr(molt_diff, "_parallel_diff_results", completions)
    status_log = tmp_path / "status.log"
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=2,
        targets=("native", "wasm"),
        live=live,
        retry_oom=retry_oom,
        log_file=status_log,
        failures_output=tmp_path / "failures.txt",
    )
    assert summary["discovered"] == len(statuses)
    assert summary["failed"] == (2 if retry_oom else 3)
    assert suite.calls == ([str(suite.files[1])] if retry_oom else [])
    assert all(reference() is None for reference in retained)
    assert "detail-out" not in status_log.read_text(encoding="utf-8")


@pytest.mark.parametrize("sink", ("directory", "aggregate", "unusable-directory"))
@pytest.mark.parametrize("live", (False, True))
def test_parallel_detail_sink_owns_payload_or_console_receives_it(
    suite, tmp_path, capsys, sink, live
):
    log_dir = tmp_path / "details" if sink != "aggregate" else None
    aggregate = tmp_path / "aggregate.log" if sink == "aggregate" else None
    stale_pass_log = None
    if sink == "unusable-directory":
        log_dir.write_text("occupied", encoding="utf-8")
    elif log_dir is not None:
        log_dir.mkdir()
        stale_pass_log = molt_diff._log_path_for_test(log_dir, str(suite.files[1]))
        stale_pass_log.write_text("stale pass details", encoding="utf-8")
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=2,
        live=live,
        log_dir=log_dir,
        log_aggregate=aggregate,
        failures_output=tmp_path / "failures.txt",
    )
    captured = capsys.readouterr()
    assert summary["failed"] == 1
    if stale_pass_log is not None:
        assert not stale_pass_log.exists()
    if sink == "unusable-directory":
        assert captured.out.count("worker output") == 1
        assert "Warning: failed to create log dir" in captured.out
    else:
        assert "worker output" not in captured.out
        detail_path = (
            aggregate
            if aggregate is not None
            else molt_diff._log_path_for_test(log_dir, str(suite.files[0]))
        )
        assert detail_path.read_text(encoding="utf-8").count("worker output") == 1


@pytest.mark.parametrize("absolute_log_dir", (False, True))
def test_case_log_identity_is_contained_bounded_and_unambiguous(
    tmp_path, monkeypatch, absolute_log_dir
):
    repo = tmp_path / "checkout"
    repo.mkdir()
    invocation = tmp_path / "invocation"
    invocation.mkdir()
    monkeypatch.chdir(invocation)
    monkeypatch.setattr(molt_diff, "_repo_root", lambda: repo)
    log_dir = invocation / "logs" if absolute_log_dir else Path("logs")
    inside = "tests/differential/basic/one.py"
    canonical = molt_diff._log_path_for_test(log_dir, inside)
    assert canonical == molt_diff._log_path_for_test(log_dir, str(repo / inside))
    assert canonical == molt_diff._log_path_for_test(
        log_dir, "tests/differential/basic/../basic/one.py"
    )
    external = tmp_path / "external" / "one.py"
    assert molt_diff._log_path_for_test(log_dir, str(external)) == (
        molt_diff._log_path_for_test(log_dir, "../external/one.py")
    )
    cases = [
        inside,
        str(external),
        str(tmp_path / "other-external" / "one.py"),
        "a_/b.py",
        "a/_b.py",
        "a__b.py",
        "CON.py",
        "Case.py",
        "case.py",
        "unicode-雪.py",
        "/".join(["deep" * 30] * 4 + ["one.py"]),
    ]
    paths = []
    for index, case in enumerate(cases):
        path = molt_diff._write_test_log(
            log_dir, case, f"out-{index}\n", f"err-{index}\n"
        )
        paths.append(path)
        assert path.parent == log_dir
        assert path.resolve().is_relative_to(log_dir.resolve())
        assert len(path.name.encode("ascii")) == 68
        assert path.suffix == ".log"
        assert set(path.stem) <= set("0123456789abcdef")
        assert path.read_text(encoding="utf-8") == (
            f"STDOUT:\nout-{index}\n\nSTDERR:\nerr-{index}\n"
        )
    assert len(set(paths)) == len(cases)
    assert len(list(log_dir.iterdir())) == len(cases)


@pytest.mark.parametrize("jobs", (1, 2))
def test_serial_and_oom_retry_tee_diagnostics_are_not_replayed(
    suite, tmp_path, monkeypatch, capsys, jobs
):
    suite.files[:] = suite.files[:2]
    suite.status = "oom"
    serial_calls = []

    def diagnostic_case(path, _python, **kwargs):
        serial_calls.append(path)
        print("serial-out")
        print("serial-error", file=sys.stderr)
        return {"resolved_status": "pass", "guard_signal": None}

    monkeypatch.setattr(molt_diff, "diff_test", diagnostic_case)
    monkeypatch.setattr(molt_diff, "_diff_run_single", suite.original_run_single)
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=jobs,
        retry_oom=True,
        failures_output=tmp_path / "failures.txt",
    )
    captured = capsys.readouterr()
    assert summary["passed"] == 2 and summary["failed"] == 0
    expected = 2 if jobs == 1 else 1
    assert len(serial_calls) == expected
    assert captured.out.count("serial-out\n") == expected
    assert captured.err.count("serial-error\n") == expected
    if jobs == 2:
        assert captured.out.count("worker output") == 1
        assert captured.out.index("worker output") < captured.out.index("[RETRY-OOM]")
        assert captured.out.index("[RETRY-OOM]") < captured.out.index("serial-out")


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
    # The fake clock reaches only the two modules that keep differential
    # deadlines; pytest, the guard and every other reader keep real time.
    install_module_view(
        monkeypatch,
        "time",
        time,
        molt_diff,
        molt_diff.compat_backends,
        monotonic=lambda: clock.now,
    )
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
    monkeypatch.setenv(molt_diff._DIFF_MEMORY_GUARD_TRIP_FILE_ENV, str(marker))
    if origin == "marker":
        original = molt_diff._memory_guard_trip_outcome

        def read_marker(sentinel=None, *, environment=None):
            if suite.guard_trip:
                marker.write_text(
                    '{"event":"guard_tripped","violation":null}', encoding="utf-8"
                )
            return original(None, environment=environment)

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
    if jobs > 1:
        # Every worker receives the suite stop, and the signal sets it, so the
        # commands those workers are running end at once (HF-232).
        kwargs = guarded_suite.executor_kwargs
        assert kwargs["initializer"] is molt_diff._install_worker_state
        (stop,) = kwargs["initargs"]
        assert stop.value == signal.SIGTERM
    assert molt_diff.compat_backends.suite_stop_signal() is None


def test_signal_during_warmup_stops_case_and_further_warmup_admission(
    guarded_suite, tmp_path, monkeypatch
):
    warmups = []
    monkeypatch.setattr(
        molt_diff, "_backend_execution_context", lambda *a, budget: object()
    )

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


def test_item_reservation_uses_actual_mixed_backend_caps(monkeypatch):
    backends = molt_diff.compat_backends
    policy = backends.TimeoutPolicy.from_environment(
        {
            "MOLT_DIFF_TIMEOUT": "10",
            "MOLT_DIFF_BUILD_TIMEOUT": "30",
            "MOLT_COMPAT_LLVM_BUILD_TIMEOUT_SEC": "17",
            "MOLT_COMPAT_WASM_RUN_TIMEOUT_SEC": "3",
        }
    )
    assert policy.reservation(("native", "wasm", "llvm", "luau")) == 1390
    assert policy.phase("wasm", "build") == 600
    assert policy.phase("llvm", "run") == 60
    assert policy.phase("luau", "build") == 600
    assert policy.reservation(("native",), cpython=False, build_only=True) == 30
    disabled = backends.TimeoutPolicy.from_environment(
        {
            "MOLT_DIFF_TIMEOUT": "10",
            "MOLT_COMPAT_LLVM_BUILD_TIMEOUT_SEC": "off",
        }
    )
    assert disabled.reservation(("llvm",)) is None


def test_delayed_worker_spends_dispatch_budget_before_any_case_side_effect(monkeypatch):
    backends = molt_diff.compat_backends
    clock = SimpleNamespace(now=100.0)
    monkeypatch.setattr(backends, "time", SimpleNamespace(monotonic=lambda: clock.now))
    policy = backends.TimeoutPolicy.from_environment(
        {
            "MOLT_DIFF_TIMEOUT": "10",
            "MOLT_DIFF_BUILD_TIMEOUT": "30",
        }
    )
    captured = []
    monkeypatch.setattr(molt_diff, "_install_worker_orphan_guard", lambda: None)
    monkeypatch.setattr(molt_diff, "_record_diff_result", captured.append)
    monkeypatch.setattr(
        molt_diff.test_policy,
        "parse_metadata",
        lambda path: pytest.fail("expired worker parsed case metadata"),
    )
    monkeypatch.setattr(
        molt_diff.diff_output_layout,
        "admit",
        lambda *a, **k: pytest.fail("expired worker acquired artifact roots"),
    )

    class DelayedExecutor:
        def submit(self, worker, *args):
            assert args[-1].deadline == 150.0
            clock.now = 151.0
            future = concurrent.futures.Future()
            future.set_result(worker(*args))
            return future

    result = list(
        molt_diff._parallel_diff_results(
            DelayedExecutor(),
            [Path("case.py")],
            sys.executable,
            "dev",
            ("native",),
            molt_diff.TargetPythonVersion(3, 12, 0),
            jobs=1,
            fail_fast=False,
            trip_outcome=lambda: None,
            new_budget=lambda: backends.ItemBudget.start(policy, ("native",)),
        )
    )
    assert len(result) == 1 and result[0]["status"] == "fail"
    assert captured[0]["reason_tag"] == "timeout"


def test_warm_cache_uses_dispatch_pinned_build_only_budget(
    suite, tmp_path, monkeypatch
):
    backends = molt_diff.compat_backends
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(backends, "time", SimpleNamespace(monotonic=lambda: clock.now))
    monkeypatch.setenv("MOLT_DIFF_TIMEOUT", "10")
    monkeypatch.setenv("MOLT_DIFF_BUILD_TIMEOUT", "30")
    budgets = []

    def context(file, python, profile, target, *, budget):
        budgets.append(budget)
        return SimpleNamespace(budget=budget)

    def warm(file, profile, *, execution_context):
        assert execution_context.budget.deadline == clock.now + 30
        clock.now += 1
        return backends.BackendResult("", "", 0)

    monkeypatch.setattr(molt_diff, "_backend_execution_context", context)
    monkeypatch.setattr(molt_diff, "run_molt_build_only", warm)
    molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=1,
        warm_cache=True,
        failures_output=tmp_path / "failures.txt",
    )
    assert len(budgets) == len(suite.files)
    assert [item.deadline for item in budgets] == [30, 31, 32, 33, 34]


def test_suite_preflight_consumes_deadline_and_keeps_not_run_summary(
    suite, tmp_path, monkeypatch
):
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(molt_diff, "_PROCESS_STARTED", 0.0)
    # The fake clock reaches only the two modules that keep differential
    # deadlines; pytest, the guard and every other reader keep real time.
    install_module_view(
        monkeypatch,
        "time",
        time,
        molt_diff,
        molt_diff.compat_backends,
        monotonic=lambda: clock.now,
    )
    monkeypatch.setenv("MOLT_DIFF_TIMEOUT", "1")
    monkeypatch.setenv("MOLT_DIFF_BUILD_TIMEOUT", "1")
    observed = []

    def resolve(version, *, budget):
        observed.append(budget.probe_timeout(10))
        clock.now = 5.0
        budget.check()

    monkeypatch.setattr(molt_diff, "_resolve_diff_python_command", resolve)
    monkeypatch.setattr(
        molt_diff,
        "_ensure_diff_run_lock",
        lambda: pytest.fail("expired preflight acquired run lock"),
    )
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        oracle_version="3.13",
        deadline_s=5,
        jobs=1,
        failures_output=tmp_path / "failures.txt",
    )
    assert observed == [5]
    assert suite.calls == []
    assert summary["not_run"] == 5
    assert summary["config"]["preflight_timed_out"] is True
    assert summary["config"]["compiler_target_python"] is None
    assert {row["reason"] for row in summary["item_results"]} == {"deadline"}
