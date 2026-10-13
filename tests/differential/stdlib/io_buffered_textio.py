"""Purpose: differential coverage for io buffered textio."""

import io


raw = io.BytesIO()
buf = io.BufferedWriter(raw)
buf.write(b"hello")
buf.flush()

raw.seek(0)
reader = io.BufferedReader(raw)
print(reader.read().decode())

raw2 = io.BytesIO(b"a\r\n\n")
text = io.TextIOWrapper(raw2, newline=None, encoding="utf-8")
print(text.read())

# Capability admission follows the inherited concrete buffered owner, not
# exact class identity. FileIO supplies real read-only/write-only handles.
import os
import tempfile


class Reader(io.BufferedReader):
    pass


class Writer(io.BufferedWriter):
    pass


class Random(io.BufferedRandom):
    pass


with tempfile.TemporaryDirectory() as directory:
    path = os.path.join(directory, "buffered-construction")
    with open(path, "wb") as stream:
        stream.write(b"abc")
    for kind, mode in ((Reader, "r"), (Writer, "w"), (Random, "r+")):
        raw = io.FileIO(path, mode)
        stream = kind(raw)
        print("subclass", type(stream) is kind, stream.readable(), stream.writable())
        stream.close()
    for kind, mode in ((Reader, "w"), (Writer, "r"), (Random, "r"), (Random, "w")):
        raw = io.FileIO(path, mode)
        try:
            kind(raw)
        except io.UnsupportedOperation as error:
            print("capability-refused", kind.__name__, type(error).__name__)
        finally:
            raw.close()
