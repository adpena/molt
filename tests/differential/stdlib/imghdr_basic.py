"""Purpose: differential coverage for imghdr basic."""

import imghdr
import io


cases = [
    ("jpeg", b"\xff\xd8\xff\xdb" + b"\x00" * 12),
    ("png", b"\x89PNG\r\n\x1a\n" + b"\x00" * 12),
    ("gif", b"GIF89a" + b"\x00" * 12),
    ("tiff", b"MM" + b"\x00" * 12),
    ("rgb", b"\x01\xda" + b"\x00" * 12),
    ("pbm", b"P1 " + b"\x00" * 12),
    ("pgm", b"P2 " + b"\x00" * 12),
    ("ppm", b"P3 " + b"\x00" * 12),
    ("rast", b"\x59\xa6\x6a\x95" + b"\x00" * 12),
    ("xbm", b"#define " + b"\x00" * 12),
    ("bmp", b"BM" + b"\x00" * 12),
    ("webp", b"RIFF" + b"\x00\x00\x00\x00" + b"WEBP" + b"\x00" * 4),
    ("exr", b"\x76\x2f\x31\x01" + b"\x00" * 12),
]

print("builtin_count", len(imghdr.tests))

for kind, header in cases:
    print(kind, imghdr.what(None, h=header))

print("jpeg_str", imghdr.what(None, h="\xff\xd8\xff\xdb"))
print("jpeg_bytearray", imghdr.what(None, h=bytearray(b"\xff\xd8\xff\xdb")))
print("jpeg_memoryview", imghdr.what(None, h=memoryview(b"\xff\xd8\xff\xdb")))

bio = io.BytesIO(b"GIF87a" + b"\x00" * 12)
print("file_like", imghdr.what(bio))


def test_custom(h, f):
    _ = f
    if h.startswith(b"XYZ"):
        return "xyz"


imghdr.tests.append(test_custom)
print("custom", imghdr.what(None, h=b"XYZ123"))
imghdr.tests.pop()

print("unknown", imghdr.what(None, h=b""))
