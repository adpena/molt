from __future__ import annotations

from tools import determinism_perf_gate


def test_determinism_perf_gate_accepts_current_authority() -> None:
    assert determinism_perf_gate.run() == 0


def test_determinism_perf_gate_rejects_contraction_probe() -> None:
    assert determinism_perf_gate.run(probe_unsafe_flag="-ffp-contract=fast") == 1


def test_determinism_perf_gate_rejects_fast_math_probe() -> None:
    assert determinism_perf_gate.run(probe_unsafe_flag="-ffast-math") == 1
