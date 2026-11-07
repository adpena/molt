from __future__ import annotations

import json
from pathlib import Path

from tools import gate_yield


def _command(argv, *, seconds, reason=None, rss=0):
    return {
        "argv": argv,
        "duration_seconds": seconds,
        "child_cpu_seconds": seconds * 2,
        "peak_rss_bytes": rss,
        "failure_reason": reason,
    }


def _write(directory: Path, name: str, source: str, commands) -> None:
    payload = {"source_commit": source, "commands": commands}
    (directory / name).write_text(json.dumps(payload), encoding="utf-8")


def test_yield_separates_candidate_failures_flakes_and_infrastructure(tmp_path):
    wasm = ["/venv/bin/python", "-m", "pytest", "tests/test_wasm_x.py"]
    lint = ["/bin/sh", "-c", "python tools/check_x.py"]
    crate = ["/venv/bin/python", "tools/check_changed_crate_tests.py"]
    # Commit a: WASM hits memory pressure beside other gates, then passes alone.
    _write(
        tmp_path,
        "a.try-run.heavy.json",
        "a",
        [
            _command(wasm, seconds=600, reason="unsafe memory pressure", rss=7 << 30),
            _command(crate, seconds=100, reason="executor cancellation"),
        ],
    )
    _write(tmp_path, "a.try-run.heavy-retry.json", "a", [_command(wasm, seconds=500)])
    # Commit b: the lint gate exits 1 and nothing retries it: a candidate.
    _write(
        tmp_path, "b.precheck.json", "b", [_command(lint, seconds=2, reason="exit 1")]
    )
    (tmp_path / "notes.json").write_text("[1, 2]", encoding="utf-8")  # not a receipt

    gates = {
        gate.command: gate
        for gate in gate_yield.gate_yields(gate_yield.load_receipts(tmp_path))
    }
    wasm_gate = gates["python -m pytest tests/test_wasm_x.py"]
    assert (wasm_gate.runs, wasm_gate.flake, wasm_gate.candidate) == (2, 1, 0)
    assert wasm_gate.seconds == 1100 and wasm_gate.peak_rss_bytes == 7 << 30
    assert gates["python tools/check_changed_crate_tests.py"].infrastructure == 1
    lint_gate = gates["/bin/sh -c python tools/check_x.py"]
    assert (lint_gate.candidate, lint_gate.flake) == (1, 0)
    ordered = gate_yield.gate_yields(gate_yield.load_receipts(tmp_path))
    assert ordered[0].command == "python -m pytest tests/test_wasm_x.py"


def test_a_failed_retry_is_still_a_candidate_failure(tmp_path):
    gate = ["python", "-m", "pytest", "tests/test_y.py"]
    _write(
        tmp_path,
        "c.try-run.heavy.json",
        "c",
        [_command(gate, seconds=5, reason="exit 2")],
    )
    _write(
        tmp_path,
        "c.try-run.heavy-retry.json",
        "c",
        [_command(gate, seconds=5, reason="exit 2")],
    )
    (result,) = gate_yield.gate_yields(gate_yield.load_receipts(tmp_path))
    assert (result.candidate, result.flake) == (2, 0)
