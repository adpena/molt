"""Purpose: differential coverage for bz2 basic compress/decompress."""

import bz2

# --- Round-trip: empty bytes ---
empty_compressed = bz2.compress(b"")
empty_roundtrip = bz2.decompress(empty_compressed)
print("empty roundtrip:", empty_roundtrip == b"")

# --- Round-trip: small data ---
small = b"hello world"
small_compressed = bz2.compress(small)
small_roundtrip = bz2.decompress(small_compressed)
print("small roundtrip:", small_roundtrip == small)

# --- Round-trip: repeated data (compressible) ---
repeated = b"abcdefgh" * 1000
repeated_compressed = bz2.compress(repeated)
repeated_roundtrip = bz2.decompress(repeated_compressed)
print("repeated roundtrip:", repeated_roundtrip == repeated)
print("repeated compressed smaller:", len(repeated_compressed) < len(repeated))

# --- compresslevel parameter ---
data = b"x" * 5000
c1 = bz2.compress(data, compresslevel=1)
c9 = bz2.compress(data, compresslevel=9)
print("level 1 roundtrip:", bz2.decompress(c1) == data)
print("level 9 roundtrip:", bz2.decompress(c9) == data)

# --- Round-trip: binary data with all byte values ---
all_bytes = bytes(range(256)) * 10
all_compressed = bz2.compress(all_bytes)
all_roundtrip = bz2.decompress(all_compressed)
print("all bytes roundtrip:", all_roundtrip == all_bytes)

# --- Incremental compressor ---
comp = bz2.BZ2Compressor(9)
chunks = []
chunks.append(comp.compress(b"hello "))
chunks.append(comp.compress(b"world"))
chunks.append(comp.flush())
incremental_compressed = b"".join(chunks)
incremental_roundtrip = bz2.decompress(incremental_compressed)
print("incremental compress roundtrip:", incremental_roundtrip == b"hello world")

# --- Incremental decompressor ---
compressed_for_decomp = bz2.compress(b"test data 12345")
decomp = bz2.BZ2Decompressor()
result = decomp.decompress(compressed_for_decomp)
print("incremental decompress:", result == b"test data 12345")
print("decomp eof:", decomp.eof)
