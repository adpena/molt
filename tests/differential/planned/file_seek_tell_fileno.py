# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
import os
import tempfile
from pathlib import Path


def show(label: str, value) -> None:
    print(label, value)


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / f"molt_open_seek_{os.getpid()}.txt"
path.write_text("abcdef")

with open(path, "rb") as handle:
    show("fileno_is_int", isinstance(handle.fileno(), int))
    show("tell0", handle.tell())
    handle.seek(2)
    show("tell2", handle.tell())
    show("read2", handle.read(2))
    show("tell_after", handle.tell())
    handle.seek(-1, os.SEEK_END)
    show("seek_end", handle.tell())

with open(path, "r+") as handle:
    handle.truncate(3)

with open(path, "r") as handle:
    show("truncate_read", handle.read())

path.unlink()
