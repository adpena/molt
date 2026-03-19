"""Fully intrinsic-backed `lzma` module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- One-shot compress / decompress ---
_MOLT_LZMA_COMPRESS = _require_intrinsic("molt_lzma_compress")
_MOLT_LZMA_DECOMPRESS = _require_intrinsic("molt_lzma_decompress")

# --- Format constants ---
_MOLT_LZMA_FORMAT_XZ = _require_intrinsic("molt_lzma_format_xz")
_MOLT_LZMA_FORMAT_ALONE = _require_intrinsic("molt_lzma_format_alone")
_MOLT_LZMA_FORMAT_RAW = _require_intrinsic("molt_lzma_format_raw")
_MOLT_LZMA_FORMAT_AUTO = _require_intrinsic("molt_lzma_format_auto")

FORMAT_XZ: int = int(_MOLT_LZMA_FORMAT_XZ())
FORMAT_ALONE: int = int(_MOLT_LZMA_FORMAT_ALONE())
FORMAT_RAW: int = int(_MOLT_LZMA_FORMAT_RAW())
FORMAT_AUTO: int = int(_MOLT_LZMA_FORMAT_AUTO())

# --- Check constants ---
_MOLT_LZMA_CHECK_NONE = _require_intrinsic("molt_lzma_check_none")
_MOLT_LZMA_CHECK_CRC32 = _require_intrinsic("molt_lzma_check_crc32")
_MOLT_LZMA_CHECK_CRC64 = _require_intrinsic("molt_lzma_check_crc64")
_MOLT_LZMA_CHECK_SHA256 = _require_intrinsic("molt_lzma_check_sha256")

CHECK_NONE: int = int(_MOLT_LZMA_CHECK_NONE())
CHECK_CRC32: int = int(_MOLT_LZMA_CHECK_CRC32())
CHECK_CRC64: int = int(_MOLT_LZMA_CHECK_CRC64())
CHECK_SHA256: int = int(_MOLT_LZMA_CHECK_SHA256())

# --- Preset constants ---
_MOLT_LZMA_PRESET_DEFAULT = _require_intrinsic("molt_lzma_preset_default")
_MOLT_LZMA_PRESET_EXTREME = _require_intrinsic("molt_lzma_preset_extreme")

PRESET_DEFAULT: int = int(_MOLT_LZMA_PRESET_DEFAULT())
PRESET_EXTREME: int = int(_MOLT_LZMA_PRESET_EXTREME())

# --- Incremental compressor ---
_MOLT_LZMA_COMPRESSOR_NEW = _require_intrinsic("molt_lzma_compressor_new")
_MOLT_LZMA_COMPRESSOR_COMPRESS = _require_intrinsic(
    "molt_lzma_compressor_compress")
_MOLT_LZMA_COMPRESSOR_FLUSH = _require_intrinsic(
    "molt_lzma_compressor_flush")
_MOLT_LZMA_COMPRESSOR_DROP = _require_intrinsic("molt_lzma_compressor_drop")

# --- Incremental decompressor ---
_MOLT_LZMA_DECOMPRESSOR_NEW = _require_intrinsic(
    "molt_lzma_decompressor_new")
_MOLT_LZMA_DECOMPRESSOR_DECOMPRESS = _require_intrinsic(
    "molt_lzma_decompressor_decompress")
_MOLT_LZMA_DECOMPRESSOR_EOF = _require_intrinsic(
    "molt_lzma_decompressor_eof")
_MOLT_LZMA_DECOMPRESSOR_NEEDS_INPUT = _require_intrinsic(
    "molt_lzma_decompressor_needs_input")
_MOLT_LZMA_DECOMPRESSOR_UNUSED_DATA = _require_intrinsic(
    "molt_lzma_decompressor_unused_data")
_MOLT_LZMA_DECOMPRESSOR_DROP = _require_intrinsic(
    "molt_lzma_decompressor_drop")

# --- File handle intrinsics ---
_MOLT_LZMA_FILE_OPEN = _require_intrinsic("molt_lzma_file_open")
_MOLT_LZMA_FILE_READ = _require_intrinsic("molt_lzma_file_read")
_MOLT_LZMA_FILE_WRITE = _require_intrinsic("molt_lzma_file_write")
_MOLT_LZMA_FILE_CLOSE = _require_intrinsic("molt_lzma_file_close")
_MOLT_LZMA_FILE_DROP = _require_intrinsic("molt_lzma_file_drop")


class LZMAError(Exception):
    """Exception raised for LZMA-related errors."""

    pass


class LZMACompressor:
    """Incremental LZMA compressor backed by Rust intrinsics."""

    def __init__(
        self,
        format: int = FORMAT_XZ,
        check: int = -1,
        preset: int | None = None,
        filters=None,
    ) -> None:
        if check == -1:
            check = CHECK_CRC64 if format == FORMAT_XZ else CHECK_NONE
        if preset is None:
            preset = PRESET_DEFAULT
        self._handle = _MOLT_LZMA_COMPRESSOR_NEW(format, check, preset)
        self._flushed = False

    def compress(self, data: bytes) -> bytes:
        if self._flushed:
            raise ValueError("Compressor has been flushed")
        return bytes(_MOLT_LZMA_COMPRESSOR_COMPRESS(self._handle, data))

    def flush(self) -> bytes:
        if self._flushed:
            raise ValueError("Repeated call to flush()")
        self._flushed = True
        return bytes(_MOLT_LZMA_COMPRESSOR_FLUSH(self._handle))

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_LZMA_COMPRESSOR_DROP(handle)
            except Exception:
                pass


class LZMADecompressor:
    """Incremental LZMA decompressor backed by Rust intrinsics."""

    def __init__(
        self,
        format: int = FORMAT_AUTO,
        memlimit: int | None = None,
        filters=None,
    ) -> None:
        effective_memlimit = memlimit if memlimit is not None else 0
        self._handle = _MOLT_LZMA_DECOMPRESSOR_NEW(format, effective_memlimit)

    def decompress(self, data: bytes, max_length: int = -1) -> bytes:
        if self.eof:
            raise EOFError("Already at end of stream")
        return bytes(_MOLT_LZMA_DECOMPRESSOR_DECOMPRESS(self._handle, data, max_length))

    @property
    def eof(self) -> bool:
        return bool(_MOLT_LZMA_DECOMPRESSOR_EOF(self._handle))

    @property
    def needs_input(self) -> bool:
        return bool(_MOLT_LZMA_DECOMPRESSOR_NEEDS_INPUT(self._handle))

    @property
    def unused_data(self) -> bytes:
        return bytes(_MOLT_LZMA_DECOMPRESSOR_UNUSED_DATA(self._handle))

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_LZMA_DECOMPRESSOR_DROP(handle)
            except Exception:
                pass


class LZMAFile:
    """LZMA file object backed entirely by Rust intrinsics."""

    def __init__(
        self,
        filename: str,
        mode: str = "rb",
        *,
        format: int = FORMAT_XZ,
        check: int = -1,
        preset: int | None = None,
    ) -> None:
        if mode not in ("rb", "wb", "ab", "r", "w", "a"):
            raise ValueError(f"Invalid mode: {mode!r}")
        if check == -1:
            check = CHECK_CRC64 if format == FORMAT_XZ else CHECK_NONE
        if preset is None:
            preset = PRESET_DEFAULT
        self._handle = _MOLT_LZMA_FILE_OPEN(filename, mode, format, check, preset)
        self._mode = mode
        self._closed = False
        self._writing = "w" in mode or "a" in mode
        self._reading = "r" in mode

    def write(self, data: bytes) -> int:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if not self._writing:
            raise OSError("File not open for writing")
        return int(_MOLT_LZMA_FILE_WRITE(self._handle, data))

    def read(self, size: int = -1) -> bytes:
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if not self._reading:
            raise OSError("File not open for reading")
        return bytes(_MOLT_LZMA_FILE_READ(self._handle, size))

    def readable(self) -> bool:
        return self._reading

    def writable(self) -> bool:
        return self._writing

    def seekable(self) -> bool:
        return False

    def close(self) -> None:
        if self._closed:
            return
        _MOLT_LZMA_FILE_CLOSE(self._handle)
        _MOLT_LZMA_FILE_DROP(self._handle)
        self._handle = None
        self._closed = True

    @property
    def closed(self) -> bool:
        return self._closed

    def __enter__(self) -> "LZMAFile":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass


def compress(
    data: bytes,
    format: int = FORMAT_XZ,
    check: int = -1,
    preset: int | None = None,
    filters=None,
) -> bytes:
    """Compress *data* in one shot, returning the compressed bytes."""
    if check == -1:
        check = CHECK_CRC64 if format == FORMAT_XZ else CHECK_NONE
    if preset is None:
        preset = PRESET_DEFAULT
    try:
        return bytes(_MOLT_LZMA_COMPRESS(data, format, check, preset))
    except Exception as exc:
        raise LZMAError(str(exc)) from exc


def decompress(
    data: bytes,
    format: int = FORMAT_AUTO,
    memlimit: int | None = None,
    filters=None,
) -> bytes:
    """Decompress *data* in one shot, returning the decompressed bytes."""
    effective_memlimit = memlimit if memlimit is not None else 0
    try:
        return bytes(_MOLT_LZMA_DECOMPRESS(data, format, effective_memlimit))
    except Exception as exc:
        raise LZMAError(str(exc)) from exc


def open(
    filename: str,
    mode: str = "rb",
    *,
    format: int | None = None,
    check: int = -1,
    preset: int | None = None,
    filters=None,
    encoding: str | None = None,
    errors: str | None = None,
    newline: str | None = None,
) -> LZMAFile:
    """Open an LZMA-compressed file in binary mode."""
    if format is None:
        format = FORMAT_XZ
    return LZMAFile(filename, mode, format=format, check=check, preset=preset)


__all__ = [
    "compress",
    "decompress",
    "open",
    "LZMAFile",
    "LZMACompressor",
    "LZMADecompressor",
    "LZMAError",
    "FORMAT_XZ",
    "FORMAT_ALONE",
    "FORMAT_RAW",
    "FORMAT_AUTO",
    "CHECK_NONE",
    "CHECK_CRC32",
    "CHECK_CRC64",
    "CHECK_SHA256",
    "PRESET_DEFAULT",
    "PRESET_EXTREME",
]
