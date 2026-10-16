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


# Fixed stored-block streams keep the inflate oracle independent of Molt's
# compressor, including trailer bytes that a read-ahead decoder can swallow.
body = b"hello world"
raw = b"\x01\x0b\x00\xf4\xffhello world"
zstream = b"\x78\x01" + raw + b"\x1a\x0b\x04\x5d"
gstream = (
    b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03"
    + raw + b"\x85\x11\x4a\x0d\x0b\x00\x00\x00"
)
for label, window, encoded in (
    ("raw", -15, raw), ("zlib", 15, zstream), ("gzip", 31, gstream),
    ("zlib-header-window", 0, zstream), ("gzip-header-window", 16, gstream),
    ("auto-zlib", 47, zstream), ("auto-gzip", 47, gstream),
    ("auto-header-window", 32, gstream),
):
    decoder = zlib.decompressobj(window)
    print("stream-tail", label, decoder.decompress(encoded + b"TAIL") == body,
          decoder.eof, decoder.unused_data, decoder.unconsumed_tail)
    print("after-eof", label, decoder.decompress(b"MORE"), decoder.unused_data)
    print("finish", label, decoder.flush(), decoder.flush(), decoder.eof)
    try:
        decoder.decompress(b"")
    except zlib.error as exc:
        print("after-flush", label, type(exc).__name__)

    decoder = zlib.decompressobj(window)
    chunks = []
    early_eof = False
    for index in range(len(encoded)):
        chunks.append(decoder.decompress(encoded[index:index + 1]))
        if index + 1 < len(encoded):
            early_eof = early_eof or decoder.eof
    print("bytewise", label, b"".join(chunks) == body, early_eof, decoder.eof,
          decoder.unused_data, decoder.unconsumed_tail)

    decoder = zlib.decompressobj(window)
    chunks = [decoder.decompress(encoded + b"TAIL", 3)]
    print("limited-first", label, chunks[0], decoder.eof,
          bool(decoder.unconsumed_tail), decoder.unused_data)
    for _ in range(20):
        if decoder.eof:
            break
        chunks.append(decoder.decompress(decoder.unconsumed_tail, 3))
    print("limited-refeed", label, b"".join(chunks) == body, decoder.eof,
          decoder.unused_data)

    decoder = zlib.decompressobj(window)
    first = decoder.decompress(encoded, 2)
    print("limited-flush", label, first + decoder.flush(1) == body,
          decoder.eof, decoder.unconsumed_tail, decoder.unused_data)

# Empty input is not an implicit resubmission of the public tail.
decoder = zlib.decompressobj(-15)
first = decoder.decompress(raw, 2)
tail = decoder.unconsumed_tail
print("explicit-input-only", first, decoder.decompress(b"", 2),
      decoder.unconsumed_tail, decoder.eof)
print("explicit-tail-resume", decoder.decompress(tail) == body[2:], decoder.eof)

# Incomplete flush must neither invent EOF nor dispose of reusable stream state.
decoder = zlib.decompressobj(-15)
first = decoder.decompress(raw[:8])
print("incomplete-flush", first, decoder.flush(1), decoder.eof)
print("incomplete-resume", first + decoder.decompress(raw[8:]) == body, decoder.eof)

for window in (-16, -7, 1, 7, 17, 23, 33, 39, 48):
    try:
        zlib.decompressobj(window)
    except Exception as exc:
        print("bad-window", window, type(exc).__name__, str(exc))
for window in (-15, -8, 0, 8, 15, 16, 24, 31, 32, 40, 47):
    print("valid-window", window, zlib.decompressobj(window).eof)


class IndexValue:
    def __init__(self, value, events, error=None):
        self.value = value
        self.events = events
        self.error = error
    def __index__(self):
        self.events.append("index")
        if self.error is not None:
            raise self.error
        return self.value


events = []
decoder = zlib.decompressobj(IndexValue(-15, events))
print("window-index", events, decoder.decompress(raw) == body)
for method, argument in (("decompress", -1), ("decompress", None),
                         ("flush", 0), ("flush", -1), ("flush", None)):
    decoder = zlib.decompressobj()
    try:
        if method == "decompress":
            decoder.decompress(b"", argument)
        else:
            decoder.flush(argument)
    except Exception as exc:
        print("invalid-size", method, argument, type(exc).__name__, str(exc))


class StreamInput:
    def __init__(self, payload, events):
        self.data = bytearray(payload)
        self.events = events
        self.view = None
    def __buffer__(self, flags):
        self.events.append(("buffer", flags))
        self.view = memoryview(self.data)
        return self.view
    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))
        self.view = None


events = []
source = StreamInput(raw, events)
decoder = zlib.decompressobj(-15)
print("buffer-index-order", decoder.decompress(source, IndexValue(2, events)), events)
source.data.extend(b"!")
print("buffer-unpinned", True)
events = []
source = StreamInput(raw, events)
sentinel = RuntimeError("size callback")
try:
    zlib.decompressobj(-15).decompress(source, IndexValue(0, events, sentinel))
except RuntimeError as exc:
    print("buffer-index-error", exc is sentinel, events)
source.data.extend(b"!")
print("buffer-error-unpinned", True)

# A callback may finish this same stream; the outer operation must recheck the
# pinned receiver after callbacks, with no stream mutex held during __index__.
events = []
decoder = zlib.decompressobj(-15)
class ReentrantLimit:
    def __index__(self):
        events.append(decoder.decompress(raw) == body)
        events.append(decoder.flush())
        return 0
try:
    decoder.decompress(b"", ReentrantLimit())
except zlib.error as exc:
    print("reentrant-finished", events, type(exc).__name__)

# The max_length callback sees the input's counted export still alive.
events = []
source = StreamInput(raw, events)
class PinnedLimit:
    def __index__(self):
        try:
            source.data.extend(b"!")
        except BufferError:
            events.append("resize-blocked")
        return 2
decoder = zlib.decompressobj(-15)
print("index-export-pin", decoder.decompress(source, PinnedLimit()), events)
source.data.extend(b"!")
print("index-export-retired", True)
