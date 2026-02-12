# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for file reconfigure."""

import os
import tempfile
from pathlib import Path


def show(label: str, value) -> None:
    print(label, value)


def show_err(label: str, func) -> None:
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / f"molt_reconfigure_{os.getpid()}.txt"
if path.exists():
    path.unlink()

path.write_bytes(b"a\r\nb\r")

handle = open(path, "r", newline="")
show("newline_initial", repr(handle.read()))
handle.seek(0)
handle.reconfigure(newline=None)
handle.seek(0)
show("newline_after", repr(handle.read()))
handle.close()

handle = open(path, "r", encoding="latin-1")
show("reconfigure_none", handle.reconfigure() is None)
show("encoding_initial", handle.encoding)
handle.reconfigure(encoding=None)
show("encoding_none", handle.encoding)
show_err("reconfigure_pos", lambda: handle.reconfigure("utf-8"))
show_err("reconfigure_kw", lambda: handle.reconfigure(foo="bar"))
show_err("newline_type", lambda: handle.reconfigure(newline=123))
show_err("newline_bad", lambda: handle.reconfigure(newline="bad"))
show_err("encoding_type", lambda: handle.reconfigure(encoding=123))
show_err("encoding_bad", lambda: handle.reconfigure(encoding="madeup"))
handle.reconfigure(line_buffering=1, write_through=1)
show("line_buffering", handle.line_buffering)
show("write_through", handle.write_through)
show_err("line_buffering_type", lambda: handle.reconfigure(line_buffering="no"))
show_err("write_through_type", lambda: handle.reconfigure(write_through="no"))
handle.close()

bin_handle = open(path, "rb")
show("binary_has_reconfigure", hasattr(bin_handle, "reconfigure"))
bin_handle.close()

path.unlink()
