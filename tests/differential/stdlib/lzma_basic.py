"""Purpose: differential coverage for lzma basic compress/decompress."""

import lzma

# --- Round-trip: empty bytes ---
empty_compressed = lzma.compress(b"")
empty_roundtrip = lzma.decompress(empty_compressed)
print("empty roundtrip:", empty_roundtrip == b"")

# --- Round-trip: small data ---
small = b"hello world"
small_compressed = lzma.compress(small)
small_roundtrip = lzma.decompress(small_compressed)
print("small roundtrip:", small_roundtrip == small)

# --- Round-trip: repeated data (compressible) ---
repeated = b"abcdefgh" * 1000
repeated_compressed = lzma.compress(repeated)
repeated_roundtrip = lzma.decompress(repeated_compressed)
print("repeated roundtrip:", repeated_roundtrip == repeated)
print("repeated compressed smaller:", len(repeated_compressed) < len(repeated))

# --- Round-trip: binary data with all byte values ---
all_bytes = bytes(range(256)) * 10
all_compressed = lzma.compress(all_bytes)
all_roundtrip = lzma.decompress(all_compressed)
print("all bytes roundtrip:", all_roundtrip == all_bytes)

# --- FORMAT_XZ explicit ---
xz_compressed = lzma.compress(b"xz format test", format=lzma.FORMAT_XZ)
xz_roundtrip = lzma.decompress(xz_compressed)
print("xz roundtrip:", xz_roundtrip == b"xz format test")

# --- FORMAT_ALONE ---
alone_compressed = lzma.compress(b"alone format test", format=lzma.FORMAT_ALONE)
alone_roundtrip = lzma.decompress(alone_compressed, format=lzma.FORMAT_ALONE)
print("alone roundtrip:", alone_roundtrip == b"alone format test")

# --- Constants exist ---
print("FORMAT_XZ type:", type(lzma.FORMAT_XZ).__name__)
print("FORMAT_ALONE type:", type(lzma.FORMAT_ALONE).__name__)
print("FORMAT_RAW type:", type(lzma.FORMAT_RAW).__name__)
print("FORMAT_AUTO type:", type(lzma.FORMAT_AUTO).__name__)
print("CHECK_NONE type:", type(lzma.CHECK_NONE).__name__)
print("CHECK_CRC32 type:", type(lzma.CHECK_CRC32).__name__)
print("CHECK_CRC64 type:", type(lzma.CHECK_CRC64).__name__)
print("CHECK_SHA256 type:", type(lzma.CHECK_SHA256).__name__)
print("PRESET_DEFAULT type:", type(lzma.PRESET_DEFAULT).__name__)
print("PRESET_EXTREME type:", type(lzma.PRESET_EXTREME).__name__)

# --- Incremental compressor ---
comp = lzma.LZMACompressor()
chunks = []
chunks.append(comp.compress(b"hello "))
chunks.append(comp.compress(b"world"))
chunks.append(comp.flush())
incremental_compressed = b"".join(chunks)
incremental_roundtrip = lzma.decompress(incremental_compressed)
print("incremental compress roundtrip:", incremental_roundtrip == b"hello world")

# --- Incremental decompressor ---
compressed_for_decomp = lzma.compress(b"test data 12345")
decomp = lzma.LZMADecompressor()
result = decomp.decompress(compressed_for_decomp)
print("incremental decompress:", result == b"test data 12345")
print("decomp eof:", decomp.eof)
