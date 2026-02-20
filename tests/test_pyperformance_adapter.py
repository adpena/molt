from __future__ import annotations

import json
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_SUITE = (
    REPO_ROOT / "tests/differential/pyperformance/fixtures/pyperformance_smoke"
)


def _run_tool(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", "tools/pyperformance_adapter.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_catalog_reports_expected_fixture_metadata() -> None:
    result = _run_tool("catalog", "--suite-root", str(FIXTURE_SUITE), "--json")
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload["benchmark_count"] == 2
    assert payload["groups"] == ["default", "math"]
    assert payload["smoke_available"] == ["nbody", "fannkuch"]


def test_run_subset_executes_fixture_benchmarks() -> None:
    result = _run_tool("run-subset", "--suite-root", str(FIXTURE_SUITE), "--json")
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload["benchmarks"] == ["nbody", "fannkuch"]
    assert len(payload["results"]) == 2
    for row in payload["results"]:
        assert float(row["elapsed_s"]) > 0.0
        assert str(row["result_fingerprint"]) != ""
