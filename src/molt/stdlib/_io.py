"""Native I/O provider facade for Molt."""

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
DEFAULT_BUFFER_SIZE = 8192

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
]
__all__ = [name for name in __all__ if name in _NS]

del _MOLT_IO_CLASS, _NS
globals().pop("_require_intrinsic", None)
