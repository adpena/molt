"""
Performance regression guard for Falcon-OCR Worker.

CI-safe checks that catch performance regressions:
  - /health p95 latency < 500ms
  - /ocr TTFB < 5s (with test image)
  - /wasm/falcon-ocr.wasm Content-Length < 10 MB
  - /weights/falcon-ocr-int4/config.json response < 2 KB

These are non-destructive read-only checks against the live deployment.
"""

import base64
import io
import os
import time
import unittest

import requests

WORKER_URL = os.environ.get(
    "FALCON_OCR_WORKER_URL",
    "https://falcon-ocr.adpena.workers.dev",
)
ORIGIN_HEADER = "https://freeinvoicemaker.app"


def _headers():
    return {
        "Origin": ORIGIN_HEADER,
    }


class TestPerformanceGuard(unittest.TestCase):
    """Performance regression guards for CI."""

    def test_health_p95_under_500ms(self):
        """GET /health 5 times, verify p95 < 500ms."""
        latencies = []
        for _ in range(5):
            start = time.time()
            resp = requests.get(
                f"{WORKER_URL}/health",
                headers=_headers(),
                timeout=10,
            )
            latency_ms = (time.time() - start) * 1000
            latencies.append(latency_ms)
            # Health should return 200 or 503 (loading), not 500
            self.assertIn(
                resp.status_code,
                (200, 503),
                f"Health returned unexpected status: {resp.status_code}",
            )
            # Must be JSON
            data = resp.json()
            self.assertIn("request_id", data)

        # p95 = 95th percentile (with 5 samples, this is the max minus outlier)
        latencies.sort()
        p95_idx = int(0.95 * len(latencies))
        p95 = latencies[min(p95_idx, len(latencies) - 1)]
        print(
            f"  /health latencies: "
            f"{[f'{latency:.0f}ms' for latency in latencies]}, p95={p95:.0f}ms"
        )
        self.assertLess(
            p95, 500, f"/health p95 latency {p95:.0f}ms exceeds 500ms threshold"
        )

    def test_ocr_ttfb_under_5s(self):
        """POST /ocr with a minimal test image, verify TTFB < 5s."""
        # Generate a minimal 100x100 white PNG
        try:
            from PIL import Image

            img = Image.new("RGB", (100, 100), "white")
            buf = io.BytesIO()
            img.save(buf, format="PNG")
            image_bytes = buf.getvalue()
        except ImportError:
            # Minimal valid 1x1 PNG (67 bytes)
            image_bytes = base64.b64decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4"
                "2mP8/58BAwAI/AL+hc2rNAAAAABJRU5ErkJggg=="
            )

        b64 = base64.b64encode(image_bytes).decode("ascii")
        payload = {"image": b64}

        start = time.time()
        resp = requests.post(
            f"{WORKER_URL}/ocr",
            json=payload,
            headers={
                "Content-Type": "application/json",
                "Origin": ORIGIN_HEADER,
            },
            timeout=30,
        )
        ttfb_ms = (time.time() - start) * 1000

        print(f"  /ocr TTFB: {ttfb_ms:.0f}ms, status: {resp.status_code}")

        # Accept 200 (success) or 503 (model loading) but not 500 (broken)
        self.assertIn(
            resp.status_code,
            (200, 503),
            f"OCR returned unexpected status: {resp.status_code}",
        )

        # Verify response is structured JSON with request_id
        data = resp.json()
        self.assertIn("request_id", data)

        # TTFB should be under 5 seconds even with cold start
        self.assertLess(
            ttfb_ms, 5000, f"/ocr TTFB {ttfb_ms:.0f}ms exceeds 5000ms threshold"
        )

    def test_wasm_binary_size_under_10mb(self):
        """GET /wasm/falcon-ocr.wasm, verify Content-Length < 10 MB."""
        resp = requests.head(
            f"{WORKER_URL}/wasm/falcon-ocr.wasm",
            headers=_headers(),
            timeout=10,
        )

        # May be 404 if not deployed yet, or 200
        if resp.status_code == 404:
            self.skipTest("WASM binary not deployed to R2 yet")

        self.assertEqual(
            resp.status_code, 200, f"WASM endpoint returned {resp.status_code}"
        )

        content_length = int(resp.headers.get("Content-Length", "0"))
        max_size = 10 * 1024 * 1024  # 10 MB

        print(f"  /wasm/falcon-ocr.wasm size: {content_length / (1024 * 1024):.2f} MB")
        self.assertGreater(content_length, 0, "WASM binary has zero Content-Length")
        self.assertLess(
            content_length,
            max_size,
            f"WASM binary {content_length} bytes exceeds 10 MB limit",
        )

    def test_weights_config_under_2kb(self):
        """GET /weights/falcon-ocr-int4/config.json, verify response < 2 KB."""
        resp = requests.get(
            f"{WORKER_URL}/weights/falcon-ocr-int4/config.json",
            headers=_headers(),
            timeout=10,
        )

        # May be 404 if weights not deployed
        if resp.status_code == 404:
            self.skipTest("INT4 config not deployed to R2 yet")

        self.assertEqual(
            resp.status_code, 200, f"Weights config returned {resp.status_code}"
        )

        # Verify it's valid JSON
        data = resp.json()
        self.assertIsInstance(data, dict)

        # Verify size
        body_size = len(resp.content)
        max_size = 2 * 1024  # 2 KB

        print(f"  /weights/falcon-ocr-int4/config.json size: {body_size} bytes")
        self.assertLess(
            body_size, max_size, f"Config response {body_size} bytes exceeds 2 KB limit"
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
