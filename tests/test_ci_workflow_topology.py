from __future__ import annotations

from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def _read(path: str) -> str:
    return (REPO_ROOT / path).read_text(encoding="utf-8")


def test_ci_push_path_is_cheap_only() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert "docs-gates:" in ci_text
    assert "python-tooling-smoke:" in ci_text
    assert "rust-build-unit-smoke:" in ci_text
    assert "differential-tests:" not in ci_text
    assert "benchmark:" not in ci_text
    assert "parity:" not in ci_text
    assert "runs-on: ubuntu-latest" in ci_text
    assert "runs-on: macos-14" not in ci_text
    assert "Swatinem/rust-cache@v2" in ci_text
    assert "tests/test_bench_harness.py" in ci_text
    assert "tests/test_bench_tool.py" in ci_text
    assert "tests/test_ci_workflow_topology.py" in ci_text
    assert "tests/test_harness_conformance.py" in ci_text
    assert "tests/test_harness_layers.py" in ci_text
    assert "tests/test_monty_conformance_runner.py" in ci_text


def test_nightly_contains_correctness_jobs() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")

    assert "schedule:" in nightly_text
    assert "workflow_dispatch:" in nightly_text
    assert "molt-conformance-full:" in nightly_text
    assert "differential-basic-stdlib:" in nightly_text
    assert "tests/harness/run_molt_conformance.py" in nightly_text
    assert "--suite full" in nightly_text
    assert "--build-profile dev" in nightly_text
    assert 'MOLT_DIFF_MEASURE_RSS: "1"' in nightly_text
    assert 'MOLT_DIFF_RLIMIT_GB: "10"' in nightly_text
    assert "tests/differential/basic" in nightly_text
    assert "tests/differential/stdlib" in nightly_text
    assert 'REPRO_ROOT="$PWD/tmp/repro_sweep"' in nightly_text
    assert "mkdir -p /tmp/repro_sweep" not in nightly_text
    assert "MOLT_CACHE=/tmp/repro_sweep" not in nightly_text
    assert "~/.molt/build/" not in nightly_text


def test_release_and_perf_workflows_exist_for_hosted_validation() -> None:
    release_text = _read(".github/workflows/release.yml")
    perf_text = _read(".github/workflows/perf-validation.yml")

    assert "push:" in release_text
    assert "tags:" in release_text
    assert "workflow_dispatch:" in release_text
    assert "macos-14" in release_text
    assert "ubuntu-24.04" in release_text
    assert "schedule:" not in perf_text
    assert "MOLT_SESSION_ID: perf-validation" in perf_text
    assert "CARGO_TARGET_DIR: ${{ github.workspace }}/target" in perf_text
    assert "MOLT_CACHE: ${{ github.workspace }}/.molt_cache" in perf_text
    assert "TMPDIR: ${{ github.workspace }}/tmp" in perf_text
    assert "tools/bench.py" in perf_text
    assert "--molt-profile release" in perf_text
    assert "bench/results/" in perf_text
