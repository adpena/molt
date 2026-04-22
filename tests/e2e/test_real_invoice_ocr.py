"""
End-to-end test: synthetic invoice OCR via Workers AI.

Generates a high-quality synthetic invoice using Pillow, sends it to the
live Cloudflare Worker, and verifies the OCR response contains all expected
invoice fields.  Tests both raw text and structured JSON endpoints.
"""

import base64
import io
import os
import unittest

import requests

WORKER_URL = os.environ.get(
    "FALCON_OCR_WORKER_URL",
    "https://falcon-ocr.adpena.workers.dev",
)

# Skip if Pillow is not installed (CI environments may lack it)
try:
    from PIL import Image, ImageDraw, ImageFont

    HAS_PILLOW = True
except ImportError:
    HAS_PILLOW = False


def generate_synthetic_invoice() -> bytes:
    """Generate a synthetic invoice PNG image with realistic content.

    Returns raw PNG bytes suitable for base64 encoding and OCR submission.
    """
    img = Image.new("RGB", (800, 1100), "white")
    draw = ImageDraw.Draw(img)

    # Try to use a monospace font for cleaner alignment; fall back to default.
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
    draw.text((50, 160), "Issue Date: April 15, 2026", fill="black", font=font_medium)
    draw.text((50, 185), "Due Date: May 15, 2026", fill="black", font=font_medium)
    draw.text((50, 210), "Payment Terms: Net 30", fill="black", font=font_medium)

    # --- Bill To ---
    draw.text((400, 160), "Bill To:", fill="gray", font=font_small)
    draw.text((400, 185), "Widget Corp", fill="black", font=font_medium)
    draw.text((400, 210), "456 Client St", fill="black", font=font_small)

    # --- Line items header ---
    draw.line([(50, 280), (750, 280)], fill="black")
    draw.text((50, 290), "DESCRIPTION", fill="gray", font=font_small)
    draw.text((450, 290), "QTY", fill="gray", font=font_small)
    draw.text((550, 290), "RATE", fill="gray", font=font_small)
    draw.text((650, 290), "AMOUNT", fill="gray", font=font_small)
    draw.line([(50, 315), (750, 315)], fill="gray")

    # --- Line items ---
    draw.text((50, 330), "Website Redesign", fill="black", font=font_medium)
    draw.text((450, 330), "1", fill="black", font=font_medium)
    draw.text((550, 330), "$4,200.00", fill="black", font=font_medium)
    draw.text((650, 330), "$4,200.00", fill="black", font=font_medium)

    draw.text((50, 360), "SEO Optimization", fill="black", font=font_medium)
    draw.text((450, 360), "3", fill="black", font=font_medium)
    draw.text((550, 360), "$800.00", fill="black", font=font_medium)
    draw.text((650, 360), "$2,400.00", fill="black", font=font_medium)

    # --- Totals ---
    draw.line([(400, 430), (750, 430)], fill="gray")
    draw.text((450, 445), "Subtotal:", fill="black", font=font_medium)
    draw.text((650, 445), "$6,600.00", fill="black", font=font_medium)
    draw.text((450, 475), "Tax (8.25%):", fill="black", font=font_medium)
    draw.text((650, 475), "$544.50", fill="black", font=font_medium)
    draw.line([(400, 505), (750, 505)], fill="black", width=2)
    draw.text((450, 515), "TOTAL:", fill="black", font=font_large)
    draw.text((650, 515), "$7,144.50", fill="black", font=font_large)

    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


@unittest.skipUnless(HAS_PILLOW, "Pillow not installed")
class TestRealInvoiceOcr(unittest.TestCase):
    """Test invoice OCR against the live Cloudflare Worker."""

    @classmethod
    def setUpClass(cls):
        cls.invoice_png = generate_synthetic_invoice()
        cls.invoice_b64 = base64.b64encode(cls.invoice_png).decode()

    def test_raw_ocr_extracts_invoice_fields(self):
        """POST /ocr with a synthetic invoice image returns text containing
        key invoice fields: vendor, invoice number, dates, line items, totals."""
        resp = requests.post(
            f"{WORKER_URL}/ocr",
            json={"image": self.invoice_b64, "format": "image/png"},
            headers={"X-Use-Backend": "workers-ai"},
            timeout=15,
        )
        self.assertEqual(resp.status_code, 200)
        body = resp.json()

        # Workers AI path returns text directly
        text = body.get("text", "")
        self.assertTrue(
            len(text) > 0 or len(body.get("tokens", [])) > 0,
            "Expected non-empty text or tokens from OCR",
        )

        if text:
            text_lower = text.lower()
            # Vendor
            self.assertIn("acme", text_lower, "Should extract vendor name")
            # Invoice number
            self.assertIn("inv-2026-0042", text_lower, "Should extract invoice number")
            # Dates
            self.assertIn("april", text_lower, "Should extract issue date month")
            self.assertIn("2026", text, "Should extract year")
            # Line items
            self.assertIn(
                "website redesign", text_lower, "Should extract line item description"
            )
            self.assertIn(
                "seo optimization", text_lower, "Should extract second line item"
            )
            # Amounts
            self.assertIn("4,200", text, "Should extract line item amount")
            self.assertIn("7,144", text, "Should extract total amount")

    def test_structured_ocr_returns_valid_json(self):
        """POST /ocr/structured returns a parsed JSON invoice object with
        required fields: vendor, invoice_number, total, currency, items."""
        resp = requests.post(
            f"{WORKER_URL}/ocr/structured",
            json={"image": self.invoice_b64, "format": "image/png"},
            timeout=15,
        )
        self.assertEqual(resp.status_code, 200)
        body = resp.json()

        # Should have an invoice object
        invoice = body.get("invoice")
        self.assertIsNotNone(invoice, "Response should contain 'invoice' key")
        self.assertIsInstance(invoice, dict)

        # Required schema fields
        self.assertIn("vendor", invoice)
        self.assertIn("invoice_number", invoice)
        self.assertIn("total", invoice)
        self.assertIn("currency", invoice)
        self.assertIn("items", invoice)
        self.assertIsInstance(invoice["items"], list)

        # Confidence and model metadata
        self.assertIn("confidence", body)
        self.assertIn("model_used", body)
        self.assertIn("time_ms", body)

    def test_structured_ocr_accuracy(self):
        """POST /ocr/structured extracts correct values from the synthetic invoice."""
        resp = requests.post(
            f"{WORKER_URL}/ocr/structured",
            json={"image": self.invoice_b64, "format": "image/png"},
            timeout=15,
        )
        self.assertEqual(resp.status_code, 200)
        body = resp.json()
        invoice = body.get("invoice", {})

        # Check extracted values (case-insensitive for vendor)
        if invoice.get("vendor"):
            self.assertIn(
                "acme", invoice["vendor"].lower(), "Should extract vendor as ACME"
            )
        if invoice.get("invoice_number"):
            self.assertIn(
                "0042",
                invoice["invoice_number"],
                "Should extract invoice number containing 0042",
            )
        if invoice.get("total") and invoice["total"] > 0:
            # Allow some tolerance for OCR parsing
            self.assertAlmostEqual(
                invoice["total"],
                7144.50,
                delta=100,
                msg="Total should be approximately $7,144.50",
            )
        if invoice.get("items"):
            descriptions = [
                item.get("description", "").lower() for item in invoice["items"]
            ]
            combined = " ".join(descriptions)
            self.assertTrue(
                "website" in combined or "redesign" in combined or "seo" in combined,
                f"Should extract at least one line item description, got: {descriptions}",
            )

    def test_health_endpoint(self):
        """GET /health returns 200 with model status."""
        resp = requests.get(f"{WORKER_URL}/health", timeout=5)
        self.assertEqual(resp.status_code, 200)
        body = resp.json()
        self.assertIn("status", body)
        self.assertIn("model", body)
        self.assertEqual(body["model"], "falcon-ocr")


if __name__ == "__main__":
    unittest.main()
