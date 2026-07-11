from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _load_gate():
    path = ROOT / "tools" / "determinism_perf_gate.py"
    spec = importlib.util.spec_from_file_location("determinism_perf_gate", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_determinism_perf_gate_accepts_current_authority() -> None:
    assert _load_gate().run() == 0


def test_determinism_perf_gate_rejects_contraction_probe() -> None:
    assert _load_gate().run(probe_unsafe_flag="-ffp-contract=fast") == 1


def test_determinism_perf_gate_rejects_fast_math_probe() -> None:
    assert _load_gate().run(probe_unsafe_flag="-ffast-math") == 1
