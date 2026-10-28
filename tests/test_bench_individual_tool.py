from __future__ import annotations

import contextlib
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest

from tools import bench_individual as bench
from molt.backend_daemon_custody import BackendDaemonObservation
from molt.process_model import ProcessSample


ROOT = Path(__file__).resolve().parents[1]


@contextlib.contextmanager
def _admitted_workloads(scripts):
    from tools.bench_evidence import admit_benchmark_workload

    with contextlib.ExitStack() as custody:
        yield {
            script: custody.enter_context(
                admit_benchmark_workload(
                    Path(script),
                    project_root=bench.REPO_ROOT,
                    run_args=[],
                    molt_args=bench.molt_args_for_benchmark(script),
                )
            )
            for script in scripts
        }


def test_bench_individual_reuses_backend_daemon_by_default(
    tmp_path: Path, monkeypatch
) -> None:
    cleanups = 0

    def fake_cleanup(*, quiet: bool = False) -> None:
        nonlocal cleanups
        del quiet
        cleanups += 1

    monkeypatch.setattr(bench, "_ensure_clean_slate", fake_cleanup)
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None, workload=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.02, None, "ok", 0, stdout="1", stderr=""
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.04, None, "ok", 0, stdout="1", stderr=""
        ),
    )

    with _admitted_workloads(["tests/benchmarks/bench_bytes_find.py"]) as workloads:
        result = bench.bench_one(
            "tests/benchmarks/bench_bytes_find.py",
            samples=1,
            warmup=1,
            timeout_build=1.0,
            timeout_run=1.0,
            workload=workloads["tests/benchmarks/bench_bytes_find.py"],
        )

    assert cleanups == 0
    from tools import bench_diff

    assert result["workload"]["entry"] == "tests/benchmarks/bench_bytes_find.py"
    cells = {"tests/benchmarks/bench_bytes_find.py": result}
    external = bench_diff._compute_metric_diffs("cpython_time_s", cells, cells)
    assert len(external) == 1 and external[0].delta == 0
    assert result["build_ok"] is True
    assert result["run_ok"] is True
    assert result["molt_ok"] is True
    assert result["molt_warmup_samples_s"] == [0.02]
    assert result["molt_samples_s"] == [0.02]
    assert result["cpython_warmup_samples_s"] == [0.04]
    assert result["cpython_samples_s"] == [0.04]


def test_bench_individual_can_opt_into_cold_daemon_isolation(
    tmp_path: Path, monkeypatch
) -> None:
    cleanups = 0

    def fake_cleanup(*, quiet: bool = False) -> None:
        nonlocal cleanups
        del quiet
        cleanups += 1

    monkeypatch.setattr(bench, "_ensure_clean_slate", fake_cleanup)
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None, workload=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.02, None, "ok", 0, stdout="1", stderr=""
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.04, None, "ok", 0, stdout="1", stderr=""
        ),
    )

    with _admitted_workloads(["tests/benchmarks/bench_bytes_find.py"]) as workloads:
        bench.bench_one(
            "tests/benchmarks/bench_bytes_find.py",
            samples=1,
            warmup=1,
            timeout_build=1.0,
            timeout_run=1.0,
            isolate_daemon=True,
            workload=workloads["tests/benchmarks/bench_bytes_find.py"],
        )

    assert cleanups == 1


@pytest.fixture
def daemon_control_environment(tmp_path: Path, monkeypatch) -> None:
    for name in (
        "MOLT_BUILD_STATE_DIR",
        "MOLT_DIFF_GUEST_OUTPUT_ROOT",
        "MOLT_DIFF_GUEST_OUTPUT_IDENTITY",
    ):
        monkeypatch.delenv(name, raising=False)
    for name, value in {
        "MOLT_SESSION_ID": "alpha-session",
        "CARGO_TARGET_DIR": str(tmp_path / "target"),
        "MOLT_EXT_ROOT": str(tmp_path / "canonical"),
    }.items():
        monkeypatch.setenv(name, value)


def test_bench_individual_isolate_daemon_preserves_foreign_sessions(
    tmp_path: Path, monkeypatch, daemon_control_environment
) -> None:
    environment = bench.harness_memory_guard.canonical_harness_env(
        os.environ, repo_root=ROOT
    )
    daemon_root = bench.daemon_custody.backend_daemon_root_from_env(
        environment, project_root=ROOT
    )
    assert daemon_root.is_relative_to(tmp_path / "canonical" / "tmp" / "build-control")
    daemon_root.mkdir(parents=True)
    owned_socket = tmp_path / "owned.sock"
    foreign_socket = tmp_path / "foreign.sock"
    owned_socket.write_text("", encoding="utf-8")
    foreign_socket.write_text("", encoding="utf-8")

    def write_identity(name: str, *, pid: int, socket_path: Path) -> None:
        (daemon_root / name).write_text(
            json.dumps(
                {
                    "schema": bench.daemon_custody.IDENTITY_SCHEMA,
                    "pid": pid,
                    "socket_path": str(socket_path),
                    "project_root": str(ROOT),
                    "cargo_profile": "dev-fast",
                    "config_digest": None,
                    "backend_bin": "/repo/target/molt-backend",
                    "created_at": 1_700_000_000.0,
                    "command": None,
                    "started_at_ns": pid * 10,
                    "backend_sha256": "a" * 64,
                    "suite_lease": None,
                },
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )

    write_identity(
        "molt-backend.dev-fast.alpha-session.aaaa.identity.json",
        pid=101,
        socket_path=owned_socket,
    )
    write_identity(
        "molt-backend.dev-fast.beta-session.bbbb.identity.json",
        pid=202,
        socket_path=foreign_socket,
    )
    killed: list[int] = []

    def fake_processes():
        # Session custody is platform-neutral; enumeration is the OS boundary.
        return [
            BackendDaemonObservation(
                sample=ProcessSample(
                    pid=pid,
                    ppid=1,
                    rss_kb=0,
                    command=f"/repo/target/molt-backend --daemon --socket {socket}",
                    elapsed_sec=elapsed,
                ),
                socket_path=socket,
            )
            for pid, elapsed, socket in [
                (101, 120, owned_socket),
                (202, 240, foreign_socket),
            ]
        ]

    monkeypatch.setattr(bench, "BENCH_TMP_ROOT", tmp_path / "bench-tmp")
    monkeypatch.setattr(bench, "_list_backend_daemon_processes", fake_processes)

    def fake_terminate(identity, *, grace: float = 0.75, health_probe=None) -> bool:
        del grace, health_probe
        killed.append(identity.pid)
        return True

    monkeypatch.setattr(
        bench.daemon_custody,
        "terminate_backend_daemon_identity",
        fake_terminate,
    )

    report = bench._cleanup_current_session_backend_daemons()

    assert killed == [101]
    assert report.killed_count == 1
    assert report.killed[0].pid == 101
    assert report.killed[0].reason == "session_identity_verified"
    assert report.skipped_foreign == 1
    assert report.session_id == "alpha-session"
    assert report.killed_at is not None
    assert report.artifact is not None
    payload = json.loads(Path(report.artifact).read_text(encoding="utf-8"))
    assert payload["event"] == "bench_individual_backend_daemon_cleanup"
    assert payload["killed"][0]["pid"] == 101
    assert payload["skipped_foreign"] == 1


def test_bench_individual_isolate_daemon_requires_identity_not_socket_env(
    tmp_path: Path, monkeypatch, daemon_control_environment
) -> None:
    environment = bench.harness_memory_guard.canonical_harness_env(
        os.environ, repo_root=ROOT
    )
    daemon_root = bench.daemon_custody.backend_daemon_root_from_env(
        environment, project_root=ROOT
    )
    assert daemon_root.is_relative_to(tmp_path / "canonical" / "tmp" / "build-control")
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "loose.sock"
    socket_path.write_text("", encoding="utf-8")
    killed: list[int] = []

    def fake_processes():
        return [
            BackendDaemonObservation(
                sample=ProcessSample(
                    pid=404,
                    ppid=1,
                    rss_kb=0,
                    command=f"/repo/target/molt-backend --daemon --socket {socket_path}",
                    elapsed_sec=120,
                ),
                socket_path=socket_path,
            )
        ]

    def fake_terminate(identity, *, grace: float = 0.75, health_probe=None) -> bool:
        del grace, health_probe
        killed.append(identity.pid)
        return True

    monkeypatch.setenv("MOLT_BACKEND_DAEMON_SOCKET", str(socket_path))
    monkeypatch.setattr(bench, "BENCH_TMP_ROOT", tmp_path / "bench-tmp")
    monkeypatch.setattr(bench, "_list_backend_daemon_processes", fake_processes)
    monkeypatch.setattr(
        bench.daemon_custody,
        "terminate_backend_daemon_identity",
        fake_terminate,
    )

    report = bench._cleanup_current_session_backend_daemons()

    assert killed == []
    assert report.killed_count == 0
    assert report.skipped_foreign == 1
    assert report.artifact is None


def test_bench_individual_rejects_partial_sample_failure(
    tmp_path: Path, monkeypatch
) -> None:
    calls = 0

    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None, workload=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )

    def fake_run_binary(
        binary: Path, timeout_s: float, limits=None, workload=None
    ) -> bench.RunOutcome:
        del limits
        nonlocal calls
        calls += 1
        if calls == 3:
            return bench.RunOutcome(False, 0.03, None, "error", 1, stdout="", stderr="")
        return bench.RunOutcome(True, 0.02, None, "ok", 0, stdout="1", stderr="")

    monkeypatch.setattr(bench, "run_binary", fake_run_binary)
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.04, None, "ok", 0, stdout="1", stderr=""
        ),
    )

    with _admitted_workloads(["tests/benchmarks/bench_bytes_find.py"]) as workloads:
        result = bench.bench_one(
            "tests/benchmarks/bench_bytes_find.py",
            samples=2,
            warmup=1,
            timeout_build=1.0,
            timeout_run=1.0,
            workload=workloads["tests/benchmarks/bench_bytes_find.py"],
        )

    assert result["run_ok"] is False
    assert result["molt_ok"] is False
    assert result["molt_warmup_samples_s"] == [0.02]
    assert result["molt_samples_s"] == [0.02]
    assert result["error"] == "Molt run failed during sample 2/2: rc=1"


def test_bench_individual_records_molt_run_failure_detail(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None, workload=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None, workload=None: bench.RunOutcome(
            False, 0.02, None, "error", 7, stdout="", stderr="runtime intrinsic missing"
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.04, None, "ok", 0, stdout="1", stderr=""
        ),
    )

    with _admitted_workloads(["tests/benchmarks/bench_bytes_find.py"]) as workloads:
        result = bench.bench_one(
            "tests/benchmarks/bench_bytes_find.py",
            samples=1,
            warmup=0,
            timeout_build=1.0,
            timeout_run=1.0,
            workload=workloads["tests/benchmarks/bench_bytes_find.py"],
        )

    assert result["run_ok"] is False
    assert result["molt_failure_detail"] == "rc=7\nstderr:\nruntime intrinsic missing"
    assert "runtime intrinsic missing" in result["error"]
    assert result["molt_output"] == ""
    assert result["molt_stderr"] == "runtime intrinsic missing"
    assert result["molt_time_s"] is None
    assert result["molt_speedup"] is None


def test_bench_individual_marks_intrinsic_benchmarks_molt_only(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None, workload=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.02, None, "ok", 0, stdout="intrinsic-only", stderr=""
        ),
    )

    def fail_cpython(
        script: str, timeout_s: float, limits=None, workload=None
    ) -> bench.RunOutcome:
        del limits
        raise AssertionError("Molt-only intrinsic benchmarks must not run CPython")

    monkeypatch.setattr(bench, "run_cpython", fail_cpython)

    with _admitted_workloads(["tests/benchmarks/bench_ptr_registry.py"]) as workloads:
        result = bench.bench_one(
            "tests/benchmarks/bench_ptr_registry.py",
            samples=1,
            warmup=0,
            timeout_build=1.0,
            timeout_run=1.0,
            workload=workloads["tests/benchmarks/bench_ptr_registry.py"],
        )

    assert result["reference_runtime"] == "molt"
    assert (
        result["reference_reason"]
        == "molt_runtime_intrinsics_without_external_reference"
    )
    assert result["molt_ok"] is True
    assert result["cpython_samples_s"] is None
    assert result["cpython_time_s"] is None
    assert result["output_match"] is None


def test_bench_individual_custom_same_basename_keeps_cpython_reference(
    tmp_path: Path, monkeypatch
) -> None:
    custom = tmp_path / "bench_ptr_registry.py"
    custom.write_text("print('custom')\n", encoding="utf-8")

    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None, workload=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.02, None, "ok", 0, stdout="custom", stderr=""
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None, workload=None: bench.RunOutcome(
            True, 0.03, None, "ok", 0, stdout="custom", stderr=""
        ),
    )

    with _admitted_workloads([str(custom)]) as workloads:
        result = bench.bench_one(
            str(custom),
            samples=1,
            warmup=0,
            timeout_build=1.0,
            timeout_run=1.0,
            workload=workloads[str(custom)],
        )

    assert result["reference_runtime"] == "cpython"
    assert result["reference_reason"] == "cpython_reference"
    assert result["cpython_time_s"] == 0.03
    assert result["output_match"] is True


def test_bench_individual_process_helpers_use_molt_bench_guard(
    tmp_path: Path, monkeypatch, daemon_control_environment
) -> None:
    binary = tmp_path / "bench_molt"
    binary.write_text("", encoding="utf-8")
    script = tmp_path / "bench.py"
    script.write_text("print('ok')\n", encoding="utf-8")
    limits = object()
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd: list[str], **kwargs: object):
        calls.append({"cmd": list(cmd), **kwargs})
        if "build" in cmd:
            payload = {"status": "ok", "data": {"output": str(binary)}}
            stdout = json.dumps(payload)
        else:
            stdout = "ok\n"
        return bench.harness_memory_guard.GuardedCompletedProcess(
            cmd,
            0,
            stdout,
            "",
            elapsed_s=9.0,
            child_elapsed_s=0.0125,
            child_returncode=0,
            child_stderr="",
            descendants_closed=True,
            peak=bench.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            ),
        )

    monkeypatch.setattr(
        bench.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    monkeypatch.setenv("MOLT_SESSION_ID", "caller-session")

    with _admitted_workloads([str(script)]) as workloads:
        workload = workloads[str(script)]
        built_binary, build_s, build_err = bench.molt_build(
            str(script),
            tmp_path,
            3.0,
            limits=limits,
            workload=workload,
        )
        run = bench.run_binary(binary, 4.0, limits=limits, workload=workload)
        cpy = bench.run_cpython(str(script), 5.0, limits=limits, workload=workload)

    assert built_binary == binary
    assert build_err == ""
    assert build_s >= 0
    assert run.ok and run.elapsed_s == 0.0125 and run.stdout == "ok\n"
    assert cpy.ok and cpy.elapsed_s == 0.0125 and cpy.stdout == "ok\n"
    assert [call["prefix"] for call in calls] == [
        "MOLT_BENCH",
        "MOLT_BENCH",
        "MOLT_BENCH",
    ]
    assert [call["limits"] for call in calls] == [limits, limits, limits]
    assert calls[0]["timeout"] == 3.0
    assert calls[0]["cmd"][:4] == [sys.executable, "-m", "molt.cli", "build"]
    assert calls[0]["cwd"] == bench.REPO_ROOT
    assert calls[0]["env"]["MOLT_EXT_ROOT"] == str(tmp_path / "canonical")
    assert calls[0]["env"]["CARGO_TARGET_DIR"] == str(tmp_path / "target")
    assert calls[0]["env"]["MOLT_SESSION_ID"] == "caller-session"
    assert calls[0]["env"]["PYTHONPATH"] == str(bench.REPO_ROOT / "src")
    assert calls[1]["timeout"] == 4.0
    assert calls[2]["timeout"] == 5.0
    assert calls[2]["cmd"] == [sys.executable, str(script)]


def test_bench_individual_main_uses_suite_sentinel_and_shared_limits(
    tmp_path: Path, monkeypatch
) -> None:
    limits = object()
    bench_calls: list[dict[str, object]] = []
    sentinel_calls: list[dict[str, object]] = []
    summary_calls: list[object] = []

    monkeypatch.setattr(
        bench,
        "parse_args",
        lambda: SimpleNamespace(
            samples=1,
            warmup=0,
            timeout_build=1.0,
            timeout_run=1.0,
            isolate_daemon=False,
            bench=None,
            applications=False,
            skip=None,
            json_out=str(tmp_path / "result.json"),
        ),
    )
    monkeypatch.setattr(
        bench.harness_memory_guard,
        "limits_from_env",
        lambda prefix: limits if prefix == "MOLT_BENCH" else None,
    )

    @contextlib.contextmanager
    def fake_repo_process_sentinel(**kwargs: object):
        sentinel_calls.append(kwargs)
        yield

    def fake_bench_one(script: str, **kwargs: object) -> dict[str, object]:
        bench_calls.append({"script": script, **kwargs})
        return {
            "workload": kwargs["workload"].record,
            "build_ok": True,
            "run_ok": True,
            "molt_time_s": 0.01,
            "cpython_time_s": 0.02,
            "speedup": 2.0,
        }

    monkeypatch.setattr(
        bench.harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(bench, "BENCHMARKS", ["tests/benchmarks/bench_bytes_find.py"])
    monkeypatch.setattr(bench, "bench_one", fake_bench_one)
    monkeypatch.setattr(bench, "print_summary", lambda results: None)
    monkeypatch.setattr(
        bench.harness_memory_guard,
        "limits_summary",
        lambda got: summary_calls.append(got) or {"enabled": True},
    )
    monkeypatch.setattr(bench, "_git_rev", lambda: "rev")

    bench.main()

    assert len(sentinel_calls) == 1
    assert sentinel_calls[0]["label"] == "bench_individual"
    assert sentinel_calls[0]["limits"] is limits
    assert summary_calls == [limits]
    assert bench_calls[0]["limits"] is limits
    assert bench_calls[0]["samples"] == 1
    assert bench_calls[0]["warmup"] == 0


def test_individual_executes_admitted_guest_arguments(tmp_path, monkeypatch):
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "program.py"
    script.write_text("print(1)\n", encoding="utf-8")
    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs))
        return bench.harness_memory_guard.GuardedCompletedProcess(
            command,
            0,
            "1\n",
            "",
            elapsed_s=0.1,
            child_elapsed_s=0.1,
            child_returncode=0,
            child_stderr="",
            descendants_closed=True,
            peak=bench.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            ),
        )

    monkeypatch.setattr(bench.harness_memory_guard, "guarded_completed_process", run)
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=["--count", "7"], molt_args=[]
    ) as admitted:
        assert bench.run_cpython(str(script), 1, workload=admitted).stdout == "1\n"
        assert (
            bench.run_binary(tmp_path / "binary", 1, workload=admitted).stdout == "1\n"
        )
    assert calls[0][0] == [sys.executable, str(script), "--count", "7"]
    assert calls[1][0] == [str(tmp_path / "binary"), "--count", "7"]
    assert all(options["encoding"] == "utf-8" for _, options in calls)
    assert all(options["errors"] == "surrogateescape" for _, options in calls)


@pytest.mark.parametrize("descriptor", [1, 2])
def test_individual_actual_guard_preserves_distinct_invalid_output_bytes(
    tmp_path, daemon_control_environment, descriptor
):
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "raw_output.py"
    # The reference receives the admitted argument after its script. Using the
    # real Python executable as the guest capture fixture consumes that argument
    # as its script instead; both observations cross the actual guard boundary.
    script.write_text(
        "import os, sys\n"
        "payload = b'raw\\xff\\r\\nno-final-newline' if len(sys.argv) > 1 "
        "else b'raw\\xfe\\r\\nno-final-newline'\n"
        f"os.write({descriptor}, payload)\n",
        encoding="utf-8",
    )
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=[str(script)], molt_args=[]
    ) as workload:
        reference = bench.run_cpython(str(script), 15, workload=workload)
        guest = bench.run_binary(Path(sys.executable), 15, workload=workload)
    assert reference.ok and guest.ok
    for outcome in (reference, guest):
        assert outcome.guard_result is not None
        assert outcome.guard_result.descendants_closed is True
    reference_text = reference.stdout if descriptor == 1 else reference.stderr
    guest_text = guest.stdout if descriptor == 1 else guest.stderr
    assert reference_text is not None and guest_text is not None
    assert reference_text.encode("utf-8", "surrogateescape") == (
        b"raw\xff\r\nno-final-newline"
    )
    assert guest_text.encode("utf-8", "surrogateescape") == (
        b"raw\xfe\r\nno-final-newline"
    )
    assert not bench.compare_outputs(
        bench.Outputs(reference.stdout, reference.stderr, reference.exit_code),
        bench.Outputs(guest.stdout, guest.stderr, guest.exit_code),
        stderr_mode="exact",
    ).equal


@pytest.mark.parametrize("runner", ["reference", "guest"])
@pytest.mark.parametrize(
    "condition", ["ok", "unclosed", "clock", "rss", "timeout", "nonzero"]
)
def test_individual_requires_complete_child_observation(
    tmp_path, monkeypatch, daemon_control_environment, runner, condition
):
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "observation.py"
    script.write_text("print('ok')\n", encoding="utf-8")
    stdout = b"out\xff\r\n".decode("utf-8", "surrogateescape")
    stderr = b"err\xfe\r\n".decode("utf-8", "surrogateescape")
    code = 124 if condition == "timeout" else 7 if condition == "nonzero" else 0
    observed = bench.harness_memory_guard.GuardedCompletedProcess(
        ["fixture"],
        code,
        stdout,
        "guard diagnostics",
        elapsed_s=9.0,
        child_elapsed_s=None if condition == "clock" else 0.025,
        child_returncode=code,
        child_stderr=stderr,
        descendants_closed=condition != "unclosed",
        timed_out=condition == "timeout",
        peak=(
            None
            if condition == "rss"
            else bench.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            )
        ),
    )
    monkeypatch.setattr(
        bench.harness_memory_guard,
        "guarded_completed_process",
        lambda *a, **kw: observed,
    )
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=[], molt_args=[]
    ) as workload:
        outcome = (
            bench.run_cpython(str(script), 1, workload=workload)
            if runner == "reference"
            else bench.run_binary(tmp_path / "binary", 1, workload=workload)
        )
    assert outcome.ok is (condition == "ok")
    assert outcome.elapsed_s == (None if condition == "clock" else 0.025)
    assert outcome.stdout == stdout and outcome.stderr == stderr
    assert outcome.guard_result is observed
    assert (
        outcome.status
        == {
            "ok": "ok",
            "unclosed": "infrastructure_error",
            "clock": "error",
            "rss": "error",
            "timeout": "timeout",
            "nonzero": "nonzero",
        }[condition]
    )


@pytest.mark.parametrize(
    "failure",
    [
        None,
        "reference",
        "reference_sample",
        "guest_warmup",
        "guest_sample",
        "stderr_warmup",
        "stderr_sample",
    ],
)
def test_individual_sealed_invocations_and_output_oracle(
    tmp_path, monkeypatch, daemon_control_environment, failure
):
    from tests.tools.perf_scoreboard_fixtures import sealed_workload

    binary = tmp_path / "binary"
    binary.write_bytes(b"fixture executable boundary")
    calls = []
    guest_count = 0
    reference_count = 0

    def run(command, **kwargs):
        nonlocal guest_count, reference_count
        calls.append((command, kwargs))
        if "build" in command:
            stdout = json.dumps({"data": {"output": str(binary)}})
        elif command[0] == str(binary):
            guest_count += 1
            bad = (
                failure == "guest_warmup"
                and guest_count == 1
                or failure == "guest_sample"
                and guest_count == 3
            )
            stdout = "sealed:7" if bad else "sealed:7\n"
        else:
            reference_count += 1
            bad = failure == "reference" or (
                failure == "reference_sample" and reference_count == 3
            )
            stdout = "sealed:7" if bad else "sealed:7\n"
        stderr = (
            "unexpected guest diagnostic\n"
            if (
                command[0] == str(binary)
                and (
                    (failure == "stderr_warmup" and guest_count == 1)
                    or (failure == "stderr_sample" and guest_count == 3)
                )
            )
            else ""
        )
        return bench.harness_memory_guard.GuardedCompletedProcess(
            command,
            0,
            stdout,
            stderr,
            elapsed_s=0.1,
            child_elapsed_s=0.1,
            child_returncode=0,
            child_stderr=stderr,
            descendants_closed=True,
            peak=bench.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            ),
        )

    monkeypatch.setattr(bench.harness_memory_guard, "guarded_completed_process", run)
    monkeypatch.setattr(bench, "BENCH_TMP_ROOT", tmp_path / "scratch")
    with sealed_workload(tmp_path) as workload:
        if failure in {"reference", "reference_sample"}:
            with pytest.raises(ValueError, match="semantic oracle"):
                bench.bench_one(str(workload.script), 2, 1, 1, 1, workload=workload)
        else:
            result = bench.bench_one(
                str(workload.script), 2, 1, 1, 1, workload=workload
            )
            assert result["molt_ok"] is (failure is None)
            assert result["output_match"] is (failure is None)
            if failure is not None:
                assert result["molt_speedup"] is None
            assert result["workload"]["entry"] == str(workload.script)
        assert calls[0][0][-1] == str(workload.execution_script)
        assert calls[0][1]["env"]["MOLT_HERMETIC_MODULE_ROOTS"] == "1"
        assert calls[0][1]["env"]["MOLT_MODULE_ROOTS"] == str(
            workload.execution_root / "packages"
        )
        assert all(kwargs["cwd"] == workload.execution_root for _, kwargs in calls)
        references = [command for command, _ in calls if "-P" in command]
        assert references
        assert references[0] == [
            sys.executable,
            "-P",
            "-S",
            "-B",
            str(workload.execution_script),
            "data/input.txt",
            "7",
        ]
        assert calls[1][0] == [str(binary), "data/input.txt", "7"]
