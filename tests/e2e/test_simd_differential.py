"""Differential testing: Zig vs Rust WASM SIMD kernels.

Both implementations must produce identical results for identical inputs.
Runs the Node.js differential harness and parses results.

Requires: Node.js v16+, both WASM binaries built.

Run:
    pytest tests/e2e/test_simd_differential.py -v
"""

import json
import os
import subprocess
import sys

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HARNESS = os.path.join(ROOT, "tests", "e2e", "simd_diff_harness.js")
RUST_WASM = os.path.join(
    ROOT,
    "deploy",
    "browser",
    "simd-ops-rs",
    "target",
    "wasm32-unknown-unknown",
    "release",
    "simd_ops.wasm",
)
ZIG_WASM = os.path.join(ROOT, "deploy", "browser", "simd-ops-zig", "simd.wasm")
JSON_MARKER = "@@SIMD_DIFF_JSON@@"

EXPECTED_GROUP_COUNTS = {
    "add_f32": 7,
    "mul_f32": 5,
    "neg_f32": 5,
    "sqrt_f32": 5,
    "reciprocal_f32": 5,
    "max_f32": 5,
    "exp2_f32": 5,
    "reduce_sum_f32": 5,
    "reduce_max_f32": 5,
    "softmax_f32_fused": 6,
    "matmul_f32_tiled": 6,
    "rms_norm_f32": 5,
    "rope_f32": 4,
    "adversarial_inputs": 2,
    "determinism": 2,
}


def _check_prerequisites():
    """Verify both WASM binaries exist."""
    missing = []
    if not os.path.isfile(RUST_WASM):
        missing.append(f"Rust WASM not found: {RUST_WASM}")
    if not os.path.isfile(ZIG_WASM):
        missing.append(f"Zig WASM not found: {ZIG_WASM}")
    if missing:
        pytest.skip("\n".join(missing))


def _run_node(script_path: str, timeout: int = 60) -> subprocess.CompletedProcess:
    """Run a Node.js script, returning the completed process."""
    return subprocess.run(
        ["node", script_path],
        capture_output=True,
        text=True,
        timeout=timeout,
        cwd=ROOT,
    )


def _parse_harness_json(stdout: str) -> dict:
    for line in stdout.splitlines():
        if line.startswith(JSON_MARKER + " "):
            return json.loads(line[len(JSON_MARKER) + 1 :])
    raise AssertionError(f"Harness did not emit {JSON_MARKER}")


def _run_harness_json() -> dict:
    result = _run_node(HARNESS)
    if result.stdout:
        print(result.stdout)
    if result.stderr:
        print(result.stderr, file=sys.stderr)

    assert result.returncode == 0, (
        f"Differential test harness failed with exit code {result.returncode}.\n"
        f"stdout:\n{result.stdout}\n"
        f"stderr:\n{result.stderr}"
    )
    return _parse_harness_json(result.stdout)


class TestSIMDDifferential:
    """Verify Zig and Rust WASM SIMD produce bit-identical results."""

    @pytest.fixture(autouse=True)
    def check_binaries(self):
        _check_prerequisites()

    def test_full_differential_harness(self):
        """Run the comprehensive Node.js differential test harness."""
        data = _run_harness_json()

        assert data["version"] == 1
        assert data["summary"] == {"total": 72, "passed": 72, "failed": 0}

        groups = {group["name"]: group for group in data["groups"]}
        assert set(groups) == set(EXPECTED_GROUP_COUNTS)
        for name, expected_cases in EXPECTED_GROUP_COUNTS.items():
            group = groups[name]
            assert group["caseCount"] == expected_cases
            assert group["passed"] == expected_cases
            assert group["failed"] == 0
            assert len(group["cases"]) == expected_cases

    def test_matmul_bit_identical(self):
        """Focused matmul test via harness JSON -- all matmul cases must PASS."""
        data = _run_harness_json()
        matmul = next(
            group for group in data["groups"] if group["name"] == "matmul_f32_tiled"
        )
        assert matmul["caseCount"] == 6
        assert matmul["failed"] == 0
        assert all(case["passed"] for case in matmul["cases"])

    def test_softmax_bit_identical(self):
        """Focused softmax test via harness JSON -- all softmax cases must PASS."""
        data = _run_harness_json()
        softmax = next(
            group for group in data["groups"] if group["name"] == "softmax_f32_fused"
        )
        assert softmax["caseCount"] == 6
        assert softmax["failed"] == 0
        assert all(case["passed"] for case in softmax["cases"])

    def test_adversarial_inputs(self):
        """Adversarial inputs (NaN, inf, -0, subnormals) must match in JSON."""
        data = _run_harness_json()
        adversarial = next(
            group for group in data["groups"] if group["name"] == "adversarial_inputs"
        )
        assert adversarial["caseCount"] == 2
        assert adversarial["failed"] == 0
        assert all(case["passed"] for case in adversarial["cases"])

    def test_deterministic_100_runs(self):
        """Both implementations must be deterministic across 100 runs."""
        data = _run_harness_json()
        determinism = next(
            group for group in data["groups"] if group["name"] == "determinism"
        )
        assert determinism["caseCount"] == 2
        assert determinism["failed"] == 0
        assert all(case["passed"] for case in determinism["cases"])


class TestSIMDBinarySize:
    """Verify WASM binary sizes stay within targets."""

    @pytest.fixture(autouse=True)
    def check_binaries(self):
        _check_prerequisites()

    def test_rust_under_5kb(self):
        """Rust SIMD WASM must be under 5 KB."""
        size = os.path.getsize(RUST_WASM)
        assert size < 5 * 1024, (
            f"Rust WASM is {size} bytes ({size / 1024:.1f} KB), exceeds 5 KB target"
        )

    def test_zig_under_8kb(self):
        """Zig SIMD WASM must be under 8 KB (expanded from 2 KB with full SIMD suite)."""
        size = os.path.getsize(ZIG_WASM)
        assert size < 8 * 1024, (
            f"Zig WASM is {size} bytes ({size / 1024:.1f} KB), exceeds 8 KB target"
        )

    def test_zig_smaller_than_rust(self):
        """Zig binary should be smaller than or comparable to Rust."""
        rust_size = os.path.getsize(RUST_WASM)
        zig_size = os.path.getsize(ZIG_WASM)
        # Allow up to 2x larger since Zig now has legacy scalar functions too
        assert zig_size < rust_size * 2, (
            f"Zig ({zig_size / 1024:.1f} KB) is more than 2x larger than Rust ({rust_size / 1024:.1f} KB)"
        )
