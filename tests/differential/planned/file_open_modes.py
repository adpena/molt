# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
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
path = tmpdir / f"molt_open_modes_{os.getpid()}.txt"
if path.exists():
    path.unlink()

show("pathlike", isinstance(path, Path))

with open(path, "w") as handle:
    handle.write("one")
with open(path, "r") as handle:
    show("read_w", handle.read())

with open(path, "a") as handle:
    handle.write("two")
with open(path, "r") as handle:
    show("read_a", handle.read())

with open(path, "r+") as handle:
    handle.seek(0)
    handle.write("X")
    handle.seek(0)
    show("rplus", handle.read())

with open(path, "w+") as handle:
    handle.write("wipe")
    handle.seek(0)
    show("wplus", handle.read())
with open(path, "r") as handle:
    show("wplus_read", handle.read())

with open(path, "a+") as handle:
    handle.write("Z")
    handle.seek(0)
    show("aplus", handle.read())

if path.exists():
    path.unlink()
with open(path, "x") as handle:
    handle.write("new")
show_err("x_exists", lambda: open(path, "x"))

with open(path, mode="r") as handle:
    show("mode_kw", handle.read())
show_err("mode_dupe", lambda: open(path, "r", mode="r"))

show_err("mode_rw", lambda: open(path, "rw"))
show_err("mode_rr", lambda: open(path, "rr"))
show_err("mode_rbt", lambda: open(path, "rbt"))

path.unlink()
