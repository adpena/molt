# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for file iteration context."""

import os
import tempfile
from pathlib import Path


def show(label: str, value) -> None:
    print(label, value)


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / f"molt_open_iter_{os.getpid()}.txt"
path.write_text("one\ntwo\n")

with open(path, "r") as handle:
    show("iter_self", iter(handle) is handle)
    show("iter_lines", [line for line in handle])

handle = open(path, "r")
with handle as inner:
    show("enter_same", inner is handle)
show("closed_after_with", handle.closed)

path.unlink()
