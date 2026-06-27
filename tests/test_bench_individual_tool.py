from __future__ import annotations

import contextlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
from types import ModuleType, SimpleNamespace

import molt.dx as molt_dx


ROOT = Path(__file__).resolve().parents[1]


def _load_bench_individual() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "molt_test_bench_individual",
        ROOT / "tools" / "bench_individual.py",
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_bench_individual_reuses_backend_daemon_by_default(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
    cleanups = 0

    def fake_cleanup(*, quiet: bool = False) -> None:
        nonlocal cleanups
        del quiet
        cleanups += 1

    monkeypatch.setattr(bench, "_ensure_clean_slate", fake_cleanup)
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None: (True, 0.02, "1"),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None: (True, 0.04, "1"),
    )

    result = bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=1,
        warmup=1,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert cleanups == 0
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
    bench = _load_bench_individual()
    cleanups = 0

    def fake_cleanup(*, quiet: bool = False) -> None:
        nonlocal cleanups
        del quiet
        cleanups += 1

    monkeypatch.setattr(bench, "_ensure_clean_slate", fake_cleanup)
    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None: (True, 0.02, "1"),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None: (True, 0.04, "1"),
    )

    bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=1,
        warmup=1,
        timeout_build=1.0,
        timeout_run=1.0,
        isolate_daemon=True,
    )

    assert cleanups == 1


def test_bench_individual_isolate_daemon_preserves_foreign_sessions(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
    target = tmp_path / "target"
    daemon_root = target / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    owned_socket = tmp_path / "owned.sock"
    foreign_socket = tmp_path / "foreign.sock"
    owned_socket.write_text("", encoding="utf-8")
    foreign_socket.write_text("", encoding="utf-8")

    def write_identity(name: str, *, pid: int, socket_path: Path) -> None:
        (daemon_root / name).write_text(
            json.dumps(
                {
                    "schema": "molt.backend_daemon.identity.v1",
                    "pid": pid,
                    "socket_path": str(socket_path),
                    "project_root": str(ROOT),
                    "cargo_profile": "dev-fast",
                    "config_digest": None,
                    "backend_bin": "/repo/target/molt-backend",
                    "created_at": 1_700_000_000.0,
                    "command": None,
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

    def fake_ps(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        assert cmd == ["ps", "-axo", "pid=,etimes=,command="]
        stdout = "\n".join(
            [
                f" 101 120 /repo/target/molt-backend --daemon --socket {owned_socket}",
                f" 202 240 /repo/target/molt-backend --daemon --socket {foreign_socket}",
                " 303 300 /repo/target/molt-backend --not-daemon",
            ]
        )
        return subprocess.CompletedProcess(cmd, 0, stdout, "")

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.setattr(bench, "BENCH_TMP_ROOT", tmp_path / "bench-tmp")
    monkeypatch.setattr(bench, "_guarded_bench_process", fake_ps)

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
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
    target = tmp_path / "target"
    (target / ".molt_state" / "backend_daemon").mkdir(parents=True)
    socket_path = tmp_path / "loose.sock"
    socket_path.write_text("", encoding="utf-8")
    killed: list[int] = []

    def fake_ps(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        assert cmd == ["ps", "-axo", "pid=,etimes=,command="]
        stdout = f" 404 120 /repo/target/molt-backend --daemon --socket {socket_path}\n"
        return subprocess.CompletedProcess(cmd, 0, stdout, "")

    def fake_terminate(identity, *, grace: float = 0.75, health_probe=None) -> bool:
        del grace, health_probe
        killed.append(identity.pid)
        return True

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_SOCKET", str(socket_path))
    monkeypatch.setattr(bench, "BENCH_TMP_ROOT", tmp_path / "bench-tmp")
    monkeypatch.setattr(bench, "_guarded_bench_process", fake_ps)
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
    bench = _load_bench_individual()
    calls = 0

    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )

    def fake_run_binary(
        binary: Path, timeout_s: float, limits=None
    ) -> tuple[bool, float, str]:
        del limits
        nonlocal calls
        calls += 1
        if calls == 3:
            return False, 0.03, ""
        return True, 0.02, "1"

    monkeypatch.setattr(bench, "run_binary", fake_run_binary)
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None: (True, 0.04, "1"),
    )

    result = bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=2,
        warmup=1,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert result["run_ok"] is False
    assert result["molt_ok"] is False
    assert result["molt_warmup_samples_s"] == [0.02]
    assert result["molt_samples_s"] == [0.02]
    assert result["error"] == "Molt run failed during sample 2/2"


def test_bench_individual_records_molt_run_failure_detail(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()

    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None: (
            False,
            0.02,
            "rc=7\nstderr:\nruntime intrinsic missing",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None: (True, 0.04, "1"),
    )

    result = bench.bench_one(
        "tests/benchmarks/bench_bytes_find.py",
        samples=1,
        warmup=0,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert result["run_ok"] is False
    assert result["molt_failure_detail"] == "rc=7\nstderr:\nruntime intrinsic missing"
    assert "runtime intrinsic missing" in result["error"]


def test_bench_individual_marks_intrinsic_benchmarks_molt_only(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()

    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None: (True, 0.02, "intrinsic-only"),
    )

    def fail_cpython(
        script: str, timeout_s: float, limits=None
    ) -> tuple[bool, float, str]:
        del limits
        raise AssertionError("Molt-only intrinsic benchmarks must not run CPython")

    monkeypatch.setattr(bench, "run_cpython", fail_cpython)

    result = bench.bench_one(
        "tests/benchmarks/bench_ptr_registry.py",
        samples=1,
        warmup=0,
        timeout_build=1.0,
        timeout_run=1.0,
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
    bench = _load_bench_individual()
    custom = tmp_path / "bench_ptr_registry.py"
    custom.write_text("print('custom')\n", encoding="utf-8")

    monkeypatch.setattr(
        bench,
        "molt_build",
        lambda script, out_dir, timeout_s, extra_args=None, limits=None: (
            tmp_path / "bench_molt",
            0.01,
            "",
        ),
    )
    monkeypatch.setattr(
        bench,
        "run_binary",
        lambda binary, timeout_s, limits=None: (True, 0.02, "custom"),
    )
    monkeypatch.setattr(
        bench,
        "run_cpython",
        lambda script, timeout_s, limits=None: (True, 0.03, "custom"),
    )

    result = bench.bench_one(
        str(custom),
        samples=1,
        warmup=0,
        timeout_build=1.0,
        timeout_run=1.0,
    )

    assert result["reference_runtime"] == "cpython"
    assert result["reference_reason"] == "cpython_reference"
    assert result["cpython_time_s"] == 0.03
    assert result["output_match"] is True


def test_bench_individual_process_helpers_use_molt_bench_guard(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
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
            result = subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")
        else:
            result = subprocess.CompletedProcess(cmd, 0, "ok\n", "")
        result.elapsed_s = 0.0125  # type: ignore[attr-defined]
        return result

    monkeypatch.setattr(
        bench.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    monkeypatch.setenv("MOLT_SESSION_ID", "caller-session")
    # Pin MOLT_EXT_ROOT to its repo-local fallback so the assertions below stay
    # deterministic on developer hosts that have an external (non-C:) artifact
    # drive attached; canonical_harness_env() prefers an external root whenever
    # one is available.
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    for key in (
        "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
        "MOLT_PREFER_EXTERNAL_ARTIFACTS",
        "MOLT_USE_EXTERNAL_ARTIFACTS",
        "MOLT_EXTERNAL_ARTIFACT_ROOTS",
        "MOLT_EXTERNAL_ARTIFACT_CANDIDATES",
        "MOLT_ALLOW_C_DRIVE_ARTIFACTS",
    ):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(molt_dx, "_candidate_roots", lambda _env: ())

    built_binary, build_s, build_err = bench.molt_build(
        str(script),
        tmp_path,
        3.0,
        limits=limits,
    )
    run_ok, run_s, run_out = bench.run_binary(binary, 4.0, limits=limits)
    cpy_ok, cpy_s, cpy_out = bench.run_cpython(str(script), 5.0, limits=limits)

    assert built_binary == binary
    assert build_err == ""
    assert build_s >= 0
    assert (run_ok, run_s, run_out) == (True, 0.0125, "ok")
    assert (cpy_ok, cpy_s, cpy_out) == (True, 0.0125, "ok")
    assert [call["prefix"] for call in calls] == [
        "MOLT_BENCH",
        "MOLT_BENCH",
        "MOLT_BENCH",
    ]
    assert [call["limits"] for call in calls] == [limits, limits, limits]
    assert calls[0]["timeout"] == 3.0
    assert calls[0]["cwd"] == bench.REPO_ROOT
    assert calls[0]["env"]["MOLT_EXT_ROOT"] == str(bench.REPO_ROOT)
    assert calls[0]["env"]["CARGO_TARGET_DIR"] == str(
        bench.REPO_ROOT / "target" / "sessions" / calls[0]["env"]["MOLT_SESSION_ID"]
    )
    assert calls[0]["env"]["MOLT_SESSION_ID"] == "caller-session"
    assert calls[0]["env"]["PYTHONPATH"] == str(bench.REPO_ROOT / "src")
    assert calls[1]["timeout"] == 4.0
    assert calls[2]["timeout"] == 5.0
    assert calls[2]["cmd"] == [sys.executable, str(script)]


def test_bench_individual_main_uses_suite_sentinel_and_shared_limits(
    tmp_path: Path, monkeypatch
) -> None:
    bench = _load_bench_individual()
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
