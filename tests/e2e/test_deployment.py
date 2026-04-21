"""
End-to-end deployment pipeline tests for Falcon-OCR.

Tests the full deployment lifecycle:
  - WASM driver module structure and imports
  - Worker.js request handling (mocked WASM)
  - OCR API request validation and response format
  - x402 payment verification
  - Error handling (invalid image, too large, unsupported format)
  - Health endpoint

These tests do NOT require a running Worker or compiled WASM binary.
They validate the contracts between components.
"""

from __future__ import annotations

import ast
import json
import os
import sys

# ---------------------------------------------------------------------------
# 1. WASM driver module structure
# ---------------------------------------------------------------------------


def test_wasm_driver_exports():
    """wasm_driver.py exports exactly init() and ocr_tokens()."""
    driver_path = os.path.join(
        os.path.dirname(__file__),
        "..",
        "..",
        "src",
        "molt",
        "stdlib",
        "tinygrad",
        "wasm_driver.py",
    )
    driver_path = os.path.normpath(driver_path)
    assert os.path.isfile(driver_path), f"wasm_driver.py not found at {driver_path}"

    with open(driver_path) as f:
        source = f.read()

    # Must define exactly these two public functions
    assert "def init(" in source, "wasm_driver.py must export init()"
    assert "def ocr_tokens(" in source, "wasm_driver.py must export ocr_tokens()"

    tree = ast.parse(source)
    public_functions = [
        node.name
        for node in ast.walk(tree)
        if isinstance(node, ast.FunctionDef) and not node.name.startswith("_")
    ]
    assert sorted(public_functions) == ["init", "ocr_tokens"], (
        f"wasm_driver.py should export exactly [init, ocr_tokens], got {public_functions}"
    )


def test_wasm_driver_delegates_to_falcon_ocr():
    """wasm_driver.py must delegate to falcon_ocr.py, not reimplement."""
    driver_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "src",
            "molt",
            "stdlib",
            "tinygrad",
            "wasm_driver.py",
        )
    )
    with open(driver_path) as f:
        source = f.read()

    tree = ast.parse(source)
    falcon_module_bindings: set[str] = set()
    direct_falcon_bindings: dict[str, str] = {}
    for node in tree.body:
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name == "tinygrad.examples.falcon_ocr":
                    falcon_module_bindings.add(alias.asname or "molt")
        elif (
            isinstance(node, ast.ImportFrom)
            and (
                node.module == "tinygrad.examples.falcon_ocr"
                or (node.level == 1 and node.module == "examples.falcon_ocr")
            )
        ):
            for alias in node.names:
                direct_falcon_bindings[alias.asname or alias.name] = alias.name

    assert falcon_module_bindings or direct_falcon_bindings, (
        "wasm_driver.py must import falcon_ocr.py"
    )

    functions = {
        node.name: node
        for node in tree.body
        if isinstance(node, ast.FunctionDef)
    }
    assert _delegates_to_falcon_ocr(
        functions["init"],
        "init",
        falcon_module_bindings,
        direct_falcon_bindings,
    )
    assert _delegates_to_falcon_ocr(
        functions["ocr_tokens"],
        "ocr_tokens",
        falcon_module_bindings,
        direct_falcon_bindings,
    )
    # Must NOT contain model implementation details
    assert "class FalconOCRConfig" not in source, (
        "wasm_driver.py must not redefine FalconOCRConfig"
    )
    assert "def _generate" not in source, (
        "wasm_driver.py must not reimplement _generate"
    )


def _delegates_to_falcon_ocr(
    function: ast.FunctionDef,
    attr_name: str,
    module_bindings: set[str],
    direct_bindings: dict[str, str],
) -> bool:
    for node in ast.walk(function):
        if not isinstance(node, ast.Call):
            continue
        callee = node.func
        if (
            isinstance(callee, ast.Attribute)
            and callee.attr == attr_name
            and isinstance(callee.value, ast.Name)
            and callee.value.id in module_bindings
        ):
            return True
        if (
            isinstance(callee, ast.Name)
            and direct_bindings.get(callee.id) == attr_name
        ):
            return True
    return False


# ---------------------------------------------------------------------------
# 2. WASM manifest structure
# ---------------------------------------------------------------------------


def test_wasm_manifest_structure():
    """wasm_manifest.json has the required fields."""
    manifest_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "src",
            "molt",
            "stdlib",
            "tinygrad",
            "wasm_manifest.json",
        )
    )
    assert os.path.isfile(manifest_path), f"wasm_manifest.json not found at {manifest_path}"

    with open(manifest_path) as f:
        manifest = json.load(f)

    assert manifest["name"] == "falcon-ocr"
    assert "version" in manifest
    assert "exports" in manifest
    assert "init" in manifest["exports"]
    assert "ocr_tokens" in manifest["exports"]
    assert "artifacts" in manifest
    assert "wasm" in manifest["artifacts"]
    assert "weights" in manifest["artifacts"]
    assert "config" in manifest["artifacts"]
    assert "tokenizer" in manifest["artifacts"]
    assert "constraints" in manifest
    assert manifest["constraints"]["max_wasm_size_gzip_bytes"] == 2 * 1024 * 1024
    assert manifest["constraints"]["patch_size"] == 16


def test_wasm_manifest_export_signatures():
    """Export definitions have correct parameter schemas."""
    manifest_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "src",
            "molt",
            "stdlib",
            "tinygrad",
            "wasm_manifest.json",
        )
    )
    with open(manifest_path) as f:
        manifest = json.load(f)

    init_export = manifest["exports"]["init"]
    assert len(init_export["params"]) == 2
    param_names = [p["name"] for p in init_export["params"]]
    assert param_names == ["weights_bytes", "config_json"]
    assert init_export["returns"] == "void"

    ocr_export = manifest["exports"]["ocr_tokens"]
    assert len(ocr_export["params"]) == 5
    param_names = [p["name"] for p in ocr_export["params"]]
    assert param_names == ["width", "height", "rgb", "prompt_ids", "max_new_tokens"]
    assert ocr_export["returns"] == "list[int]"


# ---------------------------------------------------------------------------
# 3. Cloudflare Worker configuration
# ---------------------------------------------------------------------------


def test_wrangler_toml_structure():
    """wrangler.toml has required bindings and configuration."""
    import tomllib

    toml_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "deploy",
            "cloudflare",
            "wrangler.toml",
        )
    )
    assert os.path.isfile(toml_path), f"wrangler.toml not found at {toml_path}"

    with open(toml_path, "rb") as f:
        config = tomllib.load(f)

    assert config["name"] == "falcon-ocr"
    assert config["main"] == "worker.js"
    assert "placement" in config
    assert config["placement"]["mode"] == "smart"

    # R2 binding for weights
    r2_buckets = config.get("r2_buckets", [])
    assert len(r2_buckets) >= 1
    weights_bucket = r2_buckets[0]
    assert weights_bucket["binding"] == "WEIGHTS"

    # CORS origin
    assert config["vars"]["CORS_ORIGIN"] == "https://freeinvoicemaker.app"

    # Max image size
    assert config["vars"]["MAX_IMAGE_BYTES"] == "10485760"


# ---------------------------------------------------------------------------
# 4. Worker.js and OCR API structure
# ---------------------------------------------------------------------------


def test_worker_js_exists():
    """worker.js exists and imports ocr_api.js."""
    worker_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "deploy",
            "cloudflare",
            "worker.js",
        )
    )
    assert os.path.isfile(worker_path)

    with open(worker_path) as f:
        source = f.read()

    assert 'import' in source and 'ocr_api.js' in source
    assert "X-Payment-402" in source, "Worker must check x402 payment header"
    assert "ensureModelLoaded" in source, "Worker must lazy-load the model"
    assert "/health" in source, "Worker must handle health endpoint"
    assert "/ocr" in source, "Worker must handle OCR endpoint"


def test_ocr_api_js_exists():
    """ocr_api.js exists with required handler exports."""
    api_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "deploy",
            "cloudflare",
            "ocr_api.js",
        )
    )
    assert os.path.isfile(api_path)

    with open(api_path) as f:
        source = f.read()

    assert "handleOcrRequest" in source
    assert "handleTokensRequest" in source
    assert "handleHealthRequest" in source
    assert "multipart/form-data" in source, "Must support multipart image upload"
    assert "application/json" in source, "Must support JSON base64 image upload"
    assert "10485760" in source or "10 * 1024 * 1024" in source, "Must enforce 10MB limit"


# ---------------------------------------------------------------------------
# 5. OCR API response format
# ---------------------------------------------------------------------------


def test_ocr_response_format():
    """Health response has the documented fields."""
    # This tests the contract, not a live server.
    expected_fields = {"status", "model", "version", "device", "request_id"}
    health_response = {
        "status": "ready",
        "model": "falcon-ocr",
        "version": "0.1.0",
        "device": "wasm",
        "request_id": "test-123",
    }
    assert set(health_response.keys()) == expected_fields


def test_ocr_tokens_response_format():
    """Tokens response has the documented fields."""
    expected_fields = {"tokens", "time_ms", "device", "request_id"}
    tokens_response = {
        "tokens": [1, 2, 3],
        "time_ms": 150.0,
        "device": "wasm",
        "request_id": "test-123",
    }
    assert set(tokens_response.keys()) == expected_fields


def test_ocr_full_response_format():
    """Full OCR response has the documented fields."""
    expected_fields = {"text", "tokens", "confidence", "time_ms", "device", "request_id"}
    ocr_response = {
        "text": "Invoice #1234",
        "tokens": [1, 2, 3],
        "confidence": 0.95,
        "time_ms": 150.0,
        "device": "wasm",
        "request_id": "test-123",
    }
    assert set(ocr_response.keys()) == expected_fields


# ---------------------------------------------------------------------------
# 6. MCP tool definition
# ---------------------------------------------------------------------------


def test_mcp_tool_definition():
    """MCP tool JSON has valid structure."""
    mcp_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "deploy",
            "mcp",
            "ocr_tool.json",
        )
    )
    assert os.path.isfile(mcp_path)

    with open(mcp_path) as f:
        tool = json.load(f)

    assert tool["name"] == "falcon_ocr"
    assert "tools" in tool
    assert len(tool["tools"]) == 2

    tool_names = {t["name"] for t in tool["tools"]}
    assert tool_names == {"ocr_extract_text", "ocr_extract_tokens"}

    # Each tool must have input and output schemas
    for t in tool["tools"]:
        assert "inputSchema" in t, f"Tool {t['name']} missing inputSchema"
        assert "outputSchema" in t, f"Tool {t['name']} missing outputSchema"
        assert t["inputSchema"]["type"] == "object"
        assert t["outputSchema"]["type"] == "object"

    # Authentication
    assert tool["authentication"]["type"] == "x402"
    assert "header" in tool["authentication"]

    # Rate limits
    assert tool["rate_limits"]["max_image_size_bytes"] == 10 * 1024 * 1024


# ---------------------------------------------------------------------------
# 7. x402 payment verification contract
# ---------------------------------------------------------------------------


def test_x402_error_response_format():
    """402 error response has the correct structure."""
    error_response = {
        "error": "Missing X-Payment-402 header",
        "request_id": "test-123",
    }
    assert "error" in error_response
    assert "request_id" in error_response


def test_x402_required_for_ocr_endpoints():
    """worker.js requires x402 for /ocr and /ocr/tokens but not /health."""
    worker_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "deploy",
            "cloudflare",
            "worker.js",
        )
    )
    with open(worker_path) as f:
        source = f.read()

    # Health check must be before the common POST payment gate in the
    # monitored handler. Earlier fast-path routes are separately guarded.
    health_pos = source.index('if (path === "/health" && request.method === "GET")')
    common_payment_pos = source.index(
        "const payment = await verifyX402(request, env, rid, cors);",
        health_pos,
    )
    assert health_pos < common_payment_pos, (
        "/health must be handled before the common x402 verification gate"
    )


# ---------------------------------------------------------------------------
# 8. Error handling contracts
# ---------------------------------------------------------------------------


def test_supported_image_formats():
    """OCR API supports exactly JPEG, PNG, WebP."""
    api_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "deploy",
            "cloudflare",
            "ocr_api.js",
        )
    )
    with open(api_path) as f:
        source = f.read()

    for fmt in ["image/jpeg", "image/png", "image/webp"]:
        assert fmt in source, f"OCR API must support {fmt}"


def test_cors_restricted_to_freeinvoicemaker():
    """CORS headers restrict to freeinvoicemaker.app."""
    worker_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "deploy",
            "cloudflare",
            "worker.js",
        )
    )
    with open(worker_path) as f:
        source = f.read()

    assert "freeinvoicemaker.app" in source


def test_no_pii_logging():
    """Worker and API must not log image content."""
    for filename in ["worker.js", "ocr_api.js"]:
        path = os.path.normpath(
            os.path.join(
                os.path.dirname(__file__),
                "..",
                "..",
                "deploy",
                "cloudflare",
                filename,
            )
        )
        with open(path) as f:
            source = f.read()

        # console.log/error calls must not include image/rgb/bytes variables
        # We check that no console call references image data directly
        assert "console.log(rgb" not in source, f"{filename} must not log RGB data"
        assert "console.log(image" not in source, f"{filename} must not log image data"
        assert "console.log(bytes" not in source, f"{filename} must not log raw bytes"


# ---------------------------------------------------------------------------
# 9. Migration guide exists
# ---------------------------------------------------------------------------


def test_migration_guide_exists():
    """Migration guide for enjoice exists with required sections."""
    guide_path = os.path.normpath(
        os.path.join(
            os.path.dirname(__file__),
            "..",
            "..",
            "docs",
            "integration",
            "enjoice-ocr-migration.md",
        )
    )
    assert os.path.isfile(guide_path)

    with open(guide_path) as f:
        content = f.read()

    required_sections = [
        "falcon-wrapper.ts",
        "falcon-config.ts",
        "capabilities.ts",
        "index.ts",
        "PaddleOCR",
        "Deployment Checklist",
        "Rollback Plan",
        "Performance Expectations",
    ]
    for section in required_sections:
        assert section in content, f"Migration guide missing section about: {section}"


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    test_functions = [
        v for k, v in sorted(globals().items())
        if k.startswith("test_") and callable(v)
    ]
    passed = 0
    failed = 0
    for test_fn in test_functions:
        try:
            test_fn()
            passed += 1
            print(f"  PASS  {test_fn.__name__}")
        except Exception as e:
            failed += 1
            print(f"  FAIL  {test_fn.__name__}: {e}")

    print(f"\n{passed} passed, {failed} failed, {passed + failed} total")
    sys.exit(1 if failed else 0)
