"""Purpose: binascii.crc32 takes CPython's optional running value (HF-57).

The value continues a checksum across chunks, is taken bitwise as an unsigned
32-bit int, and must be an integer.
"""

import binascii
import zlib

data = b"The quick brown fox jumps over the lazy dog"
print("default", binascii.crc32(data))
print("explicit_zero", binascii.crc32(data, 0))
first = binascii.crc32(data[:10])
print("chained", binascii.crc32(data[10:], first) == binascii.crc32(data))
print("negative", binascii.crc32(data, -1))
print("wide", binascii.crc32(b"", 0xFFFFFFFF))
print("bytearray", binascii.crc32(bytearray(data), 7))
print("memoryview", binascii.crc32(memoryview(data), 7))
print("matches_zlib", binascii.crc32(data, 12345) == zlib.crc32(data, 12345))
try:
    binascii.crc32(data, "x")
except TypeError as exc:
    print("str_value", type(exc).__name__, exc)
try:
    binascii.crc32(data, value=1)
except TypeError as exc:
    print("keyword_value", type(exc).__name__)
