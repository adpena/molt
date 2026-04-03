from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
HARNESS_PATH = REPO_ROOT / "bench" / "harness.py"
SPEC = importlib.util.spec_from_file_location("bench_harness_under_test", HARNESS_PATH)
assert SPEC is not None and SPEC.loader is not None
bench_harness = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench_harness)


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
    monkeypatch.setattr(bench_harness, "build_json_report", lambda *args, **kwargs: {})
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


def test_bench_harness_uses_canonical_defaults() -> None:
    assert bench_harness.DEFAULT_OUTPUT == (
        bench_harness.BENCH_DIR / "results" / "harness.json"
    )
    assert bench_harness.DEFAULT_BASELINE == (
        bench_harness.BENCH_DIR / "results" / "harness-baseline.json"
    )
