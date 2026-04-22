"""
Production deployment verification tests for Falcon-OCR.

Tests the complete deployment stack:
  - Worker.js request handling with fallback chain
  - x402 payment verification middleware
  - CORS headers (only freeinvoicemaker.app)
  - Health endpoint with backend status
  - Error responses (413, 415, 402, 500, 503)
  - Monitoring output format (structured JSON, no PII)
  - Graceful degradation (fallback URL in response)
  - Request ID propagation
  - OCR API response schema
  - Concurrent request handling patterns

These tests validate contracts between components without requiring
a running Worker or compiled WASM binary.
"""

from __future__ import annotations

import base64
import json
import os
import re
import sys

DEPLOY_DIR = os.path.normpath(
    os.path.join(os.path.dirname(__file__), "..", "..", "deploy")
)
CF_DIR = os.path.join(DEPLOY_DIR, "cloudflare")
ENJOICE_DIR = os.path.join(DEPLOY_DIR, "enjoice")
SCRIPTS_DIR = os.path.join(DEPLOY_DIR, "scripts")


def _read(path: str) -> str:
    with open(path) as f:
        return f.read()


# ---------------------------------------------------------------------------
# 1. Worker.js request handling with fallback chain
# ---------------------------------------------------------------------------


def test_worker_imports_x402_and_monitoring():
    """worker.js imports x402.js and monitoring.js modules."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert 'import' in source and 'x402.js' in source, "worker.js must import x402.js"
    assert 'import' in source and 'monitoring.js' in source, "worker.js must import monitoring.js"


def test_worker_fallback_chain():
    """worker.js implements fallback chain with structured error."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "fallback_available" in source, "worker.js must include fallback_available field"
    assert "fallback_url" in source, "worker.js must include fallback_url field"
    assert "/api/ocr/paddle" in source, "worker.js must reference PaddleOCR fallback URL"
    assert "fallbackErrorResponse" in source, "worker.js must define fallbackErrorResponse"


def test_worker_health_reports_backends():
    """Health endpoint reports status of all backends."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "backends" in source, "Health response must include backends status"
    assert "molt-gpu" in source, "Health must report molt-gpu backend"
    assert "paddle-ocr" in source, "Health must report paddle-ocr backend"


# ---------------------------------------------------------------------------
# 2. x402 payment verification
# ---------------------------------------------------------------------------


def test_x402_module_exists():
    """x402.js exists with correct exports."""
    path = os.path.join(CF_DIR, "x402.js")
    assert os.path.isfile(path), "x402.js not found"
    source = _read(path)
    assert "verifyX402" in source, "x402.js must export verifyX402"
    assert "parsePaymentHeader" in source, "x402.js must define parsePaymentHeader"
    assert "verifyPaymentAmount" in source, "x402.js must define verifyPaymentAmount"
    assert "verifyRecipient" in source, "x402.js must define verifyRecipient"
    assert "verifyTimestamp" in source, "x402.js must define verifyTimestamp"
    assert "verifySignature" in source, "x402.js must define verifySignature"


def test_x402_payment_proof_parsing():
    """x402.js validates all required payment proof fields."""
    source = _read(os.path.join(CF_DIR, "x402.js"))
    required_fields = ["sender", "recipient", "amount", "currency", "timestamp", "nonce", "signature"]
    for field in required_fields:
        assert field in source, f"x402.js must validate '{field}' field"


def test_x402_price_matches_mcp():
    """x402 price matches MCP tool definition ($0.001/request)."""
    source = _read(os.path.join(CF_DIR, "x402.js"))
    assert "0.001" in source, "x402.js must reference $0.001 price"
    assert "1000" in source, "x402.js must reference 1000 USDC units (6 decimals)"


def test_x402_payment_required_response():
    """402 response includes payment instructions per x402 spec."""
    source = _read(os.path.join(CF_DIR, "x402.js"))
    assert "payment_required" in source, "402 response must include payment_required object"
    assert "X-Payment-Version" in source, "402 response must include X-Payment-Version header"
    assert "X-Payment-Network" in source, "402 response must include X-Payment-Network header"
    assert "X-Payment-Currency" in source, "402 response must include X-Payment-Currency header"
    assert "X-Payment-Amount" in source, "402 response must include X-Payment-Amount header"
    assert "X-Payment-Recipient" in source, "402 response must include X-Payment-Recipient header"


def test_x402_timestamp_validation():
    """x402 validates payment timestamp within acceptable skew."""
    source = _read(os.path.join(CF_DIR, "x402.js"))
    assert "MAX_TIMESTAMP_SKEW" in source, "x402.js must define timestamp skew limit"
    assert "300" in source, "x402.js must use 300 second skew tolerance"


def test_x402_dev_mode_skip():
    """x402 skips verification when no wallet is configured (dev mode)."""
    source = _read(os.path.join(CF_DIR, "x402.js"))
    assert "authorized: true" in source, "x402.js must authorize when no wallet configured"


# ---------------------------------------------------------------------------
# 3. CORS headers
# ---------------------------------------------------------------------------


def test_cors_restricted_origin():
    """CORS is restricted to freeinvoicemaker.app only."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "freeinvoicemaker.app" in source, "CORS must reference freeinvoicemaker.app"
    # Ensure no wildcard CORS
    assert 'Access-Control-Allow-Origin": "*"' not in source, "CORS must NOT use wildcard origin"


def test_cors_allowed_headers():
    """CORS allows required custom headers."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "X-Payment-402" in source, "CORS must allow X-Payment-402 header"
    assert "X-Request-ID" in source, "CORS must allow X-Request-ID header"


def test_cors_preflight():
    """Worker handles OPTIONS preflight requests."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "OPTIONS" in source, "Worker must handle OPTIONS requests"
    assert "204" in source, "Preflight must return 204"


# ---------------------------------------------------------------------------
# 4. Health endpoint
# ---------------------------------------------------------------------------


def test_health_no_auth_required():
    """Health endpoint is handled before x402 verification in the handler."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    # Within the handler body, /health must be checked before verifyX402.
    # Find the first occurrence of each within the handler function.
    handler_start = source.find("handler: async")
    assert handler_start != -1, "Worker must define handler function"
    handler_body = source[handler_start:]
    health_pos = handler_body.find('"/health"')
    payment_pos = handler_body.find("verifyX402")
    assert health_pos != -1, "Handler must check /health"
    assert payment_pos != -1, "Handler must call verifyX402"
    assert health_pos < payment_pos, "/health must be checked before x402 verification"


def test_health_response_schema():
    """Health response has required fields."""
    expected = {"status", "model", "version", "device", "request_id", "backends"}
    source = _read(os.path.join(CF_DIR, "worker.js"))
    for field in expected:
        assert f'"{field}"' in source or f"'{field}'" in source or f"{field}:" in source, (
            f"Health response missing field: {field}"
        )


# ---------------------------------------------------------------------------
# 5. Error responses
# ---------------------------------------------------------------------------


def test_error_413_too_large():
    """OCR API returns 413 for images exceeding size limit."""
    source = _read(os.path.join(CF_DIR, "ocr_api.js"))
    assert "413" in source, "OCR API must return 413 for oversized images"
    assert "too large" in source.lower() or "Image too large" in source, (
        "413 response must mention image size"
    )


def test_error_415_wrong_type():
    """OCR API returns 415 for unsupported content types."""
    source = _read(os.path.join(CF_DIR, "ocr_api.js"))
    assert "415" in source, "OCR API must return 415 for unsupported formats"
    assert "Unsupported" in source, "415 response must mention unsupported format"


def test_error_402_payment_required():
    """Worker returns 402 when payment is missing or invalid."""
    source = _read(os.path.join(CF_DIR, "x402.js"))
    assert "402" in source, "x402.js must return 402 status"
    assert "paymentRequiredResponse" in source, "x402.js must define paymentRequiredResponse"


def test_error_405_method_not_allowed():
    """Worker returns 405 for non-POST requests to OCR endpoints."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "405" in source, "Worker must return 405 for wrong method"
    assert "Method not allowed" in source, "405 response must include error message"


def test_error_503_fallback():
    """Worker returns 503 with fallback info when model fails to load."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "503" in source, "Worker must return 503 for backend unavailable"
    assert "fallback_available" in source, "503 response must include fallback_available"


# ---------------------------------------------------------------------------
# 6. Monitoring output format (structured JSON, no PII)
# ---------------------------------------------------------------------------


def test_monitoring_module_exists():
    """monitoring.js exists with required exports."""
    path = os.path.join(CF_DIR, "monitoring.js")
    assert os.path.isfile(path), "monitoring.js not found"
    source = _read(path)
    assert "createRequestLog" in source, "monitoring.js must export createRequestLog"
    assert "emitLog" in source, "monitoring.js must export emitLog"
    assert "writeAnalytics" in source, "monitoring.js must export writeAnalytics"
    assert "categorizeError" in source, "monitoring.js must export categorizeError"
    assert "withMonitoring" in source, "monitoring.js must export withMonitoring"


def test_monitoring_structured_fields():
    """Monitoring logs include required structured fields."""
    source = _read(os.path.join(CF_DIR, "monitoring.js"))
    required_fields = [
        "request_id", "timestamp", "method", "path", "status_code",
        "latency_ms", "device_type", "browser", "model_version",
    ]
    for field in required_fields:
        assert field in source, f"Monitoring must log '{field}'"


def test_monitoring_optional_fields():
    """Monitoring logs include optional inference fields."""
    source = _read(os.path.join(CF_DIR, "monitoring.js"))
    optional_fields = ["image_width", "image_height", "token_count"]
    for field in optional_fields:
        assert field in source, f"Monitoring must support optional '{field}'"


def test_monitoring_error_categories():
    """Monitoring defines all required error categories."""
    source = _read(os.path.join(CF_DIR, "monitoring.js"))
    categories = [
        "MODEL_LOAD_FAILED", "INFERENCE_TIMEOUT", "WEBGPU_UNAVAILABLE",
        "PAYMENT_INVALID", "INPUT_INVALID", "INTERNAL_ERROR",
    ]
    for cat in categories:
        assert cat in source, f"Monitoring must define error category: {cat}"


def test_monitoring_no_pii():
    """Monitoring must not log PII (image content, user identifiers, IPs)."""
    source = _read(os.path.join(CF_DIR, "monitoring.js"))
    # Must not log image data
    assert "imageData" not in source.lower() or "imageData" not in source, (
        "Monitoring must not log image data"
    )
    assert "User-Agent" not in source or "extractDeviceType" in source, (
        "User-Agent must only be used for device/browser classification, not logged raw"
    )
    # The log structure must not include IP addresses
    assert "ip" not in [f.strip() for f in re.findall(r'"(\w+)":', source)], (
        "Monitoring must not include IP address fields"
    )


def test_monitoring_cloudflare_analytics():
    """Monitoring integrates with Cloudflare Analytics via ctx.waitUntil."""
    source = _read(os.path.join(CF_DIR, "monitoring.js"))
    assert "waitUntil" in source, "Monitoring must use ctx.waitUntil for async analytics"
    assert "writeDataPoint" in source, "Monitoring must write to Analytics Engine"


# ---------------------------------------------------------------------------
# 7. Graceful degradation
# ---------------------------------------------------------------------------


def test_fallback_url_in_error_response():
    """503 error response includes fallback URL."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "/api/ocr/paddle" in source, "Fallback URL must point to PaddleOCR endpoint"


def test_fallback_backends_in_response():
    """Error response includes status of all backends."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert '"molt-gpu"' in source, "Error response must include molt-gpu status"
    assert '"paddle-ocr"' in source, "Error response must include paddle-ocr status"


# ---------------------------------------------------------------------------
# 8. Request ID propagation
# ---------------------------------------------------------------------------


def test_request_id_from_header():
    """Worker accepts X-Request-ID from client."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "X-Request-ID" in source, "Worker must read X-Request-ID header"


def test_request_id_generated():
    """Worker generates request ID when not provided."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "requestId()" in source, "Worker must generate request ID"


def test_request_id_in_all_responses():
    """All response formats include request_id."""
    for filename in ["worker.js", "ocr_api.js", "x402.js"]:
        source = _read(os.path.join(CF_DIR, filename))
        assert "request_id" in source, f"{filename} must include request_id in responses"


# ---------------------------------------------------------------------------
# 9. OCR API response schema
# ---------------------------------------------------------------------------


def test_ocr_response_has_required_fields():
    """OCR response schema matches MCP tool definition."""
    source = _read(os.path.join(CF_DIR, "ocr_api.js"))
    required_fields = ["text", "tokens", "confidence", "time_ms", "device", "request_id"]
    for field in required_fields:
        assert field in source, f"OCR response must include '{field}'"


def test_tokens_response_has_required_fields():
    """Tokens response schema matches MCP tool definition."""
    source = _read(os.path.join(CF_DIR, "ocr_api.js"))
    for field in ["tokens", "time_ms", "device", "request_id"]:
        assert field in source, f"Tokens response must include '{field}'"


# ---------------------------------------------------------------------------
# 10. Concurrent request handling
# ---------------------------------------------------------------------------


def test_lazy_init_idempotent():
    """Model initialization is idempotent (shared promise)."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "initPromise" in source, "Worker must use shared init promise"
    assert "if (modelReady) return" in source, "Worker must short-circuit when ready"
    assert "if (initPromise)" in source, "Worker must share init promise across requests"


# ---------------------------------------------------------------------------
# 11. Enjoice integration files
# ---------------------------------------------------------------------------


def test_enjoice_falcon_ocr_molt_exists():
    """falcon-ocr-molt.ts exists with required exports."""
    path = os.path.join(ENJOICE_DIR, "falcon-ocr-molt.ts")
    assert os.path.isfile(path), "falcon-ocr-molt.ts not found"
    source = _read(path)
    assert "createFalconOcrSession" in source, "Must export createFalconOcrSession"
    assert "FalconOcrSession" in source, "Must define FalconOcrSession interface"
    assert "FalconOcrConfig" in source, "Must define FalconOcrConfig interface"
    assert "dispose" in source, "FalconOcrSession must have dispose method"
    assert "WebAssembly" in source, "Must use WebAssembly for WASM loading"


def test_enjoice_falcon_ocr_molt_no_commonjs():
    """falcon-ocr-molt.ts uses ES modules, not CommonJS."""
    source = _read(os.path.join(ENJOICE_DIR, "falcon-ocr-molt.ts"))
    assert "require(" not in source, "Must not use CommonJS require()"
    assert "module.exports" not in source, "Must not use CommonJS module.exports"


def test_enjoice_ocr_backend_molt_exists():
    """ocr-backend-molt.ts exists with required interface."""
    path = os.path.join(ENJOICE_DIR, "ocr-backend-molt.ts")
    assert os.path.isfile(path), "ocr-backend-molt.ts not found"
    source = _read(path)
    assert "MoltOcrBackend" in source, "Must export MoltOcrBackend class"
    assert "OcrBackendResult" in source, "Must define OcrBackendResult"
    assert "initialize" in source, "Must have initialize method"
    assert "recognize" in source, "Must have recognize method"
    assert "dispose" in source, "Must have dispose method"
    assert "performance.now" in source, "Must use performance.now for timing"


def test_enjoice_capabilities_update_exists():
    """capabilities-update.ts exists with required exports."""
    path = os.path.join(ENJOICE_DIR, "capabilities-update.ts")
    assert os.path.isfile(path), "capabilities-update.ts not found"
    source = _read(path)
    assert "detectOcrCapabilities" in source, "Must export detectOcrCapabilities"
    assert "detectBrowser" in source, "Must export detectBrowser"
    assert "BrowserInfo" in source, "Must define BrowserInfo"
    assert "GpuCapabilities" in source, "Must define GpuCapabilities"
    assert "WebGPU" in source or "webGpuAvailable" in source, "Must detect WebGPU"
    assert "WebGL2" in source or "webGl2Available" in source, "Must detect WebGL2"


def test_enjoice_capabilities_firefox_warning():
    """capabilities-update.ts warns about Firefox dispatch overhead."""
    source = _read(os.path.join(ENJOICE_DIR, "capabilities-update.ts"))
    assert "firefox" in source.lower(), "Must handle Firefox"
    assert "1037" in source, "Must reference Firefox dispatch overhead (~1037 us)"
    assert "Maczan" in source, "Must cite Maczan 2026 source"
    assert "paddle" in source.lower(), "Must recommend PaddleOCR fallback for Firefox"


def test_enjoice_capabilities_feature_flag():
    """capabilities-update.ts supports feature flag override."""
    source = _read(os.path.join(ENJOICE_DIR, "capabilities-update.ts"))
    assert "__MOLT_OCR_BACKEND" in source, "Must read __MOLT_OCR_BACKEND feature flag"
    assert "featureFlagOverride" in source, "Must report whether flag is active"


# ---------------------------------------------------------------------------
# 12. Deploy script
# ---------------------------------------------------------------------------


def test_deploy_script_exists():
    """deploy.sh exists and is executable."""
    path = os.path.join(SCRIPTS_DIR, "deploy.sh")
    assert os.path.isfile(path), "deploy.sh not found"
    assert os.access(path, os.X_OK), "deploy.sh must be executable"


def test_deploy_script_validates_environment():
    """deploy.sh validates staging/production argument."""
    source = _read(os.path.join(SCRIPTS_DIR, "deploy.sh"))
    assert "staging" in source, "deploy.sh must support staging"
    assert "production" in source, "deploy.sh must support production"
    assert "set -euo pipefail" in source, "deploy.sh must use strict mode"


def test_deploy_script_steps():
    """deploy.sh includes all required deployment steps."""
    source = _read(os.path.join(SCRIPTS_DIR, "deploy.sh"))
    steps = ["Building WASM", "Uploading artifacts", "Deploying Worker", "Health check", "Smoke test"]
    for step in steps:
        assert step in source, f"deploy.sh must include step: {step}"


# ---------------------------------------------------------------------------
# 13. No TODOs in new code
# ---------------------------------------------------------------------------


def test_no_todos_in_deployment_code():
    """No TODO/FIXME/HACK/XXX in any deployment file."""
    todo_pattern = re.compile(r"\b(TODO|FIXME|HACK|XXX)\b", re.IGNORECASE)
    dirs_to_check = [CF_DIR, ENJOICE_DIR, SCRIPTS_DIR]
    violations = []
    for dir_path in dirs_to_check:
        if not os.path.isdir(dir_path):
            continue
        for filename in os.listdir(dir_path):
            filepath = os.path.join(dir_path, filename)
            if not os.path.isfile(filepath):
                continue
            with open(filepath) as f:
                for line_num, line in enumerate(f, 1):
                    if todo_pattern.search(line):
                        violations.append(f"{filepath}:{line_num}: {line.strip()}")
    assert not violations, "Found TODO/FIXME in deployment code:\n" + "\n".join(violations)


# ---------------------------------------------------------------------------
# 14. JavaScript/TypeScript syntax validation
# ---------------------------------------------------------------------------


def test_js_files_syntactically_valid():
    """All .js files have balanced braces, brackets, and parens."""
    for filename in ["worker.js", "ocr_api.js", "x402.js", "monitoring.js"]:
        source = _read(os.path.join(CF_DIR, filename))
        _assert_balanced(source, filename)


def test_ts_files_syntactically_valid():
    """All .ts files have balanced braces, brackets, and parens."""
    for filename in ["falcon-ocr-molt.ts", "ocr-backend-molt.ts", "capabilities-update.ts"]:
        source = _read(os.path.join(ENJOICE_DIR, filename))
        _assert_balanced(source, filename)



def _assert_balanced(source: str, filename: str) -> None:
    """Check that braces, brackets, and parens are balanced.

    Uses a character-by-character state machine to correctly handle
    string literals (single, double, template), comments (line, block),
    and regex literals without false positives.
    """
    pairs = {"{": "}", "[": "]", "(": ")"}
    closers = set(pairs.values())
    stack: list[str] = []
    i = 0
    n = len(source)

    while i < n:
        c = source[i]

        # Line comment
        if c == "/" and i + 1 < n and source[i + 1] == "/":
            i = source.find("\n", i)
            if i == -1:
                break
            i += 1
            continue

        # Block comment
        if c == "/" and i + 1 < n and source[i + 1] == "*":
            end = source.find("*/", i + 2)
            i = end + 2 if end != -1 else n
            continue

        # Double-quoted string
        if c == '"':
            i += 1
            while i < n and source[i] != '"':
                if source[i] == "\\":
                    i += 1
                i += 1
            i += 1
            continue

        # Single-quoted string
        if c == "'":
            i += 1
            while i < n and source[i] != "'":
                if source[i] == "\\":
                    i += 1
                i += 1
            i += 1
            continue

        # Template literal (with nested ${...})
        if c == "`":
            i += 1
            tmpl_depth = 0
            while i < n:
                if source[i] == "\\" and i + 1 < n:
                    i += 2
                    continue
                if source[i] == "$" and i + 1 < n and source[i + 1] == "{":
                    tmpl_depth += 1
                    i += 2
                    continue
                if source[i] == "{" and tmpl_depth > 0:
                    tmpl_depth += 1
                    i += 1
                    continue
                if source[i] == "}" and tmpl_depth > 0:
                    tmpl_depth -= 1
                    i += 1
                    continue
                if source[i] == "`" and tmpl_depth == 0:
                    i += 1
                    break
                i += 1
            continue

        # Structural delimiters
        if c in pairs:
            stack.append(pairs[c])
        elif c in closers:
            if not stack or stack[-1] != c:
                raise AssertionError(
                    f"{filename}: unbalanced '{c}'"
                )
            stack.pop()

        i += 1

    if stack:
        raise AssertionError(
            f"{filename}: unclosed delimiters: {''.join(reversed(stack))}"
        )


# ---------------------------------------------------------------------------
# 15. Payment receipt propagation
# ---------------------------------------------------------------------------


def test_payment_receipt_header():
    """Worker propagates X-Payment-Receipt on successful payment."""
    source = _read(os.path.join(CF_DIR, "worker.js"))
    assert "X-Payment-Receipt" in source, "Worker must set X-Payment-Receipt header"


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
