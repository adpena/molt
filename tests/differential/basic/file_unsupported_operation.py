# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,os.getpid,env.read
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


# Native file writelines consumes arbitrary iterators and preserves callback errors.
with open(path, "w") as handle:
    assert handle.writelines(iter(["one\n", "two\n"])) is None
assert path.read_text() == "one\ntwo\n"

import gc
import weakref


class LinesFailure(RuntimeError):
    pass


acquisition_failure = LinesFailure("acquisition")
next_failure = LinesFailure("next")
references = []


class BadLines:
    def __iter__(self):
        raise acquisition_failure


class Line(str):
    pass


class Lines:
    def __init__(self):
        self.position = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.position:
            raise next_failure
        self.position += 1
        value = Line("prefix\n")
        references.append(weakref.ref(value))
        return value


with open(path, "w") as handle:
    for source, expected in (
        (BadLines(), acquisition_failure),
        (Lines(), next_failure),
    ):
        try:
            handle.writelines(source)
        except LinesFailure as caught:
            assert caught is expected
        else:
            raise AssertionError("writelines swallowed callback failure")
assert path.read_text() == "prefix\n"
gc.collect()
assert len(references) == 1 and references[0]() is None
print("writelines iterator", len(references), references[0]() is None)

path.unlink()
