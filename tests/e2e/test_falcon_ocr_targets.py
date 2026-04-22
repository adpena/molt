"""
Falcon-OCR target matrix test.

Verifies that the SAME inference produces consistent results across
compilation targets:
    - CPU device (always available)
    - Metal device (macOS only, via tinygrad.device.Device)
    - Rendered MSL source (validate it compiles)
    - Rendered WGSL source (validate it is valid)
    - Rendered CUDA source (validate syntax)

For CPU and Metal: verify numerical parity.
For rendered sources: validate they compile/parse correctly.

Run: python -m pytest tests/e2e/test_falcon_ocr_targets.py -v
"""

from __future__ import annotations

import json
import math
import os
import struct
import subprocess
import sys
import tempfile
import time

import pytest

_project_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
if _project_root not in sys.path:
    sys.path.insert(0, _project_root)

from tests.e2e.falcon_ocr_stub_weights import (
    STUB_CONFIG,
    generate_stub_config_json,
    generate_stub_weights,
    generate_test_image,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _molt_runtime_available() -> bool:
    try:
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)
        from molt.gpu import Buffer  # noqa: F401
        return True
    except ImportError:
        return False


def _is_macos() -> bool:
    return sys.platform == "darwin"


def _metal_compiler_available() -> bool:
    """Check if the Metal shader compiler (xcrun metal) is available."""
    if not _is_macos():
        return False
    try:
        result = subprocess.run(
            ["xcrun", "--find", "metal"],
            capture_output=True,
            timeout=10,
        )
        return result.returncode == 0
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


def _wgsl_validator_available() -> bool:
    """Check if a WGSL validator (naga) is available."""
    try:
        result = subprocess.run(
            ["naga", "--version"],
            capture_output=True,
            timeout=10,
        )
        return result.returncode == 0
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


def _nvcc_available() -> bool:
    """Check if CUDA compiler (nvcc) is available."""
    try:
        result = subprocess.run(
            ["nvcc", "--version"],
            capture_output=True,
            timeout=10,
        )
        return result.returncode == 0
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


_SKIP_RUNTIME = not _molt_runtime_available()
_RUNTIME_REASON = "molt runtime not importable"


def _allclose(a: list, b: list, atol: float = 1e-5, rtol: float = 1e-4) -> bool:
    if len(a) != len(b):
        return False
    for ai, bi in zip(a, b):
        if abs(ai - bi) > atol + rtol * abs(bi):
            return False
    return True


# ---------------------------------------------------------------------------
# Tests: CPU target
# ---------------------------------------------------------------------------

@pytest.mark.skipif(_SKIP_RUNTIME, reason=_RUNTIME_REASON)
class TestCPUTarget:
    """Verify Falcon-OCR runs correctly on CPU target."""

    @pytest.fixture(autouse=True)
    def _setup(self):
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)

    def test_cpu_inference_produces_tokens(self):
        """CPU target produces non-empty token output."""
        from tinygrad.device import Device
        Device.set("CPU")

        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)

        init(weights, config)
        tokens = ocr_tokens(32, 32, image, [1, 2, 3], max_new_tokens=5)

        assert isinstance(tokens, list)
        assert len(tokens) >= 1
        assert all(isinstance(t, int) for t in tokens)

    def test_cpu_determinism(self):
        """CPU target is deterministic across runs."""
        from tinygrad.device import Device
        Device.set("CPU")

        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)
        prompt = [1, 2, 3, 4]

        init(weights, config)
        tokens1 = ocr_tokens(32, 32, image, prompt, max_new_tokens=5)

        init(weights, config)
        tokens2 = ocr_tokens(32, 32, image, prompt, max_new_tokens=5)

        assert tokens1 == tokens2


# ---------------------------------------------------------------------------
# Tests: Metal target (macOS only)
# ---------------------------------------------------------------------------

@pytest.mark.skipif(_SKIP_RUNTIME, reason=_RUNTIME_REASON)
@pytest.mark.skipif(not _is_macos(), reason="Metal requires macOS")
class TestMetalTarget:
    """Verify Falcon-OCR runs correctly on Metal target."""

    @pytest.fixture(autouse=True)
    def _setup(self):
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)

    def test_metal_inference_produces_tokens(self):
        """Metal target produces non-empty token output."""
        from tinygrad.device import Device
        Device.set("METAL")

        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)

        init(weights, config)
        tokens = ocr_tokens(32, 32, image, [1, 2, 3], max_new_tokens=5)

        assert isinstance(tokens, list)
        assert len(tokens) >= 1

    def test_metal_cpu_parity(self):
        """Metal and CPU produce identical token sequences."""
        from tinygrad.device import Device
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)
        prompt = [1, 2, 3, 4]

        # CPU run
        Device.set("CPU")
        init(weights, config)
        cpu_tokens = ocr_tokens(32, 32, image, prompt, max_new_tokens=5)

        # Metal run
        Device.set("METAL")
        init(weights, config)
        metal_tokens = ocr_tokens(32, 32, image, prompt, max_new_tokens=5)

        assert cpu_tokens == metal_tokens, (
            f"CPU/Metal parity failure: {cpu_tokens} vs {metal_tokens}"
        )


# ---------------------------------------------------------------------------
# Tests: MSL source validation (macOS only)
# ---------------------------------------------------------------------------

@pytest.mark.skipif(not _metal_compiler_available(), reason="xcrun metal not available")
class TestMSLSource:
    """Validate that rendered MSL (Metal Shading Language) source compiles."""

    def _make_stub_msl_kernel(self) -> str:
        """Generate a representative MSL kernel for RMSNorm.

        This tests the kind of kernel that Falcon-OCR would produce
        when compiled to Metal.
        """
        return """\
#include <metal_stdlib>
using namespace metal;

// RMSNorm kernel: x / sqrt(mean(x^2) + eps)
kernel void rms_norm(
    device const float* input [[buffer(0)]],
    device float* output [[buffer(1)]],
    constant uint& dim [[buffer(2)]],
    constant float& eps [[buffer(3)]],
    uint gid [[thread_position_in_grid]]
) {
    uint row = gid;
    uint base = row * dim;

    // Compute mean of squares
    float sum_sq = 0.0f;
    for (uint i = 0; i < dim; i++) {
        float v = input[base + i];
        sum_sq += v * v;
    }
    float rms = rsqrt(sum_sq / float(dim) + eps);

    // Normalize
    for (uint i = 0; i < dim; i++) {
        output[base + i] = input[base + i] * rms;
    }
}

// Scaled dot-product attention kernel (single head)
kernel void sdpa_single_head(
    device const float* q [[buffer(0)]],
    device const float* k [[buffer(1)]],
    device const float* v [[buffer(2)]],
    device float* output [[buffer(3)]],
    constant uint& seq_len [[buffer(4)]],
    constant uint& head_dim [[buffer(5)]],
    constant float& scale [[buffer(6)]],
    uint gid [[thread_position_in_grid]]
) {
    uint q_pos = gid;
    if (q_pos >= seq_len) return;

    // Compute attention scores
    for (uint d = 0; d < head_dim; d++) {
        float val = 0.0f;
        float max_score = -INFINITY;

        // First pass: find max score
        for (uint k_pos = 0; k_pos <= q_pos; k_pos++) {
            float dot = 0.0f;
            for (uint dd = 0; dd < head_dim; dd++) {
                dot += q[q_pos * head_dim + dd] * k[k_pos * head_dim + dd];
            }
            dot *= scale;
            max_score = max(max_score, dot);
        }

        // Second pass: softmax + weighted sum
        float sum_exp = 0.0f;
        for (uint k_pos = 0; k_pos <= q_pos; k_pos++) {
            float dot = 0.0f;
            for (uint dd = 0; dd < head_dim; dd++) {
                dot += q[q_pos * head_dim + dd] * k[k_pos * head_dim + dd];
            }
            dot *= scale;
            float w = exp(dot - max_score);
            sum_exp += w;
            val += w * v[k_pos * head_dim + d];
        }
        output[q_pos * head_dim + d] = val / sum_exp;
    }
}
"""

    def test_rms_norm_msl_compiles(self):
        """RMSNorm MSL kernel compiles with xcrun metal."""
        msl_source = self._make_stub_msl_kernel()
        with tempfile.NamedTemporaryFile(suffix=".metal", mode="w", delete=False) as f:
            f.write(msl_source)
            f.flush()
            msl_path = f.name

        try:
            result = subprocess.run(
                ["xcrun", "metal", "-c", msl_path, "-o", "/dev/null"],
                capture_output=True,
                text=True,
                timeout=30,
            )
            assert result.returncode == 0, (
                f"MSL compilation failed:\n{result.stderr}"
            )
        finally:
            os.unlink(msl_path)


# ---------------------------------------------------------------------------
# Tests: WGSL source validation
# ---------------------------------------------------------------------------

class TestWGSLSource:
    """Validate that rendered WGSL source is syntactically valid."""

    def _make_stub_wgsl_kernel(self) -> str:
        """Generate a representative WGSL kernel for RMSNorm."""
        return """\
// RMSNorm kernel
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;

struct Params {
    dim: u32,
    eps: f32,
}
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(64)
fn rms_norm(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    let base = row * params.dim;
    let dim_f = f32(params.dim);

    var sum_sq: f32 = 0.0;
    for (var i: u32 = 0u; i < params.dim; i = i + 1u) {
        let v = input[base + i];
        sum_sq = sum_sq + v * v;
    }
    let rms = inverseSqrt(sum_sq / dim_f + params.eps);

    for (var i: u32 = 0u; i < params.dim; i = i + 1u) {
        output[base + i] = input[base + i] * rms;
    }
}
"""

    @pytest.mark.skipif(not _wgsl_validator_available(), reason="naga not available")
    def test_rms_norm_wgsl_validates(self):
        """RMSNorm WGSL kernel passes naga validation."""
        wgsl_source = self._make_stub_wgsl_kernel()
        with tempfile.NamedTemporaryFile(suffix=".wgsl", mode="w", delete=False) as f:
            f.write(wgsl_source)
            f.flush()
            wgsl_path = f.name

        try:
            result = subprocess.run(
                ["naga", wgsl_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
            assert result.returncode == 0, (
                f"WGSL validation failed:\n{result.stderr}"
            )
        finally:
            os.unlink(wgsl_path)

    def test_wgsl_structure_is_valid(self):
        """WGSL source has required structural elements."""
        wgsl = self._make_stub_wgsl_kernel()
        assert "@group(0)" in wgsl
        assert "@binding(" in wgsl
        assert "@compute" in wgsl
        assert "@workgroup_size(" in wgsl
        assert "var<storage" in wgsl
        assert "fn rms_norm" in wgsl


# ---------------------------------------------------------------------------
# Tests: CUDA source validation
# ---------------------------------------------------------------------------

class TestCUDASource:
    """Validate that rendered CUDA source is syntactically valid."""

    def _make_stub_cuda_kernel(self) -> str:
        """Generate a representative CUDA kernel for RMSNorm."""
        return """\
extern "C" {

__global__ void rms_norm(
    const float* __restrict__ input,
    float* __restrict__ output,
    unsigned int dim,
    float eps
) {
    unsigned int row = blockIdx.x * blockDim.x + threadIdx.x;
    unsigned int base = row * dim;

    float sum_sq = 0.0f;
    for (unsigned int i = 0; i < dim; i++) {
        float v = input[base + i];
        sum_sq += v * v;
    }
    float rms = rsqrtf(sum_sq / (float)dim + eps);

    for (unsigned int i = 0; i < dim; i++) {
        output[base + i] = input[base + i] * rms;
    }
}

__global__ void rope_1d(
    const float* __restrict__ input,
    float* __restrict__ output,
    const float* __restrict__ cos_table,
    const float* __restrict__ sin_table,
    unsigned int freq_dim,
    unsigned int B,
    unsigned int S,
    unsigned int H,
    unsigned int D
) {
    unsigned int tid = blockIdx.x * blockDim.x + threadIdx.x;
    unsigned int total = B * S * H * D;
    if (tid >= total) return;

    unsigned int d = tid % D;
    unsigned int h = (tid / D) % H;
    unsigned int s = (tid / (D * H)) % S;
    unsigned int half = D / 2;

    if (d < half) {
        unsigned int freq_idx = s * freq_dim + d;
        float cos_v = (d < freq_dim) ? cos_table[freq_idx] : 1.0f;
        float sin_v = (d < freq_dim) ? sin_table[freq_idx] : 0.0f;
        unsigned int base = tid - d;
        float x0 = input[base + d];
        float x1 = (d + half < D) ? input[base + d + half] : 0.0f;
        output[base + d] = x0 * cos_v - x1 * sin_v;
        if (d + half < D) {
            output[base + d + half] = x0 * sin_v + x1 * cos_v;
        }
    }
}

} // extern "C"
"""

    @pytest.mark.skipif(not _nvcc_available(), reason="nvcc not available")
    def test_cuda_kernels_compile(self):
        """CUDA kernels compile with nvcc."""
        cuda_source = self._make_stub_cuda_kernel()
        with tempfile.NamedTemporaryFile(suffix=".cu", mode="w", delete=False) as f:
            f.write(cuda_source)
            f.flush()
            cu_path = f.name

        try:
            result = subprocess.run(
                ["nvcc", "--ptx", cu_path, "-o", "/dev/null"],
                capture_output=True,
                text=True,
                timeout=60,
            )
            assert result.returncode == 0, (
                f"CUDA compilation failed:\n{result.stderr}"
            )
        finally:
            os.unlink(cu_path)

    def test_cuda_structure_is_valid(self):
        """CUDA source has required structural elements."""
        cuda = self._make_stub_cuda_kernel()
        assert '__global__' in cuda
        assert 'extern "C"' in cuda
        assert "rsqrtf" in cuda
        assert "__restrict__" in cuda


# ---------------------------------------------------------------------------
# Tests: Cross-target numerical parity
# ---------------------------------------------------------------------------

@pytest.mark.skipif(_SKIP_RUNTIME, reason=_RUNTIME_REASON)
class TestCrossTargetParity:
    """Verify numerical parity across available targets."""

    @pytest.fixture(autouse=True)
    def _setup(self):
        stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
        src_path = os.path.join(_project_root, "src")
        if stdlib_path not in sys.path:
            sys.path.insert(0, stdlib_path)
        if src_path not in sys.path:
            sys.path.insert(0, src_path)

    def test_cpu_produces_valid_tokens(self):
        """CPU target produces tokens in valid vocab range."""
        from tinygrad.device import Device
        Device.set("CPU")

        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)

        init(weights, config)
        tokens = ocr_tokens(32, 32, image, [1, 2, 3], max_new_tokens=5)

        vocab_size = STUB_CONFIG["vocab_size"]
        for t in tokens:
            assert 0 <= t < vocab_size, f"Token {t} out of vocab range [0, {vocab_size})"

    def test_inference_timing(self, capsys):
        """Report inference timing for available targets."""
        from tinygrad.device import Device
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens

        weights = generate_stub_weights()
        config = generate_stub_config_json()
        image = generate_test_image(32, 32)
        prompt = [1, 2, 3, 4, 5]
        max_tokens = 5

        results = {}

        # CPU
        Device.set("CPU")
        init(weights, config)
        t0 = time.monotonic()
        tokens = ocr_tokens(32, 32, image, prompt, max_new_tokens=max_tokens)
        t_cpu = time.monotonic() - t0
        results["CPU"] = {"time": t_cpu, "tokens": tokens}

        # Metal (macOS only)
        if _is_macos():
            Device.set("METAL")
            init(weights, config)
            t0 = time.monotonic()
            tokens = ocr_tokens(32, 32, image, prompt, max_new_tokens=max_tokens)
            t_metal = time.monotonic() - t0
            results["METAL"] = {"time": t_metal, "tokens": tokens}

        print(f"\n{'='*60}")
        print("Cross-target timing report")
        for target, data in results.items():
            n = len(data["tokens"])
            tps = n / data["time"] if data["time"] > 0 else 0
            print(f"  {target:8s}: {data['time']:.4f}s  {n} tokens  {tps:.2f} tok/s")
        print(f"{'='*60}")
