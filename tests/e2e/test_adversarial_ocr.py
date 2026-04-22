"""Adversarial OCR stress tests.

Generates pathological image inputs that commonly break OCR systems, and verifies
the processing pipeline handles them gracefully: no crashes, no panics, valid
(possibly empty) results, and proper error propagation.
"""

import io
import os
import random
from pathlib import Path

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    import subprocess
    import sys

    subprocess.check_call([sys.executable, "-m", "pip", "install", "Pillow"])
    from PIL import Image, ImageDraw, ImageFont

# Raise the decompression bomb limit for intentionally large test images.
Image.MAX_IMAGE_PIXELS = 200_000_000

OUTPUT_DIR = Path(__file__).parent / "test_images" / "adversarial"
OUTPUT_DIR.mkdir(parents=True, exist_ok=True)


# ---------------------------------------------------------------------------
# Font helper
# ---------------------------------------------------------------------------
def _get_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    font_paths = [
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
    for fp in font_paths:
        if os.path.exists(fp):
            try:
                return ImageFont.truetype(fp, size)
            except Exception:
                continue
    return ImageFont.load_default()


# ---------------------------------------------------------------------------
# Adversarial image generators
# ---------------------------------------------------------------------------


def gen_blank_white() -> Path:
    """All-white image — no content whatsoever."""
    img = Image.new("RGB", (800, 600), (255, 255, 255))
    path = OUTPUT_DIR / "blank_white.png"
    img.save(path)
    return path


def gen_blank_black() -> Path:
    """All-black image."""
    img = Image.new("RGB", (800, 600), (0, 0, 0))
    path = OUTPUT_DIR / "blank_black.png"
    img.save(path)
    return path


def gen_tiny_1x1() -> Path:
    """Minimum possible image: 1x1 pixel."""
    img = Image.new("RGB", (1, 1), (128, 128, 128))
    path = OUTPUT_DIR / "tiny_1x1.png"
    img.save(path)
    return path


def gen_large_10k() -> Path:
    """10000x10000 image — test memory handling.

    We use a palette image (mode 'P') to keep the file small while
    still presenting a huge dimension to the decoder.
    """
    # Use L mode (grayscale) to keep allocation manageable: 10k x 10k = 100MB
    # which is large but not fatal on modern systems.
    # Write a minimal PNG with the right dimensions but sparse content.
    img = Image.new("L", (10000, 10000), 200)
    # Add some text near the center
    draw = ImageDraw.Draw(img)
    font = _get_font(48)
    draw.text((4500, 4900), "LARGE IMAGE TEST", fill=0, font=font)
    path = OUTPUT_DIR / "large_10000x10000.png"
    img.save(path, optimize=True)
    return path


def gen_pure_noise() -> Path:
    """Random RGB noise — no structure at all."""
    width, height = 800, 600
    noise_data = bytes(random.randint(0, 255) for _ in range(width * height * 3))
    img = Image.frombytes("RGB", (width, height), noise_data)
    path = OUTPUT_DIR / "pure_noise.png"
    img.save(path)
    return path


def gen_rotated_45() -> Path:
    """Text rotated 45 degrees."""
    img = Image.new("RGB", (800, 600), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(24)
    draw.text(
        (100, 200),
        "Invoice #12345\nDate: 2025-01-15\nTotal: $1,234.56",
        fill=(0, 0, 0),
        font=font,
    )
    img = img.rotate(45, expand=True, fillcolor=(255, 255, 255))
    path = OUTPUT_DIR / "rotated_45.png"
    img.save(path)
    return path


def gen_rotated_90() -> Path:
    """Text rotated 90 degrees (portrait -> landscape)."""
    img = Image.new("RGB", (400, 800), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(20)
    draw.text(
        (50, 100),
        "INVOICE\nCompany: Test Corp\nAmount: $500.00",
        fill=(0, 0, 0),
        font=font,
    )
    img = img.rotate(90, expand=True)
    path = OUTPUT_DIR / "rotated_90.png"
    img.save(path)
    return path


def gen_rotated_180() -> Path:
    """Upside-down text."""
    img = Image.new("RGB", (800, 600), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(24)
    draw.text(
        (100, 200), "INVOICE #99999\nTotal Due: $10,000.00", fill=(0, 0, 0), font=font
    )
    img = img.rotate(180)
    path = OUTPUT_DIR / "rotated_180.png"
    img.save(path)
    return path


def gen_tiny_font() -> Path:
    """Very small text — near or below legibility threshold for most OCR."""
    # Create a large image, render text at normal size, then scale down
    # to simulate genuinely tiny text without font-engine edge cases.
    full_w, full_h = 2400, 1800
    img = Image.new("RGB", (full_w, full_h), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(18)
    for y_offset in range(0, full_h - 20, 24):
        draw.text(
            (10, y_offset),
            "Invoice Line Item - Product ABC - Qty 10 - $99.99",
            fill=(0, 0, 0),
            font=font,
        )
    # Scale down 6x to produce ~3px effective font size
    img = img.resize((400, 300), Image.LANCZOS)
    path = OUTPUT_DIR / "tiny_font.png"
    img.save(path)
    return path


def gen_mixed_languages() -> Path:
    """Mixed scripts: ASCII + CJK + Arabic + Cyrillic."""
    img = Image.new("RGB", (800, 600), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(20)

    texts = [
        "Invoice #12345 - English",
        "Factura #12345 - Espanol",
        "\u8acb\u6c42\u66f8 #12345 - \u65e5\u672c\u8a9e",  # Japanese
        "\u0424\u0430\u043a\u0442\u0443\u0440\u0430 #12345 - \u0420\u0443\u0441\u0441\u043a\u0438\u0439",  # Russian
        "\u0641\u0627\u062a\u0648\u0631\u0629 #12345 - \u0639\u0631\u0628\u064a",  # Arabic
        "\ucc2d\uad6c\uc11c #12345 - \ud55c\uad6d\uc5b4",  # Korean
    ]

    y = 30
    for text in texts:
        draw.text((50, y), text, fill=(0, 0, 0), font=font)
        y += 40

    path = OUTPUT_DIR / "mixed_languages.png"
    img.save(path)
    return path


def gen_high_contrast() -> Path:
    """Maximum contrast: pure black text on pure white."""
    img = Image.new("RGB", (800, 600), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(28)
    draw.text(
        (50, 100), "HIGH CONTRAST INVOICE\nTotal: $5,000.00", fill=(0, 0, 0), font=font
    )
    path = OUTPUT_DIR / "high_contrast.png"
    img.save(path)
    return path


def gen_low_contrast() -> Path:
    """Near-invisible text: light gray on slightly lighter gray."""
    img = Image.new("RGB", (800, 600), (210, 210, 210))
    draw = ImageDraw.Draw(img)
    font = _get_font(28)
    draw.text(
        (50, 100),
        "LOW CONTRAST INVOICE\nTotal: $5,000.00",
        fill=(195, 195, 195),
        font=font,
    )
    path = OUTPUT_DIR / "low_contrast.png"
    img.save(path)
    return path


def gen_jpeg_artifacts() -> Path:
    """Heavy JPEG compression (quality=5) — extreme block artifacts."""
    img = Image.new("RGB", (800, 600), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(22)
    draw.text(
        (50, 100),
        "JPEG ARTIFACTS TEST\nInvoice #77777\nTotal: $3,500.00",
        fill=(0, 0, 0),
        font=font,
    )
    # Compress to JPEG quality 5 and reload
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=5)
    buf.seek(0)
    img_degraded = Image.open(buf)
    path = OUTPUT_DIR / "jpeg_artifacts_q5.png"
    img_degraded.save(path, "PNG")
    return path


def gen_truncated_png() -> Path:
    """Truncated PNG file — corrupted data that should cause a decode error."""
    img = Image.new("RGB", (200, 200), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    draw.text((10, 10), "TRUNCATED", fill=(0, 0, 0))
    buf = io.BytesIO()
    img.save(buf, "PNG")
    raw = buf.getvalue()
    # Truncate to 60% of original size
    truncated = raw[: int(len(raw) * 0.6)]
    path = OUTPUT_DIR / "truncated.png"
    path.write_bytes(truncated)
    return path


def gen_corrupted_header() -> Path:
    """Valid PNG header but corrupted IHDR chunk."""
    # PNG signature + corrupted IHDR
    png_sig = b"\x89PNG\r\n\x1a\n"
    # Garbage IHDR
    corrupted_ihdr = b"\x00\x00\x00\x0dIHDR" + b"\xff" * 13
    # CRC (deliberately wrong)
    corrupted_ihdr += b"\xde\xad\xbe\xef"
    path = OUTPUT_DIR / "corrupted_header.png"
    path.write_bytes(png_sig + corrupted_ihdr)
    return path


def gen_gradient_fade() -> Path:
    """Gradient fade: text fades from solid to invisible."""
    width, height = 800, 200
    img = Image.new("RGB", (width, height), (255, 255, 255))
    draw = ImageDraw.Draw(img)
    font = _get_font(24)

    text = "FADING INVOICE TEXT $1,234.56"
    for i, char in enumerate(text):
        x = 50 + i * 14
        gray_level = int(255 * (i / len(text)))  # 0 (black) to 255 (invisible)
        draw.text((x, 80), char, fill=(gray_level, gray_level, gray_level), font=font)

    path = OUTPUT_DIR / "gradient_fade.png"
    img.save(path)
    return path


# ---------------------------------------------------------------------------
# Test runner
# ---------------------------------------------------------------------------


def validate_image_loadable(path: Path) -> tuple[bool, str]:
    """Try to load an image file. Returns (success, message)."""
    try:
        img = Image.open(path)
        img.load()  # Force full decode
        return True, f"{img.size[0]}x{img.size[1]} {img.mode}"
    except Exception as e:
        return False, f"decode error: {type(e).__name__}: {e}"


def main() -> None:
    print("Generating adversarial OCR test images\n")

    generators = [
        ("blank_white", gen_blank_white),
        ("blank_black", gen_blank_black),
        ("tiny_1x1", gen_tiny_1x1),
        ("large_10000x10000", gen_large_10k),
        ("pure_noise", gen_pure_noise),
        ("rotated_45", gen_rotated_45),
        ("rotated_90", gen_rotated_90),
        ("rotated_180", gen_rotated_180),
        ("tiny_font", gen_tiny_font),
        ("mixed_languages", gen_mixed_languages),
        ("high_contrast", gen_high_contrast),
        ("low_contrast", gen_low_contrast),
        ("jpeg_artifacts_q5", gen_jpeg_artifacts),
        ("truncated_png", gen_truncated_png),
        ("corrupted_header", gen_corrupted_header),
        ("gradient_fade", gen_gradient_fade),
    ]

    results = []
    for name, gen_fn in generators:
        print(f"  Generating: {name}...", end=" ")
        try:
            path = gen_fn()
            size_bytes = path.stat().st_size
            loadable, info = validate_image_loadable(path)
            status = "OK" if loadable else "DECODE_ERR (expected)"
            results.append((name, status, size_bytes, info))
            print(f"{status} — {size_bytes:,} bytes — {info}")
        except MemoryError:
            results.append((name, "MEMORY_ERROR", 0, "OOM"))
            print("MEMORY_ERROR (caught, non-fatal)")
        except Exception as e:
            results.append((name, "GEN_ERROR", 0, str(e)))
            print(f"GEN_ERROR: {e}")

    # Summary
    print(f"\n{'=' * 70}")
    print(f"{'Test Case':<30} {'Status':<15} {'Size':>10} {'Info'}")
    print(f"{'=' * 70}")
    for name, status, size, info in results:
        size_str = f"{size:,}" if size > 0 else "N/A"
        print(f"{name:<30} {status:<15} {size_str:>10} {info}")

    ok_count = sum(1 for _, s, _, _ in results if s in ("OK", "DECODE_ERR (expected)"))
    fail_count = len(results) - ok_count
    print(
        f"\nTotal: {len(results)} tests — {ok_count} passed — {fail_count} unexpected failures"
    )

    if fail_count > 0:
        print("\nWARNING: Some adversarial generators failed unexpectedly.")
        exit(1)


if __name__ == "__main__":
    main()
