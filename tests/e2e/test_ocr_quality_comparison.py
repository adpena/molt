"""
OCR Quality Head-to-Head: Falcon-OCR (Workers AI / Gemma 3 12B) vs PaddleOCR

Generates 5 synthetic invoice images with known ground truth, sends each to both
OCR engines, and computes character-level accuracy, field extraction success, and
latency.

Requirements:
    pip3 install Pillow paddleocr  (paddleocr is optional; test degrades gracefully)

Usage:
    python3 -m pytest tests/e2e/test_ocr_quality_comparison.py -v
    python3 tests/e2e/test_ocr_quality_comparison.py  # standalone report generation
"""

from __future__ import annotations

import base64
import io
import json
import os
import sys
import time
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    sys.exit("Pillow is required: pip3 install Pillow")


# ---------------------------------------------------------------------------
# Levenshtein distance (pure Python, no deps)
# ---------------------------------------------------------------------------


def levenshtein(s: str, t: str) -> int:
    """Compute the Levenshtein edit distance between two strings."""
    if not s:
        return len(t)
    if not t:
        return len(s)
    m, n = len(s), len(t)
    prev = list(range(n + 1))
    curr = [0] * (n + 1)
    for i in range(1, m + 1):
        curr[0] = i
        for j in range(1, n + 1):
            cost = 0 if s[i - 1] == t[j - 1] else 1
            curr[j] = min(curr[j - 1] + 1, prev[j] + 1, prev[j - 1] + cost)
        prev, curr = curr, prev
    return prev[n]


def char_accuracy(ground_truth: str, predicted: str) -> float:
    """Character-level accuracy: 1 - (edit_distance / len(ground_truth))."""
    if not ground_truth:
        return 1.0 if not predicted else 0.0
    dist = levenshtein(ground_truth, predicted)
    return max(0.0, 1.0 - dist / len(ground_truth))


# ---------------------------------------------------------------------------
# Image generation
# ---------------------------------------------------------------------------


def _get_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    """Get a font at the given size, falling back to default if no TTF available."""
    font_paths = [
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ]
    for fp in font_paths:
        if os.path.exists(fp):
            try:
                return ImageFont.truetype(fp, size)
            except Exception:
                continue
    return ImageFont.load_default()


@dataclass
class TestCase:
    name: str
    ground_truth: str
    image: Image.Image = field(repr=False)
    fields: dict[str, str] = field(default_factory=dict)


def generate_clean_text() -> TestCase:
    """Test 1: Clean text at standard size."""
    text = "INVOICE #2026-042 | Acme Corp | Total: $4,200.00"
    img = Image.new("RGB", (800, 100), "white")
    draw = ImageDraw.Draw(img)
    font = _get_font(24)
    draw.text((20, 30), text, fill="black", font=font)
    return TestCase(
        name="clean_text",
        ground_truth=text,
        image=img,
        fields={
            "vendor": "Acme Corp",
            "invoice_number": "2026-042",
            "total": "$4,200.00",
        },
    )


def generate_small_font() -> TestCase:
    """Test 2: Small font (8pt equivalent)."""
    text = "INVOICE #2026-042 | Acme Corp | Total: $4,200.00"
    img = Image.new("RGB", (600, 60), "white")
    draw = ImageDraw.Draw(img)
    font = _get_font(11)  # ~8pt at 96dpi
    draw.text((10, 20), text, fill="black", font=font)
    return TestCase(
        name="small_font",
        ground_truth=text,
        image=img,
        fields={
            "vendor": "Acme Corp",
            "invoice_number": "2026-042",
            "total": "$4,200.00",
        },
    )


def generate_rotated() -> TestCase:
    """Test 3: Text rotated 5 degrees."""
    text = "INVOICE #2026-042 | Acme Corp | Total: $4,200.00"
    base = Image.new("RGB", (900, 200), "white")
    draw = ImageDraw.Draw(base)
    font = _get_font(24)
    draw.text((50, 80), text, fill="black", font=font)
    img = base.rotate(-5, expand=True, fillcolor="white")
    return TestCase(
        name="rotated_5deg",
        ground_truth=text,
        image=img,
        fields={
            "vendor": "Acme Corp",
            "invoice_number": "2026-042",
            "total": "$4,200.00",
        },
    )


def generate_low_contrast() -> TestCase:
    """Test 4: Light gray text on white background."""
    text = "INVOICE #2026-042 | Acme Corp | Total: $4,200.00"
    img = Image.new("RGB", (800, 100), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(24)
    draw.text((20, 30), text, fill=(200, 200, 200), font=font)
    return TestCase(
        name="low_contrast",
        ground_truth=text,
        image=img,
        fields={
            "vendor": "Acme Corp",
            "invoice_number": "2026-042",
            "total": "$4,200.00",
        },
    )


def generate_dense_table() -> TestCase:
    """Test 5: Dense table with 5 columns x 10 rows."""
    headers = ["Item", "Description", "Qty", "Rate", "Amount"]
    rows = [
        [
            f"ITEM-{i:03d}",
            f"Service line item {i}",
            str(i + 1),
            f"${(i + 1) * 100:.2f}",
            f"${(i + 1) * (i + 1) * 100:.2f}",
        ]
        for i in range(10)
    ]

    # Build ground truth as tab-separated text
    gt_lines = ["\t".join(headers)]
    for row in rows:
        gt_lines.append("\t".join(row))
    ground_truth = "\n".join(gt_lines)

    img = Image.new("RGB", (900, 400), "white")
    draw = ImageDraw.Draw(img)
    font = _get_font(14)

    col_widths = [80, 200, 50, 80, 100]
    x_start = 20
    y = 20

    # Draw headers
    x = x_start
    for i, h in enumerate(headers):
        draw.text((x, y), h, fill="black", font=font)
        x += col_widths[i]
    y += 25
    draw.line([(x_start, y), (x_start + sum(col_widths), y)], fill="black")
    y += 5

    # Draw rows
    for row in rows:
        x = x_start
        for i, cell in enumerate(row):
            draw.text((x, y), cell, fill="black", font=font)
            x += col_widths[i]
        y += 22

    return TestCase(
        name="dense_table",
        ground_truth=ground_truth,
        image=img,
        fields={"vendor": "", "invoice_number": "", "total": ""},
    )


ALL_GENERATORS = [
    generate_clean_text,
    generate_small_font,
    generate_rotated,
    generate_low_contrast,
    generate_dense_table,
]


# ---------------------------------------------------------------------------
# OCR backends
# ---------------------------------------------------------------------------


def image_to_b64(img: Image.Image, fmt: str = "PNG") -> str:
    buf = io.BytesIO()
    img.save(buf, format=fmt)
    return base64.b64encode(buf.getvalue()).decode()


def ocr_workers_ai(image_b64: str) -> dict[str, Any]:
    """Call the Falcon-OCR Workers AI endpoint."""
    req = urllib.request.Request(
        "https://falcon-ocr.adpena.workers.dev/ocr",
        data=json.dumps({"image": image_b64}).encode(),
        headers={
            "Content-Type": "application/json",
            "Origin": "https://freeinvoicemaker.app",
        },
    )
    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=30) as r:
        body = json.loads(r.read())
    latency = time.perf_counter() - t0
    return {"text": body.get("text", ""), "raw": body, "latency_ms": latency * 1000}


def ocr_paddleocr(img: Image.Image) -> dict[str, Any] | None:
    """Run PaddleOCR locally. Returns None if paddleocr is not installed."""
    try:
        from paddleocr import PaddleOCR
    except ImportError:
        return None

    ocr = PaddleOCR(use_angle_cls=True, lang="en", show_log=False)
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    buf.seek(0)

    t0 = time.perf_counter()
    result = ocr.ocr(buf.getvalue(), cls=True)
    latency = time.perf_counter() - t0

    # Flatten PaddleOCR result into text
    lines = []
    if result:
        for line_group in result:
            if line_group:
                for item in line_group:
                    if item and len(item) >= 2:
                        lines.append(item[1][0])
    return {"text": " ".join(lines), "raw": result, "latency_ms": latency * 1000}


# ---------------------------------------------------------------------------
# Field extraction check
# ---------------------------------------------------------------------------


def check_field_extraction(
    ocr_text: str, expected_fields: dict[str, str]
) -> dict[str, bool]:
    """Check if expected fields are present in OCR output (case-insensitive substring)."""
    results = {}
    text_lower = ocr_text.lower()
    for field_name, value in expected_fields.items():
        if not value:
            results[field_name] = True  # skip empty expected fields
        else:
            results[field_name] = value.lower() in text_lower
    return results


# ---------------------------------------------------------------------------
# Result aggregation
# ---------------------------------------------------------------------------


@dataclass
class OCRResult:
    engine: str
    test_name: str
    char_accuracy: float
    field_results: dict[str, bool]
    latency_ms: float
    available: bool = True


def run_comparison() -> list[OCRResult]:
    """Run the full comparison suite."""
    results: list[OCRResult] = []

    for gen in ALL_GENERATORS:
        tc = gen()
        b64 = image_to_b64(tc.image)

        # Workers AI (Falcon-OCR)
        try:
            wai = ocr_workers_ai(b64)
            acc = char_accuracy(tc.ground_truth, wai["text"])
            fields = check_field_extraction(wai["text"], tc.fields)
            results.append(
                OCRResult(
                    engine="Falcon-OCR (Workers AI)",
                    test_name=tc.name,
                    char_accuracy=acc,
                    field_results=fields,
                    latency_ms=wai["latency_ms"],
                )
            )
        except Exception as e:
            results.append(
                OCRResult(
                    engine="Falcon-OCR (Workers AI)",
                    test_name=tc.name,
                    char_accuracy=0.0,
                    field_results={k: False for k in tc.fields},
                    latency_ms=0.0,
                    available=False,
                )
            )
            print(f"  [WARN] Workers AI failed for {tc.name}: {e}", file=sys.stderr)

        # PaddleOCR
        paddle_result = ocr_paddleocr(tc.image)
        if paddle_result is None:
            results.append(
                OCRResult(
                    engine="PaddleOCR",
                    test_name=tc.name,
                    char_accuracy=0.0,
                    field_results={k: False for k in tc.fields},
                    latency_ms=0.0,
                    available=False,
                )
            )
        else:
            acc = char_accuracy(tc.ground_truth, paddle_result["text"])
            fields = check_field_extraction(paddle_result["text"], tc.fields)
            results.append(
                OCRResult(
                    engine="PaddleOCR",
                    test_name=tc.name,
                    char_accuracy=acc,
                    field_results=fields,
                    latency_ms=paddle_result["latency_ms"],
                )
            )

    return results


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------


def generate_report(results: list[OCRResult]) -> str:
    """Generate a Markdown comparison table."""
    lines = [
        "# OCR Quality Comparison: Falcon-OCR vs PaddleOCR",
        "",
        "## Methodology",
        "",
        "5 synthetic invoice images generated with Pillow, each with known ground truth.",
        "Character accuracy = 1 - (Levenshtein distance / ground truth length).",
        "Field extraction = substring match (case-insensitive) for vendor, invoice number, total.",
        "",
        "## Results",
        "",
        "| Test | Engine | Char Accuracy | Vendor | Invoice # | Total | Latency (ms) | Available |",
        "|------|--------|--------------|--------|-----------|-------|--------------|-----------|",
    ]

    for r in results:
        vendor = "Y" if r.field_results.get("vendor", False) else "N"
        inv_num = "Y" if r.field_results.get("invoice_number", False) else "N"
        total = "Y" if r.field_results.get("total", False) else "N"
        avail = "Y" if r.available else "N/A"
        lines.append(
            f"| {r.test_name} | {r.engine} | {r.char_accuracy:.1%} | "
            f"{vendor} | {inv_num} | {total} | {r.latency_ms:.0f} | {avail} |"
        )

    # Summary
    lines.extend(["", "## Summary", ""])

    for engine in ["Falcon-OCR (Workers AI)", "PaddleOCR"]:
        engine_results = [r for r in results if r.engine == engine and r.available]
        if engine_results:
            avg_acc = sum(r.char_accuracy for r in engine_results) / len(engine_results)
            avg_lat = sum(r.latency_ms for r in engine_results) / len(engine_results)
            lines.append(
                f"**{engine}**: avg accuracy {avg_acc:.1%}, avg latency {avg_lat:.0f}ms"
            )
        else:
            lines.append(
                f"**{engine}**: not available (install paddleocr or check endpoint)"
            )

    lines.extend(
        [
            "",
            "## Setup Notes",
            "",
            "- Falcon-OCR: Live endpoint at https://falcon-ocr.adpena.workers.dev/ocr (Gemma 3 12B via Workers AI)",
            "- PaddleOCR: `pip3 install paddleocr` (requires paddlepaddle). If not installed, results show N/A.",
            "- Images generated at runtime with Pillow; no external image assets needed.",
            "",
        ]
    )

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# pytest integration
# ---------------------------------------------------------------------------


def test_ocr_workers_ai_clean_text():
    """Smoke test: Workers AI returns non-empty text for clean input."""
    tc = generate_clean_text()
    b64 = image_to_b64(tc.image)
    result = ocr_workers_ai(b64)
    assert result["text"], "Workers AI returned empty text"
    acc = char_accuracy(tc.ground_truth, result["text"])
    assert acc > 0.5, f"Accuracy too low: {acc:.1%}"


def test_ocr_workers_ai_field_extraction():
    """Workers AI should extract key fields from clean invoice text."""
    tc = generate_clean_text()
    b64 = image_to_b64(tc.image)
    result = ocr_workers_ai(b64)
    fields = check_field_extraction(result["text"], tc.fields)
    # At minimum, vendor or total should be found
    assert fields["vendor"] or fields["total"], (
        f"No key fields found in: {result['text']!r}"
    )


def test_levenshtein_basic():
    """Unit test for Levenshtein distance."""
    assert levenshtein("", "") == 0
    assert levenshtein("abc", "") == 3
    assert levenshtein("", "abc") == 3
    assert levenshtein("abc", "abc") == 0
    assert levenshtein("kitten", "sitting") == 3


def test_char_accuracy_basic():
    """Unit test for character accuracy computation."""
    assert char_accuracy("hello", "hello") == 1.0
    assert char_accuracy("hello", "") == 0.0
    assert 0.0 < char_accuracy("hello", "helo") < 1.0


# ---------------------------------------------------------------------------
# Standalone execution
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    print("Running OCR quality comparison...")
    print("=" * 60)

    results = run_comparison()

    report = generate_report(results)
    print(report)

    # Save report
    out_path = (
        Path(__file__).resolve().parent.parent.parent
        / "docs"
        / "benchmarks"
        / "ocr_quality_comparison.md"
    )
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(report)
    print(f"\nReport saved to: {out_path}")
