# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for file text encoding newline."""

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
path = tmpdir / f"molt_open_text_{os.getpid()}.txt"
path.write_bytes(b"a\r\nb\rc\n")

with open(path, "r", newline=None) as handle:
    show("newline_none", repr(handle.read()))
with open(path, "r", newline="") as handle:
    show("newline_empty", repr(handle.read()))
with open(path, "r", newline="\n") as handle:
    show("newline_n", repr(handle.read()))

show_err("newline_x", lambda: open(path, "r", newline="x"))
show_err("newline_binary", lambda: open(path, "rb", newline="\n"))

show_err("encoding_binary", lambda: open(path, "rb", encoding="utf-8"))
show_err("encoding_type", lambda: open(path, "r", encoding=123))
show_err("errors_type", lambda: open(path, "r", errors=123))

path.unlink()
