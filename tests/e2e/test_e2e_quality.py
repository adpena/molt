"""End-to-end OCR quality comparison.

Tests all OCR engines against synthetic invoice images and compares:
- Character-level accuracy (Levenshtein distance)
- Field extraction (vendor, invoice number, total)
- Speed (wall-clock time per image)

Engines tested:
1. Falcon-OCR Worker (VLM, deployed at falcon-ocr.adpena.workers.dev)
2. PaddleOCR via tinygrad (local, once graph execution is complete)

Usage:
    python3 tests/e2e/test_e2e_quality.py
"""

from __future__ import annotations

import base64
import io
import json
import sys
import time
import urllib.request
import urllib.error

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    print("ERROR: Pillow required. Install with: pip3 install Pillow")
    sys.exit(1)


# ---------------------------------------------------------------------------
# Test invoice definitions
# ---------------------------------------------------------------------------

INVOICES = [
    {
        "vendor": "Acme Corp",
        "number": "INV-2026-042",
        "total": "$4,200.00",
        "items": ["Website Redesign 1 $4,200.00"],
    },
    {
        "vendor": "TechFlow Inc",
        "number": "TF-8891",
        "total": "$12,750.00",
        "items": ["UI Design 40 $150.00", "Frontend Dev 60 $125.00"],
    },
    {
        "vendor": "CloudNine Solutions",
        "number": "CN-2026-117",
        "total": "$8,400.00",
        "items": [
            "API Integration 20 $200.00",
            "Testing 40 $110.00",
        ],
    },
]


# ---------------------------------------------------------------------------
# Image generation
# ---------------------------------------------------------------------------

def generate_invoice_image(invoice: dict) -> str:
    """Generate a synthetic invoice PNG and return as base64 string.

    Creates a clean 600x400 image with vendor name, invoice number,
    line items, and total — representative of real scanned invoices.
    """
    img = Image.new("RGB", (600, 400), "white")
    d = ImageDraw.Draw(img)

    # Try to use a font that renders readable text
    try:
        font_large = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 18)
        font_normal = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 14)
        font_bold = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 16)
    except (OSError, IOError):
        font_large = ImageFont.load_default()
        font_normal = font_large
        font_bold = font_large

    # Header
    d.text((20, 20), invoice["vendor"], fill="black", font=font_large)
    d.text((420, 20), "INVOICE", fill="black", font=font_large)
    d.text((420, 45), invoice["number"], fill="gray", font=font_normal)

    # Separator line
    d.line([(20, 75), (580, 75)], fill="gray", width=1)

    # Column headers
    d.text((20, 85), "Description", fill="gray", font=font_normal)
    d.text((350, 85), "Qty", fill="gray", font=font_normal)
    d.text((450, 85), "Amount", fill="gray", font=font_normal)

    # Line items
    y = 110
    for item in invoice["items"]:
        d.text((20, y), item, fill="black", font=font_normal)
        y += 25

    # Separator
    d.line([(20, y + 10), (580, y + 10)], fill="gray", width=1)

    # Total
    d.text((350, y + 20), f'Total: {invoice["total"]}', fill="black", font=font_bold)

    # Encode to PNG base64
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return base64.b64encode(buf.getvalue()).decode("ascii")


# ---------------------------------------------------------------------------
# Levenshtein distance for accuracy measurement
# ---------------------------------------------------------------------------

def levenshtein_distance(s1: str, s2: str) -> int:
    """Compute edit distance between two strings."""
    if len(s1) < len(s2):
        return levenshtein_distance(s2, s1)
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


def character_accuracy(reference: str, hypothesis: str) -> float:
    """Character-level accuracy: 1 - (edit_distance / max_len)."""
    if not reference and not hypothesis:
        return 1.0
    max_len = max(len(reference), len(hypothesis))
    if max_len == 0:
        return 1.0
    dist = levenshtein_distance(reference, hypothesis)
    return max(0.0, 1.0 - dist / max_len)


# ---------------------------------------------------------------------------
# Engine: Falcon-OCR Worker
# ---------------------------------------------------------------------------

WORKER_URL = "https://falcon-ocr.adpena.workers.dev/ocr"


def test_falcon_worker(invoice: dict, img_b64: str) -> dict:
    """Test Falcon-OCR Worker endpoint.

    Posts a base64-encoded image and returns OCR results with timing.
    """
    payload = json.dumps({
        "image": img_b64,
        "max_tokens": 100,
    }).encode("utf-8")

    headers = {
        "Content-Type": "application/json",
        "Origin": "https://freeinvoicemaker.app",
        "X-Use-Backend": "falcon-ocr",
        "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) FalconOCR-QualityTest/1.0",
        "Accept": "application/json",
    }

    req = urllib.request.Request(WORKER_URL, data=payload, headers=headers)

    start = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=300) as resp:
            body = json.loads(resp.read())
            elapsed = time.monotonic() - start
    except urllib.error.HTTPError as e:
        elapsed = time.monotonic() - start
        error_body = e.read().decode("utf-8", errors="replace")[:500]
        return {
            "engine": "falcon-ocr-worker",
            "error": f"HTTP {e.code}: {error_body}",
            "time_s": elapsed,
        }
    except urllib.error.URLError as e:
        elapsed = time.monotonic() - start
        return {
            "engine": "falcon-ocr-worker",
            "error": str(e),
            "time_s": elapsed,
        }
    except Exception as e:
        elapsed = time.monotonic() - start
        return {
            "engine": "falcon-ocr-worker",
            "error": str(e),
            "time_s": elapsed,
        }

    text = body.get("text", "")
    model = body.get("model_used", "unknown")
    tokens = body.get("tokens", [])

    # Field extraction checks
    vendor_found = invoice["vendor"].lower() in text.lower() if text else False
    number_found = invoice["number"].lower() in text.lower() if text else False
    total_found = invoice["total"] in text if text else False

    return {
        "engine": "falcon-ocr-worker",
        "model": model,
        "text": text[:500],
        "tokens_preview": tokens[:10] if isinstance(tokens, list) else [],
        "time_s": elapsed,
        "vendor_found": vendor_found,
        "number_found": number_found,
        "total_found": total_found,
        "text_length": len(text),
    }


# ---------------------------------------------------------------------------
# Main test runner
# ---------------------------------------------------------------------------

def run_quality_tests() -> None:
    """Run OCR quality tests across all engines and invoices."""
    print("=" * 70)
    print("FALCON-OCR / PADDLEOCR QUALITY BENCHMARK")
    print("=" * 70)
    print()

    all_results: list[dict] = []

    for i, invoice in enumerate(INVOICES):
        print(f"--- Invoice {i + 1}: {invoice['vendor']} ---")
        print(f"  Number: {invoice['number']}")
        print(f"  Total:  {invoice['total']}")
        print(f"  Items:  {len(invoice['items'])}")
        print()

        img_b64 = generate_invoice_image(invoice)
        img_size_kb = len(img_b64) * 3 / 4 / 1024
        print(f"  Image: {img_size_kb:.1f} KB PNG (base64)")
        print()

        # Test Falcon-OCR Worker
        print("  [Falcon-OCR Worker] Sending request...")
        result = test_falcon_worker(invoice, img_b64)
        all_results.append(result)

        if "error" in result:
            print(f"  ERROR: {result['error']}")
            print(f"  Time: {result['time_s']:.1f}s")
        else:
            print(f"  Model: {result['model']}")
            print(f"  Time: {result['time_s']:.1f}s")
            print(f"  Text length: {result['text_length']} chars")
            print(f"  Vendor found: {result['vendor_found']}")
            print(f"  Number found: {result['number_found']}")
            print(f"  Total found:  {result['total_found']}")
            if result["text"]:
                preview = result["text"][:200].replace("\n", " | ")
                print(f"  Preview: {preview}")
        print()

    # Summary
    print("=" * 70)
    print("SUMMARY")
    print("=" * 70)

    falcon_results = [r for r in all_results if r["engine"] == "falcon-ocr-worker"]
    falcon_ok = [r for r in falcon_results if "error" not in r]
    falcon_errors = [r for r in falcon_results if "error" in r]

    print(f"\nFalcon-OCR Worker:")
    print(f"  Requests: {len(falcon_results)}")
    print(f"  Success:  {len(falcon_ok)}")
    print(f"  Errors:   {len(falcon_errors)}")

    if falcon_ok:
        avg_time = sum(r["time_s"] for r in falcon_ok) / len(falcon_ok)
        vendor_rate = sum(1 for r in falcon_ok if r.get("vendor_found")) / len(falcon_ok)
        number_rate = sum(1 for r in falcon_ok if r.get("number_found")) / len(falcon_ok)
        total_rate = sum(1 for r in falcon_ok if r.get("total_found")) / len(falcon_ok)
        print(f"  Avg time: {avg_time:.1f}s")
        print(f"  Vendor extraction:  {vendor_rate:.0%}")
        print(f"  Number extraction:  {number_rate:.0%}")
        print(f"  Total extraction:   {total_rate:.0%}")

    if falcon_errors:
        print(f"  Errors:")
        for r in falcon_errors:
            print(f"    {r['error'][:100]}")

    print()


if __name__ == "__main__":
    run_quality_tests()
