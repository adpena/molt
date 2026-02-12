# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for file detach."""

import os
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
