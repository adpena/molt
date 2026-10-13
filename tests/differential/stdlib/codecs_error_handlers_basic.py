"""Purpose: differential coverage for codecs error handler basics."""

import codecs
import json
import os
from _json import scanstring

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


# Typed decoder exceptions retain the entire immutable input and an exclusive
# end, through both public convenience and stateful codec consumers.


def show_decode_error(label, action):
    try:
        action()
    except UnicodeDecodeError as exc:
        print("decode-payload", label, type(exc.object).__name__, exc.object.hex(),
              exc.encoding, exc.start, exc.end, exc.reason, str(exc),
              exc.args == (exc.encoding, exc.object, exc.start, exc.end, exc.reason),
              exc.args[2:])
    else:
        raise AssertionError("invalid input unexpectedly decoded")


for data, encoding in (
    (b"a\xffz", "utf-8"),
    (b"a\xffz", "ascii"),
    (b"A\x00\x00\xd8", "utf-16-le"),
    (b"\x00\x00\x11\x00", "utf-32-le"),
    (b"\\x1", "unicode_escape"),
):
    show_decode_error("codecs-" + encoding, lambda: codecs.decode(data, encoding))
    show_decode_error("bytes-" + encoding, lambda: data.decode(encoding))
    show_decode_error("bytearray-" + encoding, lambda: bytearray(data).decode(encoding))
    show_decode_error("incremental-" + encoding,
                      lambda: codecs.getincrementaldecoder(encoding)().decode(data, True))


# The failing call includes bytes buffered by an earlier call, not only its
# latest argument. Incomplete suffixes wait for final input; malformed input does not.
decoder = codecs.getincrementaldecoder("utf-8")()
print("buffered-prefix", decoder.decode(b"a\xc3", False))
show_decode_error("incremental-buffered", lambda: decoder.decode(b"\xff", True))

show_decode_error("charmap-dict", lambda: codecs.charmap_decode(b"a\xffz", "strict", {97: 97, 122: 122}))
show_decode_error("charmap-table", lambda: codecs.charmap_decode(b"a\xffz", "strict", "".join(chr(i) for i in range(255))))
# JSON deliberately permits UTF16 surrogates; an incomplete code unit must
# fail decoding before JSON syntax is considered.
show_decode_error("json-utf16", lambda: json.loads(b'\xff\xfe"\x00\x00'))

for errors in ("strict", "replace", "backslashreplace"):
    text = "a€z"
    try:
        codecs.charmap_encode(text, errors, {97: b"a", 122: b"z"})
    except UnicodeEncodeError as exc:
        print("charmap-payload", errors, exc.encoding, exc.object is text,
              exc.start, exc.end, exc.reason, str(exc),
              exc.args == (exc.encoding, text, exc.start, exc.end, exc.reason))
    else:
        raise AssertionError("undefined mapping unexpectedly encoded")


# CPython's scanner is the oracle for legal lead ranges, restricted second
# bytes, the first bad continuation, and incomplete prefixes. Error handlers
# must consume precisely the reported span, including after a buffered prefix.
utf8_cases = (
    b"a\x80z", b"a\xc0\x80z", b"a\xc1\xbfz", b"a\xf5\x80z",
    b"a\xc2z", b"a\xe1\x80z", b"a\xf1\x80\x80z",
    b"a\xe0\x9f\x80z", b"a\xed\xa0\x80z",
    b"a\xf0\x8f\x80\x80z", b"a\xf4\x90\x80\x80z",
    b"a\xc2", b"a\xe1\x80", b"a\xf1\x80\x80", b"a\xed\xa0",
    b"a\xc2\x80\xdf\xbfz", b"a\xe0\xa0\x80\xed\x9f\xbfz",
    b"a\xf0\x90\x80\x80\xf4\x8f\xbf\xbfz",
)


def decode_result(label, action):
    try:
        value = action()
        print("decode-ok", label, value.encode("unicode_escape").hex())
    except UnicodeDecodeError as exc:
        print("decode-error", label, exc.object.hex(), exc.encoding,
              exc.start, exc.end, exc.reason)


for data in utf8_cases:
    for errors in ("strict", "replace", "ignore", "backslashreplace", "surrogateescape", "surrogatepass"):
        label = data.hex() + ":" + errors
        decode_result(label + ":full", lambda: data.decode("utf-8", errors))
        # Every split uses the real incremental object. On an error the call
        # index and full exception input distinguish premature/delayed refusal.
        for split in range(len(data) + 1):
            decoder = codecs.getincrementaldecoder("utf-8")(errors)
            phase = 0
            try:
                left = decoder.decode(data[:split], False)
                phase = 1
                right = decoder.decode(data[split:], True)
                print("utf8-split-ok", label, split,
                      (left + right).encode("unicode_escape").hex())
            except UnicodeDecodeError as exc:
                print("utf8-split-error", label, split, phase, exc.object.hex(),
                      exc.start, exc.end, exc.reason)

# UTF8-SIG reports the scanner's post-BOM input rather than offsets into the
# discarded signature. Plain UTF8 reports that signature as part of its input.
for encoding in ("utf-8", "utf-8-sig"):
    data = b"\xef\xbb\xbfa\xffz"
    show_decode_error(encoding + "-bom", lambda: codecs.decode(data, encoding))


# Complete surrogates, malformed tails and partial surrogateescape consumption
# use the same stateless authority through every public byte-input route.
for encoding, data in (
    ("utf-16-le", b"\x00\xd8\xff"),
    ("utf-16-be", b"\xdc\x00A"),
    ("utf-16-le", b"\x80\xd8A"),
    ("utf-16-be", b"\xd8\x00\xdc\x00"),
    ("utf-32-le", b"\x00\xd8\x00\x00\xff"),
    ("utf-32-be", b"\x00\x00\xdc\x00"),
    ("utf-32-le", b"\x80\x00\x11\x00"),
):
    for errors in ("strict", "surrogatepass", "surrogateescape", "replace", "ignore", "backslashreplace"):
        label = encoding + ":" + data.hex() + ":" + errors
        decode_result(label + ":bytes", lambda: data.decode(encoding, errors))
        decode_result(label + ":bytearray", lambda: bytearray(data).decode(encoding, errors))
        decode_result(label + ":codecs", lambda: codecs.decode(data, encoding, errors))


# A handled error and a later fatal one share an exception within one decode
# call: args retain the first span, attributes advance to the final span.
for encoding, errors, data in (
    ("utf-8", "surrogatepass", b"\xed\xa0\x80\xff"),
    ("utf-8-sig", "surrogatepass", b"\xef\xbb\xbf\xed\xa0\x80\xff"),
    ("utf-16-le", "surrogatepass", b"\x00\xd8\xff"),
    ("utf-16-be", "surrogatepass", b"\xdc\x00\xff"),
    ("utf-32-le", "surrogatepass", b"\x00\xd8\x00\x00\xff"),
    ("utf-32-be", "surrogatepass", b"\x00\x00\xdc\x00\xff"),
    ("utf-16-be", "surrogateescape", b"\xdc\x80A"),
    ("utf-32-le", "surrogateescape", b"\x80\x00\x11\x00"),
):
    label = encoding + ":" + errors
    show_decode_error(label + ":bytes", lambda: data.decode(encoding, errors))
    show_decode_error(label + ":bytearray", lambda: bytearray(data).decode(encoding, errors))
    show_decode_error(label + ":codecs", lambda: codecs.decode(data, encoding, errors))
    show_decode_error(label + ":incremental",
                      lambda: codecs.getincrementaldecoder(encoding)(errors).decode(data, True))

# Surrogateescape refuses undefined ASCII. Both table and dictionary mappings
# must retain an earlier escaped byte's arguments, but direct ASCII starts fresh.
for mapping in ({}, ""):
    for data in (b"A", b"\xffA"):
        show_decode_error("charmap-escape-" + type(mapping).__name__ + data.hex(),
                          lambda: codecs.charmap_decode(data, "surrogateescape", mapping))

# Exception reuse is local to a decode call, not the incremental decoder object.
decoder = codecs.getincrementaldecoder("utf-8")("surrogatepass")
print("handled-before-next-call", decoder.decode(b"\xed\xa0\x80", False).encode("unicode_escape").hex())
show_decode_error("fatal-in-next-call", lambda: decoder.decode(b"\xff", True))


# JSON byte coercion must decode before parsing and preserve typed error
# objects/labels/offsets, including the different BOM treatment of UTF8-SIG.
for data in (
    b'\xef\xbb\xbf"\xff"',
    b'\xff\xfe"\x00\x00\xd8\xff',
    b'\xfe\xff\x00"\xdc\x00\xff',
    b'\xff\xfe\x00\x00"\x00\x00\x00\x00\xd8\x00\x00\xff',
    b'\x00\x00\xfe\xff\x00\x00\x00"\x00\x00\xdc\x00\xff',
    b'\x00\x00\x01"\xff',
):
    show_decode_error("json-bytes-" + data.hex(), lambda: json.loads(data))
    show_decode_error("json-bytearray-" + data.hex(), lambda: json.loads(bytearray(data)))


# Raw surrogate code points remain distinct; adjacent JSON escapes combine.
# Do not use dumps as an oracle for loads. CPython executes these exact inputs.
for document in ('"\\ud800"', '"\\udfff"', '"\\ud800\\udfff"',
                 '"\ud800\udfff"', '"\U00010000"', '"\U0010ffff"'):
    print("json-text", document.encode("unicode_escape").hex(),
          json.loads(document).encode("unicode_escape").hex())
    for encoding in ("utf-8", "utf-8-sig", "utf-16", "utf-16-le", "utf-16-be",
                     "utf-32", "utf-32-le", "utf-32-be"):
        data = document.encode(encoding, "surrogatepass")
        for payload in (data, bytearray(data)):
            print("json-bytes", encoding, type(payload).__name__,
                  json.loads(payload).encode("unicode_escape").hex())

document = 'x"\U00010000\ud800" tail'
for value, end in (json.JSONDecoder().raw_decode(document, 1),
                   scanstring(document, 2)):
    print("json-end", value.encode("unicode_escape").hex(), end)
for document in ('[\n "é\ud800",?]', '["\U00010000",\n"\udfff",?]'):
    try:
        json.loads(document)
    except json.JSONDecodeError as exc:
        print("json-position", exc.doc == document, exc.pos, exc.lineno, exc.colno,
              str(exc), exc.doc.encode("unicode_escape").hex())
    else:
        raise AssertionError("invalid JSON unexpectedly parsed")


# BufferedIncrementalDecoder owns the concatenated byte snapshot before final
# coercion. Mutation may resize the original; failed coercion must preserve the
# old buffered prefix and propagate the callback's exact exception.
class MutatingFinal:
    def __init__(self, data, failure):
        self.data = data
        self.failure = failure
        self.calls = 0

    def __bool__(self):
        self.calls += 1
        self.data[:] = b"changed-and-resized"
        if self.failure is not None:
            raise self.failure
        return True


for fail in (False, True):
    decoder = codecs.getincrementaldecoder("utf-8")()
    assert decoder.decode(b"\xc3", False) == ""
    data = bytearray(b"\xa9")
    failure = RuntimeError("final-coercion") if fail else None
    final = MutatingFinal(data, failure)
    try:
        value = decoder.decode(data, final)
        assert not fail
        assert value == "é"
        assert decoder.decode(b"z", True) == "z"
        print("final-snapshot", value, final.calls, bytes(data))
    except RuntimeError as exc:
        assert fail and exc is failure
        # The failed call must not consume the original C3 prefix.
        assert decoder.decode(b"\xa9", True) == "é"
        print("final-snapshot-error", str(exc), final.calls, bytes(data))


# Registered encoding-only handlers remain lazy on valid input, but reject a
# decoding exception with TypeError rather than pretending the name is unknown.
for errors in ("xmlcharrefreplace", "namereplace"):
    for encoding, invalid in (("utf-8", b"\xff"), ("ascii", b"\xff"), ("cp1252", b"\x81")):
        assert codecs.decode(b"valid", encoding, errors) == "valid"
        try:
            codecs.decode(invalid, encoding, errors)
        except TypeError as exc:
            print("encoding-only-handler", encoding, errors, str(exc))
        else:
            raise AssertionError("encoding-only handler accepted a decode error")
    decoder = codecs.getincrementaldecoder("utf-8")(errors)
    assert decoder.decode(b"a\xc3", False) == "a"
    try:
        decoder.decode(b"z", True)
    except TypeError as exc:
        print("encoding-only-incremental", errors, str(exc))
    else:
        raise AssertionError("encoding-only handler accepted buffered decode error")
