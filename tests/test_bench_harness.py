from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import molt.dx as molt_dx
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
        )

    monkeypatch.setattr(
        bench_harness.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    # Pin MOLT_EXT_ROOT to its repo-local fallback so the assertions stay
    # deterministic on developer hosts that have an external (non-C:) artifact
    # drive attached; _base_env() -> development_artifact_env() prefers an
    # external root whenever one is available.
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
    assert call["env"]["MOLT_EXT_ROOT"] == str(bench_harness.REPO_ROOT)
    assert call["env"]["CARGO_TARGET_DIR"] == str(
        bench_harness.REPO_ROOT / "target" / "sessions" / call["env"]["MOLT_SESSION_ID"]
    )
    assert call["env"]["TMPDIR"] == str(bench_harness.REPO_ROOT / "tmp")


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
    assert sentinel_calls[0]["artifact_root"] == (
        bench_harness.REPO_ROOT / "tmp" / "bench" / "harness"
    )
    assert sentinel_calls[0]["label"] == "bench_harness"
    assert "memory_guard" in report_calls[0]


def test_bench_harness_uses_canonical_defaults() -> None:
    assert bench_harness.DEFAULT_OUTPUT == (
        bench_harness.BENCH_DIR / "results" / "harness.json"
    )
    assert bench_harness.DEFAULT_BASELINE == (
        bench_harness.BENCH_DIR / "results" / "harness-baseline.json"
    )
