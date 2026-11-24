from __future__ import annotations

import hashlib
import json
import sys
import textwrap
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process
from tools import bench as bench_tool


REPO_ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.slow
def test_bench_cli_native_smoke_contract_batch_reuses_compiler(
    tmp_path: Path,
) -> None:
    fast_script = tmp_path / "fast_script.py"
    fast_script.write_text("print(1)\n", encoding="utf-8")
    slow_script = tmp_path / "slow_script.py"
    slow_script.write_text(
        textwrap.dedent(
            """
            i = 0
            while True:
                i += 1
            print("done")
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    out_json = tmp_path / "bench_combined.json"

    res = run_native_test_process(
        [
            sys.executable,
            "tools/bench.py",
            "--no-cpython",
            "--no-pypy",
            "--no-codon",
            "--no-nuitka",
            "--no-pyodide",
            "--samples",
            "1",
            "--warmup",
            "0",
            "--molt-profile",
            "dev",
            # The slow script never ends, so this bound sets the test's floor. It
            # also must fit the fast script's first exec: on macOS a freshly
            # linked binary waited in admission past 5 s under landing-gate
            # concurrency (2026-11-04: `print(1)` timed out at 4.95 s in two of
            # three land --fast runs).
            "--runtime-timeout-sec",
            "30.0",
            "--json-out",
            str(out_json),
            "--script",
            str(fast_script),
            "--script",
            str(slow_script),
        ],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads(out_json.read_text(encoding="utf-8"))
    fast_entry = payload["benchmarks"][str(fast_script)]
    assert fast_entry["workload"]["entry"] == str(fast_script)
    assert fast_entry["workload"]["entry_source"] == fast_script.name
    assert fast_entry["workload"]["sources"] == [
        {
            "path": fast_script.name,
            "size": 9,
            "sha256": hashlib.sha256(b"print(1)\n").hexdigest(),
        }
    ]
    assert fast_entry["cpython_time_s"] is None
    assert fast_entry["cpython_samples_s"] is None
    assert fast_entry["molt_ok"] is True, res.stderr
    assert len(fast_entry["molt_samples_s"]) == 1
    assert fast_entry["molt_speedup"] is None
    assert fast_entry["requested_profile"] == "dev"
    observation = fast_entry["build_observation"]
    assert observation["kind"] == "molt-build-observation-v1"
    assert observation["selected_profiles"]["guest_profile"] == "dev"
    assert observation["selected_profiles"]["runtime_profile"]
    assert observation["selected_profiles"]["compiler_profile"]
    assert fast_entry["molt_output_parity"] == {
        "checked": False,
        "ok": None,
        "reference_runtime": "cpython",
        "reason": "reference_unavailable",
        "stdout_match": None,
        "stderr_match": None,
        "reference_stdout_sha256": None,
        "reference_stderr_sha256": None,
        "molt_stdout_sha256": None,
        "molt_stderr_sha256": None,
    }

    slow_entry = payload["benchmarks"][str(slow_script)]
    assert slow_entry["molt_ok"] is False
    assert slow_entry["molt_time_s"] is None
    assert slow_entry["molt_samples_s"] == []
    assert slow_entry["molt_status"] == "timeout"
    assert slow_entry["molt_run_status"] == "timeout"
    assert slow_entry["molt_run_timed_out"] is True
    assert slow_entry["molt_failure_phase"] == "run"
    assert slow_entry["molt_failure_status"] == "timeout"
    assert (
        slow_entry["molt_failure_returncode"]
        == bench_tool.memory_guard.TIMEOUT_RETURN_CODE
    )
    assert slow_entry["molt_output_parity"]["ok"] is None
    assert slow_entry["molt_output_parity"]["reason"] == "reference_unavailable"
