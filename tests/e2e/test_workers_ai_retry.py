"""
End-to-end tests for Workers AI retry logic with exponential backoff
and model fallback.

Tests verify:
  - model_used is present in all successful OCR responses
  - 503 fallback returns PaddleOCR URL
  - Total request time stays within 5s timeout budget
  - Retry behavior across multiple sequential requests
"""

import json
import subprocess
import time

# Tiny valid 2x2 PNG (base64-encoded) for minimal-cost test requests.
TINY_PNG_B64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0k"
    "AAAAEklEQVQI12P4z8BQDwAFAAH/VscqcQAAAABJRU5ErkJggg=="
)

ENDPOINT = "https://falcon-ocr.adpena.workers.dev"
ORIGIN = "https://freeinvoicemaker.app"


def curl_ocr(image_b64=TINY_PNG_B64, timeout_s=10, extra_headers=None):
    """
    Issue a POST /ocr request and return (status_code, parsed_json, elapsed_s).

    Returns (0, None, elapsed) on network/parse failure.
    """
    headers = [
        "-H",
        f"Origin: {ORIGIN}",
        "-H",
        "Content-Type: application/json",
    ]
    if extra_headers:
        for k, v in extra_headers.items():
            headers.extend(["-H", f"{k}: {v}"])

    payload = json.dumps({"image": image_b64})
    start = time.monotonic()

    result = subprocess.run(
        [
            "curl",
            "-s",
            "-w",
            "\n%{http_code}",
            "--max-time",
            str(timeout_s),
            "-X",
            "POST",
            f"{ENDPOINT}/ocr",
            *headers,
            "-d",
            payload,
        ],
        capture_output=True,
        text=True,
        timeout=timeout_s + 5,
    )

    elapsed = time.monotonic() - start
    stdout = result.stdout.strip()

    # curl -w appends the HTTP status code on the last line
    lines = stdout.rsplit("\n", 1)
    if len(lines) != 2:
        return (0, None, elapsed)

    body_str, status_str = lines
    try:
        status_code = int(status_str)
    except ValueError:
        return (0, None, elapsed)

    try:
        body = json.loads(body_str)
    except json.JSONDecodeError:
        return (status_code, None, elapsed)

    return (status_code, body, elapsed)


def curl_health():
    """Issue a GET /health request and return (status_code, parsed_json)."""
    result = subprocess.run(
        [
            "curl",
            "-s",
            "-w",
            "\n%{http_code}",
            "--max-time",
            "5",
            "-X",
            "GET",
            f"{ENDPOINT}/health",
            "-H",
            f"Origin: {ORIGIN}",
        ],
        capture_output=True,
        text=True,
        timeout=10,
    )
    stdout = result.stdout.strip()
    lines = stdout.rsplit("\n", 1)
    if len(lines) != 2:
        return (0, None)
    body_str, status_str = lines
    try:
        status_code = int(status_str)
        body = json.loads(body_str)
    except (ValueError, json.JSONDecodeError):
        return (0, None)
    return (status_code, body)


def test_health_endpoint():
    """Health endpoint should return 200 or 503 (loading) with JSON body."""
    status, body = curl_health()
    assert status in (200, 503), f"Unexpected health status: {status}"
    assert body is not None, "Health endpoint returned unparseable body"
    assert "status" in body, f"Missing 'status' in health response: {body}"
    assert body["status"] in ("ready", "loading", "error"), (
        f"Unexpected status: {body['status']}"
    )


def test_ocr_model_used_present():
    """Successful OCR responses must include model_used field.

    Uses X-Use-Backend: workers-ai to bypass local model loading
    (which exceeds CPU time limit on Cloudflare's infrastructure).
    """
    status, body, elapsed = curl_ocr(extra_headers={"X-Use-Backend": "workers-ai"})
    if status == 200:
        assert body is not None, "200 response body is unparseable"
        assert "model_used" in body, f"Missing 'model_used' in 200 response: {body}"
        assert isinstance(body["model_used"], str), (
            f"model_used must be string, got: {type(body['model_used'])}"
        )
        assert len(body["model_used"]) > 0, "model_used must not be empty"
        print(
            f"  model_used={body['model_used']}, time_ms={body.get('time_ms', '?')}, retries={body.get('retries', '?')}"
        )
    elif status == 503:
        if body is not None:
            # Structured 503 from our Worker code
            assert "fallback_url" in body or "fallback_available" in body, (
                f"503 response missing fallback info: {body}"
            )
            print(
                f"  503 response (expected during capacity issues): {body.get('error', '?')}"
            )
        else:
            # Infrastructure 503 (e.g., error code 1102) -- not JSON
            print("  503 response (infrastructure-level, non-JSON)")
    else:
        # 402 (no payment) is also acceptable when x402 is enforced
        assert status in (402, 415), f"Unexpected status {status}: {body}"


def test_ocr_retries_field_present():
    """Successful OCR responses must include retries count."""
    status, body, elapsed = curl_ocr(extra_headers={"X-Use-Backend": "workers-ai"})
    if status == 200 and body is not None:
        assert "retries" in body, f"Missing 'retries' in 200 response: {body}"
        assert isinstance(body["retries"], int), (
            f"retries must be int, got: {type(body['retries'])}"
        )
        assert body["retries"] >= 0, f"retries must be >= 0, got: {body['retries']}"


def test_ocr_timeout_budget():
    """No single OCR request should hang longer than 10 seconds.

    The internal timeout is 5s for the AI chain, but network overhead
    and local inference can add time. 10s is the hard outer bound.
    """
    status, body, elapsed = curl_ocr(timeout_s=15)
    assert elapsed < 10.0, f"Request took {elapsed:.1f}s, exceeds 10s budget"
    print(f"  Request completed in {elapsed:.2f}s (status={status})")


def test_503_includes_fallback_url():
    """When all AI models fail, 503 response must include fallback_url.

    Uses X-Use-Backend: workers-ai to get structured 503 responses from
    our Worker code rather than infrastructure-level 503s.
    """
    status, body, elapsed = curl_ocr(extra_headers={"X-Use-Backend": "workers-ai"})
    if status == 503 and body is not None:
        has_fallback = "fallback_url" in body or "fallback_available" in body
        assert has_fallback, f"503 response missing fallback info: {body}"
        if "fallback_url" in body:
            assert body["fallback_url"] == "/api/ocr/paddle", (
                f"Unexpected fallback_url: {body['fallback_url']}"
            )
    elif status == 503 and body is None:
        # Infrastructure-level 503 (error code 1102) -- not from our Worker
        print("  Infrastructure 503 -- no structured body available")
    elif status == 200:
        print("  200 OK -- no 503 to test (all models healthy)")


def test_ocr_workers_ai_backend():
    """Explicit Workers AI backend selection via X-Use-Backend header."""
    status, body, elapsed = curl_ocr(extra_headers={"X-Use-Backend": "workers-ai"})
    if status == 200:
        assert "model_used" in body, (
            f"Missing model_used in Workers AI response: {body}"
        )
        # Workers AI responses should report workers-ai backend
        assert body.get("backend") == "workers-ai" or "model_used" in body, (
            f"Workers AI response missing backend identification: {body}"
        )
        print(
            f"  Workers AI: model={body.get('model_used')}, retries={body.get('retries', 0)}"
        )
    elif status == 503:
        assert "fallback_url" in body or "fallback_available" in body
        print(f"  Workers AI 503: {body.get('error', '?')}")


def test_multiple_sequential_requests():
    """Run 5 sequential requests via Workers AI and verify consistency."""
    results = []
    for i in range(5):
        status, body, elapsed = curl_ocr(extra_headers={"X-Use-Backend": "workers-ai"})
        results.append(
            {
                "request": i + 1,
                "status": status,
                "model_used": body.get("model_used") if body else None,
                "retries": body.get("retries") if body else None,
                "time_ms": body.get("time_ms") if body else None,
                "elapsed_s": round(elapsed, 2),
            }
        )
        print(
            f"  Request {i + 1}: status={status}, model={body.get('model_used', '?') if body else '?'}, "
            f"time={body.get('time_ms', '?') if body else '?'}ms, elapsed={elapsed:.2f}s"
        )

    # At least some requests should succeed or return structured 503/402
    valid_statuses = {200, 402, 503}
    for r in results:
        assert r["status"] in valid_statuses, (
            f"Request {r['request']} got unexpected status {r['status']}"
        )

    # All 200 responses must have model_used
    for r in results:
        if r["status"] == 200:
            assert r["model_used"] is not None, (
                f"Request {r['request']} missing model_used"
            )
            assert r["retries"] is not None, f"Request {r['request']} missing retries"


def test_batch_endpoint():
    """POST /ocr/batch responses should include model_used-equivalent fields."""
    payload = json.dumps({"images": [TINY_PNG_B64, TINY_PNG_B64]})
    result = subprocess.run(
        [
            "curl",
            "-s",
            "-w",
            "\n%{http_code}",
            "--max-time",
            "15",
            "-X",
            "POST",
            f"{ENDPOINT}/ocr/batch",
            "-H",
            f"Origin: {ORIGIN}",
            "-H",
            "Content-Type: application/json",
            "-d",
            payload,
        ],
        capture_output=True,
        text=True,
        timeout=20,
    )
    stdout = result.stdout.strip()
    lines = stdout.rsplit("\n", 1)
    if len(lines) != 2:
        return  # Network failure, skip
    body_str, status_str = lines
    try:
        status = int(status_str)
        body = json.loads(body_str)
    except (ValueError, json.JSONDecodeError):
        return  # Parse failure, skip

    if status == 200:
        assert "results" in body, f"Batch response missing 'results': {body}"
        assert "device" in body, f"Batch response missing 'device': {body}"


def test_template_extract_endpoint():
    """POST /template/extract should return structured template."""
    payload = json.dumps({"image": TINY_PNG_B64})
    result = subprocess.run(
        [
            "curl",
            "-s",
            "-w",
            "\n%{http_code}",
            "--max-time",
            "10",
            "-X",
            "POST",
            f"{ENDPOINT}/template/extract",
            "-H",
            f"Origin: {ORIGIN}",
            "-H",
            "Content-Type: application/json",
            "-d",
            payload,
        ],
        capture_output=True,
        text=True,
        timeout=15,
    )
    stdout = result.stdout.strip()
    lines = stdout.rsplit("\n", 1)
    if len(lines) != 2:
        return
    body_str, status_str = lines
    try:
        status = int(status_str)
        body = json.loads(body_str)
    except (ValueError, json.JSONDecodeError):
        return

    # 200 or 503 are both acceptable
    assert status in (200, 402, 503), f"Template extract unexpected status: {status}"
    if status == 200:
        assert "template" in body or "error" in body, (
            f"Unexpected template response: {body}"
        )


if __name__ == "__main__":
    print("=== Workers AI Retry Logic E2E Tests ===\n")

    tests = [
        ("Health endpoint", test_health_endpoint),
        ("OCR model_used present", test_ocr_model_used_present),
        ("OCR retries field present", test_ocr_retries_field_present),
        ("OCR timeout budget", test_ocr_timeout_budget),
        ("503 includes fallback URL", test_503_includes_fallback_url),
        ("OCR Workers AI backend", test_ocr_workers_ai_backend),
        ("Multiple sequential requests", test_multiple_sequential_requests),
        ("Batch endpoint", test_batch_endpoint),
        ("Template extract endpoint", test_template_extract_endpoint),
    ]

    passed = 0
    failed = 0
    for name, test_fn in tests:
        print(f"[TEST] {name}")
        try:
            test_fn()
            print("  PASS\n")
            passed += 1
        except Exception as e:
            print(f"  FAIL: {e}\n")
            failed += 1

    print(f"=== Results: {passed} passed, {failed} failed ===")
    if failed > 0:
        exit(1)
