"""Purpose: differential coverage for codecs error handler basics."""

import codecs

print(codecs.decode(b"\xff", "utf-8", "replace").encode("unicode_escape").decode("ascii"))
print(codecs.decode(b"\xff", "utf-8", "ignore"))
try:
    codecs.decode(b"\xff", "utf-8", "strict")
except UnicodeDecodeError as exc:
    print(type(exc).__name__)


# These routes must preserve the encoder's full code-point span and input,
# including lone surrogates. CPython is the differential oracle.
def encode_route(route, text, encoding, errors):
    if route == "str":
        return text.encode(encoding, errors)
    if route == "bytes":
        return bytes(text, encoding, errors)
    if route == "bytearray":
        return bytes(bytearray(text, encoding, errors))
    if route == "codecs":
        return codecs.encode(text, encoding, errors)
    return codecs.getincrementalencoder(encoding)(errors).encode(text, True)


for text, encoding, errors in (
    ("aé€z", "ascii", "strict"),
    ("éa€𐀀z", "latin-1", "strict"),
    ("a\udc80\ud800\udc81z", "utf-8", "surrogateescape"),
    ("a\udc80\ud800\udc81z", "cp1252", "surrogateescape"),
    ("a\ud800\ud801z", "utf-16-le", "strict"),
    ("a\udc80\udcffz", "utf-32-be", "surrogateescape"),
    ("a\ud800\ud801z", "utf-16-be", "replace"),
    ("a\ud800\ud801z", "utf-32-le", "replace"),
    ("a\udc80\udcffz", "cp1252", "surrogateescape"),
    ("valid", "ascii", "missing-encode-handler"),
):
    for route in ("str", "bytes", "bytearray", "codecs", "incremental"):
        try:
            result = encode_route(route, text, encoding, errors)
            print("encoded", route, encoding, errors, result.hex())
        except UnicodeEncodeError as exc:
            print("encoding-error", route, exc.encoding, exc.start, exc.end,
                  exc.reason, exc.object == text, exc.object is text)


for text, errors in (("a€Жz", "strict"), ("a\udc80\udcffz", "surrogateescape")):
    try:
        encoded, consumed = codecs.charmap_encode(text, errors, None)
        print("identity-charmap", encoded.hex(), consumed)
    except UnicodeEncodeError as exc:
        print("identity-charmap-error", exc.encoding, exc.start, exc.end,
              exc.reason, exc.object is text)


class RejectWrites:
    def __init__(self):
        self.calls = 0

    def write(self, data):
        self.calls += 1
        raise AssertionError("failed encoding must not write partial bytes")


for encoding, text in (("ascii", "aé€z"), ("utf-8", "a\ud800\ud801z")):
    stream = RejectWrites()
    writer = codecs.getwriter(encoding)(stream, "strict")
    try:
        writer.write(text)
    except UnicodeEncodeError as exc:
        print("stream-error", exc.encoding, exc.start, exc.end,
              exc.object is text, stream.calls)


# Filesystem encoding consumes the same raw Python string. An escaped prefix
# must not shift or broaden the later unencodable surrogate span.
import os


class EncodedPath:
    def __init__(self, value):
        self.value = value

    def __fspath__(self):
        return self.value


for value in ("a\udc80\udcffz", "a\udc80\ud800\udc81z"):
    for path in (value, EncodedPath(value)):
        try:
            print("fsencoded", os.fsencode(path).hex())
        except UnicodeEncodeError as exc:
            print("fsencode-error", exc.encoding, exc.start, exc.end,
                  exc.reason, exc.object is value)
