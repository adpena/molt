# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for file buffering text."""

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
path = tmpdir / f"molt_open_buffer_{os.getpid()}.txt"
path.write_text("a\nb\n")

with open(path, "rb", buffering=0) as handle:
    show("buffering_rb_type", type(handle).__name__)
    show("buffering_rb_read", handle.read())

show_err("buffering_text", lambda: open(path, "r", buffering=0))

with open(path, "r", buffering=1) as handle:
    show("line_buffering", handle.line_buffering)
    show("readline", handle.readline())

with open(path, "r", buffering=-1) as handle:
    show("buffer_class", type(handle.buffer).__name__)

path.unlink()
