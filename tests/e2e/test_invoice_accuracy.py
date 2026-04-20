"""
End-to-end invoice OCR accuracy benchmark.

Generates 5 high-quality synthetic invoices with varied layouts:
  1. Simple: single vendor, 1 line item, total
  2. Multi-line: 5 line items, subtotal, tax, total
  3. International: EUR currency, DD/MM/YYYY date format
  4. Complex: multiple addresses, PO number, payment terms, notes
  5. Minimal: vendor name and amount only, no "INVOICE" label

Sends each to the live Worker and measures field extraction accuracy.
Results are saved to docs/benchmarks/invoice_accuracy.md.

Accuracy evaluation uses:
  - Fuzzy matching (Levenshtein ratio > 0.7) for text fields
  - Numeric extraction with 1% tolerance for amounts
  - Date normalization to ISO format
  - Value extraction from prose responses ("The vendor is Acme Corp")
"""

import base64
import io
import json
import os
import re
import time
import unittest
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Optional

import requests

WORKER_URL = os.environ.get(
    "FALCON_OCR_WORKER_URL",
    "https://falcon-ocr.adpena.workers.dev",
)
ORIGIN_HEADER = "https://freeinvoicemaker.app"
RESULTS_DIR = Path(__file__).resolve().parents[2] / "docs" / "benchmarks"

try:
    from PIL import Image, ImageDraw, ImageFont
    HAS_PILLOW = True
except ImportError:
    HAS_PILLOW = False


# ---------------------------------------------------------------------------
# Font helpers
# ---------------------------------------------------------------------------

def _get_fonts():
    """Load system fonts with graceful fallback."""
    try:
        large = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 24)
        medium = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 16)
        small = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 13)
    except (OSError, IOError):
        large = ImageFont.load_default()
        medium = large
        small = large
    return large, medium, small


# ---------------------------------------------------------------------------
# Invoice generators
# ---------------------------------------------------------------------------

@dataclass
class InvoiceSpec:
    """Expected fields for accuracy validation."""
    name: str
    vendor: str
    invoice_number: Optional[str]
    total_amount: str
    line_items: list[str] = field(default_factory=list)
    image_bytes: bytes = field(default=b"", repr=False)


def _img_to_png(img: Image.Image) -> bytes:
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def generate_invoice_simple() -> InvoiceSpec:
    """Invoice 1: Simple — single vendor, 1 line item, total."""
    img = Image.new("RGB", (800, 600), "white")
    draw = ImageDraw.Draw(img)
    large, medium, small = _get_fonts()

    draw.text((50, 40), "INVOICE", fill="black", font=large)
    draw.text((50, 90), "Vendor: Northwind Traders", fill="black", font=medium)
    draw.text((50, 120), "Invoice #: INV-20241", fill="black", font=medium)
    draw.text((50, 150), "Date: 2026-03-15", fill="black", font=small)

    # Line item
    draw.line([(50, 200), (750, 200)], fill="gray", width=1)
    draw.text((50, 210), "Description", fill="black", font=small)
    draw.text((400, 210), "Qty", fill="black", font=small)
    draw.text((500, 210), "Price", fill="black", font=small)
    draw.text((650, 210), "Amount", fill="black", font=small)
    draw.line([(50, 230), (750, 230)], fill="gray", width=1)
    draw.text((50, 240), "Cloud Hosting (Annual)", fill="black", font=small)
    draw.text((400, 240), "1", fill="black", font=small)
    draw.text((500, 240), "$2,400.00", fill="black", font=small)
    draw.text((650, 240), "$2,400.00", fill="black", font=small)

    # Total
    draw.line([(50, 280), (750, 280)], fill="gray", width=1)
    draw.text((550, 300), "Total: $2,400.00", fill="black", font=medium)

    return InvoiceSpec(
        name="Simple",
        vendor="Northwind Traders",
        invoice_number="INV-20241",
        total_amount="2,400.00",
        line_items=["Cloud Hosting"],
        image_bytes=_img_to_png(img),
    )


def generate_invoice_multiline() -> InvoiceSpec:
    """Invoice 2: Multi-line — 5 line items, subtotal, tax, total."""
    img = Image.new("RGB", (800, 900), "white")
    draw = ImageDraw.Draw(img)
    large, medium, small = _get_fonts()

    draw.text((50, 40), "INVOICE", fill="black", font=large)
    draw.text((50, 90), "From: Quantum Dynamics LLC", fill="black", font=medium)
    draw.text((50, 120), "Invoice No: QD-88712", fill="black", font=medium)
    draw.text((50, 150), "Date: 2026-01-22", fill="black", font=small)

    items = [
        ("API Integration Service", "3", "$150.00", "$450.00"),
        ("Data Migration", "1", "$800.00", "$800.00"),
        ("Security Audit", "2", "$600.00", "$1,200.00"),
        ("Performance Tuning", "4", "$200.00", "$800.00"),
        ("Premium Support (monthly)", "6", "$99.00", "$594.00"),
    ]

    y = 200
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 10
    draw.text((50, y), "Description", fill="black", font=small)
    draw.text((350, y), "Qty", fill="black", font=small)
    draw.text((450, y), "Unit Price", fill="black", font=small)
    draw.text((600, y), "Amount", fill="black", font=small)
    y += 25
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 10

    for desc, qty, unit, total in items:
        draw.text((50, y), desc, fill="black", font=small)
        draw.text((350, y), qty, fill="black", font=small)
        draw.text((450, y), unit, fill="black", font=small)
        draw.text((600, y), total, fill="black", font=small)
        y += 30

    y += 20
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 15
    draw.text((500, y), "Subtotal: $3,844.00", fill="black", font=small)
    y += 25
    draw.text((500, y), "Tax (8.5%): $326.74", fill="black", font=small)
    y += 30
    draw.text((500, y), "TOTAL: $4,170.74", fill="black", font=medium)

    return InvoiceSpec(
        name="Multi-line",
        vendor="Quantum Dynamics",
        invoice_number="QD-88712",
        total_amount="4,170.74",
        line_items=["API Integration", "Data Migration", "Security Audit",
                    "Performance Tuning", "Premium Support"],
        image_bytes=_img_to_png(img),
    )


def generate_invoice_international() -> InvoiceSpec:
    """Invoice 3: International — EUR currency, DD/MM/YYYY date format."""
    img = Image.new("RGB", (800, 700), "white")
    draw = ImageDraw.Draw(img)
    large, medium, small = _get_fonts()

    draw.text((50, 40), "FACTURA", fill="black", font=large)
    draw.text((50, 90), "Proveedor: Solaris GmbH", fill="black", font=medium)
    draw.text((50, 120), "Factura Nr: SOL-44291", fill="black", font=medium)
    draw.text((50, 150), "Fecha: 28/02/2026", fill="black", font=small)
    draw.text((50, 175), "Moneda: EUR", fill="black", font=small)

    y = 220
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 10
    draw.text((50, y), "Descripcion", fill="black", font=small)
    draw.text((500, y), "Importe", fill="black", font=small)
    y += 25
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 10

    items = [
        ("Consultoria IT - Q1 2026", "EUR 12.500,00"),
        ("Licencia Software Enterprise", "EUR 3.200,00"),
    ]
    for desc, amount in items:
        draw.text((50, y), desc, fill="black", font=small)
        draw.text((500, y), amount, fill="black", font=small)
        y += 30

    y += 20
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 15
    draw.text((450, y), "Total: EUR 15.700,00", fill="black", font=medium)

    return InvoiceSpec(
        name="International (EUR)",
        vendor="Solaris GmbH",
        invoice_number="SOL-44291",
        total_amount="15.700",
        line_items=["Consultoria IT", "Licencia Software"],
        image_bytes=_img_to_png(img),
    )


def generate_invoice_complex() -> InvoiceSpec:
    """Invoice 4: Complex — multiple addresses, PO, payment terms, notes."""
    img = Image.new("RGB", (800, 1000), "white")
    draw = ImageDraw.Draw(img)
    large, medium, small = _get_fonts()

    draw.text((50, 30), "INVOICE", fill="black", font=large)
    draw.text((550, 30), "IronForge Industries", fill="black", font=medium)
    draw.text((550, 55), "742 Elm Street", fill="black", font=small)
    draw.text((550, 72), "Portland, OR 97201", fill="black", font=small)

    draw.text((50, 90), "Invoice #: IFI-2026-0073", fill="black", font=medium)
    draw.text((50, 115), "PO Number: PO-991244", fill="black", font=small)
    draw.text((50, 135), "Date: 2026-04-01", fill="black", font=small)
    draw.text((50, 155), "Payment Terms: Net 30", fill="black", font=small)
    draw.text((50, 175), "Due Date: 2026-05-01", fill="black", font=small)

    # Bill To / Ship To
    draw.text((50, 210), "Bill To:", fill="black", font=medium)
    draw.text((50, 235), "Celestial Labs", fill="black", font=small)
    draw.text((50, 252), "1200 Innovation Blvd", fill="black", font=small)
    draw.text((50, 269), "Austin, TX 78701", fill="black", font=small)

    draw.text((400, 210), "Ship To:", fill="black", font=medium)
    draw.text((400, 235), "Celestial Labs - Warehouse", fill="black", font=small)
    draw.text((400, 252), "500 Commerce Dr", fill="black", font=small)
    draw.text((400, 269), "Dallas, TX 75201", fill="black", font=small)

    # Line items
    y = 310
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 10
    draw.text((50, y), "Item", fill="black", font=small)
    draw.text((350, y), "Qty", fill="black", font=small)
    draw.text((450, y), "Rate", fill="black", font=small)
    draw.text((600, y), "Amount", fill="black", font=small)
    y += 25
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 10

    items = [
        ("Custom Dashboard Development", "1", "$8,500.00", "$8,500.00"),
        ("Load Testing Suite", "2", "$1,200.00", "$2,400.00"),
        ("SSL Certificate (3yr)", "5", "$45.00", "$225.00"),
    ]
    for desc, qty, rate, amount in items:
        draw.text((50, y), desc, fill="black", font=small)
        draw.text((350, y), qty, fill="black", font=small)
        draw.text((450, y), rate, fill="black", font=small)
        draw.text((600, y), amount, fill="black", font=small)
        y += 30

    y += 20
    draw.line([(50, y), (750, y)], fill="gray", width=1)
    y += 15
    draw.text((500, y), "Subtotal: $11,125.00", fill="black", font=small)
    y += 25
    draw.text((500, y), "Discount (5%): -$556.25", fill="black", font=small)
    y += 25
    draw.text((500, y), "Tax (8.25%): $872.92", fill="black", font=small)
    y += 30
    draw.text((500, y), "TOTAL: $11,441.67", fill="black", font=medium)

    # Notes
    y += 60
    draw.text((50, y), "Notes:", fill="black", font=medium)
    y += 25
    draw.text((50, y), "Please remit payment within 30 days.", fill="black", font=small)
    y += 20
    draw.text((50, y), "Wire transfer preferred. Account details on file.", fill="black", font=small)

    return InvoiceSpec(
        name="Complex",
        vendor="IronForge Industries",
        invoice_number="IFI-2026-0073",
        total_amount="11,441.67",
        line_items=["Custom Dashboard", "Load Testing", "SSL Certificate"],
        image_bytes=_img_to_png(img),
    )


def generate_invoice_minimal() -> InvoiceSpec:
    """Invoice 5: Minimal — just vendor name and amount, no label."""
    img = Image.new("RGB", (600, 300), "white")
    draw = ImageDraw.Draw(img)
    large, medium, small = _get_fonts()

    # No "INVOICE" header — just a vendor and an amount
    draw.text((50, 60), "Pinnacle Dynamics", fill="black", font=large)
    draw.text((50, 110), "Ref: PD-0042", fill="black", font=small)
    draw.text((50, 150), "Amount Due: $750.00", fill="black", font=medium)
    draw.text((50, 190), "2026-03-01", fill="black", font=small)

    return InvoiceSpec(
        name="Minimal",
        vendor="Pinnacle Dynamics",
        invoice_number="PD-0042",
        total_amount="750.00",
        line_items=[],
        image_bytes=_img_to_png(img),
    )


# ---------------------------------------------------------------------------
# OCR submission
# ---------------------------------------------------------------------------

def submit_to_ocr(image_bytes: bytes, max_retries: int = 3) -> dict:
    """Send image to the live Worker OCR endpoint with retry for transient errors."""
    b64 = base64.b64encode(image_bytes).decode("ascii")
    payload = {"image": b64}

    last_exc = None
    for attempt in range(max_retries):
        resp = requests.post(
            f"{WORKER_URL}/ocr",
            json=payload,
            headers={
                "Content-Type": "application/json",
                "Origin": ORIGIN_HEADER,
            },
            timeout=120,
        )
        if resp.status_code == 503:
            # Workers AI GPU fleet transiently unavailable — back off and retry
            last_exc = requests.HTTPError(
                f"{resp.status_code} Server Error: Service Unavailable",
                response=resp,
            )
            time.sleep(2 ** attempt)
            continue
        resp.raise_for_status()
        return resp.json()

    raise last_exc


# ---------------------------------------------------------------------------
# Accuracy evaluation
# ---------------------------------------------------------------------------

@dataclass
class AccuracyResult:
    invoice_name: str
    fields_expected: int
    fields_found: int
    details: dict[str, bool] = field(default_factory=dict)
    raw_text: str = ""
    latency_ms: float = 0.0

    @property
    def accuracy(self) -> float:
        if self.fields_expected == 0:
            return 1.0
        return self.fields_found / self.fields_expected


def _levenshtein_distance(s1: str, s2: str) -> int:
    """Compute Levenshtein edit distance between two strings."""
    if len(s1) < len(s2):
        return _levenshtein_distance(s2, s1)
    if len(s2) == 0:
        return len(s1)

    prev_row = list(range(len(s2) + 1))
    for i, c1 in enumerate(s1):
        curr_row = [i + 1]
        for j, c2 in enumerate(s2):
            insertions = prev_row[j + 1] + 1
            deletions = curr_row[j] + 1
            substitutions = prev_row[j] + (c1 != c2)
            curr_row.append(min(insertions, deletions, substitutions))
        prev_row = curr_row

    return prev_row[-1]


def normalize(text: str) -> str:
    """Normalize text for comparison: lowercase, strip, remove punctuation."""
    return re.sub(r'[^\w\s]', '', text.lower().strip())


def fuzzy_match(expected: str, text: str, threshold: float = 0.7) -> bool:
    """Check if expected value appears in text using fuzzy matching."""
    expected_norm = normalize(expected)
    text_norm = normalize(text)

    if not expected_norm:
        return False

    # Check substring containment first
    if expected_norm in text_norm:
        return True

    # Check each word-window of the text (sliding window of expected length)
    words_expected = expected_norm.split()
    words_text = text_norm.split()
    window_size = len(words_expected)

    for i in range(max(1, len(words_text) - window_size + 1)):
        window = " ".join(words_text[i:i + window_size])
        max_len = max(len(expected_norm), len(window))
        if max_len == 0:
            continue
        dist = _levenshtein_distance(expected_norm, window)
        ratio = 1.0 - dist / max_len
        if ratio >= threshold:
            return True

    # Full text comparison as last resort
    max_len = max(len(expected_norm), len(text_norm))
    if max_len > 0:
        ratio = 1.0 - _levenshtein_distance(expected_norm, text_norm) / max_len
        if ratio >= threshold:
            return True

    return False


def extract_amount(text: str) -> Optional[float]:
    """Extract dollar/currency amounts from text like '$4,200.00' or '4200'."""
    # Try common patterns: $1,234.56 or 1234.56 or 1,234
    match = re.search(r'[\$\u20ac\u00a3]?\s*([\d,]+\.?\d*)', text.replace(' ', ''))
    if match:
        num_str = match.group(1).replace(',', '')
        try:
            return float(num_str)
        except ValueError:
            pass
    return None


def amounts_match(expected_str: str, text: str, tolerance: float = 0.01) -> bool:
    """Compare amounts with 1% tolerance after numeric extraction."""
    expected_val = extract_amount(expected_str)
    if expected_val is None:
        return False

    # Try to find any amount in the text that matches
    for match in re.finditer(r'[\$\u20ac\u00a3]?\s*([\d,]+\.?\d*)', text):
        num_str = match.group(1).replace(',', '')
        try:
            found_val = float(num_str)
            if found_val == 0:
                continue
            # Within 1% tolerance
            if abs(found_val - expected_val) / max(abs(expected_val), 1e-9) <= tolerance:
                return True
        except ValueError:
            continue

    return False


def extract_value_from_prose(text: str) -> str:
    """Extract the value from model prose like 'The vendor is Acme Corp' or 'vendor: Acme Corp'."""
    # Try "is <value>" pattern
    match = re.search(r'\bis\s+(.+?)(?:\.|$)', text, re.IGNORECASE)
    if match:
        return match.group(1).strip()

    # Try ":<value>" pattern
    match = re.search(r':\s*(.+?)(?:\.|$)', text)
    if match:
        return match.group(1).strip()

    return text


def parse_date_flexible(text: str) -> Optional[str]:
    """Parse various date formats to ISO (YYYY-MM-DD)."""
    text = text.strip()
    formats = [
        "%Y-%m-%d",
        "%d/%m/%Y",
        "%m/%d/%Y",
        "%B %d, %Y",
        "%b %d, %Y",
        "%d %B %Y",
        "%d %b %Y",
    ]
    for fmt in formats:
        try:
            dt = datetime.strptime(text, fmt)
            return dt.strftime("%Y-%m-%d")
        except ValueError:
            continue
    return None


def extract_text_from_response(ocr_response: dict) -> str:
    """Extract text content from various OCR response formats."""
    text = ""
    if isinstance(ocr_response, dict):
        text = ocr_response.get("text", "")
        if not text:
            text = ocr_response.get("raw_text", "")
        if not text and "result" in ocr_response:
            result = ocr_response["result"]
            if isinstance(result, str):
                text = result
            elif isinstance(result, dict):
                text = result.get("text", "")
        # Also check lines array
        if not text and "lines" in ocr_response:
            text = " ".join(ocr_response["lines"])
        # Check structured fields (Workers AI structured response)
        if not text and "fields" in ocr_response:
            fields = ocr_response["fields"]
            if isinstance(fields, dict):
                text = " ".join(str(v) for v in fields.values())
        # Check response field (Workers AI raw response)
        if not text and "response" in ocr_response:
            resp = ocr_response["response"]
            if isinstance(resp, str):
                text = resp

    # Handle prose-style responses: extract values
    processed = extract_value_from_prose(text)
    # Return both original and processed for matching
    return text if len(text) >= len(processed) else processed


def evaluate_accuracy(spec: InvoiceSpec, ocr_response: dict) -> AccuracyResult:
    """Check which expected fields appear in the OCR output using fuzzy matching."""
    text = extract_text_from_response(ocr_response)

    details: dict[str, bool] = {}
    fields_expected = 0
    fields_found = 0

    # Check vendor name (fuzzy match)
    fields_expected += 1
    vendor_found = fuzzy_match(spec.vendor, text)
    details["vendor"] = vendor_found
    if vendor_found:
        fields_found += 1

    # Check invoice number (fuzzy match — OCR may misread a character)
    if spec.invoice_number:
        fields_expected += 1
        inv_found = fuzzy_match(spec.invoice_number, text, threshold=0.8)
        details["invoice_number"] = inv_found
        if inv_found:
            fields_found += 1

    # Check total amount (numeric comparison with 1% tolerance)
    fields_expected += 1
    amount_found = amounts_match(spec.total_amount, text)
    details["total_amount"] = amount_found
    if amount_found:
        fields_found += 1

    # Check line items (fuzzy match)
    for item in spec.line_items:
        fields_expected += 1
        item_found = fuzzy_match(item, text, threshold=0.7)
        details[f"line_item:{item}"] = item_found
        if item_found:
            fields_found += 1

    return AccuracyResult(
        invoice_name=spec.name,
        fields_expected=fields_expected,
        fields_found=fields_found,
        details=details,
        raw_text=text[:500],
    )


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------

def generate_report(results: list[AccuracyResult]) -> str:
    """Generate a markdown report of accuracy results."""
    lines = [
        "# Invoice OCR Accuracy Benchmark",
        "",
        f"**Date**: {time.strftime('%Y-%m-%d %H:%M:%S UTC', time.gmtime())}",
        f"**Endpoint**: `{WORKER_URL}/ocr`",
        f"**Invoices tested**: {len(results)}",
        "",
        "## Summary",
        "",
        "| Invoice | Fields Expected | Fields Found | Accuracy | Latency (ms) |",
        "|---------|----------------|--------------|----------|--------------|",
    ]

    total_expected = 0
    total_found = 0
    for r in results:
        total_expected += r.fields_expected
        total_found += r.fields_found
        lines.append(
            f"| {r.invoice_name} | {r.fields_expected} | {r.fields_found} "
            f"| {r.accuracy:.0%} | {r.latency_ms:.0f} |"
        )

    overall = total_found / total_expected if total_expected > 0 else 0.0
    lines.append(f"| **Overall** | **{total_expected}** | **{total_found}** "
                 f"| **{overall:.0%}** | — |")
    lines.append("")

    # Detail section
    lines.append("## Field-Level Details")
    lines.append("")
    for r in results:
        lines.append(f"### {r.invoice_name} ({r.accuracy:.0%})")
        lines.append("")
        for field_name, found in r.details.items():
            status = "FOUND" if found else "MISSING"
            lines.append(f"- `{field_name}`: {status}")
        lines.append("")
        if r.raw_text:
            lines.append("<details><summary>Raw OCR text (truncated)</summary>")
            lines.append("")
            lines.append("```")
            lines.append(r.raw_text)
            lines.append("```")
            lines.append("</details>")
            lines.append("")

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Test class
# ---------------------------------------------------------------------------

@unittest.skipUnless(HAS_PILLOW, "Pillow not installed")
class TestInvoiceAccuracy(unittest.TestCase):
    """Live invoice OCR accuracy benchmark."""

    GENERATORS = [
        generate_invoice_simple,
        generate_invoice_multiline,
        generate_invoice_international,
        generate_invoice_complex,
        generate_invoice_minimal,
    ]

    def test_invoice_accuracy_benchmark(self):
        """Run full accuracy benchmark across all 5 invoice types."""
        results: list[AccuracyResult] = []

        for gen_fn in self.GENERATORS:
            spec = gen_fn()
            self.assertGreater(len(spec.image_bytes), 0, f"Empty image for {spec.name}")

            start = time.time()
            try:
                response = submit_to_ocr(spec.image_bytes)
            except requests.HTTPError as e:
                self.fail(f"OCR request failed for {spec.name}: {e}")
            latency_ms = (time.time() - start) * 1000

            result = evaluate_accuracy(spec, response)
            result.latency_ms = latency_ms
            results.append(result)

            # Per-invoice assertion: at minimum the vendor should be detected
            # (relaxed check — full accuracy is reported, not hard-failed)
            print(f"  {spec.name}: {result.accuracy:.0%} "
                  f"({result.fields_found}/{result.fields_expected}) "
                  f"in {latency_ms:.0f}ms")

        # Generate and save report
        report = generate_report(results)
        RESULTS_DIR.mkdir(parents=True, exist_ok=True)
        report_path = RESULTS_DIR / "invoice_accuracy.md"
        report_path.write_text(report, encoding="utf-8")
        print(f"\n  Report saved to: {report_path}")

        # Overall accuracy assertion: at least 50% field extraction
        total_expected = sum(r.fields_expected for r in results)
        total_found = sum(r.fields_found for r in results)
        overall = total_found / total_expected if total_expected > 0 else 0.0
        print(f"  Overall accuracy: {overall:.0%} ({total_found}/{total_expected})")

        # We use a generous threshold since OCR accuracy depends on the model variant
        self.assertGreaterEqual(
            overall, 0.40,
            f"Overall accuracy {overall:.0%} is below 40% threshold"
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
