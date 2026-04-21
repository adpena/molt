"""
End-to-end correctness tests for the WebGPU matmul PoC and compute engine.

Validates:
1. The WGSL shader source in webgpu-engine.js contains correct tiled 16x16 matmul patterns
2. Shared memory (workgroup) declarations are present
3. All 6 compute ops are exported: matmul, softmax, rmsnorm, rope, add, mul
4. The webgpu-matmul.js PoC file is structurally valid
"""

import re
import subprocess
import sys
from pathlib import Path

import pytest

DEPLOY_BROWSER = Path(__file__).resolve().parent.parent.parent / "deploy" / "browser"
WEBGPU_ENGINE = DEPLOY_BROWSER / "webgpu-engine.js"
WEBGPU_MATMUL = DEPLOY_BROWSER / "webgpu-matmul.js"
WORKER_URL = "https://falcon-ocr.adpena.workers.dev"


@pytest.fixture(scope="module")
def webgpu_engine_source() -> str:
    """Read the webgpu-engine.js source."""
    assert WEBGPU_ENGINE.exists(), f"Missing: {WEBGPU_ENGINE}"
    return WEBGPU_ENGINE.read_text()


@pytest.fixture(scope="module")
def webgpu_matmul_source() -> str:
    """Read the webgpu-matmul.js source."""
    assert WEBGPU_MATMUL.exists(), f"Missing: {WEBGPU_MATMUL}"
    return WEBGPU_MATMUL.read_text()


class TestWebGPUShaderCorrectness:
    """Validate WGSL shader structure in webgpu-engine.js."""

    def test_matmul_tiled_16x16(self, webgpu_engine_source: str):
        """The matmul shader must use 16x16 tiling with correct workgroup_size."""
        # TILE_SIZE is interpolated via JS template literal: ${TILE_SIZE}
        assert "TILE_SIZE = 16" in webgpu_engine_source
        assert "@workgroup_size(${TILE_SIZE}, ${TILE_SIZE}, 1)" in webgpu_engine_source
        assert "fn molt_kernel(" in webgpu_engine_source

    def test_matmul_shared_memory(self, webgpu_engine_source: str):
        """The tiled matmul must declare workgroup shared memory tiles."""
        assert "var<workgroup> tile_a:" in webgpu_engine_source
        assert "var<workgroup> tile_b:" in webgpu_engine_source
        # Tile arrays are ${TILE_SIZE * TILE_SIZE} = 256 elements (via JS template)
        assert "array<f32, ${TILE_SIZE * TILE_SIZE}>" in webgpu_engine_source

    def test_matmul_storage_buffers(self, webgpu_engine_source: str):
        """Matmul shader must have 3 storage buffers + 1 uniform (dims)."""
        assert "@group(0) @binding(0) var<storage, read_write> buf0:" in webgpu_engine_source
        assert "@group(0) @binding(1) var<storage, read> buf1:" in webgpu_engine_source
        assert "@group(0) @binding(2) var<storage, read> buf2:" in webgpu_engine_source
        assert "@group(0) @binding(3) var<uniform> dims:" in webgpu_engine_source

    def test_matmul_dimensions_uniform(self, webgpu_engine_source: str):
        """The dimensions uniform must be vec4<u32> (M, K, N, padding)."""
        assert "dims: vec4<u32>" in webgpu_engine_source

    def test_softmax_entry_point(self, webgpu_engine_source: str):
        """Softmax shader must have the molt_softmax entry point."""
        assert "fn molt_softmax(" in webgpu_engine_source

    def test_rmsnorm_entry_point(self, webgpu_engine_source: str):
        """RMSNorm shader must have the molt_rms_norm entry point."""
        assert "fn molt_rms_norm(" in webgpu_engine_source

    def test_rope_entry_point(self, webgpu_engine_source: str):
        """RoPE shader must have the molt_rope entry point."""
        assert "fn molt_rope(" in webgpu_engine_source

    def test_add_entry_point(self, webgpu_engine_source: str):
        """Element-wise add shader must have the molt_add entry point."""
        assert "fn molt_add(" in webgpu_engine_source

    def test_mul_entry_point(self, webgpu_engine_source: str):
        """Element-wise mul shader must have the molt_mul entry point."""
        assert "fn molt_mul(" in webgpu_engine_source


class TestWebGPUEngineOps:
    """Validate all 6 compute ops are present and exported."""

    REQUIRED_OPS = ["matmul", "softmax", "rmsnorm", "rope", "add", "mul"]

    def test_all_ops_have_wgsl_constants(self, webgpu_engine_source: str):
        """Each op must have a corresponding WGSL constant."""
        expected_constants = [
            "MATMUL_WGSL",
            "SOFTMAX_WGSL",
            "RMSNORM_WGSL",
            "ROPE_WGSL",
            "ADD_WGSL",
            "MUL_WGSL",
        ]
        for const_name in expected_constants:
            assert f"const {const_name}" in webgpu_engine_source, (
                f"Missing WGSL constant: {const_name}"
            )

    def test_engine_class_exported(self, webgpu_engine_source: str):
        """WebGPUEngine class must be exported."""
        assert "export class WebGPUEngine" in webgpu_engine_source

    def test_engine_has_matmul_method(self, webgpu_engine_source: str):
        """The engine must expose a matmul method."""
        # Look for method definition (async matmul or matmul(...))
        assert re.search(r"(?:async\s+)?matmul\s*\(", webgpu_engine_source)

    def test_engine_has_destroy_method(self, webgpu_engine_source: str):
        """The engine must have a destroy/cleanup method."""
        assert re.search(r"destroy\s*\(", webgpu_engine_source)


class TestWebGPUMatmulPoC:
    """Validate the standalone WebGPU matmul PoC file."""

    def test_file_exists(self):
        """webgpu-matmul.js must exist."""
        assert WEBGPU_MATMUL.exists()

    def test_has_shader_source(self, webgpu_matmul_source: str):
        """PoC must contain a WGSL shader."""
        assert "@compute" in webgpu_matmul_source

    def test_has_gpu_device_request(self, webgpu_matmul_source: str):
        """PoC must request a GPU device."""
        assert "requestDevice" in webgpu_matmul_source or "requestAdapter" in webgpu_matmul_source


def _curl_available() -> bool:
    """Check if curl is available on the system."""
    try:
        subprocess.run(["curl", "--version"], capture_output=True, timeout=5)
        return True
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return False


class TestBrowserTestPageDeployment:
    """Validate the test page is accessible from the deployed Worker."""

    @pytest.mark.skipif(
        not _curl_available(),
        reason="curl not available",
    )
    def test_test_page_accessible(self):
        """GET /test should return 200 with HTML content."""
        result = subprocess.run(
            ["curl", "-s", "-w", "%{http_code}", "-o", "/dev/null", f"{WORKER_URL}/test"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert result.stdout.strip() == "200", f"Expected 200, got {result.stdout}"

    @pytest.mark.skipif(
        not _curl_available(),
        reason="curl not available",
    )
    def test_browser_js_accessible(self):
        """GET /browser/compute-engine.js should return 200."""
        result = subprocess.run(
            ["curl", "-s", "-w", "%{http_code}", "-o", "/dev/null",
             f"{WORKER_URL}/browser/compute-engine.js"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert result.stdout.strip() == "200", f"Expected 200, got {result.stdout}"

    @pytest.mark.skipif(
        not _curl_available(),
        reason="curl not available",
    )
    def test_webgpu_engine_js_accessible(self):
        """GET /browser/webgpu-engine.js should return 200."""
        result = subprocess.run(
            ["curl", "-s", "-w", "%{http_code}", "-o", "/dev/null",
             f"{WORKER_URL}/browser/webgpu-engine.js"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert result.stdout.strip() == "200", f"Expected 200, got {result.stdout}"
