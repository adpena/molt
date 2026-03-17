"""Fully intrinsic-backed `bz2` module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- One-shot compress / decompress ---
_MOLT_BZ2_COMPRESS = _require_intrinsic("molt_bz2_compress", globals())
_MOLT_BZ2_DECOMPRESS = _require_intrinsic("molt_bz2_decompress", globals())

# --- Incremental compressor ---
_MOLT_BZ2_COMPRESSOR_NEW = _require_intrinsic("molt_bz2_compressor_new", globals())
_MOLT_BZ2_COMPRESSOR_COMPRESS = _require_intrinsic(
    "molt_bz2_compressor_compress", globals()
)
_MOLT_BZ2_COMPRESSOR_FLUSH = _require_intrinsic("molt_bz2_compressor_flush", globals())
_MOLT_BZ2_COMPRESSOR_DROP = _require_intrinsic("molt_bz2_compressor_drop", globals())

# --- Incremental decompressor ---
_MOLT_BZ2_DECOMPRESSOR_NEW = _require_intrinsic("molt_bz2_decompressor_new", globals())
_MOLT_BZ2_DECOMPRESSOR_DECOMPRESS = _require_intrinsic(
    "molt_bz2_decompressor_decompress", globals()
)
_MOLT_BZ2_DECOMPRESSOR_EOF = _require_intrinsic("molt_bz2_decompressor_eof", globals())
_MOLT_BZ2_DECOMPRESSOR_NEEDS_INPUT = _require_intrinsic(
    "molt_bz2_decompressor_needs_input", globals()
)
_MOLT_BZ2_DECOMPRESSOR_UNUSED_DATA = _require_intrinsic(
    "molt_bz2_decompressor_unused_data", globals()
)
_MOLT_BZ2_DECOMPRESSOR_DROP = _require_intrinsic(
    "molt_bz2_decompressor_drop", globals()
)

# --- File handle intrinsics ---
_MOLT_BZ2_FILE_OPEN = _require_intrinsic("molt_bz2_file_open", globals())
_MOLT_BZ2_FILE_READ = _require_intrinsic("molt_bz2_file_read", globals())
_MOLT_BZ2_FILE_WRITE = _require_intrinsic("molt_bz2_file_write", globals())
_MOLT_BZ2_FILE_CLOSE = _require_intrinsic("molt_bz2_file_close", globals())
_MOLT_BZ2_FILE_DROP = _require_intrinsic("molt_bz2_file_drop", globals())


class BZ2Compressor:
    """Incremental bz2 compressor backed by Rust intrinsics."""

    def __init__(self, compresslevel: int = 9) -> None:
        if not 1 <= compresslevel <= 9:
            raise ValueError("compresslevel must be between 1 and 9")
        self._handle = _MOLT_BZ2_COMPRESSOR_NEW(compresslevel)
        self._flushed = False

    def compress(self, data: bytes) -> bytes:
        if self._flushed:
            raise ValueError("Compressor has been flushed")
        return bytes(_MOLT_BZ2_COMPRESSOR_COMPRESS(self._handle, data))

    def flush(self) -> bytes:
        if self._flushed:
            raise ValueError("Repeated call to flush()")
        self._flushed = True
        return bytes(_MOLT_BZ2_COMPRESSOR_FLUSH(self._handle))

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_BZ2_COMPRESSOR_DROP(handle)
            except Exception:
                pass


class BZ2Decompressor:
    """Incremental bz2 decompressor backed by Rust intrinsics."""

    def __init__(self) -> None:
        self._handle = _MOLT_BZ2_DECOMPRESSOR_NEW()

    def decompress(self, data: bytes, max_length: int = -1) -> bytes:
        if self.eof:
            raise EOFError("End of stream already reached")
        return bytes(_MOLT_BZ2_DECOMPRESSOR_DECOMPRESS(self._handle, data, max_length))

    @property
    def eof(self) -> bool:
        return bool(_MOLT_BZ2_DECOMPRESSOR_EOF(self._handle))

    @property
    def needs_input(self) -> bool:
        return bool(_MOLT_BZ2_DECOMPRESSOR_NEEDS_INPUT(self._handle))

    @property
    def unused_data(self) -> bytes:
        return bytes(_MOLT_BZ2_DECOMPRESSOR_UNUSED_DATA(self._handle))

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_BZ2_DECOMPRESSOR_DROP(handle)
            except Exception:
                pass


class BZ2File:
    """BZ2 file object backed entirely by Rust intrinsics."""

    def __init__(
        self,
        filename: str,
        mode: str = "rb",
        *,
        compresslevel: int = 9,
    ) -> None:
        if mode not in ("rb", "wb", "ab", "r", "w", "a"):
            raise ValueError(f"Invalid mode: {mode!r}")
        self._handle = _MOLT_BZ2_FILE_OPEN(filename, mode, compresslevel)
        self._mode = mode
        self._closed = False
        self._writing = "w" in mode or "a" in mode
        self._reading = "r" in mode

    def write(self, data: bytes) -> int:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if not self._writing:
            raise OSError("File not open for writing")
        return int(_MOLT_BZ2_FILE_WRITE(self._handle, data))

    def read(self, size: int = -1) -> bytes:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if not self._reading:
            raise OSError("File not open for reading")
        return bytes(_MOLT_BZ2_FILE_READ(self._handle, size))

    def readable(self) -> bool:
        return self._reading

    def writable(self) -> bool:
        return self._writing

    def seekable(self) -> bool:
        return False

    def close(self) -> None:
        if self._closed:
            return
        _MOLT_BZ2_FILE_CLOSE(self._handle)
        _MOLT_BZ2_FILE_DROP(self._handle)
        self._handle = None
        self._closed = True

    @property
    def closed(self) -> bool:
        return self._closed

    def __enter__(self) -> "BZ2File":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass


def compress(data: bytes, compresslevel: int = 9) -> bytes:
    """Compress *data* in one shot, returning the compressed bytes."""
    try:
        return bytes(_MOLT_BZ2_COMPRESS(data, compresslevel))
    except Exception as exc:
        raise ValueError(str(exc)) from exc


def decompress(data: bytes) -> bytes:
    """Decompress *data* in one shot, returning the decompressed bytes."""
    try:
        return bytes(_MOLT_BZ2_DECOMPRESS(data))
    except Exception as exc:
        raise ValueError(str(exc)) from exc


def open(
    filename: str,
    mode: str = "rb",
    compresslevel: int = 9,
    encoding: str | None = None,
    errors: str | None = None,
    newline: str | None = None,
) -> BZ2File:
    """Open a bz2-compressed file in binary mode."""
    return BZ2File(filename, mode, compresslevel=compresslevel)


__all__ = [
    "compress",
    "decompress",
    "open",
    "BZ2File",
    "BZ2Compressor",
    "BZ2Decompressor",
]
