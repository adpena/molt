"""Purpose: differential coverage for zlib basic."""

import zlib


data = b"hello" * 20
compressed = zlib.compress(data)
roundtrip = zlib.decompress(compressed)

print(len(compressed) < len(data), roundtrip == data)

# The public constructor owns all five Clinic conversions; range checks happen
# only after conversions and the optional dictionary export have completed.
events = []


class IndexOption:
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def __index__(self):
        events.append(self.name)
        return self.value


class Dictionary:
    def __buffer__(self, flags):
        events.append(("dictionary-get", flags))
        return memoryview(b"hello dictionary")

    def __release_buffer__(self, view):
        events.append("dictionary-release")


for level in (6, 10):
    events.clear()
    try:
        compressor = zlib.compressobj(
            IndexOption("level", level),
            IndexOption("method", 8),
            IndexOption("window", 15),
            IndexOption("memory", 8),
            IndexOption("strategy", 0),
            Dictionary(),
        )
        dictionary_output = (
            compressor.compress(b"hello dictionary") + compressor.flush()
        )
        print("dictionary-header", bool(dictionary_output[1] & 32))
        print(
            "dictionary-checksum",
            dictionary_output[2:6]
            == zlib.adler32(b"hello dictionary").to_bytes(4, "big"),
        )
    except Exception as error:
        print("constructor-error", type(error).__name__)
    print("constructor-order", events)

for dictionary in (None, b"", b"dictionary"):
    try:
        compressor = zlib.compressobj(wbits=31, zdict=dictionary)
        print("gzip-dictionary", "accepted")
    except Exception as error:
        print("gzip-dictionary", type(error).__name__)

# Real raw/zlib/gzip window choices and every supported strategy must survive
# multiple writes and non-final flushes, then retire exactly at Z_FINISH.
for window in (-15, -9, 8, 9, 15, 25, 31):
    for strategy in range(5):
        compressor = zlib.compressobj(6, 8, window, 1, strategy)
        output = compressor.compress(b"first")
        output += compressor.flush(2)
        output += compressor.compress(b"second")
        output += compressor.flush(3)
        output += compressor.flush()
        read_window = 15 if window == 8 else window
        print(
            "stream",
            window,
            strategy,
            zlib.decompress(output, read_window) == b"firstsecond",
        )
        print("finished-noop", compressor.flush(0) == b"")

for mode in (1, 2, 3, 5):
    compressor = zlib.compressobj(wbits=-15)
    output = compressor.compress(b"first") + compressor.flush(mode)
    output += compressor.compress(b"second") + compressor.flush()
    print("flush-mode", mode, zlib.decompress(output, -15) == b"firstsecond")

for options in (
    (10, 8, 15, 8, 0),
    (6, 7, 15, 8, 0),
    (6, 8, -8, 8, 0),
    (6, 8, 24, 8, 0),
    (6, 8, 15, 0, 0),
    (6, 8, 15, 8, 5),
):
    try:
        zlib.compressobj(*options)
    except Exception as error:
        print("invalid-option", type(error).__name__)

for mode in (-1, 6, 1 << 31):
    compressor = zlib.compressobj()
    try:
        compressor.flush(IndexOption("flush", mode))
    except Exception as error:
        print("invalid-flush", type(error).__name__)
    print(
        "after-invalid-flush",
        zlib.decompress(compressor.compress(b"ok") + compressor.flush()) == b"ok",
    )

# One-shot input export precedes option callbacks and remains pinned until the
# entire native call returns. Reentrant resize must fail, then succeed afterward.
input_bytes = bytearray(b"lease")


class MutatingLevel:
    def __index__(self):
        try:
            input_bytes.extend(b"!")
        except BufferError:
            events.append("pinned")
        input_bytes[0] = ord("L")
        return 6


events.clear()
result = zlib.compress(input_bytes, MutatingLevel(), IndexOption("window", -15))
input_bytes.extend(b"!")
print("one-shot-lease", events, zlib.decompress(result, -15), bytes(input_bytes))

saved_error = RuntimeError("index callback")


class FailedIndex:
    def __index__(self):
        events.append("failed-index")
        raise saved_error


for operation in (
    lambda: zlib.compress(b"x", FailedIndex()),
    lambda: zlib.compressobj(FailedIndex()),
    lambda: zlib.compressobj().flush(FailedIndex()),
):
    events.clear()
    try:
        operation()
    except RuntimeError as error:
        print("index-identity", error is saved_error, events)

for window in (-15, 9, 15, 25, 31):
    compressed = zlib.compress(b"one shot", wbits=window)
    print(
        "one-shot-window", window, zlib.decompress(compressed, window, 1) == b"one shot"
    )

# Finished-stream argument admission still precedes the stream-state error.
compressor = zlib.compressobj()
compressor.flush()
for value in (None, b"x"):
    try:
        compressor.compress(value)
    except Exception as error:
        print("finished-admission", type(error).__name__)

compressor = zlib.compressobj()
events.clear()


class FinishDuringExport:
    def __buffer__(self, flags):
        events.append("get")
        compressor.flush()
        return memoryview(b"after finish")

    def __release_buffer__(self, view):
        events.append("release")


try:
    compressor.compress(FinishDuringExport())
except zlib.error:
    print("reentrant-finish", events)

compressor = zlib.compressobj()


class FinishDuringIndex:
    def __index__(self):
        compressor.flush()
        return 0


print("reentrant-noop", compressor.flush(FinishDuringIndex()) == b"")
