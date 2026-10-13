# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read,os.getpid
"""Purpose: differential coverage for file detach."""

import os
import io
import tempfile
from pathlib import Path


def show_err(label: str, func) -> None:
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / f"molt_detach_{os.getpid()}.txt"
if path.exists():
    path.unlink()

path.write_text("hello")

handle = open(path, "r")
buf = handle.detach()
print("text_detach", buf is not None)
show_err("text_read", handle.read)
show_err("text_close", handle.close)
show_err("text_closed", lambda: handle.closed)
print("text_buffer_none", handle.buffer is None)
print("text_buf_read", buf.read())
buf.close()

handle = open(path, "rb")
raw = handle.detach()
print("binary_detach", raw is not None)
show_err("binary_read", handle.read)
show_err("binary_close", handle.close)
show_err("binary_closed", lambda: handle.closed)
print("binary_raw_read", raw.read())
raw.close()

path.unlink()

path.write_bytes(b"abcdef")
handle = open(path, "rb", buffering=4)
mode_before = handle.mode
raw = handle.raw
print("raw_identity", raw is handle.raw, type(raw) is io.FileIO)
print("raw_mode", mode_before, handle.mode, raw.mode)
print("raw_closefd", raw.closefd)
raw.name = "published-name"
print("raw_name", handle.name, raw.name)
print("buffered_read", handle.read(1))
print("raw_position", raw.tell())
detached = handle.detach()
print("detached_identity", detached is raw, handle.raw is None)
print("detached_readahead", detached.read())
detached.close()

for constructor, mode in (
    (io.BufferedReader, "rb"),
    (io.BufferedWriter, "wb"),
    (io.BufferedRandom, "r+b"),
):
    raw = io.FileIO(path, mode)
    buffered = constructor(raw)
    print("explicit_raw", buffered.raw is raw, buffered.detach() is raw)
    raw.close()

with open(path, "w+") as text:
    print("text_raw_closefd", text.buffer.raw.closefd)
    text.buffer.raw.name = "text-published"
    print("text_raw_name", text.name, text.buffer.name)
path.unlink()

for selected_mode in ("xb", "xb+"):
    with open(path, selected_mode) as handle:
        before = handle.mode
        raw = handle.raw
        print("exclusive_raw_mode", before, handle.mode, raw.mode)
    path.unlink()

path.write_bytes(b"fd-owner")
fd = os.open(path, os.O_RDONLY)
try:
    oversized = fd + (1 << 32)
    for custom_opener in (False, True):
        try:
            if custom_opener:
                open(path, "rb", opener=lambda path, flags: oversized)
            else:
                open(oversized, "rb")
        except Exception as error:
            print("descriptor-width", custom_opener, type(error).__name__)
    print("descriptor-width-owner", os.read(fd, 2))
finally:
    os.close(fd)
    path.unlink()

for stream_type in (
    io.IOBase,
    io.RawIOBase,
    io.BufferedIOBase,
    io.TextIOBase,
    io.FileIO,
    io.BytesIO,
    io.StringIO,
):
    print("detach-class", stream_type.__name__, hasattr(stream_type, "detach"))
for stream in (io.BufferedIOBase(), io.TextIOBase(), io.BytesIO(), io.StringIO()):
    try:
        stream.detach()
    except Exception as error:
        print("detach-unsupported", type(stream).__name__, type(error).__name__)
    stream.close()
for stream_type in (io.BufferedIOBase, io.TextIOBase):
    print(
        "detach-abstract-surface",
        hasattr(stream_type, "getvalue"),
        hasattr(stream_type, "reconfigure"),
    )


class CustomBuffered(io.BufferedIOBase):
    def detach(self):
        return "override"


print("detach-override", CustomBuffered().detach())
path.write_bytes(b"raw")
with io.FileIO(path, "rb") as raw:
    try:
        io.BufferedIOBase.detach(raw)
    except Exception as error:
        print("detach-owner", type(error).__name__)
for stream in (io.FileIO(path, "rb"), open(path, "rb", buffering=0)):
    print("raw-detach-absent", hasattr(stream, "detach"))
    stream.close()
path.unlink()
