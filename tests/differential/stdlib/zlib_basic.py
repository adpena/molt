"""Purpose: differential coverage for zlib basic."""

import zlib


data = b"hello" * 20
compressed = zlib.compress(data)
roundtrip = zlib.decompress(compressed)

print(len(compressed) < len(data), roundtrip == data)
