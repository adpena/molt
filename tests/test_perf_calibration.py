"""Tests for tools/perf_calibration.py (doc 69 C1-C4 calibration substrate).

Pure-Python: no molt build required. Validates the cross-platform peak-RSS path on
the host these tests run on -- on Windows this exercises the ctypes
GetProcessMemoryInfo path that fixes the native board's "RSS=0" gap.
"""

import importlib.util
import sys
from pathlib import Path

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


def test_run_and_measure_timeout():
    m = pc.run_and_measure(
        [sys.executable, "-c", "import time; time.sleep(10)"], timeout=0.5
    )
    assert m.timed_out
    assert m.elapsed_s < 5  # killed well before the 10s sleep


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
