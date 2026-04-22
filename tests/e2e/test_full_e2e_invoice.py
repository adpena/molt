"""
Full end-to-end invoice OCR test against the LIVE deployed Worker.

Exercises the complete pipeline: image generation -> base64 encode -> HTTP POST
to Cloudflare Worker -> OCR inference -> response validation.

Tests raw text extraction, structured JSON extraction, template detection,
and cache behavior.
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

ALLOWED_ORIGIN = "https://freeinvoicemaker.app"

try:
    from PIL import Image, ImageDraw, ImageFont

    HAS_PILLOW = True
except ImportError:
    HAS_PILLOW = False


def create_test_invoice() -> str:
    """Generate a realistic synthetic invoice and return base64-encoded PNG.

    The invoice includes:
    - Company header (ACME CORPORATION)
    - Invoice number (#INV-2026-0042)
    - Issue/due dates
    - Bill-to address
    - Line items with quantities, rates, amounts
    - Subtotal, tax, and total
    """
    img = Image.new("RGB", (800, 1100), "white")
    draw = ImageDraw.Draw(img)

    try:
        font_large = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 24)
        font_medium = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 16)
        font_small = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 13)
    except (OSError, IOError):
        font_large = ImageFont.load_default()
        font_medium = ImageFont.load_default()
        font_small = ImageFont.load_default()

    # --- Header ---
    draw.text((50, 40), "ACME CORPORATION", fill="black", font=font_large)
    draw.text((50, 70), "123 Business Ave, Suite 400", fill="gray", font=font_small)
    draw.text((50, 90), "San Francisco, CA 94102", fill="gray", font=font_small)
    draw.text((550, 40), "INVOICE", fill="black", font=font_large)
    draw.text((550, 70), "#INV-2026-0042", fill="black", font=font_medium)

    # --- Dates ---
    draw.text((50, 160), "Issue Date: April 20, 2026", fill="black", font=font_medium)
    draw.text((50, 185), "Due Date: May 20, 2026", fill="black", font=font_medium)
    draw.text((50, 210), "Payment Terms: Net 30", fill="black", font=font_medium)

    # --- Bill To ---
    draw.text((400, 160), "Bill To:", fill="gray", font=font_small)
    draw.text((400, 185), "Widget Corp", fill="black", font=font_medium)
    draw.text((400, 210), "456 Client St, Floor 2", fill="black", font=font_small)
    draw.text((400, 230), "New York, NY 10001", fill="black", font=font_small)

    # --- Line items header ---
    draw.line([(50, 280), (750, 280)], fill="black")
    draw.text((50, 290), "DESCRIPTION", fill="gray", font=font_small)
    draw.text((450, 290), "QTY", fill="gray", font=font_small)
    draw.text((550, 290), "RATE", fill="gray", font=font_small)
    draw.text((650, 290), "AMOUNT", fill="gray", font=font_small)
    draw.line([(50, 315), (750, 315)], fill="gray")

    # --- Line items ---
    items = [
        ("Website Redesign", "1", "$4,200.00", "$4,200.00"),
        ("SEO Optimization", "3", "$800.00", "$2,400.00"),
        ("Content Writing", "5", "$150.00", "$750.00"),
        ("Logo Design", "1", "$1,500.00", "$1,500.00"),
    ]
    y = 330
    for desc, qty, rate, amount in items:
        draw.text((50, y), desc, fill="black", font=font_medium)
        draw.text((450, y), qty, fill="black", font=font_medium)
        draw.text((550, y), rate, fill="black", font=font_medium)
        draw.text((650, y), amount, fill="black", font=font_medium)
        y += 30

    # --- Totals ---
    draw.line([(400, y + 20), (750, y + 20)], fill="gray")
    draw.text((450, y + 35), "Subtotal:", fill="black", font=font_medium)
    draw.text((650, y + 35), "$8,850.00", fill="black", font=font_medium)
    draw.text((450, y + 60), "Tax (8.25%):", fill="black", font=font_medium)
    draw.text((650, y + 60), "$730.13", fill="black", font=font_medium)
    draw.line([(400, y + 90), (750, y + 90)], fill="black", width=2)
    draw.text((450, y + 100), "TOTAL:", fill="black", font=font_large)
    draw.text((630, y + 100), "$9,580.13", fill="black", font=font_large)

    # --- Footer ---
    draw.text((50, 900), "Payment Method: Wire Transfer", fill="gray", font=font_small)
    draw.text((50, 920), "Bank: First National Bank", fill="gray", font=font_small)
    draw.text((50, 940), "Account: XXXX-XXXX-4521", fill="gray", font=font_small)
    draw.text((50, 980), "Thank you for your business!", fill="black", font=font_medium)

    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return base64.b64encode(buf.getvalue()).decode()


@unittest.skipUnless(HAS_PILLOW, "Pillow not installed")
class TestFullE2EInvoice(unittest.TestCase):
    """Full pipeline test against the live Cloudflare Worker."""

    @classmethod
    def setUpClass(cls):
        cls.invoice_b64 = create_test_invoice()

    def _post_ocr(self, path: str = "/ocr", **extra_json) -> requests.Response:
        """Helper: POST image to the Worker with correct headers."""
        payload = {"image": self.invoice_b64, "format": "image/png"}
        payload.update(extra_json)
        return requests.post(
            f"{WORKER_URL}{path}",
            headers={
                "Origin": ALLOWED_ORIGIN,
                "Content-Type": "application/json",
            },
            json=payload,
            timeout=30,
        )

    def test_ocr_endpoint_returns_text(self):
        """POST /ocr returns 200 with text containing key invoice fields."""
        resp = self._post_ocr()
        self.assertEqual(
            resp.status_code,
            200,
            f"Expected 200, got {resp.status_code}: {resp.text[:200]}",
        )
        data = resp.json()

        # Must have text or tokens
        text = data.get("text", "")
        tokens = data.get("tokens", [])
        self.assertTrue(
            len(text) > 0 or len(tokens) > 0,
            "Expected non-empty text or tokens from OCR",
        )

        if text:
            text_upper = text.upper()
            # Verify key fields were extracted
            self.assertTrue(
                any(word in text_upper for word in ["INVOICE", "ACME", "TOTAL"]),
                f"OCR text missing key invoice fields. Got: {text[:500]}",
            )

    def test_ocr_extracts_invoice_number(self):
        """OCR output should contain the invoice number."""
        resp = self._post_ocr()
        self.assertEqual(resp.status_code, 200)
        text = resp.json().get("text", "").upper()
        if text:
            self.assertTrue(
                "INV-2026-0042" in text or "0042" in text,
                f"Invoice number not found in OCR text: {text[:300]}",
            )

    def test_ocr_extracts_amounts(self):
        """OCR output should contain monetary amounts."""
        resp = self._post_ocr()
        self.assertEqual(resp.status_code, 200)
        text = resp.json().get("text", "")
        if text:
            # At least one amount should be visible
            self.assertTrue(
                any(amt in text for amt in ["9,580", "4,200", "8,850"]),
                f"No monetary amounts found in OCR text: {text[:500]}",
            )

    def test_structured_ocr_returns_invoice_object(self):
        """POST /ocr/structured returns a parsed invoice JSON with required fields."""
        resp = self._post_ocr(path="/ocr/structured")
        self.assertEqual(
            resp.status_code, 200, f"Got {resp.status_code}: {resp.text[:200]}"
        )
        data = resp.json()

        invoice = data.get("invoice")
        self.assertIsNotNone(
            invoice, f"No 'invoice' key in response: {list(data.keys())}"
        )
        self.assertIsInstance(invoice, dict)

        # Required schema fields
        for field in ("vendor", "invoice_number", "total", "currency", "items"):
            self.assertIn(field, invoice, f"Missing field '{field}' in invoice object")

        self.assertIsInstance(invoice["items"], list)
        self.assertGreater(len(invoice["items"]), 0, "Items list should not be empty")

    def test_structured_ocr_accuracy(self):
        """Structured extraction produces accurate values."""
        resp = self._post_ocr(path="/ocr/structured")
        if resp.status_code != 200:
            self.skipTest(f"Structured endpoint returned {resp.status_code}")

        invoice = resp.json().get("invoice", {})

        if invoice.get("vendor"):
            self.assertIn("acme", invoice["vendor"].lower())
        if invoice.get("total") and invoice["total"] > 0:
            self.assertAlmostEqual(
                invoice["total"],
                9580.13,
                delta=200,
                msg="Total should be approximately $9,580.13",
            )
        if invoice.get("currency"):
            self.assertIn(invoice["currency"].upper(), ("USD", "$"))

    def test_template_extract(self):
        """POST /ocr/template extracts fields based on a template schema."""
        template = {
            "fields": [
                {"name": "vendor", "type": "string"},
                {"name": "invoice_number", "type": "string"},
                {"name": "total", "type": "number"},
                {"name": "due_date", "type": "string"},
            ]
        }
        resp = self._post_ocr(path="/ocr/template", template=template)
        # Template endpoint may not exist yet — skip gracefully
        if resp.status_code == 404:
            self.skipTest("Template endpoint not deployed yet")
        self.assertEqual(
            resp.status_code, 200, f"Got {resp.status_code}: {resp.text[:200]}"
        )
        data = resp.json()
        self.assertIn("fields", data)

    def test_cache_hit(self):
        """Second identical request should be served from cache (faster response)."""
        # First request — cold
        t0 = time.time()
        resp1 = self._post_ocr()
        cold_ms = (time.time() - t0) * 1000
        self.assertEqual(resp1.status_code, 200)

        # Second request — same payload, should be cached
        t0 = time.time()
        resp2 = self._post_ocr()
        warm_ms = (time.time() - t0) * 1000
        self.assertEqual(resp2.status_code, 200)

        # Verify responses are identical (cache hit)
        self.assertEqual(
            resp1.json(), resp2.json(), "Cached response should match original"
        )

        # Cache hit should be faster (allow generous tolerance for network jitter)
        # If cold was > 2s, warm should be noticeably faster
        if cold_ms > 2000:
            self.assertLess(
                warm_ms,
                cold_ms * 0.8,
                f"Cache miss? cold={cold_ms:.0f}ms warm={warm_ms:.0f}ms",
            )

    def test_cors_headers(self):
        """Response includes correct CORS headers for the allowed origin."""
        resp = self._post_ocr()
        self.assertEqual(resp.status_code, 200)
        acao = resp.headers.get("Access-Control-Allow-Origin", "")
        self.assertTrue(
            acao == ALLOWED_ORIGIN or acao == "*",
            f"Expected CORS header for {ALLOWED_ORIGIN}, got: {acao}",
        )

    def test_invalid_image_returns_error(self):
        """Sending garbage base64 should return a clear error, not 500."""
        resp = requests.post(
            f"{WORKER_URL}/ocr",
            headers={
                "Origin": ALLOWED_ORIGIN,
                "Content-Type": "application/json",
            },
            json={"image": "not_valid_base64!!!", "format": "image/png"},
            timeout=15,
        )
        # Should be 400 (bad request) not 500 (server error)
        self.assertIn(
            resp.status_code, (400, 422), f"Expected 4xx, got {resp.status_code}"
        )

    def test_missing_image_field_returns_error(self):
        """Missing 'image' field should return 400."""
        resp = requests.post(
            f"{WORKER_URL}/ocr",
            headers={
                "Origin": ALLOWED_ORIGIN,
                "Content-Type": "application/json",
            },
            json={"format": "image/png"},
            timeout=15,
        )
        self.assertIn(resp.status_code, (400, 422))

    def test_health_check(self):
        """GET /health returns 200 with status."""
        resp = requests.get(f"{WORKER_URL}/health", timeout=5)
        self.assertEqual(resp.status_code, 200)
        data = resp.json()
        self.assertIn("status", data)


if __name__ == "__main__":
    unittest.main()
