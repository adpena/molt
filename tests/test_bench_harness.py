from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import molt.dx as molt_dx
from molt import custody_layout
import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
HARNESS_PATH = REPO_ROOT / "bench" / "harness.py"
SPEC = importlib.util.spec_from_file_location("bench_harness_under_test", HARNESS_PATH)
assert SPEC is not None and SPEC.loader is not None
bench_harness = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench_harness)


def test_bench_harness_run_cmd_uses_memory_guard(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": cmd, **kwargs})
        return bench_harness.harness_memory_guard.GuardedCompletedProcess(
            cmd,
            0,
            "ok\n",
            "",
            elapsed_s=0.02,
            child_stderr="",
        )

    monkeypatch.setattr(
        bench_harness.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    # Remove every explicit artifact-root candidate so the assertions stay
    # deterministic on any host: with none, artifacts live under the checkout's
    # custody root (the checkout itself for a plain clone, `<root>` for a
    # `<root>/molt-src` family, the ephemeral root under hosted CI).
    for key in (
        *molt_dx.CANONICAL_RUN_ENV_KEYS,
        *molt_dx.DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS,
        molt_dx.EXTERNAL_ARTIFACT_ROOTS_ENV,
    ):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(molt_dx, "_candidate_roots", lambda _root, _env: ())

    stdout, stderr, returncode, elapsed = bench_harness.run_cmd(
        ["python3", "--version"],
        9.0,
        cwd=tmp_path,
    )

    assert (stdout, stderr, returncode, elapsed) == ("ok\n", "", 0, 0.02)
    call = calls[0]
    assert call["cmd"] == ["python3", "--version"]
    assert call["prefix"] == bench_harness.BENCH_MEMORY_PREFIX
    assert call["cwd"] == tmp_path
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["timeout"] == 9.0
    artifact_root = molt_dx.checkout_custody(bench_harness.REPO_ROOT).custody_root
    assert call["env"]["MOLT_EXT_ROOT"] == str(artifact_root)
    assert call["env"]["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_environment(
            artifact_root, call["env"], source_root=bench_harness.REPO_ROOT
        )
    )
    assert call["env"]["TMPDIR"] == str(
        custody_layout.scratch_root(artifact_root, bench_harness.REPO_ROOT)
    )


def test_bench_harness_supports_explicit_molt_profile(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    captured: dict[str, object] = {}

    def fake_run_suite(
        suite_name,
        scripts,
        molt_cmd,
        python_cmd,
        timeout_s,
        parallel,
        colors,
        verbose,
    ):
        captured["molt_cmd"] = molt_cmd
        return [], bench_harness.SuiteSummary(suite=suite_name)

    monkeypatch.setattr(
        bench_harness, "collect_bench_scripts", lambda filter_pat=None: []
    )
    monkeypatch.setattr(bench_harness, "run_suite", fake_run_suite)
    monkeypatch.setattr(bench_harness, "detect_regressions", lambda *args, **kwargs: [])
    monkeypatch.setattr(
        bench_harness, "print_summary_table", lambda *args, **kwargs: None
    )
    report_calls: list[dict[str, object]] = []

    def fake_build_json_report(*args, **kwargs):
        report_calls.append(kwargs)
        return {}

    class FakeSentinel:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            return None

    sentinel_calls: list[dict[str, object]] = []

    def fake_repo_process_sentinel(**kwargs):
        sentinel_calls.append(kwargs)
        return FakeSentinel()

    monkeypatch.setattr(bench_harness, "build_json_report", fake_build_json_report)
    monkeypatch.setattr(
        bench_harness.harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bench/harness.py",
            "--bench",
            "--molt",
            ".venv/bin/molt",
            "--molt-profile",
            "release",
            "--output",
            str(tmp_path / "bench.json"),
        ],
    )

    with pytest.raises(SystemExit) as excinfo:
        bench_harness.main()

    assert excinfo.value.code == 0
    assert captured["molt_cmd"] == [
        ".venv/bin/molt",
        "run",
        "--profile",
        "release",
    ]
    assert sentinel_calls[0]["repo_root"] == bench_harness.REPO_ROOT
    harness_scratch = sentinel_calls[0]["artifact_root"]
    assert harness_scratch.parts[-2:] == ("bench", "harness")
    assert not harness_scratch.is_relative_to(bench_harness.REPO_ROOT.resolve())
    assert sentinel_calls[0]["label"] == "bench_harness"
    assert "memory_guard" in report_calls[0]


def test_bench_harness_uses_canonical_defaults() -> None:
    assert bench_harness.DEFAULT_OUTPUT == (
        bench_harness.BENCH_DIR / "results" / "harness.json"
    )
    assert bench_harness.DEFAULT_BASELINE == (
        bench_harness.BENCH_DIR / "results" / "harness-baseline.json"
    )


@pytest.mark.parametrize("current_time", [0.01, 2.0])
def test_legacy_harness_refuses_unbound_timing_comparison_and_keeps_raw_measurement(
    tmp_path: Path, current_time: float
) -> None:
    baseline = tmp_path / "baseline.json"
    baseline.write_text(
        json.dumps(
            {
                "provenance": {"profile": "release-fast"},
                "benchmarks": {"sample.py": {"molt_ok": True, "molt_time_s": 1.0}},
            }
        ),
        encoding="utf-8",
    )
    result = bench_harness.TestResult(
        name="sample.py",
        suite="bench",
        status="pass",
        molt_time_s=current_time,
        cpython_time_s=1.0,
    )
    before = result.to_dict()
    findings = bench_harness.detect_regressions([result], baseline)
    assert len(findings) == 1
    assert findings[0]["type"] == "incompatible_timing"
    assert "selected build profiles unbound" in findings[0]["detail"]
    assert result.to_dict() == before

    report = bench_harness.build_json_report(
        [result], [], findings, molt_profile="release"
    )
    assert report["provenance"]["requested_profile"] == "release"
    assert report["provenance"]["authoritative"] is False
    cell = report["benchmarks"]["sample.py"]
    assert cell["molt_time_s"] == current_time
    assert cell["requested_profile"] == "release" and cell["build_observation"] is None


def test_legacy_harness_keeps_status_regression_independent_of_unbound_profiles(
    tmp_path: Path,
) -> None:
    baseline = tmp_path / "baseline.json"
    baseline.write_text(
        json.dumps(
            {"benchmarks": {"sample.py": {"status": "pass", "molt_time_s": 1.0}}}
        ),
        encoding="utf-8",
    )
    result = bench_harness.TestResult(
        name="sample.py", suite="bench", status="fail", molt_time_s=2.0
    )
    assert bench_harness.detect_regressions([result], baseline) == [
        {
            "name": "sample.py",
            "type": "status",
            "detail": "was passing, now fail",
        }
    ]
