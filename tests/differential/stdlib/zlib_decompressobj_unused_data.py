"""Purpose: differential coverage for zlib.decompressobj().unused_data.

CPython zlib semantics (Doc/library/zlib.rst): ``unused_data`` stays ``b""``
until the last byte of compressed data has been processed, after which it holds
any bytes found *past the end* of the compressed stream (e.g. trailing bytes or
a second concatenated stream). It is distinct from ``unconsumed_tail``.
"""

import zlib


def main() -> None:
    payload = b"the quick brown fox jumps over the lazy dog" * 3
    compressed = zlib.compress(payload)
    trailing = b"TRAILING-BYTES-AFTER-STREAM"

    # 1. Whole input is exactly one compressed stream -> unused_data == b"".
    d1 = zlib.decompressobj()
    out1 = d1.decompress(compressed)
    print("exact_out_ok", out1 == payload)
    print("exact_unused", d1.unused_data)

    # 2. Compressed stream followed by trailing bytes -> unused_data == trailing.
    d2 = zlib.decompressobj()
    out2 = d2.decompress(compressed + trailing)
    print("trailing_out_ok", out2 == payload)
    print("trailing_unused", d2.unused_data)
    print("trailing_eof", d2.eof)

    # 3. Two concatenated streams: unused_data hands back the second stream so it
    #    can be fed to a fresh decompressor.
    payload2 = b"second stream body"
    compressed2 = zlib.compress(payload2)
    d3 = zlib.decompressobj()
    first = d3.decompress(compressed + compressed2)
    rest = d3.unused_data
    print("concat_first_ok", first == payload)
    d4 = zlib.decompressobj()
    second = d4.decompress(rest)
    print("concat_second_ok", second == payload2)


if __name__ == "__main__":
    main()
