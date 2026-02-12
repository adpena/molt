# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for file unsupported operation."""

import os
import tempfile
from pathlib import Path


def show_err(label: str, func) -> None:
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / f"molt_unsupported_operation_{os.getpid()}.txt"
if path.exists():
    path.unlink()

path.write_text("hello")

with open(path, "w") as handle:
    show_err("w_read", handle.read)
    show_err("w_readline", handle.readline)
    show_err("w_readlines", handle.readlines)

with open(path, "wb") as handle:
    show_err("wb_readinto", lambda: handle.readinto(bytearray(4)))

with open(path, "r") as handle:
    show_err("r_write", lambda: handle.write("x"))
    show_err("r_writelines", lambda: handle.writelines(["x"]))
    show_err("r_truncate", handle.truncate)

path.unlink()
