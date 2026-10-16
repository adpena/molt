# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read,time.wall
# MOLT_META: stderr=exact
"""Purpose: differential coverage for gzip basic."""

import gzip
import io


data = b"hello" * 10
buf = io.BytesIO()
with gzip.GzipFile(fileobj=buf, mode="wb") as handle:
    handle.write(data)

payload = buf.getvalue()
roundtrip = gzip.decompress(payload)

print(len(payload) > 0, roundtrip == data)


class GzipInput:
    def __init__(self, data):
        self.data = bytearray(data)
        self.events = []
        self.view = None
    def __buffer__(self, flags):
        self.events.append(("buffer", flags))
        self.view = memoryview(self.data)
        return self.view
    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))
        self.view = None
    def __getitem__(self, key):
        self.events.append(("slice", key.start, key.stop, key.step))
        return self.data[key]

source = GzipInput(gzip.compress(b"gzip-protocol", mtime=0))
print("gzip-observable-decode", gzip.decompress(source), source.events)
source.data.extend(b"!")
print("gzip-decode-unpinned", True)

sink = io.BytesIO()
source = GzipInput(b"gzip-write")
with gzip.GzipFile(fileobj=sink, mode="wb", mtime=0) as writer:
    print("gzip-buffer-write", writer.write(source), source.events)
print("gzip-buffer-write-roundtrip", gzip.decompress(sink.getvalue()))
source.data.extend(b"!")
print("gzip-write-unpinned", True)


sink = io.BytesIO()
writer = gzip.GzipFile(fileobj=sink, mode="wb", mtime=0)
class ClosingGzipInput(GzipInput):
    def __buffer__(self, flags):
        writer.close()
        return super().__buffer__(flags)
source = ClosingGzipInput(b"late")
try:
    writer.write(source)
except Exception as exc:
    print("gzip-close-reentry", type(exc).__name__, writer.closed, source.events)
source.data.extend(b"!")
print("gzip-close-reentry-unpinned", True)
# The reentrant close can leave bytes in the still-open inner writer. Retire
# that buffer while its weak gzip owner is alive, after recording the oracle.
try:
    writer._buffer.close()
except AttributeError:
    pass  # The already-closed gzip has fileobj=None during this cleanup flush.



# Literal members independently exercise raw-deflate EOF and the eight-byte
# trailer, then concatenated-member routing through the same zlib stream owner.
member = (
    b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03"
    b"\x01\x0b\x00\xf4\xffhello world"
    b"\x85\x11\x4a\x0d\x0b\x00\x00\x00"
)
print("gzip-literal", gzip.decompress(member))
print("gzip-concatenated", gzip.decompress(member + b"\x00\x00" + member))
for label, damaged in (("body", member[:18]), ("trailer", member[:-1]),
                       ("crc", member[:-8] + b"\x00\x00\x00\x00" + member[-4:])):
    try:
        gzip.decompress(damaged)
    except Exception as exc:
        print("gzip-member-error", label, type(exc).__name__)

import struct

# Writing is incremental for both borrowed and owned files; the raw sink retains
# only byte counts and a rolling checksum while the codec owns bounded state.
class CountingSink:
    def __init__(self):
        self.count = 0
        self.parts = 0
        self.tail = b""
        self.flushes = 0
    def write(self, data):
        self.count += len(data)
        self.parts += 1
        self.tail = (self.tail + data)[-8:]
        return len(data)
    def flush(self):
        self.flushes += 1
sink = CountingSink()
writer = gzip.GzipFile(fileobj=sink, mode="wb", compresslevel=0, mtime=0)
header_count = sink.count
chunk = b"0123456789abcdef" * 8192
for _ in range(8):
    writer.write(chunk)
print("gzip-stream-progress", header_count == 10, sink.count > header_count, writer.size == len(chunk) * 8)
writer.flush()
print("gzip-stream-flush", sink.flushes)
writer.close()
print("gzip-stream-trailer", struct.unpack("<II", sink.tail)[1], writer.closed, not writer._buffer.closed)

import tempfile
import os
fd, filename = tempfile.mkstemp(suffix=".gz")
os.close(fd)
try:
    with gzip.GzipFile(filename=filename, mode="wb", compresslevel=0, mtime=0) as writer:
        writer.write(b"owned stream")
    with open(filename, "rb") as file:
        member = file.read()
    print("gzip-owned-stream", gzip.decompress(member), member[:3], member[-4:])
finally:
    os.unlink(filename)

# Maintained versions differ in close admission for a closed or absent buffer.
for state in ("closed", "absent"):
    sink = io.BytesIO()
    writer = gzip.GzipFile(fileobj=sink, mode="wb", mtime=0)
    buffer = writer._buffer
    if state == "closed":
        buffer.close()
    else:
        writer._buffer = None
    try:
        writer.close()
    except Exception as exc:
        print("gzip-close-buffer", state, type(exc).__name__, writer.closed)
    else:
        print("gzip-close-buffer", state, "ok", writer.closed)
    writer._buffer = buffer
    writer._close()
    buffer.close()
    sink.close()
