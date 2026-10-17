"""Native I/O provider facade for Molt.

Every program's core closure contains this module, so it binds runtime
intrinsics and imports no other module: an import statement here would give a
print-only program the runtime import machinery.
"""

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


_MOLT_TEXT_ENCODING = _require_intrinsic("molt_io_text_encoding")


def text_encoding(encoding, stacklevel=2, /):
    """Return the explicit encoding or the current default text selector."""
    # The runtime owns CPython's C rule, warning and stack level included.
    return _MOLT_TEXT_ENCODING(encoding, stacklevel)


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
