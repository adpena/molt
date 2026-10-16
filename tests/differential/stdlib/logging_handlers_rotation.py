# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read,os.getpid
"""Purpose: size rotation, callback ownership and shared file configuration."""

import io
import logging

print("bare-parent", hasattr(logging, "handlers"))

import logging.handlers
import os
import sys
import tempfile
from pathlib import Path

print("child-owner", logging.handlers is sys.modules["logging.handlers"])
print("encoding-owner", type(io.text_encoding).__name__, io.text_encoding.__module__)
print("encoding-signature", io.text_encoding.__text_signature__)
print(
    "encoding-explicit",
    io.text_encoding("ascii"),
    io.text_encoding(None) == ("utf-8" if sys.flags.utf8_mode else "locale"),
)
try:
    io.text_encoding(encoding="ascii")
except TypeError:
    print("encoding-posonly", True)


def emit(handler, message):
    record = logging.LogRecord("rotation", logging.INFO, __file__, 1, message, (), None)
    handler.handle(record)


def contents(path):
    return path.read_text(encoding="utf-8") if path.exists() else None


default_handler = logging.Handler()
explicit_handler = logging.Handler()
explicit_handler.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))
record = logging.LogRecord("rotation", logging.INFO, __file__, 1, "selected", (), None)
print("default-format", default_handler.format(record), explicit_handler.format(record))
default_handler.close()
explicit_handler.close()

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    path = root / "startup-encoding.log"
    handler = logging.handlers.RotatingFileHandler(path, errors="backslashreplace")
    emit(handler, "é")
    handler.close()
    print("startup-encoding", path.read_bytes())

    path = root / "baseline.log"
    handler = logging.handlers.RotatingFileHandler(path, maxBytes=20, backupCount=1)
    emit(handler, "first")
    emit(handler, "second")
    handler.close()
    print("baseline", path.exists(), Path(str(path) + ".1").exists())

    path = root / "bounded.log"
    handler = logging.handlers.RotatingFileHandler(path, maxBytes=6, backupCount=2)
    for message in ("aaaa", "bbbb", "cccc", "dddd"):
        emit(handler, message)
    handler.close()
    print(
        "backups",
        [contents(Path(str(path) + suffix)) for suffix in ("", ".1", ".2", ".3")],
    )

    # CPython combines the current byte position with the next message's
    # character count, rather than encoding the next message to measure it.
    path = root / "unicode-size.log"
    handler = logging.handlers.RotatingFileHandler(
        path, maxBytes=6, backupCount=1, encoding="utf-8"
    )
    emit(handler, "é")
    emit(handler, "é")
    handler.close()
    print("character-measure", contents(path), Path(str(path) + ".1").exists())

    path = root / "oversize.log"
    handler = logging.handlers.RotatingFileHandler(path, maxBytes=1, backupCount=1)
    emit(handler, "oversized")
    print("empty-oversize", contents(path), Path(str(path) + ".1").exists())
    handler.close()

    path = root / "append.log"
    path.write_text("existing\n", encoding="utf-8")
    handler = logging.handlers.RotatingFileHandler(
        path, mode="w", maxBytes=1, backupCount=0
    )
    emit(handler, "next")
    handler.close()
    print("zero-backups", handler.mode, contents(path), Path(str(path) + ".1").exists())

    path = root / "disabled.log"
    handler = logging.handlers.RotatingFileHandler(
        path, maxBytes=0, backupCount=2, delay=True
    )
    print("delayed", handler.stream is None, path.exists())
    emit(handler, "first")
    emit(handler, "second")
    handler.close()
    print("disabled", contents(path), Path(str(path) + ".1").exists())

    class CustomHandler(logging.handlers.RotatingFileHandler):
        def namer(self, name):
            return name + ".saved"

        def rotator(self, source, dest):
            rotations.append((Path(source).name, Path(dest).name))
            os.rename(source, dest)

    rotations = []
    path = root / "custom.log"
    handler = CustomHandler(path, maxBytes=4, backupCount=2, delay=True)
    for message in ("one", "two", "three"):
        emit(handler, message)
    handler.close()
    print("callbacks", rotations)
    print(
        "named-backups",
        [contents(Path(str(path) + suffix)) for suffix in ("", ".1.saved", ".2.saved")],
    )

    path = root / "noncallable.log"
    handler = logging.handlers.RotatingFileHandler(path, maxBytes=1, backupCount=1)
    handler.namer = 42
    handler.rotator = 42
    emit(handler, "first")
    emit(handler, "second")
    handler.close()
    print("noncallable", contents(path), contents(Path(str(path) + ".1")))

    path = root / "delayed-roll.log"
    handler = logging.handlers.RotatingFileHandler(
        path, maxBytes=10, backupCount=1, delay=True
    )
    handler.doRollover()
    print(
        "rollover-delay",
        handler.stream is None,
        path.exists(),
        Path(str(path) + ".1").exists(),
    )
    handler.close()

    class SelectedPath:
        def __fspath__(self):
            return str(root / "encoded.log")

        def __str__(self):
            raise AssertionError("fspath is the path authority")

    handler = logging.handlers.RotatingFileHandler(
        SelectedPath(),
        encoding="ascii",
        errors="backslashreplace",
        maxBytes=40,
        delay=True,
    )
    emit(handler, "caf\N{LATIN SMALL LETTER E WITH ACUTE}")
    handler.close()
    print("path-errors", handler.errors, contents(root / "encoded.log"))

    class FailingRotation(logging.handlers.RotatingFileHandler):
        def handleError(self, record):
            failures.append(sys.exc_info()[1])

    error = OSError("rotation denied")
    failures = []
    path = root / "failure.log"
    path.write_text("prior\n", encoding="utf-8")
    handler = FailingRotation(path, maxBytes=1, backupCount=1)

    def reject(source, dest):
        raise error

    handler.rotator = reject
    emit(handler, "next")
    print(
        "rotation-error",
        len(failures),
        failures[0] is error,
        contents(path),
        handler.stream is None,
    )
    handler.close()

    # A changed working directory must not change the selected delayed file.
    previous = os.getcwd()
    try:
        os.chdir(root)
        handler = logging.handlers.RotatingFileHandler(
            "relative.log", maxBytes=1, backupCount=1, delay=True
        )
        (root / "elsewhere").mkdir()
        os.chdir(root / "elsewhere")
        emit(handler, "first")
        emit(handler, "second")
        handler.close()
        print(
            "absolute-owner",
            contents(root / "relative.log"),
            contents(root / "relative.log.1"),
            (root / "elsewhere/relative.log").exists(),
        )
    finally:
        os.chdir(previous)
