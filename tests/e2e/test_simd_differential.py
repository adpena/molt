"""Build the real crate once; compare its kernels and browser API to independent oracles."""

from __future__ import annotations

import json
from pathlib import Path

from tests.native_process_guard import run_native_test_process
from tools.build_browser_simd import build_browser_simd

ROOT = Path(__file__).resolve().parents[2]


def test_source_built_simd_numerics_and_browser_consumer(tmp_path: Path) -> None:
    artifact = build_browser_simd(tmp_path / "simd-ops.wasm")
    assert artifact.stat().st_size <= 6 * 1024, (
        "Browser SIMD exceeds its complete-artifact size budget"
    )
    assert (
        artifact.read_bytes() == (ROOT / "deploy/browser/simd-ops.wasm").read_bytes()
    ), "Published browser SIMD differs from the canonical source build; regenerate it"
    run = run_native_test_process(
        ["node", str(ROOT / "tests/e2e/simd_diff_harness.js"), str(artifact)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, f"{run.stdout}\n{run.stderr}"
    marker = "@@SIMD_DIFF_JSON@@ "
    reports = [
        json.loads(line[len(marker) :])
        for line in run.stdout.splitlines()
        if line.startswith(marker)
    ]
    assert len(reports) == 1
    report = reports[0]
    assert report["version"] == 2
    assert report["summary"]["failed"] == 0
    assert report["summary"]["total"] == report["summary"]["passed"]
    assert {group["name"] for group in report["groups"]} == {
        "add_f32",
        "mul_f32",
        "neg_f32",
        "sqrt_f32",
        "reciprocal_f32",
        "max_f32",
        "exp2_f32",
        "reduce_sum_f32",
        "reduce_max_f32",
        "softmax_f32_fused",
        "matmul_f32_tiled",
        "rms_norm_f32",
        "rope_f32",
        "determinism",
        "browser_consumer",
    }
