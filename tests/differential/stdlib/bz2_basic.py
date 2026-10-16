# MOLT_META: stderr=exact
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


class ProtocolBuffer:
    def __init__(self, data):
        self.data = bytearray(data)
        self.events = []
        self.view = None

    def __buffer__(self, flags):
        self.events.append(("get", flags))
        self.view = memoryview(self.data)
        return self.view

    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))
        self.view = None

import gzip
import lzma
import zlib

for module in (bz2, gzip, lzma, zlib):
    source = ProtocolBuffer(b"buffer compression" * 4)
    encoded = module.compress(source, mtime=0) if module is gzip else module.compress(source)
    print("buffer-compress", module.__name__, source.events)
    encoded_source = ProtocolBuffer(encoded)
    try:
        decoded = module.decompress(encoded_source)
        print("buffer-decompress", module.__name__, decoded == bytes(source.data), encoded_source.events)
    except Exception as exc:
        print("buffer-decompress-error", module.__name__, type(exc).__name__, encoded_source.events)
    source.data.extend(b"!")
    encoded_source.data.extend(b"!")
    print("buffer-compression-unpinned", module.__name__, True)

for compressor in (bz2.BZ2Compressor(), lzma.LZMACompressor(), zlib.compressobj()):
    source = ProtocolBuffer(b"incremental")
    compressor.compress(source)
    compressor.flush()
    print("buffer-compress-object", source.events)


class CompressionLevel:
    def __index__(self):
        source.events.append(("index",))
        try:
            source.data.extend(b"!")
        except BufferError:
            source.events.append(("pinned",))
        source.data[0] = 65
        return 6

source = ProtocolBuffer(b"abc")
encoded = zlib.compress(source, CompressionLevel())
print("buffer-compress-order", zlib.decompress(encoded), source.events)
source.data.extend(b"!")
print("buffer-compress-order-unpinned", source.data)


# Reentry occurs before the compressor lock. The nested output must belong to
# the same stream, and retirement may safely call back into that stream again.
for module, make in ((bz2, bz2.BZ2Compressor), (lzma, lzma.LZMACompressor), (zlib, zlib.compressobj)):
    compressor = make()
    chunks = []
    class ReentrantInput(ProtocolBuffer):
        def __buffer__(self, flags):
            chunks.append(compressor.compress(b"prefix"))
            return super().__buffer__(flags)
        def __release_buffer__(self, view):
            super().__release_buffer__(view)
            self.events.append(("release-reentry", compressor.compress(b"") == b""))
    source = ReentrantInput(b"body")
    chunks.append(compressor.compress(source))
    chunks.append(compressor.flush())
    print("buffer-compress-reentry", module.__name__, module.decompress(b"".join(chunks)) == b"prefixbody", source.events)
    source.data.extend(b"!")


class ExportFailure(RuntimeError):
    pass


failure = ExportFailure("export identity")
class FailingInput:
    def __buffer__(self, flags):
        raise failure

for module in (bz2, gzip, lzma, zlib):
    for operation in (module.compress, module.decompress):
        try:
            if module is gzip and operation is module.compress:
                operation(FailingInput(), mtime=0)
            else:
                operation(FailingInput())
        except Exception as exc:
            print("buffer-compression-exception", module.__name__, operation.__name__, exc is failure)
