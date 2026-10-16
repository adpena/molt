"""Native I/O provider facade for Molt."""

import sys as _sys
from operator import index as _index

from _intrinsics import require_intrinsic as _require_intrinsic

# Canonical publication installs admitted native exports before this body runs.
# Pure profiles retain the memory I/O surface without publishing open.
_NS = globals()

_MOLT_IO_CLASS = _require_intrinsic("molt_io_class")


UnsupportedOperation = _require_intrinsic("molt_builtin_class_lookup")(
    "UnsupportedOperation"
)


SEEK_SET = 0
SEEK_CUR = 1
SEEK_END = 2
DEFAULT_BUFFER_SIZE = _require_intrinsic("molt_io_default_buffer_size")()

_IOBase = _MOLT_IO_CLASS("_IOBase")
_RawIOBase = _MOLT_IO_CLASS("_RawIOBase")
_BufferedIOBase = _MOLT_IO_CLASS("_BufferedIOBase")
_TextIOBase = _MOLT_IO_CLASS("_TextIOBase")
FileIO = _MOLT_IO_CLASS("FileIO")
BufferedReader = _MOLT_IO_CLASS("BufferedReader")
BufferedWriter = _MOLT_IO_CLASS("BufferedWriter")
BufferedRandom = _MOLT_IO_CLASS("BufferedRandom")
TextIOWrapper = _MOLT_IO_CLASS("TextIOWrapper")
BytesIO = _MOLT_IO_CLASS("BytesIO")
StringIO = _MOLT_IO_CLASS("StringIO")


def _default_text_encoding():
    return "utf-8" if _sys.flags.utf8_mode else "locale"


def text_encoding(encoding, stacklevel=2, /):
    """Return the explicit encoding or the current default text selector."""
    stacklevel = _index(stacklevel)
    if stacklevel < -2147483648 or stacklevel > 2147483647:
        raise OverflowError("Python int too large to convert to C int")
    if encoding is None:
        encoding = _default_text_encoding()
        if _sys.flags.warn_default_encoding:
            import warnings

            # The native public callable has no Python wrapper frame in CPython.
            warnings.warn(
                "'encoding' argument not specified.",
                EncodingWarning,
                max(1, stacklevel) + 1,
            )
    return encoding


text_encoding.__text_signature__ = "($module, encoding, stacklevel=2, /)"
_require_intrinsic("molt_function_set_builtin")(text_encoding)


__all__ = [
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
    "DEFAULT_BUFFER_SIZE",
    "FileIO",
    "BufferedReader",
    "BufferedWriter",
    "BufferedRandom",
    "TextIOWrapper",
    "BytesIO",
    "StringIO",
    "UnsupportedOperation",
    "open",
    "text_encoding",
]
__all__ = [name for name in __all__ if name in _NS]

del _MOLT_IO_CLASS, _NS
globals().pop("_require_intrinsic", None)
