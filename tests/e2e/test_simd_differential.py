"""Differential testing: Zig vs Rust WASM SIMD kernels.

Both implementations must produce identical results for identical inputs.
Runs the Node.js differential harness and parses results.

Requires: Node.js v16+, both WASM binaries built.

Run:
    pytest tests/e2e/test_simd_differential.py -v
"""
import json
import os
import struct
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


class TestSIMDDifferential:
    """Verify Zig and Rust WASM SIMD produce bit-identical results."""

    @pytest.fixture(autouse=True)
    def check_binaries(self):
        _check_prerequisites()

    def test_full_differential_harness(self):
        """Run the comprehensive Node.js differential test harness."""
        result = _run_node(HARNESS)
        # Print stdout for visibility in test output
        if result.stdout:
            print(result.stdout)
        if result.stderr:
            print(result.stderr, file=sys.stderr)

        assert result.returncode == 0, (
            f"Differential test harness failed with exit code {result.returncode}.\n"
            f"stdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )

        # Verify we actually ran tests (not just an empty pass)
        assert "RESULTS:" in result.stdout, "Harness did not produce results summary"
        # Extract pass/total counts
        for line in result.stdout.splitlines():
            if "RESULTS:" in line:
                # Format: "=== RESULTS: N/M passed, F failed ==="
                assert "0 failed" in line, f"Some tests failed: {line}"
                break

    def test_matmul_bit_identical(self):
        """Focused matmul test via harness -- matmul lines must all PASS."""
        result = _run_node(HARNESS)
        matmul_lines = [
            l for l in result.stdout.splitlines() if "matmul_f32_tiled" in l
        ]
        assert len(matmul_lines) > 0, "No matmul test lines found in output"
        for line in matmul_lines:
            assert "PASS" in line, f"matmul mismatch: {line}"

    def test_softmax_bit_identical(self):
        """Focused softmax test via harness -- softmax lines must all PASS."""
        result = _run_node(HARNESS)
        softmax_lines = [
            l for l in result.stdout.splitlines() if "softmax_f32_fused" in l
        ]
        assert len(softmax_lines) > 0, "No softmax test lines found in output"
        for line in softmax_lines:
            assert "PASS" in line, f"softmax mismatch: {line}"

    def test_adversarial_inputs(self):
        """Adversarial inputs (NaN, inf, -0, subnormals) must match."""
        result = _run_node(HARNESS)
        adv_lines = [
            l for l in result.stdout.splitlines() if "adversarial" in l.lower()
        ]
        assert len(adv_lines) > 0, "No adversarial test lines found in output"
        for line in adv_lines:
            assert "PASS" in line, f"adversarial input mismatch: {line}"

    def test_deterministic_100_runs(self):
        """Both implementations must be deterministic across 100 runs."""
        result = _run_node(HARNESS)
        det_lines = [
            l for l in result.stdout.splitlines() if "deterministic" in l.lower()
        ]
        assert len(det_lines) >= 2, "Expected both Rust and Zig determinism tests"
        for line in det_lines:
            assert "PASS" in line, f"non-deterministic output: {line}"


class TestSIMDBinarySize:
    """Verify WASM binary sizes stay within targets."""

    @pytest.fixture(autouse=True)
    def check_binaries(self):
        _check_prerequisites()

    def test_rust_under_5kb(self):
        """Rust SIMD WASM must be under 5 KB."""
        size = os.path.getsize(RUST_WASM)
        assert size < 5 * 1024, f"Rust WASM is {size} bytes ({size/1024:.1f} KB), exceeds 5 KB target"

    def test_zig_under_8kb(self):
        """Zig SIMD WASM must be under 8 KB (expanded from 2 KB with full SIMD suite)."""
        size = os.path.getsize(ZIG_WASM)
        assert size < 8 * 1024, f"Zig WASM is {size} bytes ({size/1024:.1f} KB), exceeds 8 KB target"

    def test_zig_smaller_than_rust(self):
        """Zig binary should be smaller than or comparable to Rust."""
        rust_size = os.path.getsize(RUST_WASM)
        zig_size = os.path.getsize(ZIG_WASM)
        # Allow up to 2x larger since Zig now has legacy scalar functions too
        assert zig_size < rust_size * 2, (
            f"Zig ({zig_size/1024:.1f} KB) is more than 2x larger than Rust ({rust_size/1024:.1f} KB)"
        )
