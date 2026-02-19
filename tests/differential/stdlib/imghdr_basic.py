"""Purpose: basic CPython parity coverage for imghdr image header detection."""

import io
import importlib
import os
import sys
import tempfile
import warnings

warnings.filterwarnings(
    "ignore",
    category=DeprecationWarning,
    message="'imghdr' is deprecated and slated for removal in Python 3.13",
)

if sys.version_info >= (3, 13):
    try:
        importlib.import_module("imghdr")
    except ModuleNotFoundError:
        print("imghdr_absent", tuple(sys.version_info[:3]))
    else:
        raise AssertionError("imghdr must be absent on >=3.13")
    raise SystemExit(0)

import imghdr

samples = [
    ("jpeg", b"\xff\xd8\xff\xdb" + b"\x00" * 32),
    ("png", b"\x89PNG\r\n\x1a\n" + b"\x00" * 24),
    ("gif", b"GIF89a" + b"\x00" * 26),
    ("tiff", b"MM" + b"\x00" * 30),
    ("rgb", b"\x01\xda" + b"\x00" * 30),
    ("pbm", b"P1 " + b"\x00" * 29),
    ("pgm", b"P2 " + b"\x00" * 29),
    ("ppm", b"P3 " + b"\x00" * 29),
    ("rast", b"\x59\xa6\x6a\x95" + b"\x00" * 28),
    ("xbm", b"#define " + b"\x00" * 24),
    ("bmp", b"BM" + b"\x00" * 30),
    ("webp", b"RIFF" + b"\x00" * 4 + b"WEBP" + b"\x00" * 20),
    ("exr", b"\x76\x2f\x31\x01" + b"\x00" * 28),
]

for expected, header in samples:
    assert imghdr.what(None, header) == expected, (expected, imghdr.what(None, header))
    bio = io.BytesIO(header + b"payload")
    pos = bio.tell()
    assert imghdr.what(bio) == expected
    assert bio.tell() == pos
    with tempfile.NamedTemporaryFile(delete=False) as tmp:
        tmp.write(header + b"payload")
        tmp_path = tmp.name
    try:
        assert imghdr.what(tmp_path) == expected
    finally:
        os.unlink(tmp_path)

assert imghdr.what(None, b"not-an-image") is None

assert imghdr.test_png(b"\x89PNG\r\n\x1a\nxxxx", None) == "png"
assert imghdr.test_jpeg(b"\xff\xd8\xff\xdbxxxx", None) == "jpeg"

print("imghdr_test_count", len(imghdr.tests))
print("imghdr_sample_count", len(samples))
