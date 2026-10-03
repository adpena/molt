"""Tests for tools/perf_calibration.py (doc 69 C1-C4 calibration substrate).

Pure-Python: no molt build required. Validates the cross-platform peak-RSS path on
the host these tests run on -- on Windows this exercises the ctypes
GetProcessMemoryInfo path that fixes the native board's "RSS=0" gap.
"""

import importlib.util
import sys
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

_MOD_PATH = Path(__file__).resolve().parents[1] / "tools" / "perf_calibration.py"
_spec = importlib.util.spec_from_file_location("perf_calibration", _MOD_PATH)
pc = importlib.util.module_from_spec(_spec)
# Register before exec: `from __future__ import annotations` makes @dataclass resolve
# string annotations via sys.modules[cls.__module__].
sys.modules["perf_calibration"] = pc
_spec.loader.exec_module(pc)


def test_host_fingerprint_shape():
    fp = pc.host_fingerprint()
    assert fp.os and fp.arch and fp.cpu
    assert fp.logical_cores >= 1
    assert fp.python_version
    assert len(fp.key()) == 16
    # deterministic + stable
    assert fp.key() == pc.host_fingerprint().key()


def test_peak_rss_self_positive():
    v = pc.peak_rss_self_bytes()
    # Every OS molt targets (Windows/macOS/Linux) must report a real peak RSS.
    assert v is not None, (
        "peak RSS unavailable -- the memory dimension is broken on this OS"
    )
    assert v > 1_000_000  # the test process itself is well over 1 MB


def test_run_and_measure_captures_output_and_peak():
    # Child allocates ~60 MB and HOLDS it for 150 ms (many poll intervals), as a real
    # benchmark holds its working set; peak RSS must reflect the live allocation.
    m = pc.run_and_measure(
        [
            sys.executable,
            "-c",
            "import time; x=bytearray(60_000_000); time.sleep(0.15); print(len(x))",
        ]
    )
    assert m.returncode == 0
    assert "60000000" in m.stdout
    assert not m.timed_out
    assert m.peak_rss_bytes is not None, (
        "child peak RSS is None -- cross-platform RSS poll failed"
    )
    assert m.peak_rss_bytes > 30_000_000, (
        f"peak {m.peak_rss_bytes} too low for a held 60 MB allocation"
    )
    assert m.elapsed_s > 0
    if sys.platform == "win32":
        assert m.peak_job_commit_bytes is not None
        assert m.peak_job_commit_bytes > 0
    else:
        assert m.peak_job_commit_bytes is None


def test_run_and_measure_timeout():
    m = pc.run_and_measure(
        [sys.executable, "-c", "import time; time.sleep(10)"], timeout=0.5
    )
    assert m.timed_out
    assert m.elapsed_s < 5  # killed well before the 10s sleep


def test_run_and_measure_preserves_infrastructure_outcome(monkeypatch):
    infrastructure_failure = (
        pc.harness_memory_guard.memory_guard.GuardInfrastructureFailure(
            phase="temporary_artifact_custody",
            details=("scratch receipt retention failed",),
        )
    )
    guarded_result = SimpleNamespace(
        returncode=125,
        child_returncode=0,
        infrastructure_failure=infrastructure_failure,
        elapsed_s=0.75,
        child_elapsed_s=0.25,
        peak_total=None,
        peak_job_commit_bytes=None,
        stdout=b"child completed\n",
        stderr=b"guard failed\n",
        timed_out=False,
    )
    context = SimpleNamespace(
        limits=SimpleNamespace(poll_interval=0.1),
        run=lambda *_args, **_kwargs: guarded_result,
    )
    monkeypatch.setattr(pc, "replace", lambda value, **_kwargs: value)
    monkeypatch.setattr(
        pc.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        lambda *_args, **_kwargs: context,
    )

    measurement = pc.run_and_measure(["successful-child"])

    assert measurement.elapsed_s == 0.25
    assert measurement.returncode == 125
    assert measurement.child_returncode == 0
    assert measurement.infrastructure_failure is infrastructure_failure
    assert measurement.status == "infrastructure_error"
    assert measurement.evidence_eligible is False


def test_run_and_measure_closes_child_when_spawn_observer_fails():
    observed_pids: list[int] = []

    def reject_spawn(_pid: int) -> None:
        observed_pids.append(_pid)
        raise RuntimeError("observer rejected child")

    with pytest.raises(RuntimeError, match="observer rejected child"):
        pc.run_and_measure(
            [sys.executable, "-c", "import time; time.sleep(10)"],
            on_spawn=reject_spawn,
        )
    assert observed_pids
    assert (
        observed_pids[0] not in pc.harness_memory_guard.memory_guard.sample_processes()
    )


def test_run_and_measure_reaps_nested_child_after_root_exit(tmp_path, monkeypatch):
    pid_path = tmp_path / "nested.pid"
    release_path = tmp_path / "root-may-exit"
    guard = pc.harness_memory_guard.memory_guard
    spawned_pids: list[int] = []
    observed_nested_pids: set[int] = set()
    original_update = guard.ProcessTreeTracker.update

    def observe_custody(tracker, samples):
        watched = original_update(tracker, samples)
        if (
            not spawned_pids
            or tracker.root_pid != spawned_pids[0]
            or observed_nested_pids
        ):
            return watched
        try:
            nested_pid = int(pid_path.read_text(encoding="utf-8"))
        except (FileNotFoundError, ValueError):
            # The root may not have published the complete PID yet.
            return watched
        expected_pids = {spawned_pids[0], nested_pid}
        identities = tracker.custody_identities(expected_pids)
        if (
            expected_pids <= watched
            and set(identities) == expected_pids
            and all(
                guard._process_model.process_identity_has_creation_marker(identity)
                for identity in identities.values()
            )
        ):
            observed_nested_pids.add(nested_pid)
            release_path.touch()
        return watched

    # Observe the real admission result on every platform, including native
    # Windows Job membership. This hook grants no custody and changes no sample.
    # POSIX cleanup intentionally cannot claim an unseen reparented PGID peer.
    monkeypatch.setattr(guard.ProcessTreeTracker, "update", observe_custody)
    child_source = (
        "import pathlib, subprocess, sys, time\n"
        "p=subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])\n"
        f"pathlib.Path({str(pid_path)!r}).write_text(str(p.pid), encoding='utf-8')\n"
        f"release=pathlib.Path({str(release_path)!r})\n"
        "deadline=time.monotonic()+5.0\n"
        "while not release.exists():\n"
        "    if time.monotonic() >= deadline:\n"
        "        raise RuntimeError('guard did not observe nested child custody')\n"
        "    time.sleep(0.005)\n"
    )

    measurement = pc.run_and_measure(
        [sys.executable, "-c", child_source], on_spawn=spawned_pids.append
    )

    assert measurement.returncode == 0
    nested_pid = int(pid_path.read_text(encoding="utf-8"))
    assert observed_nested_pids == {nested_pid}
    deadline = time.monotonic() + 5.0
    while nested_pid in pc.harness_memory_guard.memory_guard.sample_processes():
        if time.monotonic() >= deadline:
            pytest.fail(f"nested process {nested_pid} survived benchmark custody")
        time.sleep(0.05)


def test_adaptive_samples_converges_on_stable_signal():
    import random

    rng = random.Random(1234)
    s = pc.adaptive_samples(
        lambda: 1.000 + rng.uniform(-0.0005, 0.0005),
        min_n=5,
        max_n=40,
        target_rel_ci=0.01,
        warmup=0,
    )
    assert s.n >= 5
    assert s.converged
    assert 0.99 < s.median < 1.01
    assert s.ci95_low <= s.mean <= s.ci95_high
    assert s.cv < 0.01


def test_adaptive_samples_reports_without_false_convergence_on_noise():
    import random

    rng = random.Random(7)
    # Wildly noisy signal: must hit max_n and honestly report not-converged.
    s = pc.adaptive_samples(
        lambda: rng.uniform(0.1, 2.0), min_n=5, max_n=12, target_rel_ci=0.001, warmup=0
    )
    assert s.n == 12
    assert not s.converged  # honest: noise is not silently called stable


def test_measure_quiescence_shape():
    q = pc.measure_quiescence()
    assert isinstance(q.certified, bool)
    assert isinstance(q.competing_builds, int)
    assert q.detail


def test_cold_budget_calibration():
    r = pc.calibrate_cold_budget([sys.executable, "-c", "pass"], runs=5)
    assert r["kind"] == "cold_budget_calibration"
    assert r["runs"] == 5
    assert r["measured_max_ms"] is not None and r["measured_max_ms"] > 0
    assert r["budget_ms"] is not None
    # budget is the measured max plus the margin -> strictly above the max.
    assert r["budget_ms"] >= r["measured_max_ms"]


def test_cold_budget_rejects_infrastructure_measurements(monkeypatch):
    infrastructure_failure = (
        pc.harness_memory_guard.memory_guard.GuardInfrastructureFailure(
            phase="temporary_artifact_custody",
            details=("scratch receipt retention failed",),
        )
    )
    measurement = pc.RunMeasurement(
        returncode=125,
        child_returncode=0,
        infrastructure_failure=infrastructure_failure,
        elapsed_s=0.25,
        peak_rss_bytes=4096,
        peak_job_commit_bytes=None,
        stdout="child completed\n",
        stderr="guard failed\n",
    )
    monkeypatch.setattr(pc, "run_and_measure", lambda *_args, **_kwargs: measurement)

    result = pc.calibrate_cold_budget(["successful-child"], runs=5)

    assert result["status"] == "infrastructure_error"
    assert result["evidence_runs"] == 0
    assert result["budget_ms"] is None
    assert result["measured_max_ms"] is None
    assert result["guard_returncode"] == 125
    assert result["child_returncode"] == 0
    assert result["infrastructure_failure"] == {
        "phase": "temporary_artifact_custody",
        "details": ["scratch receipt retention failed"],
    }


def test_cold_budget_cli_uses_remainder_command(monkeypatch, capsys):
    captured = {}

    def fake_calibrate(run_argv, *, runs=11, **kwargs):
        del kwargs
        captured["run_argv"] = list(run_argv)
        captured["runs"] = runs
        return {"kind": "cold_budget_calibration", "runs": runs, "budget_ms": 42}

    monkeypatch.setattr(pc, "calibrate_cold_budget", fake_calibrate)

    rc = pc._main(["cold-budget", "--runs", "3", "--", sys.executable, "-c", "pass"])

    assert rc == 0
    assert captured == {"run_argv": [sys.executable, "-c", "pass"], "runs": 3}
    assert '"budget_ms": 42' in capsys.readouterr().out


def test_calibration_cache_roundtrip(tmp_path):
    saved = pc.save_calibration({"budget_ms": 123, "kind": "test"}, repo_root=tmp_path)
    assert saved.exists()
    loaded = pc.load_calibration(repo_root=tmp_path)
    assert loaded is not None
    assert loaded["calibration"]["budget_ms"] == 123
    assert loaded["fingerprint_key"] == pc.host_fingerprint().key()


def test_load_calibration_absent_is_none(tmp_path):
    assert pc.load_calibration(repo_root=tmp_path) is None


@pytest.mark.parametrize("child_time", [None, True, float("nan"), float("inf"), -0.1])
def test_run_and_measure_rejects_missing_child_clock(monkeypatch, child_time):
    result = SimpleNamespace(
        child_elapsed_s=child_time,
        elapsed_s=1.0,
        returncode=0,
        timed_out=False,
        infrastructure_failure=None,
    )
    context = SimpleNamespace(
        limits=SimpleNamespace(poll_interval=0.1),
        run=lambda *_args, **_kwargs: result,
    )
    monkeypatch.setattr(pc, "replace", lambda value, **_kwargs: value)
    monkeypatch.setattr(
        pc.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        lambda *_args, **_kwargs: context,
    )
    with pytest.raises(RuntimeError, match="child elapsed-time telemetry"):
        pc.run_and_measure(["child"])


@pytest.mark.parametrize("returncode,timed_out", [(124, True), (125, False)])
def test_run_and_measure_preserves_failure_without_child_clock(
    monkeypatch, returncode, timed_out
):
    result = SimpleNamespace(
        child_elapsed_s=None,
        elapsed_s=1.0,
        returncode=returncode,
        timed_out=timed_out,
        peak_total=None,
        peak_job_commit_bytes=None,
        stdout=b"",
        stderr=b"owned failure",
        child_returncode=None,
        infrastructure_failure=None,
    )
    context = SimpleNamespace(
        limits=SimpleNamespace(poll_interval=0.1), run=lambda *_args, **_kwargs: result
    )
    monkeypatch.setattr(pc, "replace", lambda value, **_kwargs: value)
    monkeypatch.setattr(
        pc.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        lambda *_args, **_kwargs: context,
    )
    measured = pc.run_and_measure(["failed-child"])
    assert measured.returncode == returncode
    assert measured.timed_out == timed_out
    assert measured.evidence_eligible is False
    assert measured.stderr == "owned failure"


@pytest.mark.parametrize("metadata", ["violation", "orphaned_process_groups"])
def test_calibration_preserves_guard_failure_metadata_without_clock(
    monkeypatch, metadata
):
    result = SimpleNamespace(
        child_elapsed_s=None,
        elapsed_s=1.0,
        returncode=0,
        timed_out=False,
        infrastructure_failure=None,
        peak_total=None,
        peak_job_commit_bytes=None,
        stdout=b"",
        stderr=b"guard failure",
    )
    setattr(result, metadata, object() if metadata == "violation" else (12345,))
    context = SimpleNamespace(
        limits=SimpleNamespace(poll_interval=0.1), run=lambda *_args, **_kwargs: result
    )
    monkeypatch.setattr(pc, "replace", lambda value, **_kwargs: value)
    monkeypatch.setattr(
        pc.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        lambda *_args, **_kwargs: context,
    )
    measured = pc.run_and_measure(["failed-child"])
    assert measured.returncode == 0
    assert measured.status == "failed" and not measured.evidence_eligible
    assert getattr(measured, metadata) == getattr(result, metadata)
    assert measured.stderr == "guard failure"
