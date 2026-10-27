"""Admission, result custody, and worker-state regressions without compiler builds."""

from __future__ import annotations

from molt.target_python import _parse_target_python_version

import concurrent.futures
import gc
import json
import os
import sys
import signal
import threading
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


def _footprint(root: Path) -> dict[str, int]:
    """Each directory under ``root``, at any depth, with its count of files.

    One guarded launch writes records whose names carry its pid and a nonce,
    so two launches match by shape, not by name; an extra file anywhere,
    the guard's own directories included, changes a count.
    """
    if not root.exists():
        return {}
    shape: dict[str, int] = {".": 0}
    for path in root.rglob("*"):
        relative = path.relative_to(root)
        if path.is_dir():
            shape.setdefault(relative.as_posix(), 0)
        else:
            parent = relative.parent.as_posix()
            shape[parent] = shape.get(parent, 0) + 1
    return shape


@pytest.mark.parametrize(
    "inherited,compiler_profile,build_profile,expected",
    [
        (None, None, "dev", None),
        (None, None, "release", None),
        ("release", None, "dev", "release"),
        ("dev", None, "release", "dev"),
        ("release", "dev", "release", "dev"),
        ("dev", "release", "dev", "release"),
        ("release", "debug", "dev", "release"),
    ],
)
def test_cli_compiler_profile_is_explicit_and_independent_of_build_profile(
    tmp_path, inherited, compiler_profile, build_profile, expected
):
    artifact_root = tmp_path / "artifacts"
    env = {
        **os.environ,
        "MOLT_EXT_ROOT": str(artifact_root),
        # molt_diff's own outputs live here, beside the guard's state.
        "MOLT_DIFF_ROOT": str(artifact_root / "diff"),
        "MOLT_TARGET_ROOT": str(tmp_path / "tools"),
        "MOLT_USE_SCCACHE": "0",
        "MOLT_DIFF_SUMMARY": str(tmp_path / "old-summary.json"),
        "MOLT_DIFF_BUILD_PROFILE": "release",
    }
    if inherited is None:
        env.pop("MOLT_BACKEND_PROFILE", None)
    else:
        env["MOLT_BACKEND_PROFILE"] = inherited
    command = [
        sys.executable,
        "-B",
        "-c",
        "import atexit,json,os,runpy,sys\n"
        "atexit.register(lambda: print(json.dumps({\n"
        "    'compiler': os.environ.get('MOLT_BACKEND_PROFILE'),\n"
        "    'guest': os.environ.get('MOLT_DIFF_BUILD_PROFILE'),\n"
        "})))\n"
        "sys.argv = sys.argv[1:]\n"
        "runpy.run_path(sys.argv[0], run_name='__main__')\n",
        str(_ROOT / "tests/molt_diff.py"),
        "--build-profile",
        build_profile,
        "--evidence-dir",
        "profile-receiver",
        str(_ROOT / "examples/hello.py"),
    ]
    if compiler_profile is not None:
        command.extend(["--compiler-profile", compiler_profile])

    result = run_guarded_test_process(
        command, env=env, capture_output=True, text=True, timeout=15
    )

    assert result.returncode == 2, result.stdout + result.stderr
    assert json.loads(result.stdout) == {"compiler": expected, "guest": "release"}
    if compiler_profile == "debug":
        assert "argument --compiler-profile: invalid choice" in result.stderr
        assert "conflicts with explicit artifact paths" not in result.stderr
        # Only the guarded launch itself left state under the artifact root:
        # a guarded no-op with the same environment, in its own artifact
        # root, leaves the same shape.
        baseline_root = tmp_path / "baseline-artifacts"
        baseline = run_guarded_test_process(
            [sys.executable, "-B", "-c", "pass"],
            env={
                **env,
                "MOLT_EXT_ROOT": str(baseline_root),
                "MOLT_DIFF_ROOT": str(baseline_root / "diff"),
            },
            capture_output=True,
            timeout=15,
        )
        assert baseline.returncode == 0
        assert _footprint(artifact_root) == _footprint(baseline_root)
        assert not (tmp_path / "tools").exists()
    else:
        assert "conflicts with explicit artifact paths" in result.stderr
    assert not (tmp_path / "old-summary.json").exists()


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
    worker_context = molt_diff.compiler_process.worker_context()
    raw_value = worker_context.RawValue

    def shared_stop(*args):
        state.shared_stop = raw_value(*args)
        return state.shared_stop

    monkeypatch.setattr(worker_context, "RawValue", shared_stop)
    monkeypatch.setattr(
        molt_diff.compiler_process, "worker_context", lambda: worker_context
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

    def suite_sentinel(context, *, label, **_kwargs):
        # The scheduler owns only this sentinel. Keep the real context factory
        # and guarded run method available to fresh interpreter admission.
        assert isinstance(
            context, molt_diff.harness_memory_guard.HarnessExecutionContext
        )
        assert context.prefix == "MOLT_DIFF" and label == "molt_diff_suite"
        return getattr(state, "sentinel", None)

    monkeypatch.setattr(
        molt_diff.harness_memory_guard.HarnessExecutionContext,
        "start_repo_sentinel",
        suite_sentinel,
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
            assert kwargs["mp_context"] is worker_context
            assert kwargs["initargs"] in (
                (state.shared_stop,),
                (state.shared_stop, True),
            )
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
    assert json.loads(Path(writes[0]).read_text(encoding="utf-8")) == json.loads(
        json.dumps(summary)
    )
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
def test_parallel_diagnostics_publish_at_completion_without_retaining_history(
    suite, tmp_path, monkeypatch, capsys, live
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
                "duration_s": 0.25 + index,
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
    failures = tmp_path / "failures.txt"
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=2,
        targets=("native", "wasm"),
        live=live,
        log_file=status_log,
        failures_output=failures,
    )
    assert summary["discovered"] == len(statuses)
    assert summary["failed"] == 3 and summary["oom"] == 1
    assert summary["passed"] == 1 and summary["skipped"] == 1
    assert suite.calls == []
    assert [row["status"] for row in summary["item_results"]] == statuses
    assert [row["duration_s"] for row in summary["item_results"]] == [
        0.25 + index for index in range(len(statuses))
    ]
    expected_failed = [str(path) for path in suite.files[:3]]
    assert summary["failed_files"] == expected_failed
    assert failures.read_text(encoding="utf-8").splitlines() == expected_failed
    assert all(reference() is None for reference in retained)
    assert "detail-out" not in status_log.read_text(encoding="utf-8")
    captured = capsys.readouterr()
    assert "detail-out" not in captured.out and "detail-error" not in captured.err


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
def test_serial_tee_and_parallel_failure_diagnostics_are_not_replayed(
    suite, tmp_path, monkeypatch, capsys, jobs
):
    suite.files[:] = suite.files[:2]
    suite.status = "oom"
    serial_calls = []

    def diagnostic_case(path, _python, **kwargs):
        serial_calls.append(path)
        print("serial-out")
        print("serial-error", file=sys.stderr)
        return {
            "resolved_status": "oom" if path == str(suite.files[0]) else "pass",
            "guard_signal": None,
        }

    monkeypatch.setattr(molt_diff, "diff_test", diagnostic_case)
    monkeypatch.setattr(molt_diff, "_diff_run_single", suite.original_run_single)
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=jobs,
        failures_output=tmp_path / "failures.txt",
    )
    captured = capsys.readouterr()
    assert summary["passed"] == 1 and summary["failed"] == 1
    assert summary["oom"] == 1
    assert [row["status"] for row in summary["item_results"]] == ["oom", "pass"]
    expected = 2 if jobs == 1 else 0
    assert serial_calls == ([str(path) for path in suite.files] if jobs == 1 else [])
    assert captured.out.count("serial-out\n") == expected
    assert captured.err.count("serial-error\n") == expected
    if jobs == 2:
        assert suite.calls == [str(path) for path in suite.files]
        assert captured.out.count("worker output") == 1


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


@pytest.mark.parametrize("cutoff_phase", ("first-completion", "drained", "never"))
def test_oom_completions_remain_terminal_when_the_admission_deadline_expires(
    suite, tmp_path, monkeypatch, capsys, cutoff_phase
):
    clock = SimpleNamespace(now=0.0)
    monkeypatch.setattr(molt_diff, "_PROCESS_STARTED", 0.0)
    # The fake clock reaches only the modules that own differential deadlines.
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
    original_wait = molt_diff.concurrent.futures.wait
    completions = []

    def complete(pending, **kwargs):
        done, remaining = original_wait(pending, **kwargs)
        for future in done:
            result = future.result()
            index = suite.files.index(Path(result["path"]))
            result.update(status="oom", duration_s=0.25 + index)
            completions.append(result["path"])
        # Two workers have already been admitted at the first completion.
        # Expiry prevents replenishment but cannot erase either running result.
        if cutoff_phase == "first-completion" or (
            cutoff_phase == "drained" and len(completions) == len(suite.files)
        ):
            clock.now = 100.0
        return done, remaining

    monkeypatch.setattr(molt_diff.concurrent.futures, "wait", complete)
    monkeypatch.setattr(
        molt_diff,
        "_diff_run_single",
        lambda *a, **k: pytest.fail(
            "a terminal parallel outcome started a serial attempt"
        ),
    )
    failures = tmp_path / "failures.txt"
    summary = molt_diff.run_diff(
        suite.files,
        sys.executable,
        jobs=2,
        deadline_s=100,
        failures_output=failures,
    )
    expected_count = 2 if cutoff_phase == "first-completion" else len(suite.files)
    expected_paths = [str(path) for path in suite.files[:expected_count]]
    assert suite.calls == completions == expected_paths
    assert summary["discovered"] == expected_count
    assert summary["passed"] == 0
    assert summary["failed"] == summary["oom"] == expected_count
    assert summary["failed_files"] == expected_paths
    assert failures.read_text(encoding="utf-8").splitlines() == expected_paths
    started = _started(summary)
    assert [row["status"] for row in started] == ["oom"] * expected_count
    assert [row["duration_s"] for row in started] == [
        0.25 + index for index in range(expected_count)
    ]
    assert len(summary["item_results"]) == len(suite.files)
    assert summary["not_run"] == len(suite.files) - expected_count
    assert {
        row["reason"] for row in summary["item_results"] if row["status"] == "not_run"
    } == ({"deadline"} if cutoff_phase == "first-completion" else set())
    assert capsys.readouterr().out.count("worker output") == expected_count


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
    # Observe only the suite's callback. Fresh guarded metadata children retain
    # their real process-wide atexit custody through ProcessStateUse.
    install_module_view(
        monkeypatch,
        "atexit",
        molt_diff.atexit,
        molt_diff,
        register=suite.exit_callbacks.append,
        unregister=suite.exit_callbacks.remove,
    )
    return suite


def test_suite_guard_covers_oom_drain_failure_receipt_and_summary(
    guarded_suite, tmp_path, monkeypatch
):
    guarded_suite.status = "oom"
    failures = tmp_path / "failures.txt"
    summary_emissions = []
    completions = []
    original_parallel = molt_diff._parallel_diff_results

    def drain(*args, **kwargs):
        for result in original_parallel(*args, **kwargs):
            assert guarded_suite.exits == []
            assert len(guarded_suite.exit_callbacks) == 1
            completions.append((result["path"], result["status"]))
            yield result

    def emit_summary(summary, *args, **kwargs):
        assert guarded_suite.exits == []
        assert len(guarded_suite.exit_callbacks) == 1
        assert failures.read_text(encoding="utf-8").splitlines() == [
            str(guarded_suite.files[0])
        ]
        summary_emissions.append(summary)

    monkeypatch.setattr(molt_diff, "_parallel_diff_results", drain)
    monkeypatch.setattr(
        molt_diff,
        "_diff_run_single",
        lambda *a, **k: pytest.fail("a drained OOM started a serial attempt"),
    )
    monkeypatch.setattr(molt_diff, "_emit_json", emit_summary)
    summary = molt_diff.run_diff(
        guarded_suite.files,
        sys.executable,
        jobs=2,
        failures_output=failures,
    )

    assert completions == [
        (str(path), "oom" if index == 0 else "pass")
        for index, path in enumerate(guarded_suite.files)
    ]
    assert summary["failed"] == summary["oom"] == 1
    assert summary["passed"] == len(guarded_suite.files) - 1
    assert summary_emissions == [summary]
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


def test_operator_signal_after_oom_drains_admitted_results_without_replacing_failures(
    guarded_suite, tmp_path, monkeypatch, capsys
):
    original_wait = molt_diff.concurrent.futures.wait
    previous_handler = signal.getsignal(signal.SIGTERM)
    completions = []

    def complete(pending, **kwargs):
        assert guarded_suite.exits == []
        assert len(guarded_suite.exit_callbacks) == 1
        done, remaining = original_wait(pending, **kwargs)
        for future in done:
            result = future.result()
            index = guarded_suite.files.index(Path(result["path"]))
            result.update(status="oom", duration_s=0.25 + index)
            completions.append(result["path"])
        # The first OOM is already retained and a third case has been admitted.
        # The signal stops replenishment while all three receipts still drain.
        if len(completions) == 2:
            signal.raise_signal(signal.SIGTERM)
        return done, remaining

    monkeypatch.setattr(molt_diff.concurrent.futures, "wait", complete)
    monkeypatch.setattr(
        molt_diff,
        "_diff_run_single",
        lambda *a, **k: pytest.fail("an interrupted cohort started a serial attempt"),
    )
    failures = tmp_path / "failures.txt"
    summary = molt_diff.run_diff(
        guarded_suite.files,
        sys.executable,
        jobs=2,
        failures_output=failures,
    )
    expected_paths = [str(path) for path in guarded_suite.files[:3]]
    assert guarded_suite.calls == completions == expected_paths
    assert summary["guard_signal"] == signal.SIGTERM
    assert summary["oom"] == 3
    assert summary["failed"] == 4 and summary["passed"] == 0
    assert summary["failed_files"] == [*expected_paths, "<operator_signal>"]
    assert failures.read_text(encoding="utf-8").splitlines() == summary["failed_files"]
    assert [row["status"] for row in _started(summary)] == ["oom"] * 3
    assert [row["duration_s"] for row in _started(summary)] == [0.25, 1.25, 2.25]
    assert len(summary["item_results"]) == len(guarded_suite.files)
    assert summary["not_run"] == len(guarded_suite.files) - 3
    assert len(guarded_suite.exits) == 1
    assert guarded_suite.exit_callbacks == []
    assert signal.getsignal(signal.SIGTERM) is previous_handler
    assert molt_diff.compat_backends.suite_stop_signal() is None
    assert capsys.readouterr().out.count("worker output") == 3


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


@pytest.mark.parametrize("backend", ["native", "llvm"])
def test_first_native_image_spends_only_its_existing_attempt_budget(
    monkeypatch, backend
):
    backends = molt_diff.compat_backends
    clock = SimpleNamespace(now=100.0)
    monkeypatch.setattr(backends, "time", SimpleNamespace(monotonic=lambda: clock.now))
    policy = backends.TimeoutPolicy(
        10.0, ((backend, 30.0, 10.0), ("wasm", 600.0, 60.0))
    )
    whole = backends.ItemBudget.start(policy, (backend, "wasm"))
    assert whole.deadline == 810.0
    attempt = whole.for_backend(backend)
    assert attempt.deadline == 140.0
    assert policy.reservation((backend,), cpython=False) == 40.0
    clock.now = 125.0  # Availability and compilation already spent 25 seconds.
    assert attempt.limit(policy.phase(backend, "first_run")) == 15.0
    assert attempt.limit(policy.phase(backend, "run")) == 10.0
    assert attempt.for_backend(backend) is attempt
    assert whole.deadline == 810.0
    assert backends.ItemBudget(policy, 130.0).for_backend(backend).deadline == 130.0
    assert whole.for_backend(backend, build_only=True).deadline == 155.0
    clock.now = 140.0
    assert attempt.limit(policy.phase(backend, "first_run")) == 0.0


@pytest.mark.parametrize(
    "build,run,first_run",
    [(None, 10.0, 10.0), (30.0, None, None), (None, None, None), (1e308, 1e308, 1e308)],
)
def test_first_native_image_preserves_unbounded_or_overflowing_phase_policy(
    monkeypatch, build, run, first_run
):
    backends = molt_diff.compat_backends
    monkeypatch.setattr(backends, "time", SimpleNamespace(monotonic=lambda: 100.0))
    policy = backends.TimeoutPolicy(10.0, (("native", build, run),))
    parent = backends.ItemBudget(policy, 107.0)
    attempt = parent.for_backend("native")
    assert attempt is parent
    assert policy.phase("native", "first_run") == first_run
    assert attempt.limit(policy.phase("native", "first_run")) == 7.0
    assert policy.phase("native", "run") == run


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


def test_a_test_import_path_reaches_the_native_build_under_an_execution_context(
    tmp_path, monkeypatch
):
    """MOLT_ENV PYTHONPATH must survive into the Molt build, as for CPython."""
    case = tmp_path / "case.py"
    case.write_text(
        "# MOLT_ENV: PYTHONPATH=src:tests/differential/basic/ns_part_a\nprint(1)\n",
        encoding="utf-8",
    )
    plain = tmp_path / "plain.py"
    plain.write_text("print(1)\n", encoding="utf-8")
    monkeypatch.setenv("PYTHONPATH", "host-path")
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))
    budget = molt_diff.compat_backends.ItemBudget(
        molt_diff.compat_backends.TimeoutPolicy.from_environment({}), None
    )
    root = Path(molt_diff.__file__).resolve().parents[1]
    expected = os.pathsep.join(
        str(root / part) for part in ("src", "tests/differential/basic/ns_part_a")
    )
    context = molt_diff._backend_execution_context(
        str(case), sys.executable, "dev", None, budget=budget
    )
    assert context.environment["PYTHONPATH"] == expected
    # Without MOLT_ENV the build sees the oracle's `src`, never the host path.
    assert (
        molt_diff._backend_execution_context(
            str(plain), sys.executable, "dev", None, budget=budget
        ).environment["PYTHONPATH"]
        == "src"
    )

    class BuildStarted(Exception):
        pass

    seen = {}

    def build(command, *, env, timeout, time_path):
        seen.update(env)
        raise BuildStarted

    monkeypatch.setattr(molt_diff, "_diff_batch_compile_server_enabled", lambda: False)
    monkeypatch.setattr(molt_diff, "_run_with_optional_time", build)
    with pytest.raises(BuildStarted):
        molt_diff._run_molt_owned(
            str(case),
            build_only=True,
            build_profile="dev",
            daemon_enabled=False,
            no_cache=False,
            rebuild=False,
            extra_env=None,
            execution_context=context,
            output_root=tmp_path / "out",
            environment=context.environment,
            budget=budget,
        )
    assert seen["PYTHONPATH"] == expected


def _diff_worker_stop_observation():
    import multiprocessing

    parent = multiprocessing.parent_process()
    assert parent is not None
    return (
        molt_diff.compat_backends.suite_stop_signal(),
        parent.pid,
        parent.is_alive(),
    )


def test_parallel_worker_state_uses_fresh_context_and_live_shared_stop():
    import textwrap

    probe = textwrap.dedent(
        """
        import json, os, signal
        from concurrent.futures import ProcessPoolExecutor
        from molt import compiler_process
        from tests.test_molt_diff_scheduling import (
            _diff_worker_stop_observation, molt_diff,
        )

        if __name__ == "__main__":
            context = compiler_process.worker_context()
            stop = context.RawValue("i", 0)
            with ProcessPoolExecutor(
                max_workers=1,
                mp_context=context,
                initializer=molt_diff._install_worker_state,
                initargs=(stop,),
            ) as executor:
                before = executor.submit(_diff_worker_stop_observation).result()
                stop.value = signal.SIGTERM
                after = executor.submit(_diff_worker_stop_observation).result()
            print(json.dumps([before, after, os.getpid(), int(signal.SIGTERM)]))
        """
    )
    result = run_guarded_test_process([sys.executable, "-c", probe], timeout=120)
    assert result.returncode == 0, result.stderr
    before, after, parent_pid, stop_signal = json.loads(result.stdout)
    assert before == [None, parent_pid, True]
    assert after == [stop_signal, parent_pid, True]


def test_pruning_a_stale_build_lock_takes_its_holder_record_along(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli.build_locks import lock_holder_path

    monkeypatch.setattr(molt_diff, "_diff_build_lock_root", lambda: tmp_path)
    monkeypatch.setenv("MOLT_DIFF_BUILD_LOCK_MAX_AGE_SEC", "60")
    stale, fresh = tmp_path / "stale.lock", tmp_path / "fresh.lock"
    for lock in (stale, fresh):
        lock.write_bytes(b"")
        lock_holder_path(lock).write_bytes(b"")
    old = time.time() - 3600
    os.utime(stale, (old, old))
    molt_diff._prune_stale_build_locks()
    assert sorted(path.name for path in tmp_path.iterdir()) == [
        "fresh.lock",
        "fresh.lock.holder",
    ]


def _install_held_backend_probe(initializer, initargs, released, entered):
    """Use the real worker/capture/comparison path with build-free backends."""
    initializer(*initargs)
    backends = molt_diff.compat_backends
    version = _parse_target_python_version("3.12")
    molt_diff._resolve_python_command = lambda *a, **k: (sys.executable,)
    molt_diff._python_exe_version = lambda *a, **k: (3, 12)
    molt_diff._backend_execution_context = lambda *a, **k: (
        backends.BackendExecutionContext(
            molt_cli_python=sys.executable,
            target_python=version,
            build_profile="dev",
            capabilities="",
            environment=dict(os.environ),
            budget=k["budget"],
        )
    )
    molt_diff.run_cpython = lambda *a, **k: backends.BackendResult("oracle\n", "", 0)

    def backend(*, backend, file_path, context):
        if backend == "native":
            return backends.BackendResult(
                "X" * (256 * 1024) + "NATIVE_END\n", "native diagnostic-tail", 1
            )
        assert backend == "wasm"
        entered.set()
        assert released.wait(15), "test must release the admitted WASM backend"
        return backends.BackendResult("oracle\n", "", 0)

    molt_diff._run_backend_for_diff = backend


@pytest.mark.parametrize("live", [False, True])
def test_live_backend_failure_is_visible_while_later_backend_is_held(
    suite, tmp_path, monkeypatch, capfd, live
):
    from concurrent.futures._base import wait
    from concurrent.futures.process import ProcessPoolExecutor

    context = molt_diff.compiler_process.worker_context()
    released, entered = context.Event(), context.Event()
    for path in suite.files:
        path.write_text("print('oracle')\n", encoding="utf-8")
    records = tmp_path / "backend-results.jsonl"
    monkeypatch.setenv("MOLT_DIFF_RESULTS_JSONL", str(records))

    class Executor(ProcessPoolExecutor):
        def __init__(self, *, initializer, initargs, **kwargs):
            super().__init__(
                initializer=_install_held_backend_probe,
                initargs=(initializer, initargs, released, entered),
                **kwargs,
            )

    monkeypatch.setattr(molt_diff.concurrent.futures, "ProcessPoolExecutor", Executor)
    monkeypatch.setattr(molt_diff.concurrent.futures, "wait", wait)
    seen = []
    observed = SimpleNamespace(prompt=False, output="", error="")

    def observe():
        try:
            assert entered.wait(10), "a later backend must actually start"
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                captured = capfd.readouterr()
                observed.output += captured.out
                observed.error += captured.err
                previews = [
                    line
                    for line in observed.output.splitlines()
                    if line.startswith("[BACKEND FAIL]")
                ]
                if not live:
                    assert not previews, "non-live workers keep completion-only output"
                    observed.prompt = True
                    return
                if len(previews) == 2:
                    assert not released.is_set()
                    assert all(
                        "(native)" in line and "rc=1" in line for line in previews
                    )
                    assert all("native diagnostic-tail" in line for line in previews)
                    assert all("final verdict pending" in line for line in previews)
                    assert len(observed.output) < 4096, (
                        "full output stays with the result"
                    )
                    observed.prompt = True
                    return
                time.sleep(0.01)
        except BaseException as error:
            seen.append(error)
        finally:
            released.set()

    observer = threading.Thread(target=observe)
    observer.start()
    aggregate = tmp_path / "aggregate.log"
    try:
        summary = molt_diff.run_diff(
            suite.files,
            sys.executable,
            jobs=2,
            targets=("native", "wasm"),
            live=live,
            fail_fast=True,
            log_aggregate=aggregate,
            failures_output=tmp_path / "failures.txt",
        )
    finally:
        released.set()
        observer.join(timeout=20)
    assert not observer.is_alive()
    assert not seen, seen
    assert observed.prompt, "known native failure was buffered behind WASM completion"
    captured = capfd.readouterr()
    output = observed.output + captured.out
    assert output.count("[BACKEND FAIL]") == (2 if live else 0)
    assert "Molt    stdout:" not in output
    assert summary["failed"] == 2 and summary["not_run"] == 3
    assert len(summary["item_results"]) == len(suite.files)
    detail = aggregate.read_text(encoding="utf-8")
    assert detail.count("NATIVE_END") == 2
    assert "[BACKEND FAIL]" not in detail
    rows = [
        json.loads(line) for line in records.read_text(encoding="utf-8").splitlines()
    ]
    backend_rows = [row for row in rows if row.get("record_type") == "backend"]
    assert sorted((row["backend"], row["raw_status"]) for row in backend_rows) == [
        ("native", "fail"),
        ("native", "fail"),
        ("wasm", "pass"),
        ("wasm", "pass"),
    ]


@pytest.mark.parametrize("error", [BrokenPipeError(), ValueError("closed stream")])
def test_optional_live_preview_failure_does_not_replace_captured_verdict(
    monkeypatch, capsys, error
):
    class FailedPreview:
        def write(self, _text):
            raise error

        def flush(self):
            raise AssertionError("failed write must not flush")

    monkeypatch.setattr(molt_diff, "_WORKER_DIAGNOSTIC_STDOUT", FailedPreview())
    outcome = molt_diff.compat_backends.BackendResult("wrong", "callback error", 1)
    molt_diff._emit_backend_fail(
        "case.py", "native", "oracle", "", 0, outcome, "mismatch"
    )
    captured = capsys.readouterr()
    assert "[FAIL] case.py (native) mismatch" in captured.out
    assert "Molt    return: 1" in captured.out
    assert "callback error" in captured.out


def test_live_preview_is_one_bounded_write_and_not_a_full_result_copy(monkeypatch):
    writes = []

    class Preview:
        def write(self, text):
            writes.append(text)

        def flush(self):
            pass

    monkeypatch.setattr(molt_diff, "_WORKER_DIAGNOSTIC_STDOUT", Preview())
    molt_diff._publish_backend_diagnostic(
        "case.py", "wasm", "uncalibrated", "x" * 100000
    )
    assert len(writes) == 1 and len(writes[0]) < 900
    assert writes[0].startswith("[BACKEND UNCALIBRATED]")
    assert writes[0].endswith("[matrix still running; final verdict pending]\n")
