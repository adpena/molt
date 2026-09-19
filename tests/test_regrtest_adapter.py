from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))


def _load_adapter():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_regrtest_adapter", TOOLS_ROOT / "regrtest_adapter.py"
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_regrtest_infrastructure_outcome_is_not_semantic_failure(
    monkeypatch,
) -> None:
    adapter = _load_adapter()
    infrastructure_failure = (
        adapter.pc.harness_memory_guard.memory_guard.GuardInfrastructureFailure(
            phase="temporary_artifact_custody",
            details=("scratch receipt retention failed",),
        )
    )
    measurement = adapter.pc.RunMeasurement(
        returncode=125,
        child_returncode=0,
        infrastructure_failure=infrastructure_failure,
        elapsed_s=0.25,
        peak_rss_bytes=4096,
        peak_job_commit_bytes=None,
        stdout="test_math passed\n",
        stderr="guard failed\n",
        timed_out=False,
    )
    monkeypatch.setattr(
        adapter.pc,
        "run_and_measure",
        lambda *_args, **_kwargs: measurement,
    )

    result = adapter.run_module(
        adapter.RunnerMode.CPYTHON,
        "test_math",
        python_exe=sys.executable,
        molt_cmd=None,
        timeout_s=30.0,
        env=None,
    )

    assert result.passed is None
    assert result.status == "infrastructure_error"
    assert result.evidence_eligible is False
    payload = result.to_json()
    assert payload["returncode"] == 125
    assert payload["child_returncode"] == 0
    assert payload["elapsed_s"] is None
    assert payload["diagnostic_elapsed_s"] == 0.25
    assert payload["peak_rss_bytes"] is None
    assert payload["infrastructure_failure"] == {
        "phase": "temporary_artifact_custody",
        "details": ["scratch receipt retention failed"],
    }

    report = adapter.RunReport(
        runner="cpython",
        python_version="3.14.0",
        python_version_info=[3, 14, 0],
        python_executable=sys.executable,
        host_os="test",
        host_arch="test",
        host_fingerprint="test",
        test_dir="Lib/test",
        requested_modules=["test_math"],
        results=[result],
    ).to_json()
    assert report["status"] == "infrastructure_error"
    assert report["passed_count"] == 0
    assert report["evidence_module_count"] == 0
    assert report["failed_modules"] == []
    assert report["infrastructure_error_modules"] == ["test_math"]
    assert report["total_elapsed_s"] == 0
    assert report["peak_rss_bytes_max"] is None
