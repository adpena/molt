"""Generate synthetic invoice test images for OCR accuracy testing.

Creates 10 varied invoice images with different layouts, fonts, rotations,
noise levels, and resolutions. Also attempts to download 3 public-domain
invoice samples from the web.
"""

import os
import random
import math
import struct
import urllib.request
import urllib.error
from pathlib import Path

# ---------------------------------------------------------------------------
# PIL import with auto-install fallback
# ---------------------------------------------------------------------------
try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    import subprocess
    import sys
    subprocess.check_call([sys.executable, "-m", "pip", "install", "Pillow"])
    from PIL import Image, ImageDraw, ImageFont

OUTPUT_DIR = Path(__file__).parent / "test_images"
OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

# ---------------------------------------------------------------------------
# Invoice data generators
# ---------------------------------------------------------------------------
COMPANY_NAMES = [
    "Acme Corporation", "TechNova Inc.", "BlueStar Solutions",
    "GlobalEdge Ltd.", "Pinnacle Dynamics", "Verdant Systems",
    "NexGen Consulting", "IronForge Industries", "Celestial Labs",
    "Quantum Drift Co.",
]

ITEM_NAMES = [
    "Widget Assembly", "Cloud Hosting (monthly)", "Design Consultation",
    "API Integration Service", "Data Migration", "Premium Support Plan",
    "SSL Certificate", "Domain Registration", "Custom Dashboard",
    "Security Audit", "Performance Tuning", "Load Testing",
    "Backup Service", "Email Hosting", "Content Delivery Network",
]


def _random_date() -> str:
    year = random.choice([2024, 2025, 2026])
    month = random.randint(1, 12)
    day = random.randint(1, 28)
    return f"{year}-{month:02d}-{day:02d}"


def _random_invoice_number() -> str:
    prefix = random.choice(["INV", "BILL", "REC", "ORD"])
    return f"{prefix}-{random.randint(10000, 99999)}"


def _random_line_items(count: int) -> list[dict]:
    items = random.sample(ITEM_NAMES, min(count, len(ITEM_NAMES)))
    result = []
    for name in items:
        qty = random.randint(1, 20)
        price = round(random.uniform(9.99, 999.99), 2)
        result.append({"name": name, "qty": qty, "price": price})
    return result


# ---------------------------------------------------------------------------
# Font helper — use built-in default bitmap font
# ---------------------------------------------------------------------------
def _get_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    """Attempt to load a TrueType font at the given size, falling back to default."""
    font_paths = [
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
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


# ---------------------------------------------------------------------------
# Invoice renderer
# ---------------------------------------------------------------------------
def render_invoice(
    *,
    index: int,
    width: int,
    height: int,
    bg_color: tuple[int, int, int],
    fg_color: tuple[int, int, int],
    noise_level: float,
    rotation: float,
    font_size: int,
    jpeg_quality: int | None,
) -> Path:
    """Render a single synthetic invoice image."""
    img = Image.new("RGB", (width, height), bg_color)
    draw = ImageDraw.Draw(img)
    font = _get_font(font_size)
    small_font = _get_font(max(font_size // 2, 8))

    company = COMPANY_NAMES[index % len(COMPANY_NAMES)]
    inv_num = _random_invoice_number()
    date = _random_date()
    items = _random_line_items(random.randint(3, 8))

    y = int(height * 0.05)
    x_margin = int(width * 0.08)

    # Header
    draw.text((x_margin, y), company, fill=fg_color, font=font)
    y += font_size + 10
    draw.text((x_margin, y), f"Invoice: {inv_num}", fill=fg_color, font=small_font)
    y += font_size // 2 + 8
    draw.text((x_margin, y), f"Date: {date}", fill=fg_color, font=small_font)
    y += font_size // 2 + 20

    # Separator line
    draw.line([(x_margin, y), (width - x_margin, y)], fill=fg_color, width=2)
    y += 15

    # Column headers
    col_name_x = x_margin
    col_qty_x = int(width * 0.50)
    col_price_x = int(width * 0.65)
    col_total_x = int(width * 0.82)

    header_texts = [
        (col_name_x, "Item"),
        (col_qty_x, "Qty"),
        (col_price_x, "Price"),
        (col_total_x, "Total"),
    ]
    for hx, ht in header_texts:
        draw.text((hx, y), ht, fill=fg_color, font=small_font)
    y += font_size // 2 + 10

    # Line items
    grand_total = 0.0
    for item in items:
        line_total = item["qty"] * item["price"]
        grand_total += line_total
        draw.text((col_name_x, y), item["name"], fill=fg_color, font=small_font)
        draw.text((col_qty_x, y), str(item["qty"]), fill=fg_color, font=small_font)
        draw.text((col_price_x, y), f"${item['price']:.2f}", fill=fg_color, font=small_font)
        draw.text((col_total_x, y), f"${line_total:.2f}", fill=fg_color, font=small_font)
        y += font_size // 2 + 6

    # Grand total
    y += 10
    draw.line([(col_total_x, y), (width - x_margin, y)], fill=fg_color, width=2)
    y += 8
    draw.text(
        (col_price_x, y), "TOTAL:", fill=fg_color, font=font
    )
    draw.text(
        (col_total_x, y), f"${grand_total:.2f}", fill=fg_color, font=font
    )

    # Add noise
    if noise_level > 0:
        pixels = img.load()
        assert pixels is not None
        noise_count = int(width * height * noise_level)
        for _ in range(noise_count):
            nx = random.randint(0, width - 1)
            ny = random.randint(0, height - 1)
            r, g, b = pixels[nx, ny]
            delta = random.randint(-40, 40)
            pixels[nx, ny] = (
                max(0, min(255, r + delta)),
                max(0, min(255, g + delta)),
                max(0, min(255, b + delta)),
            )

    # Rotation
    if rotation != 0:
        img = img.rotate(rotation, expand=True, fillcolor=bg_color)

    # Save
    filename = f"invoice_{index:02d}.png"
    filepath = OUTPUT_DIR / filename

    if jpeg_quality is not None:
        # Save as JPEG with specified quality, then convert back to PNG filename
        jpeg_path = filepath.with_suffix(".jpg")
        img.save(jpeg_path, "JPEG", quality=jpeg_quality)
        # Also keep a PNG copy
        img.save(filepath, "PNG")
    else:
        img.save(filepath, "PNG")

    return filepath


# ---------------------------------------------------------------------------
# Generate 10 diverse invoices
# ---------------------------------------------------------------------------
INVOICE_CONFIGS = [
    # 0: Standard clean invoice, 800x1000
    dict(width=800, height=1000, bg_color=(255, 255, 255), fg_color=(0, 0, 0),
         noise_level=0.0, rotation=0, font_size=24, jpeg_quality=None),
    # 1: Small resolution, compact
    dict(width=300, height=400, bg_color=(255, 255, 255), fg_color=(30, 30, 30),
         noise_level=0.0, rotation=0, font_size=12, jpeg_quality=None),
    # 2: Large high-res
    dict(width=2000, height=3000, bg_color=(252, 252, 248), fg_color=(10, 10, 10),
         noise_level=0.0, rotation=0, font_size=36, jpeg_quality=None),
    # 3: Slight rotation
    dict(width=800, height=1000, bg_color=(255, 255, 255), fg_color=(0, 0, 0),
         noise_level=0.01, rotation=3, font_size=22, jpeg_quality=None),
    # 4: Moderate noise
    dict(width=800, height=1000, bg_color=(245, 245, 240), fg_color=(20, 20, 20),
         noise_level=0.15, rotation=0, font_size=22, jpeg_quality=None),
    # 5: Heavy noise + rotation
    dict(width=600, height=800, bg_color=(240, 235, 230), fg_color=(40, 30, 20),
         noise_level=0.3, rotation=-5, font_size=20, jpeg_quality=None),
    # 6: JPEG artifacts (quality=5)
    dict(width=800, height=1000, bg_color=(255, 255, 255), fg_color=(0, 0, 0),
         noise_level=0.0, rotation=0, font_size=24, jpeg_quality=5),
    # 7: Low contrast
    dict(width=800, height=1000, bg_color=(200, 200, 200), fg_color=(150, 150, 150),
         noise_level=0.05, rotation=0, font_size=22, jpeg_quality=None),
    # 8: Dark background, light text
    dict(width=800, height=1000, bg_color=(30, 30, 40), fg_color=(220, 220, 210),
         noise_level=0.02, rotation=0, font_size=22, jpeg_quality=None),
    # 9: Very small font
    dict(width=500, height=700, bg_color=(255, 255, 255), fg_color=(0, 0, 0),
         noise_level=0.0, rotation=0, font_size=10, jpeg_quality=None),
]

# ---------------------------------------------------------------------------
# Public invoice template URLs (CC0 / public domain or permissive)
# ---------------------------------------------------------------------------
SAMPLE_URLS = [
    "https://www.invoicesimple.com/wp-content/uploads/2023/04/Simple-Invoice-Template.png",
    "https://create.microsoft.com/en-us/templates/invoices",  # Will likely fail — placeholder
    "https://images.template.net/wp-content/uploads/2015/08/Simple-Invoice-Example.jpg",
]


def download_samples() -> list[Path]:
    """Try downloading public invoice samples. Returns paths of successful downloads."""
    downloaded = []
    for i, url in enumerate(SAMPLE_URLS):
        dest = OUTPUT_DIR / f"sample_invoice_{i:02d}.png"
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
            with urllib.request.urlopen(req, timeout=10) as resp:
                data = resp.read()
                if len(data) < 500:
                    print(f"  [SKIP] {url} — response too small ({len(data)} bytes)")
                    continue
                # Verify it looks like an image
                if data[:4] in (b"\x89PNG", b"\xff\xd8\xff\xe0", b"\xff\xd8\xff\xe1"):
                    dest.write_bytes(data)
                    downloaded.append(dest)
                    print(f"  [OK] Downloaded {url} -> {dest.name}")
                else:
                    print(f"  [SKIP] {url} — not a valid image format")
        except (urllib.error.URLError, urllib.error.HTTPError, OSError) as e:
            print(f"  [FAIL] {url} — {e}")
    return downloaded


def main() -> None:
    print(f"Generating 10 synthetic invoices in {OUTPUT_DIR}/\n")
    for i, config in enumerate(INVOICE_CONFIGS):
        path = render_invoice(index=i, **config)
        w, h = config["width"], config["height"]
        rotation = config["rotation"]
        noise = config["noise_level"]
        jpeg_q = config["jpeg_quality"]
        extras = []
        if rotation != 0:
            extras.append(f"rot={rotation}")
        if noise > 0:
            extras.append(f"noise={noise}")
        if jpeg_q is not None:
            extras.append(f"jpeg_q={jpeg_q}")
        extra_str = f" ({', '.join(extras)})" if extras else ""
        print(f"  [{i:02d}] {w}x{h} font={config['font_size']}{extra_str} -> {path.name}")

    print("\nDownloading public-domain invoice samples...")
    downloaded = download_samples()
    print(f"\nDone: {len(downloaded)} samples downloaded")

    # List all generated files
    all_files = sorted(OUTPUT_DIR.glob("*.png")) + sorted(OUTPUT_DIR.glob("*.jpg"))
    print(f"\nTotal test images: {len(all_files)}")
    for f in all_files:
        size = f.stat().st_size
        print(f"  {f.name}: {size:,} bytes")


if __name__ == "__main__":
    main()
